//! In-process blob reads with gix, including the scratch store as an alternate (T1.4).
//!
//! Design §6.1 (ADR-0014): git produces structure, gix reads blob contents in process,
//! so no subprocess runs per blob. `BlobReader` opens the repo's object store
//! (`<common_dir>/objects`, shared by every linked worktree) directly with gix-odb:
//!
//! - **No replace objects.** The store is created without `refs/replace/*`
//!   replacements and every handle also sets `ignore_replacements`, so reads return
//!   the object with the requested id, like the runner's `GIT_NO_REPLACE_OBJECTS=1`
//!   (Global constraints "Offline"). Opening a full `gix::Repository` would read
//!   replacements according to repo config, and gix 0.88 reads `core.useReplaceRefs`
//!   in the inverted `GIT_NO_REPLACE_OBJECTS` sense.
//! - **No config, refs or environment.** The object format comes from `RepoInfo`
//!   (git's own answer), so nothing else about the repo can make reads fail.
//! - **Never fetches.** gix has no lazy fetch: an object a partial clone lacks is
//!   `ObjectError::Missing` (MCP `objects_missing`, ADR-0005).
//!
//! A live (unpinned) state's objects live in a scratch store (design §5.1).
//! `with_scratch` writes the scratch store's `info/alternates` so it points at the
//! repo's objects dir; one handle then reads scratch and repo objects alike, and so
//! does git given `GIT_OBJECT_DIRECTORY=<scratch>`.
//!
//! `BlobReader` is `Clone + Send + Sync`. gix handles are not `Sync`, so the reader
//! keeps a small pool of handles (each with its own pack delta cache) and lends one
//! to each read.

use std::ffi::OsStr;
use std::io::Write as _;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use gix::objs::{Exists as _, Find as _};
use gix::odb::Header as _;
use polygloss_diff::{ObjectFormat, Oid};

use crate::git::RepoInfo;

/// git's binary heuristic looks for a NUL byte in this many leading bytes
/// (`FIRST_FEW_BYTES` in git's `xdiff-interface.c`).
pub const BINARY_SNIFF_LEN: usize = 8_000;

/// Idle handles kept per reader; more concurrent readers than this just open (and
/// then drop) extra handles.
const MAX_POOLED_HANDLES: usize = 32;

/// Returns `true` when `bytes` contains a NUL in its first 8,000 bytes (git's rule).
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_LEN)].contains(&0)
}

/// Why a read failed.
#[derive(Debug, thiserror::Error)]
pub enum ObjectError {
    /// The object is not in the store (or its alternates). In a shallow or partial
    /// clone this is expected; it is never fetched. Maps to MCP `objects_missing`.
    #[error("object {0} is not available in this repository")]
    Missing(Oid),
    /// The store is damaged, or the object exists but is not a blob.
    #[error("object store: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

type OdbHandle = gix::odb::Cache<gix::odb::store::Handle<Arc<gix::odb::Store>>>;

/// Reads blobs from one repo's object store, optionally through a scratch store
/// that has the repo's objects as an alternate. Cheap to clone; share it freely
/// across threads.
#[derive(Clone)]
pub struct BlobReader {
    inner: Arc<Inner>,
}

struct Inner {
    store: Arc<gix::odb::Store>,
    /// The directory this reader's store was opened at (the repo's objects dir or
    /// a scratch store).
    objects_dir: PathBuf,
    /// The repo's own objects dir, which scratch stores point at.
    repo_objects: PathBuf,
    format: ObjectFormat,
    handles: Mutex<Vec<OdbHandle>>,
}

impl std::fmt::Debug for BlobReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BlobReader")
            .field("objects_dir", &self.inner.objects_dir)
            .field("repo_objects", &self.inner.repo_objects)
            .field("format", &self.inner.format)
            .finish_non_exhaustive()
    }
}

impl BlobReader {
    /// Opens the object store of `repo` (`<common_dir>/objects`). Reads no config,
    /// refs or environment, and never replaces objects.
    pub fn open(repo: &RepoInfo) -> Result<BlobReader, ObjectError> {
        let objects = repo.common_dir.join("objects");
        BlobReader::at(objects.clone(), objects, repo.object_format)
    }

