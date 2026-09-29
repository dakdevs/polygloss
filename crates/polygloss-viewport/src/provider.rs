//! `DiffProvider`: where the viewport gets a diff's files and blobs.
//!
//! The viewport never touches git or the store. The app adapts core data to
//! this trait (`CoreDiffProvider`, T2.8); tests and the perf harness use
//! in-memory providers.

use std::sync::Arc;

use polygloss_diff::{FileChange, ObjectFormat, Oid};

/// The files of one diff and read access to their blobs.
pub trait DiffProvider: Send + Sync + 'static {
    /// The repository's object format (sha1 or sha256).
    fn object_format(&self) -> ObjectFormat;

    /// Every file of the diff, in display order (`FileChange::idx` is the
    /// position). Called once when the viewport is created.
    fn files(&self) -> Arc<Vec<FileChange>>;

    /// A blob's bytes. Called on the background executor only, never for the
    /// all-zero id of a missing side.
    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>>;

    /// A blob's size in bytes, for height estimates before it is loaded.
    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64>;
}
