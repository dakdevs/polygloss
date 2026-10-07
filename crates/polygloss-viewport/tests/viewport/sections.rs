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

    // Flat: a card is 46 + 3 × 20 = 106 pt; a band is 36 pt in the lead of
    // its section's first file. 0, 3, 5 at 0, 106, 212; the band of 7 at
    // 318; 4 at 354, 1 at 460; the closed band of 9 (file 2's lead) at 566.
    let d = debug(&view, cx);
    let headers: Vec<(u32, f32)> = d.headers.iter().map(|h| (h.file_idx, h.y)).collect();
    assert_eq!(
        headers,
        [(0, 0.0), (3, 106.0), (5, 212.0), (4, 354.0), (1, 460.0)]
    );
    let bands: Vec<(u32, f32, bool)> = d.bands.iter().map(|b| (b.id, b.y, b.open)).collect();
    assert_eq!(bands, [(7, 318.0, true), (9, 566.0, false)]);
    assert_eq!(read(&view, cx, |v| v.document().total_height()), 602.0);

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

    // Cards: 0 is 46 + 60 + 8 = 114 pt; 3 is a 12 pt gap and its card
    // (114..240); file 1 is the band's lead, a gap and the band, then the
    // gap below the last thing in the document (240..300); file 2 is
    // nothing.
    read(&view, cx, |v| {
        let doc = v.document();
        assert_eq!(v.display_order(), &[0, 3, 1, 2]);
        assert_eq!(
            (doc.file_top(1), doc.file_height(1), doc.file_height(2)),
            (240.0, 60.0, 0.0)
        );
        assert_eq!(doc.total_height(), 300.0);
    });
    let d = debug(&view, cx);
    let headers: Vec<(u32, f32)> = d.headers.iter().map(|h| (h.file_idx, h.y)).collect();
    assert_eq!(headers, [(0, 0.0), (3, 126.0)]);
    let b = band(&d, 5);
    assert_eq!(
        (b.y, b.open, b.label.as_str()),
        (252.0, false, "2 test files")
    );
    assert_eq!(b.links, ["Show", "Mark all viewed"]);
    let rows = &d.visible_rows;
    assert!(
        !rows.iter().any(|r| r.contains("f1 ") || r.contains("f2 ")),
        "{rows:?}"
    );

    // Open: file 1's card follows the band (header at 240 + 48), file 2's
    // card holds the last gap (414..528 + 12).
    view.update(cx, |v, cx| v.set_section_open(5, true, cx));
    settle(cx);
    let d = debug(&view, cx);
    let headers: Vec<(u32, f32)> = d.headers.iter().map(|h| (h.file_idx, h.y)).collect();
    assert_eq!(headers, [(0, 0.0), (3, 126.0), (1, 288.0), (2, 414.0)]);
    assert_eq!(band(&d, 5).y, 252.0);
    assert_eq!(band(&d, 5).links, ["Hide", "Mark all viewed"]);
    assert_eq!(read(&view, cx, |v| v.document().total_height()), 540.0);

    // Closed again: the band alone.
    view.update(cx, |v, cx| v.set_section_open(5, false, cx));
    settle(cx);
    assert_eq!(read(&view, cx, |v| v.document().total_height()), 300.0);
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
    // Four files of 30 lines: a header and 600 pt each.
    let file_h = HEADER_H + 600.0;
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
    // File 3 is second now: line 10 is at a file, a header and 200 pt,
    // right below the pinned header.
    assert_eq!(
        read(&view, cx, |v| v.document().scroll_top()),
        f64::from(file_h + 200.0)
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
    // Files 0, 3 and 4 come first: the band is at three files' height and
    // the last thing in the document, so the scroll stops at the end with
    // the band in view.
    let file_h = HEADER_H + 600.0;
    assert_eq!(
        read(&view, cx, |v| v.document().scroll_top()),
        f64::from(3.0 * file_h + BAND_H - 300.0)
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
    // File 0 is last: three cards (a header and three rows each) and its
    // band above it.
    let d = debug(&view, cx);
    let (_, y, _, _) = control(&d, ControlAction::Viewed(0));
    let header = d.headers.iter().find(|h| h.file_idx == 0).unwrap();
    assert_eq!(header.y, 3.0 * (HEADER_H + 3.0 * ROW_H) + BAND_H);
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

// ---------------------------------------------------------------------------
// the spacing system (T7.4, ADR-0031 C2)

/// A band's chevron box and label start where the next card's header puts
/// its chevron box and path. A band icon is the label's leading box: with
/// one, the icon starts there and the text 6 pt (`ICON_LABEL`) after the
/// 16 pt icon.
#[gpui_kit::test]
fn bands_align_with_file_headers(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let opts = card_options(LayoutMode::Unified);
    let (view, cx) = open(cx, added_files(3, 2), opts, 1000., 800.);
    let plain = Section {
        icon: None,
        ..section(1, "plain", &[1], true)
    };
    set_sections(&view, cx, vec![plain, section(2, "flask", &[2], true)]);
    let d = debug(&view, cx);
    // Three headers and two bands, all open: one chevron column.
    let chevrons = icons_named(&d, "chevron-down");
    assert_eq!(chevrons.len(), 5, "{chevrons:?}");
    assert!(
        chevrons.iter().all(|c| c.0 == chevrons[0].0 && c.2 == 16.0),
        "{chevrons:?}"
    );
    let (path_1, _) = text_at(&d, "f1.txt");
    let (path_2, _) = text_at(&d, "f2.txt");
    assert_eq!(text_at(&d, "plain").0, path_1);
    let flask = icons_named(&d, "flask-conical");
    assert_eq!(flask.len(), 1, "{flask:?}");
    assert_eq!(flask[0].0, path_2);
    assert!(near(text_at(&d, "flask").0, path_2 + 16.0 + 6.0));
}

// ---------------------------------------------------------------------------
// motion (T7.11, ADR-0030 M4 and M5)

use std::time::{Duration, Instant};

use gpui_kit::{Modifiers, MouseButton, point};
use polygloss_viewport::RevealKind;
use polygloss_viewport::motion::{Initiator, MotionPolicy, Settle};

use crate::reveal::{
    FIRST_STEP, VIEW_H, advance, assert_y, center, frame, header_y, ms, q, requested_frames,
    running, set_policy, slide, step_to,
};

/// Section A (id 1: `a0.txt`, `a1.txt`, 12 lines each) after `m.txt` (3
/// lines), and section B (id 2: `b0.txt`) after it, in cards, unified, in a
/// 900 pt viewport at scale 2. `m.txt`'s card is 46 + 60 + 8 = 114 pt; A's
/// band is 12 below it (126..162); A's cards are 46 + 240 + 8 = 294 pt, 12
/// apart, so A's region below its band is 600 pt open (162..762) and empty
/// closed; B's band is 12 below that: at 774 open, 174 closed.
fn motion_sections(
    cx: &mut TestAppContext,
    a_open: bool,
    height: f32,
) -> (Entity<DiffViewport>, &mut VisualTestContext) {
    let provider = MemProvider::new(vec![
        Spec::added("m.txt", &numbered("m", 3).concat()),
        Spec::added("a0.txt", &numbered("a0", 12).concat()),
        Spec::added("a1.txt", &numbered("a1", 12).concat()),
        Spec::added("b0.txt", &numbered("b0", 3).concat()),
    ]);
    let (view, cx) = open(
        cx,
        provider,
        card_options(LayoutMode::Unified),
        1000.,
        height,
    );
    set_sections(
        &view,
        cx,
        vec![
            section(1, "2 test files", &[1, 2], a_open),
            section(2, "1 generated file", &[3], false),
        ],
    );
    (view, cx)
}

const B_CLOSED: f32 = 174.0;
const B_OPEN: f32 = 774.0;
const REGION: f32 = 600.0;

/// A's band's chevron, as painted.
fn band_chevron(d: &ViewportDebug, id: u32) -> f32 {
    band(d, id).chevron
}

/// Clicks section `id`'s Show / Hide link, as painted.
fn click_show(view: &Entity<DiffViewport>, cx: &mut VisualTestContext, id: u32) {
    let link = *controls_of(&debug(view, cx), ControlAction::SectionToggle(id))
        .first()
        .expect("the band's Show / Hide link");
    crate::reveal::click(cx, center(link));
}

#[gpui_kit::test]
fn section_open_reveals_over_time(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = motion_sections(cx, false, VIEW_H);
    set_policy(cx, MotionPolicy::Full);
    assert_eq!(band(&debug(&view, cx), 2).y, B_CLOSED);
    click_show(&view, cx, 1);
    // reveal(600 pt in a 900 pt viewport) = 150 + 60, capped at 200 ms.
    let total = 200.0;
    assert_eq!(read(&view, cx, |v| v.section_open(1)), Some(true));
    assert!(running(&view, cx));
    let d = debug(&view, cx);
    assert_y(band(&d, 2).y, B_CLOSED, "commit frame");
    assert_eq!(band_chevron(&d, 1), -90.0, "commit frame: closed");
    let reveal = d.reveal.expect("revealing");
    assert_eq!(reveal.kind, RevealKind::Section(1));
    assert_y(reveal.top, 162.0, "the region starts at the band's bottom");
    // Nothing of the region shows on the commit frame.
    assert_eq!(reveal.height, 0.0);
    let region: Vec<_> = d.cards.iter().filter(|c| c.file_idx != 0).collect();
    assert!(region.is_empty(), "commit frame: {region:?}");
    let quads: Vec<_> = shaped_quads(cx)
        .into_iter()
        .filter(|q| q.bounds.1 >= 162.0 && q.bounds.1 < 774.0 && q.bounds.2 > 900.0)
        .collect();
    assert!(
        quads.is_empty(),
        "commit frame paints in the region: {quads:?}"
    );
    step_to(cx, FIRST_STEP);
    let first = f64::from(B_CLOSED) + f64::from(REGION) * slide(16.667 / total);
    assert_y(band(&debug(&view, cx), 2).y, q(first), "first step");
    advance(cx, ms(100.0) - FIRST_STEP);
    let d = debug(&view, cx);
    let half = f64::from(B_CLOSED) + f64::from(REGION) * slide(0.5);
    assert_y(band(&d, 2).y, q(half), "100 ms");
    let angle = f64::from(band_chevron(&d, 1));
    assert!(
        (angle - -90.0 * (1.0 - slide(0.5))).abs() < 1e-2,
        "the chevron turns with the region: {angle}"
    );
    // The section's cards are cut where the region's frame is: nothing of
    // them below it.
    let edge = d.reveal.unwrap().top + d.reveal.unwrap().height;
    for c in d.cards.iter().filter(|c| [1, 2].contains(&c.file_idx)) {
        assert!(c.bounds.1 + c.bounds.3 <= edge + 1e-3, "{c:?} past {edge}");
    }
    let curtain = d.reveal.unwrap().curtain;
    for (text, (y, _)) in d.visible_rows.iter().zip(&d.row_bounds) {
        if text.contains(" a0 ") || text.contains(" a1 ") {
            assert!(*y < curtain, "{text} at {y}, below the curtain {curtain}");
        }
    }
    advance(cx, ms(100.0));
    let d = debug(&view, cx);
    assert_y(band(&d, 2).y, B_OPEN, "200 ms");
    assert_eq!(band_chevron(&d, 1), 0.0);
    assert!(!running(&view, cx));
}

#[gpui_kit::test]
fn section_close_takes_its_exit_duration(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = motion_sections(cx, true, VIEW_H);
    set_policy(cx, MotionPolicy::Full);
    assert_eq!(band(&debug(&view, cx), 2).y, B_OPEN);
    let before = debug(&view, cx);
    click_show(&view, cx, 1);
    assert_eq!(read(&view, cx, |v| v.section_open(1)), Some(false));
    let d = debug(&view, cx);
    assert_y(band(&d, 2).y, B_OPEN, "commit frame");
    // The closing cards show where they were, though the model hides them.
    assert_eq!(header_y(&d, 1), header_y(&before, 1));
    // exit(200 ms) = 150 ms.
    step_to(cx, ms(75.0));
    let d = debug(&view, cx);
    let half = f64::from(B_OPEN) - f64::from(REGION) * slide(0.5);
    assert_y(band(&d, 2).y, q(half), "75 ms");
    assert_y(header_y(&d, 1), header_y(&before, 1), "a0's card stays put");
    // Closed in the model: the closing cards take no clicks.
    assert!(
        !d.controls
            .iter()
            .any(|c| c.action == ControlAction::Collapse(1)),
        "a closing card's chevron is inert"
    );
    advance(cx, ms(75.0));
    assert_y(band(&debug(&view, cx), 2).y, B_CLOSED, "150 ms");
    assert!(!running(&view, cx));
}

#[gpui_kit::test]
fn section_open_commit_stays_under_budget(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // 300 files of 10 lines behind the band of the second file.
    let (view, cx) = open(
        cx,
        added_files(302, 10),
        card_options(LayoutMode::Unified),
        1000.,
        VIEW_H,
    );
    let files: Vec<u32> = (1..301).collect();
    set_sections(&view, cx, vec![section(1, "300 test files", &files, false)]);
    set_policy(cx, MotionPolicy::Full);
    let started = Instant::now();
    click_show(&view, cx, 1);
    frame(cx);
    let elapsed = started.elapsed();
    assert!(running(&view, cx), "the open glides");
    // The M6 timing-test rule: wall-clock budgets only below a load of 4.
    let load = std::process::Command::new("/usr/sbin/sysctl")
        .args(["-n", "vm.loadavg"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.split_whitespace().find_map(|w| w.parse::<f64>().ok()));
    match load {
        Some(load) if load < 4.0 => assert!(
            elapsed < Duration::from_millis(50),
            "the commit frame took {elapsed:?} (load {load})"
        ),
        load => eprintln!("the commit frame took {elapsed:?}; load {load:?}: not asserted"),
    }
}

#[gpui_kit::test]
fn band_off_screen_or_anchor_move_snaps(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // A 100 pt viewport: A's band (126..162) is below it.
    let (view, cx) = motion_sections(cx, false, 100.0);
    set_policy(cx, MotionPolicy::Full);
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.toggle_section_by(0, Initiator::Pointer, window, cx)
        })
    });
    assert_eq!(read(&view, cx, |v| v.section_open(1)), Some(true));
    assert!(!running(&view, cx), "a band off screen snaps");
    assert_eq!(requested_frames(cx), 0);

    // A's band half above the viewport's top (in a 500 pt viewport, where
    // the 822 pt document scrolls): the anchor is in its lead (a0's, 32 pt
    // in), so closing A moves it to the band's top.
    cx.simulate_resize(gpui_kit::size(px(1000.), px(500.)));
    settle(cx);
    wheel(cx, 146.0);
    assert_eq!(
        read(&view, cx, |v| v.anchor()),
        anchor(1, RowKey::Lead, 32.0)
    );
    set_policy(cx, MotionPolicy::Full);
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.toggle_section_by(0, Initiator::Pointer, window, cx)
        })
    });
    assert_eq!(read(&view, cx, |v| v.section_open(1)), Some(false));
    assert_eq!(
        read(&view, cx, |v| v.anchor()),
        anchor(1, RowKey::Lead, 0.0)
    );
    assert!(!running(&view, cx), "a close that moves the anchor snaps");
    assert_eq!(requested_frames(cx), 0);

    // The keyboard (the palette's) and programmatic paths snap.
    for initiator in [Initiator::Keyboard, Initiator::Programmatic] {
        cx.update(|window, cx| {
            view.update(cx, |v, cx| v.toggle_section_by(0, initiator, window, cx))
        });
        assert!(!running(&view, cx), "{initiator:?} snaps");
    }
}

