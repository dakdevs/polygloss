//! Hunk computation with context grouping on gix-imara-diff (T1.6, design §6.3).
//!
//! imara yields changed line ranges only; context and hunk merging are ours and
//! follow git's `xdl_get_hunk`/`xdl_emit_diff`: changes join one hunk while the
//! unchanged gap between them is at most `2 * context + inter_hunk_context`
//! lines, and each hunk carries up to `context` lines on both ends.

use std::ops::Range;

use gix_imara_diff::sources::byte_lines;
use gix_imara_diff::{Diff, IndentHeuristic, IndentLevel, InternedInput, Interner, Token};

use crate::lines::LineIndex;
use crate::options::{Algorithm, DiffOptions};
use crate::whitespace::strip_whitespace;

/// A run of lines inside a hunk. `Equal` ranges have the same length (in
/// whitespace mode their lines may differ in whitespace only). In a `Change`,
/// either range may be empty (a pure insertion or deletion).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Block {
    Equal { old: Range<u32>, new: Range<u32> },
    Change { old: Range<u32>, new: Range<u32> },
}

/// One hunk: 0-based, half-open line ranges on both sides, context included,
/// and its blocks in order (they tile both ranges exactly).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Hunk {
    pub old: Range<u32>,
    pub new: Range<u32>,
    pub blocks: Vec<Block>,
}

/// The line diff of two blobs: their line indexes, the grouped hunks and the
/// number of added and removed lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub old: LineIndex,
    pub new: LineIndex,
    pub hunks: Vec<Hunk>,
    pub additions: u32,
    pub deletions: u32,
}

/// Diffs two blobs line by line: imara over `byte_lines` (the `\n` belongs to
/// the line, so a missing final newline is a change, as in git), then
/// `postprocess_lines` (git's indent/slider heuristic, tab width 8), then
/// context grouping per `opts`.
pub fn diff_blobs(old: &[u8], new: &[u8], opts: &DiffOptions) -> FileDiff {
    let changes = line_changes(old, new, opts);
    let old_index = LineIndex::new(old);
    let new_index = LineIndex::new(new);
    let additions = changes.iter().map(|(_, n)| n.len() as u32).sum();
    let deletions = changes.iter().map(|(o, _)| o.len() as u32).sum();
    let hunks = group(&changes, old_index.len(), new_index.len(), opts);
    FileDiff {
        old: old_index,
        new: new_index,
        hunks,
        additions,
        deletions,
    }
}

/// The changed line ranges `(old, new)` of two blobs, in order, without any
/// context: imara over `byte_lines` with `opts.algorithm` and whitespace mode,
/// then the indent heuristic. Everything between them is equal. Shared by
/// [`diff_blobs`] and [`crate::line_map::LineMap::new`], so both see the same
/// alignment; `opts.context` and `opts.inter_hunk_context` are ignored.
pub(crate) fn line_changes(
    old: &[u8],
    new: &[u8],
    opts: &DiffOptions,
) -> Vec<(Range<u32>, Range<u32>)> {
    if opts.ignore_whitespace {
        return changes_ignoring_whitespace(old, new, opts.algorithm);
    }
    let input = InternedInput::new(byte_lines(old), byte_lines(new));
    let mut diff = Diff::compute(imara_algorithm(opts.algorithm), &input);
    diff.postprocess_lines(&input);
    diff.hunks().map(|h| (h.before, h.after)).collect()
}

fn imara_algorithm(algorithm: Algorithm) -> gix_imara_diff::Algorithm {
    match algorithm {
        Algorithm::Myers => gix_imara_diff::Algorithm::Myers,
        Algorithm::Histogram => gix_imara_diff::Algorithm::Histogram,
    }
}

/// Changed ranges when lines compare with all whitespace removed. The slider
/// heuristic still needs indentation, which the stripped keys no longer carry,
/// so each key scores with the indentation of the first line that produced it.
/// git scores by position instead; the two only differ when a slidable block's
/// lines repeat with different indentation.
fn changes_ignoring_whitespace(
    old: &[u8],
    new: &[u8],
    algorithm: Algorithm,
) -> Vec<(Range<u32>, Range<u32>)> {
    let mut interner: Interner<Vec<u8>> = Interner::new(0);
    let mut indents: Vec<IndentLevel> = Vec::new();
    let mut tokens = |blob: &[u8]| -> Vec<Token> {
        byte_lines(blob)
            .map(|line| {
                let token = interner.intern(strip_whitespace(line));
                if token.0 as usize == indents.len() {
                    indents.push(IndentLevel::for_ascii_line(line.iter().copied(), 8));
                }
                token
            })
            .collect()
    };
    let before = tokens(old);
    let after = tokens(new);
    let mut diff = Diff::default();
    diff.compute_with(
        imara_algorithm(algorithm),
        &before,
        &after,
        interner.num_tokens(),
    );
    diff.postprocess_with(
        &before,
        &after,
        IndentHeuristic::new(|token: Token| indents[token.0 as usize]),
    );
    diff.hunks().map(|h| (h.before, h.after)).collect()
}

/// Groups imara's changed ranges into hunks with context, like git's
/// `xdl_get_hunk` (merge while the gap is <= `2 * context + inter_hunk_context`)
/// and `xdl_emit_diff` (context clamped at both file ends).
fn group(
    changes: &[(Range<u32>, Range<u32>)],
    old_len: u32,
    new_len: u32,
    opts: &DiffOptions,
) -> Vec<Hunk> {
    let max_gap = opts.max_merge_gap();
    let mut hunks = Vec::new();
    let mut i = 0;
    while i < changes.len() {
        let mut j = i + 1;
        while j < changes.len() && changes[j].0.start - changes[j - 1].0.end <= max_gap {
            j += 1;
        }
        let group = &changes[i..j];
        let (first, last) = (&group[0], &group[group.len() - 1]);

        let old_start = first.0.start.saturating_sub(opts.context);
        let new_start = first.1.start.saturating_sub(opts.context);
        let tail = opts
            .context
            .min(old_len - last.0.end)
            .min(new_len - last.1.end);
        let old_end = last.0.end + tail;
        let new_end = last.1.end + tail;

        let mut blocks = Vec::with_capacity(group.len() * 2 + 1);
        let (mut o, mut n) = (old_start, new_start);
        for (old, new) in group {
            if o < old.start {
                blocks.push(Block::Equal {
                    old: o..old.start,
                    new: n..new.start,
                });
            }
            blocks.push(Block::Change {
                old: old.clone(),
                new: new.clone(),
            });
            (o, n) = (old.end, new.end);
        }
        if o < old_end {
            blocks.push(Block::Equal {
                old: o..old_end,
                new: n..new_end,
            });
        }
        hunks.push(Hunk {
            old: old_start..old_end,
            new: new_start..new_end,
            blocks,
        });
        i = j;
    }
    hunks
}
