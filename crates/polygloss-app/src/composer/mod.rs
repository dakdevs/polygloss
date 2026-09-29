//! Composer and drafts (design §8.2, §8.3, §8.7, OQ-31, OQ-P16).
//!
//! - `c` (`viewport::Comment` → `ViewportEvent::CommentRequested`, T3.8)
//!   opens a composer under the cursor's line or range; the file header's
//!   ⋯ "Comment on file" (`FileCommentRequested`) and `tab::CommentOnFile`
//!   one under the file header; the threads panel's "Comment on review" and
//!   `tab::CommentOnReview` one at the top of the panel; a thread's "Reply…"
//!   box one in its card; a comment's Edit button one in place of its body.
//! - `⌘⏎` saves a **draft** ([`draft_store::save`]: `Core::create_thread`,
//!   `reply` or `edit_comment`; on a live diff a new thread first pins the
//!   state shown, `PinnedBy::Comment`), `Esc` cancels. Unsaved text is
//!   autosaved in the view state (`view_state::set_composer_text`, keyed by
//!   [`ComposerKey`]) and composers with saved text open again with the tab.
//! - Resolve and unresolve act at once (`Core::set_resolved`, OQ-10); own
//!   (human) comments can be edited and deleted.
//! - A thread owned by another review, shown here through its
//!   `origin_diff_id` (design §8.6), offers no reply composer: a draft would
//!   belong to that review's submission (OQ-P16). Its card offers "Reply in
//!   <that review>", which opens that review's tab; resolve stays.
//!
//! [`Composers`] (one per tab, a tab extension) holds the open composers;
//! line and file composers are viewport blocks next to the threads
//! (`ReviewThreads::set_extra_blocks`), the others render inside a thread
//! card ([`card_footer`], [`comment_editor`]) or the threads panel
//! ([`review_composer`]).

pub mod draft_store;
pub mod view;

use std::collections::HashMap;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    MenuItem, ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, WeakEntity, Window, div, px,
};
use polygloss_core::review::{AuthorKind, CommentView, OpenRequest, ThreadStatus, ThreadView};
use polygloss_core::store::events::Actor;
use polygloss_diff::Side;
use polygloss_viewport::{BlockAnchor, BlockSpec, ViewportEvent};

pub use draft_store::{ComposerKey, DiffShown, SaveRequest, Saved};
pub use view::{Composer, ComposerEvent};

use crate::app_state::AppState;
use crate::keymap::actions::tab as tab_actions;
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;
use crate::threads::{self, ReviewThreads, ThreadsEvent};
use crate::window::MenuKind;

/// Registers "Comment on file" and "Comment on review" (palette, Review
/// menu).
pub fn init(cx: &mut App) {
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::CommentOnFile, window, cx| {
            comment_on_current_file(tab, window, cx);
        },
    );
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &tab_actions::CommentOnReview, window, cx| {
            open_review_composer(tab, window, cx);
        },
    );
    crate::window::add_menu_items(
        MenuKind::Review,
        vec![
            MenuItem::separator(),
            MenuItem::action("Comment on File", tab_actions::CommentOnFile),
            MenuItem::action("Comment on Review", tab_actions::CommentOnReview),
        ],
        cx,
    );
}

/// Another review a thread shown here belongs to (OQ-P16).
#[derive(Debug, Clone)]
pub enum OtherReview {
    Loading,
    /// Its title (as Home shows it) and how to open it.
    Known {
        title: String,
        open: Option<OpenRequest>,
    },
    /// It is gone (pruned).
    Missing,
}

/// One open composer.
struct Open {
    key: ComposerKey,
    view: Entity<Composer>,
    /// Line and file composers: their block's file and anchor.
    block: Option<(u32, BlockAnchor)>,
    /// Saved: closes once the threads show the save (the reload count it
    /// waits past).
    saved_at_load: Option<u32>,
    _subscription: Subscription,
}

/// A review tab's open composers.
pub struct Composers {
    tab: WeakEntity<ReviewTab>,
    open: Vec<Open>,
    others: HashMap<String, OtherReview>,
}

impl Composers {
    /// The composer for `key`, when open.
    pub fn get(&self, key: &ComposerKey) -> Option<&Entity<Composer>> {
        self.open.iter().find(|o| &o.key == key).map(|o| &o.view)
    }