#[gpui_kit::test]
fn explicit_targets_open_sections_without_motion(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = motion_sections(cx, false, VIEW_H);
    type Target = fn(&mut DiffViewport, &mut gpui_kit::Context<DiffViewport>);
    let targets: [(&str, Target); 5] = [
        // A tree row and ⌘P.
        ("go_to_file", |v, cx| v.go_to_file(1, cx)),
        // A thread and a URL.
        ("scroll_to", |v, cx| {
            v.scroll_to(
                ScrollTarget::Line {
                    file_idx: 2,
                    side: Side::New,
                    line: 3,
                },
                cx,
            )
        }),
        ("scroll_to file", |v, cx| {
            v.scroll_to(ScrollTarget::File(2), cx)
        }),
        // Find.
        ("reveal_line", |v, cx| v.reveal_line(1, Side::New, 4, cx)),
        ("set_cursor", |v, cx| v.set_cursor(Some(new_line(2, 1)), cx)),
    ];
    for (what, target) in targets {
        set_policy(cx, MotionPolicy::Off);
        view.update(cx, |v, cx| {
            v.set_section_open(1, false, cx);
            v.scroll_to(ScrollTarget::File(0), cx);
        });
        settle(cx);
        set_policy(cx, MotionPolicy::Full);
        view.update(cx, target);
        frame(cx);
        assert_eq!(read(&view, cx, |v| v.section_open(1)), Some(true), "{what}");
        assert!(!running(&view, cx), "{what} opens without motion");
        assert_eq!(requested_frames(cx), 0, "{what}");
    }
}

