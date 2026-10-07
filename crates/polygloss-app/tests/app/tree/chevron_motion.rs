//! M10, the tree's folder chevrons (T7.9, ADR-0030): a folder toggled by the
//! pointer turns its chevron 0° (open, pointing down) ↔ −90° over MICRO on
//! MOVE while its rows insert or leave at once; the arrows (the tree binds
//! no `⏎` or space to a folder) put it at its end. Stepped by ADR-0030's protocol; the curve is this
//! file's own closed form.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::{ElementId, Modifiers, SharedString, TestAppContext};
use polygloss_app::motion::tokens::FIRST_STEP;
use polygloss_app::motion::{self, MotionPolicy, Recorded};

use super::{Opened, bounds, open_tree, row_labels, tree_repo};
use crate::support::Sandbox;
use crate::support::motion::{advance, frame, requested_frames, step_to};

/// MOVE, ease-in-out cubic, in closed form.
fn in_out(t: f64) -> f64 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

fn ms(ms: f64) -> Duration {
    Duration::from_secs_f64(ms / 1000.0)
}

type Drawn = Rc<RefCell<Vec<Recorded>>>;

/// The angle the last frame painted folder `key`'s chevron at, in degrees.
fn angle(drawn: &Drawn, key: &str) -> f32 {
    let id = ElementId::Name(SharedString::from(format!("tree-chevron-{key}")));
    drawn
        .borrow()
        .iter()
        .rev()
        .find(|r| r.id == id)
        .map(|r| r.rotation)
        .unwrap_or_else(|| panic!("no chevron drawn for {key}"))
}

/// Draws one frame by `step`, what was drawn before it forgotten.
fn drawn_after(o: &mut Opened, drawn: &Drawn, step: impl FnOnce(&mut Opened)) {
    drawn.borrow_mut().clear();
    step(o);
}

fn press_row(o: &mut Opened, key: &str) {
    let at = bounds(o.shell.cx, &format!("tree-row-{key}")).center();
    o.shell.cx.simulate_click(at, Modifiers::none());
}

#[gpui_kit::test]
fn folder_chevron_turns_on_pointer_only(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let drawn: Drawn = Rc::default();
    cx.update(|cx| {
        motion::record(drawn.clone(), cx);
        motion::set_override(Some(MotionPolicy::Full), cx);
    });
    let mut o = open_tree(cx, &repo);
    frame(o.shell.cx);
    assert_eq!(angle(&drawn, "d:src"), 0.0, "open, at rest");

    // A click on the folder's row: its rows leave on the commit frame, the
    // chevron still points down.
    drawn_after(&mut o, &drawn, |o| {
        press_row(o, "d:src");
        frame(o.shell.cx);
    });
    assert_eq!(
        row_labels(&mut o),
        ["README.md", "config.toml", "docs/guide", "intro.md", "src"],
        "the rows go at once"
    );
    assert_eq!(angle(&drawn, "d:src"), 0.0, "the commit frame");
    drawn_after(&mut o, &drawn, |o| step_to(o.shell.cx, FIRST_STEP));
    let first = -90.0 * in_out(16.667 / 100.0) as f32;
    assert!((angle(&drawn, "d:src") - first).abs() < 1e-3, "16.667 ms");
    drawn_after(&mut o, &drawn, |o| {
        advance(o.shell.cx, ms(50.0) - FIRST_STEP)
    });
    assert!(
        (angle(&drawn, "d:src") + 45.0).abs() < 1e-3,
        "50 ms: in_out(½) = ½ of −90°: {}",
        angle(&drawn, "d:src")
    );
    drawn_after(&mut o, &drawn, |o| advance(o.shell.cx, ms(50.0)));
    assert_eq!(angle(&drawn, "d:src"), -90.0, "settled after 100 ms");
    assert_eq!(requested_frames(o.shell.cx), 0, "no frame after it settled");

    // Opened again by the pointer: −90° → 0°, −45° half way.
    drawn_after(&mut o, &drawn, |o| {
        press_row(o, "d:src");
        frame(o.shell.cx);
    });
    assert_eq!(row_labels(&mut o).len(), 10, "its rows back at once");
    assert_eq!(angle(&drawn, "d:src"), -90.0, "the commit frame");
    drawn_after(&mut o, &drawn, |o| step_to(o.shell.cx, ms(50.0)));
    assert!((angle(&drawn, "d:src") + 45.0).abs() < 1e-3);
    drawn_after(&mut o, &drawn, |o| advance(o.shell.cx, ms(50.0)));
    assert_eq!(angle(&drawn, "d:src"), 0.0);

    // The keyboard (left closes, right opens; the tree binds no ⏎ or space
    // to a folder): at its end on the commit frame, with no frame asked for.
    let tree = o.tree.clone();
    o.shell.cx.update(|window, cx| {
        tree.update(cx, |t, cx| {
            t.focus(window, cx);
            let state = t.tree_state().expect("an open panel").clone();
            state.update(cx, |s, cx| {
                let ix = s.index_of(&"d:src".into()).expect("the src row");
                s.set_selected_index(Some(ix), cx);
            });
        })
    });
    frame(o.shell.cx);
    for (keys, end) in [("left", -90.0), ("right", 0.0)] {
        drawn_after(&mut o, &drawn, |o| {
            o.shell.cx.simulate_keystrokes(keys);
            frame(o.shell.cx);
        });
        assert_eq!(angle(&drawn, "d:src"), end, "{keys}: the commit frame");
        assert_eq!(requested_frames(o.shell.cx), 0, "{keys}: nothing turns");
    }
}

