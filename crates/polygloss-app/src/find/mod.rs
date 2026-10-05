//! Find across all files (⌘F, design §11.14).
//!
//! Every review tab gets a [`FindBar`]. ⌘F (`tab::Find`) opens it with its
//! field focused, in the sidebar's Files segment (shown first) in place of
//! the file tree
//! ([`render_pane`], like Xcode's Find navigator, so it never covers the
//! diff); closing it brings the tree back as it was. Typing starts a search of every file's old and new blobs on the
//! background executor ([`search`]): files the viewport has not loaded
//! included, several chunks of files at once, results streamed into the bar
//! in the viewport's display order (category sections last, design §11.14)
//! with a count and a result list. A new query (or toggle) cancels the
//! search in flight, and so does a change of the viewport's diff options,
//! file list or display order (the matches' rows or order would be stale):
//! the search then runs again. `⏎` / `⇧⏎` go to the next and previous match
//! (wrapping; the first from the diff's cursor), and so does clicking a
//! result: a closed category section holding it opens (and its sidebar
//! panel, which follows the viewport), the file is expanded if collapsed,
//! a generated or large file loads its diff, hidden context around the
//! match is revealed, and the line cursor lands on the match.
//! `Esc` closes the bar and gives the keyboard back to the diff. While the
//! bar is open the diff marks every visible match of the query, the match
//! gone to in a stronger color (`DiffViewport::set_find_highlights`); closing
//! it clears the marks.
//!
//! ⌥⌘C and ⌥⌘R toggle match case and regex while the bar has focus (the
//! bar's own bindings, like a text field's; key context `FindBar`).

pub mod search;
mod view;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::component::input::{Escape, InputEvent, InputState};
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Focusable as _, Font,
    IntoElement as _, KeyBinding, ScrollStrategy, SharedString, Subscription, Task,
    UniformListScrollHandle, Window,
};
use polygloss_diff::options::DiffOptions;
use polygloss_diff::{FileChange, Side};
use polygloss_viewport::{
    BodyRow, CursorPos, DiffViewport, FileState, FindCurrent, FindHighlights, RowKey, ScrollTarget,
};
use regex::bytes::Regex;

use crate::categories::Repartitioned;
use crate::keymap::actions::tab::Find;
use crate::review_tab::ReviewTab;
use search::{FindMatch, FindOptions};

gpui_kit::actions!(
    find,
    [
        /// ⌥⌘C in the find bar: match case on or off.
        ToggleCaseSensitive,
        /// ⌥⌘R in the find bar: regular expression on or off.
        ToggleRegex,
    ]
);

/// The find bar's key context.
pub const CONTEXT: &str = "FindBar";

/// Matches kept at most; the search stops there ("10,000+ matches").
pub const MAX_MATCHES: usize = 10_000;

/// Files one background job searches.
const CHUNK: usize = 32;
/// Background jobs in flight at once.
const PARALLEL: usize = 4;

/// Registers ⌘F on review tabs and the bar's own bindings.
pub fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("alt-cmd-c", ToggleCaseSensitive, Some(CONTEXT)),
        KeyBinding::new("alt-cmd-r", ToggleRegex, Some(CONTEXT)),
    ]);
    crate::keymap::handlers::on_action::<ReviewTab, Find>(cx, |tab, _, window, cx| {
        // The bar takes the tree's place in the sidebar's Files segment.
        crate::chrome::show_files(window, cx);
        if let Some(bar) = find_bar(tab).cloned() {
            bar.update(cx, |bar, cx| bar.open(window, cx));
        }
    });
}

/// Gives the new tab its find bar.
pub fn attach(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let viewport = tab.viewport.clone();
    let diff_focus = tab.viewport_focus().clone();
    let bar = cx.new(|cx| FindBar::new(viewport, diff_focus, window, cx));
    // Opening and closing swap the left pane.
    cx.observe(&bar, |_, _, cx| cx.notify()).detach();
    // A new partition reorders the files: list the matches in the new order.
    let reorder = bar.clone();
    cx.subscribe_self(move |_: &mut ReviewTab, _: &Repartitioned, cx| {
        reorder.update(cx, FindBar::order_changed)
    })
    .detach();
    tab.insert_extension(bar);
}

/// The find bar of `tab`.
pub fn find_bar(tab: &ReviewTab) -> Option<&Entity<FindBar>> {
    tab.extension::<Entity<FindBar>>()
}

