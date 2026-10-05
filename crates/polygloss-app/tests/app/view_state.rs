//! View-state persistence (T3.14, design §11.12, §7.2 `view_state`): per
//! `diff_id`, the scroll position as a line (the first line shown below the
//! pinned header, never pixels), collapsed files, revealed context, the
//! layout choice, the tree's expansion and composer text; saved on change
//! (debounced), restored when the diff is opened again.

use std::time::Duration;

use gpui_kit::{Entity, IntoElement as _, Styled as _, TestAppContext, div, px};
use polygloss_app::keymap::actions;
use polygloss_app::palette::view_toggles::{self, LayoutChoices};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::settings::SettingsStore;
use polygloss_app::tree;
use polygloss_app::view_state::{self, SAVE_DEBOUNCE, SessionViewStates};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::review::{OpenRequest, PinnedBy, ScrollAnchorState, ViewState};
use polygloss_core::store::events::Actor;
use polygloss_diff::rows::Layout;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::{LayoutMode, ScrollAnchor, ScrollTarget};

use crate::shell::{Shell, draw, start};
use crate::support::{FixtureRepo, Sandbox};

/// Lines of the fixture files.
const LINES: u32 = 160;

/// `<name> line <n>` for n in 1..=LINES, with the lines `changed` picks
/// (1-based) edited.
fn text(name: &str, changed: impl Fn(u32) -> bool) -> String {
    (1..=LINES)
        .map(|n| {
            if changed(n) {
                format!("{name} line {n} changed\n")
            } else {
                format!("{name} line {n}\n")
            }
        })
        .collect()
}

/// The lines `head` changes in `name`: every 4th of `alpha` (one long
/// hunk, no gaps), lines 5, 80 and 155 of the others (gap rows between the
/// hunks).
fn head_changes(name: &str) -> impl Fn(u32) -> bool + use<> {
    let every_4th = name == "alpha";
    move |n| {
        if every_4th {
            n % 4 == 0
        } else {
            [5, 80, 155].contains(&n)
        }
    }
}

/// A repo with tags `base` and `head` changing three long files (see
/// [`head_changes`]). Diff (and tree) order: `docs/gamma.md`,
/// `src/alpha.rs`, `src/lib/beta.rs`; directories `docs`, `src`, `src/lib`.
fn long_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    for (path, name) in PATHS {
        repo.write(path, text(name, |_| false).as_bytes());
    }
    repo.commit("base");
    repo.git(&["tag", "base"]);
    for (path, name) in PATHS {
        repo.write(path, text(name, head_changes(name)).as_bytes());
    }
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

const PATHS: [(&str, &str); 3] = [
    ("docs/gamma.md", "gamma"),
    ("src/alpha.rs", "alpha"),
    ("src/lib/beta.rs", "beta"),
];

fn compare(repo: &FixtureRepo) -> OpenRequest {
    OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/tags/head".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

/// Closes review tab `tab` with ⌘W.
fn close(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    let ix = shell
        .main
        .read_with(shell.cx, |m, _| {
            m.tabs()
                .items()
                .iter()
                .position(|t| t.review() == Some(tab))
        })
        .expect("the tab is open");
    let main = shell.main.clone();
    shell
        .cx
        .update(|window, cx| main.update(cx, |m, cx| m.activate_tab(ix, window, cx)));
    shell.cx.simulate_keystrokes("cmd-w");
    draw(shell.cx);
}

/// Forgets what this session remembers per diff, as a restart would: only
/// the store is left.
fn restart(shell: &mut Shell) {
    shell.cx.update(|_, cx| {
        cx.set_global(SessionViewStates::default());
        cx.set_global(LayoutChoices::default());
    });
}

/// Lets the debounced save run.
fn settle(shell: &mut Shell) {
    shell
        .cx
        .executor()
        .advance_clock(SAVE_DEBOUNCE + Duration::from_millis(50));
    draw(shell.cx);
}

fn scroll_to_line(shell: &mut Shell, tab: &Entity<ReviewTab>, file_idx: u32, line: u32) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx,
                side: Side::New,
                line,
            },
            cx,
        )
    });
    draw(shell.cx);
}

fn scroll_by(shell: &mut Shell, tab: &Entity<ReviewTab>, dy: f32) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| v.scroll_by(dy, cx));
    draw(shell.cx);
}

