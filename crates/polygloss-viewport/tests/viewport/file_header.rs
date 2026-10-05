//! The file header (T6.7, design §11.6 "File header"), 2.25 rows = 45 px.
//!
//! Flat geometry at 7.8 px a column, 1000 px wide: the chevron control is
//! 0..23.4 and the title starts there; right to left, the ⋯ control
//! (976.6..1000), 6 px, the Viewed pill (8 + 16 box + 6 + label + 8), 6 px,
//! the `+a −d` pill (6 + "+a" + one column + "−d" + 6), 6 px, the
//! open-in-editor control (24), 6 px, then the review-state pills (6 + 14
//! icon + 4 + label + 6, or 6 + label + 6), 6 px apart. Kind pills (6 +
//! label + 6) follow the title 6 px after it. Text tops are at
//! (45 − 20) / 2 = 12.5; 16 px icons at 14.5, 14 px ones at 15.5.

use gpui_kit::{TestAppContext, VisualTestContext};
use polygloss_diff::Side;
use polygloss_viewport::{
    ControlAction, FileFlags, HeaderDebug, LayoutMode, TitleStyle, ViewportEvent,
};

use crate::support::*;

fn header(d: &polygloss_viewport::ViewportDebug, file_idx: u32) -> &HeaderDebug {
    d.headers
        .iter()
        .find(|h| h.file_idx == file_idx)
        .unwrap_or_else(|| panic!("no header for file {file_idx}: {:?}", d.headers))
}

fn events_of(events: &std::rc::Rc<std::cell::RefCell<Vec<ViewportEvent>>>) -> Vec<ViewportEvent> {
    events
        .borrow()
        .iter()
        .filter(|e| !matches!(e, ViewportEvent::FrameStats(_)))
        .cloned()
        .collect()
}

/// Two added files, `a.rs` (30 lines) and `b.rs` (20 lines).
fn two_added() -> std::sync::Arc<MemProvider> {
    MemProvider::new(vec![
        Spec::added("a.rs", &numbered("a", 30).concat()),
        Spec::added("b.rs", &numbered("b", 20).concat()),
    ])
}

#[gpui_kit::test]
fn header_title_dims_the_directory_and_bolds_the_name(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![
        Spec::modified("src/app/main.rs", "a\n", "b\n"),
        Spec::modified("top.rs", "a\n", "b\n"),
    ]);
    let opts = options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, provider, opts, 1000., 400.);
    let d = debug(&view, cx);
    let (a, b) = (header(&d, 0), header(&d, 1));
    assert_eq!(a.title, "src/app/main.rs");
    assert_eq!(
        a.title_runs,
        [
            ("src/app/".to_owned(), TitleStyle::Dim),
            ("main.rs".to_owned(), TitleStyle::Bold)
        ]
    );
    assert_eq!(b.title_runs, [("top.rs".to_owned(), TitleStyle::Bold)]);
    // Painted right of the chevron; its first run is the muted directory.
    let (x, y) = text_at(&d, "src/app/main.rs");
    assert!(near(x, 3.0 * ADVANCE) && near(y, 12.5), "{x},{y}");
    assert_eq!(text_color(&d, "src/app/main.rs"), theme.muted);
    assert_eq!(text_color(&d, "top.rs"), theme.header_foreground);
}

#[gpui_kit::test]
fn rename_title_shows_old_arrow_new(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::renamed(
        "src/old.rs",
        "lib/new.rs",
        "a\n",
        "b\n",
    )]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 400.);
    let d = debug(&view, cx);
    let h = header(&d, 0);
    assert_eq!(h.title, "src/old.rs → lib/new.rs");
    assert_eq!(d.visible_rows[0], "== src/old.rs → lib/new.rs");
    assert_eq!(
        h.title_runs,
        [
            ("src/".to_owned(), TitleStyle::Dim),
            ("old.rs".to_owned(), TitleStyle::Bold),
            (" → lib/".to_owned(), TitleStyle::Dim),
            ("new.rs".to_owned(), TitleStyle::Bold)
        ]
    );
}

