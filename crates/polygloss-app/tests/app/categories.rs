//! File categories in the app (T6.14, design §11.15, §11.6 "Sections",
//! ADR-0028): the synchronous first partition, the default open rule and
//! the waiting-question rule, hot reloads and palette toggles re-partitioning
//! in the background (latest result only), Generated verdicts flipping in
//! place, the section actions and Explain file category.
//!
//! Expected partitions are written by hand from design §11.15's table and
//! the catalog (`*.test.*`, `tests/`, `Cargo.lock`, `docs/`), never computed
//! by the categorizer under test.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{Action, Entity, TestAppContext, point, px};
use polygloss_app::categories;
use polygloss_app::keymap::actions::categories as category_actions;
use polygloss_app::review_tab::{self, ReviewTab};
use polygloss_app::settings::SettingsStore;
use polygloss_app::viewed;
use polygloss_core::review::{OpenRequest, ThreadKind};
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::{
    ControlAction, CursorPos, RowKey, ScrollAnchor, ScrollTarget, ViewportEvent,
};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox};
use crate::threads::{agent, create, human, line};

/// `name line <n>` for n in 1..=lines.
fn text(name: &str, lines: usize) -> String {
    (1..=lines).map(|n| format!("{name} line {n}\n")).collect()
}

/// `name line <n>` for n in 1..=lines, with line 1 and every 4th line
/// `changed`: one hunk over the whole file, no gap rows.
fn changed(name: &str, lines: usize) -> String {
    (1..=lines)
        .map(|n| {
            if n == 1 || n % 4 == 0 {
                format!("{name} line {n} changed\n")
            } else {
                format!("{name} line {n}\n")
            }
        })
        .collect()
}

/// A repo with tags `base` and `head` where `head` changes line 1 and every
/// 4th line of every file of `paths` (`lines` long each), so every line is
/// in one hunk. Diff order is `paths` sorted by bytes.
pub fn repo_with(paths: &[&str], lines: usize) -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    for p in paths {
        repo.write(p, text(p, lines).as_bytes());
    }
    repo.commit("base");
    repo.git(&["tag", "base"]);
    for p in paths {
        repo.write(p, changed(p, lines).as_bytes());
    }
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

/// The mixed fixture, in diff order: 0 `Cargo.lock` (Generated), 1
/// `docs/x.md` (Docs, off by default), 2 `src/a.rs`, 3 `src/a.test.rs`
/// (Tests), 4 `src/b.rs`, 5 `tests/it.rs` (Tests).
pub const MIXED: [&str; 6] = [
    "Cargo.lock",
    "docs/x.md",
    "src/a.rs",
    "src/a.test.rs",
    "src/b.rs",
    "tests/it.rs",
];

pub fn mixed_repo(lines: usize) -> FixtureRepo {
    repo_with(&MIXED, lines)
}

/// `(category, files, open)` of every section, in display order, as the
/// tab's partition lists them and the viewport shows them.
pub fn sections(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<(String, Vec<u32>, bool)> {
    tab.read_with(shell.cx, |t, cx| {
        let p = categories::partition(t).expect("the tab is partitioned");
        let v = t.viewport.read(cx);
        p.sections
            .iter()
            .map(|s| {
                let open = v
                    .section_open(s.viewport_id)
                    .expect("the viewport has the section");
                (s.id.to_string(), s.files.clone(), open)
            })
            .collect()
    })
}

pub fn main_files(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<u32> {
    tab.read_with(shell.cx, |t, _| {
        categories::partition(t).expect("partitioned").main.clone()
    })
}

pub fn display_order(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<u32> {
    tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).display_order().to_vec()
    })
}

pub fn hidden(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<u32> {
    tab.read_with(shell.cx, |t, cx| {
        let v = t.viewport.read(cx);
        (0..t.opened.files.len() as u32)
            .filter(|&f| v.is_hidden(f))
            .collect()
    })
}

pub fn rows(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<String> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).debug().visible_rows)
}

