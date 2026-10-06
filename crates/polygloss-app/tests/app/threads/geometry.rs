//! Thread blocks, snippets and the threads panel on the spacing system
//! (T7.7, ADR-0031 C4, S1, the density table and the ladder), and the press
//! ink of chips and panel rows (ADR-0030). Expected values are written by
//! hand from the ADRs' rules; origins come from layout bounds (debug
//! selectors, the viewport's `card_bounds` and debug frame).

use gpui_kit::{Bounds, Entity, Hsla, Pixels, Point, VisualTestContext, px};
use polygloss_app::keymap::actions::viewport as viewport_actions;
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::threads::placement::{self, ThreadPlace};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{NewThread, OpenRequest, ThreadKind, Viewer};
use polygloss_core::store::events::Actor;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::ScrollTarget;

use super::{agent, bounds, create, human, keys, line, model, painted, reload};
use crate::shell::{Shell, compare_req, draw, resize_window, start};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

/// Where the viewport's origin is in the window: the first painted file
/// card's window bounds less its viewport-relative ones.
fn viewport_origin(shell: &mut Shell, tab: &Entity<ReviewTab>) -> (f32, f32) {
    tab.read_with(shell.cx, |t, cx| {
        let v = t.viewport.read(cx);
        let d = v.debug();
        let at = d.cards.first().expect("a card is painted");
        let card = v.card_bounds(at.file_idx).expect("its bounds");
        (
            card.left().as_f32() - at.bounds.0,
            card.top().as_f32() - at.bounds.1,
        )
    })
}

/// The corner radius of the bordered quad painted at `at` (window points).
fn border_radius(cx: &mut VisualTestContext, at: Bounds<Pixels>) -> f32 {
    cx.update(|window, _| {
        let scale = window.scale_factor();
        window
            .painted_quads()
            .into_iter()
            .find(|q| {
                let b = q.bounds;
                (b.origin.x.0 / scale - at.left().as_f32()).abs() < 0.01
                    && (b.origin.y.0 / scale - at.top().as_f32()).abs() < 0.01
                    && (b.size.width.0 / scale - at.size.width.as_f32()).abs() < 0.01
                    && q.border_widths.left.0 > 0.0
            })
            .map(|q| q.corner_radii.top_left.0 / scale)
            .unwrap_or_else(|| panic!("no bordered quad at {at:?}"))
    })
}

#[gpui_kit::test]
fn thread_blocks_sit_on_the_nested_edge(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    // Unified: the block spans its card.
    shell.cx.dispatch_action(viewport_actions::LayoutUnified);
    draw(shell.cx);
    // A question: a header row with its badge, then the root comment.
    let id = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 5, 5),
        ThreadKind::Question,
        "Why a BTreeMap?",
        agent(),
    );
    reload(&mut shell, &tab);
    let m = model(&mut shell, &tab);
    let Some(ThreadPlace::Line { file_idx, .. }) =
        m.read_with(shell.cx, |m, _| m.place(&id).copied())
    else {
        panic!("the thread is on a line");
    };
    let comment = shell.core.thread(&id, Viewer::Agent).unwrap().comments[0]
        .id
        .clone();
    let thread = bounds(shell.cx, format!("thread-{id}"));
    let card = tab
        .read_with(shell.cx, |t, cx| t.viewport.read(cx).card_bounds(file_idx))
        .expect("the file card is painted");

    // 12 from the file card's inner edges (its 1 pt border inside).
    assert_eq!(thread.left() - (card.left() + px(1.)), px(12.));
    assert_eq!((card.right() - px(1.)) - thread.right(), px(12.));
    // 8 below its line and 8 above the next one: inside its block row.
    let block = format!("[block {}]", placement::block_id(&id).0);
    let origin = viewport_origin(&mut shell, &tab);
    let (top, height) = tab.read_with(shell.cx, |t, cx| {
        let d = t.viewport.read(cx).debug();
        let i = d
            .visible_rows
            .iter()
            .position(|r| *r == block)
            .unwrap_or_else(|| panic!("{block} not in {:#?}", d.visible_rows));
        d.row_bounds[i]
    });
    let row_top = origin.1 + top;
    assert_eq!(thread.top().as_f32() - row_top, 8.0);
    assert_eq!(row_top + height - thread.bottom().as_f32(), 8.0);
    // A card: radius 8.
    assert_eq!(border_radius(shell.cx, thread), 8.0);

    // The header row is a list row: 28, inside the block's border.
    let header = bounds(shell.cx, format!("thread-header-{id}"));
    assert_eq!(header.size.height, px(28.));
    assert_eq!(header.top(), thread.top() + px(1.));
    // The comment's content box (its avatar) 12 from the block's inner left
    // edge and 8 below the header row; its body after the avatar and its
    // gap: 12 + 20 + 8 = 40.
    let inner_left = thread.left() + px(1.);
    let avatar = bounds(shell.cx, format!("thread-avatar-{comment}"));
    assert_eq!(avatar.left() - inner_left, px(12.));
    assert_eq!(avatar.top() - header.bottom(), px(8.));
    assert_eq!(avatar.size.width, px(20.));
    let body = bounds(shell.cx, format!("thread-body-{comment}"));
    assert_eq!(body.left() - inner_left, px(40.));
}

