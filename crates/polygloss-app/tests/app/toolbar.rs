//! The review tab's toolbar (T6.8, design §11.4): the repo block, the pills
//! of each kind, the threads button, and the order in which a narrowing
//! toolbar gives way. The features' own controls are tested with them
//! (`palette`, `viewed`, `iterations`, `live`).

use std::path::Path;

use gpui_kit::{Entity, TestAppContext, VisualTestContext, px};
use polygloss_app::keymap::actions;
use polygloss_app::keymap::actions::tab as tab_actions;
use polygloss_app::palette::MenuEntry;
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::threads;
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{ThreadKind, Verdict};
use polygloss_core::store::events::Actor;
use polygloss_diff::Side;

use crate::shell::{
    Shell, bounds, click, commit_req, compare_req, draw, hover, painted, resize_window,
    set_sidebar_width, start, unhover,
};
use crate::support::{Sandbox, code_change_repo};
use crate::threads::{agent, create, human, line, reload};

/// Whether the toolbar drew label `name` saying `text` (labels are found as
/// `"<name>: <text>"`).
pub fn shows(cx: &mut VisualTestContext, name: &str, text: &str) -> bool {
    painted(cx, &format!("{name}: {text}")).is_some()
}

#[gpui_kit::test]
fn commit_toolbar_shows_the_sha_pill(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    let short = repo.git(&["rev-parse", "--short=7", "refs/tags/head"]);
    assert!(
        shows(shell.cx, "commit-pill-label", short.trim()),
        "the head commit's short id"
    );
    assert!(shows(shell.cx, "repo-name", "repo"));
    assert!(bounds(shell.cx, "repo-block").right() <= bounds(shell.cx, "commit-pill").left());
    for other in ["ref-pill-base", "ref-pill-head", "branch-pill", "live-base"] {
        assert!(
            painted(shell.cx, other).is_none(),
            "{other} is for other kinds"
        );
    }
}

#[gpui_kit::test]
fn compare_toolbar_shows_both_ref_pills_and_the_mode(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    // Direct: base..head.
    shell.open(compare_req(repo.path())).unwrap();
    assert!(shows(shell.cx, "ref-pill-base-label", "base"));
    assert!(shows(shell.cx, "compare-mode", ".."));
    assert!(shows(shell.cx, "ref-pill-head-label", "head"));
    let (base, mode, head) = (
        bounds(shell.cx, "ref-pill-base"),
        bounds(shell.cx, "compare-mode: .."),
        bounds(shell.cx, "ref-pill-head"),
    );
    assert!(base.right() <= mode.left() && mode.right() <= head.left());
    assert!(painted(shell.cx, "commit-pill").is_none());

    // Three-dot: base…head, an ellipsis between the pills.
    let mut three_dot = compare_req(repo.path());
    three_dot.source = Source::Compare {
        base: "refs/tags/base".into(),
        head: "refs/tags/head".into(),
        mode: CompareMode::ThreeDot,
    };
    shell.open(three_dot).unwrap();
    assert!(shows(shell.cx, "compare-mode", "\u{2026}"));
    assert!(!shows(shell.cx, "compare-mode", ".."));
}

#[gpui_kit::test]
fn repo_parent_path_abbreviates_home(cx: &mut TestAppContext) {
    use polygloss_app::review_tab::toolbar::parent_path;
    let home = Some(Path::new("/Users/ada"));
    assert_eq!(
        parent_path(Path::new("/Users/ada/dev/godiff"), home),
        "~/dev"
    );
    assert_eq!(parent_path(Path::new("/Users/ada/godiff"), home), "~");
    // Only whole components: /Users/adam is not under /Users/ada.
    assert_eq!(
        parent_path(Path::new("/Users/adam/godiff"), home),
        "/Users/adam"
    );
    assert_eq!(parent_path(Path::new("/opt/src/godiff"), home), "/opt/src");
    assert_eq!(parent_path(Path::new("/opt/src/godiff"), None), "/opt/src");

    // Drawn under the repo's name: `HOME` is the fixture's own temp dir,
    // which holds the repo.
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let root = repo.path().parent().expect("the fixture's root");
    // SAFETY: one test per process, before the app starts any thread.
    unsafe { std::env::set_var("HOME", root) };
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    assert!(shows(shell.cx, "repo-name", "repo"));
    assert!(shows(shell.cx, "repo-parent", "~"));
    let name = bounds(shell.cx, "repo-name: repo");
    let parent = bounds(shell.cx, "repo-parent: ~");
    assert!(
        name.bottom() <= parent.top(),
        "the parent is under the name"
    );
}

