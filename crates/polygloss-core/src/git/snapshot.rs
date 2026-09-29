//! Worktree snapshots in a scratch store, pinning and pruning (T1.5, design §5, ADR-0008).
//!
//! A snapshot turns a working tree into a tree OID exactly as `git add -A && git
//! write-tree` would, without touching the user's index, HEAD, refs or object store:
//!
//! 1. The worktree's index (`<git_dir>/index`) is copied, with its mtime, to a
//!    per-worktree scratch index outside the worktree, so git's stat cache is reused
//!    and only changed files are hashed. An `index.lock` next to the user's index
//!    (before or after the copy) is `SnapshotError::IndexLocked`: retry on the next
//!    debounce (§5.3).
//! 2. `git add -A` and `git write-tree` run with `GIT_INDEX_FILE=<scratch index>` and
//!    `GIT_OBJECT_DIRECTORY=<scratch objects>`, so new blobs and trees land only in
//!    the scratch store. The scratch store reaches the repo's objects through its
//!    `info/alternates` file (written by the same helper `BlobReader::with_scratch`
//!    uses), which, unlike the colon-separated `GIT_ALTERNATE_OBJECT_DIRECTORIES`,
//!    handles any repo path. `core.splitIndex=false` is forced so a split user index
//!    never makes git write a new `sharedindex.*` into the user's git dir.
//! 3. `diff_env` gives any later git call (`diff-tree`, §6) the same object view.
//!
//! Pinning (§5.1) copies every object reachable from the tree that the repo lacks
//! into the repo's object store (as loose objects, written through gix, which also
//! verifies each id) and then points `refs/polygloss/snapshots/<tree>` at the tree.
//! Objects missing from both stores (a partial clone's promisor objects) are skipped:
//! they were missing before the snapshot too, and nothing is ever fetched.
//!
//! Layout under the scratch root (`DataPaths.scratch_dir`, design §13.2):
//!
//! ```text
//! <scratch_root>/<sha256(common_dir)[..16]>/        0700, one per repo
//!   objects/                                       shared by every worktree of the repo
//!   lock                                           shared: snapshot, pin; exclusive: prune
//!   <sha256(worktree)[..16]>/{index, states, lock} one per worktree
//! ```
//!
//! `states` lists this worktree's recent snapshot trees, oldest first; `prune_scratch`
//! trims it and deletes every scratch object that no listed state of any worktree
//! of the repo reaches. The locks (`File::lock`) make snapshots, pins and prunes
//! safe across threads and processes (app, CLI, MCP).

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gix::objs::{Exists as _, Find as _, Write as _};
use polygloss_diff::{ObjectFormat, Oid};
use sha2::{Digest, Sha256};

use crate::git::repo::{RepoInfo, discover};
use crate::git::runner::{Git, GitError};
use crate::objects::ensure_alternates;

/// The ref namespace pinned snapshots live in (design §5.1).
pub const SNAPSHOT_REF_PREFIX: &str = "refs/polygloss/snapshots/";

/// Recent states remembered per worktree even if `prune_scratch` is never called.
const MAX_RECORDED_STATES: usize = 64;

/// `refs/polygloss/snapshots/<tree>`: the ref that keeps a pinned tree alive.
pub fn snapshot_ref(tree: &Oid) -> String {
    format!("{SNAPSHOT_REF_PREFIX}{tree}")
}

/// Takes snapshots into, and prunes, the scratch stores under one root.
#[derive(Debug, Clone)]
pub struct Snapshotter {
    scratch_root: PathBuf,
}

/// An unpinned live state: the worktree's content as a tree whose new objects live
/// in `scratch_objects` (which has the repo's objects as an alternate).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LiveState {
    pub head_tree: Oid,
    /// The repo's shared scratch object store (`…/<repo-hash>/objects`).
    pub scratch_objects: PathBuf,
    /// The worktree root (canonical).
    pub worktree: PathBuf,
}

