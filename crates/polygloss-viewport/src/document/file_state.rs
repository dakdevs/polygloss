//! Per-file lifecycle and height source (design §12.4 "File states").

use std::sync::Arc;

use super::file_layout::FileLayout;
use super::metrics::SizeHint;
use super::placement::PlacedBlock;
use crate::materialize::MaterializedFile;

/// Where a file's data is: `Estimated` (metadata only) → `Loading` →
/// `Materialized` (rows, word ranges, tokens) → `Evicted` (data dropped by the
/// LRU, exact height kept). `Failed` holds the error to show in the body.
#[derive(Debug, Clone)]
pub enum FileState {
    Estimated,
    /// Background work is running; results from any other generation are stale.
    Loading {
        generation: u64,
    },
    Materialized(Arc<MaterializedFile>),
    Evicted,
    Failed(String),
}

impl FileState {
    pub fn is_materialized(&self) -> bool {
        matches!(self, FileState::Materialized(_))
    }
}

/// How a file's body height is known.
#[derive(Debug, Clone)]
pub(crate) enum Body {
    /// From [`super::metrics::Metrics::estimate_body`].
    Estimated(f32),
    /// Set exactly ([`super::Document::set_file_height`], or kept on eviction)
    /// but without rows.
    Explicit(f32),
    /// Exact rows.
    Laid(FileLayout),
}

impl Body {
    pub(crate) fn height(&self) -> f64 {
        match self {
            Body::Estimated(h) | Body::Explicit(h) => f64::from(*h),
            Body::Laid(layout) => layout.height(),
        }
    }
}

/// One file of the document: metadata lives in the shared `FileChange` list,
/// this is the viewport's state for it.
#[derive(Debug, Clone)]
pub(crate) struct FileEntry {
    pub(crate) state: FileState,
    /// Bumped by every load, eviction and cancellation; results carrying an
    /// older generation are dropped.
    pub(crate) generation: u64,
    pub(crate) collapsed: bool,
    pub(crate) hint: Option<SizeHint>,
    pub(crate) body: Body,
    /// Host blocks, in the host's order. A layout has them as rows and an
    /// explicit body includes them; an estimate adds them.
    pub(crate) blocks: Vec<PlacedBlock>,
}

impl FileEntry {
    /// The file's height: the header, plus the body unless collapsed. The body
    /// is rounded to `f32` on its own, so keeping a layout's height as an
    /// explicit one (eviction) yields exactly the same file height.
    pub(crate) fn height(&self, header: f32) -> f32 {
        if self.collapsed {
            header
        } else if let Body::Estimated(body) = self.body {
            header + body + self.blocks_height()
        } else {
            header + self.body.height() as f32
        }
    }

    /// Total height of the file's blocks.
    pub(crate) fn blocks_height(&self) -> f32 {
        self.blocks.iter().map(|b| b.height).sum()
    }

    pub(crate) fn layout(&self) -> Option<&FileLayout> {
        match &self.body {
            Body::Laid(layout) => Some(layout),
            Body::Estimated(_) | Body::Explicit(_) => None,
        }
    }

    /// Heap bytes held while materialized: the data, its rows and its layout.
    pub(crate) fn resident_bytes(&self) -> usize {
        match &self.state {
            FileState::Materialized(file) => {
                file.resident_bytes() + self.layout().map_or(0, FileLayout::heap_bytes)
            }
            _ => 0,
        }
    }
}