/// The toolbar items a narrow-toolbar test looks for.
const NARROW_ITEMS: [&str; 17] = [
    "repo-name: repo",
    "repo-parent: ~",
    "ref-pill-base",
    "ref-pill-base-label: base",
    "compare-mode: ..",
    "ref-pill-head",
    "ref-pill-head-label: head",
    "iteration-picker",
    "iteration-picker-label: Iteration 1 of 1",
    "iteration-picker-label: 1/1",
    "toolbar-find",
    "toggle-threads-panel",
    "viewed-progress",
    "layout-toggle",
    "view-options",
    "submit-review-label: Submit review",
    "submit-review-label: Submit",
];

/// The [`NARROW_ITEMS`] painted, each whole inside its side of the row.
fn toolbar_items(shell: &mut Shell) -> Vec<&'static str> {
    let left = bounds(shell.cx, "toolbar-left");
    let row = bounds(shell.cx, "review-toolbar");
    let mut shown = Vec::new();
    for item in NARROW_ITEMS {
        let Some(b) = painted(shell.cx, item) else {
            continue;
        };
        assert!(b.right() <= row.right(), "{item} is past the window's edge");
        if b.left() < left.right() {
            assert!(b.right() <= left.right(), "{item} is cut off");
        }
        shown.push(item);
    }
    shown
}

/// What design §11.4 leaves in this test's toolbar at step `narrow`, each
/// step taking one more thing away.
fn shown_at(narrow: polygloss_app::review_tab::toolbar::Narrow) -> Vec<&'static str> {
    use polygloss_app::review_tab::toolbar::Narrow;
    let mut items = vec![
        "repo-name: repo",
        "repo-parent: ~",
        "ref-pill-base",
        "ref-pill-base-label: base",
        "compare-mode: ..",
        "ref-pill-head",
        "ref-pill-head-label: head",
        "iteration-picker",
        "iteration-picker-label: Iteration 1 of 1",
        "toolbar-find",
        "toggle-threads-panel",
        "viewed-progress",
        "layout-toggle",
        "view-options",
        "submit-review-label: Submit review",
    ];
    let mut swap = |from: &str, to: Option<&'static str>| {
        let at = items.iter().position(|i| *i == from).expect("in the list");
        match to {
            Some(to) => items[at] = to,
            None => {
                items.remove(at);
            }
        }
    };
    if narrow >= Narrow::NoParentPath {
        swap("repo-parent: ~", None);
    }
    // ShortPills: "base" and "head" are shorter than 64 pt already.
    if narrow >= Narrow::ShortIteration {
        swap(
            "iteration-picker-label: Iteration 1 of 1",
            Some("iteration-picker-label: 1/1"),
        );
    }
    if narrow >= Narrow::ShortSubmit {
        swap(
            "submit-review-label: Submit review",
            Some("submit-review-label: Submit"),
        );
    }
    if narrow >= Narrow::FindInMenu {
        swap("toolbar-find", None);
    }
    if narrow >= Narrow::ProgressInMenu {
        swap("viewed-progress", None);
    }
    if narrow >= Narrow::IconPills {
        swap("ref-pill-base-label: base", None);
        swap("ref-pill-head-label: head", None);
        swap("iteration-picker-label: 1/1", None);
    }
    items
}

