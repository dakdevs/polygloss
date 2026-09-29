//! A materialized file: the data the viewport paints for one file (design §12.4
//! "Materialized").
//!
//! [`MaterializedFile::load`] is one background load of the pipeline
//! ([`crate::pipeline`]) in stages: blobs (the old one first: a NUL byte in
//! either side makes the file binary, git's rule) → `diff_blobs` → word
//! ranges of paired lines (none for files over the "Load diff" threshold) →
//! the rows of the current layout. A cancel flag is checked between stages,
//! so a file that scrolled away stops at the next one.
//!
//! Everything but the tokens sits behind an `Arc`, so a new version of a file
//! (tokens swapped in or dropped) shares the diff, rows, word ranges and
//! blobs of the old one instead of copying them on the main thread.

use std::fmt;
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use polygloss_diff::hunks::{Block, FileDiff, Hunk, diff_blobs};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::rows::{Expansions, Layout, Row, build_rows};
use polygloss_diff::word::{Granularity, LinePair, WordRanges, pair_lines, word_ranges};
use polygloss_diff::{FileChange, FileKind, Oid, Side};
use polygloss_highlight::Tokens;

use crate::provider::DiffProvider;

/// How [`MaterializedFile::load`] builds a file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LoadOptions {
    pub diff: DiffOptions,
    /// Word or char ranges on paired lines; `None` computes none.
    pub word_diff: Option<Granularity>,
    /// A file with more changed lines gets neither word ranges nor rows: it
    /// shows a "Load diff" placeholder (design §12.3).
    pub large_file_changed_lines: u32,
    /// Also build the rows of this layout (never for a large file).
    pub rows: Option<Layout>,
}

impl Default for LoadOptions {
    fn default() -> LoadOptions {
        LoadOptions {
            diff: DiffOptions::default(),
            word_diff: Some(Granularity::Word),
            large_file_changed_lines: 20_000,
            rows: None,
        }
    }
}

/// What [`MaterializedFile::load`] found.
#[derive(Debug)]
pub enum Loaded {
    File(MaterializedFile),
    /// A file listed as text has a NUL byte in either blob (git's rule).
    Binary,
}

