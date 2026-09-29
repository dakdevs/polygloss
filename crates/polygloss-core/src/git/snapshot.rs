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
//!    never makes git write a new `sharedindex.*` into the user's git dir, and
//!    `core.fsmonitor=false` so a configured fsmonitor daemon is never started (it
//!    would create its socket and cookie dir in the user's git dir).
//! 3. `diff_env` gives any later git call (`diff-tree`, §6) the same object view.
//!
//! Pinning (§5.1) copies every object reachable from the tree that the repo lacks
//! into the repo's object store and then points `refs/polygloss/snapshots/<tree>` at
//! the tree. Objects are written by `git hash-object -w` (so `core.sharedRepository`,
//! `core.fsync` and git's own temp-file cleanup apply, and each id is checked),
//! blobs first and then trees children-first: a tree in the repo is always
//! complete, even after a pin that stopped midway. The walk trusts only subtrees
//! equal to the worktree's `HEAD^{tree}` at the same path (a ref keeps those
//! complete); every other tree is walked even when the repo has it, so a partially
//! copied tree is completed rather than referenced. An object missing from both
//! stores is an error, except in a partial clone, where it is a promisor object that
//! was missing before the snapshot too (nothing is ever fetched). Ref updates run
//! with `core.hooksPath=/dev/null`: the user's `reference-transaction` hook never sees
//! Polygloss's private refs.
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

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use gix::objs::{Exists as _, Find as _};
use polygloss_diff::{ObjectFormat, Oid};
use sha2::{Digest, Sha256};