/// The narrow tests' review: a compare review with an iteration pill (a
/// submission gives it a second state to show, "Changes since last
/// review"), the threads panel hidden.
fn iteration_review(shell: &mut Shell, repo: &Path) -> Entity<ReviewTab> {
    let tab = shell.open(compare_req(repo)).unwrap();
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    shell
        .core
        .submit_review(&review, Verdict::Comment, "", None)
        .unwrap();
    tab.update(shell.cx, polygloss_app::iterations::reload);
    draw(shell.cx);
    assert!(!tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    tab
}

/// A 720 pt window with the sidebar at its 400 pt cap (a wider one
/// stored): a 320 pt main column.
fn narrowest(shell: &mut Shell) {
    resize_window(shell, 1440., 900.);
    set_sidebar_width(shell, 480.);
    resize_window(shell, 720., 900.);
    assert_eq!(bounds(shell.cx, "review-toolbar").size.width, px(320.));
}

#[gpui_kit::test]
fn narrow_toolbar_collapses_in_order(cx: &mut TestAppContext) {
    use polygloss_app::review_tab::toolbar::{self, Narrow};
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    // SAFETY: one test per process, before the app starts any thread.
    unsafe { std::env::set_var("HOME", repo.path().parent().expect("the fixture's root")) };
    let mut shell = start(cx);
    let tab = iteration_review(&mut shell, repo.path());
    let narrow = |shell: &mut Shell| tab.read_with(shell.cx, |t, _| toolbar::narrow(t));
    let menu = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            polygloss_app::features::display_menu_entries(t, cx)
        })
    };

    // Wide: everything, in design order.
    assert_eq!(narrow(&mut shell), Narrow::Full);
    assert_eq!(toolbar_items(&mut shell), shown_at(Narrow::Full));

    // A 720 pt window: the main column is 439 pt beside the 280 pt sidebar
    // and its 1 pt divider. The parent path, the long labels, Find and `N/M`
    // have given way, and the pills show their icons.
    resize_window(&mut shell, 720., 900.);
    assert_eq!(bounds(shell.cx, "review-toolbar").size.width, px(439.));
    assert_eq!(narrow(&mut shell), Narrow::IconPills);
    assert_eq!(toolbar_items(&mut shell), shown_at(Narrow::IconPills));
    // `N/M` and Find are the display options menu's first rows.
    let first: Vec<_> = menu(&mut shell).into_iter().take(3).collect();
    assert_eq!(
        first,
        [
            MenuEntry::note("0 of 3 files viewed"),
            MenuEntry::action("Find in all files", actions::tab::Find),
            MenuEntry::Separator,
        ]
    );

    // The sidebar at its 399 pt cap (and its divider) leaves 320 pt: the
    // repo name keeps at
    // most 48 pt and the four controls on the right never hide. That is
    // still too wide here, so the left side's items go from its end; the
    // repo block stays.
    narrowest(&mut shell);
    assert_eq!(narrow(&mut shell), Narrow::ShortRepo);
    assert_eq!(
        toolbar_items(&mut shell),
        [
            "repo-name: repo",
            "toggle-threads-panel",
            "layout-toggle",
            "view-options",
            "submit-review-label: Submit",
        ]
    );
    assert!(bounds(shell.cx, "repo-block").size.width <= px(48.));
    let left = bounds(shell.cx, "toolbar-left");
    assert!(left.right() <= bounds(shell.cx, "toggle-threads-panel").left());

    // From 960 pt down to 320 pt the steps only ever advance, and each step
    // shows what design §11.4 leaves at it.
    let mut last = Narrow::Full;
    let mut seen = Vec::new();
    for window in (720..=1440).rev().step_by(10) {
        resize_window(&mut shell, window as f32, 900.);
        let now = narrow(&mut shell);
        assert!(
            now >= last,
            "in a {window} pt window: {now:?} after {last:?}"
        );
        if now < Narrow::ShortRepo {
            assert_eq!(toolbar_items(&mut shell), shown_at(now), "at {now:?}");
        }
        if !seen.contains(&now) {
            seen.push(now);
        }
        last = now;
    }
    assert!(seen.len() >= 5, "the sweep met {seen:?}");
    // And back: a wide window shows everything again.
    resize_window(&mut shell, 1440., 900.);
    assert_eq!(narrow(&mut shell), Narrow::Full);
    assert_eq!(toolbar_items(&mut shell), shown_at(Narrow::Full));
}