/// What the last frame painted right below the pinned header: the row
/// crossing the header's bottom edge (read from the painted rows, not from
/// the view state).
fn row_below_header(shell: &mut Shell, tab: &Entity<ReviewTab>) -> String {
    tab.read_with(shell.cx, |t, cx| {
        let v = t.viewport.read(cx);
        let header = v.document().metrics().header_height;
        let debug = v.debug();
        debug
            .visible_rows
            .iter()
            .zip(&debug.row_bounds)
            .skip(1)
            .find(|(_, (top, height))| top + height > header + 0.5)
            .map(|(row, _)| row.clone())
            .expect("a row below the header")
    })
}

/// The pinned (first painted) header's title.
fn top_header(shell: &mut Shell, tab: &Entity<ReviewTab>) -> String {
    tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).debug().visible_rows[0].clone()
    })
}

fn stored(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<ViewState> {
    let diff_id = tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone());
    shell.core.load_view_state(&diff_id).unwrap()
}

fn stored_anchor(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<ScrollAnchorState> {
    stored(shell, tab).and_then(|s| s.scroll_anchor)
}

fn updated_at(shell: &Shell) -> Vec<i64> {
    shell
        .core
        .store
        .read(|c| {
            let mut stmt = c.prepare("SELECT updated_at FROM view_state")?;
            let rows = stmt.query_map([], |r| r.get(0))?;
            Ok(rows.collect::<Result<Vec<i64>, _>>()?)
        })
        .unwrap()
}

#[gpui_kit::test]
fn reopen_restores_scroll_anchor_by_line_not_pixels(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = long_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();

    // New line 61 of `src/alpha.rs` right below its pinned header, then a
    // few pixels further: that line is partly under the header.
    scroll_to_line(&mut shell, &tab, 1, 60);
    scroll_by(&mut shell, &tab, 7.0);
    let shown = row_below_header(&mut shell, &tab);
    assert!(shown.contains("alpha line 61"), "{shown}");
    assert_eq!(top_header(&mut shell, &tab), "== src/alpha.rs");
    settle(&mut shell);

    // Stored as a line (1-based, like every stored line), never pixels.
    assert_eq!(
        stored_anchor(&mut shell, &tab),
        Some(ScrollAnchorState {
            path: "src/alpha.rs".into(),
            side: Side::New,
            line: 61,
        })
    );

    // Pixels change (a bigger font), the line does not.
    close(&mut shell, &tab);
    let settings = sb.config_dir().join("polygloss/settings.json");
    std::fs::write(&settings, r#"{ "buffer_font": { "size": 17 } }"#).unwrap();
    shell.cx.update(|_, cx| SettingsStore::reload(cx));
    draw(shell.cx);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(top_header(&mut shell, &tab), "== src/alpha.rs");
    assert!(row_below_header(&mut shell, &tab).contains("alpha line 61"));
    // Fully shown now: the line's top is the header's bottom.
    tab.read_with(shell.cx, |t, cx| {
        let v = t.viewport.read(cx);
        assert_eq!(v.options().code_font_size, 17.0);
        let header = v.document().metrics().header_height;
        let debug = v.debug();
        let (ix, _) = debug
            .visible_rows
            .iter()
            .enumerate()
            .skip(1)
            .find(|(_, r)| r.contains("alpha line 61"))
            .unwrap();
        assert!((debug.row_bounds[ix].0 - header).abs() < 0.5);
    });

    // After a restart, from the store alone.
    close(&mut shell, &tab);
    restart(&mut shell);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(top_header(&mut shell, &tab), "== src/alpha.rs");
    assert!(row_below_header(&mut shell, &tab).contains("alpha line 61"));
    assert_eq!(
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).document().top_line()),
        Some((1, Side::New, 60))
    );
}

#[gpui_kit::test]
fn scroll_anchor_in_a_gap_and_at_the_top(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();

    // At the very top nothing is saved (no anchor, nothing else changed).
    settle(&mut shell);
    assert_eq!(stored(&mut shell, &tab), None);

    // A hidden line: the gap row hiding it is below the header, keyed by
    // its first hidden old line.
    scroll_to_line(&mut shell, &tab, 0, 40);
    let shown = row_below_header(&mut shell, &tab);
    assert!(shown.contains("unchanged line"), "{shown}");
    settle(&mut shell);
    let anchor = stored_anchor(&mut shell, &tab).unwrap();
    assert_eq!(anchor.path, "docs/gamma.md");
    assert_eq!(anchor.side, Side::Old);
    close(&mut shell, &tab);
    restart(&mut shell);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(top_header(&mut shell, &tab), "== docs/gamma.md");
    assert_eq!(row_below_header(&mut shell, &tab), shown);

    // Back at the top: the anchor is cleared.
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    draw(shell.cx);
    settle(&mut shell);
    assert_eq!(stored_anchor(&mut shell, &tab), None);
}