    /// Every open composer's key, in opening order.
    pub fn keys(&self) -> Vec<ComposerKey> {
        self.open.iter().map(|o| o.key.clone()).collect()
    }

    /// The tab these composers belong to.
    pub fn tab(&self) -> &WeakEntity<ReviewTab> {
        &self.tab
    }

    /// What is known about another review (OQ-P16).
    pub fn other_review(&self, review_id: &str) -> Option<&OtherReview> {
        self.others.get(review_id)
    }
}

/// The tab's [`Composers`].
pub fn composers(tab: &ReviewTab) -> Option<&Entity<Composers>> {
    tab.extension::<Entity<Composers>>()
}

/// The composer for `key` in `tab`, when open.
pub fn composer(tab: &ReviewTab, key: &ComposerKey, cx: &App) -> Option<Entity<Composer>> {
    composers(tab)?.read(cx).get(key).cloned()
}

/// Sets composers up on a new review tab: `c` and "Comment on file" from
/// the viewport, the threads' cards, and the composers whose text was
/// autosaved (opened again once the tab is set up).
pub fn attach(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let weak = cx.weak_entity();
    let entity = cx.new(|_| Composers {
        tab: weak,
        open: Vec::new(),
        others: HashMap::new(),
    });
    cx.subscribe_in(
        &tab.viewport,
        window,
        |tab: &mut ReviewTab, _, event: &ViewportEvent, window, cx| match *event {
            ViewportEvent::CommentRequested {
                file_idx,
                side,
                start_line,
                line,
            } => open_line(tab, file_idx, side, start_line, line, window, cx),
            ViewportEvent::FileCommentRequested(file_idx) => {
                open_file(tab, file_idx, window, cx);
            }
            _ => {}
        },
    )
    .detach();
    if let Some(model) = threads::threads(tab).cloned() {
        model.update(cx, |m, _| m.set_composers(entity.downgrade()));
        cx.subscribe_in(
            &model,
            window,
            |tab: &mut ReviewTab, _, event: &ThreadsEvent, window, cx| match event {
                ThreadsEvent::Changed => threads_changed(tab, window, cx),
            },
        )
        .detach();
    }
    tab.insert_extension(entity);
    // The view state is restored after every feature attached.
    cx.defer_in(window, restore_autosaved);
}

/// Opens the composers whose unsaved text the view state kept.
fn restore_autosaved(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let saved = crate::view_state::snapshot(tab, cx).composer;
    for (key, text) in saved {
        if text.trim().is_empty() {
            continue;
        }
        let Some(key) = ComposerKey::parse(&key) else {
            continue;
        };
        let block = match &key {
            ComposerKey::Line {
                path, side, line, ..
            } => file_index(tab, path).map(|f| {
                (
                    f,
                    BlockAnchor::Line {
                        side: *side,
                        line: line - 1,
                    },
                )
            }),
            ComposerKey::File { path } => file_index(tab, path).map(|f| (f, BlockAnchor::FileTop)),
            _ => None,
        };
        if key.path().is_some() && block.is_none() {
            // Its file is not in this diff any more.
            continue;
        }
        open(tab, key, block, None, false, window, cx);
    }
}

fn file_index(tab: &ReviewTab, path: &str) -> Option<u32> {
    tab.opened
        .files
        .iter()
        .position(|f| f.display_path() == path)
        .map(|i| i as u32)
}

/// Opens (or focuses) the composer for lines `start_line..=line` (0-based,
/// the viewport's) of `side` in file `file_idx`.
pub fn open_line(
    tab: &mut ReviewTab,
    file_idx: u32,
    side: Side,
    start_line: u32,
    line: u32,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let Some(file) = tab.opened.files.get(file_idx as usize) else {
        return;
    };
    let key = ComposerKey::line(file.display_path(), side, start_line, line);
    let anchor = BlockAnchor::Line {
        side,
        line: start_line.max(line),
    };
    open(tab, key, Some((file_idx, anchor)), None, true, window, cx);
}