/// Why a snapshot, pin or prune failed.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    /// The user's `index.lock` exists (git is writing the index); retry later.
    #[error("the git index is locked (index.lock exists); retry after the next change")]
    IndexLocked,
    #[error(transparent)]
    Git(#[from] GitError),
    /// Filesystem errors, and damaged or unreadable scratch objects (`InvalidData`).
    #[error("snapshot I/O: {0}")]
    Io(#[from] io::Error),
}

type OdbHandle = gix::odb::Cache<gix::odb::store::Handle<Arc<gix::odb::Store>>>;

impl Snapshotter {
    /// A snapshotter whose scratch stores live under `scratch_root`
    /// (`DataPaths.scratch_dir`). Nothing is created until the first snapshot.
    pub fn new(scratch_root: PathBuf) -> Snapshotter {
        Snapshotter { scratch_root }
    }

    /// The root passed to `new`.
    pub fn scratch_root(&self) -> &Path {
        &self.scratch_root
    }

    /// Snapshots `worktree` (any path inside a worktree of `repo`) into the scratch
    /// store (design §5.1 steps 1–4). Never writes to the user's git dir.
    pub fn snapshot(&self, repo: &RepoInfo, worktree: &Path) -> Result<LiveState, SnapshotError> {
        let (toplevel, git_dir) = worktree_dirs(repo, worktree)?;
        let repo_dir = self.repo_dir(repo);
        let objects = repo_dir.join("objects");
        let wt_dir = repo_dir.join(path_hash(&toplevel));
        create_private_dir(&objects)?;
        create_private_dir(&wt_dir)?;
        ensure_alternates(&objects, &repo.common_dir.join("objects"))?;

        // Worktree lock first, then the shared repo lock (prune takes only the latter).
        let _wt_lock = lock(&wt_dir.join("lock"), true)?;
        let _repo_lock = lock(&repo_dir.join("lock"), false)?;

        let user_index = git_dir.join("index");
        let user_lock = git_dir.join("index.lock");
        let scratch_index = wt_dir.join("index");
        if user_lock.exists() {
            return Err(SnapshotError::IndexLocked);
        }
        // We hold the worktree lock, so a scratch index.lock is a crashed run's leftover.
        remove_if_exists(&wt_dir.join("index.lock"))?;
        copy_index(&user_index, &scratch_index)?;
        if user_lock.exists() {
            return Err(SnapshotError::IndexLocked);
        }

        let git = Git::new(&toplevel)
            .with_env("GIT_INDEX_FILE", &scratch_index)
            .with_env("GIT_OBJECT_DIRECTORY", &objects)
            .with_env("GIT_CONFIG_COUNT", "1")
            .with_env("GIT_CONFIG_KEY_0", "core.splitIndex")
            .with_env("GIT_CONFIG_VALUE_0", "false");
        git.output(&[OsStr::new("add"), OsStr::new("-A")])?;
        let out = git.output(&[OsStr::new("write-tree")])?;
        let head_tree = parse_oid(&out, repo.object_format)?;

        record_state(&wt_dir.join("states"), &head_tree)?;
        Ok(LiveState {
            head_tree,
            scratch_objects: objects,
            worktree: toplevel,
        })
    }

    /// `git` with the scratch object store, so `diff-tree` and friends see both the
    /// repo's trees and the live state's tree.
    pub fn diff_env(&self, state: &LiveState, git: Git) -> Git {
        git.with_env("GIT_OBJECT_DIRECTORY", &state.scratch_objects)
    }

    /// Makes `state` durable: copies the objects the repo lacks into it and points
    /// `refs/polygloss/snapshots/<tree>` at the tree. Returns the ref name.
    /// Idempotent.
    pub fn pin(&self, repo: &RepoInfo, state: &LiveState) -> Result<String, SnapshotError> {
        let name = snapshot_ref(&state.head_tree);
        if ref_target(repo, &name)?.as_deref() == Some(state.head_tree.as_str()) {
            return Ok(name);
        }
        {
            let repo_dir = self.repo_dir(repo);
            create_private_dir(&repo_dir)?;
            let _repo_lock = lock(&repo_dir.join("lock"), false)?;
            copy_missing_objects(repo, &state.scratch_objects, &state.head_tree)?;
        }
        update_ref(repo, &name, &state.head_tree)?;
        Ok(name)
    }

    /// Points `refs/polygloss/snapshots/<tree>` at a tree the repo already has (a
    /// fixed `since=<commit>` base, §5.3). A tree the repo lacks is an error.
    pub fn pin_tree(&self, repo: &RepoInfo, tree: &Oid) -> Result<String, SnapshotError> {
        let name = snapshot_ref(tree);
        if ref_target(repo, &name)?.as_deref() != Some(tree.as_str()) {
            update_ref(repo, &name, tree)?;
        }
        Ok(name)
    }

    /// Keeps the last `keep_last` states of `worktree` (pass `LiveState.worktree`)
    /// and deletes every scratch object that no remembered state of any worktree of
    /// the repo reaches (design §5.1 step 5).
    pub fn prune_scratch(
        &self,
        repo: &RepoInfo,
        worktree: &Path,
        keep_last: usize,
    ) -> Result<(), SnapshotError> {
        let repo_dir = self.repo_dir(repo);
        if !repo_dir.is_dir() {
            return Ok(());
        }
        let _repo_lock = lock(&repo_dir.join("lock"), true)?;
        let worktree = fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf());
        let states_file = repo_dir.join(path_hash(&worktree)).join("states");
        let mut states = read_states(&states_file)?;
        if states.len() > keep_last {
            states.drain(..states.len() - keep_last);
            write_states(&states_file, &states)?;
        }

        let objects = repo_dir.join("objects");
        if !objects.is_dir() {
            return Ok(());
        }
        let hash = hash_kind(repo.object_format);
        let mut keep_trees = Vec::new();
        for entry in fs::read_dir(&repo_dir)? {
            let file = entry?.path().join("states");
            if file.is_file() {
                keep_trees.extend(read_states(&file)?);
            }
        }
        let scratch = open_handle(&objects, hash)?;
        let mut keep = HashSet::new();
        for tree in keep_trees {
            if let Some(id) = object_id(&tree) {
                reachable(&scratch, id, hash, &mut keep, |_| true)?;
            }
        }
        drop(scratch);
        delete_loose_except(&objects, &keep)?;
        delete_packs_except(&objects.join("pack"), hash, &keep)?;
        Ok(())
    }

    /// Deletes every `refs/polygloss/snapshots/*` ref not in `referenced` (full ref
    /// names) in one transaction and returns the deleted names. Never touches refs
    /// outside that namespace.
    pub fn delete_unreferenced_refs(
        &self,
        repo: &RepoInfo,
        referenced: &HashSet<String>,
    ) -> Result<Vec<String>, SnapshotError> {
        let git = Git::new(&repo.common_dir);
        let out = git.output(&[
            OsStr::new("for-each-ref"),
            OsStr::new("--format=%(refname)"),
            OsStr::new(SNAPSHOT_REF_PREFIX),
        ])?;
        let doomed: Vec<String> = String::from_utf8_lossy(&out)
            .lines()
            .filter(|name| name.starts_with(SNAPSHOT_REF_PREFIX) && !referenced.contains(*name))
            .map(str::to_owned)
            .collect();
        if doomed.is_empty() {
            return Ok(doomed);
        }
        let mut stdin = Vec::new();
        for name in &doomed {
            stdin.extend_from_slice(format!("delete {name}\n").as_bytes());
        }
        git.output_stdin(&[OsStr::new("update-ref"), OsStr::new("--stdin")], &stdin)?;
        Ok(doomed)
    }

    fn repo_dir(&self, repo: &RepoInfo) -> PathBuf {
        self.scratch_root.join(path_hash(&repo.common_dir))
    }
}

