//! Diff options: algorithm, whitespace mode and context sizes (T1.6).

use serde::{Deserialize, Serialize};

/// The line diff algorithm (design §6.3). Myers plus the indent heuristic is the
/// GitHub-like default; Histogram is a setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Algorithm {
    #[default]
    Myers,
    Histogram,
}

/// How [`crate::hunks::diff_blobs`] computes and groups hunks. Together with the
/// two blob ids this is the hunk cache key (design §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DiffOptions {
    pub algorithm: Algorithm,
    /// Compare lines with all whitespace removed (git `-w`, the `w` toggle).
    pub ignore_whitespace: bool,
    /// Unchanged lines shown around each change (git `-U`).
    pub context: u32,
    /// Extra unchanged lines that may join two hunks (git `--inter-hunk-context`).
    /// Hunks merge when the gap between changes is at most
    /// `2 * context + inter_hunk_context` lines.
    pub inter_hunk_context: u32,
}

impl Default for DiffOptions {
    /// Myers, whitespace shown, 3 lines of context, inter-hunk context 1: hunks
    /// merge when the unchanged gap is 7 lines or fewer, like
    /// `git diff -U3 --inter-hunk-context=1` (design §6.3).
    fn default() -> DiffOptions {
        DiffOptions {
            algorithm: Algorithm::Myers,
            ignore_whitespace: false,
            context: 3,
            inter_hunk_context: 1,
        }
    }
}

impl DiffOptions {
    /// The largest unchanged gap, in lines, that still joins two changes into one hunk.
    pub fn max_merge_gap(&self) -> u32 {
        self.context
            .saturating_mul(2)
            .saturating_add(self.inter_hunk_context)
    }
}
