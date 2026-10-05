//! Category sections (T6.17, design §11.6 "Sections", §11.15): sections
//! follow the other files in display order, a section's band is in the lead
//! of its first file, a closed section is its band only, the band's controls
//! and indicators, the anchor policy, explicit and restoring targets, and
//! relabeling files as generated in place (`set_generated`).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::{Entity, IntoElement as _, Styled as _, TestAppContext, VisualTestContext, div, px};
use polygloss_diff::Side;
use polygloss_viewport::{
    BandDebug, BandFlags, BlockAnchor, BlockId, BlockSpec, ControlAction, CursorPos, DiffViewport,
    FileFlags, LayoutMode, RenderBlock, RowKey, ScrollAnchor, ScrollTarget, Section, SectionCounts,
    ViewportDebug, ViewportEvent,
};

use crate::support::*;

// ---------------------------------------------------------------------------
// helpers

/// A section band's height (design §11.6: 36 pt on the canvas).
const BAND_H: f32 = 36.0;

fn section(id: u32, label: &str, files: &[u32], open: bool) -> Section {
    Section {
        id,
        label: label.to_owned().into(),
        icon: Some("icons/flask-conical.svg".into()),
        files: files.to_vec(),
        open,
    }
}

fn set_sections(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, sections: Vec<Section>) {
    view.update(cx, |v, cx| v.set_sections(sections, cx));
    settle(cx);
}

/// `n` added files `f<i>.txt` of `lines` lines each.
fn added_files(n: u32, lines: u32) -> Arc<MemProvider> {
    MemProvider::new(
        (0..n)
            .map(|i| {
                Spec::added(
                    &format!("f{i}.txt"),
                    &numbered(&format!("f{i}"), lines).concat(),
                )
            })
            .collect(),
    )
}

fn band(d: &ViewportDebug, id: u32) -> &BandDebug {
    d.bands
        .iter()
        .find(|b| b.id == id)
        .unwrap_or_else(|| panic!("no band {id}: {:?}", d.bands))
}

fn new_line(file_idx: u32, line: u32) -> CursorPos {
    CursorPos {
        file_idx,
        side: Side::New,
        line,
        range_start: None,
    }
}

/// Scrolls `at` to the top and puts the cursor on it, as a host does to go
/// to a thread (a cursor moved into a file not laid out does not scroll).
fn go_to_line(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, at: CursorPos) {
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: at.file_idx,
                side: at.side,
                line: at.line,
            },
            cx,
        )
    });
    settle(cx);
    view.update(cx, |v, cx| v.set_cursor(Some(at), cx));
    settle(cx);
}

fn anchor(file_idx: u32, row: RowKey, offset_px: f32) -> ScrollAnchor {
    ScrollAnchor {
        file_idx,
        row,
        offset_px,
    }
}

fn line_key(line: u32) -> RowKey {
    RowKey::Line {
        side: Side::New,
        line,
    }
}

/// The section events among recorded `events`, in order.
fn section_events(events: &Rc<RefCell<Vec<ViewportEvent>>>) -> Vec<ViewportEvent> {
    events
        .borrow()
        .iter()
        .filter(|e| {
            matches!(
                e,
                ViewportEvent::SectionToggled { .. } | ViewportEvent::SectionMarkViewed(_)
            )
        })
        .cloned()
        .collect()
}

/// Bounds of every control doing `action` in the last frame, in paint order.
fn controls_of(d: &ViewportDebug, action: ControlAction) -> Vec<(f32, f32, f32, f32)> {
    d.controls
        .iter()
        .filter(|c| c.action == action)
        .map(|c| c.bounds)
        .collect()
}

fn click_bounds(cx: &mut VisualTestContext, (x, y, w, h): (f32, f32, f32, f32)) {
    click_at(cx, x + w / 2.0, y + h / 2.0);
}

/// The visible row whose top is `y`.
fn row_at(d: &ViewportDebug, y: f32) -> &str {
    let i = d
        .row_bounds
        .iter()
        .position(|b| b.0 == y)
        .unwrap_or_else(|| panic!("no row at {y}: {:?}", d.row_bounds));
    &d.visible_rows[i]
}

fn read<R>(
    view: &Entity<DiffViewport>,
    cx: &mut VisualTestContext,
    f: impl FnOnce(&DiffViewport) -> R,
) -> R {
    view.read_with(cx, |v, _| f(v))
}

// ---------------------------------------------------------------------------
// order and geometry

