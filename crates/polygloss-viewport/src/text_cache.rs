//! Shaped text: turning a source line into what is displayed (tabs, control
//! chars, cut long lines), its syntax-colored runs, the shaped result with
//! its word highlights, and the cache that keeps it (design §12.4 "Rows":
//! shape only visible rows, cache shaped lines per row, theme and font).
//!
//! GPUI's own line-layout cache keeps only the current and previous frame, so
//! scrolling back to rows seen a second ago would shape them again. This cache
//! keeps shaped lines across frames, keyed by what they show (file, side and
//! line; whether syntax tokens were applied; the wrap width), and is cleared
//! when the theme or the code font changes. Entries are least-recently-used
//! evicted by count.

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::{
    App, Font, Hsla, Pixels, Point, SharedString, TextAlign, TextRun, Window, WindowTextSystem,
    WrappedLine, px,
};
use polygloss_diff::Side;
use polygloss_highlight::{Span, Tokens};

use crate::layout::{Geometry, TAB_WIDTH};
use crate::materialize::MaterializedFile;
use crate::style::ViewportTheme;

/// Entries kept; a screen of split rows uses a few hundred.
pub(crate) const TEXT_CACHE_CAPACITY: usize = 4096;

/// What a cached shaped line shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum TextKey {
    /// Line `line` of `side` of file `file`, with or without syntax tokens,
    /// wrapped at `wrap` px (0 = not wrapped).
    Code {
        file: u32,
        side: Side,
        line: u32,
        styled: bool,
        wrap: u32,
    },
    /// A 1-based line number.
    Number(u32),
    /// A header's `+n` or `−n`, by value and color slot.
    Count { n: u32, color: u8 },
    /// Any other text (headers, gap labels, markers), by content hash and a
    /// color slot.
    Label { hash: u64, color: u8 },
}

/// A shaped line. Always a GPUI `WrappedLine` (wrapped or not): unlike
/// `ShapedLine`, which carries a 32-entry inline decoration buffer (~3 KB),
/// it keeps its runs on the heap at their real size, so the cache stays small.
pub(crate) struct Shaped(WrappedLine);

impl Shaped {
    /// Paints with the top-left corner at `origin`; each visual row is
    /// `line_height` tall.
    pub fn paint(
        &self,
        origin: Point<Pixels>,
        line_height: Pixels,
        window: &mut Window,
        cx: &mut App,
    ) {
        let _ = self
            .0
            .paint(origin, line_height, TextAlign::Left, None, window, cx);
    }

    /// Visual rows: 1 plus the wrap boundaries.
    pub fn visual_rows(&self) -> u32 {
        self.0.wrap_boundaries().len() as u32 + 1
    }

    /// Width of the text (of its widest visual row when wrapped).
    pub fn width(&self) -> f32 {
        self.0.width().as_f32()
    }

    /// The char boundary (display byte index) closest to `x` on visual row
    /// `row`, relative to the text origin: 0 before the text, the row's end
    /// past it. A binary search over [`Shaped::position`] (positions grow
    /// along the text, row by row), so it costs `O(log n)` lookups.
    pub fn closest_index(&self, x: f32, row: u32, line_height: f32) -> usize {
        let text = &self.0.text;
        let bounds: Vec<usize> = text
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
            .collect();
        let key = |i: usize| self.position(bounds[i], line_height);
        let before = |(bx, br): (f32, u32)| br < row || (br == row && bx < x);
        let (mut lo, mut hi) = (0, bounds.len());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if before(key(mid)) {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let at = lo;
        let candidates = [at.checked_sub(1), (at < bounds.len()).then_some(at)];
        candidates
            .into_iter()
            .flatten()
            .filter(|&i| key(i).1 == row)
            .min_by(|&a, &b| (key(a).0 - x).abs().total_cmp(&(key(b).0 - x).abs()))
            .or(candidates[0])
            .map_or(0, |i| bounds[i])
    }

    /// Position of byte `index` relative to the text origin: `(x, visual row)`.
    pub fn position(&self, index: usize, line_height: f32) -> (f32, u32) {
        match self.0.position_for_index(index, px(line_height)) {
            Some(p) => (p.x.as_f32(), (p.y.as_f32() / line_height).round() as u32),
            None => (self.width(), self.visual_rows() - 1),
        }
    }
}

/// A word highlight within a shaped line: `x0..x1` on visual row `row`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct WordRect {
    pub x0: f32,
    pub x1: f32,
    pub row: u32,
}