#[gpui_kit::test]
fn section_open_reduced_fades_the_region_in(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = motion_sections(cx, false, VIEW_H);
    set_policy(cx, MotionPolicy::Reduced);
    let canvas = read(&view, cx, |v| v.options().theme.canvas);
    click_show(&view, cx, 1);
    let check = |d: &ViewportDebug, what: &str| {
        assert_y(band(d, 2).y, B_OPEN, &format!("{what}: no displacement"));
        assert_eq!(band_chevron(d, 1), 0.0, "{what}: no rotation");
    };
    check(&debug(&view, cx), "commit frame");
    step_to(cx, ms(75.0));
    check(&debug(&view, cx), "75 ms");
    // Half of QUICK: a veil of the canvas over the region, strictly
    // between transparent and opaque.
    let veil = region_veil(cx, canvas).expect("a fade over the region");
    assert!(veil > 0.0 && veil < 1.0, "veil {veil}");
    // Over the cards' headers too: the region fades as one.
    let header = read(&view, cx, |v| v.options().theme.header_background);
    let quads = shaped_quads(cx);
    let strip = quads
        .iter()
        .position(|q| q.fill == Some(header) && q.bounds.1 == 162.0)
        .expect("a0's header strip");
    let over = quads.iter().position(|q| {
        q.fill.is_some_and(|f| f.l == canvas.l && f.a < canvas.a) && q.bounds.1 <= 162.0
    });
    assert!(
        over.is_some_and(|v| v > strip),
        "veil {over:?}, strip {strip}"
    );
    advance(cx, ms(75.0));
    assert_eq!(region_veil(cx, canvas), None);
    assert!(!running(&view, cx));
}