/// Opens (or focuses) the file-level composer of file `file_idx`, under its
/// header (OQ-31).
pub fn open_file(
    tab: &mut ReviewTab,
    file_idx: u32,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let Some(file) = tab.opened.files.get(file_idx as usize) else {
        return;
    };
    let key = ComposerKey::File {
        path: file.display_path().to_owned(),
    };
    tab.viewport
        .update(cx, |v, cx| v.set_collapsed(file_idx, false, cx));
    open(
        tab,
        key,
        Some((file_idx, BlockAnchor::FileTop)),
        None,
        true,
        window,
        cx,
    );
    tab.viewport.update(cx, |v, cx| {
        v.scroll_to(polygloss_viewport::ScrollTarget::File(file_idx), cx)
    });
}

/// `tab::CommentOnFile`: the file of the cursor, else the file at the top.
pub fn comment_on_current_file(
    tab: &mut ReviewTab,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let file_idx = {
        let v = tab.viewport.read(cx);
        v.cursor()
            .map(|c| c.file_idx)
            .unwrap_or(v.anchor().file_idx)
    };
    open_file(tab, file_idx, window, cx);
}

/// Opens (or focuses) the review-level composer at the top of the threads
/// panel, showing the panel (OQ-31).
pub fn open_review_composer(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    if !tab.threads_panel_visible() {
        tab.toggle_threads_panel(cx);
    }
    open(tab, ComposerKey::Review, None, None, true, window, cx);
}

