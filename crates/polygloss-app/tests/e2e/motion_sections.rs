//! Section and gap reveals in the full app (T7.11, ADR-0030 M4 and M6),
//! drawn by Metal: their filmstrips (a category section opening by its
//! band's Show, a gap's ↓20) and a band's Show pressed while a card
//! reveals, through the review tab's registration with `motion::settle`:
//! the mouse down freezes the card's reveal, the click settles it and
//! starts the section's, which runs on after the mouse up.
//!
//! The harness names these tests `motion_sections::<test>`.
//!
//! Fixture: a commit changing lines 4 and 71 of `a.rs` (80 lines: a 60-line
//! gap between its hunks) and adding `tests/t0.rs` and `tests/t1.rs` (12
//! lines each: the Tests section, closed by default, whose region below its
//! band is two 294 pt cards 12 apart, 600 pt, so it opens over BASE), in a
//! 1280 × 800 window under the header card. Durations are written here from
//! ADR-0030's tokens.

use gpui_kit::HeadlessAppContext;
use polygloss_app::motion::{MotionPolicy, tokens};
use polygloss_diff::ObjectFormat;
use polygloss_diff::rows::ExpandBy;
use polygloss_viewport::{ControlAction, RevealKind};

use crate::motion_viewport::{App, FIRST_STEP, click_at, lines, ms, open_settled};
use crate::support::filmstrip::{assert_filmstrip, filmstrip};
use crate::support::harness::Test;
use crate::support::screenshot;
use crate::support::{FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &[
    Test::new("motion_sections::e2e_motion_section", e2e_motion_section),
    Test::new("motion_sections::e2e_motion_gap", e2e_motion_gap),
    Test::new(
        "motion_sections::a_band_click_mid_reveal_runs_its_own_reveal",
        a_band_click_mid_reveal_runs_its_own_reveal,
    ),
];

/// A commit of `a.rs`, then one changing its lines 4 and 71 and adding two
/// test files.
fn sections_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    let old = lines("a", 80);
    repo.write("a.rs", old.as_bytes());
    repo.commit("base");
    let new: String = old
        .lines()
        .enumerate()
        .map(|(i, l)| match i {
            3 | 70 => format!("{l} // changed\n"),
            _ => format!("{l}\n"),
        })
        .collect();
    repo.write("a.rs", new.as_bytes());
    repo.write("tests/t0.rs", lines("t0", 12).as_bytes());
    repo.write("tests/t1.rs", lines("t1", 12).as_bytes());
    repo.commit("change a.rs, add two tests");
    repo
}

/// The Tests section's id; its region below the band is 600 pt open (so
/// it opens over `reveal(600)` = BASE, 200 ms).
fn tests_section(app: &mut App) -> u32 {
    let viewport = app.viewport.clone();
    app.cx.update(|cx| {
        let v = viewport.read(cx);
        let doc = v.document();
        assert_eq!(doc.sections().len(), 1, "one section: Tests");
        let s = &doc.sections()[0];
        let h = |f: u32| (doc.card_bottom(f) - doc.header_top(f)) as f32;
        let region = h(s.files[0]) + 12.0 + h(s.files[1]);
        assert_eq!(region, 600.0, "the Tests cards open");
        s.id
    })
}

fn e2e_motion_section() {
    let _sb = Sandbox::isolate();
    let repo = sections_repo();
    let mut app = open_settled(&repo);
    let id = tests_section(&mut app);
    let window = app.window;
    // a.rs collapsed, so the band sits high and most of the region's
    // reveal is in view.
    let (a, viewport) = (app.file(0), app.viewport.clone());
    app.cx
        .update(|cx| viewport.update(cx, |v, cx| v.set_collapsed(a, true, cx)));
    screenshot::draw(&mut app.cx, window);
    // The band's Show link.
    let at = app.control(ControlAction::SectionToggle(id));
    let click = move |cx: &mut HeadlessAppContext| click_at(cx, window, at);
    let full = filmstrip(&mut app.cx, window, MotionPolicy::Full, tokens::BASE, click);
    app.policy(MotionPolicy::Off);
    app.cx
        .update(|cx| viewport.update(cx, |v, cx| v.set_section_open(id, false, cx)));
    screenshot::park_pointer(&mut app.cx, window);
    screenshot::draw(&mut app.cx, window);
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        click,
    );
    assert_filmstrip("section", full, reduced);
}

fn e2e_motion_gap() {
    let _sb = Sandbox::isolate();
    let repo = sections_repo();
    let mut app = open_settled(&repo);
    let a = app.file(0);
    let window = app.window;
    // The 60-line gap's "↓ 20" (the 6-line one offers Expand all only):
    // 400 pt over QUICK.
    let action = app
        .debug()
        .controls
        .iter()
        .find(|c| {
            matches!(
                c.action,
                ControlAction::Expand {
                    by: ExpandBy::Down(20),
                    ..
                }
            )
        })
        .expect("a ↓ 20 link")
        .action;
    let at = app.control(action);
    let click = move |cx: &mut HeadlessAppContext| click_at(cx, window, at);
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::QUICK,
        click,
    );
    app.policy(MotionPolicy::Off);
    let viewport = app.viewport.clone();
    app.cx
        .update(|cx| viewport.update(cx, |v, cx| v.set_expansions(a, &[], cx)));
    screenshot::park_pointer(&mut app.cx, window);
    screenshot::draw(&mut app.cx, window);
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        click,
    );
    assert_filmstrip("gap", full, reduced);
}

fn a_band_click_mid_reveal_runs_its_own_reveal() {
    let _sb = Sandbox::isolate();
    let repo = sections_repo();
    let mut app = open_settled(&repo);
    let id = tests_section(&mut app);
    let a = app.file(0);
    let viewport = app.viewport.clone();
    app.policy(MotionPolicy::Off);
    app.cx
        .update(|cx| viewport.update(cx, |v, cx| v.set_collapsed(a, true, cx)));
    screenshot::draw(&mut app.cx, app.window);
    app.policy(MotionPolicy::Full);
    // a.rs opens by its chevron (392 pt: reveal(392) = 189.2 ms).
    let chevron = app.control(ControlAction::Collapse(a));
    app.press(chevron);
    app.release(chevron);
    assert!(app.running());
    app.step_to(ms(60.0));
    // The mouse down on the band's Show freezes a.rs's reveal where it is
    // painted; three frames pass with no frame requested.
    let show = app.control(ControlAction::SectionToggle(id));
    app.press(show);
    assert!(app.debug().reveal.expect("revealing").frozen, "frozen");
    let counts: Vec<usize> = (0..4).map(|_| app.advance(FIRST_STEP)).collect();
    assert_eq!(counts[1..], [0, 0, 0], "no frame while frozen: {counts:?}");
    assert_eq!(app.control(ControlAction::SectionToggle(id)), show);
    // The mouse up there clicks Show: a.rs's reveal settles, the section's
    // starts and runs on past the deferred settle.
    app.release(show);
    app.frame();
    let (open, a_open) = app.cx.update(|cx| {
        let v = viewport.read(cx);
        (v.section_open(id), !v.document().is_collapsed(a))
    });
    assert_eq!(open, Some(true), "the click landed on Show");
    assert!(a_open, "a.rs ended open");
    let reveal = app.debug().reveal.expect("the section's reveal runs on");
    assert_eq!(reveal.kind, RevealKind::Section(id));
    assert!(!reveal.frozen);
    app.step_to(ms(100.0));
    let h = app.debug().reveal.expect("still revealing").height;
    assert!(h > 0.0 && h < 600.0, "the region between at ½: {h}");
}