#[test]
fn toolbar_fit_is_searched_from_the_last_frame() {
    use polygloss_app::review_tab::toolbar::{Narrow, first_fit};
    let last = Narrow::ALL.len() - 1;
    // A row that fits from `fit` on, its left side `all` items long: how far
    // the search went, the row it kept (named by where it was built), and
    // what it built on the way.
    let search = |from: usize, fit: usize, all: usize| {
        let mut built = Vec::new();
        let (at, row) = first_fit(from, |at| {
            built.push(at);
            (at, at >= fit, all)
        });
        (at, row, built)
    };
    // Steady: one build when nothing gave way, else the one before (too
    // wide) and its own.
    assert_eq!(search(0, 0, 4), (0, 0, vec![0]));
    assert_eq!(search(3, 3, 4), (3, 3, vec![2, 3]));
    assert_eq!(
        search(last + 2, last + 2, 4),
        (last + 2, last + 2, vec![last + 1, last + 2])
    );
    // Narrowing goes on from the last frame's; widening goes back while
    // the one before fits.
    assert_eq!(search(2, 5, 4), (5, 5, vec![1, 2, 3, 4, 5]));
    assert_eq!(search(5, 2, 4), (2, 2, vec![4, 3, 2, 1]));
    // Nothing fits: every step, then the left side's items but the first.
    assert_eq!(
        search(0, usize::MAX, 4),
        (last + 3, last + 3, (0..=last + 3).collect())
    );
    // The left side has fewer items than last frame: as far as they go (the
    // row built past that is the same row).
    assert_eq!(
        search(last + 5, usize::MAX, 2),
        (last + 1, last + 5, vec![last + 4, last + 5])
    );
}

#[gpui_kit::test]
fn i_opens_the_iteration_menu_when_its_pill_gave_way(cx: &mut TestAppContext) {
    use polygloss_app::iterations;
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = iteration_review(&mut shell, repo.path());
    let open = |shell: &mut Shell| tab.read_with(shell.cx, |t, _| iterations::menu_open(t));
    let hangs_from = |shell: &mut Shell, anchor: &str| {
        let at = bounds(shell.cx, anchor);
        let menu = bounds(shell.cx, "key-menu");
        assert_eq!(
            (menu.left(), menu.top()),
            (at.left(), at.bottom()),
            "the menu hangs from {anchor}"
        );
    };

    // Opened while the pill shows, it hangs from the pill; when the pill
    // gives way, from the left side's corner, under the repo name.
    shell.cx.simulate_keystrokes("i");
    draw(shell.cx);
    hangs_from(&mut shell, "iteration-picker");
    narrowest(&mut shell);
    assert!(painted(shell.cx, "iteration-picker").is_none(), "gave way");
    assert!(open(&mut shell));
    hangs_from(&mut shell, "toolbar-left");
    // It has the keyboard: Esc closes it.
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    assert!(!open(&mut shell), "Esc reached the menu");
    assert!(painted(shell.cx, "key-menu").is_none());

    // `i` with the pill gone opens it there too, with the keyboard.
    shell.cx.simulate_keystrokes("i");
    draw(shell.cx);
    assert!(open(&mut shell));
    hangs_from(&mut shell, "toolbar-left");
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    assert!(!open(&mut shell), "Esc reached the menu");
}

#[gpui_kit::test]
fn display_menu_opens_from_its_button_and_acts_on_the_diff(cx: &mut TestAppContext) {
    use polygloss_app::review_tab::toolbar::{self, Narrow};
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let wrap = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).options().style.wrap)
    };
    let find_open = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            polygloss_app::find::find_bar(t).is_some_and(|bar| bar.read(cx).is_open())
        })
    };

    // Wide: the view toggles; the third row is Wrap lines, and choosing it
    // reaches the diff.
    assert!(!wrap(&mut shell));
    click(shell.cx, "view-options");
    shell.cx.simulate_keystrokes("down down down enter");
    draw(shell.cx);
    assert!(wrap(&mut shell), "Wrap lines reached the diff");

    // Narrow: `N/M` (a row that only informs) leads the menu, then Find,
    // which opens the find bar.
    narrowest(&mut shell);
    assert!(tab.read_with(shell.cx, |t, _| toolbar::narrow(t)) >= Narrow::ProgressInMenu);
    assert!(!find_open(&mut shell));
    click(shell.cx, "view-options");
    shell.cx.simulate_keystrokes("down down enter");
    draw(shell.cx);
    assert!(find_open(&mut shell), "Find reached the tab");
    assert!(wrap(&mut shell), "nothing else changed");
}