/// A shaped line and, for code on a paired line, where its changed words are
/// (computed once, when the line is shaped).
pub(crate) struct ShapedText {
    pub shaped: Shaped,
    pub words: Vec<WordRect>,
}

impl ShapedText {
    /// The displayed text.
    #[cfg_attr(not(feature = "debug-inspect"), allow(dead_code))]
    pub fn text(&self) -> &str {
        &self.shaped.0.text
    }
}

struct Entry {
    shaped: Rc<ShapedText>,
    used: u64,
}

/// Shaped lines by [`TextKey`], with hit and miss counters.
pub(crate) struct TextCache {
    map: HashMap<TextKey, Entry>,
    capacity: usize,
    tick: u64,
    pub hits: u64,
    pub misses: u64,
}

impl TextCache {
    pub fn new(capacity: usize) -> TextCache {
        TextCache {
            map: HashMap::with_capacity(capacity),
            capacity: capacity.max(1),
            tick: 0,
            hits: 0,
            misses: 0,
        }
    }

    /// Marks the start of a frame: entries used from now on are the most
    /// recent.
    pub fn begin_frame(&mut self) {
        self.tick += 1;
    }

    /// The cached line for `key`, or `shape()`'s result, cached.
    pub fn get_or_shape(
        &mut self,
        key: TextKey,
        shape: impl FnOnce() -> ShapedText,
    ) -> Rc<ShapedText> {
        if let Some(entry) = self.map.get_mut(&key) {
            entry.used = self.tick;
            self.hits += 1;
            return entry.shaped.clone();
        }
        self.misses += 1;
        let shaped = Rc::new(shape());
        self.map.insert(
            key,
            Entry {
                shaped: shaped.clone(),
                used: self.tick,
            },
        );
        if self.map.len() > self.capacity + self.capacity / 4 {
            self.evict();
        }
        shaped
    }

    /// Drops everything (theme or font change); counters are kept.
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Keeps the `capacity` most recently used entries. Runs once per
    /// `capacity / 4` insertions, so it is amortized O(1) per insert.
    fn evict(&mut self) {
        let mut ticks: Vec<u64> = self.map.values().map(|e| e.used).collect();
        let drop = self.map.len() - self.capacity;
        let (_, &mut cutoff, _) = ticks.select_nth_unstable(drop);
        // Entries used before `cutoff` go; ties at `cutoff` stay unless the
        // current frame alone overflows the cache.
        self.map.retain(|_, e| e.used >= cutoff);
        if self.map.len() > self.capacity + self.capacity / 4 {
            let current = self.tick;
            self.map.retain(|_, e| e.used == current);
        }
    }
}

/// Chars shaped per line without wrap: more than any column shows (no
/// horizontal scrolling yet); the rest is cut with `…`.
pub(crate) const MAX_CHARS_UNWRAPPED: usize = 1024;
/// Chars shaped per line with wrap on (hundreds of visual rows); a longer
/// line (minified code) is cut with `…`.
pub(crate) const MAX_CHARS_WRAPPED: usize = 16 * 1024;

/// The Control Picture shown for a C0 control char or DEL (`␊` for a
/// newline, `␡` for DEL), or `None` for any other char.
pub(crate) fn control_picture(c: char) -> Option<char> {
    match c {
        '\u{7f}' => Some('\u{2421}'),
        c if (c as u32) < 0x20 => char::from_u32(0x2400 + c as u32),
        _ => None,
    }
}

/// A source line as it is displayed: tabs expanded to [`TAB_WIDTH`] stops,
/// control chars as Control Pictures (`␛`), invalid UTF-8 as `U+FFFD`, a
/// trailing `\r` hidden, and cut after `max_chars` chars with `…`.
pub(crate) struct DisplayLine {
    pub text: String,
    /// `(source offset, display offset)` after every char that changed
    /// length; offsets in between shift by the last entry.
    breaks: Vec<(u32, u32)>,
    /// `(source offset, display offset)` before each of those chars.
    starts: Vec<(u32, u32)>,
    /// Source bytes shown (everything before the cut or the hidden `\r`).
    shown: u32,
    /// Display bytes before the `…`.
    shown_display: usize,
}