#[gpui_kit::test]
fn display_order_puts_sections_after_the_other_files(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(6, 3),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    let (events, _sub) = record_events(&view, cx);
    // File 4 is listed twice (it stays in the first section), 99 does not
    // exist, and a section without files has no band.
    set_sections(
        &view,
        cx,
        vec![
            section(7, "2 test files", &[4, 1, 99], true),
            section(9, "1 generated file", &[2, 4], false),
            section(11, "nothing", &[], true),
        ],
    );
    read(&view, cx, |v| {
        assert_eq!(v.display_order(), &[0, 3, 5, 4, 1, 2]);
        let of: Vec<Option<u32>> = (0..6).map(|f| v.section_of(f)).collect();
        assert_eq!(of, [None, Some(7), Some(9), None, Some(7), None]);
        assert_eq!(
            (v.section_open(7), v.section_open(9), v.section_open(11)),
            (Some(true), Some(false), None)
        );
        let hidden: Vec<bool> = (0..6).map(|f| v.is_hidden(f)).collect();
        assert_eq!(hidden, [false, false, true, false, false, false]);
    });

    // Flat: a card is 45 + 3 × 20 = 105 px; a band is 36 px in the lead of
    // its section's first file. 0, 3, 5 at 0, 105, 210; the band of 7 at
    // 315; 4 at 351, 1 at 456; the closed band of 9 (file 2's lead) at 561.
    let d = debug(&view, cx);
    let headers: Vec<(u32, f32)> = d.headers.iter().map(|h| (h.file_idx, h.y)).collect();
    assert_eq!(
        headers,
        [(0, 0.0), (3, 105.0), (5, 210.0), (4, 351.0), (1, 456.0)]
    );
    let bands: Vec<(u32, f32, bool)> = d.bands.iter().map(|b| (b.id, b.y, b.open)).collect();
    assert_eq!(bands, [(7, 315.0, true), (9, 561.0, false)]);
    assert_eq!(read(&view, cx, |v| v.document().total_height()), 597.0);

    // The bands are rows of their own, top to bottom with the files.
    let rows = &d.visible_rows;
    let at = |s: &str| {
        rows.iter()
            .position(|r| r == s)
            .unwrap_or_else(|| panic!("no row {s:?}: {rows:?}"))
    };
    assert!(at("== f5.txt") < at("▾ 2 test files"));
    assert!(at("▾ 2 test files") < at("== f4.txt"));
    assert_eq!(rows.last().map(String::as_str), Some("▸ 1 generated file"));
    assert!(!rows.iter().any(|r| r == "== f2.txt"), "{rows:?}");
    // The host's own sections are no event.
    assert_eq!(section_events(&events), []);
}

#[gpui_kit::test]
fn closed_section_is_its_band_only(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(4, 3),
        card_options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    set_sections(&view, cx, vec![section(5, "2 test files", &[1, 2], false)]);

    // Cards: 0 is 45 + 60 + 8 = 113 px; 3 is a 12 px gap and its card
    // (113..238); file 1 is the band's lead, a gap and the band, then the
    // gap below the last thing in the document (238..298); file 2 is
    // nothing.
    read(&view, cx, |v| {
        let doc = v.document();
        assert_eq!(v.display_order(), &[0, 3, 1, 2]);
        assert_eq!(
            (doc.file_top(1), doc.file_height(1), doc.file_height(2)),
            (238.0, 60.0, 0.0)
        );
        assert_eq!(doc.total_height(), 298.0);
    });
    let d = debug(&view, cx);
    let headers: Vec<(u32, f32)> = d.headers.iter().map(|h| (h.file_idx, h.y)).collect();
    assert_eq!(headers, [(0, 0.0), (3, 125.0)]);
    let b = band(&d, 5);
    assert_eq!(
        (b.y, b.open, b.label.as_str()),
        (250.0, false, "2 test files")
    );
    assert_eq!(b.links, ["Show", "Mark all viewed"]);
    let rows = &d.visible_rows;
    assert!(
        !rows.iter().any(|r| r.contains("f1 ") || r.contains("f2 ")),
        "{rows:?}"
    );

    // Open: file 1's card follows the band (header at 238 + 48), file 2's
    // card holds the last gap (411..536 + 12).
    view.update(cx, |v, cx| v.set_section_open(5, true, cx));
    settle(cx);
    let d = debug(&view, cx);
    let headers: Vec<(u32, f32)> = d.headers.iter().map(|h| (h.file_idx, h.y)).collect();
    assert_eq!(headers, [(0, 0.0), (3, 125.0), (1, 286.0), (2, 411.0)]);
    assert_eq!(band(&d, 5).y, 250.0);
    assert_eq!(band(&d, 5).links, ["Hide", "Mark all viewed"]);
    assert_eq!(read(&view, cx, |v| v.document().total_height()), 536.0);

    // Closed again: the band alone.
    view.update(cx, |v, cx| v.set_section_open(5, false, cx));
    settle(cx);
    assert_eq!(read(&view, cx, |v| v.document().total_height()), 298.0);
    let d = debug(&view, cx);
    assert_eq!(d.headers.len(), 2);
}