/// The worktree root and its git dir for `worktree` (canonical). Reuses `repo` when
/// it was discovered from this worktree, else discovers it and checks it is the
/// same repository.
fn worktree_dirs(repo: &RepoInfo, worktree: &Path) -> Result<(PathBuf, PathBuf), SnapshotError> {
    let canonical = fs::canonicalize(worktree)?;
    if repo.toplevel.as_deref() == Some(canonical.as_path()) {
        return Ok((canonical, repo.git_dir.clone()));
    }
    let found = discover(&canonical)?;
    let Some(toplevel) = found.toplevel else {
        return Err(GitError::NotARepo(canonical).into());
    };
    if found.common_dir != repo.common_dir {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "{} is a worktree of {}, not of {}",
                toplevel.display(),
                found.common_dir.display(),
                repo.common_dir.display()
            ),
        )
        .into());
    }
    Ok((toplevel, found.git_dir))
}

/// First 16 hex chars of sha256(path bytes): a stable, short directory name.
fn path_hash(path: &Path) -> String {
    let digest = Sha256::digest(path.as_os_str().as_bytes());
    hex::encode(digest)[..16].to_owned()
}

fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

/// Opens (creating) `path` and takes an exclusive or shared advisory lock, held
/// until the returned file is dropped.
fn lock(path: &Path, exclusive: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    if exclusive {
        file.lock()?;
    } else {
        file.lock_shared()?;
    }
    Ok(file)
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => Err(err),
        _ => Ok(()),
    }
}