/// The alpha (as a share of `color`'s) of a veil of `color` painted over
/// A's region (162..762).
fn region_veil(cx: &mut VisualTestContext, color: gpui_kit::Hsla) -> Option<f32> {
    shaped_quads(cx).into_iter().find_map(|q| {
        let fill = q.fill?;
        let (_, y, _, h) = q.bounds;
        let same = fill.h == color.h && fill.s == color.s && fill.l == color.l;
        (same && fill.a < color.a && y <= 162.0 && y + h >= 700.0).then_some(fill.a / color.a)
    })
}

#[gpui_kit::test]
fn band_button_mid_reveal_animates_its_own_reveal(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = motion_sections(cx, false, VIEW_H);
    view.update(cx, |v, cx| v.set_collapsed(0, true, cx));
    settle(cx);
    set_policy(cx, MotionPolicy::Full);
    // m.txt opens: 68 pt (its rows and padding), reveal(68) = 156.8 ms.
    let chevron = center(control(&debug(&view, cx), ControlAction::Collapse(0)));
    crate::reveal::click(cx, chevron);
    step_to(cx, ms(60.0));
    let link = *controls_of(&debug(&view, cx), ControlAction::SectionToggle(1))
        .first()
        .unwrap();
    let at = point(px(center(link).0), px(center(link).1));
    // The mouse down freezes the card's reveal (the app's settle root does
    // it before the viewport sees the press), three frames pass, and the
    // mouse up there clicks Show; then the deferred `Settle::Frozen`.
    let now = cx.update(|_, cx| cx.background_executor().now());
    view.update(cx, |v, cx| v.freeze_motion(now, cx));
    cx.simulate_mouse_move(at, None, Modifiers::default());
    cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
    for _ in 0..3 {
        advance(cx, FIRST_STEP);
    }
    assert_eq!(debug(&view, cx).reveal.unwrap().kind, RevealKind::Card);
    cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
    view.update(cx, |v, cx| v.settle_motion(Settle::Frozen, cx));
    frame(cx);
    // m.txt ended open; A's reveal runs from B's band at its settled y.
    let d = debug(&view, cx);
    assert_eq!(read(&view, cx, |v| v.section_open(1)), Some(true));
    let reveal = d.reveal.expect("the section's reveal runs on");
    assert_eq!(reveal.kind, RevealKind::Section(1));
    assert!(!reveal.frozen);
    assert_eq!(d.cards[0].bounds.3, 114.0, "m.txt's card is open");
    assert_y(band(&d, 2).y, B_CLOSED, "the section's commit");
    step_to(cx, ms(100.0));
    let y = band(&debug(&view, cx), 2).y;
    assert!(y > B_CLOSED && y < B_OPEN, "between at ½: {y}");
}