#[test]
fn a_closed_section_of_10000_files_is_one_slot_per_walk() {
    use polygloss_viewport::{Document, Metrics, SectionFiles, SlotRange};
    // 10,005 estimated files of 440 px (20 rows and a 40 px header); files
    // 1..=10,000 in a closed section, its band in file 1's lead.
    let files = MemProvider::new(
        (0..10_005)
            .map(|i| Spec::modified(&format!("f{i}.txt"), "a\n", "b\n"))
            .collect(),
    );
    let metrics = Metrics {
        header_height: 40.0,
        ..Metrics::default()
    };
    let mut d = Document::new(polygloss_viewport::DiffProvider::files(&*files), metrics);
    d.set_viewport_height(1_000.0);
    d.set_sections(vec![SectionFiles {
        id: 5,
        files: (1..=10_000).collect(),
        open: false,
    }]);
    assert_eq!(d.total_height(), 5.0 * 440.0 + 36.0);
    assert_eq!(&d.display_order()[..5], [0, 10_001, 10_002, 10_003, 10_004]);
    assert_eq!(
        d.sections_in(SlotRange {
            start: 0,
            end: 10_005
        })
        .count(),
        1
    );
    // The whole document: the five shown files and the band's slot.
    let mut walk = d.shown_files(SlotRange {
        start: 0,
        end: 10_005,
    });
    assert_eq!(
        walk.by_ref().collect::<Vec<_>>(),
        [0, 10_001, 10_002, 10_003, 10_004]
    );
    assert_eq!(walk.visits(), 6);
}

// ---------------------------------------------------------------------------
// the band

#[gpui_kit::test]
fn band_controls_toggle_and_mark_viewed(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(4, 3),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    set_sections(&view, cx, vec![section(5, "2 test files", &[1, 2], false)]);
    let (events, _sub) = record_events(&view, cx);

    // The chevron and label open it.
    let toggles = controls_of(&debug(&view, cx), ControlAction::SectionToggle(5));
    assert_eq!(toggles.len(), 2, "the chevron and label, and Show");
    click_bounds(cx, toggles[0]);
    assert_eq!(read(&view, cx, |v| v.section_open(5)), Some(true));
    assert_eq!(
        section_events(&events),
        [ViewportEvent::SectionToggled { id: 5, open: true }]
    );
    let d = debug(&view, cx);
    assert!(d.headers.iter().any(|h| h.file_idx == 2));

    // "Hide" closes it.
    let toggles = controls_of(&d, ControlAction::SectionToggle(5));
    click_bounds(cx, toggles[1]);
    assert_eq!(read(&view, cx, |v| v.section_open(5)), Some(false));
    assert_eq!(
        section_events(&events)[1..],
        [ViewportEvent::SectionToggled { id: 5, open: false }]
    );

    // "Mark all viewed" asks the host; the section stays closed.
    click_control(&view, cx, ControlAction::SectionMarkViewed(5));
    assert_eq!(
        section_events(&events)[2..],
        [ViewportEvent::SectionMarkViewed(5)]
    );
    assert_eq!(read(&view, cx, |v| v.section_open(5)), Some(false));
    assert!(
        events
            .borrow()
            .iter()
            .all(|e| !matches!(e, ViewportEvent::ViewedToggled(_)))
    );
}