#[gpui_kit::test]
fn reopen_restores_collapsed_expanded_layout_tree(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let file_tree = tab.read_with(shell.cx, |t, _| tree::file_tree(t).cloned().unwrap());
    assert_eq!(
        file_tree.read_with(shell.cx, |t, _| t.expanded_dirs()),
        ["docs", "src", "src/lib"]
    );

    // Reveal all of `docs/gamma.md` (loaded: it is on screen), collapse
    // `src/lib/beta.rs`, choose split, collapse the `src/lib` folder.
    viewport.update(shell.cx, |v, cx| v.expand_file(0, cx));
    draw(shell.cx);
    let expansions = viewport.read_with(shell.cx, |v, _| v.expansions());
    assert_eq!(expansions.len(), 1);
    assert_eq!(expansions[0].0, 0);
    viewport.update(shell.cx, |v, cx| v.set_collapsed(2, true, cx));
    draw(shell.cx);
    shell
        .cx
        .update(|window, cx| window.dispatch_action(Box::new(actions::viewport::LayoutSplit), cx));
    draw(shell.cx);
    file_tree.update(shell.cx, |t, cx| {
        t.set_expanded_dirs(["docs".to_owned(), "src".to_owned()], cx)
    });
    draw(shell.cx);
    settle(&mut shell);

    let state = stored(&mut shell, &tab).unwrap();
    assert_eq!(state.collapsed, ["src/lib/beta.rs"]);
    assert_eq!(
        state.expanded.get("docs/gamma.md"),
        Some(&expansions[0].1),
        "{state:?}"
    );
    assert_eq!(state.expanded.len(), 1);
    assert_eq!(state.layout, Some(Layout::Split));
    assert_eq!(
        state.tree_expanded,
        Some(vec!["docs".to_owned(), "src".to_owned()])
    );

    close(&mut shell, &tab);
    restart(&mut shell);
    let tab = shell.open(compare(&repo)).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let file_tree = tab.read_with(shell.cx, |t, _| tree::file_tree(t).cloned().unwrap());
    viewport.read_with(shell.cx, |v, _| {
        assert_eq!(v.collapsed(), [2]);
        assert_eq!(v.expansions(), expansions);
        assert_eq!(v.effective_layout(), Layout::Split);
    });
    assert_eq!(
        tab.read_with(shell.cx, |t, _| view_toggles::layout_choice(t)),
        Some(LayoutMode::Split)
    );
    assert_eq!(
        file_tree.read_with(shell.cx, |t, _| t.expanded_dirs()),
        ["docs", "src"]
    );
    // Nothing changed, so nothing is written again.
    let written = updated_at(&shell);
    settle(&mut shell);
    assert_eq!(updated_at(&shell), written);

    // Every folder collapsed is a state of its own (an empty list).
    file_tree.update(shell.cx, |t, cx| t.set_expanded_dirs(Vec::new(), cx));
    draw(shell.cx);
    settle(&mut shell);
    close(&mut shell, &tab);
    restart(&mut shell);
    let tab = shell.open(compare(&repo)).unwrap();
    let file_tree = tab.read_with(shell.cx, |t, _| tree::file_tree(t).cloned().unwrap());
    assert!(
        file_tree
            .read_with(shell.cx, |t, _| t.expanded_dirs())
            .is_empty()
    );
}

#[gpui_kit::test]
fn layout_only_state_keeps_the_tree_default(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    // A state the tree never saved (T3.2's layout write, an older build).
    let diff_id = tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone());
    close(&mut shell, &tab);
    restart(&mut shell);
    let state = ViewState {
        layout: Some(Layout::Split),
        ..ViewState::default()
    };
    shell.core.save_view_state(&diff_id, &state).unwrap();
    let tab = shell.open(compare(&repo)).unwrap();
    let file_tree = tab.read_with(shell.cx, |t, _| tree::file_tree(t).cloned().unwrap());
    assert_eq!(
        file_tree.read_with(shell.cx, |t, _| t.expanded_dirs()),
        ["docs", "src", "src/lib"],
        "every folder stays expanded"
    );
    assert_eq!(
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).effective_layout()),
        Layout::Split
    );
}

