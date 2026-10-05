//! Where threads go (design §8.6, §11.6 "Threads"): `Core::threads` plus
//! `Core::positions` (carry-forward into the diff on screen) become, per
//! file, the anchors of the viewport's blocks. Pure; the block renderers are
//! attached in [`super`].
//!
//! | Position                                   | Place                                   |
//! | ------------------------------------------ | --------------------------------------- |
//! | line subject, `exact` / `moved`            | below its last line, on its side        |
//! | line subject, `outdated`, with lines       | below the nearest mapped line           |
//! | `outdated` without lines, file subject     | under the file header (`FileTop`)       |
//! | review subject, `absent` (file or side gone), unknown | the threads panel only       |

use std::collections::{BTreeMap, HashMap};
use std::ops::Range;

use polygloss_core::review::{Position, PositionState};
use polygloss_diff::hunks::{Block, FileDiff};
use polygloss_diff::rows::Layout;
use polygloss_diff::{FileChange, Side};
use polygloss_viewport::{BlockAnchor, BlockId};

/// Where a thread shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThreadPlace {
    /// Below line `line` of `side` (0-based; the last line of a range that
    /// starts at `start_line`).
    Line {
        file_idx: u32,
        side: Side,
        start_line: u32,
        line: u32,
    },
    /// Under the file's header.
    File { file_idx: u32 },
    /// Only in the threads panel.
    Panel,
}

impl ThreadPlace {
    /// The file the thread shows in, if any.
    pub fn file_idx(&self) -> Option<u32> {
        match *self {
            ThreadPlace::Line { file_idx, .. } | ThreadPlace::File { file_idx } => Some(file_idx),
            ThreadPlace::Panel => None,
        }
    }

    /// The viewport block anchor, for places in the diff.
    pub fn anchor(&self) -> Option<BlockAnchor> {
        match *self {
            ThreadPlace::Line { side, line, .. } => Some(BlockAnchor::Line { side, line }),
            ThreadPlace::File { .. } => Some(BlockAnchor::FileTop),
            ThreadPlace::Panel => None,
        }
    }

    /// Order of blocks within a file for [`by_file`]: the header's threads
    /// first, then by line number (old side before new on the same number).
    /// Old and new line numbers are different coordinates, so this is not
    /// the diff's top-to-bottom order across sides; that is [`diff_order`].
    pub fn order(&self) -> (u32, u32, u8) {
        match *self {
            ThreadPlace::File { file_idx } => (file_idx, 0, 0),
            ThreadPlace::Line {
                file_idx,
                side,
                line,
                ..
            } => (file_idx, line + 1, side_rank(side)),
            ThreadPlace::Panel => (u32::MAX, u32::MAX, u8::MAX),
        }
    }
}

pub(crate) fn side_rank(side: Side) -> u8 {
    match side {
        Side::Old => 0,
        Side::New => 1,
    }
}

/// A file's changed blocks, `(old lines, new lines)` (0-based, half-open,
/// either may be empty), in file order.
pub type Changes = [(Range<u32>, Range<u32>)];

/// The changed blocks of `fd` (its hunks' `Change` blocks).
pub fn changes_of(fd: &FileDiff) -> Vec<(Range<u32>, Range<u32>)> {
    fd.hunks
        .iter()
        .flat_map(|h| &h.blocks)
        .filter_map(|b| match b {
            Block::Change { old, new } => Some((old.clone(), new.clone())),
            Block::Equal { .. } => None,
        })
        .collect()
}

/// Where line `line` (0-based) of `side` shows in its file's diff, top to
/// bottom, in one coordinate for both sides: `(row, phase, offset)`.
///
/// `row` is a new-side line number: an unchanged line's own (old lines
/// shifted by the changes above them), a changed line's block start. In a
/// change block, unified shows the removed lines (`phase` 0) before the
/// added ones (`phase` 1), each by `offset` into the block; split pairs them
/// row by row (both `phase` 0, the same `offset`). Unchanged lines are
/// `(new line, 1, 0)`, so a pure deletion sorts before the line after it.
/// Without `changes` (the file's diff is not known) lines order by number.
pub fn line_order(
    changes: Option<&Changes>,
    layout: Layout,
    side: Side,
    line: u32,
) -> (u32, u8, u32) {
    let Some(changes) = changes else {
        return (line, 1, 0);
    };
    let range = |c: &(Range<u32>, Range<u32>)| match side {
        Side::Old => c.0.clone(),
        Side::New => c.1.clone(),
    };
    // The first change that ends past `line` on this side.
    let i = changes.partition_point(|c| range(c).end <= line);
    if let Some(c) = changes.get(i)
        && range(c).contains(&line)
    {
        let offset = line - range(c).start;
        let phase = match (layout, side) {
            (Layout::Unified, Side::New) => 1,
            _ => 0,
        };
        return (c.1.start, phase, offset);
    }
    match side {
        Side::New => (line, 1, 0),
        // Unchanged: the offset the last change above leaves.
        Side::Old => match i.checked_sub(1).map(|p| &changes[p]) {
            Some((old, new)) => (new.end + (line - old.end), 1, 0),
            None => (line, 1, 0),
        },
    }
}