#[gpui_kit::test]
fn band_shows_open_threads_agent_and_changed_since_viewed(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(4, 3),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    set_sections(&view, cx, vec![section(5, "2 test files", &[1, 2], false)]);
    let flags = |threads: u32, agent: bool, changed: bool| FileFlags {
        open_threads: threads,
        agent_threads: agent,
        changed_since_viewed: changed,
        viewed: false,
    };
    // File 0 is not in the section: its 5 threads do not count.
    view.update(cx, |v, cx| {
        v.set_file_flags(
            vec![
                flags(5, true, true),
                flags(2, true, false),
                flags(1, false, true),
                flags(0, false, false),
            ],
            cx,
        )
    });
    settle(cx);
    assert_eq!(
        read(&view, cx, |v| v.band_flags(5)),
        BandFlags {
            open_threads: 3,
            agent: true,
            changed_since_viewed: true,
        }
    );
    let d = debug(&view, cx);
    let b = band(&d, 5);
    assert_eq!(b.badges, ["3 open threads", "agent"]);
    assert!(b.dot);
    // The pills' icons are painted on the band's row, and the dot is a
    // 6 px circle in the accent color, as in the tree.
    let on_band = |name: &str| {
        icons_named(&d, name)
            .into_iter()
            .filter(|i| i.1 >= b.y && i.1 + i.3 <= b.y + BAND_H)
            .count()
    };
    assert_eq!((on_band("message-square"), on_band("bot")), (1, 1));
    let accent = read(&view, cx, |v| v.options().theme.accent);
    let dots: Vec<_> = shaped_quads(cx)
        .into_iter()
        .filter(|q| q.fill == Some(accent) && q.bounds.2 == 6.0 && q.bounds.3 == 6.0)
        .collect();
    assert_eq!(dots.len(), 1, "{dots:?}");
    assert_eq!((dots[0].bounds.1, dots[0].radii[0]), (b.y + 15.0, 3.0));

    // New flags replace the indicators.
    view.update(cx, |v, cx| {
        v.set_file_flags(vec![flags(1, false, false); 4], cx)
    });
    settle(cx);
    assert_eq!(
        read(&view, cx, |v| v.band_flags(5)),
        BandFlags {
            open_threads: 2,
            agent: false,
            changed_since_viewed: false,
        }
    );
    let d = debug(&view, cx);
    let b = band(&d, 5);
    assert_eq!(b.badges, ["2 open threads"]);
    assert!(!b.dot);
}

#[gpui_kit::test]
fn band_offers_mark_all_unviewed_once_all_are_viewed(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(4, 3),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    set_sections(&view, cx, vec![section(5, "2 test files", &[1, 2], false)]);
    let viewed = |files: &[u32]| {
        (0..4)
            .map(|f| FileFlags {
                viewed: files.contains(&f),
                ..FileFlags::default()
            })
            .collect::<Vec<_>>()
    };
    let links = |view: &Entity<DiffViewport>, cx: &mut VisualTestContext| {
        band(&debug(view, cx), 5).links.clone()
    };
    // One of two viewed (and a viewed file outside the section).
    view.update(cx, |v, cx| v.set_file_flags(viewed(&[0, 1]), cx));
    settle(cx);
    assert_eq!(links(&view, cx), ["Show", "Mark all viewed"]);
    // Both viewed.
    view.update(cx, |v, cx| v.set_file_flags(viewed(&[1, 2]), cx));
    settle(cx);
    assert_eq!(links(&view, cx), ["Show", "Mark all unviewed"]);
    // The host says otherwise until its next flags.
    view.update(cx, |v, cx| v.set_band_all_viewed(5, false, cx));
    settle(cx);
    assert_eq!(links(&view, cx), ["Show", "Mark all viewed"]);
    view.update(cx, |v, cx| v.set_band_all_viewed(5, true, cx));
    settle(cx);
    assert_eq!(links(&view, cx), ["Show", "Mark all unviewed"]);
}

#[gpui_kit::test]
fn a_long_label_is_cut_before_the_bands_links(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(2, 3),
        options(LayoutMode::Unified),
        400.,
        600.,
    );
    let long = "40 stories, fixtures & i18n files of the design system";
    set_sections(&view, cx, vec![section(5, long, &[1], false)]);
    let d = debug(&view, cx);
    let (show_x, _) = text_at(&d, "Show");
    let (x, _, label) = d
        .painted_text
        .iter()
        .find(|(_, _, t)| t.starts_with("40 stories"))
        .cloned()
        .expect("the label is painted");
    // Cut with an ellipsis, ending before the Show link (and its padding).
    assert!(
        label.ends_with('…') && label.len() < long.len(),
        "{label:?}"
    );
    let right = x + label.chars().count() as f32 * ADVANCE;
    assert!(right <= show_x - ADVANCE, "{right} vs {show_x}");
    // Both links stay; the band row still names the whole label.
    assert_eq!(band(&d, 5).links, ["Show", "Mark all viewed"]);
    assert!(d.visible_rows.contains(&format!("▸ {long}")));
}