/// `base`: `long.txt` with `n` lines. `topic` (checked out) changes line
/// `at`; a thread is written on it there, then the next commit changes it
/// again, so the thread is outdated and shows its snippet (lines `at − 3`
/// to `at`). Returns the repo, the review tab and the thread.
fn outdated_at(shell: &mut Shell, n: u32, at: u32) -> (FixtureRepo, Entity<ReviewTab>, String) {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let text = |changed: &str| -> String {
        (1..=n)
            .map(|i| {
                if i == at {
                    format!("{changed} {i}\n")
                } else {
                    format!("line {i}\n")
                }
            })
            .collect()
    };
    repo.write("long.txt", text("line").as_bytes());
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.git(&["checkout", "-q", "-b", "topic"]);
    repo.write("long.txt", text("first").as_bytes());
    repo.commit("first");
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/heads/topic".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    let first = shell.core.open(&req).unwrap();
    let blobs = BlobReader::open(&first.repo).unwrap();
    let id = shell
        .core
        .create_thread(
            &NewThread {
                review_id: first.review_id.clone(),
                diff_id: first.diff_id.clone(),
                subject: line("long.txt", Side::New, at, at),
                kind: ThreadKind::Comment,
                body_md: "Why?".into(),
                author: human(),
            },
            &blobs,
        )
        .unwrap();
    repo.write("long.txt", text("second").as_bytes());
    repo.commit("second");
    let tab = shell.open(req).unwrap();
    tab.update(shell.cx, |t, cx| {
        t.viewport.update(cx, |v, cx| {
            v.scroll_to(ScrollTarget::Block(placement::block_id(&id)), cx)
        })
    });
    draw(shell.cx);
    (repo, tab, id)
}

#[gpui_kit::test]
fn thread_snippets_use_the_card_column_formula(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    // Hand-computed from the snippet frame's inner edge at 13 pt (advance
    // 7.8): the bar, the number pad, the digits and the end pad, on the
    // 4 pt grid (at least 40), then 10 to the code.
    // 3 digits: 4 + 4 + 23.4 + 8 = 39.4 → 40, + 10 = 50.
    // 4 digits: 4 + 4 + 31.2 + 8 = 47.2 → 48, + 10 = 58.
    for (n, at, code_x) in [(120, 105, 50.0), (1_010, 1_005, 58.0)] {
        let (_repo, _tab, id) = outdated_at(&mut shell, n, at);
        let snippet = bounds(shell.cx, format!("thread-snippet-{id}"));
        let code = bounds(shell.cx, format!("thread-snippet-code-{id}-0"));
        assert_eq!(code.left() - snippet.left(), px(code_x), "line {at}");
        // Rows follow the code-row rule: round(1.5 × 13) = 20.
        assert_eq!(code.size.height, px(20.), "line {at}");
    }
}

#[gpui_kit::test]
fn threads_panel_cards_use_compact_insets(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    let ids = [1, 5].map(|n| {
        let subject = line("src/config.rs", Side::New, n, n);
        create(
            &mut shell,
            &tab,
            subject,
            ThreadKind::Comment,
            "Hm.",
            human(),
        )
    });
    reload(&mut shell, &tab);
    let panel = bounds(shell.cx, "threads-panel".into());
    for id in &ids {
        // `threads-panel-<id>` is the card's content box, inside its
        // 1 pt border.
        let row = bounds(shell.cx, format!("threads-panel-{id}"));
        let first = bounds(shell.cx, format!("threads-panel-line-{id}"));
        assert_eq!(first.left() - row.left(), px(12.), "{id}");
        assert_eq!(first.top() - row.top(), px(8.), "{id}");
        // Cards 10 from the panel's edges (their borders included).
        assert_eq!(row.left() - px(1.) - panel.left(), px(10.), "{id}");
        assert_eq!(panel.right() - (row.right() + px(1.)), px(10.), "{id}");
    }
    let rows = ids
        .each_ref()
        .map(|id| bounds(shell.cx, format!("threads-panel-{id}")));
    // Dense: cards 8 apart (between their borders).
    assert_eq!(
        (rows[1].top() - px(1.)) - (rows[0].bottom() + px(1.)),
        px(8.)
    );

    // The selection rail is 2 wide whether the panel has the keyboard or
    // not (its color says which).
    let m = model(&mut shell, &tab);
    m.update(shell.cx, |m, cx| m.select_row(0, cx));
    draw(shell.cx);
    let rail = format!("threads-panel-selected-{}", ids[0]);
    assert_eq!(bounds(shell.cx, rail.clone()).size.width, px(2.));
    let focus = m.read_with(shell.cx, |m, _| m.panel_focus().clone());
    shell.cx.update(|window, cx| window.focus(&focus, cx));
    keys(&mut shell, "j");
    keys(&mut shell, "k");
    assert!(painted(shell.cx, rail.clone()));
    assert_eq!(bounds(shell.cx, rail).size.width, px(2.));
}