/// A place in the diff, top to bottom across files and sides:
/// `(rank, row, phase, offset, side)`, `rank` being the file's display rank
/// (the viewport's order, category sections last; T6.15), with
/// [`line_order`]'s `row` plus one (0 is under the header), the old side
/// first on a shared row. Panel-only places sort last.
pub type DiffOrder = (u32, u32, u8, u32, u8);

/// [`DiffOrder`] of a line (the cursor's, a thread's) of the file at
/// display rank `rank`.
pub fn line_diff_order(
    rank: u32,
    side: Side,
    line: u32,
    changes: Option<&Changes>,
    layout: Layout,
) -> DiffOrder {
    let (row, phase, offset) = line_order(changes, layout, side, line);
    (rank, row.saturating_add(1), phase, offset, side_rank(side))
}

/// [`DiffOrder`] of `place`, whose file is at display rank `rank`;
/// `changes` are its file's.
pub fn diff_order(
    place: &ThreadPlace,
    rank: u32,
    changes: Option<&Changes>,
    layout: Layout,
) -> DiffOrder {
    match *place {
        ThreadPlace::File { .. } => (rank, 0, 0, 0, 0),
        ThreadPlace::Line { side, line, .. } => line_diff_order(rank, side, line, changes, layout),
        ThreadPlace::Panel => (u32::MAX, u32::MAX, u8::MAX, u32::MAX, u8::MAX),
    }
}

/// The viewport block id of thread `thread_id` (FNV-1a over
/// `"thread:" + id`, stable across reloads). Other block hosts (the
/// composer) must hash a different prefix.
pub fn block_id(thread_id: &str) -> BlockId {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in b"thread:".iter().chain(thread_id.as_bytes()) {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    BlockId(h)
}

/// Each file's index by its display path (what positions name).
pub fn file_index(files: &[FileChange]) -> HashMap<String, u32> {
    files
        .iter()
        .enumerate()
        .map(|(i, f)| (f.display_path().to_owned(), i as u32))
        .collect()
}

/// The place of a thread at `position` in a diff whose files are indexed by
/// `files` ([`file_index`]). Positions use 1-based lines.
pub fn place(position: Option<&Position>, files: &HashMap<String, u32>) -> ThreadPlace {
    let Some(p) = position else {
        return ThreadPlace::Panel;
    };
    if p.state == PositionState::Absent {
        return ThreadPlace::Panel;
    }
    let Some(&file_idx) = p.path.as_deref().and_then(|path| files.get(path)) else {
        return ThreadPlace::Panel;
    };
    match (p.side, p.start_line, p.line) {
        (Some(side), start, Some(line)) if line >= 1 => {
            let start = start.unwrap_or(line).clamp(1, line);
            ThreadPlace::Line {
                file_idx,
                side,
                start_line: start - 1,
                line: line - 1,
            }
        }
        _ => ThreadPlace::File { file_idx },
    }
}

/// A thread's block in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedThread {
    pub thread_id: String,
    pub block: BlockId,
    pub anchor: BlockAnchor,
}

/// The blocks of every file that shows threads, in viewport order: the
/// header's first, then by line, threads on the same line in the order
/// given (creation order).
pub fn by_file<'a>(
    threads: impl IntoIterator<Item = (&'a str, ThreadPlace)>,
) -> BTreeMap<u32, Vec<PlacedThread>> {
    let mut out: BTreeMap<u32, Vec<(ThreadPlace, PlacedThread)>> = BTreeMap::new();
    for (id, place) in threads {
        let (Some(file_idx), Some(anchor)) = (place.file_idx(), place.anchor()) else {
            continue;
        };
        out.entry(file_idx).or_default().push((
            place,
            PlacedThread {
                thread_id: id.to_owned(),
                block: block_id(id),
                anchor,
            },
        ));
    }
    out.into_iter()
        .map(|(f, mut v)| {
            v.sort_by_key(|(place, _)| place.order());
            (f, v.into_iter().map(|(_, p)| p).collect())
        })
        .collect()
}