#[gpui_kit::test]
fn section_counts_update_when_counts_land(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // a.txt: +3. m.txt: `b` → `b ` and a new `d`: −1 +2, or +1 with
    // whitespace ignored. z.txt: +4, outside the section.
    let provider = MemProvider::new(vec![
        Spec::added("z.txt", &numbered("z", 4).concat()),
        Spec::added("a.txt", &numbered("a", 3).concat()),
        Spec::modified("m.txt", "a\nb\nc\n", "a\nb \nc\nd\n"),
    ]);
    let (view, cx) = open_idle(cx, provider, options(LayoutMode::Unified), 1000., 2000.);
    view.update(cx, |v, cx| {
        v.set_sections(vec![section(5, "2 test files", &[1, 2], false)], cx)
    });
    redraw(cx);
    assert_eq!(
        read(&view, cx, |v| v.section_counts(5)),
        SectionCounts {
            files: 2,
            counted: 0,
            additions: 0,
            deletions: 0,
        }
    );
    assert_eq!(band(&debug(&view, cx), 5).counts, None);

    settle(cx);
    assert_eq!(
        read(&view, cx, |v| v.section_counts(5)),
        SectionCounts {
            files: 2,
            counted: 2,
            additions: 5,
            deletions: 1,
        }
    );
    let d = debug(&view, cx);
    assert_eq!(band(&d, 5).counts, Some((5, 1)));
    let (_, y) = text_at(&d, "+5");
    assert!(y >= band(&d, 5).y && y < band(&d, 5).y + BAND_H);
    text_at(&d, "−1");

    // Ignoring whitespace counts every file again.
    set_options(&view, cx, |o| o.diff.ignore_whitespace = true);
    assert_eq!(
        read(&view, cx, |v| v.section_counts(5)),
        SectionCounts {
            files: 2,
            counted: 2,
            additions: 4,
            deletions: 0,
        }
    );
    assert_eq!(band(&debug(&view, cx), 5).counts, Some((4, 0)));
}

// ---------------------------------------------------------------------------
// the anchor policy

#[gpui_kit::test]
fn set_sections_keeps_a_shown_anchor(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Four files of 30 lines: 45 + 600 px each.
    let (view, cx) = open(
        cx,
        added_files(4, 30),
        options(LayoutMode::Unified),
        1000.,
        300.,
    );
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 3,
                side: Side::New,
                line: 10,
            },
            cx,
        )
    });
    settle(cx);
    let before = read(&view, cx, |v| v.anchor());
    assert_eq!(before, anchor(3, line_key(10), -HEADER_H));

    set_sections(&view, cx, vec![section(5, "2 test files", &[1, 2], false)]);
    assert_eq!(read(&view, cx, |v| v.anchor()), before);
    // File 3 is second now: line 10 is at 645 + 45 + 200, right below the
    // pinned header.
    assert_eq!(
        read(&view, cx, |v| v.document().scroll_top()),
        645.0 + 200.0
    );
    let d = debug(&view, cx);
    assert_eq!(d.visible_rows[0], "== f3.txt");
    assert_eq!(row_at(&d, HEADER_H), unified(None, Some(11), '+', "f3 10"));
}

#[gpui_kit::test]
fn set_sections_hiding_the_anchor_file_moves_it_to_the_band_and_drops_the_cursor(
    cx: &mut TestAppContext,
) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(5, 30),
        options(LayoutMode::Unified),
        1000.,
        300.,
    );
    go_to_line(&view, cx, new_line(2, 12));
    assert_eq!(read(&view, cx, |v| v.anchor().file_idx), 2);

    // File 2 is the section's second file: the anchor goes to the band,
    // the top of file 1's lead.
    set_sections(&view, cx, vec![section(5, "2 test files", &[1, 2], false)]);
    assert_eq!(
        read(&view, cx, |v| (v.anchor(), v.cursor())),
        (anchor(1, RowKey::Lead, 0.0), None)
    );
    // Files 0, 3 and 4 come first: the band is at 3 × 645 and the last
    // thing in the document, so the scroll stops at the end (1,971 − 300)
    // with the band in view.
    assert_eq!(
        read(&view, cx, |v| v.document().scroll_top()),
        f64::from(3.0 * 645.0 + BAND_H - 300.0)
    );
    let d = debug(&view, cx);
    assert_eq!(band(&d, 5).y, 300.0 - BAND_H);
}