/// `src/old.rs` renamed to `src/new.rs` at 92% similar, its one line
/// changed, with every review flag.
fn flagged_rename() -> std::sync::Arc<MemProvider> {
    MemProvider::new_with(
        vec![Spec::renamed("src/old.rs", "src/new.rs", "a\n", "b\n")],
        |files| files[0].similarity = Some(92),
    )
}

fn set_all_flags(
    view: &gpui_kit::Entity<polygloss_viewport::DiffViewport>,
    cx: &mut VisualTestContext,
) {
    view.update(cx, |v, cx| {
        v.set_file_flags(
            vec![FileFlags {
                changed_since_viewed: true,
                open_threads: 2,
                agent_threads: true,
                ..FileFlags::default()
            }],
            cx,
        )
    });
    settle(cx);
}

#[gpui_kit::test]
fn header_keeps_review_state_and_similarity_badges(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let opts = options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, flagged_rename(), opts, 1000., 400.);
    set_all_flags(&view, cx);
    let d = debug(&view, cx);
    let h = header(&d, 0);
    assert_eq!(
        h.badges,
        [
            "92% similar",
            "changed since viewed",
            "2 open threads",
            "agent"
        ]
    );
    assert_eq!(h.counts, Some((1, 1)));
    // Left: the title, then the kind pill 6 px after its end (23.4 + 23
    // columns = 202.8). Right: the review-state pills, ending 6 px before
    // the open-in-editor control (798.8), then the counts and Viewed.
    for (text, x) in [
        ("src/old.rs → src/new.rs", 23.4),
        ("92% similar", 214.8),
        ("changed since viewed", 410.6),
        ("2 open threads", 602.6),
        ("agent", 747.8),
        ("+1", 834.8),
        ("−1", 858.2),
        ("Viewed", 915.8),
    ] {
        let (tx, ty) = text_at(&d, text);
        assert!(
            near(tx, x) && near(ty, 12.5),
            "{text:?} at {tx},{ty}, expected {x}"
        );
    }
    assert_eq!(text_color(&d, "92% similar"), theme.muted);
    assert_eq!(text_color(&d, "changed since viewed"), theme.accent);
    assert_eq!(text_color(&d, "2 open threads"), theme.muted);
    assert_eq!(text_color(&d, "+1"), theme.stat_added);
    assert_eq!(text_color(&d, "−1"), theme.stat_removed);
    // The icons: chevron, message and bot in their pills, open in editor,
    // the Viewed box and ⋯.
    for (name, rect) in [
        ("chevron-down", (3.7, 14.5, 16.0, 16.0)),
        ("message-square", (584.6, 15.5, 14.0, 14.0)),
        ("bot", (729.8, 15.5, 14.0, 14.0)),
        ("square-arrow-out-up-right", (802.8, 14.5, 16.0, 16.0)),
        ("square", (893.8, 14.5, 16.0, 16.0)),
        ("ellipsis", (980.3, 14.5, 16.0, 16.0)),
    ] {
        let found = icons_named(&d, name);
        assert!(
            found.len() == 1 && near_rect(found[0], rect),
            "{name}: {found:?}, expected {rect:?}"
        );
    }
    for (action, rect) in [
        (ControlAction::Collapse(0), (0.0, 0.0, 23.4, 45.0)),
        (ControlAction::OpenInEditor(0), (798.8, 10.5, 24.0, 24.0)),
        (ControlAction::Viewed(0), (885.8, 8.5, 84.8, 28.0)),
        (ControlAction::Menu(0), (976.6, 0.0, 23.4, 45.0)),
    ] {
        let at = control(&d, action);
        assert!(
            near_rect(at, rect),
            "{action:?} at {at:?}, expected {rect:?}"
        );
    }
}