/// The left pane's content while find is open (`None` while it is closed:
/// the file tree shows).
pub fn render_pane(
    tab: &ReviewTab,
    _window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> Option<AnyElement> {
    let bar = find_bar(tab)?;
    bar.read(cx).open.then(|| bar.clone().into_any_element())
}

/// A row of the result list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListRow {
    /// A file's heading.
    File(u32),
    /// Match `ix` of [`FindBar::matches`].
    Match(usize),
}

/// Searches run by one bar (tests check cancellation with it).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FindStats {
    /// Searches started (non-empty, valid queries).
    pub started: u64,
    /// Searches that ran to the end (or to [`MAX_MATCHES`]).
    pub completed: u64,
    /// Searches told to stop while running (their background jobs see the
    /// cancel flag).
    pub cancelled: u64,
}

/// Find across all files for one review tab.
pub struct FindBar {
    viewport: Entity<DiffViewport>,
    /// Where the keyboard goes when the bar closes.
    diff_focus: FocusHandle,
    input: Entity<InputState>,
    open: bool,
    query: String,
    options: FindOptions,
    /// Why the query is not searched (an invalid pattern).
    error: Option<String>,
    /// The compiled query (`None` when empty or invalid): the diff's marks.
    regex: Option<Regex>,
    matches: Vec<FindMatch>,
    rows: Vec<ListRow>,
    /// Matches per file.
    file_counts: HashMap<u32, usize>,
    current: Option<usize>,
    searching: bool,
    /// Files searched so far, of `total`.
    searched: usize,
    total: usize,
    /// The search stopped at [`MAX_MATCHES`].
    capped: bool,
    /// Tells the running search's background jobs to stop.
    cancel: Arc<AtomicBool>,
    /// The diff options and file list the results were found with (`None`
    /// without a search): the search runs again when the viewport's change.
    source: Option<(DiffOptions, Arc<Vec<FileChange>>)>,
    /// The display order the files were searched in.
    order: Arc<[u32]>,
    /// The running search; dropping it cancels it.
    search: Option<Task<()>>,
    /// Bumped by every new search, so a stale batch is never applied.
    generation: u64,
    stats: FindStats,
    list_scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl FindBar {
    fn new(
        viewport: Entity<DiffViewport>,
        diff_focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<FindBar>,
    ) -> FindBar {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Find in all files"));
        let rerun = cx.observe(&viewport, |bar: &mut FindBar, viewport, cx| {
            let stale = bar.source.as_ref().is_some_and(|(diff, files)| {
                let v = viewport.read(cx);
                v.options().diff != *diff || !Arc::ptr_eq(v.document().files(), files)
            });
            if stale {
                bar.restart(cx);
            }
        });
        let subscriptions = vec![
            rerun,
            cx.subscribe_in(
                &input,
                window,
                |bar: &mut FindBar, input, event: &InputEvent, _window, cx| match event {
                    InputEvent::Change => {
                        let text = input.read(cx).value().to_string();
                        bar.query_changed(text, cx);
                    }
                    InputEvent::PressEnter { shift: false, .. } => bar.next(cx),
                    InputEvent::PressEnter { shift: true, .. } => bar.prev(cx),
                    _ => {}
                },
            ),
        ];
        FindBar {
            viewport,
            diff_focus,
            input,
            open: false,
            query: String::new(),
            options: FindOptions::default(),
            error: None,
            regex: None,
            matches: Vec::new(),
            rows: Vec::new(),
            file_counts: HashMap::new(),
            current: None,
            searching: false,
            searched: 0,
            total: 0,
            capped: false,
            cancel: Arc::new(AtomicBool::new(false)),
            source: None,
            order: Arc::from([]),
            search: None,
            generation: 0,
            stats: FindStats::default(),
            list_scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether the keyboard is in the bar (its field).
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.open && self.input.focus_handle(cx).contains_focused(window, cx)
    }

    /// Shows the bar and focuses its field, the previous query selected.
    pub fn open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let was_open = std::mem::replace(&mut self.open, true);
        self.input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        if !was_open {
            self.sync_highlights(cx);
        }
        cx.notify();
    }

    /// Hides the bar (its query and results stay) and focuses the diff; the
    /// diff's find marks go away until it opens again.
    pub fn close(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open = false;
        self.sync_highlights(cx);
        window.focus(&self.diff_focus, cx);
        cx.notify();
    }

    /// Gives the diff the marks for the query while the bar is open (every
    /// match, the current one emphasized), else none.
    fn sync_highlights(&self, cx: &mut Context<Self>) {
        let highlights = match (&self.regex, self.open) {
            (Some(re), true) => {
                let re = re.clone();
                Some(FindHighlights {
                    matcher: Arc::new(move |line: &[u8]| search::line_matches(&re, line).collect()),
                    current: self.current_highlight(),
                })
            }
            _ => None,
        };
        self.viewport
            .update(cx, |v, cx| v.set_find_highlights(highlights, cx));
    }

    /// The match gone to, as the diff emphasizes it.
    fn current_highlight(&self) -> Option<FindCurrent> {
        let m = self.matches.get(self.current?)?;
        Some(FindCurrent {
            file_idx: m.file_idx,
            side: m.side,
            line: m.line,
            range: m.range.clone(),
        })
    }

    /// The find field.
    pub fn input(&self) -> &Entity<InputState> {
        &self.input
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    /// Replaces the query (and the field's text) and searches it.
    pub fn set_query(&mut self, query: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.input.read(cx).value().as_ref() != query {
            // `set_value` emits no `Change`.
            self.input.update(cx, |input, cx| {
                input.set_value(query.to_owned(), window, cx)
            });
        }
        self.query_changed(query.to_owned(), cx);
    }

    pub fn options(&self) -> FindOptions {
        self.options
    }

    pub fn set_case_sensitive(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.options.case_sensitive != on {
            self.options.case_sensitive = on;
            self.restart(cx);
        }
    }

    pub fn set_regex(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.options.regex != on {
            self.options.regex = on;
            self.restart(cx);
        }
    }

    /// The matches found so far, in display order.
    pub fn matches(&self) -> &[FindMatch] {
        &self.matches
    }

    /// The font the result list draws line numbers and previews in: the
    /// diff's code font, features included, so previews show `->` as typed
    /// when ligatures are off (settings `buffer_font.ligatures`).
    pub fn preview_font(&self, cx: &App) -> Font {
        self.viewport.read(cx).code_font().clone()
    }

    /// The result list: each file's heading, then its matches.
    pub fn list_rows(&self) -> Vec<ListRow> {
        self.rows.clone()
    }

    /// The match gone to last.
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    pub fn is_searching(&self) -> bool {
        self.searching
    }

    /// Why the query finds nothing: an invalid pattern.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    pub fn stats(&self) -> FindStats {
        self.stats
    }

    /// Whether the search stopped at [`MAX_MATCHES`].
    pub fn is_capped(&self) -> bool {
        self.capped
    }

    /// `(searched, total)` files of the running or last search.
    pub fn progress(&self) -> (usize, usize) {
        (self.searched, self.total)
    }

    /// The count shown next to the field: `3 of 12`, `12 matches`,
    /// `No matches`, `Searching…`, `Invalid pattern`, or nothing without a
    /// query.
    pub fn status_label(&self) -> String {
        if self.error.is_some() {
            return "Invalid pattern".into();
        }
        if self.query.is_empty() {
            return String::new();
        }
        let n = self.matches.len();
        if n == 0 {
            return if self.searching {
                "Searching…".into()
            } else {
                "No matches".into()
            };
        }
        let total = format!("{}{}", thousands(n), if self.capped { "+" } else { "" });
        let more = if self.searching { "…" } else { "" };
        match self.current {
            Some(i) => format!("{} of {total}{more}", thousands(i + 1)),
            None if n == 1 && !self.capped => format!("1 match{more}"),
            None => format!("{total} matches{more}"),
        }
    }

    /// `⏎`: the next match, wrapping at the end. When none was gone to
    /// yet: the first at or after the diff's cursor (else the first in the
    /// top file or after).
    pub fn next(&mut self, cx: &mut Context<Self>) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        let ix = match self.current {
            Some(i) => (i + 1) % n,
            None => self.first(true, cx),
        };
        self.go_to(ix, cx);
    }