#[gpui_kit::test]
fn closing_the_anchors_section_moves_the_anchor_to_the_band(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(5, 30),
        options(LayoutMode::Unified),
        1000.,
        300.,
    );
    set_sections(&view, cx, vec![section(5, "2 test files", &[1, 2], true)]);
    go_to_line(&view, cx, new_line(2, 12));
    let (events, _sub) = record_events(&view, cx);
    assert_eq!(read(&view, cx, |v| v.anchor().file_idx), 2);

    view.update(cx, |v, cx| v.set_section_open(5, false, cx));
    settle(cx);
    assert_eq!(
        read(&view, cx, |v| (v.anchor(), v.cursor())),
        (anchor(1, RowKey::Lead, 0.0), None)
    );
    // The band ends the document: it is at the bottom of the viewport.
    assert_eq!(band(&debug(&view, cx), 5).y, 300.0 - BAND_H);
    // The host closed it: no event.
    assert_eq!(section_events(&events), []);

    // An anchor in another file stays where it is.
    view.update(cx, |v, cx| {
        v.set_section_open(5, true, cx);
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 3,
                side: Side::New,
                line: 4,
            },
            cx,
        );
    });
    settle(cx);
    let at = read(&view, cx, |v| v.anchor());
    view.update(cx, |v, cx| v.set_section_open(5, false, cx));
    settle(cx);
    assert_eq!(read(&view, cx, |v| v.anchor()), at);
}

/// A prelude 72 px tall, as wide as its slot.
fn prelude_72() -> RenderBlock {
    Rc::new(|_, _| div().w_full().h(px(72.)).into_any_element())
}

#[gpui_kit::test]
fn top_anchor_stays_at_the_top_across_set_sections(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(4, 3),
        card_options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    view.update(cx, |v, cx| v.set_prelude(Some(prelude_72()), cx));
    settle(cx);
    assert_eq!(
        read(&view, cx, |v| v.anchor()),
        anchor(0, RowKey::Lead, 0.0)
    );

    // File 0 leaves for a closed section: file 1 is at the top, with the
    // prelude (72 px) and a gap in its lead.
    set_sections(&view, cx, vec![section(5, "1 test file", &[0], false)]);
    assert_eq!(
        read(&view, cx, |v| (v.anchor(), v.document().scroll_top())),
        (anchor(1, RowKey::Lead, 0.0), 0.0)
    );
    let d = debug(&view, cx);
    assert_eq!(d.prelude.map(|p| p.1), Some(0.0));
    assert_eq!(d.headers[0].file_idx, 1);
    assert_eq!(d.headers[0].y, 84.0);

    // Every file in a closed section: the first band is at the top, below
    // the prelude and a gap; the next band follows a gap below it.
    set_sections(
        &view,
        cx,
        vec![
            section(5, "2 test files", &[0, 1], false),
            section(6, "2 generated files", &[2, 3], false),
        ],
    );
    assert_eq!(
        read(&view, cx, |v| (v.anchor(), v.document().scroll_top())),
        (anchor(0, RowKey::Lead, 0.0), 0.0)
    );
    let d = debug(&view, cx);
    assert_eq!(d.prelude.map(|p| p.1), Some(0.0));
    assert!(d.headers.is_empty());
    let bands: Vec<(u32, f32)> = d.bands.iter().map(|b| (b.id, b.y)).collect();
    assert_eq!(bands, [(5, 84.0), (6, 132.0)]);
    // 72 + 12, two bands 12 apart, and the gap below the last one.
    assert_eq!(
        read(&view, cx, |v| v.document().total_height()),
        84.0 + 36.0 + 12.0 + 36.0 + 12.0
    );
}

// ---------------------------------------------------------------------------
// events, targets