#[gpui_kit::test]
fn restore_after_window_resize_keeps_line(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).effective_layout()),
        Layout::Unified
    );
    scroll_to_line(&mut shell, &tab, 1, 100);
    assert!(row_below_header(&mut shell, &tab).contains("alpha line 101"));
    settle(&mut shell);
    close(&mut shell, &tab);

    // A much wider and taller window: split, other heights, the same line.
    shell
        .cx
        .simulate_resize(gpui_kit::size(gpui_kit::px(2400.), gpui_kit::px(1300.)));
    draw(shell.cx);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).effective_layout()),
        Layout::Split
    );
    assert_eq!(top_header(&mut shell, &tab), "== src/alpha.rs");
    assert!(row_below_header(&mut shell, &tab).contains("alpha line 101"));

    // Resizing the open tab keeps it too (the viewport's anchor), and so
    // does a smaller window after a reopen.
    shell
        .cx
        .simulate_resize(gpui_kit::size(gpui_kit::px(900.), gpui_kit::px(600.)));
    draw(shell.cx);
    assert!(row_below_header(&mut shell, &tab).contains("alpha line 101"));
    settle(&mut shell);
    close(&mut shell, &tab);
    restart(&mut shell);
    let tab = shell.open(compare(&repo)).unwrap();
    assert!(row_below_header(&mut shell, &tab).contains("alpha line 101"));
}

#[gpui_kit::test]
fn view_state_saves_debounced(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(SAVE_DEBOUNCE, Duration::from_millis(500));

    scroll_to_line(&mut shell, &tab, 1, 30);
    shell
        .cx
        .executor()
        .advance_clock(Duration::from_millis(300));
    draw(shell.cx);
    assert_eq!(stored(&mut shell, &tab), None, "not before 500 ms");

    // Another change restarts the wait (trailing debounce).
    scroll_to_line(&mut shell, &tab, 1, 40);
    shell
        .cx
        .executor()
        .advance_clock(Duration::from_millis(300));
    draw(shell.cx);
    assert_eq!(stored(&mut shell, &tab), None, "the wait restarted");
    shell
        .cx
        .executor()
        .advance_clock(Duration::from_millis(250));
    draw(shell.cx);
    assert_eq!(
        stored_anchor(&mut shell, &tab).map(|a| a.line),
        Some(41),
        "the last position, once"
    );
    let written = updated_at(&shell);
    assert_eq!(written.len(), 1);

    // Nothing changes: nothing is written.
    shell.cx.executor().advance_clock(Duration::from_secs(3));
    draw(shell.cx);
    assert_eq!(updated_at(&shell), written);
}

#[gpui_kit::test]
fn closing_a_tab_saves_pending_changes_at_once(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    let diff_id = tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone());
    scroll_to_line(&mut shell, &tab, 1, 70);
    assert_eq!(stored(&mut shell, &tab), None);
    close(&mut shell, &tab);
    drop(tab);
    draw(shell.cx);
    let anchor = shell
        .core
        .load_view_state(&diff_id)
        .unwrap()
        .and_then(|s| s.scroll_anchor);
    assert_eq!(
        anchor,
        Some(ScrollAnchorState {
            path: "src/alpha.rs".into(),
            side: Side::New,
            line: 71,
        })
    );
}

#[gpui_kit::test]
fn composer_text_is_saved_with_the_view_state(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    let key = "line:src/alpha.rs:new:5";
    tab.update(shell.cx, |t, cx| {
        view_state::set_composer_text(t, key, Some("work in progress".into()), cx)
    });
    settle(&mut shell);
    assert_eq!(
        stored(&mut shell, &tab)
            .unwrap()
            .composer
            .get(key)
            .map(String::as_str),
        Some("work in progress")
    );
    // Other changes keep it.
    scroll_to_line(&mut shell, &tab, 1, 20);
    settle(&mut shell);
    assert!(stored(&mut shell, &tab).unwrap().composer.contains_key(key));

    close(&mut shell, &tab);
    restart(&mut shell);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(
        tab.read_with(shell.cx, |t, _| view_state::composer_text(t, key)
            .map(str::to_owned)),
        Some("work in progress".to_owned())
    );
    tab.update(shell.cx, |t, cx| {
        view_state::set_composer_text(t, key, None, cx)
    });
    settle(&mut shell);
    assert!(stored(&mut shell, &tab).unwrap().composer.is_empty());
}