static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn tmp_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(format!(
        ".tmp-{}-{}",
        std::process::id(),
        TMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    dest.with_file_name(name)
}

/// Writes `bytes` to `dest` through a temp file and a rename.
fn write_atomic(dest: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = tmp_path(dest);
    let result = File::create(&tmp)
        .and_then(|mut f| f.write_all(bytes))
        .and_then(|()| fs::rename(&tmp, dest));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Copies the user's index to the scratch index, keeping its mtime so git's racy-
/// entry check behaves as it would on the original. A missing user index (unborn
/// repo) removes the scratch index, so `add -A` starts from nothing.
fn copy_index(user_index: &Path, scratch_index: &Path) -> io::Result<()> {
    let meta = match fs::metadata(user_index) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return remove_if_exists(scratch_index);
        }
        Err(err) => return Err(err),
    };
    let tmp = tmp_path(scratch_index);
    let result = fs::copy(user_index, &tmp)
        .and_then(|_| OpenOptions::new().write(true).open(&tmp))
        .and_then(|f| f.set_modified(meta.modified()?))
        .and_then(|()| fs::rename(&tmp, scratch_index));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn parse_oid(out: &[u8], fmt: ObjectFormat) -> Result<Oid, SnapshotError> {
    let text = String::from_utf8_lossy(out);
    Oid::parse(text.trim_end(), fmt)
        .map_err(|e| GitError::Parse(format!("write-tree printed {text:?}: {e}")).into())
}

fn read_states(path: &Path) -> io::Result<Vec<Oid>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err),
    };
    Ok(text
        .lines()
        .filter_map(|line| {
            let fmt = match line.len() {
                40 => ObjectFormat::Sha1,
                64 => ObjectFormat::Sha256,
                _ => return None,
            };
            Oid::parse(line, fmt).ok()
        })
        .collect())
}

fn write_states(path: &Path, states: &[Oid]) -> io::Result<()> {
    let mut text = String::new();
    for tree in states {
        text.push_str(tree.as_str());
        text.push('\n');
    }
    write_atomic(path, text.as_bytes())
}

/// Moves `tree` to the end (newest) of the worktree's state list.
fn record_state(path: &Path, tree: &Oid) -> io::Result<()> {
    let mut states = read_states(path)?;
    states.retain(|t| t != tree);
    states.push(tree.clone());
    if states.len() > MAX_RECORDED_STATES {
        states.drain(..states.len() - MAX_RECORDED_STATES);
    }
    write_states(path, &states)
}

/// What `name` points at, or `None` when the ref does not exist.
fn ref_target(repo: &RepoInfo, name: &str) -> Result<Option<String>, GitError> {
    let out = Git::new(&repo.common_dir).output(&[
        OsStr::new("for-each-ref"),
        OsStr::new("--format=%(objectname)"),
        OsStr::new(name),
    ])?;
    let text = String::from_utf8_lossy(&out).trim().to_owned();
    Ok((!text.is_empty()).then_some(text))
}

fn update_ref(repo: &RepoInfo, name: &str, tree: &Oid) -> Result<(), GitError> {
    Git::new(&repo.common_dir).output(&[
        OsStr::new("update-ref"),
        OsStr::new("--no-deref"),
        OsStr::new(name),
        OsStr::new(tree.as_str()),
    ])?;
    Ok(())
}