#[gpui_kit::test]
fn narrow_header_drops_kind_then_review_pills_then_counts(cx: &mut TestAppContext) {
    use gpui_kit::{px, size};
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        flagged_rename(),
        options(LayoutMode::Unified),
        1000.,
        400.,
    );
    set_all_flags(&view, cx);
    let review = ["changed since viewed", "2 open threads", "agent"];
    let mut seen = Vec::new();
    for width in (280..=1000).rev().step_by(20) {
        cx.simulate_resize(size(px(width as f32), px(400.)));
        settle(cx);
        let d = debug(&view, cx);
        let h = header(&d, 0);
        let kind = h.badges.iter().any(|b| b == "92% similar");
        let shown: Vec<&str> = h
            .badges
            .iter()
            .map(String::as_str)
            .filter(|b| review.contains(b))
            .collect();
        let counts = h.counts.is_some();
        // Review pills go right to left: what is left is a prefix.
        assert_eq!(shown, review[..shown.len()], "at {width}");
        // A review pill goes only once the kind pill is gone; the counts
        // only once every pill is.
        assert!(shown.len() == 3 || !kind, "at {width}: {h:?}");
        assert!(counts || (shown.is_empty() && !kind), "at {width}: {h:?}");
        // The title keeps 12 columns, and the controls stay.
        assert!(h.title.chars().count() >= 12, "at {width}: {h:?}");
        for action in [
            ControlAction::OpenInEditor(0),
            ControlAction::Viewed(0),
            ControlAction::Menu(0),
        ] {
            control(&d, action);
        }
        seen.push((kind, shown.len(), counts));
    }
    // Everything at 1000 px; nothing but the title and controls at 280.
    assert_eq!(seen.first(), Some(&(true, 3, true)));
    assert_eq!(seen.last(), Some(&(false, 0, false)));
}

#[gpui_kit::test]
fn header_pill_shows_both_counts_once_known(cx: &mut TestAppContext) {
    use gpui_kit::{px, size};
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::added("a.rs", "x\ny\nz\n")]);
    let opts = options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let window = cx.open_window(size(px(1000.), px(400.)), move |window, cx| {
        polygloss_viewport::DiffViewport::new(provider, opts, window, cx)
    });
    let view = window.root(cx).unwrap();
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    redraw(cx);
    let d = debug(&view, cx);
    assert_eq!(header(&d, 0).counts, None);
    assert!(!d.painted_text.iter().any(|(_, _, t)| t.starts_with('+')));
    settle(cx);
    let d = debug(&view, cx);
    assert_eq!(header(&d, 0).counts, Some((3, 0)));
    // Both counts, the zero included, in one pill: "+3" at 834.8 + 15.6
    // (the pill is 51 px, as for "+1 −1") and "−0" a column after it.
    let (plus_x, _) = text_at(&d, "+3");
    let (minus_x, _) = text_at(&d, "−0");
    assert!(
        near(minus_x, plus_x + 2.0 * ADVANCE + ADVANCE),
        "{plus_x} {minus_x}"
    );
    let pill = shaped_quads(cx)
        .into_iter()
        .find(|q| q.fill == Some(theme.pill_background) && q.radii[0] > 0.0)
        .expect("a pill");
    assert!(
        near_painted(pill.bounds, (828.8, 11.5, 51.0, 22.0)),
        "{pill:?}"
    );
}

#[gpui_kit::test]
fn header_pill_groups_thousands(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![Spec::added("big.txt", &numbered("l", 1234).concat())]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 300.);
    let d = debug(&view, cx);
    assert_eq!(header(&d, 0).counts, Some((1234, 0)));
    let texts: Vec<&str> = d.painted_text.iter().map(|(_, _, t)| t.as_str()).collect();
    assert!(
        texts.contains(&"+1,234") && texts.contains(&"−0"),
        "{texts:?}"
    );
}