impl DisplayLine {
    pub fn new(src: &[u8], max_chars: usize) -> DisplayLine {
        let src = src.strip_suffix(b"\r").unwrap_or(src);
        let mut text = String::with_capacity(src.len().min(max_chars.saturating_mul(4)) + 3);
        let mut breaks = Vec::new();
        let mut starts = Vec::new();
        let (mut at, mut chars, mut col) = (0usize, 0usize, 0usize);
        let mut cut = false;
        'chunks: for chunk in src.utf8_chunks() {
            for ch in chunk.valid().chars() {
                if chars == max_chars {
                    cut = true;
                    break 'chunks;
                }
                let before = (at as u32, text.len() as u32);
                at += ch.len_utf8();
                chars += 1;
                match ch {
                    '\t' => {
                        starts.push(before);
                        let n = TAB_WIDTH - col % TAB_WIDTH;
                        text.extend(std::iter::repeat_n(' ', n));
                        col += n;
                        breaks.push((at as u32, text.len() as u32));
                    }
                    c if let Some(picture) = control_picture(c) => {
                        starts.push(before);
                        text.push(picture);
                        col += 1;
                        breaks.push((at as u32, text.len() as u32));
                    }
                    c => {
                        text.push(c);
                        col += 1;
                    }
                }
            }
            if !chunk.invalid().is_empty() {
                if chars == max_chars {
                    cut = true;
                    break;
                }
                starts.push((at as u32, text.len() as u32));
                at += chunk.invalid().len();
                chars += 1;
                col += 1;
                text.push('\u{fffd}');
                breaks.push((at as u32, text.len() as u32));
            }
        }
        let shown_display = text.len();
        if cut {
            text.push('…');
        }
        DisplayLine {
            text,
            breaks,
            starts,
            shown: at as u32,
            shown_display,
        }
    }

    /// The display offset of source byte offset `src` (a char boundary of the
    /// source line); offsets past what is shown map to the end of the shown
    /// text.
    pub fn map(&self, src: u32) -> usize {
        if src >= self.shown {
            return self.shown_display;
        }
        let i = self.breaks.partition_point(|&(s, _)| s <= src);
        let mut d = match i.checked_sub(1) {
            Some(i) => {
                let (s, d) = self.breaks[i];
                (d + (src - s)) as usize
            }
            None => src as usize,
        };
        d = d.min(self.shown_display);
        while !self.text.is_char_boundary(d) {
            d -= 1;
        }
        d
    }
}

impl DisplayLine {
    /// The source byte offset shown at display offset `display` (a char
    /// boundary of the display text): inside an expanded tab, a Control
    /// Picture or a replacement char, the nearer end of the source char;
    /// from the cut `…` on, the end of what is shown.
    pub fn unmap(&self, display: usize) -> u32 {
        if display >= self.shown_display {
            return self.shown;
        }
        let d = display as u32;
        // Changed chars that end at or before `d`.
        let j = self.breaks.partition_point(|&(_, e)| e <= d);
        if let Some(&(start_src, start_disp)) = self.starts.get(j)
            && start_disp < d
        {
            let (end_src, end_disp) = self.breaks[j];
            return if d - start_disp <= end_disp - d {
                start_src
            } else {
                end_src
            };
        }
        let (s, e) = j.checked_sub(1).map_or((0, 0), |i| self.breaks[i]);
        s + (d - e)
    }
}

/// Text runs for `line`: `spans` (source offsets) in their theme styles, the
/// rest in the foreground color.
pub(crate) fn code_runs(
    line: &DisplayLine,
    spans: &[Span],
    theme: &ViewportTheme,
    base: &Font,
) -> Vec<TextRun> {
    let run = |len: usize, color: Hsla, font: Font| TextRun {
        len,
        font,
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let mut runs = Vec::with_capacity(spans.len() * 2 + 1);
    let mut at = 0;
    for span in spans {
        let start = line.map(span.start);
        let end = line.map(span.start.saturating_add(span.len));
        if end <= start || start < at {
            continue;
        }
        if start > at {
            runs.push(run(start - at, theme.foreground, base.clone()));
        }
        let style = theme.token_style(span.style);
        let mut font = base.clone();
        font.weight = style.weight;
        font.style = style.style;
        runs.push(run(end - start, style.color, font));
        at = end;
    }
    if at < line.text.len() || runs.is_empty() {
        runs.push(run(line.text.len() - at, theme.foreground, base.clone()));
    }
    runs
}

/// Turns source lines and labels into [`ShapedText`] (on cache misses).
pub(crate) struct Shaper<'a> {
    pub theme: &'a ViewportTheme,
    pub font: &'a Font,
    pub geometry: Geometry,
    pub text_system: &'a WindowTextSystem,
}

