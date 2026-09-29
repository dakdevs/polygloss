//! `CoreDiffProvider`: an opened diff from `polygloss-core` behind the
//! viewport's [`DiffProvider`] trait (the viewport never touches git or the
//! store, plan "Dependency direction").

use std::sync::Arc;

use polygloss_core::objects::{BlobReader, ObjectError};
use polygloss_core::review::OpenedDiff;
use polygloss_diff::{FileChange, ObjectFormat, Oid};
use polygloss_viewport::DiffProvider;

/// The files of one [`OpenedDiff`] and in-process blob reads through a
/// [`BlobReader`] (gix, never a fetch). Cheap to share: the file list is the
/// opened diff's `Arc` and the reader is `Clone + Send + Sync`.
#[derive(Clone)]
pub struct CoreDiffProvider {
    object_format: ObjectFormat,
    files: Arc<Vec<FileChange>>,
    blobs: BlobReader,
}

impl CoreDiffProvider {
    /// A provider over `opened`'s files that reads blobs from `blobs`. For a
    /// live diff, `blobs` must see the snapshot's scratch store (see
    /// [`CoreDiffProvider::open`]).
    pub fn new(opened: &OpenedDiff, blobs: BlobReader) -> CoreDiffProvider {
        CoreDiffProvider {
            object_format: opened.repo.object_format,
            files: opened.files.clone(),
            blobs,
        }
    }

    /// Opens the repo's object store for `opened`, plus the scratch store of
    /// its live snapshot, whose working-tree blobs are not in the repo.
    pub fn open(opened: &OpenedDiff) -> Result<CoreDiffProvider, ObjectError> {
        let repo_blobs = BlobReader::open(&opened.repo)?;
        let blobs = match &opened.live {
            Some(live) => repo_blobs.with_scratch(&live.scratch_objects)?,
            None => repo_blobs,
        };
        Ok(CoreDiffProvider::new(opened, blobs))
    }
}

impl DiffProvider for CoreDiffProvider {
    fn object_format(&self) -> ObjectFormat {
        self.object_format
    }

    fn files(&self) -> Arc<Vec<FileChange>> {
        self.files.clone()
    }

    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        Ok(self.blobs.read(oid)?)
    }

    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64> {
        Ok(self.blobs.size(oid)?)
    }
}