    /// `⇧⏎`: the previous match, wrapping at the start. When none was gone
    /// to yet: the last before the diff's cursor (else the last in the top
    /// file or before).
    pub fn prev(&mut self, cx: &mut Context<Self>) {
        let n = self.matches.len();
        if n == 0 {
            return;
        }
        let ix = match self.current {
            Some(i) => (i + n - 1) % n,
            None => self.first(false, cx),
        };
        self.go_to(ix, cx);
    }

    /// Goes to match `ix`: its file expanded (and loaded when generated or
    /// large), hidden context around it revealed, the line cursor on it.
    pub fn go_to(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(m) = self.matches.get(ix).cloned() else {
            return;
        };
        self.current = Some(ix);
        if let Some(row) = self.rows.iter().position(|r| *r == ListRow::Match(ix)) {
            self.list_scroll.scroll_to_item(row, ScrollStrategy::Center);
        }
        let current = self.current_highlight();
        self.viewport.update(cx, |v, cx| {
            reveal_match(v, &m, cx);
            v.set_find_current(current, cx);
        });
        cx.notify();
    }

    /// The match the first `⏎` (`forward`) or `⇧⏎` goes to (see
    /// [`Self::next`], [`Self::prev`]), in display order; wraps when there
    /// is none that way.
    fn first(&self, forward: bool, cx: &App) -> usize {
        let last = self.matches.len().saturating_sub(1);
        let v = self.viewport.read(cx);
        let rank = |f: u32| v.display_rank(f);
        let Some(c) = v.cursor() else {
            let top = rank(v.anchor().file_idx);
            return if forward {
                self.matches.iter().position(|m| rank(m.file_idx) >= top)
            } else {
                self.matches.iter().rposition(|m| rank(m.file_idx) <= top)
            }
            .unwrap_or(if forward { 0 } else { last });
        };
        // The cursor's file is laid out, so its diff is loaded (with the
        // options the matches were found with).
        let diff = match v.document().state(c.file_idx) {
            FileState::Materialized(file) => Some(&file.diff),
            _ => None,
        };
        let at = diff.map(|fd| search::display_key(fd, c.side, c.line));
        let vs_cursor = |m: &FindMatch| {
            rank(m.file_idx)
                .cmp(&rank(c.file_idx))
                .then_with(|| match (diff, at) {
                    (Some(fd), Some(at)) => search::display_key(fd, m.side, m.line).cmp(&at),
                    _ => std::cmp::Ordering::Equal,
                })
        };
        if forward {
            self.matches
                .iter()
                .position(|m| vs_cursor(m).is_ge())
                .unwrap_or(0)
        } else {
            self.matches
                .iter()
                .rposition(|m| vs_cursor(m).is_lt())
                .unwrap_or(last)
        }
    }