// Mark all viewed (M5): one compound reveal of the cards on screen.

/// Five added files of 10 lines (cards of 254 pt, 12 apart: a 0, b 266, c
/// 532, d 798, e 1,064), in a 900 pt viewport at scale 2.
fn five_cards(cx: &mut TestAppContext) -> (Entity<DiffViewport>, &mut VisualTestContext) {
    open(
        cx,
        added_files(5, 10),
        card_options(LayoutMode::Unified),
        1000.,
        VIEW_H,
    )
}

/// The body of a 10-line card collapsing: 200 pt of rows and 8 of padding.
const BODY: f32 = 208.0;
const D_OPEN: f32 = 798.0;
const D_CLOSED: f32 = D_OPEN - 2.0 * BODY;

fn collapse_many(
    view: &Entity<DiffViewport>,
    cx: &mut VisualTestContext,
    files: &[u32],
    collapsed: bool,
) {
    cx.update(|window, cx| {
        view.update(cx, |v, cx| {
            v.set_collapsed_many_by(files, collapsed, Initiator::Pointer, window, cx)
        })
    });
    frame(cx);
}

#[gpui_kit::test]
fn mark_all_viewed_reveals_only_the_visible_strip(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = five_cards(cx);
    set_policy(cx, MotionPolicy::Full);
    assert_eq!(header_y(&debug(&view, cx), 3), D_OPEN);
    // b and c are on screen; e (at 1,064) is not.
    collapse_many(&view, cx, &[1, 2, 4], true);
    assert!(read(&view, cx, |v| (1..5)
        .filter(|&f| f != 3)
        .all(|f| v.document().is_collapsed(f))));
    let d = debug(&view, cx);
    let bodies: Vec<(RevealKind, u32)> = d.reveals.iter().map(|r| (r.kind, r.file_idx)).collect();
    assert_eq!(bodies, [(RevealKind::Card, 1), (RevealKind::Card, 2)]);
    // The commit frame: d where it was, its committed y 2 · 208 pt higher:
    // the travel is the visible collapsing height.
    assert_y(header_y(&d, 3), D_OPEN, "commit frame");
    // reveal(416) = 191.6 ms; exit(191.6) = 140 ms. Each body's height is
    // quantized on its own (rule 11), and d rides by both.
    step_to(cx, ms(70.0));
    let each = q(f64::from(BODY) * (1.0 - slide(0.5)));
    assert_y(
        header_y(&debug(&view, cx), 3),
        D_CLOSED + 2.0 * each,
        "70 ms",
    );
    // b's frame and c's card follow it.
    let d = debug(&view, cx);
    let b_bottom = crate::reveal::card(&d, 1).1 + crate::reveal::card(&d, 1).3;
    assert_y(
        crate::reveal::card(&d, 2).1,
        b_bottom + 12.0,
        "c rides below b",
    );
    advance(cx, ms(70.0));
    assert_y(header_y(&debug(&view, cx), 3), D_CLOSED, "140 ms");
    assert!(!running(&view, cx));
}