/// Why [`MaterializedFile::load`] produced nothing.
#[derive(Debug)]
pub enum LoadError {
    /// The cancel flag was raised (the file scrolled away).
    Cancelled,
    /// A blob could not be read.
    Failed(anyhow::Error),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Cancelled => f.write_str("loading was cancelled"),
            LoadError::Failed(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// One file's diff with both blobs, its rows (per layout, built on first use),
/// word ranges for paired lines and, once highlighted, syntax tokens.
/// Immutable once shared; a new version (e.g. with tokens) replaces the whole
/// `Arc` and shares everything but the tokens with the old one.
#[derive(Debug)]
pub struct MaterializedFile {
    pub diff: Arc<FileDiff>,
    /// Rows per layout, built on first use and shared by every version.
    pub rows_split: Arc<OnceLock<Vec<Row>>>,
    pub rows_unified: Arc<OnceLock<Vec<Row>>>,
    /// Word ranges of `pairs[i]`, or empty when word diff is off. `None` for
    /// a pair with a line over the word-diff limit.
    pub words: Arc<[Option<WordRanges>]>,
    /// Every paired line (GitHub-style pairing, `pair_lines` of each change
    /// block) in file order; old and new lines both strictly increase.
    pub pairs: Arc<[LinePair]>,
    /// The blobs the diff was computed from (empty for a missing side).
    pub old_text: Arc<[u8]>,
    pub new_text: Arc<[u8]>,
    pub old_tokens: Option<Arc<Tokens>>,
    pub new_tokens: Option<Arc<Tokens>>,
    /// Approximate heap size of the data and tokens, without the rows (built
    /// on demand; [`MaterializedFile::resident_bytes`] adds them).
    pub heap_bytes: usize,
}

impl MaterializedFile {
    /// Wraps a diff and its word ranges without blob text; `heap_bytes`
    /// estimates both.
    pub fn new(diff: FileDiff, words: Vec<Option<WordRanges>>) -> MaterializedFile {
        let pairs = file_pairs(&diff);
        let empty: Arc<[u8]> = Arc::from(&[][..]);
        MaterializedFile::assemble(diff, words, pairs, empty.clone(), empty)
    }

    /// Diffs two blobs with `opts` and computes word ranges for every paired
    /// line at `word_diff` granularity (none when `None`).
    pub fn from_blobs(
        old: Arc<[u8]>,
        new: Arc<[u8]>,
        opts: &DiffOptions,
        word_diff: Option<Granularity>,
    ) -> MaterializedFile {
        let never = AtomicUsize::new(0);
        MaterializedFile::build(old, new, opts, word_diff, u32::MAX, &never)
            .unwrap_or_else(|_| unreachable!("the flag is never raised"))
    }

    /// Reads `change`'s blobs from `provider` and builds the file in stages,
    /// with the rows of `opts.rows`: see the module docs. Runs on the
    /// background executor. `cancel` holding anything but zero stops the
    /// work at the next stage with [`LoadError::Cancelled`].
    pub fn load(
        provider: &dyn DiffProvider,
        change: &FileChange,
        opts: &LoadOptions,
        cancel: &AtomicUsize,
    ) -> Result<Loaded, LoadError> {
        let sniff = change.kind == FileKind::Text;
        let old = read_blob(provider, &change.old_blob, cancel)?;
        if sniff && is_binary(&old) {
            return Ok(Loaded::Binary);
        }
        let new = read_blob(provider, &change.new_blob, cancel)?;
        if sniff && is_binary(&new) {
            return Ok(Loaded::Binary);
        }
        let limit = opts.large_file_changed_lines;
        let file = MaterializedFile::build(old, new, &opts.diff, opts.word_diff, limit, cancel)?;
        if let Some(layout) = opts.rows
            && !file.is_large(limit)
        {
            check(cancel)?;
            file.rows(layout);
        }
        Ok(Loaded::File(file))
    }

    /// Diff → (cancel check) → word ranges unless there are more than `large`
    /// changed lines (checking `cancel` every 256 pairs).
    fn build(
        old: Arc<[u8]>,
        new: Arc<[u8]>,
        opts: &DiffOptions,
        word_diff: Option<Granularity>,
        large: u32,
        cancel: &AtomicUsize,
    ) -> Result<MaterializedFile, LoadError> {
        let diff = diff_blobs(&old, &new, opts);
        check(cancel)?;
        let pairs = file_pairs(&diff);
        let mut words = Vec::new();
        if let Some(g) = word_diff
            && diff.additions.saturating_add(diff.deletions) <= large
        {
            words.reserve_exact(pairs.len());
            for (i, p) in pairs.iter().enumerate() {
                if i % 256 == 255 {
                    check(cancel)?;
                }
                let (o, n) = (diff.old.line(&old, p.old), diff.new.line(&new, p.new));
                words.push(word_ranges(o, n, g));
            }
        }
        Ok(MaterializedFile::assemble(diff, words, pairs, old, new))
    }

    /// Added plus removed lines.
    pub fn changed_lines(&self) -> u32 {
        self.diff.additions.saturating_add(self.diff.deletions)
    }

    /// Whether the file has more than `limit` changed lines: it shows a
    /// "Load diff" placeholder, and has no word ranges and no rows.
    pub fn is_large(&self, limit: u32) -> bool {
        self.changed_lines() > limit
    }

    fn assemble(
        diff: FileDiff,
        words: Vec<Option<WordRanges>>,
        pairs: Vec<LinePair>,
        old_text: Arc<[u8]>,
        new_text: Arc<[u8]>,
    ) -> MaterializedFile {
        let heap_bytes = diff_heap_bytes(&diff)
            + words_heap_bytes(&words)
            + pairs.capacity() * size_of::<LinePair>()
            + old_text.len()
            + new_text.len();
        MaterializedFile {
            diff: Arc::new(diff),
            rows_split: Arc::default(),
            rows_unified: Arc::default(),
            words: words.into(),
            pairs: pairs.into(),
            old_text,
            new_text,
            old_tokens: None,
            new_tokens: None,
            heap_bytes,
        }
    }

    /// Heap bytes held for the eviction budget: [`MaterializedFile::heap_bytes`]
    /// plus the rows built so far.
    pub fn resident_bytes(&self) -> usize {
        let rows = |cell: &OnceLock<Vec<Row>>| {
            cell.get()
                .map_or(0, |rows| rows.capacity() * size_of::<Row>())
        };
        self.heap_bytes + rows(&self.rows_split) + rows(&self.rows_unified)
    }

    /// The rows of `layout` with nothing expanded, built on first use.
    pub fn rows(&self, layout: Layout) -> &[Row] {
        let cell = match layout {
            Layout::Split => &self.rows_split,
            Layout::Unified => &self.rows_unified,
        };
        cell.get_or_init(|| build_rows(&self.diff, &Expansions::default(), layout))
    }

    /// Line `line` (0-based) of `side`, without its `\n`; empty past the end.
    pub fn line(&self, side: Side, line: u32) -> &[u8] {
        let (index, text) = match side {
            Side::Old => (&self.diff.old, &self.old_text),
            Side::New => (&self.diff.new, &self.new_text),
        };
        if line < index.len() {
            index.line(text, line)
        } else {
            &[]
        }
    }

    /// Word ranges of the pair containing `line` of `side`, if the line is
    /// paired and word diff was computed for it.
    pub fn word_ranges(&self, side: Side, line: u32) -> Option<&WordRanges> {
        let key = |p: &LinePair| match side {
            Side::Old => p.old,
            Side::New => p.new,
        };
        let i = self.pairs.binary_search_by_key(&line, key).ok()?;
        self.words.get(i)?.as_ref()
    }

    pub fn tokens(&self, side: Side) -> Option<&Arc<Tokens>> {
        match side {
            Side::Old => self.old_tokens.as_ref(),
            Side::New => self.new_tokens.as_ref(),
        }
    }

    /// A new version with `old`/`new` tokens that shares everything else
    /// (rows included, also those built later) with this one: O(1), no copy.
    pub fn with_tokens(
        &self,
        old: Option<Arc<Tokens>>,
        new: Option<Arc<Tokens>>,
    ) -> MaterializedFile {
        let token_bytes = |t: &Option<Arc<Tokens>>| t.as_ref().map_or(0, |t| t.heap_bytes());
        let heap_bytes =
            self.heap_bytes - token_bytes(&self.old_tokens) - token_bytes(&self.new_tokens)
                + token_bytes(&old)
                + token_bytes(&new);
        MaterializedFile {
            diff: self.diff.clone(),
            rows_split: self.rows_split.clone(),
            rows_unified: self.rows_unified.clone(),
            words: self.words.clone(),
            pairs: self.pairs.clone(),
            old_text: self.old_text.clone(),
            new_text: self.new_text.clone(),
            old_tokens: old,
            new_tokens: new,
            heap_bytes,
        }
    }
}

/// git's binary heuristic looks for a NUL byte in this many leading bytes
/// (`FIRST_FEW_BYTES` in git's `xdiff-interface.c`; the same rule as
/// `polygloss_core::objects::is_binary`, which the viewport cannot depend on).
pub const BINARY_SNIFF_LEN: usize = 8_000;

/// Whether `bytes` is binary content by git's rule: a NUL in its first 8,000
/// bytes.
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes[..bytes.len().min(BINARY_SNIFF_LEN)].contains(&0)
}

/// Fails with [`LoadError::Cancelled`] once `cancel` is raised.
pub(crate) fn check(cancel: &AtomicUsize) -> Result<(), LoadError> {
    if cancel.load(Ordering::Relaxed) == 0 {
        Ok(())
    } else {
        Err(LoadError::Cancelled)
    }
}

/// A blob's bytes (empty for the all-zero id of a missing side), after a
/// cancel check.
pub(crate) fn read_blob(
    provider: &dyn DiffProvider,
    oid: &Oid,
    cancel: &AtomicUsize,
) -> Result<Arc<[u8]>, LoadError> {
    check(cancel)?;
    if oid.is_zero() {
        Ok(Arc::from(&[][..]))
    } else {
        provider.load_blob(oid).map_err(LoadError::Failed)
    }
}

/// Every paired line of `diff`, in file order.
fn file_pairs(diff: &FileDiff) -> Vec<LinePair> {
    diff.hunks
        .iter()
        .flat_map(|h| &h.blocks)
        .filter_map(|b| match b {
            Block::Change { old, new } => Some(pair_lines(old.clone(), new.clone())),
            Block::Equal { .. } => None,
        })
        .flatten()
        .collect()
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