/// Opens a reply composer in thread `thread_id`'s card. Refused (`false`)
/// for a thread of another review (OQ-P16) or one not loaded.
pub fn open_reply(
    tab: &mut ReviewTab,
    thread_id: &str,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> bool {
    let Some(model) = threads::threads(tab).cloned() else {
        return false;
    };
    let own = model
        .read(cx)
        .thread(thread_id)
        .is_some_and(|t| t.review_id.as_deref() == Some(tab.review_id.as_str()));
    if !own {
        return false;
    }
    let key = ComposerKey::Reply {
        thread_id: thread_id.to_owned(),
    };
    open(tab, key, None, None, true, window, cx);
    true
}

/// Opens an edit composer in place of comment `comment_id`'s body (one's
/// own comments only). `false` when it is not an editable comment.
pub fn open_edit(
    tab: &mut ReviewTab,
    comment_id: &str,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) -> bool {
    let Some(model) = threads::threads(tab).cloned() else {
        return false;
    };
    let Some(comment) = find_comment(model.read(cx), comment_id).map(|(_, c)| c.clone()) else {
        return false;
    };
    if !own_comment(&comment) {
        return false;
    }
    let key = ComposerKey::Edit {
        comment_id: comment_id.to_owned(),
    };
    open(tab, key, None, Some(comment.body_md), true, window, cx);
    true
}

/// Opens the composer for `key` (or focuses the open one): its text is the
/// autosaved one, else `initial`.
fn open(
    tab: &mut ReviewTab,
    key: ComposerKey,
    block: Option<(u32, BlockAnchor)>,
    initial: Option<String>,
    focus: bool,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let Some(entity) = composers(tab).cloned() else {
        return;
    };
    if let Some(view) = entity.read(cx).get(&key).cloned() {
        if focus {
            let handle = view.read(cx).focus_target(cx);
            window.focus(&handle, cx);
        }
        return;
    }
    let text = crate::view_state::composer_text(tab, &key.to_string())
        .map(str::to_owned)
        .or(initial)
        .unwrap_or_default();
    let view = cx.new(|cx| Composer::new(key.clone(), &text, window, cx));
    let subscription = cx.subscribe_in(
        &view,
        window,
        |tab: &mut ReviewTab, view, event: &ComposerEvent, window, cx| {
            let key = view.read(cx).key().clone();
            match event {
                ComposerEvent::Save => save(tab, &key, window, cx),
                ComposerEvent::Cancel => cancel(tab, &key, window, cx),
                ComposerEvent::Changed => autosave(tab, &key, cx),
            }
        },
    );
    entity.update(cx, |c, cx| {
        c.open.push(Open {
            key: key.clone(),
            view: view.clone(),
            block,
            saved_at_load: None,
            _subscription: subscription,
        });
        cx.notify();
    });
    push_blocks(tab, cx);
    refresh_card(tab, &key, cx);
    if focus {
        let handle = view.read(cx).focus_target(cx);
        window.focus(&handle, cx);
    }
    cx.notify();
}

/// Keeps the composer's unsaved text in the view state (debounced there).
fn autosave(tab: &mut ReviewTab, key: &ComposerKey, cx: &mut Context<ReviewTab>) {
    let Some(view) = composer(tab, key, cx) else {
        return;
    };
    let text = view.read(cx).text(cx);
    let text = (!text.trim().is_empty()).then_some(text);
    crate::view_state::set_composer_text(tab, &key.to_string(), text, cx);
}

/// `Esc` / Cancel: closes the composer and drops its text.
pub fn cancel(
    tab: &mut ReviewTab,
    key: &ComposerKey,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    crate::view_state::set_composer_text(tab, &key.to_string(), None, cx);
    close(tab, key, window, cx);
}

/// Removes the composer for `key`; the diff gets the keyboard back when
/// the composer had it.
fn close(tab: &mut ReviewTab, key: &ComposerKey, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let Some(entity) = composers(tab).cloned() else {
        return;
    };
    let Some(view) = entity.read(cx).get(key).cloned() else {
        return;
    };
    let had_focus = view.read(cx).contains_focus(window, cx);
    entity.update(cx, |c, cx| {
        c.open.retain(|o| &o.key != key);
        cx.notify();
    });
    push_blocks(tab, cx);
    refresh_card(tab, key, cx);
    if had_focus {
        window.focus(&tab.viewport_focus().clone(), cx);
    }
    cx.notify();
}

/// What the tab shows, for a save.
fn shown(tab: &ReviewTab) -> DiffShown {
    DiffShown {
        review_id: tab.review_id.clone(),
        diff_id: tab.opened.diff_id.clone(),
        repo: tab.opened.repo.clone(),
        live: tab
            .opened
            .live
            .clone()
            .map(|state| (tab.opened.base.clone(), state)),
        scratch: tab.opened.live.as_ref().map(|l| l.scratch_objects.clone()),
    }
}

/// `⌘⏎` / Save: writes the draft on the background executor; the composer
/// closes once the threads show it.
pub fn save(
    tab: &mut ReviewTab,
    key: &ComposerKey,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let Some(view) = composer(tab, key, cx) else {
        return;
    };
    if view.read(cx).is_saving() {
        return;
    }
    let body = view.read(cx).text(cx);
    if body.trim().is_empty() {
        view.update(cx, |v, cx| {
            v.set_error(Some("Write a comment first".to_owned()), cx)
        });
        return;
    }
    view.update(cx, |v, cx| v.set_saving(true, cx));
    let req = SaveRequest {
        key: key.clone(),
        body,
        shown: shown(tab),
    };
    let core = AppState::global(cx).core.clone();
    let work = cx.background_spawn(async move { draft_store::save(&core, &req) });
    let key = key.clone();
    cx.spawn_in(window, async move |tab, cx| {
        let result = work.await;
        tab.update_in(cx, |tab, window, cx| saved(tab, &key, result, window, cx))
            .ok();
    })
    .detach();
}

/// A save finished: on success the text is dropped, a pinned live state
/// becomes the tab's iteration, and the threads reload (the composer closes
/// when they show the save); on failure the composer says why.
fn saved(
    tab: &mut ReviewTab,
    key: &ComposerKey,
    result: anyhow::Result<Saved>,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let Some(view) = composer(tab, key, cx) else {
        return;
    };
    match result {
        Ok(saved) => {
            crate::view_state::set_composer_text(tab, &key.to_string(), None, cx);
            if let Some(it) = saved.pinned {
                if it.diff_id == tab.opened.diff_id {
                    tab.opened.iteration = Some(it);
                }
                crate::view_state::save_now(tab, cx);
            }
            let Some(model) = threads::threads(tab).cloned() else {
                close(tab, key, window, cx);
                return;
            };
            let loads = model.read(cx).stats().loads;
            if let Some(entity) = composers(tab).cloned() {
                entity.update(cx, |c, _| {
                    if let Some(o) = c.open.iter_mut().find(|o| &o.key == key) {
                        o.saved_at_load = Some(loads);
                    }
                });
            }
            // The diff gets the keyboard back now, not when the composer
            // goes.
            if view.read(cx).contains_focus(window, cx) {
                window.focus(&tab.viewport_focus().clone(), cx);
            }
            threads::reload(tab, cx);
            cx.notify();
        }
        Err(err) => {
            tracing::warn!("saving the {key} draft: {err:#}");
            view.update(cx, |v, cx| {
                v.set_error(Some(format!("Could not save: {err:#}")), cx)
            });
        }
    }
}

/// The threads were loaded again: saved composers close (their thread or
/// comment shows now), composers whose thread or comment is gone (or whose
/// thread is another review's) close,
/// and other reviews' threads get their review's title.
fn threads_changed(tab: &mut ReviewTab, window: &mut Window, cx: &mut Context<ReviewTab>) {
    let (Some(entity), Some(model)) = (composers(tab).cloned(), threads::threads(tab).cloned())
    else {
        return;
    };
    let (loads, loaded) = {
        let m = model.read(cx);
        (m.stats().loads, m.is_loaded())
    };
    let mut done: Vec<ComposerKey> = Vec::new();
    let mut gone: Vec<ComposerKey> = Vec::new();
    {
        let c = entity.read(cx);
        let m = model.read(cx);
        for o in &c.open {
            if o.saved_at_load.is_some_and(|at| loads != at) {
                done.push(o.key.clone());
                continue;
            }
            let exists = match &o.key {
                // A reply autosaved in another review's tab on the same diff
                // (its view state is per diff) is not this review's to
                // draft (OQ-P16).
                ComposerKey::Reply { thread_id } => m
                    .thread(thread_id)
                    .is_some_and(|t| t.review_id.as_deref() == Some(tab.review_id.as_str())),
                ComposerKey::Edit { comment_id } => find_comment(m, comment_id).is_some(),
                _ => true,
            };
            if loaded && !exists && o.saved_at_load.is_none() {
                gone.push(o.key.clone());
            }
        }
    }
    for key in done {
        close(tab, &key, window, cx);
    }
    for key in gone {
        cancel(tab, &key, window, cx);
    }
    load_other_reviews(tab, &entity, &model, cx);
}

/// Looks up the reviews that own threads shown here but are not this one
/// (their title for "Reply in <review>"), once each.
fn load_other_reviews(
    tab: &ReviewTab,
    entity: &Entity<Composers>,
    model: &Entity<ReviewThreads>,
    cx: &mut Context<ReviewTab>,
) {
    let wanted: Vec<String> = {
        let c = entity.read(cx);
        let mut ids: Vec<String> = model
            .read(cx)
            .threads()
            .filter_map(|t| t.review_id.clone())
            .filter(|id| *id != tab.review_id)
            .filter(|id| !c.others.contains_key(id))
            .collect();
        ids.sort();
        ids.dedup();
        ids
    };
    if wanted.is_empty() {
        return;
    }
    entity.update(cx, |c, _| {
        for id in &wanted {
            c.others.insert(id.clone(), OtherReview::Loading);
        }
    });
    let core = AppState::global(cx).core.clone();
    let work = cx.background_spawn(async move {
        wanted
            .into_iter()
            .map(|id| {
                let found = match core.review_summary(&id) {
                    Ok(Some(summary)) => {
                        // A commit review is named by its subject, as on Home.
                        let subject = match crate::home::row::parse_key(&summary.key) {
                            Some(crate::home::row::ParsedKey::Commit { oid }) => {
                                crate::home::commit_subject(&summary.repo_path, &oid)
                            }
                            _ => None,
                        };
                        OtherReview::Known {
                            title: crate::home::row::title(&summary, subject.as_deref()),
                            open: crate::home::row::open_request(&summary),
                        }
                    }
                    Ok(None) => OtherReview::Missing,
                    Err(e) => {
                        tracing::warn!("looking up review {id}: {e}");
                        OtherReview::Missing
                    }
                };
                (id, found)
            })
            .collect::<Vec<_>>()
    });
    let (entity, model) = (entity.downgrade(), model.downgrade());
    cx.spawn(async move |_, cx| {
        let found = work.await;
        let ids: Vec<String> = found.iter().map(|(id, _)| id.clone()).collect();
        entity
            .update(cx, |c, cx| {
                c.others.extend(found);
                cx.notify();
            })
            .ok();
        model
            .update(cx, |m, cx| {
                let threads: Vec<String> = m
                    .threads()
                    .filter(|t| t.review_id.as_ref().is_some_and(|r| ids.contains(r)))
                    .map(|t| t.id.clone())
                    .collect();
                for id in threads {
                    m.invalidate_thread(&id, cx);
                }
            })
            .ok();
    })
    .detach();
}

/// Gives the threads model the line and file composers' blocks.
fn push_blocks(tab: &ReviewTab, cx: &mut Context<ReviewTab>) {
    let (Some(entity), Some(model)) = (composers(tab).cloned(), threads::threads(tab).cloned())
    else {
        return;
    };
    let blocks: Vec<(u32, BlockSpec)> = entity
        .read(cx)
        .open
        .iter()
        .filter_map(|o| {
            let (file, anchor) = o.block?;
            let view = o.view.clone();
            Some((
                file,
                BlockSpec {
                    id: o.key.block_id(),
                    anchor,
                    render: Rc::new(move |_, _| composer_block(&view)),
                },
            ))
        })
        .collect();
    model.update(cx, |m, cx| m.set_extra_blocks(blocks, cx));
}

/// A line or file composer as a viewport block, inset like a thread card.
fn composer_block(view: &Entity<Composer>) -> AnyElement {
    div()
        .w_full()
        .px(px(10.))
        .py(px(6.))
        // Clicks stay in the composer; the wheel still scrolls the diff.
        .block_mouse_except_scroll()
        .child(view.clone())
        .into_any_element()
}

/// A reply or edit composer opened or closed inside a thread card: the
/// card is measured again.
fn refresh_card(tab: &ReviewTab, key: &ComposerKey, cx: &mut Context<ReviewTab>) {
    let Some(model) = threads::threads(tab).cloned() else {
        return;
    };
    let thread = match key {
        ComposerKey::Reply { thread_id } => Some(thread_id.clone()),
        ComposerKey::Edit { comment_id } => {
            find_comment(model.read(cx), comment_id).map(|(t, _)| t.id.clone())
        }
        _ => None,
    };
    if let Some(id) = thread {
        model.update(cx, |m, cx| m.invalidate_thread(&id, cx));
    }
}

/// The comment `comment_id` and its thread.
fn find_comment<'a>(
    m: &'a ReviewThreads,
    comment_id: &str,
) -> Option<(&'a ThreadView, &'a CommentView)> {
    m.threads().find_map(|t| {
        t.comments
            .iter()
            .find(|c| c.id == comment_id)
            .map(|c| (t, c))
    })
}