fn hash_kind(format: ObjectFormat) -> gix::hash::Kind {
    match format {
        ObjectFormat::Sha1 => gix::hash::Kind::Sha1,
        ObjectFormat::Sha256 => gix::hash::Kind::Sha256,
    }
}

fn object_id(oid: &Oid) -> Option<gix::ObjectId> {
    gix::ObjectId::from_hex(oid.as_str().as_bytes()).ok()
}

fn invalid(msg: String) -> SnapshotError {
    SnapshotError::Io(io::Error::new(io::ErrorKind::InvalidData, msg))
}

/// A gix handle on the object store at `objects` (plus its alternates), without
/// replace objects, like `BlobReader`.
fn open_handle(objects: &Path, hash: gix::hash::Kind) -> Result<OdbHandle, SnapshotError> {
    let store = gix::odb::Store::at_opts(
        objects.to_path_buf(),
        hash,
        &mut std::iter::empty(),
        gix::odb::store::init::Options::default(),
    )?;
    let mut handle = Arc::new(store).to_cache_arc();
    handle.ignore_replacements = true;
    Ok(handle)
}

/// Adds `root` and every tree and blob it reaches to `out` (gitlinks are skipped:
/// submodule commits live in another repo), descending only into trees for which
/// `descend` is true. Trees that cannot be found are skipped (a partial clone's
/// promisor objects); nothing is fetched.
fn reachable(
    odb: &OdbHandle,
    root: gix::ObjectId,
    hash: gix::hash::Kind,
    out: &mut HashSet<gix::ObjectId>,
    descend: impl Fn(&gix::ObjectId) -> bool,
) -> Result<(), SnapshotError> {
    let mut stack = vec![root];
    let mut buf = Vec::new();
    while let Some(id) = stack.pop() {
        if !out.insert(id) || !descend(&id) {
            continue;
        }
        let data = match odb.try_find(&id, &mut buf) {
            Ok(Some(data)) => data,
            Ok(None) => {
                tracing::debug!(%id, "snapshot walk: tree not available; skipped");
                continue;
            }
            Err(err) => return Err(invalid(format!("reading tree {id}: {}", err.into_error()))),
        };
        if data.kind != gix::objs::Kind::Tree {
            return Err(invalid(format!("{id} is a {}, not a tree", data.kind)));
        }
        for entry in gix::objs::TreeRefIter::from_bytes(data.data, hash) {
            let entry = entry.map_err(|e| invalid(format!("parsing tree {id}: {e}")))?;
            let child = entry.oid.to_owned();
            if entry.mode.is_tree() {
                stack.push(child);
            } else if !entry.mode.is_commit() {
                out.insert(child);
            }
        }
    }
    Ok(())
}

/// Copies every object reachable from `tree` that the repo lacks from the scratch
/// store into the repo's objects dir as loose objects (§5.1 pin step 1).
fn copy_missing_objects(
    repo: &RepoInfo,
    scratch_objects: &Path,
    tree: &Oid,
) -> Result<(), SnapshotError> {
    let hash = hash_kind(repo.object_format);
    let repo_objects = repo.common_dir.join("objects");
    let root = object_id(tree).ok_or_else(|| invalid(format!("bad tree id {tree}")))?;
    let scratch = open_handle(scratch_objects, hash)?;
    let repo_odb = open_handle(&repo_objects, hash)?;
    // A tree the repo already has comes with everything it reaches (git's
    // connectivity invariant), so only trees new in this state are walked.
    let mut ids = HashSet::new();
    reachable(&scratch, root, hash, &mut ids, |tree| {
        !repo_odb.exists(tree)
    })?;
    let loose = gix::odb::loose::Store::at(repo_objects, hash);
    let mut buf = Vec::new();
    for id in ids {
        if repo_odb.exists(&id) {
            continue;
        }
        let data = match scratch.try_find(&id, &mut buf) {
            Ok(Some(data)) => data,
            Ok(None) => {
                tracing::debug!(%id, "pin: object in neither store; skipped");
                continue;
            }
            Err(err) => return Err(invalid(format!("reading {id}: {}", err.into_error()))),
        };
        let written = loose
            .write_buf(data.kind, data.data)
            .map_err(|err| io::Error::other(format!("writing {id}: {}", err.into_error())))?;
        if written != id {
            return Err(invalid(format!("object {id} rehashed to {written}")));
        }
    }
    Ok(())
}