#[gpui_kit::test]
fn events_stay_keyed_by_file_idx_after_reordering(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(4, 3),
        options(LayoutMode::Unified),
        1000.,
        2000.,
    );
    set_sections(&view, cx, vec![section(5, "1 test file", &[0], true)]);
    let (events, _sub) = record_events(&view, cx);
    // File 0 is last: three cards (315 px) and its band above it.
    let d = debug(&view, cx);
    let (_, y, _, _) = control(&d, ControlAction::Viewed(0));
    let header = d.headers.iter().find(|h| h.file_idx == 0).unwrap();
    assert_eq!(header.y, 315.0 + BAND_H);
    assert!(y >= header.y && y < header.y + HEADER_H);
    click_control(&view, cx, ControlAction::Viewed(0));
    view.update(cx, |v, cx| v.go_to_file(0, cx));
    settle(cx);
    let seen: Vec<ViewportEvent> = events
        .borrow()
        .iter()
        .filter(|e| !matches!(e, ViewportEvent::FrameStats(_)))
        .cloned()
        .collect();
    assert_eq!(
        seen,
        [
            ViewportEvent::ViewedToggled(0),
            ViewportEvent::VisibleFileChanged(0),
            ViewportEvent::CursorMoved {
                file_idx: 0,
                side: Side::New,
                line: 0,
            },
        ]
    );
}

#[gpui_kit::test]
fn restore_target_in_a_closed_section_lands_on_its_band(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(
        cx,
        added_files(5, 30),
        options(LayoutMode::Unified),
        1000.,
        300.,
    );
    set_sections(&view, cx, vec![section(5, "2 test files", &[1, 2], false)]);
    let (events, _sub) = record_events(&view, cx);

    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Restore {
                file_idx: 2,
                side: Side::New,
                line: 12,
            },
            cx,
        )
    });
    settle(cx);
    assert_eq!(
        read(&view, cx, |v| (v.anchor(), v.section_open(5))),
        (anchor(1, RowKey::Lead, 0.0), Some(false))
    );

    // A live refresh's anchor: the same.
    view.update(cx, |v, cx| {
        v.scroll_to(ScrollTarget::File(0), cx);
        v.scroll_to_anchor(anchor(2, line_key(12), -HEADER_H), cx);
    });
    settle(cx);
    assert_eq!(
        read(&view, cx, |v| (v.anchor(), v.section_open(5))),
        (anchor(1, RowKey::Lead, 0.0), Some(false))
    );
    assert_eq!(band(&debug(&view, cx), 5).y, 300.0 - BAND_H);
    assert_eq!(section_events(&events), []);

    // In a shown file it is a line target.
    view.update(cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Restore {
                file_idx: 3,
                side: Side::New,
                line: 12,
            },
            cx,
        )
    });
    settle(cx);
    assert_eq!(
        read(&view, cx, |v| v.anchor()),
        anchor(3, line_key(12), -HEADER_H)
    );
    let d = debug(&view, cx);
    assert_eq!(row_at(&d, HEADER_H), unified(None, Some(13), '+', "f3 12"));
}

// ---------------------------------------------------------------------------
// set_generated

/// `a.txt`: 40 lines with lines 5 and 30 changed (gaps above, between and
/// below); `b.txt`: 3 lines with the middle one changed; `c.txt`: 3 added
/// lines.
fn three_kinds() -> Arc<MemProvider> {
    let old = numbered("a", 40);
    let mut new = old.clone();
    new[5] = "A 5\n".to_owned();
    new[30] = "A 30\n".to_owned();
    MemProvider::new(vec![
        Spec::modified("a.txt", &old.concat(), &new.concat()),
        Spec::modified("b.txt", "b0\nb1\nb2\n", "b0\nB1\nb2\n"),
        Spec::added("c.txt", &numbered("c", 3).concat()),
    ])
}

fn note() -> RenderBlock {
    Rc::new(|_, _| div().w_full().h(px(40.)).into_any_element())
}