/// A comment the human may edit and delete: a human's, not deleted
/// (humans own every human comment, T1.13).
fn own_comment(c: &CommentView) -> bool {
    c.author.kind == AuthorKind::Human && !c.deleted
}

/// Resolves or unresolves thread `thread_id` at once (not a draft, OQ-10).
pub fn set_resolved(
    _tab: &mut ReviewTab,
    thread_id: &str,
    resolved: bool,
    cx: &mut Context<ReviewTab>,
) {
    let core = AppState::global(cx).core.clone();
    let id = thread_id.to_owned();
    let work =
        cx.background_spawn(async move { core.set_resolved(&id, resolved, &Actor::human(), None) });
    cx.spawn(async move |tab, cx| {
        let result = work.await;
        tab.update(cx, |tab, cx| match result {
            Ok(()) => threads::reload(tab, cx),
            Err(e) => {
                let verb = if resolved { "resolve" } else { "unresolve" };
                toast_error(format!("Could not {verb} the thread: {e}"), cx);
            }
        })
        .ok();
    })
    .detach();
}

/// Deletes one's own comment: a draft at once, a published comment after
/// asking.
pub fn delete_comment(
    tab: &mut ReviewTab,
    comment_id: &str,
    window: &mut Window,
    cx: &mut Context<ReviewTab>,
) {
    let Some(model) = threads::threads(tab).cloned() else {
        return;
    };
    let Some(comment) = find_comment(model.read(cx), comment_id).map(|(_, c)| c.clone()) else {
        return;
    };
    if !own_comment(&comment) {
        return;
    }
    if comment.draft {
        delete_comment_now(tab, comment_id, cx);
        return;
    }
    let weak = cx.weak_entity();
    let id = comment_id.to_owned();
    window.open_alert_dialog(cx, move |alert, _, _| {
        let (weak, id) = (weak.clone(), id.clone());
        alert
            .title("Delete this comment?")
            .description("Agents have seen it; a thread with replies keeps a “Comment deleted” line in its place.")
            .ok_text("Delete")
            .ok_variant(ButtonVariant::Danger)
            .show_cancel(true)
            .on_ok(move |_, _, cx| {
                weak.update(cx, |tab, cx| delete_comment_now(tab, &id, cx)).ok();
                true
            })
    });
}