pub fn anchor(shell: &mut Shell, tab: &Entity<ReviewTab>) -> ScrollAnchor {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).anchor())
}

pub fn top_line(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<(u32, Side, u32)> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).document().top_line())
}

pub fn cursor(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<CursorPos> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor())
}

/// Writes `json` as `settings.json` and reloads it (what the watcher does
/// after an editor saves).
pub fn reload_settings(shell: &mut Shell, json: &str) {
    write_settings(shell, json);
    shell.cx.update(|_, cx| SettingsStore::reload(cx));
    draw(shell.cx);
}

fn write_settings(shell: &mut Shell, json: &str) {
    let path = shell
        .cx
        .update(|_, cx| SettingsStore::global(cx).path().to_owned());
    std::fs::write(&path, json).expect("write settings.json");
}

pub fn focus_viewport(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    shell.cx.update(|window, cx| {
        let focus = tab.read(cx).viewport_focus().clone();
        window.focus(&focus, cx);
    });
    draw(shell.cx);
}

/// Runs a palette action in `tab` (focus in its viewport).
pub fn act(shell: &mut Shell, tab: &Entity<ReviewTab>, action: impl Action) {
    focus_viewport(shell, tab);
    shell.cx.dispatch_action(action);
    draw(shell.cx);
}

pub fn scroll_to_line(shell: &mut Shell, tab: &Entity<ReviewTab>, file_idx: u32, line: u32) {
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

pub fn set_cursor(shell: &mut Shell, tab: &Entity<ReviewTab>, file_idx: u32, line: u32) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| {
        v.set_cursor(
            Some(CursorPos {
                file_idx,
                side: Side::New,
                line,
                range_start: None,
            }),
            cx,
        )
    });
    draw(shell.cx);
}

pub fn go_to_file(shell: &mut Shell, tab: &Entity<ReviewTab>, file_idx: u32) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| v.go_to_file(file_idx, cx));
    draw(shell.cx);
}

/// Clicks a band's control (its Show / Hide or Mark all viewed link), as
/// the user does.
pub fn click_band(shell: &mut Shell, tab: &Entity<ReviewTab>, action: ControlAction) {
    let (x, y, w, h) = tab.read_with(shell.cx, |t, cx| {
        t.viewport
            .read(cx)
            .debug()
            .controls
            .iter()
            .find(|c| c.action == action)
            .unwrap_or_else(|| panic!("no control {action:?} painted"))
            .bounds
    });
    let origin = shell
        .cx
        .debug_bounds("viewport-pane")
        .expect("the viewport pane is painted")
        .origin;
    let at = origin + point(px(x + w / 2.0), px(y + h / 2.0));
    shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
    draw(shell.cx);
}

/// The viewport id of `category`'s section.
pub fn section_id(shell: &mut Shell, tab: &Entity<ReviewTab>, category: &str) -> u32 {
    tab.read_with(shell.cx, |t, _| {
        categories::partition(t)
            .expect("partitioned")
            .sections
            .iter()
            .find(|s| s.id.to_string() == category)
            .unwrap_or_else(|| panic!("no section {category}"))
            .viewport_id
    })
}

fn s(category: &str, files: &[u32], open: bool) -> (String, Vec<u32>, bool) {
    (category.to_owned(), files.to_vec(), open)
}