#[gpui_kit::test]
fn open_in_editor_icon_emits_open_in_editor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let old = numbered("line", 10);
    let mut new = old.clone();
    new[4] = "LINE 4\n".to_owned();
    let provider = MemProvider::new(vec![
        Spec::modified("src/a.rs", &old.concat(), &new.concat()),
        Spec::deleted("gone.rs", "x\n"),
    ]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 800.);
    let d = debug(&view, cx);
    let (x, y, w, h) = control(&d, ControlAction::OpenInEditor(0));
    let icon = icons_named(&d, "square-arrow-out-up-right");
    assert!(
        icon.iter()
            .any(|i| i.0 >= x && i.0 + i.2 <= x + w && i.1 >= y && i.1 + i.3 <= y + h),
        "{icon:?} in {:?}",
        (x, y, w, h)
    );
    let (events, _sub) = record_events(&view, cx);
    // The ⋯ menu's target: the new side at the first change; the old side
    // of a deleted file.
    click_control(&view, cx, ControlAction::OpenInEditor(0));
    click_control(&view, cx, ControlAction::OpenInEditor(1));
    assert_eq!(
        events_of(&events),
        [
            ViewportEvent::OpenInEditor {
                file_idx: 0,
                side: Side::New,
                line: 4
            },
            ViewportEvent::OpenInEditor {
                file_idx: 1,
                side: Side::Old,
                line: 0
            }
        ]
    );
}

#[gpui_kit::test]
fn viewed_pill_toggles_viewed(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let opts = options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, two_added(), opts, 1000., 800.);
    let (events, _sub) = record_events(&view, cx);
    let inside = |d: &polygloss_viewport::ViewportDebug, name: &str| {
        let (x, y, w, h) = control(d, ControlAction::Viewed(1));
        icons_named(d, name)
            .iter()
            .any(|i| i.0 >= x && i.0 + i.2 <= x + w && i.1 >= y && i.1 + i.3 <= y + h)
    };
    let d = debug(&view, cx);
    assert!(inside(&d, "square") && !inside(&d, "square-check"));
    // A rounded, bordered pill exactly where the control is.
    let pill = control(&d, ControlAction::Viewed(1));
    assert!(
        shaped_quads(cx).iter().any(|q| near_painted(q.bounds, pill)
            && q.border == theme.card_border
            && q.borders == [1.0; 4]
            && q.radii[0] > 0.0),
        "no pill at {pill:?}"
    );

    click_control(&view, cx, ControlAction::Viewed(1));
    assert_eq!(events_of(&events), [ViewportEvent::ViewedToggled(1)]);
    view.update(cx, |v, cx| {
        v.set_file_flags(
            vec![
                FileFlags::default(),
                FileFlags {
                    viewed: true,
                    ..FileFlags::default()
                },
            ],
            cx,
        )
    });
    settle(cx);
    let d = debug(&view, cx);
    assert!(header(&d, 1).viewed);
    assert!(inside(&d, "square-check") && !inside(&d, "square"));
    let viewed_labels = d
        .painted_text
        .iter()
        .filter(|(_, _, t)| t == "Viewed")
        .count();
    assert_eq!(viewed_labels, 2);
}

#[gpui_kit::test]
fn header_controls_highlight_over_the_card_header(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, point, px};
    let _sb = sandbox();
    let opts = card_options(LayoutMode::Unified);
    let theme = opts.theme.clone();
    let (view, cx) = open(cx, two_added(), opts, 1000., 800.);
    for action in [
        ControlAction::Collapse(0),
        ControlAction::OpenInEditor(0),
        ControlAction::Viewed(0),
        ControlAction::Menu(0),
    ] {
        let (x, y, w, h) = control(&debug(&view, cx), action);
        cx.simulate_mouse_move(
            point(px(x + w / 2.0), px(y + h / 2.0)),
            None,
            Modifiers::default(),
        );
        settle(cx);
        // The highlight is painted after the header's strip and pills, so
        // they do not hide it.
        let all = shaped_quads(cx);
        let lit = all
            .iter()
            .rposition(|q| q.fill == Some(theme.hover))
            .unwrap_or_else(|| panic!("{action:?}: no highlight"));
        assert!(
            near_painted(all[lit].bounds, (x, y, w, h)),
            "{action:?}: {:?}",
            all[lit]
        );
        let strip = all
            .iter()
            .rposition(|q| q.fill == Some(theme.header_background) && q.border == theme.card_border)
            .expect("the header strip");
        assert!(
            lit > strip,
            "{action:?}: highlight {lit} under the strip {strip}"
        );
    }
}