#[gpui_kit::test]
fn mark_all_viewed_commits_files_above_still(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // Eight cards (a document of 2,124 pt) in a 900 pt viewport.
    let (view, cx) = open(
        cx,
        added_files(8, 10),
        card_options(LayoutMode::Unified),
        1000.,
        VIEW_H,
    );
    // b's header at the top: a (0..254) is above the viewport.
    wheel(cx, 266.0);
    let before = debug(&view, cx);
    assert_eq!((header_y(&before, 1), header_y(&before, 3)), (0.0, 532.0));
    set_policy(cx, MotionPolicy::Full);
    collapse_many(&view, cx, &[0, 2], true);
    let d = debug(&view, cx);
    // a closed above without moving anything; c on screen glides.
    let bodies: Vec<u32> = d.reveals.iter().map(|r| r.file_idx).collect();
    assert_eq!(bodies, [2]);
    assert_eq!(header_y(&d, 1), 0.0, "b holds the top");
    assert_y(header_y(&d, 3), 532.0, "commit frame: d where it was");
    // exit(reveal(208)) = 130 ms.
    step_to(cx, ms(150.0));
    assert_y(header_y(&debug(&view, cx), 3), 532.0 - BODY, "settled");
    assert!(!running(&view, cx));

    // At the document's end, collapsing c pulls the content down (the
    // scroll position is clamped): that snaps.
    set_policy(cx, MotionPolicy::Off);
    view.update(cx, |v, cx| {
        v.set_collapsed(0, false, cx);
        v.set_collapsed(2, false, cx);
    });
    settle(cx);
    wheel(cx, 5_000.0);
    set_policy(cx, MotionPolicy::Full);
    collapse_many(&view, cx, &[6], true);
    assert!(!running(&view, cx), "a clamped scroll snaps");
}