#[gpui_kit::test]
fn tests_and_generated_files_go_to_closed_sections_by_default(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();

    // Docs is off by default, so `docs/x.md` stays in the main list.
    assert_eq!(main_files(&mut shell, &tab), [1, 2, 4]);
    assert_eq!(
        sections(&mut shell, &tab),
        [s("tests", &[3, 5], false), s("generated", &[0], false)]
    );
    // The viewport: the main files in git order, then each section's.
    assert_eq!(display_order(&mut shell, &tab), [1, 2, 4, 3, 5, 0]);
    assert_eq!(hidden(&mut shell, &tab), [0, 3, 5]);
    let painted = rows(&mut shell, &tab);
    assert!(
        painted.contains(&"▸ 2 test files".to_owned()),
        "{painted:?}"
    );
    assert!(
        painted.contains(&"▸ 1 generated file".to_owned()),
        "{painted:?}"
    );
    for header in ["== src/a.test.rs", "== tests/it.rs", "== Cargo.lock"] {
        assert!(
            !painted.iter().any(|r| r == header),
            "{header}: {painted:?}"
        );
    }
}

#[gpui_kit::test]
fn first_frame_already_has_sections(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    // One main file: before anything loads, its estimated card leaves the
    // band in view.
    let repo = repo_with(
        &["Cargo.lock", "src/a.rs", "src/a.test.rs", "tests/it.rs"],
        3,
    );
    let mut shell = start(cx);
    let first: Rc<RefCell<Option<Vec<String>>>> = Rc::default();
    let seen = first.clone();
    shell.cx.update(|_, cx| {
        review_tab::on_new_tab(cx, move |tab, _, cx| {
            let viewport = tab.read(cx).viewport.clone();
            let seen = seen.clone();
            cx.subscribe(&viewport, move |v, event: &ViewportEvent, cx| {
                if matches!(event, ViewportEvent::FrameStats(_)) && seen.borrow().is_none() {
                    *seen.borrow_mut() = Some(v.read(cx).debug().visible_rows);
                }
            })
            .detach();
        });
    });
    shell.open(compare_req(repo.path())).unwrap();
    let first = first.borrow().clone().expect("a frame was painted");
    assert!(first.contains(&"▸ 2 test files".to_owned()), "{first:?}");
    for path in ["src/a.test.rs", "tests/it.rs"] {
        assert!(
            !first.iter().any(|r| r.contains(path)),
            "{path} in the first frame: {first:?}"
        );
    }
}