#[gpui_kit::test]
fn folder_chevron_retargets_freezes_and_jumps_when_reduced(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let drawn: Drawn = Rc::default();
    cx.update(|cx| {
        motion::record(drawn.clone(), cx);
        motion::set_override(Some(MotionPolicy::Full), cx);
    });
    let mut o = open_tree(cx, &repo);
    frame(o.shell.cx);

    // A second click at ½ turns it back from −45°.
    press_row(&mut o, "d:src");
    frame(o.shell.cx);
    drawn_after(&mut o, &drawn, |o| step_to(o.shell.cx, ms(50.0)));
    assert!((angle(&drawn, "d:src") + 45.0).abs() < 1e-3);
    drawn_after(&mut o, &drawn, |o| {
        press_row(o, "d:src");
        frame(o.shell.cx);
    });
    let back = angle(&drawn, "d:src");
    assert!((back + 45.0).abs() < 1e-3, "from where it was: {back}");
    drawn_after(&mut o, &drawn, |o| step_to(o.shell.cx, FIRST_STEP));
    let turning = angle(&drawn, "d:src");
    assert!(
        -45.0 < turning && turning < 0.0,
        "on its way back: {turning}"
    );

    // A mouse down elsewhere freezes it, its mouse up settles it.
    let blank = bounds(o.shell.cx, "file-tree-pane").bottom_left()
        + gpui_kit::point(gpui_kit::px(40.), gpui_kit::px(-60.));
    o.shell
        .cx
        .simulate_mouse_down(blank, gpui_kit::MouseButton::Left, Modifiers::none());
    for _ in 0..3 {
        drawn_after(&mut o, &drawn, |o| advance(o.shell.cx, FIRST_STEP));
        assert_eq!(angle(&drawn, "d:src"), turning, "frozen");
    }
    assert_eq!(requested_frames(o.shell.cx), 0, "frozen: no frames");
    o.shell
        .cx
        .simulate_mouse_up(blank, gpui_kit::MouseButton::Left, Modifiers::none());
    drawn_after(&mut o, &drawn, |o| frame(o.shell.cx));
    assert_eq!(angle(&drawn, "d:src"), 0.0, "settled at the mouse up");

    // Reduce Motion: the chevron jumps.
    o.shell
        .cx
        .update(|_, cx| motion::set_override(Some(MotionPolicy::Reduced), cx));
    drawn_after(&mut o, &drawn, |o| {
        press_row(o, "d:src");
        frame(o.shell.cx);
    });
    assert_eq!(angle(&drawn, "d:src"), -90.0, "Reduced: at its end at once");
    assert_eq!(requested_frames(o.shell.cx), 0);
}