#[gpui_kit::test]
fn mark_all_viewed_reduced_fades_then_snaps(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = five_cards(cx);
    set_policy(cx, MotionPolicy::Reduced);
    let background = read(&view, cx, |v| v.options().theme.card_background);
    collapse_many(&view, cx, &[1, 2], true);
    assert_y(header_y(&debug(&view, cx), 3), D_OPEN, "commit frame");
    step_to(cx, ms(50.0));
    assert_y(
        header_y(&debug(&view, cx), 3),
        D_OPEN,
        "50 ms: the frames hold",
    );
    let veils = card_veils(cx, background);
    assert_eq!(veils.len(), 2, "a veil over each body: {veils:?}");
    assert!(veils.iter().all(|&a| a > 0.0 && a < 1.0), "{veils:?}");
    advance(cx, ms(50.0));
    assert_y(header_y(&debug(&view, cx), 3), D_CLOSED, "100 ms: snapped");
    assert!(!running(&view, cx));
}

#[gpui_kit::test]
fn mark_all_unviewed_reduced_fades_rows_in(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = five_cards(cx);
    view.update(cx, |v, cx| {
        v.set_collapsed(1, true, cx);
        v.set_collapsed(2, true, cx);
    });
    settle(cx);
    set_policy(cx, MotionPolicy::Reduced);
    let background = read(&view, cx, |v| v.options().theme.card_background);
    collapse_many(&view, cx, &[1, 2], false);
    assert_y(
        header_y(&debug(&view, cx), 3),
        D_OPEN,
        "commit frame: in place",
    );
    step_to(cx, ms(75.0));
    assert_y(
        header_y(&debug(&view, cx), 3),
        D_OPEN,
        "75 ms: no displacement",
    );
    let veils = card_veils(cx, background);
    assert_eq!(veils.len(), 2, "{veils:?}");
    assert!(veils.iter().all(|&a| a > 0.0 && a < 1.0), "{veils:?}");
    advance(cx, ms(75.0));
    assert!(card_veils(cx, background).is_empty());
    assert!(!running(&view, cx));
}