#[gpui_kit::test]
fn every_file_categorized_opens_the_sections(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = repo_with(&["Cargo.lock", "src/a.test.rs", "tests/it.rs"], 3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(main_files(&mut shell, &tab), Vec::<u32>::new());
    assert_eq!(
        sections(&mut shell, &tab),
        [s("tests", &[1, 2], true), s("generated", &[0], true)]
    );
    assert_eq!(hidden(&mut shell, &tab), Vec::<u32>::new());
}

/// Opens the review once through core (as an agent's `open_diff` would), so
/// threads can be written before the app shows it.
fn open_in_core(shell: &Shell, req: &OpenRequest) -> polygloss_core::review::OpenedDiff {
    shell.core.open(req).expect("open the diff in core")
}

#[gpui_kit::test]
fn section_with_a_waiting_agent_question_opens_when_threads_load(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let req = compare_req(repo.path());
    let opened = open_in_core(&shell, &req);
    let blobs = polygloss_core::objects::BlobReader::open(&opened.repo).unwrap();
    shell
        .core
        .create_thread(
            &polygloss_core::review::NewThread {
                review_id: opened.review_id.clone(),
                diff_id: opened.diff_id.clone(),
                subject: line("tests/it.rs", Side::New, 1, 1),
                kind: ThreadKind::Question,
                body_md: "Is this fixture still needed?".into(),
                author: agent(),
            },
            &blobs,
        )
        .unwrap();
    let tab = shell.open(req).unwrap();
    // The question waits on the human: Tests opens once the threads load;
    // Generated keeps the default.
    assert_eq!(
        sections(&mut shell, &tab),
        [s("tests", &[3, 5], true), s("generated", &[0], false)]
    );
    // Questions that arrive later only show on the band.
    create(
        &mut shell,
        &tab,
        line("Cargo.lock", Side::New, 1, 1),
        ThreadKind::Question,
        "Why the bump?",
        agent(),
    );
    crate::threads::reload(&mut shell, &tab);
    assert_eq!(
        sections(&mut shell, &tab),
        [s("tests", &[3, 5], true), s("generated", &[0], false)]
    );
}

#[gpui_kit::test]
fn agent_note_in_a_closed_section_shows_on_its_band(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    // An agent note (it waits on no one) and a human comment on a test file.
    create(
        &mut shell,
        &tab,
        line("src/a.test.rs", Side::New, 1, 1),
        ThreadKind::Note,
        "This test covers the new parser.",
        agent(),
    );
    create(
        &mut shell,
        &tab,
        line("src/a.test.rs", Side::New, 2, 2),
        ThreadKind::Comment,
        "Rename this case?",
        human(),
    );
    crate::threads::reload(&mut shell, &tab);
    let state = shell
        .cx
        .update(|_, cx| polygloss_app::ipc::debug_state::snapshot(cx));
    let sections = &state["tabs"][0]["sections"];
    assert_eq!(sections[0]["category"], "tests");
    assert_eq!(sections[0]["open"], false);
    assert_eq!(sections[0]["open_threads"], 1);
    assert_eq!(sections[0]["agent"], true);
    // The band shows it (the section stays closed).
    let band = tab.read_with(shell.cx, |t, cx| {
        t.viewport
            .read(cx)
            .debug()
            .bands
            .into_iter()
            .find(|b| b.label == "2 test files")
            .expect("the Tests band is painted")
    });
    assert!(!band.open);
    assert_eq!(band.badges, ["1 open thread", "agent"]);
}

#[gpui_kit::test]
fn sections_follow_settings_hot_reload_keeping_a_shown_anchor(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(120);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    scroll_to_line(&mut shell, &tab, 2, 60);
    assert_eq!(top_line(&mut shell, &tab), Some((2, Side::New, 60)));

    reload_settings(
        &mut shell,
        r#"{ "categories": { "docs": { "enabled": true } } }"#,
    );
    assert_eq!(main_files(&mut shell, &tab), [2, 4]);
    assert_eq!(
        sections(&mut shell, &tab),
        [
            s("tests", &[3, 5], false),
            s("generated", &[0], false),
            s("docs", &[1], false),
        ]
    );
    assert_eq!(display_order(&mut shell, &tab), [2, 4, 3, 5, 0, 1]);
    // `src/a.rs` is still shown: the same line stays below the header.
    assert_eq!(top_line(&mut shell, &tab), Some((2, Side::New, 60)));
    let painted = rows(&mut shell, &tab);
    assert!(
        painted.iter().any(|r| r.contains("src/a.rs line 61")),
        "{painted:?}"
    );
}

#[gpui_kit::test]
fn hot_reload_hiding_the_anchor_file_lands_on_its_band(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(120);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    scroll_to_line(&mut shell, &tab, 1, 60);
    assert_eq!(top_line(&mut shell, &tab), Some((1, Side::New, 60)));

    reload_settings(
        &mut shell,
        r#"{ "categories": { "docs": { "enabled": true } } }"#,
    );
    assert_eq!(
        sections(&mut shell, &tab).last(),
        Some(&s("docs", &[1], false))
    );
    // Reading `docs/x.md` when Docs went on: the view lands on its band
    // (the top of its section's lead), the section stays closed.
    assert_eq!(
        anchor(&mut shell, &tab),
        ScrollAnchor {
            file_idx: 1,
            row: RowKey::Lead,
            offset_px: 0.0,
        }
    );
    let painted = rows(&mut shell, &tab);
    assert!(
        painted.contains(&"▸ 1 documentation file".to_owned()),
        "{painted:?}"
    );
}

#[gpui_kit::test(iterations = 8)]
fn rapid_settings_reloads_apply_the_latest_partition(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let path = shell
        .cx
        .update(|_, cx| SettingsStore::global(cx).path().to_owned());
    // Two loads before either partition lands; the scheduler (seeded per
    // iteration) finishes their background work in either order.
    std::fs::write(
        &path,
        r#"{ "categories": { "docs": { "enabled": true } } }"#,
    )
    .unwrap();
    shell.cx.update(|_, cx| SettingsStore::reload(cx));
    std::fs::write(
        &path,
        r#"{ "categories": { "docs": { "enabled": true }, "tests": { "enabled": false } } }"#,
    )
    .unwrap();
    shell.cx.update(|_, cx| SettingsStore::reload(cx));
    draw(shell.cx);
    // The second load's partition: no Tests section.
    assert_eq!(main_files(&mut shell, &tab), [2, 3, 4, 5]);
    assert_eq!(
        sections(&mut shell, &tab),
        [s("generated", &[0], false), s("docs", &[1], false)]
    );
    assert_eq!(display_order(&mut shell, &tab), [2, 3, 4, 5, 0, 1]);
}

#[gpui_kit::test]
fn hot_reload_flipping_generated_keeps_anchor_and_loaded_rows(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(120);
    let mut shell = start(cx);
    // Generated off as a category: `src/b.rs` stays in the main list when it
    // becomes generated (Generated is one notion, enabled or not).
    reload_settings(
        &mut shell,
        r#"{ "categories": { "generated": { "enabled": false } } }"#,
    );
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(main_files(&mut shell, &tab), [0, 1, 2, 4]);
    let generated = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            let v = t.viewport.read(cx);
            v.document()
                .files()
                .iter()
                .map(|f| f.generated)
                .collect::<Vec<_>>()
        })
    };
    // `Cargo.lock` is generated by the lockfiles group.
    assert_eq!(
        generated(&mut shell),
        [true, false, false, false, false, false]
    );
    // A thread in `src/a.rs`, a Viewed mark on `src/b.rs`, and the view on
    // line 41 of `src/a.rs`.
    create(
        &mut shell,
        &tab,
        line("src/a.rs", Side::New, 40, 40),
        ThreadKind::Comment,
        "Why?",
        human(),
    );
    crate::threads::reload(&mut shell, &tab);
    tab.update(shell.cx, |t, cx| viewed::toggle_file(t, 4, cx));
    draw(shell.cx);
    scroll_to_line(&mut shell, &tab, 2, 40);
    let before_anchor = anchor(&mut shell, &tab);
    let before_line = top_line(&mut shell, &tab);
    let loads = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).pipeline_stats().loads);
    let blocks = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            t.viewport.read(cx).document().blocks(2).len()
        })
    };
    assert_eq!(blocks(&mut shell), 1);

    reload_settings(
        &mut shell,
        r#"{ "categories": { "generated": { "enabled": false } },
            "diff": { "generated_patterns": ["src/b.rs"] } }"#,
    );
    assert_eq!(
        generated(&mut shell),
        [true, false, false, false, true, false]
    );
    assert_eq!(main_files(&mut shell, &tab), [0, 1, 2, 4]);
    assert_eq!(anchor(&mut shell, &tab), before_anchor);
    assert_eq!(top_line(&mut shell, &tab), before_line);
    let loads_after = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).pipeline_stats().loads);
    assert_eq!(loads_after, loads, "no file loaded again");
    assert_eq!(blocks(&mut shell), 1, "the thread block stays");
    let viewed_b = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).file_flags()[4].viewed);
    assert!(viewed_b, "the Viewed flag stays");
    // `src/b.rs` (viewed, so collapsed) shows the generated placeholder
    // with "Load diff" once expanded.
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| {
        v.set_collapsed(4, false, cx);
        v.scroll_to(ScrollTarget::File(4), cx);
    });
    draw(shell.cx);
    let painted = rows(&mut shell, &tab);
    let at = painted
        .iter()
        .position(|r| r == "== src/b.rs")
        .expect("src/b.rs's header is painted");
    assert_eq!(
        painted.get(at + 1).map(String::as_str),
        Some("Generated file")
    );
}