/// Deletes comment `comment_id` without asking.
pub fn delete_comment_now(_tab: &mut ReviewTab, comment_id: &str, cx: &mut Context<ReviewTab>) {
    let core = AppState::global(cx).core.clone();
    let id = comment_id.to_owned();
    let work = cx.background_spawn(async move { core.delete_comment(&id, &draft_store::human()) });
    let edit = ComposerKey::Edit {
        comment_id: comment_id.to_owned(),
    };
    cx.spawn(async move |tab, cx| {
        let result = work.await;
        tab.update(cx, |tab, cx| match result {
            Ok(_) => {
                crate::view_state::set_composer_text(tab, &edit.to_string(), None, cx);
                threads::reload(tab, cx);
            }
            Err(e) => toast_error(format!("Could not delete the comment: {e}"), cx),
        })
        .ok();
    })
    .detach();
}

/// Opens (or focuses) the tab of review `review_id` ("Reply in <review>",
/// OQ-P16).
pub fn open_other_review(
    review_id: &str,
    open: Option<OpenRequest>,
    window: &mut Window,
    cx: &mut App,
) {
    if let Some((_, main)) = crate::window::main_window(cx) {
        let ix = main.read(cx).tabs().find_review(review_id, cx);
        if let Some(ix) = ix {
            main.update(cx, |main, cx| main.activate_tab(ix, window, cx));
            return;
        }
    }
    match open {
        Some(req) => crate::review_tab::open_review(req, window, cx).detach(),
        None => toast_error_app(
            "That review cannot be opened from here".to_owned(),
            window,
            cx,
        ),
    }
}