#[gpui_kit::test]
fn set_generated_keeps_every_other_files_state(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = three_kinds();
    let (view, cx) = open(
        cx,
        provider.clone(),
        options(LayoutMode::Unified),
        1000.,
        300.,
    );
    view.update(cx, |v, cx| {
        v.set_file_flags(
            vec![FileFlags {
                viewed: true,
                ..FileFlags::default()
            }],
            cx,
        );
        v.set_blocks(
            0,
            vec![BlockSpec {
                id: BlockId(1),
                anchor: BlockAnchor::Line {
                    side: Side::New,
                    line: 5,
                },
                render: note(),
            }],
            cx,
        );
        v.set_collapsed(2, true, cx);
        v.set_expansions(0, &[[12, 14]], cx);
        v.set_cursor(Some(new_line(0, 6)), cx);
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 0,
                side: Side::New,
                line: 4,
            },
            cx,
        );
    });
    settle(cx);
    type Seen = (
        Vec<polygloss_viewport::BodyRow>,
        Vec<FileFlags>,
        Vec<u32>,
        Vec<(u32, Vec<[u32; 2]>)>,
        Option<CursorPos>,
        ScrollAnchor,
        Option<polygloss_viewport::FileCounts>,
        usize,
    );
    let seen = |view: &Entity<DiffViewport>, cx: &mut VisualTestContext| -> Seen {
        read(view, cx, |v| {
            (
                v.document().file_layout(0).unwrap().rows().to_vec(),
                v.file_flags().to_vec(),
                v.collapsed(),
                v.expansions(),
                v.cursor(),
                v.anchor(),
                v.file_counts(1),
                v.document().blocks(0).len(),
            )
        })
    };
    let before = seen(&view, cx);
    assert!(before.6.is_some(), "b.txt is counted");
    assert!(read(&view, cx, |v| v.document().state(1).is_materialized()));
    let loads = (pipeline_stats(&view, cx).loads, provider.load_count());
    let misses = debug(&view, cx).shaped_cache_misses;

    view.update(cx, |v, cx| v.set_generated(&[(1, true)], cx));
    settle(cx);
    assert_eq!(seen(&view, cx), before);
    assert_eq!(
        (pipeline_stats(&view, cx).loads, provider.load_count()),
        loads,
        "nothing is loaded again"
    );
    // a.txt's rows on screen were not shaped again.
    assert_eq!(debug(&view, cx).shaped_cache_misses, misses);
    read(&view, cx, |v| {
        let files = v.document().files();
        assert_eq!(
            files.iter().map(|f| f.generated).collect::<Vec<_>>(),
            [false, true, false]
        );
    });

    // b.txt shows "Generated file" with "Load diff", and the badge.
    view.update(cx, |v, cx| v.scroll_to(ScrollTarget::File(1), cx));
    settle(cx);
    let d = debug(&view, cx);
    let rows = &d.visible_rows;
    let b = rows
        .iter()
        .position(|r| r == "== b.txt")
        .expect("b.txt shows");
    assert_eq!(rows[b + 1], "Generated file");
    let header = d.headers.iter().find(|h| h.file_idx == 1).unwrap();
    assert!(header.badges.iter().any(|b| b == "generated"));
    control(&d, ControlAction::LoadDiff(1));
}

#[gpui_kit::test]
fn set_generated_moves_an_anchor_on_a_removed_row_to_its_header(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = open(cx, three_kinds(), options(LayoutMode::Unified), 1000., 300.);
    view.update(cx, |v, cx| v.set_cursor(Some(new_line(0, 30)), cx));
    settle(cx);
    let at = read(&view, cx, |v| v.anchor());
    assert_eq!(at.file_idx, 0);
    assert!(!matches!(at.row, RowKey::Lead | RowKey::Header), "{at:?}");

    view.update(cx, |v, cx| v.set_generated(&[(0, true)], cx));
    settle(cx);
    assert_eq!(
        read(&view, cx, |v| (v.anchor(), v.cursor())),
        (anchor(0, RowKey::Header, 0.0), None)
    );
    let d = debug(&view, cx);
    assert_eq!(&d.visible_rows[..2], ["== a.txt", "Generated file"]);
}

#[gpui_kit::test]
fn set_generated_off_loads_the_file(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let provider = MemProvider::new(vec![
        Spec::added("a.txt", &numbered("a", 3).concat()),
        Spec::generated("g.txt", "g0\ng1\n", "g0\nG1\n"),
    ]);
    let (view, cx) = open(cx, provider, options(LayoutMode::Unified), 1000., 600.);
    let d = debug(&view, cx);
    assert!(d.visible_rows.iter().any(|r| r == "Generated file"));
    assert!(!read(&view, cx, |v| v
        .document()
        .state(1)
        .is_materialized()));

    view.update(cx, |v, cx| v.set_generated(&[(1, false)], cx));
    settle(cx);
    assert!(read(&view, cx, |v| v.document().state(1).is_materialized()));
    let d = debug(&view, cx);
    let rows = &d.visible_rows;
    assert!(!rows.iter().any(|r| r == "Generated file"), "{rows:?}");
    assert!(
        rows.contains(&unified(Some(2), None, '-', "g1")),
        "{rows:?}"
    );
    assert!(
        rows.contains(&unified(None, Some(2), '+', "G1")),
        "{rows:?}"
    );
    let g = d.headers.iter().find(|h| h.file_idx == 1).unwrap();
    assert!(!g.badges.iter().any(|b| b == "generated"), "{:?}", g.badges);
}
