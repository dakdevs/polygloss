//! M7, a thread card ↔ its resolved chip (T7.14, ADR-0030): a click on the
//! chip opens the card by a block reveal from the block's top edge, the
//! rows and blocks below riding; the card's Collapse button swaps the chip
//! in at once and closes the vacated height; `x` and the composer's Resolve
//! (an async reload) snap. Stepped by ADR-0030's protocol; the curve is this
//! file's own solver, every height and duration written by hand.
//!
//! Fixture: an agent's one-line note on `src/config.rs` line 1 (a chip) and
//! an open comment on line 5 below it. The chip's block is 44 pt (`NESTED_Y`
//! 8, the `MD` chip 28, `NESTED_Y` 8); the note's card's is 149 pt (8, then
//! the card: border 1, its header row 28, the comment 8 + 20 + 4 + 18 + 8,
//! the footer 1 + 8 + 28 + 8, border 1, then 8): Δ = 105, so the card opens
//! over `reveal(105)` = 160.5 ms and closes over `exit(160.5)` = 120 ms.

use std::time::Duration;

use gpui_kit::{Entity, Modifiers, TestAppContext};
use polygloss_app::motion::tokens::FIRST_STEP;
use polygloss_app::motion::{self, MotionPolicy};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::threads::{self, placement};
use polygloss_core::review::{ThreadKind, ThreadStatus, Viewer};
use polygloss_diff::Side;

use super::{agent, bounds, create, human, keys, line, painted, reload};
use crate::shell::{Shell, compare_req, start};
use crate::support::motion::{advance, frame, requested_frames, step_to};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

/// The chip's block and the card's, by hand (see the module docs).
const CHIP: f32 = 8.0 + 28.0 + 8.0;
const CARD: f32 =
    8.0 + 1.0 + 28.0 + (8.0 + 20.0 + 4.0 + 18.0 + 8.0) + (1.0 + 8.0 + 28.0 + 8.0) + 1.0 + 8.0;
/// `reveal(105)`: 150 + 0.1 · 105 ms.
const OPEN_MS: f64 = 160.5;
/// `exit(160.5)`: 0.75 · 160.5 = 120.4, rounded to 10 ms.
const CLOSE_MS: f64 = 120.0;

fn ms(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1000.0)
}

/// `cubic-bezier(x1, y1, x2, y2)` at `t` by bisection on x.
fn bezier(x1: f64, y1: f64, x2: f64, y2: f64, t: f64) -> f64 {
    let at = |a: f64, b: f64, s: f64| {
        3.0 * (1.0 - s) * (1.0 - s) * s * a + 3.0 * (1.0 - s) * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..200 {
        let mid = (low + high) / 2.0;
        if at(x1, x2, mid) < t {
            low = mid;
        } else {
            high = mid;
        }
    }
    at(y1, y2, (low + high) / 2.0)
}

/// SLIDE.
fn slide(t: f64) -> f64 {
    bezier(0.25, 1.0, 0.5, 1.0, t)
}

/// `round(v · scale) / scale` at the test window's scale, 2.
fn q(v: f64) -> f32 {
    ((v * 2.0).round() / 2.0) as f32
}

fn assert_y(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-3,
        "{what}: {actual} (expected {expected})"
    );
}

/// The note (a chip) and the comment below it, under `policy`.
fn note_and_comment<'a>(
    cx: &'a mut TestAppContext,
    policy: MotionPolicy,
) -> (FixtureRepo, Shell<'a>, Entity<ReviewTab>, String, String) {
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell
        .cx
        .update(|_, cx| motion::set_override(Some(policy), cx));
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let note = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 1, 1),
        ThreadKind::Note,
        "Sorted on purpose.",
        agent(),
    );
    let below = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 5, 5),
        ThreadKind::Comment,
        "Below.",
        human(),
    );
    reload(&mut shell, &tab);
    (repo, shell, tab, note, below)
}

/// The window y of thread `id`'s card as painted.
fn top(shell: &mut Shell, id: &str) -> f32 {
    bounds(shell.cx, format!("thread-{id}")).top().as_f32()
}

/// Thread `id`'s block height in the viewport's document.
fn block_height(shell: &mut Shell, tab: &Entity<ReviewTab>, id: &str) -> f32 {
    let block = placement::block_id(id);
    tab.read_with(shell.cx, |t, cx| {
        let doc = t.viewport.read(cx).document();
        (0..doc.len())
            .flat_map(|f| doc.blocks(f).iter())
            .find(|b| b.id == block)
            .map(|b| b.height)
            .expect("the block is placed")
    })
}

fn running(shell: &mut Shell, tab: &Entity<ReviewTab>) -> bool {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).motion_running())
}

fn click(shell: &mut Shell, selector: String) {
    let at = bounds(shell.cx, selector).center();
    shell.cx.simulate_click(at, Modifiers::none());
}

