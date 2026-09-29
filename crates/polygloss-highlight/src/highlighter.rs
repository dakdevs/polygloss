//! lumis highlighting into [`Tokens`] under a budget and a cancellation flag
//! (design §11.11). Runs on the caller's thread: the viewport calls it on the
//! background executor, one whole blob per side so context is correct.

use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use lumis::events::HighlightEvent;
use lumis::highlight::{HighlightError as LumisError, HighlightOptions};
use lumis::languages::Language as LumisLanguage;

use crate::language::Language;
use crate::scope_map::SyntaxTheme;
use crate::tokens::{StyleId, Tokens, TokensBuilder};

/// Why [`Highlighter::highlight`] produced no tokens. In every case the caller
/// shows plain text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HighlightError {
    /// The cancellation flag was set (the file scrolled away).
    #[error("highlighting was cancelled")]
    Cancelled,
    /// More lines than [`Budget::max_lines`], or the time ran out.
    #[error("highlighting exceeded its budget")]
    BudgetExceeded,
    /// Not UTF-8, or lumis could not run the grammar.
    #[error("highlighting is not supported for this input")]
    Unsupported,
}

/// Bounds for one [`Highlighter::highlight`] call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Wall-clock limit for tokenizing (lumis's parse and query walk included).
    /// Compiling a grammar's queries the first time a process uses it is not
    /// counted, as in lumis.
    pub time: Duration,
    /// Sources with more lines are refused up front ("Files over 100k lines per
    /// side render without syntax unless the user picks Highlight anyway",
    /// OQ-14). `u32::MAX` lifts the limit.
    pub max_lines: u32,
}

impl Budget {
    /// OQ-14 (**Provisional**).
    pub const DEFAULT_MAX_LINES: u32 = 100_000;
    /// Generous: lumis took 341 ms on a 60k-line TypeScript file (design §12.4).
    pub const DEFAULT_TIME: Duration = Duration::from_secs(2);
}

impl Default for Budget {
    fn default() -> Budget {
        Budget {
            time: Budget::DEFAULT_TIME,
            max_lines: Budget::DEFAULT_MAX_LINES,
        }
    }
}

/// Highlights whole blobs with lumis and maps scopes through a [`SyntaxTheme`].
/// Cheap to share across threads (`Send + Sync`).
pub struct Highlighter {
    theme: Arc<SyntaxTheme>,
}

/// How many events pass between cancellation checks while building tokens.
const CANCEL_CHECK_EVERY: usize = 4096;

impl Highlighter {
    pub fn new(theme: Arc<SyntaxTheme>) -> Highlighter {
        Highlighter { theme }
    }

    pub fn theme(&self) -> &Arc<SyntaxTheme> {
        &self.theme
    }

    /// Tokenizes `src` as `lang`. `cancel` holding anything but zero stops the
    /// work (checked before, during lumis's parse and query walk, and while
    /// building tokens). Budget: see [`Budget`]; the time limit bounds lumis's
    /// work, and turning its events into [`Tokens`] afterwards is one linear pass.
    pub fn highlight(
        &self,
        src: &[u8],
        lang: &Language,
        cancel: &AtomicUsize,
        budget: Budget,
    ) -> Result<Tokens, HighlightError> {
        let cancelled = || cancel.load(Ordering::Relaxed) != 0;
        if cancelled() {
            return Err(HighlightError::Cancelled);
        }
        compile_queries(lang.lumis());
        let started = Instant::now();

        let text = std::str::from_utf8(src).map_err(|_| HighlightError::Unsupported)?;
        if u32::try_from(src.len()).is_err() || exceeds_lines(src, budget.max_lines) {
            return Err(HighlightError::BudgetExceeded);
        }
        let remaining = budget.time.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(HighlightError::BudgetExceeded);
        }
        let limit_ms = u64::try_from(remaining.as_micros().div_ceil(1000)).unwrap_or(u64::MAX);
        let options = HighlightOptions::new()
            .budget(lumis::Budget::new().time_limit(Some(limit_ms)))
            .cancellation(cancel);
        let events = lumis::highlight::highlight_events_with_options(text, lang.lumis(), options)
            .map_err(from_lumis)?;