fn tests_section(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<(String, Vec<u32>, bool)> {
    sections(shell, tab).into_iter().find(|s| s.0 == "tests")
}

#[gpui_kit::test]
fn palette_toggle_changes_only_this_tab(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let one = mixed_repo(3);
    let two = mixed_repo(4);
    let mut shell = start(cx);
    let first = shell.open(compare_req(one.path())).unwrap();
    let second = shell.open(compare_req(two.path())).unwrap();

    act(&mut shell, &second, category_actions::ToggleTests);
    assert_eq!(tests_section(&mut shell, &second), None);
    assert_eq!(main_files(&mut shell, &second), [1, 2, 3, 4, 5]);
    assert_eq!(
        tests_section(&mut shell, &first),
        Some(s("tests", &[3, 5], false))
    );
    // And back on.
    act(&mut shell, &second, category_actions::ToggleTests);
    assert_eq!(
        tests_section(&mut shell, &second),
        Some(s("tests", &[3, 5], false))
    );
}

#[gpui_kit::test]
fn palette_toggle_survives_an_unrelated_hot_reload(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    act(&mut shell, &tab, category_actions::ToggleTests);
    assert_eq!(tests_section(&mut shell, &tab), None);

    reload_settings(
        &mut shell,
        r#"{ "categories": { "docs": { "enabled": true } } }"#,
    );
    assert_eq!(tests_section(&mut shell, &tab), None);
    assert_eq!(main_files(&mut shell, &tab), [2, 3, 4, 5]);
    assert_eq!(
        sections(&mut shell, &tab),
        [s("generated", &[0], false), s("docs", &[1], false)]
    );
}

#[gpui_kit::test]
fn palette_toggle_wins_over_a_reload_of_the_same_key(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    reload_settings(
        &mut shell,
        r#"{ "categories": { "docs": { "enabled": true } } }"#,
    );
    let tab = shell.open(compare_req(repo.path())).unwrap();
    act(&mut shell, &tab, category_actions::ToggleDocs);
    assert_eq!(main_files(&mut shell, &tab), [1, 2, 4]);

    // settings.json turns Docs off, then on again: the tab's toggle wins.
    reload_settings(
        &mut shell,
        r#"{ "categories": { "docs": { "enabled": false } } }"#,
    );
    reload_settings(
        &mut shell,
        r#"{ "categories": { "docs": { "enabled": true } } }"#,
    );
    assert_eq!(main_files(&mut shell, &tab), [1, 2, 4]);
    assert_eq!(
        sections(&mut shell, &tab),
        [s("tests", &[3, 5], false), s("generated", &[0], false)]
    );
    // A tab opened now follows settings.json.
    let other = mixed_repo(4);
    let fresh = shell.open(compare_req(other.path())).unwrap();
    assert_eq!(main_files(&mut shell, &fresh), [2, 4]);
}

#[gpui_kit::test]
fn show_next_hide_and_mark_section_actions(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    set_cursor(&mut shell, &tab, 2, 0);

    // Show Next Section: the first closed section after the cursor opens and
    // the cursor goes to its first line.
    act(&mut shell, &tab, category_actions::ShowNextSection);
    assert_eq!(
        sections(&mut shell, &tab),
        [s("tests", &[3, 5], true), s("generated", &[0], false)]
    );
    assert_eq!(
        cursor(&mut shell, &tab).map(|c| (c.file_idx, c.line)),
        Some((3, 0))
    );
    // Again from there: Generated.
    act(&mut shell, &tab, category_actions::ShowNextSection);
    assert_eq!(
        sections(&mut shell, &tab),
        [s("tests", &[3, 5], true), s("generated", &[0], true)]
    );
    // `Cargo.lock` shows "Load diff": no line for the cursor, its header
    // at the top.
    assert_eq!(cursor(&mut shell, &tab), None);
    assert_eq!(anchor(&mut shell, &tab).file_idx, 0);

    // Hide Section: the anchor's section closes (no cursor).
    act(&mut shell, &tab, category_actions::HideSection);
    assert_eq!(
        sections(&mut shell, &tab),
        [s("tests", &[3, 5], true), s("generated", &[0], false)]
    );
    // The cursor's section when there is one.
    set_cursor(&mut shell, &tab, 5, 0);
    act(&mut shell, &tab, category_actions::HideSection);
    assert_eq!(
        sections(&mut shell, &tab),
        [s("tests", &[3, 5], false), s("generated", &[0], false)]
    );
    assert_eq!(
        cursor(&mut shell, &tab),
        None,
        "the cursor's file is hidden"
    );

    // Mark Section Viewed on the cursor's section (the cursor reopens it).
    set_cursor(&mut shell, &tab, 5, 0);
    act(&mut shell, &tab, category_actions::MarkSectionViewed);
    let viewed = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            t.viewport
                .read(cx)
                .file_flags()
                .iter()
                .map(|f| f.viewed)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(viewed(&mut shell), [false, false, false, true, false, true]);
    // They held the cursor: it jumps from the section's last file to the
    // next unviewed shown file, wrapping past the closed Generated section.
    assert_eq!(cursor(&mut shell, &tab).map(|c| c.file_idx), Some(1));
    // From a main file: the next section below (Tests), which toggles back.
    act(&mut shell, &tab, category_actions::MarkSectionViewed);
    assert_eq!(viewed(&mut shell), [false; 6]);
    act(&mut shell, &tab, category_actions::MarkSectionViewed);
    assert_eq!(viewed(&mut shell), [false, false, false, true, false, true]);
}

#[gpui_kit::test]
fn band_controls_open_and_mark_the_section(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let tests = section_id(&mut shell, &tab, "tests");
    click_band(&mut shell, &tab, ControlAction::SectionMarkViewed(tests));
    let viewed = tab.read_with(shell.cx, |t, _| viewed::states(t).unwrap().to_vec());
    use polygloss_core::review::ViewedState::{NotViewed, Viewed};
    assert_eq!(
        viewed,
        [NotViewed, NotViewed, NotViewed, Viewed, NotViewed, Viewed]
    );
    click_band(&mut shell, &tab, ControlAction::SectionToggle(tests));
    assert_eq!(sections(&mut shell, &tab)[0], s("tests", &[3, 5], true));
}

fn toasts(shell: &mut Shell) -> Vec<String> {
    shell.main.read_with(shell.cx, |m, _| {
        m.toasts().iter().map(|t| t.to_string()).collect()
    })
}

#[gpui_kit::test]
fn explain_file_toasts_the_verdict(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = mixed_repo(3);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();

    set_cursor(&mut shell, &tab, 3, 0);
    act(&mut shell, &tab, category_actions::ExplainFile);
    set_cursor(&mut shell, &tab, 2, 0);
    act(&mut shell, &tab, category_actions::ExplainFile);
    // `Cargo.lock` has no line ("Load diff"): the file at the top.
    go_to_file(&mut shell, &tab, 0);
    act(&mut shell, &tab, category_actions::ExplainFile);
    reload_settings(
        &mut shell,
        r#"{ "categories": { "tests": { "patterns": ["!tests/"] } } }"#,
    );
    set_cursor(&mut shell, &tab, 5, 0);
    act(&mut shell, &tab, category_actions::ExplainFile);
    assert_eq!(
        toasts(&mut shell),
        [
            r#"src/a.test.rs: Tests (pattern "*.test.*", built-in group "unit")"#,
            "src/a.rs: no category",
            r#"Cargo.lock: Generated (pattern "Cargo.lock", built-in group "lockfiles")"#,
            r#"tests/it.rs: no category ("!tests/" skips Tests)"#,
        ]
    );
}