    /// The viewport's display order may have changed (a new partition):
    /// a search listed in another order runs again.
    fn order_changed(&mut self, cx: &mut Context<Self>) {
        let stale = self.source.is_some() && *self.order != *self.viewport.read(cx).display_order();
        if stale {
            self.restart(cx);
        }
    }

    fn query_changed(&mut self, query: String, cx: &mut Context<Self>) {
        if query != self.query {
            self.query = query;
            self.restart(cx);
        }
    }

    /// Cancels the search in flight, clears the results and searches the
    /// current query and options.
    fn restart(&mut self, cx: &mut Context<Self>) {
        // The running jobs see the flag; the task is dropped below.
        if !self.cancel.swap(true, Ordering::Relaxed) && self.searching {
            self.stats.cancelled += 1;
        }
        self.search = None;
        self.source = None;
        self.generation += 1;
        self.matches.clear();
        self.rows.clear();
        self.file_counts.clear();
        self.current = None;
        self.error = None;
        self.searching = false;
        self.capped = false;
        self.searched = 0;
        self.total = 0;
        self.regex = None;
        match search::compile(&self.query, self.options) {
            Err(e) => self.error = Some(e),
            Ok(None) => {}
            Ok(Some(re)) => {
                self.regex = Some(re.clone());
                self.start(re, cx);
            }
        }
        self.sync_highlights(cx);
        cx.notify();
    }