#[gpui_kit::test]
fn chip_click_reveals_the_card(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (_repo, mut shell, tab, note, below) = note_and_comment(cx, MotionPolicy::Full);
    assert_eq!(block_height(&mut shell, &tab, &note), CHIP);
    let old = top(&mut shell, &below);

    // The commit frame: the card replaces the chip, measured once at its
    // final height, the comment below where it was.
    click(&mut shell, format!("thread-chip-{note}"));
    frame(shell.cx);
    assert!(running(&mut shell, &tab), "the block reveals");
    assert!(painted(shell.cx, format!("thread-{note}")), "the card");
    assert!(!painted(shell.cx, format!("thread-chip-{note}")));
    assert_eq!(block_height(&mut shell, &tab, &note), CARD);
    assert_y(top(&mut shell, &below), old, "commit frame");

    step_to(shell.cx, FIRST_STEP);
    assert_y(
        top(&mut shell, &below),
        old + q(105.0 * slide(16.667 / OPEN_MS)),
        "first step",
    );
    advance(shell.cx, ms(OPEN_MS / 2.0) - FIRST_STEP);
    assert_y(
        top(&mut shell, &below),
        old + q(105.0 * slide(0.5)),
        "half way",
    );
    advance(shell.cx, ms(OPEN_MS / 2.0));
    assert_y(top(&mut shell, &below), old + CARD - CHIP, "settled");
    assert!(!running(&mut shell, &tab));
    assert_eq!(requested_frames(shell.cx), 0, "no frame after it settled");
}

#[gpui_kit::test]
fn collapse_swaps_to_the_chip_at_once_then_closes_the_height(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (_repo, mut shell, tab, note, below) = note_and_comment(cx, MotionPolicy::Full);
    // Opened at once (a jump from the panel opens it programmatically).
    let m = tab.read_with(shell.cx, |t, _| threads::threads(t).cloned().unwrap());
    m.update(shell.cx, |m, cx| m.toggle_expanded(&note, cx));
    frame(shell.cx);
    assert!(!running(&mut shell, &tab), "a programmatic open snaps");
    let old = top(&mut shell, &below);

    // The Collapse button: the chip is in on the commit frame, the vacated
    // height closes from there.
    click(&mut shell, format!("thread-hide-{note}"));
    frame(shell.cx);
    assert!(running(&mut shell, &tab), "the vacated height closes");
    assert!(painted(shell.cx, format!("thread-chip-{note}")), "the chip");
    assert!(!painted(shell.cx, format!("thread-{note}")), "no card");
    assert_eq!(block_height(&mut shell, &tab, &note), CHIP);
    assert_y(top(&mut shell, &below), old, "commit frame");

    step_to(shell.cx, ms(CLOSE_MS / 2.0));
    assert_y(
        top(&mut shell, &below),
        old - 105.0 + q(105.0 * (1.0 - slide(0.5))),
        "half way",
    );
    advance(shell.cx, ms(CLOSE_MS / 2.0));
    assert_y(top(&mut shell, &below), old - (CARD - CHIP), "settled");
    assert!(!running(&mut shell, &tab));
    assert_eq!(requested_frames(shell.cx), 0, "no frame after it settled");
}

#[gpui_kit::test]
fn resolve_by_x_and_the_composer_snap(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let (_repo, mut shell, tab, _, below) = note_and_comment(cx, MotionPolicy::Full);
    let open = create(
        &mut shell,
        &tab,
        line("src/config.rs", Side::New, 3, 3),
        ThreadKind::Comment,
        "Rename this.",
        agent(),
    );
    reload(&mut shell, &tab);
    let resolved = |shell: &mut Shell, id: &str| {
        shell.core.thread(id, Viewer::Human).unwrap().status == ThreadStatus::Resolved
    };

    // `x` in the threads panel, its row selected: the card is a chip on
    // the first frame after the reload, with nothing moving.
    tab.update(shell.cx, |t, cx| t.set_threads_panel_visible(true, cx));
    crate::shell::draw(shell.cx);
    let m = tab.read_with(shell.cx, |t, _| threads::threads(t).cloned().unwrap());
    shell
        .cx
        .update(|window, cx| threads::focus_panel(&m, window, cx));
    let row = m.read_with(shell.cx, |m, cx| {
        threads::panel::rows(m, cx)
            .iter()
            .position(|(id, _)| *id == open)
            .expect("the open thread's row")
    });
    m.update(shell.cx, |m, cx| m.select_row(row, cx));
    frame(shell.cx);
    let before = top(&mut shell, &below);
    let card = block_height(&mut shell, &tab, &open);
    keys(&mut shell, "x");
    assert!(resolved(&mut shell, &open), "x resolved it");
    assert!(painted(shell.cx, format!("thread-chip-{open}")));
    assert!(!running(&mut shell, &tab), "x snaps");
    assert_y(
        top(&mut shell, &below),
        before - (card - CHIP),
        "the rows below at their end",
    );
    assert_eq!(requested_frames(shell.cx), 0);

    // Unresolved from the panel, then resolved by the composer's button:
    // the click's write reloads the threads, which snaps.
    keys(&mut shell, "x");
    assert!(!resolved(&mut shell, &open));
    let before = top(&mut shell, &below);
    click(&mut shell, format!("thread-resolve-{open}"));
    crate::shell::draw(shell.cx);
    assert!(resolved(&mut shell, &open), "the composer resolved it");
    assert!(painted(shell.cx, format!("thread-chip-{open}")));
    assert!(!running(&mut shell, &tab), "the composer's Resolve snaps");
    assert_y(
        top(&mut shell, &below),
        before - (card - CHIP),
        "the rows below at their end",
    );
    assert_eq!(requested_frames(shell.cx), 0);
}