/// The alphas (as shares of `color`'s) of the veils of `color` painted
/// over a card's 200 pt of rows.
fn card_veils(cx: &mut VisualTestContext, color: gpui_kit::Hsla) -> Vec<f32> {
    shaped_quads(cx)
        .into_iter()
        .filter_map(|q| {
            let fill = q.fill?;
            let same = fill.h == color.h && fill.s == color.s && fill.l == color.l;
            (same && fill.a < color.a && q.bounds.3 >= 190.0).then_some(fill.a / color.a)
        })
        .collect()
}

#[gpui_kit::test]
fn the_last_section_opens_from_its_band(cx: &mut TestAppContext) {
    let _sb = sandbox();
    // A is the document's last section: the gap below the last card is not
    // part of its region, so nothing of it shows on the commit frame.
    let provider = MemProvider::new(vec![
        Spec::added("m.txt", &numbered("m", 3).concat()),
        Spec::added("a0.txt", &numbered("a0", 12).concat()),
        Spec::added("a1.txt", &numbered("a1", 12).concat()),
    ]);
    let (view, cx) = open(
        cx,
        provider,
        card_options(LayoutMode::Unified),
        1000.,
        VIEW_H,
    );
    set_sections(&view, cx, vec![section(1, "2 test files", &[1, 2], false)]);
    set_policy(cx, MotionPolicy::Full);
    click_show(&view, cx, 1);
    let d = debug(&view, cx);
    let reveal = d.reveal.expect("revealing");
    assert_eq!((reveal.top, reveal.height), (162.0, 0.0), "commit frame");
    assert!(d.cards.iter().all(|c| c.file_idx == 0), "{:?}", d.cards);
    // reveal(600) = 200 ms: at ½ the region is 600 · SLIDE(½) tall.
    step_to(cx, ms(100.0));
    let h = debug(&view, cx).reveal.expect("revealing").height;
    assert_y(h, q(f64::from(REGION) * slide(0.5)), "100 ms");
    advance(cx, ms(100.0));
    assert!(!running(&view, cx));
    // Settled: a1's card ends 600 below the band.
    let d = debug(&view, cx);
    let a1 = crate::reveal::card(&d, 2);
    assert_eq!(a1.1 + a1.3, 162.0 + REGION);
}

#[gpui_kit::test]
fn second_band_click_reverses_from_the_painted_height(cx: &mut TestAppContext) {
    let _sb = sandbox();
    let (view, cx) = motion_sections(cx, false, VIEW_H);
    set_policy(cx, MotionPolicy::Full);
    click_show(&view, cx, 1);
    step_to(cx, ms(100.0));
    let painted = debug(&view, cx);
    let b = band(&painted, 2).y;
    assert!(b > B_CLOSED && b < B_OPEN, "{b}");
    // Hide, mid-open: closed in the model, from where it is painted, the
    // cards on screen kept where they were.
    click_show(&view, cx, 1);
    assert_eq!(read(&view, cx, |v| v.section_open(1)), Some(false));
    let d = debug(&view, cx);
    assert_y(band(&d, 2).y, b, "the reversal's commit");
    assert_eq!(header_y(&d, 1), header_y(&painted, 1), "a0 stays put");
    // It closes over exit(200) = 150 ms × the share travelled.
    step_to(cx, ms(60.0));
    let y = band(&debug(&view, cx), 2).y;
    assert!(y > B_CLOSED && y < b, "closing: {y}");
    advance(cx, ms(100.0));
    assert_y(band(&debug(&view, cx), 2).y, B_CLOSED, "closed");
    assert!(!running(&view, cx));
}