    /// A reader for a scratch store (design §5.1) whose `info/alternates` points at
    /// this reader's repo objects dir, written here if missing or different. The
    /// scratch store must already exist; it is never created. `self` is unchanged.
    pub fn with_scratch(&self, scratch_objects: &Path) -> Result<BlobReader, ObjectError> {
        if !std::fs::metadata(scratch_objects)?.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!(
                    "scratch store {} is not a directory",
                    scratch_objects.display()
                ),
            )
            .into());
        }
        ensure_alternates(scratch_objects, &self.inner.repo_objects)?;
        BlobReader::at(
            scratch_objects.to_path_buf(),
            self.inner.repo_objects.clone(),
            self.inner.format,
        )
    }

    /// The blob's contents. A missing object is `Missing`; a non-blob is `Corrupt`.
    pub fn read(&self, oid: &Oid) -> Result<Arc<[u8]>, ObjectError> {
        let Some(id) = self.object_id(oid) else {
            return Err(ObjectError::Missing(oid.clone()));
        };
        self.with_handle(|handle| {
            let mut buf = Vec::new();
            match handle.try_find(&id, &mut buf) {
                Ok(Some(data)) if data.kind == gix::objs::Kind::Blob => Ok(Arc::from(data.data)),
                Ok(Some(data)) => Err(not_a_blob(oid, data.kind)),
                Ok(None) => Err(ObjectError::Missing(oid.clone())),
                Err(err) => Err(ObjectError::Corrupt(format!(
                    "reading {oid}: {}",
                    err.into_error()
                ))),
            }
        })
    }

    /// Whether any object (of any kind) with this id is in the store.
    pub fn exists(&self, oid: &Oid) -> bool {
        self.object_id(oid)
            .is_some_and(|id| self.with_handle(|handle| handle.exists(&id)))
    }

    /// The blob's size in bytes, from the object header (the contents are not
    /// inflated for loose objects or undeltified pack entries).
    pub fn size(&self, oid: &Oid) -> Result<u64, ObjectError> {
        let Some(id) = self.object_id(oid) else {
            return Err(ObjectError::Missing(oid.clone()));
        };
        self.with_handle(|handle| match handle.try_header(&id) {
            Ok(Some(header)) if header.kind() == gix::objs::Kind::Blob => Ok(header.size()),
            Ok(Some(header)) => Err(not_a_blob(oid, header.kind())),
            Ok(None) => Err(ObjectError::Missing(oid.clone())),
            Err(err) => Err(ObjectError::Corrupt(format!(
                "reading the header of {oid}: {}",
                err.into_error()
            ))),
        })
    }

    fn at(
        objects_dir: PathBuf,
        repo_objects: PathBuf,
        format: ObjectFormat,
    ) -> Result<BlobReader, ObjectError> {
        if !std::fs::metadata(&objects_dir)?.is_dir() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotADirectory,
                format!("{} is not an objects directory", objects_dir.display()),
            )
            .into());
        }
        let store = gix::odb::Store::at_opts(
            objects_dir.clone(),
            hash_kind(format),
            &mut std::iter::empty(),
            gix::odb::store::init::Options::default(),
        )?;
        Ok(BlobReader {
            inner: Arc::new(Inner {
                store: Arc::new(store),
                objects_dir,
                repo_objects,
                format,
                handles: Mutex::new(Vec::new()),
            }),
        })
    }

    /// `None` for ids that cannot be in this store: the null id or another format.
    fn object_id(&self, oid: &Oid) -> Option<gix::ObjectId> {
        if oid.is_zero() || oid.object_format() != self.inner.format {
            return None;
        }
        gix::ObjectId::from_hex(oid.as_str().as_bytes()).ok()
    }

    fn with_handle<T>(&self, f: impl FnOnce(&OdbHandle) -> T) -> T {
        let pooled = self
            .inner
            .handles
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop();
        let handle = pooled.unwrap_or_else(|| self.new_handle());
        let out = f(&handle);
        let mut pool = self
            .inner
            .handles
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if pool.len() < MAX_POOLED_HANDLES {
            pool.push(handle);
        }
        out
    }

    fn new_handle(&self) -> OdbHandle {
        let mut handle = self.inner.store.to_cache_arc();
        handle.ignore_replacements = true;
        handle
            .set_pack_cache(|| Box::<gix::odb::pack::cache::lru::StaticLinkedList<64>>::default());
        handle
    }
}

fn hash_kind(format: ObjectFormat) -> gix::hash::Kind {
    match format {
        ObjectFormat::Sha1 => gix::hash::Kind::Sha1,
        ObjectFormat::Sha256 => gix::hash::Kind::Sha256,
    }
}

fn not_a_blob(oid: &Oid, kind: gix::objs::Kind) -> ObjectError {
    ObjectError::Corrupt(format!("object {oid} is a {kind}, not a blob"))
}

/// Makes `<scratch>/info/alternates` exactly one entry: `repo_objects`. Written to
/// a temp file and renamed, so concurrent snapshotters of one repo (which write the
/// same content) never expose a torn file to git or gix.
pub(crate) fn ensure_alternates(
    scratch_objects: &Path,
    repo_objects: &Path,
) -> std::io::Result<()> {
    let info = scratch_objects.join("info");
    let path = info.join("alternates");
    let mut want = alternates_entry(repo_objects.as_os_str());
    want.push(b'\n');
    match std::fs::read(&path) {
        Ok(have) if have == want => return Ok(()),
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    std::fs::create_dir_all(&info)?;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let tmp = info.join(format!(
        "alternates.tmp-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    let written = std::fs::File::create(&tmp).and_then(|mut file| {
        file.write_all(&want)?;
        file.sync_all()
    });
    match written.and_then(|()| std::fs::rename(&tmp, &path)) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(err)
        }
    }
}

/// One `info/alternates` line. Plain unless the path has a control character or
/// starts with `"`; then git's C-style quoting, which both git and gix unquote, so
/// repo paths containing newlines still work.
fn alternates_entry(path: &OsStr) -> Vec<u8> {
    let bytes = path.as_bytes();
    let needs_quoting =
        bytes.first() == Some(&b'"') || bytes.iter().any(|&b| b < 0x20 || b == 0x7f);
    if !needs_quoting {
        return bytes.to_vec();
    }
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'"');
    for &b in bytes {
        match b {
            b'"' => out.extend_from_slice(b"\\\""),
            b'\\' => out.extend_from_slice(b"\\\\"),
            b'\n' => out.extend_from_slice(b"\\n"),
            b'\t' => out.extend_from_slice(b"\\t"),
            b if b < 0x20 || b == 0x7f => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            b => out.push(b),
        }
    }
    out.push(b'"');
    out
}

#[cfg(test)]
mod tests {
    use super::alternates_entry;
    use std::ffi::OsStr;

    #[test]
    fn alternates_entry_plain_path_is_verbatim() {
        assert_eq!(
            alternates_entry(OsStr::new("/repo with space/.git/objects")),
            b"/repo with space/.git/objects"
        );
    }

    #[test]
    fn alternates_entry_quotes_control_characters() {
        assert_eq!(
            alternates_entry(OsStr::new("/a\"b\nc\\d\te\x01/objects")),
            b"\"/a\\\"b\\nc\\\\d\\te\\001/objects\""
        );
    }
}