impl Shaper<'_> {
    /// Line `line` of `side` of `file`, colored by `tokens` (plain when
    /// `None`), wrapped at `wrap_width` px (0 = not wrapped), with its changed
    /// words located when `words` is set.
    pub fn code(
        &self,
        file: &MaterializedFile,
        side: Side,
        line: u32,
        tokens: Option<&Tokens>,
        wrap_width: f32,
        words: bool,
    ) -> ShapedText {
        let max = if wrap_width > 0.0 {
            MAX_CHARS_WRAPPED
        } else {
            MAX_CHARS_UNWRAPPED
        };
        let display = DisplayLine::new(file.line(side, line), max);
        let spans = tokens.map_or(&[][..], |t| t.line(line));
        let runs = code_runs(&display, spans, self.theme, self.font);
        let wrap = (wrap_width > 0.0).then(|| px(wrap_width));
        let shaped = self.shape(SharedString::from(display.text.clone()), &runs, wrap);
        let words = if words {
            word_rects(
                file,
                side,
                line,
                &display,
                &shaped,
                self.geometry,
                wrap_width,
            )
        } else {
            Vec::new()
        };
        ShapedText { shaped, words }
    }

    /// `text` on one line in `color`. Control chars (a newline or tab in a
    /// path, in an error message) are shown as Control Pictures, so nothing
    /// splits the label or disappears from it.
    pub fn label(&self, text: &str, color: Hsla) -> ShapedText {
        let text: String = if text.chars().any(|c| control_picture(c).is_some()) {
            text.chars()
                .map(|c| control_picture(c).unwrap_or(c))
                .collect()
        } else {
            text.to_owned()
        };
        let run = TextRun {
            len: text.len(),
            font: self.font.clone(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        ShapedText {
            shaped: self.shape(SharedString::from(text), &[run], None),
            words: Vec::new(),
        }
    }

    fn shape(&self, text: SharedString, runs: &[TextRun], wrap: Option<Pixels>) -> Shaped {
        let size = px(self.geometry.font_size);
        let line = self
            .text_system
            .shape_text(text, size, runs, wrap, None)
            .ok()
            .and_then(|lines| lines.into_iter().next())
            .unwrap_or_default();
        Shaped(line)
    }
}

/// Where the changed words of `line` are in its shaped text.
fn word_rects(
    file: &MaterializedFile,
    side: Side,
    line: u32,
    display: &DisplayLine,
    shaped: &Shaped,
    geometry: Geometry,
    wrap_width: f32,
) -> Vec<WordRect> {
    let Some(words) = file.word_ranges(side, line) else {
        return Vec::new();
    };
    let ranges = match side {
        Side::Old => &words.old,
        Side::New => &words.new,
    };
    let mut rects = Vec::with_capacity(ranges.len());
    for r in ranges {
        let (start, end) = (display.map(r.start), display.map(r.end));
        if end <= start {
            continue;
        }
        let (x0, row0) = shaped.position(start, geometry.row_height);
        let (x1, row1) = shaped.position(end, geometry.row_height);
        if row0 == row1 {
            rects.push(WordRect { x0, x1, row: row0 });
        } else {
            // Across wrap boundaries: to the end of the first row, whole rows
            // in between, from the start of the last row.
            rects.push(WordRect {
                x0,
                x1: wrap_width,
                row: row0,
            });
            for row in row0 + 1..row1 {
                rects.push(WordRect {
                    x0: 0.0,
                    x1: wrap_width,
                    row,
                });
            }
            rects.push(WordRect {
                x0: 0.0,
                x1,
                row: row1,
            });
        }
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::DisplayLine;

    #[test]
    fn unmap_inverts_map_and_snaps_inside_expanded_chars() {
        // `\t` expands to 4 spaces, `\x1b` to `␛` (3 bytes), `é` is 2 bytes
        // on both sides.
        let src = "a\tb\x1bé".as_bytes();
        let line = DisplayLine::new(src, 1024);
        assert_eq!(line.text, "a   b␛é");
        for s in [0u32, 1, 2, 3, 4, 6] {
            assert_eq!(line.unmap(line.map(s)), s, "source offset {s}");
        }
        // Inside the tab (display 1..4): the nearer end of the `\t`.
        assert_eq!(line.unmap(2), 1);
        assert_eq!(line.unmap(3), 2);
        // Past the end.
        assert_eq!(line.unmap(100), src.len() as u32);
        // A cut line: from the `…` on, the end of what is shown.
        let cut = DisplayLine::new(b"abcdef", 3);
        assert_eq!(cut.text, "abc…");
        assert_eq!(cut.unmap(3), 3);
        assert_eq!(cut.unmap(6), 3);
    }
}
