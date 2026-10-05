//! Geometry the document lays out with, and height estimates for files that
//! are not laid out yet (design §12.4 "Estimated").

use polygloss_diff::rows::Layout;
use polygloss_diff::{FileChange, FileKind};

/// Pixel geometry and estimation parameters. Everything here affects heights,
/// so changing it ([`crate::document::Document::set_metrics`]) re-estimates
/// every file and drops every row layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Metrics {
    /// The effective layout; row counts differ between split and unified.
    pub layout: Layout,
    /// One code row (a line, or a `\ No newline` marker).
    pub row_height: f32,
    /// The file header, including any separator above it. A collapsed card is
    /// exactly this tall.
    pub header_height: f32,
    /// Canvas between file cards: above every card but the first (whose lead
    /// is the prelude and a gap, or nothing), and below the last one. 0 in the
    /// flat layout.
    pub card_gap: f32,
    /// Card padding below the last row of a non-empty body. 0 in the flat
    /// layout.
    pub card_pad_bottom: f32,
    /// A category section's band (design §11.6 "Sections": 36 pt on the
    /// canvas), in the lead of the section's first file.
    pub band_height: f32,
    /// A gap expander row.
    pub gap_height: f32,
    /// A body that is a single message: binary, "Load diff", loading, failed.
    pub placeholder_height: f32,
    /// Context lines around each hunk (design §6.3: 3).
    pub context_lines: u32,
    /// Files with more changed lines show "Load diff" instead of rows
    /// (design §12.3: ~20k).
    pub load_diff_changed_lines: u32,
    /// Average bytes per line, to turn blob sizes into line counts.
    pub bytes_per_line: u32,
    /// Body rows assumed for a text file before anything is known about it.
    pub default_body_rows: u32,
}

impl Default for Metrics {
    fn default() -> Metrics {
        Metrics {
            layout: Layout::Split,
            row_height: 20.0,
            header_height: 40.0,
            card_gap: 0.0,
            card_pad_bottom: 0.0,
            band_height: 36.0,
            gap_height: 32.0,
            placeholder_height: 48.0,
            context_lines: 3,
            load_diff_changed_lines: 20_000,
            bytes_per_line: 32,
            default_body_rows: 20,
        }
    }
}

/// What is known about a file's size before it is materialized, from the
/// cheapest source available. Counts win over blob sizes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeHint {
    /// Added and removed lines (and hunks, when known) from a background diff
    /// (design §6.3 "Counts").
    Counts {
        additions: u32,
        deletions: u32,
        hunks: Option<u32>,
    },
    /// Blob sizes in bytes (`DiffProvider::blob_size`); 0 for a missing side.
    BlobSizes { old: u64, new: u64 },
}

impl Metrics {
    /// Estimated body height (below the header) of `change` in this layout.
    ///
    /// Special bodies are exact: binary and generated files are a placeholder
    /// (design §6.4, §12.3; a file relabeled generated or not,
    /// [`crate::DiffViewport::set_generated`], is estimated again), a
    /// submodule one row, a change without content changes (mode only, pure
    /// rename) nothing. Text files use counts when
    /// known, else blob sizes, else [`Metrics::default_body_rows`].
    pub fn estimate_body(&self, change: &FileChange, hint: Option<SizeHint>) -> f32 {
        match change.kind {
            FileKind::Binary => return self.placeholder_height,
            FileKind::Submodule => return self.row_height,
            FileKind::Text | FileKind::Symlink => {}
        }
        if change.generated {
            return self.placeholder_height;
        }
        let added = change.old_blob.is_zero();
        let deleted = change.new_blob.is_zero();
        if !added && !deleted && change.old_blob == change.new_blob {
            return 0.0;
        }
        // Whole-file bodies (added, deleted, a symlink's one-line target) have
        // no context and no gaps.
        let whole = added || deleted || change.kind == FileKind::Symlink;
        let (additions, deletions, hunks) = match hint {
            Some(SizeHint::Counts {
                additions,
                deletions,
                hunks,
            }) => (u64::from(additions), u64::from(deletions), hunks),
            Some(SizeHint::BlobSizes { old, new }) => {
                let (old, new) = (self.lines(old), self.lines(new));
                if added || deleted {
                    (new, old, Some(1))
                } else {
                    let rows = (old + new).min(u64::from(self.default_body_rows));
                    return rows as f32 * self.row_height;
                }
            }
            None if change.kind == FileKind::Symlink => {
                (u64::from(!deleted), u64::from(!added), Some(1))
            }
            None => return self.default_body_rows as f32 * self.row_height,
        };
        self.body_from_counts(additions, deletions, hunks, whole)
    }

    fn body_from_counts(
        &self,
        additions: u64,
        deletions: u64,
        hunks: Option<u32>,
        whole: bool,
    ) -> f32 {
        let changed = additions + deletions;
        if changed == 0 {
            // Nothing changed (e.g. whitespace-only with `w`): one gap row.
            return if whole { 0.0 } else { self.gap_height };
        }
        if changed > u64::from(self.load_diff_changed_lines) {
            return self.placeholder_height;
        }
        let change_rows = match self.layout {
            Layout::Unified => changed,
            // Split pairs removed and added lines row by row.
            Layout::Split => additions.max(deletions),
        };
        if whole {
            return change_rows as f32 * self.row_height;
        }
        // Without a hunk count, assume one hunk per 16 changed lines.
        let hunks = hunks.map_or(changed.div_ceil(16), u64::from).max(1);
        let context = hunks * 2 * u64::from(self.context_lines);
        let gaps = hunks + 1;
        (change_rows + context) as f32 * self.row_height + gaps as f32 * self.gap_height
    }

    /// Lines in a blob of `bytes` bytes.
    fn lines(&self, bytes: u64) -> u64 {
        bytes.div_ceil(u64::from(self.bytes_per_line.max(1)))
    }
}
