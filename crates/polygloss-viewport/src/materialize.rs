//! A materialized file: the data the viewport paints for one file (design §12.4
//! "Materialized").
//!
//! T2.3 defines the struct so the document can hold it; T2.6 owns this module
//! and adds the tokens (`old_tokens`/`new_tokens`, from T2.2) and the stages
//! that build it.

use std::ops::Range;
use std::sync::OnceLock;

use polygloss_diff::hunks::{Block, FileDiff, Hunk};
use polygloss_diff::rows::Row;
use polygloss_diff::word::WordRanges;

/// One file's diff with its rows (per layout, built on first use) and word
/// ranges for paired lines. Immutable once shared; a new version (e.g. with
/// tokens) replaces the whole `Arc`.
#[derive(Debug)]
pub struct MaterializedFile {
    pub diff: FileDiff,
    pub rows_split: OnceLock<Vec<Row>>,
    pub rows_unified: OnceLock<Vec<Row>>,
    pub words: Vec<Option<WordRanges>>,
    /// Approximate heap size, for the document's eviction budget.
    pub heap_bytes: usize,
}

impl MaterializedFile {
    /// Wraps a diff and its word ranges; `heap_bytes` estimates both.
    pub fn new(diff: FileDiff, words: Vec<Option<WordRanges>>) -> MaterializedFile {
        let heap_bytes = diff_heap_bytes(&diff) + words_heap_bytes(&words);
        MaterializedFile {
            diff,
            rows_split: OnceLock::new(),
            rows_unified: OnceLock::new(),
            words,
            heap_bytes,
        }
    }
}

/// Approximate heap bytes of a diff: both line indexes (one offset per line)
/// and the hunks with their blocks.
fn diff_heap_bytes(diff: &FileDiff) -> usize {
    let lines = (diff.old.len() as usize + diff.new.len() as usize) * size_of::<usize>();
    let hunks = diff.hunks.capacity() * size_of::<Hunk>();
    let blocks: usize = diff
        .hunks
        .iter()
        .map(|h| h.blocks.capacity() * size_of::<Block>())
        .sum();
    lines + hunks + blocks
}

fn words_heap_bytes(words: &[Option<WordRanges>]) -> usize {
    let ranges: usize = words
        .iter()
        .flatten()
        .map(|w| (w.old.capacity() + w.new.capacity()) * size_of::<Range<u32>>())
        .sum();
    size_of_val(words) + ranges
}
