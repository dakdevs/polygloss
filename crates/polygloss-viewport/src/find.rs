//! Find highlights (⌘F, design §11.14): the host's search marks every match
//! in the code the viewport paints, the current match emphasized.
//!
//! The viewport knows nothing about queries: the host hands it a
//! [`FindMatcher`] (the match ranges in one source line, as the host's search
//! finds them) and which match is current ([`FindCurrent`]). Only visible
//! lines are matched, once per shaped line: the rectangles are cached with
//! the shaped line they were computed for, so a line shaped again (a new
//! theme, font or wrap width, new data) is matched again.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use polygloss_diff::Side;

use crate::layout::Geometry;
use crate::materialize::MaterializedFile;
use crate::text_cache::{DisplayLine, ShapedText, TextKey, WordRect, max_chars, span_rects};

/// The byte ranges of the matches in one source line (without its line
/// ending), in order and not overlapping; empty ranges are ignored.
pub type FindMatcher = Arc<dyn Fn(&[u8]) -> Vec<Range<usize>> + Send + Sync>;

/// The current match: `range` (bytes of the source line, as the matcher
/// returned it) in line `line` (0-based) of `side` of file `file_idx`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindCurrent {
    pub file_idx: u32,
    pub side: Side,
    pub line: u32,
    pub range: Range<usize>,
}

/// What [`crate::DiffViewport::set_find_highlights`] marks.
#[derive(Clone)]
pub struct FindHighlights {
    pub matcher: FindMatcher,
    /// Emphasized; `None` before the host went to a match.
    pub current: Option<FindCurrent>,
}

impl fmt::Debug for FindHighlights {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FindHighlights")
            .field("current", &self.current)
            .finish_non_exhaustive()
    }
}

/// A painted match: its rectangle(s) in the shaped line and its source range.
pub(crate) struct FindRect {
    pub range: Range<u32>,
    pub rect: WordRect,
}

/// The highlights in effect and the rectangles of the lines matched so far.
pub(crate) struct FindState {
    pub highlights: FindHighlights,
    /// Per shaped line: the line they were computed for and its matches.
    /// Weak, so a line the text cache evicts is freed rather than kept
    /// alive here; the allocation it points at stays reserved while the
    /// weak lives, so pointer equality still identifies the same line.
    rects: HashMap<TextKey, (Weak<ShapedText>, Rc<[FindRect]>)>,
}

/// Lines whose rectangles are kept; the cache starts over past it (a screen
/// shows a few hundred).
const CAPACITY: usize = 4096;

impl FindState {
    pub fn new(highlights: FindHighlights) -> FindState {
        FindState {
            highlights,
            rects: HashMap::new(),
        }
    }

    /// The matches of line `line` of `side` in `file`, shaped as `shaped`
    /// under `key` (wrapped at `wrap_width`, 0 = not wrapped).
    #[allow(clippy::too_many_arguments)]
    pub fn rects(
        &mut self,
        key: TextKey,
        file: &MaterializedFile,
        side: Side,
        line: u32,
        shaped: &Rc<ShapedText>,
        geometry: Geometry,
        wrap_width: f32,
    ) -> Rc<[FindRect]> {
        if let Some((at, rects)) = self.rects.get(&key)
            && std::ptr::eq(at.as_ptr(), Rc::as_ptr(shaped))
        {
            return rects.clone();
        }
        // As the search matches it: without a trailing `\r` (which the
        // display hides too).
        let src = file.line(side, line);
        let src = src.strip_suffix(b"\r").unwrap_or(src);
        let ranges = (self.highlights.matcher)(src);
        let rects: Rc<[FindRect]> = if ranges.is_empty() {
            Rc::from([])
        } else {
            let display = DisplayLine::new(src, max_chars(wrap_width));
            let mut out = Vec::with_capacity(ranges.len());
            let mut buf = Vec::new();
            for r in ranges.into_iter().filter(|r| r.start < r.end) {
                let range = r.start as u32..r.end as u32;
                buf.clear();
                span_rects(
                    range.start,
                    range.end,
                    &display,
                    &shaped.shaped,
                    geometry,
                    wrap_width,
                    &mut buf,
                );
                out.extend(buf.iter().map(|&rect| FindRect {
                    range: range.clone(),
                    rect,
                }));
            }
            out.into()
        };
        if self.rects.len() >= CAPACITY {
            self.rects.clear();
        }
        self.rects
            .insert(key, (Rc::downgrade(shaped), rects.clone()));
        rects
    }

    /// `(live, freed)`: cached lines whose shaped line is still alive, and
    /// those whose shaped line was dropped (evicted or cleared) since.
    #[cfg(feature = "debug-inspect")]
    pub fn cached_lines(&self) -> (usize, usize) {
        let live = self
            .rects
            .values()
            .filter(|(at, _)| at.strong_count() > 0)
            .count();
        (live, self.rects.len() - live)
    }

    /// The current match's range if it is in line `line` of `side` in file
    /// `f` (compare with [`FindRect::range`]).
    pub fn current_in(&self, f: u32, side: Side, line: u32) -> Option<Range<u32>> {
        self.highlights
            .current
            .as_ref()
            .filter(|c| c.file_idx == f && c.side == side && c.line == line)
            .map(|c| c.range.start as u32..c.range.end as u32)
    }
}