fn toast_error(message: String, cx: &mut App) {
    let Some((handle, main)) = crate::window::main_window(cx) else {
        return;
    };
    handle
        .update(cx, |_, window, cx| {
            main.update(cx, |main, cx| main.toast_error(message.into(), window, cx))
        })
        .ok();
}

fn toast_error_app(message: String, window: &mut Window, cx: &mut App) {
    if let Some((_, main)) = crate::window::main_window(cx) {
        main.update(cx, |main, cx| main.toast_error(message.into(), window, cx));
    }
}

/// The review composer, for the top of the threads panel.
pub fn review_composer(model: &ReviewThreads, cx: &App) -> Option<AnyElement> {
    let entity = model.composers()?;
    let view = entity.read(cx).get(&ComposerKey::Review)?.clone();
    Some(
        div()
            .px_2()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(view)
            .into_any_element(),
    )
}

/// Comment `c`'s body while it is being edited: the edit composer.
pub fn comment_editor(model: &ReviewThreads, c: &CommentView, cx: &App) -> Option<AnyElement> {
    let entity = model.composers()?;
    let key = ComposerKey::Edit {
        comment_id: c.id.clone(),
    };
    let view = entity.read(cx).get(&key)?.clone();
    Some(div().pt_1().child(view).into_any_element())
}

/// The Edit and Delete buttons of one's own comment (none while it is
/// being edited).
pub fn comment_tools(model: &ReviewThreads, c: &CommentView, cx: &App) -> Option<AnyElement> {
    if !own_comment(c) {
        return None;
    }
    let entity = model.composers()?;
    let composers = entity.read(cx);
    let editing = composers
        .get(&ComposerKey::Edit {
            comment_id: c.id.clone(),
        })
        .is_some();
    if editing {
        return None;
    }
    let tab = composers.tab().clone();
    let (edit_id, delete_id) = (c.id.clone(), c.id.clone());
    let tab2 = tab.clone();
    Some(
        h_flex()
            .gap_0p5()
            .child(
                Button::new(SharedString::from(format!("comment-edit-{}", c.id)))
                    .debug_selector({
                        let id = c.id.clone();
                        move || format!("comment-edit-{id}")
                    })
                    .xsmall()
                    .ghost()
                    .label("Edit")
                    .tooltip("Edit your comment")
                    .on_click(move |_, window, cx| {
                        tab.update(cx, |tab, cx| open_edit(tab, &edit_id, window, cx))
                            .ok();
                    }),
            )
            .child(
                Button::new(SharedString::from(format!("comment-delete-{}", c.id)))
                    .debug_selector({
                        let id = c.id.clone();
                        move || format!("comment-delete-{id}")
                    })
                    .xsmall()
                    .ghost()
                    .label("Delete")
                    .tooltip("Delete your comment")
                    .on_click(move |_, window, cx| {
                        tab2.update(cx, |tab, cx| delete_comment(tab, &delete_id, window, cx))
                            .ok();
                    }),
            )
            .into_any_element(),
    )
}