    fn start(&mut self, re: Regex, cx: &mut Context<Self>) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = cancel.clone();
        let (provider, files, diff, order) = {
            let v = self.viewport.read(cx);
            (
                v.provider().clone(),
                v.document().files().clone(),
                v.options().diff,
                Arc::<[u32]>::from(v.display_order()),
            )
        };
        self.source = Some((diff, files.clone()));
        self.order = order.clone();
        let generation = self.generation;
        self.stats.started += 1;
        self.searching = true;
        self.total = files.len();
        self.search = Some(cx.spawn(async move |this, cx| {
            // Matches each job may keep: up to one past the cap, which tells
            // a capped search from one with exactly `MAX_MATCHES`.
            let mut left = MAX_MATCHES + 1;
            let mut next = 0;
            while next < order.len() {
                let mut round = Vec::with_capacity(PARALLEL);
                for _ in 0..PARALLEL {
                    if next >= order.len() {
                        break;
                    }
                    let chunk = next..(next + CHUNK).min(order.len());
                    next = chunk.end;
                    let (provider, files, order, re, cancel) = (
                        provider.clone(),
                        files.clone(),
                        order.clone(),
                        re.clone(),
                        cancel.clone(),
                    );
                    round.push(cx.background_spawn(async move {
                        let chunk = order[chunk].iter().filter_map(|&f| files.get(f as usize));
                        search::search_chunk(chunk, &*provider, &re, &diff, &cancel, left)
                    }));
                }
                let batches = futures::future::join_all(round).await;
                let searched = next;
                let kept = this
                    .update(cx, |bar, cx| {
                        let more = bar.append(generation, batches, searched, cx);
                        more.then_some(bar.matches.len())
                    })
                    .ok()
                    .flatten();
                let Some(kept) = kept else {
                    return;
                };
                left = (MAX_MATCHES + 1).saturating_sub(kept);
            }
            this.update(cx, |bar, cx| bar.finish(generation, cx)).ok();
        }));
    }

    /// Adds a round of results (in display order). Returns whether the
    /// search goes on.
    fn append(
        &mut self,
        generation: u64,
        batches: Vec<Vec<FindMatch>>,
        searched: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation != self.generation {
            return false;
        }
        self.searched = searched;
        for m in batches.into_iter().flatten() {
            if self.matches.len() >= MAX_MATCHES {
                self.capped = true;
                break;
            }
            if self.matches.last().map(|l| l.file_idx) != Some(m.file_idx) {
                self.rows.push(ListRow::File(m.file_idx));
            }
            self.rows.push(ListRow::Match(self.matches.len()));
            *self.file_counts.entry(m.file_idx).or_default() += 1;
            self.matches.push(m);
        }
        cx.notify();
        if self.capped {
            self.cancel.store(true, Ordering::Relaxed);
            self.finish(generation, cx);
            return false;
        }
        true
    }

    fn finish(&mut self, generation: u64, cx: &mut Context<Self>) {
        if generation != self.generation || !self.searching {
            return;
        }
        self.searching = false;
        self.searched = self.total;
        self.stats.completed += 1;
        cx.notify();
    }

    fn on_escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        self.close(window, cx);
    }
}

/// Brings match `m` on screen with the line cursor on it (see
/// [`FindBar::go_to`]).
fn reveal_match(v: &mut DiffViewport, m: &FindMatch, cx: &mut Context<DiffViewport>) {
    let f = m.file_idx;
    if f >= v.document().len() {
        return;
    }
    // A match in a closed category section opens it first (an explicit
    // target), so its file is shown before it is asked to load.
    if v.is_hidden(f) {
        v.scroll_to(ScrollTarget::File(f), cx);
    }
    if v.document().is_collapsed(f) {
        v.set_collapsed(f, false, cx);
    }
    // A generated file, one over the "Load diff" threshold, or one not
    // loaded yet (it may turn out large): shown whatever its size.
    let limit = v.options().large_file_changed_lines;
    let shown =
        matches!(v.document().state(f), FileState::Materialized(file) if !file.is_large(limit));
    if !shown {
        v.load_diff(f, cx);
    }
    if let Some(old) = m.old_line {
        let mut revealed: Vec<[u32; 2]> = v
            .expansions()
            .into_iter()
            .find(|(file, _)| *file == f)
            .map(|(_, ranges)| ranges)
            .unwrap_or_default();
        let hidden = match v.document().state(f) {
            FileState::Materialized(file) => search::is_hidden(&file.diff, &revealed, old),
            // Not loaded: as the search saw it, unless revealed since.
            _ => m.hidden && !revealed.iter().any(|&[s, e]| s <= old && old < e),
        };
        if hidden {
            revealed.push(search::reveal_range(old));
            v.set_expansions(f, &revealed, cx);
        }
    }
    let (side, line) = (m.side, m.line);
    if !shows_line(v, f, side, line) {
        // Lands once the file is laid out.
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: f,
                side,
                line,
            },
            cx,
        );
    }
    v.set_cursor(
        Some(CursorPos {
            file_idx: f,
            side,
            line,
            range_start: None,
        }),
        cx,
    );
}

/// Whether file `f` is laid out with a code row showing `line` of `side`.
fn shows_line(v: &DiffViewport, f: u32, side: Side, line: u32) -> bool {
    let Some(layout) = v.document().file_layout(f) else {
        return false;
    };
    let Some(r) = layout.find(RowKey::Line { side, line }) else {
        return false;
    };
    match layout.rows()[r] {
        BodyRow::Line { old, new, .. } => match side {
            Side::Old => old == Some(line),
            Side::New => new == Some(line),
        },
        _ => false,
    }
}

/// `12345` → `12,345`.
fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A file's label in the result list.
fn file_label(v: &DiffViewport, f: u32) -> (SharedString, bool) {
    v.document()
        .files()
        .get(f as usize)
        .map_or((SharedString::default(), false), |c| {
            (c.display_path().to_owned().into(), c.generated)
        })
}