/// Deletes every loose object in `objects` not in `keep`, plus leftover temp files
/// in the fan-out dirs (the caller holds the exclusive repo lock, so no writer runs).
fn delete_loose_except(objects: &Path, keep: &HashSet<gix::ObjectId>) -> io::Result<()> {
    for entry in fs::read_dir(objects)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(prefix) = name.to_str().filter(|n| n.len() == 2 && is_hex(n)) else {
            continue;
        };
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let mut empty = true;
        for file in fs::read_dir(entry.path())? {
            let file = file?;
            let rest = file.file_name();
            let id = rest
                .to_str()
                .filter(|r| is_hex(r))
                .and_then(|r| gix::ObjectId::from_hex(format!("{prefix}{r}").as_bytes()).ok());
            if id.is_some_and(|id| keep.contains(&id)) {
                empty = false;
            } else {
                remove_if_exists(&file.path())?;
            }
        }
        if empty {
            let _ = fs::remove_dir(entry.path());
        }
    }
    Ok(())
}

/// Deletes every pack (all `pack-<name>.*` files) that holds no object in `keep`,
/// and any other file in the pack dir (leftover temp packs).
fn delete_packs_except(
    pack_dir: &Path,
    hash: gix::hash::Kind,
    keep: &HashSet<gix::ObjectId>,
) -> Result<(), SnapshotError> {
    let entries = match fs::read_dir(pack_dir) {
        Ok(entries) => entries.collect::<io::Result<Vec<_>>>()?,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err.into()),
    };
    let mut kept_stems = HashSet::new();
    for entry in &entries {
        let path = entry.path();
        if path.extension() != Some(OsStr::new("idx")) {
            continue;
        }
        let index = gix::odb::pack::index::File::at(&path, hash).map_err(|err| {
            invalid(format!(
                "reading pack index {}: {}",
                path.display(),
                err.into_error()
            ))
        })?;
        if index.iter().any(|e| keep.contains(&e.oid)) {
            kept_stems.insert(stem(&entry.file_name()));
        }
    }
    for entry in &entries {
        if !kept_stems.contains(&stem(&entry.file_name())) {
            remove_if_exists(&entry.path())?;
        }
    }
    Ok(())
}

/// `pack-abc.idx` -> `pack-abc`.
fn stem(name: &OsStr) -> Vec<u8> {
    let bytes = name.as_bytes();
    let end = bytes.iter().position(|&b| b == b'.').unwrap_or(bytes.len());
    bytes[..end].to_vec()
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_hash_is_16_hex_and_stable() {
        let a = path_hash(Path::new("/repo/.git"));
        assert_eq!(a.len(), 16);
        assert!(is_hex(&a));
        assert_eq!(a, path_hash(Path::new("/repo/.git")));
        assert_ne!(a, path_hash(Path::new("/repo2/.git")));
    }

    #[test]
    fn stem_strips_every_extension() {
        assert_eq!(stem(OsStr::new("pack-ab.idx")), b"pack-ab");
        assert_eq!(stem(OsStr::new("tmp_pack_x")), b"tmp_pack_x");
    }

    #[test]
    fn record_state_moves_repeats_to_the_end_and_caps() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("states");
        let oid = |n: u32| Oid::parse(&format!("{n:040x}"), ObjectFormat::Sha1).unwrap();
        record_state(&file, &oid(1)).unwrap();
        record_state(&file, &oid(2)).unwrap();
        record_state(&file, &oid(1)).unwrap();
        assert_eq!(read_states(&file).unwrap(), [oid(2), oid(1)]);
        for n in 10..(10 + MAX_RECORDED_STATES as u32) {
            record_state(&file, &oid(n)).unwrap();
        }
        let states = read_states(&file).unwrap();
        assert_eq!(states.len(), MAX_RECORDED_STATES);
        assert_eq!(states[0], oid(10));
    }
}