/// The foot of a thread card: the reply composer when open, else the
/// "Reply…" box (or, for another review's thread, "Reply in <review>") and
/// the Resolve / Unresolve button.
pub fn card_footer(model: &ReviewThreads, thread: &ThreadView, cx: &App) -> Option<AnyElement> {
    let entity = model.composers()?;
    let composers = entity.read(cx);
    let tab = composers.tab().clone();
    let own = thread.review_id.as_deref() == Some(model.review_id());
    let id = thread.id.clone();
    let theme = cx.theme();
    if own
        && let Some(view) = composers.get(&ComposerKey::Reply {
            thread_id: id.clone(),
        })
    {
        return Some(
            div()
                .w_full()
                .px_3()
                .py_2()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.secondary)
                .child(view.clone())
                .into_any_element(),
        );
    }
    let reply: AnyElement = if own {
        let (tab, id) = (tab.clone(), id.clone());
        div()
            .id(SharedString::from(format!("thread-reply-{}", thread.id)))
            .debug_selector({
                let id = thread.id.clone();
                move || format!("thread-reply-{id}")
            })
            .flex_1()
            .min_w_0()
            .h(px(28.))
            .px_2p5()
            .flex()
            .items_center()
            .rounded(px(6.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.background)
            .text_sm()
            .text_color(theme.muted_foreground)
            .cursor_text()
            .hover(|s| s.border_color(theme.ring.opacity(0.6)))
            .child("Reply…")
            .on_click(move |_, window, cx| {
                tab.update(cx, |tab, cx| open_reply(tab, &id, window, cx))
                    .ok();
            })
            .into_any_element()
    } else {
        let review_id = thread.review_id.clone().unwrap_or_default();
        let other = composers.other_review(&review_id).cloned();
        let (label, open) = match other {
            Some(OtherReview::Known { title, open }) => (format!("Reply in {title}"), open),
            Some(OtherReview::Missing) => ("Its review is gone".to_owned(), None),
            _ => ("Reply in its review".to_owned(), None),
        };
        h_flex()
            .flex_1()
            .min_w_0()
            .child(
                Button::new(SharedString::from(format!(
                    "thread-reply-elsewhere-{}",
                    thread.id
                )))
                .debug_selector({
                    let id = thread.id.clone();
                    move || format!("thread-reply-elsewhere-{id}")
                })
                .small()
                .ghost()
                .icon(IconName::ExternalLink)
                .label(label)
                .tooltip("This thread belongs to another review: reply there")
                .on_click(move |_, window, cx| {
                    open_other_review(&review_id, open.clone(), window, cx)
                }),
            )
            .into_any_element()
    };
    // A draft thread is not visible to anyone yet: nothing to resolve.
    let resolve = (!thread.draft).then(|| {
        let resolved = thread.status == ThreadStatus::Resolved;
        let (tab, id) = (tab.clone(), id.clone());
        Button::new(SharedString::from(format!("thread-resolve-{}", thread.id)))
            .debug_selector({
                let id = thread.id.clone();
                move || format!("thread-resolve-{id}")
            })
            .small()
            .outline()
            .when(!resolved, |b| b.icon(IconName::CircleCheck))
            .label(if resolved {
                "Unresolve conversation"
            } else {
                "Resolve conversation"
            })
            .on_click(move |_, _, cx| {
                tab.update(cx, |tab, cx| set_resolved(tab, &id, !resolved, cx))
                    .ok();
            })
    });
    Some(
        h_flex()
            .w_full()
            .px_3()
            .py_2()
            .gap_2()
            .border_t_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .child(reply)
            .when_some(resolve, |el, b| el.child(b))
            .into_any_element(),
    )
}