#[gpui_kit::test]
fn threads_button_counts_open_threads_and_toggles_the_panel(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert!(shows(shell.cx, "threads-count", "0"));
    let at = |line_no| line("src/config.rs", Side::New, line_no, line_no);
    create(
        &mut shell,
        &tab,
        at(1),
        ThreadKind::Comment,
        "One.",
        human(),
    );
    create(
        &mut shell,
        &tab,
        at(3),
        ThreadKind::Question,
        "Two?",
        agent(),
    );
    let done = create(
        &mut shell,
        &tab,
        at(5),
        ThreadKind::Comment,
        "Done.",
        agent(),
    );
    shell
        .core
        .set_resolved(&done, true, &Actor::human(), None)
        .expect("resolve");
    create(&mut shell, &tab, at(10), ThreadKind::Note, "FYI.", agent());
    reload(&mut shell, &tab);
    // Two open threads; the resolved one and the note do not count.
    assert_eq!(tab.read_with(shell.cx, threads::open_counts), (2, 1));
    assert!(shows(shell.cx, "threads-count", "2"));

    let visible = |shell: &mut Shell| tab.read_with(shell.cx, |t, _| t.threads_panel_visible());
    assert!(!visible(&mut shell), "hidden by default");
    click(shell.cx, "toggle-threads-panel");
    assert!(visible(&mut shell), "a click shows the panel");
    assert!(painted(shell.cx, "threads-pane").is_some());
    click(shell.cx, "toggle-threads-panel");
    assert!(!visible(&mut shell), "and hides it again");
    assert!(painted(shell.cx, "threads-pane").is_none());
}

#[gpui_kit::test]
fn threads_button_marks_hidden_notes(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    for (n, body) in [(1, "First note."), (3, "Second note.")] {
        let subject = line("src/config.rs", Side::New, n, n);
        create(&mut shell, &tab, subject, ThreadKind::Note, body, agent());
    }
    reload(&mut shell, &tab);
    let tooltip = |shell: &mut Shell| {
        hover(shell.cx, "toggle-threads-panel");
        shell
            .cx
            .executor()
            .advance_clock(std::time::Duration::from_secs(2));
        draw(shell.cx);
        let shown = [
            "Show threads panel",
            "Show threads panel · 2 agent notes hidden",
        ]
        .into_iter()
        .find(|t| painted(shell.cx, &format!("tooltip: {t}")).is_some());
        unhover(shell.cx);
        shown
    };
    // Notes showing: no dot, a plain tooltip (the panel is hidden).
    assert!(!painted(shell.cx, "threads-notes-hidden").is_some());
    assert_eq!(tooltip(&mut shell), Some("Show threads panel"));

    shell.cx.dispatch_action(tab_actions::ToggleAgentNotes);
    draw(shell.cx);
    assert!(
        painted(shell.cx, "threads-notes-hidden").is_some(),
        "the muted dot"
    );
    assert_eq!(
        tooltip(&mut shell),
        Some("Show threads panel · 2 agent notes hidden")
    );

    shell.cx.dispatch_action(tab_actions::ToggleAgentNotes);
    draw(shell.cx);
    assert!(!painted(shell.cx, "threads-notes-hidden").is_some());
    assert_eq!(tooltip(&mut shell), Some("Show threads panel"));
}

/// ADR-0031 (one segmented control): the sidebar's `[Files | Reviews]` and
/// the toolbar's split | unified toggle have tracks of one height and
/// segments of one size, each segment inset from its track (and from its
/// neighbour) by one amount on every side, the same in both.
#[gpui_kit::test]
fn both_toggles_are_one_segmented_control(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    let controls = [
        ("sidebar-segments", ["segment-files", "segment-reviews"]),
        ("layout-toggle", ["layout-split", "layout-unified"]),
    ];
    let segment = bounds(shell.cx, "segment-files").size;
    let track_height = bounds(shell.cx, "sidebar-segments").size.height;
    let mut insets = Vec::new();
    for (track, [first, second]) in controls {
        let t = bounds(shell.cx, track);
        let (a, b) = (bounds(shell.cx, first), bounds(shell.cx, second));
        assert_eq!(t.size.height, track_height, "{track}");
        assert_eq!((a.size, b.size), (segment, segment), "{track}");
        let inset = a.left() - t.left();
        for (side, value) in [
            ("top", a.top() - t.top()),
            ("bottom", t.bottom() - a.bottom()),
            ("between", b.left() - a.right()),
            ("trailing", t.right() - b.right()),
        ] {
            assert_eq!(value, inset, "{track} {side}");
        }
        insets.push(inset);
    }
    assert_eq!(insets[0], insets[1]);
}