#[gpui_kit::test]
fn unpinned_live_state_is_saved_once_pinned(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_repo();
    // Uncommitted edits to `src/alpha.rs` alone (every even line): a live
    // review of the working tree since HEAD, of that one file.
    repo.write("src/alpha.rs", text("alpha", |n| n % 2 == 0).as_bytes());
    let mut shell = start(cx);
    let tab = shell
        .open(OpenRequest {
            source: Source::Live { since: Since::Head },
            ..compare(&repo)
        })
        .unwrap();
    let (review_id, base, live) = tab.read_with(shell.cx, |t, _| {
        assert!(t.opened.iteration.is_none(), "not pinned");
        (
            t.review_id.clone(),
            t.opened.base.clone(),
            t.opened.live.clone().unwrap(),
        )
    });

    scroll_to_line(&mut shell, &tab, 0, 100);
    settle(&mut shell);
    // No `diffs` row: nothing stored, nothing pinned just for this, no error.
    assert_eq!(stored(&mut shell, &tab), None);
    assert!(shell.core.iterations(&review_id).unwrap().is_empty());
    assert!(shell.main.read_with(shell.cx, |m, _| m.toasts().is_empty()));

    // This session still remembers it.
    close(&mut shell, &tab);
    let tab = shell
        .open(OpenRequest {
            source: Source::Live { since: Since::Head },
            ..compare(&repo)
        })
        .unwrap();
    assert!(row_below_header(&mut shell, &tab).contains("alpha line 101"));

    // Once pinned (a comment, T3.10), the state is written.
    shell
        .core
        .pin_live_on_base(&review_id, &base, &live, PinnedBy::Comment, &Actor::human())
        .unwrap();
    tab.update(shell.cx, view_state::save_now);
    draw(shell.cx);
    assert_eq!(
        stored_anchor(&mut shell, &tab).map(|a| (a.path, a.line)),
        Some(("src/alpha.rs".to_owned(), 101))
    );
}

/// Sets a 72 pt prelude on `tab`'s viewport, standing in for the header
/// card (T6.13).
fn set_prelude(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let prelude: polygloss_viewport::RenderBlock =
        std::rc::Rc::new(|_, _| div().h(px(72.)).into_any_element());
    viewport.update(shell.cx, |v, cx| v.set_prelude(Some(prelude), cx));
    draw(shell.cx);
}

fn top_line(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<(u32, Side, u32)> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).document().top_line())
}

#[gpui_kit::test]
fn view_state_top_line_round_trips_with_cards(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = long_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare(&repo)).unwrap();
    set_prelude(&mut shell, &tab);

    // On cards, below a prelude: the line right below the pinned header is
    // what is saved and restored.
    scroll_to_line(&mut shell, &tab, 1, 60);
    assert_eq!(top_line(&mut shell, &tab), Some((1, Side::New, 60)));
    assert!(row_below_header(&mut shell, &tab).contains("alpha line 61"));
    settle(&mut shell);
    let saved = ScrollAnchorState {
        path: "src/alpha.rs".into(),
        side: Side::New,
        line: 61,
    };
    assert_eq!(stored_anchor(&mut shell, &tab), Some(saved));
    close(&mut shell, &tab);
    let tab = shell.open(compare(&repo)).unwrap();
    assert_eq!(top_line(&mut shell, &tab), Some((1, Side::New, 60)));
    // The header card loading late does not move it.
    set_prelude(&mut shell, &tab);
    assert_eq!(top_line(&mut shell, &tab), Some((1, Side::New, 60)));
    assert!(row_below_header(&mut shell, &tab).contains("alpha line 61"));

    // 30 pt into the 72 pt prelude: the review is at its header card, so
    // no anchor is saved and it reopens at the top.
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| {
        v.scroll_to_anchor(ScrollAnchor::default(), cx);
        v.scroll_by(30.0, cx);
    });
    draw(shell.cx);
    assert_eq!(
        viewport.read_with(shell.cx, |v, _| v.document().scroll_top()),
        30.0
    );
    settle(&mut shell);
    assert_eq!(stored_anchor(&mut shell, &tab), None);
    close(&mut shell, &tab);
    // With the header card back, the top shows it (a line saved for the
    // first file would put that line below the pinned header instead).
    let tab = shell.open(compare(&repo)).unwrap();
    set_prelude(&mut shell, &tab);
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    assert_eq!(
        viewport.read_with(shell.cx, |v, _| v.document().scroll_top()),
        0.0
    );
}