use crate::git::repo::{RepoInfo, discover};
use crate::git::runner::{Git, GitError};
use crate::objects::{c_quoted_line, ensure_alternates};

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

        let git = with_config(
            Git::new(&toplevel)
                .with_env("GIT_INDEX_FILE", &scratch_index)
                .with_env("GIT_OBJECT_DIRECTORY", &objects),
            &[("core.splitIndex", "false"), ("core.fsmonitor", "false")],
        );
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

    /// `git` with `GIT_OBJECT_DIRECTORY` = the scratch object store, whose
    /// `info/alternates` reaches the repo's objects, so `diff-tree` and friends see
    /// both the repo's trees and the live state's tree.
    pub fn diff_env(&self, state: &LiveState, git: Git) -> Git {
        git.with_env("GIT_OBJECT_DIRECTORY", &state.scratch_objects)
    }

    /// Makes `state` durable: copies the objects the repo lacks into it and points
    /// `refs/polygloss/snapshots/<tree>` at the tree. Returns the ref name.
    /// Idempotent. An object missing from both the repo and the scratch store (a
    /// damaged or purged scratch store) is `Io` with kind `InvalidData`, and no ref
    /// is created; in a partial clone such objects are promisor objects and skipped.
    pub fn pin(&self, repo: &RepoInfo, state: &LiveState) -> Result<String, SnapshotError> {
        let name = snapshot_ref(&state.head_tree);
        if ref_target(repo, &name)?.as_deref() == Some(state.head_tree.as_str()) {
            return Ok(name);
        }
        {
            let repo_dir = self.repo_dir(repo);
            create_private_dir(&repo_dir)?;
            let _repo_lock = lock(&repo_dir.join("lock"), false)?;
            let head = head_tree(&state.worktree);
            copy_missing_objects(
                repo,
                &repo_dir,
                &state.scratch_objects,
                &state.head_tree,
                head,
            )?;
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

    /// Keeps the last `keep_last` states of `worktree` (any path inside it; pass
    /// `LiveState.worktree`) and deletes every scratch object that no remembered
    /// state of any worktree of the repo reaches (design §5.1 step 5), plus temp
    /// dirs a crashed pin left behind.
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
        // A removed worktree cannot be resolved; its recorded path is the canonical one.
        let worktree = match worktree_dirs(repo, worktree) {
            Ok((toplevel, _)) => toplevel,
            Err(_) => fs::canonicalize(worktree).unwrap_or_else(|_| worktree.to_path_buf()),
        };
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
            let path = entry?.path();
            let is_pin_tmp = path
                .file_name()
                .is_some_and(|n| n.as_bytes().starts_with(PIN_TMP_PREFIX.as_bytes()));
            if is_pin_tmp {
                // We hold the exclusive lock, so no pin is using it.
                fs::remove_dir_all(&path)?;
                continue;
            }
            let file = path.join("states");
            if file.is_file() {
                keep_trees.extend(read_states(&file)?);
            }
        }
        let scratch = open_handle(&objects, hash)?;
        let mut keep = HashSet::new();
        for tree in keep_trees {
            if let Some(id) = object_id(&tree) {
                reachable(&scratch, id, hash, &mut keep)?;
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
    ///
    /// Not atomic with respect to a concurrent `pin`: a ref created after the caller
    /// built `referenced` is deleted too. Callers must build `referenced` and call
    /// this under a guard that every pin also holds until the row naming its ref is
    /// committed (plan T1.12).
    pub fn delete_unreferenced_refs(
        &self,
        repo: &RepoInfo,
        referenced: &HashSet<String>,
    ) -> Result<Vec<String>, SnapshotError> {
        let git = refs_git(repo);
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

/// Name prefix of a pin's temp dir under the repo's scratch dir.
const PIN_TMP_PREFIX: &str = "pin-tmp-";

/// A private temp dir, removed (with its contents) on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(parent: &Path) -> io::Result<TempDir> {
        let dir = parent.join(format!(
            "{PIN_TMP_PREFIX}{}-{}",
            std::process::id(),
            TMP_SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        create_private_dir(&dir)?;
        Ok(TempDir(dir))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

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

/// Adds `pairs` as per-invocation config (`GIT_CONFIG_COUNT/KEY_n/VALUE_n`), which
/// keeps the subcommand the first argument (the runner's spawn hook counts by it).
fn with_config(mut git: Git, pairs: &[(&str, &str)]) -> Git {
    git = git.with_env("GIT_CONFIG_COUNT", pairs.len().to_string());
    for (i, (key, value)) in pairs.iter().enumerate() {
        git = git
            .with_env(&format!("GIT_CONFIG_KEY_{i}"), key)
            .with_env(&format!("GIT_CONFIG_VALUE_{i}"), value);
    }
    git
}

/// `git` in the common dir with hooks disabled, for updating Polygloss's own refs:
/// the user's `reference-transaction` hook must not see or veto them.
fn refs_git(repo: &RepoInfo) -> Git {
    with_config(
        Git::new(&repo.common_dir),
        &[("core.hooksPath", "/dev/null")],
    )
}

fn update_ref(repo: &RepoInfo, name: &str, tree: &Oid) -> Result<(), GitError> {
    refs_git(repo).output(&[
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
/// submodule commits live in another repo). Trees that cannot be found are skipped
/// (a partial clone's promisor objects); nothing is fetched.
fn reachable(
    odb: &OdbHandle,
    root: gix::ObjectId,
    hash: gix::hash::Kind,
    out: &mut HashSet<gix::ObjectId>,
) -> Result<(), SnapshotError> {
    let mut stack = vec![root];
    let mut buf = Vec::new();
    while let Some(id) = stack.pop() {
        if !out.insert(id) {
            continue;
        }
        let Some(entries) = read_tree(odb, &id, hash, &mut buf)? else {
            tracing::debug!(%id, "snapshot walk: tree not available; skipped");
            continue;
        };
        for entry in entries {
            match entry.kind {
                EntryKind::Tree => stack.push(entry.id),
                EntryKind::Blob => {
                    out.insert(entry.id);
                }
                EntryKind::Gitlink => {}
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntryKind {
    Tree,
    /// Any blob: file, executable or symlink.
    Blob,
    /// A submodule commit, which lives in another repo.
    Gitlink,
}

struct TreeEntry {
    name: Vec<u8>,
    id: gix::ObjectId,
    kind: EntryKind,
}

/// The entries of tree `id`, or `None` when the store does not have it.
fn read_tree(
    odb: &OdbHandle,
    id: &gix::ObjectId,
    hash: gix::hash::Kind,
    buf: &mut Vec<u8>,
) -> Result<Option<Vec<TreeEntry>>, SnapshotError> {
    let data = match odb.try_find(id, buf) {
        Ok(Some(data)) => data,
        Ok(None) => return Ok(None),
        Err(err) => return Err(invalid(format!("reading tree {id}: {}", err.into_error()))),
    };
    if data.kind != gix::objs::Kind::Tree {
        return Err(invalid(format!("{id} is a {}, not a tree", data.kind)));
    }
    gix::objs::TreeRefIter::from_bytes(data.data, hash)
        .map(|entry| {
            let entry = entry.map_err(|e| invalid(format!("parsing tree {id}: {e}")))?;
            let kind = if entry.mode.is_tree() {
                EntryKind::Tree
            } else if entry.mode.is_commit() {
                EntryKind::Gitlink
            } else {
                EntryKind::Blob
            };
            Ok(TreeEntry {
                name: entry.filename.to_vec(),
                id: entry.oid.to_owned(),
                kind,
            })
        })
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// The worktree's `HEAD^{tree}`, or `None` (unborn HEAD, worktree gone, any error:
/// the pin then walks every tree, which is only slower).
fn head_tree(worktree: &Path) -> Option<gix::ObjectId> {
    let out = Git::new(worktree)
        .run(&[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("-q"),
            OsStr::new("HEAD^{tree}"),
        ])
        .ok()?;
    if !out.success() {
        return None;
    }
    gix::ObjectId::from_hex(String::from_utf8_lossy(&out.stdout).trim().as_bytes()).ok()
}

/// Whether the repo is a partial clone (`extensions.partialClone`, or any
/// `remote.<name>.promisor` set to true), whose missing objects are promised by a
/// remote rather than lost.
fn is_promisor_repo(repo: &RepoInfo) -> Result<bool, GitError> {
    let args = [
        OsStr::new("config"),
        OsStr::new("--get-regexp"),
        OsStr::new(r"^(extensions\.partialclone|remote\..*\.promisor)$"),
    ];
    let out = Git::new(&repo.common_dir).run(&args)?;
    match out.code {
        Some(0) => {}
        // No such key.
        Some(1) => return Ok(false),
        code => {
            return Err(GitError::Failed {
                args: "config --get-regexp".into(),
                code,
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
    }
    Ok(String::from_utf8_lossy(&out.stdout).lines().any(|line| {
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        if key == "extensions.partialclone" {
            !value.is_empty()
        } else {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "" | "true" | "yes" | "on" | "1"
            )
        }
    }))
}

/// Decides what an object missing from both stores means: skipped (with the answer
/// cached in `promisor`) in a partial clone, else an `InvalidData` error.
fn allow_missing(
    repo: &RepoInfo,
    promisor: &mut Option<bool>,
    id: &gix::ObjectId,
) -> Result<(), SnapshotError> {
    let is_promisor = match *promisor {
        Some(known) => known,
        None => *promisor.insert(is_promisor_repo(repo)?),
    };
    if is_promisor {
        tracing::debug!(%id, "pin: promisor object in neither store; skipped");
        Ok(())
    } else {
        Err(invalid(format!(
            "object {id} is in neither the repository nor the scratch store"
        )))
    }
}

/// Copies every object reachable from `tree` that the repo lacks from the scratch
/// store into the repo's object store (§5.1 pin step 1): blobs first, then trees
/// children-first, so any tree the repo has is complete even if this stops midway.
///
/// Subtrees equal to the tree at the same path of `head` (the worktree's
/// `HEAD^{tree}`) are trusted: a ref keeps them and everything they reach in the
/// repo. Every other tree is walked, even when the repo already has it: a pin that
/// crashed before this fix, or any other writer, may have left it without children.
fn copy_missing_objects(
    repo: &RepoInfo,
    repo_dir: &Path,
    scratch_objects: &Path,
    tree: &Oid,
    head: Option<gix::ObjectId>,
) -> Result<(), SnapshotError> {
    let hash = hash_kind(repo.object_format);
    let root = object_id(tree).ok_or_else(|| invalid(format!("bad tree id {tree}")))?;
    // The scratch store also sees the repo's objects, through its alternates.
    let scratch = open_handle(scratch_objects, hash)?;
    let (blobs, trees) = plan_copy(repo, &scratch, root, head)?;
    if blobs.is_empty() && trees.is_empty() {
        return Ok(());
    }
    let tmp = TempDir::new(repo_dir)?;
    write_objects(repo, &scratch, &tmp.0, gix::objs::Kind::Blob, &blobs)?;
    write_objects(repo, &scratch, &tmp.0, gix::objs::Kind::Tree, &trees)?;
    Ok(())
}

/// The blobs and trees `copy_missing_objects` must write, trees in post-order
/// (every tree after the subtrees it contains).
fn plan_copy(
    repo: &RepoInfo,
    scratch: &OdbHandle,
    root: gix::ObjectId,
    head: Option<gix::ObjectId>,
) -> Result<(Vec<gix::ObjectId>, Vec<gix::ObjectId>), SnapshotError> {
    enum Step {
        /// Walk a tree; the second id is the `head` tree at the same path, if any.
        Enter(gix::ObjectId, Option<gix::ObjectId>),
        /// Every child of this tree is in the repo (or queued before it).
        Exit(gix::ObjectId),
    }

    let hash = hash_kind(repo.object_format);
    let repo_odb = open_handle(&repo.common_dir.join("objects"), hash)?;
    let mut promisor = None;
    let mut blobs = Vec::new();
    let mut trees = Vec::new();
    let mut seen = HashSet::new();
    let mut buf = Vec::new();
    let mut stack = vec![Step::Enter(root, head)];
    while let Some(step) = stack.pop() {
        let (id, head) = match step {
            Step::Exit(id) => {
                if !repo_odb.exists(&id) {
                    trees.push(id);
                }
                continue;
            }
            Step::Enter(id, head) => (id, head),
        };
        if head == Some(id) || !seen.insert(id) {
            continue;
        }
        let Some(entries) = read_tree(scratch, &id, hash, &mut buf)? else {
            allow_missing(repo, &mut promisor, &id)?;
            continue;
        };
        let head_subtrees: HashMap<Vec<u8>, gix::ObjectId> = head
            .and_then(|h| read_tree(scratch, &h, hash, &mut buf).ok().flatten())
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.kind == EntryKind::Tree)
            .map(|e| (e.name, e.id))
            .collect();
        stack.push(Step::Exit(id));
        for entry in entries {
            match entry.kind {
                EntryKind::Tree => {
                    let head = head_subtrees.get(&entry.name).copied();
                    stack.push(Step::Enter(entry.id, head));
                }
                EntryKind::Blob => {
                    if seen.insert(entry.id) && !repo_odb.exists(&entry.id) {
                        if scratch.exists(&entry.id) {
                            blobs.push(entry.id);
                        } else {
                            allow_missing(repo, &mut promisor, &entry.id)?;
                        }
                    }
                }
                EntryKind::Gitlink => {}
            }
        }
    }
    Ok((blobs, trees))
}

/// Writes `ids` (all of `kind`, read from `scratch`) into the repo with `git
/// hash-object -w --no-filters --stdin-paths` over temp files in `dir`, and checks
/// that git computed the same ids. git writes them in the order given (the caller
/// passes trees children-first).
///
/// Trees get `--literally`: without it hash-object runs a strict fsck on each tree
/// that treats even INFO-level findings as fatal, so a tree holding a symlinked
/// `.gitignore`, `.gitattributes` or `.mailmap` is refused ("refusing to create
/// malformed object") although `git add -A` and `write-tree` accept it. The id
/// check below guards integrity. Blobs keep the default (streamed) path.
fn write_objects(
    repo: &RepoInfo,
    scratch: &OdbHandle,
    dir: &Path,
    kind: gix::objs::Kind,
    ids: &[gix::ObjectId],
) -> Result<(), SnapshotError> {
    if ids.is_empty() {
        return Ok(());
    }
    let mut buf = Vec::new();
    let mut stdin = Vec::new();
    for (i, id) in ids.iter().enumerate() {
        let data = match scratch.try_find(id, &mut buf) {
            Ok(Some(data)) => data,
            Ok(None) => return Err(invalid(format!("{id} vanished from the scratch store"))),
            Err(err) => return Err(invalid(format!("reading {id}: {}", err.into_error()))),
        };
        if data.kind != kind {
            return Err(invalid(format!("{id} is a {}, not a {kind}", data.kind)));
        }
        let path = dir.join(format!("{kind}-{i}"));
        fs::write(&path, data.data)?;
        stdin.extend(c_quoted_line(path.as_os_str()));
        stdin.push(b'\n');
    }
    let kind_name = kind.to_string();
    let mut args = vec![
        OsStr::new("hash-object"),
        OsStr::new("-w"),
        OsStr::new("--no-filters"),
    ];
    if kind == gix::objs::Kind::Tree {
        args.push(OsStr::new("--literally"));
    }
    args.extend([
        OsStr::new("-t"),
        OsStr::new(&kind_name),
        OsStr::new("--stdin-paths"),
    ]);
    let out = Git::new(&repo.common_dir).output_stdin(&args, &stdin)?;
    let text = String::from_utf8_lossy(&out);
    let written: Vec<&str> = text.lines().collect();
    if written.len() != ids.len() {
        return Err(invalid(format!(
            "hash-object wrote {} of {} objects",
            written.len(),
            ids.len()
        )));
    }
    for (id, line) in ids.iter().zip(written) {
        if gix::ObjectId::from_hex(line.as_bytes()).ok() != Some(*id) {
            return Err(invalid(format!("object {id} rehashed to {line}")));
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
    fn pin_plan_writes_trees_children_first() {
        use crate::testing::{FixtureRepo, Sandbox};
        let sb = Sandbox::isolate();
        let repo = FixtureRepo::init(ObjectFormat::Sha1);
        repo.write("top.txt", b"top\n");
        repo.commit("c1");
        repo.write("a/b/c/deep.txt", b"deep\n");
        repo.write("a/b/side.txt", b"side\n");
        repo.write("a/x/y.txt", b"y\n");
        repo.write("top.txt", b"top changed\n");
        let info = crate::git::discover(repo.path()).unwrap();
        let snap = Snapshotter::new(sb.cache_dir().join("scratch"));
        let state = snap.snapshot(&info, repo.path()).unwrap();
        let hash = hash_kind(info.object_format);
        let scratch = open_handle(&state.scratch_objects, hash).unwrap();
        let root = object_id(&state.head_tree).unwrap();
        let head = head_tree(repo.path());
        assert!(head.is_some());

        let (blobs, trees) = plan_copy(&info, &scratch, root, head).unwrap();

        assert_eq!(blobs.len(), 4);
        // root, a, a/b, a/b/c, a/x: every tree after each subtree it contains.
        assert_eq!(trees.len(), 5);
        assert_eq!(trees.last(), Some(&root));
        let mut buf = Vec::new();
        for (i, tree) in trees.iter().enumerate() {
            let entries = read_tree(&scratch, tree, hash, &mut buf).unwrap().unwrap();
            for child in entries.iter().filter(|e| e.kind == EntryKind::Tree) {
                let pos = trees.iter().position(|t| *t == child.id).unwrap();
                assert!(pos < i, "subtree written after its parent");
            }
        }
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