#[gpui_kit::test]
fn threads_panel_header_is_36(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    let panel = bounds(shell.cx, "threads-panel".into());
    let header = bounds(shell.cx, "threads-panel-header".into());
    assert_eq!(header.size.height, px(36.));
    assert_eq!(header.top(), panel.top());
}

#[gpui_kit::test]
fn empty_states_use_24(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    let header = bounds(shell.cx, "threads-panel-header".into());
    let empty = bounds(shell.cx, "threads-panel-empty".into());
    assert_eq!(empty.top() - header.bottom(), px(24.));
}

/// Asks the split for a threads panel `width` pt wide (its range clamps
/// it). gpui-kit resizes from its record of the panes' widths, and the
/// viewport's goes stale once `panes::render` resets it, so a first call
/// asking for the panel's current width only brings that record up to date
/// (the reason a drag of the panel's edge misbehaves today; T7.10 replaces
/// the split).
fn resize_threads_panel(shell: &mut Shell, tab: &Entity<ReviewTab>, width: f32) {
    let split = tab.read_with(shell.cx, |t, _| t.threads_split().clone());
    shell.cx.update(|window, cx| {
        split.update(cx, |s, cx| {
            let current = s.sizes()[1];
            s.resize_panel(1, current, window, cx);
            s.resize_panel(1, px(width), window, cx);
        })
    });
    draw(shell.cx);
}

#[gpui_kit::test]
fn threads_pane_keeps_its_range(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    resize_window(&mut shell, 1_440., 900.);
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    draw(shell.cx);
    let width = |shell: &mut Shell, name: &str| bounds(shell.cx, name.into()).size.width;
    // Wider than allowed: the panel stops at 720 (design §11.1).
    resize_threads_panel(&mut shell, &tab, 900.);
    assert_eq!(width(&mut shell, "threads-pane"), px(720.));
    // Narrower: it stops at 220.
    resize_threads_panel(&mut shell, &tab, 100.);
    assert_eq!(width(&mut shell, "threads-pane"), px(220.));
    // In a narrower window, as wide as it goes: the diff keeps 260.
    resize_window(&mut shell, 1_000., 800.);
    resize_threads_panel(&mut shell, &tab, 900.);
    assert_eq!(width(&mut shell, "viewport-pane"), px(260.));
}

/// Whether the last frame painted a quad of `color` under `at`.
fn ink_at(cx: &mut VisualTestContext, at: Point<Pixels>, color: Hsla) -> bool {
    cx.update(|window, _| {
        let scale = window.scale_factor();
        window.painted_quads().into_iter().any(|q| {
            let b = q.bounds;
            let (x, y) = (at.x.as_f32() * scale, at.y.as_f32() * scale);
            q.background.as_solid() == Some(color)
                && b.origin.x.0 <= x
                && x <= b.origin.x.0 + b.size.width.0
                && b.origin.y.0 <= y
                && y <= b.origin.y.0 + b.size.height.0
        })
    })
}

/// ADR-0030 (Feedback without motion), T7.7: a thread's chip and its row
/// in the threads panel show the foreground at α 0.06 under the pointer
/// and at α 0.12 while pressed.
#[gpui_kit::test]
fn chips_and_panel_rows_show_press_ink(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::{Modifiers, MouseButton, point};
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    // An agent's note: a chip in the diff and a row in the panel.
    let id = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 5, 5),
        ThreadKind::Note,
        "FYI: sorted on purpose.",
        agent(),
    );
    reload(&mut shell, &tab);
    let foreground = shell.cx.update(|_, cx| cx.theme().foreground);
    let away = point(px(-50.), px(-50.));
    for control in [format!("thread-chip-{id}"), format!("threads-panel-{id}")] {
        let at = bounds(shell.cx, control.clone()).center();
        assert!(
            !ink_at(shell.cx, at, foreground.opacity(0.06)),
            "{control} at rest"
        );
        shell.cx.simulate_mouse_move(at, None, Modifiers::none());
        draw(shell.cx);
        assert!(
            ink_at(shell.cx, at, foreground.opacity(0.06)),
            "{control} hovered"
        );
        shell
            .cx
            .simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        draw(shell.cx);
        assert!(
            ink_at(shell.cx, at, foreground.opacity(0.12)),
            "{control} pressed"
        );
        // Released away from it: no click.
        shell
            .cx
            .simulate_mouse_move(away, Some(MouseButton::Left), Modifiers::none());
        shell
            .cx
            .simulate_mouse_up(away, MouseButton::Left, Modifiers::none());
        draw(shell.cx);
    }
    assert!(
        painted(shell.cx, format!("thread-chip-{id}")),
        "still a chip"
    );
}