        // Innermost style, inherited by nested scopes the theme leaves unstyled.
        let mut stack: Vec<StyleId> = Vec::with_capacity(16);
        let mut lines = LineCursor::new(src);
        let mut out = TokensBuilder::with_capacity(src.len() / 32, events.len() / 2);
        for (n, event) in events.iter().enumerate() {
            if n % CANCEL_CHECK_EVERY == 0 && cancelled() {
                return Err(HighlightError::Cancelled);
            }
            let top = stack.last().copied().unwrap_or_default();
            match event {
                HighlightEvent::Start { scope_index, .. } => {
                    let style = self.theme.lumis_style(*scope_index);
                    stack.push(if style == StyleId::DEFAULT {
                        top
                    } else {
                        style
                    });
                }
                HighlightEvent::DecorationStart { .. } => stack.push(top),
                HighlightEvent::End | HighlightEvent::DecorationEnd => {
                    stack.pop();
                }
                HighlightEvent::Source { start, end } if top != StyleId::DEFAULT => {
                    lines.emit(*start, *end, top, &mut out);
                }
                _ => {}
            }
        }
        drop(events);
        let line_count = lines.line_count();
        Ok(out.finish(line_count))
    }
}

fn from_lumis(e: LumisError) -> HighlightError {
    match e {
        LumisError::Cancelled => HighlightError::Cancelled,
        LumisError::TimeLimit => HighlightError::BudgetExceeded,
        // Grammar setup or event errors: nothing the caller can retry.
        _ => HighlightError::Unsupported,
    }
}

/// Forces lumis's lazily compiled queries for `lang` (a one-time cost per
/// process that lumis keeps out of its own budget), so it is not charged to
/// the caller's [`Budget`] either.
fn compile_queries(lang: LumisLanguage) {
    static COMPILED: LazyLock<Mutex<HashSet<LumisLanguage>>> = LazyLock::new(Default::default);
    let done = |set: &Mutex<HashSet<LumisLanguage>>| {
        set.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(&lang)
    };
    if done(&COMPILED) {
        return;
    }
    // Highlighting nothing still builds the language's configuration. Any
    // error resurfaces on the real call.
    let _ = lumis::highlight::highlight_events_with_options("", lang, HighlightOptions::new());
    COMPILED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(lang);
}

/// Whether `src` has more than `max` lines (`LineIndex` rules).
fn exceeds_lines(src: &[u8], max: u32) -> bool {
    // Every line holds at least one byte (content or its `\n`).
    if src.len() <= max as usize {
        return false;
    }
    let newlines = src.iter().filter(|&&b| b == b'\n').count();
    let lines = newlines + usize::from(src.last().is_some_and(|&b| b != b'\n'));
    lines > max as usize
}

/// Walks the source once, front to back, tracking which line a byte is on.
struct LineCursor<'a> {
    src: &'a [u8],
    /// Bytes before `pos` are accounted for.
    pos: usize,
    line: u32,
    line_start: usize,
}

impl<'a> LineCursor<'a> {
    fn new(src: &'a [u8]) -> LineCursor<'a> {
        LineCursor {
            src,
            pos: 0,
            line: 0,
            line_start: 0,
        }
    }

    /// Moves to byte `to` (never backwards), counting the lines passed.
    fn advance(&mut self, to: usize) {
        let to = to.max(self.pos);
        while let Some(i) = find_newline(&self.src[self.pos..to]) {
            self.line += 1;
            self.line_start = self.pos + i + 1;
            self.pos = self.line_start;
        }
        self.pos = to;
    }

    /// Adds `start..end` in `style`, one span per line it touches (newlines
    /// excluded).
    fn emit(&mut self, start: usize, end: usize, style: StyleId, out: &mut TokensBuilder) {
        self.advance(start);
        let mut at = self.pos;
        while at < end {
            let newline = find_newline(&self.src[at..end]).map(|i| at + i);
            let segment_end = newline.unwrap_or(end);
            if segment_end > at {
                out.push(
                    self.line,
                    (at - self.line_start) as u32,
                    (segment_end - at) as u32,
                    style,
                );
            }
            match newline {
                Some(i) => {
                    self.line += 1;
                    self.line_start = i + 1;
                    at = i + 1;
                }
                None => at = end,
            }
        }
        self.pos = self.pos.max(end);
    }

    /// Total lines of the source (`LineIndex` rules).
    fn line_count(&mut self) -> u32 {
        self.advance(self.src.len());
        match self.src.last() {
            None => 0,
            Some(b'\n') => self.line,
            Some(_) => self.line + 1,
        }
    }
}

fn find_newline(hay: &[u8]) -> Option<usize> {
    hay.iter().position(|&b| b == b'\n')
}
