//! Command palette, cheat sheet and view toggles (T3.2, design §11.4, §11.6,
//! §11.8, §11.9).

use gpui_kit::TestAppContext;
use gpui_kit::component::WindowExt as _;
use polygloss_app::keymap::{KeymapStore, actions};
use polygloss_app::palette::view_toggles::{self, LayoutChoices};
use polygloss_app::palette::{cheat_sheet, command};
use polygloss_app::settings::SettingsStore;
use polygloss_core::git::Source;
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;
use polygloss_diff::rows::Layout;
use polygloss_viewport::{LayoutMode, RowKey, ScrollTarget};

use crate::keymap::wait_until;
use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

fn has_dialog(shell: &mut Shell) -> bool {
    shell.cx.update(|window, cx| window.has_active_dialog(cx))
}

#[gpui_kit::test]
fn palette_lists_every_action_with_binding_hint(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();

    shell.cx.simulate_keystrokes("cmd-k");
    draw(shell.cx);
    assert!(has_dialog(&mut shell));
    let palette = shell
        .cx
        .update(|_, cx| command::current(cx))
        .expect("the palette is open");
    let rows: Vec<command::PaletteRow> =
        palette.read_with(shell.cx, |p, _| p.rows().cloned().collect());
    // Every registered action, once, in registry order.
    let names: Vec<&str> = rows.iter().map(|r| r.action).collect();
    let registry: Vec<&str> = actions::ACTIONS.iter().map(|a| a.name).collect();
    assert_eq!(names, registry);
    // Each with the binding in effect as its hint (none for palette-only).
    let hint = |name: &str| {
        rows.iter()
            .find(|r| r.action == name)
            .and_then(|r| r.hint())
    };
    assert_eq!(hint("window::CommandPalette").as_deref(), Some("⌘K"));
    assert_eq!(
        hint("tab::SubmitReview"),
        Some(command::format_keys("cmd-shift-enter"))
    );
    assert_eq!(
        hint("viewport::ToggleLayout"),
        Some(command::format_keys("s"))
    );
    assert_eq!(
        hint("viewport::ExpandFile"),
        Some(command::format_keys("shift-e"))
    );
    assert_eq!(hint("viewport::LayoutAuto"), None);
    assert_eq!(hint("tab::Snapshot"), None);
    for row in &rows {
        let expected = shell.cx.update(|_, cx| {
            KeymapStore::global(cx)
                .resolved()
                .hint_for(row.action)
                .map(|b| b.keys.clone())
        });
        assert_eq!(row.keys, expected, "{}", row.action);
    }
    // Grouped by where the actions work.
    let groups: Vec<&str> = palette.read_with(shell.cx, |p, _| {
        p.groups().iter().map(|(h, _)| *h).collect()
    });
    assert_eq!(groups, ["Diff", "File tree", "Comment", "Review", "Window"]);

    // Choosing a row runs its action in the tab the palette was opened
    // from: "Split view".
    let before = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).effective_layout());
    assert_eq!(before, Layout::Unified, "the test window is narrow");
    shell.cx.simulate_input("split view");
    draw(shell.cx);
    shell.cx.simulate_keystrokes("enter");
    draw(shell.cx);
    assert!(!has_dialog(&mut shell), "the palette closed");
    let after = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).effective_layout());
    assert_eq!(after, Layout::Split);
    // Focus is back in the viewport: its keys work again.
    shell.cx.simulate_keystrokes("s");
    draw(shell.cx);
    let toggled = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).effective_layout());
    assert_eq!(toggled, Layout::Unified);

    // A keymap.json rebinding shows up in the palette.
    let file = sb.config_dir().join("polygloss/keymap.json");
    std::fs::write(
        &file,
        r#"[{ "context": "Viewport", "bindings": { "s": null, "alt-s": "viewport::ToggleLayout" } }]"#,
    )
    .unwrap();
    shell.cx.update(|_, cx| KeymapStore::reload(cx));
    shell.cx.simulate_keystrokes("cmd-k");
    draw(shell.cx);
    let palette = shell.cx.update(|_, cx| command::current(cx)).unwrap();
    let hint = palette.read_with(shell.cx, |p, _| {
        p.rows()
            .find(|r| r.action == "viewport::ToggleLayout")
            .and_then(|r| r.hint())
    });
    assert_eq!(hint, Some(command::format_keys("alt-s")));
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    assert!(!has_dialog(&mut shell));
}

#[gpui_kit::test]
fn cheat_sheet_opens_on_question_mark(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    // From Home…
    shell.cx.simulate_keystrokes("?");
    draw(shell.cx);
    assert!(has_dialog(&mut shell));
    let sheet = shell
        .cx
        .update(|_, cx| cheat_sheet::current(cx))
        .expect("the cheat sheet is open");
    let sections = sheet.read_with(shell.cx, |s, _| s.sections().to_vec());
    let headings: Vec<&str> = sections.iter().map(|(h, _)| *h).collect();
    assert_eq!(
        headings,
        ["Diff", "File tree", "Comment", "Review", "Window"]
    );
    let diff = &sections[0].1;
    let cursor = diff
        .iter()
        .find(|r| r.action == "viewport::CursorDown")
        .unwrap();
    assert_eq!(cursor.keys, ["j", "down"]);
    assert_eq!(cursor.title, "Move cursor down");
    // Every default binding is on it.
    let listed: usize = sections
        .iter()
        .flat_map(|(_, rows)| rows)
        .map(|r| r.keys.len())
        .sum();
    assert_eq!(
        listed,
        polygloss_app::keymap::defaults::DEFAULT_BINDINGS.len()
    );
    // Esc closes it.
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    assert!(!has_dialog(&mut shell));

    // …and from a review tab's viewport.
    shell.open(compare_req(repo.path())).unwrap();
    shell.cx.simulate_keystrokes("?");
    draw(shell.cx);
    assert!(has_dialog(&mut shell));
    assert!(shell.cx.update(|_, cx| cheat_sheet::current(cx)).is_some());
}

/// A commit on top of `refs/tags/head` of `repo` as a commit review
/// (another diff: the commit of `head` itself is the same diff as
/// `base..head`, diff ids being tree pairs).
fn other_commit_req(repo: &FixtureRepo) -> OpenRequest {
    repo.write("NOTES.md", b"Another change.\n");
    repo.commit("notes");
    repo.git(&["tag", "notes"]);
    OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Commit {
            rev: "refs/tags/notes".into(),
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

#[gpui_kit::test]
fn toggle_split_unified_is_remembered_per_diff(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let layout = |shell: &mut Shell,
                  tab: &gpui_kit::Entity<polygloss_app::review_tab::ReviewTab>| {
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).effective_layout())
    };
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(layout(&mut shell, &tab), Layout::Unified);
    let anchor = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).anchor());

    shell.cx.simulate_keystrokes("s");
    draw(shell.cx);
    assert_eq!(layout(&mut shell, &tab), Layout::Split);
    assert_eq!(
        tab.read_with(shell.cx, |t, _| view_toggles::layout_choice(t)),
        Some(LayoutMode::Split)
    );
    assert_eq!(
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).anchor()),
        anchor,
        "the view stays put"
    );
    // The toolbar shows it.
    assert!(shell.cx.debug_bounds("layout-toggle").is_some());
    // Stored in the diff's view state.
    let diff_id = tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone());
    let stored = |shell: &Shell| {
        shell
            .core
            .load_view_state(&diff_id)
            .unwrap()
            .and_then(|s| s.layout)
    };
    assert_eq!(stored(&shell), Some(Layout::Split));

    // Another diff keeps its own (automatic) layout.
    let other = shell.open(other_commit_req(&repo)).unwrap();
    assert_eq!(layout(&mut shell, &other), Layout::Unified);
    assert_eq!(
        other.read_with(shell.cx, |t, _| view_toggles::layout_choice(t)),
        None
    );

    // A settings reload keeps the choice (it is an override on top).
    let settings = sb.config_dir().join("polygloss/settings.json");
    std::fs::write(&settings, r#"{ "buffer_font": { "size": 14 } }"#).unwrap();
    shell.cx.update(|_, cx| SettingsStore::reload(cx));
    draw(shell.cx);
    assert_eq!(
        tab.read_with(shell.cx, |t, cx| t
            .viewport
            .read(cx)
            .options()
            .code_font_size),
        14.0
    );
    assert_eq!(layout(&mut shell, &tab), Layout::Split);

    // Closed and reopened: split again, from this session's memory…
    close(&mut shell, &tab);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(layout(&mut shell, &tab), Layout::Split);
    // …and after a restart, from the store.
    close(&mut shell, &tab);
    shell
        .cx
        .update(|_, cx| cx.set_global(LayoutChoices::default()));
    let tab = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(layout(&mut shell, &tab), Layout::Split);

    // "Automatic layout" clears the stored choice.
    shell
        .cx
        .update(|window, cx| window.dispatch_action(Box::new(actions::viewport::LayoutAuto), cx));
    draw(shell.cx);
    assert_eq!(layout(&mut shell, &tab), Layout::Unified);
    assert_eq!(stored(&shell), None);
}

/// Closes review tab `tab` with ⌘W.
fn close(shell: &mut Shell, tab: &gpui_kit::Entity<polygloss_app::review_tab::ReviewTab>) {
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

/// Two long files: `a.txt` with whitespace-only changes every 10 lines and
/// one real change, `b.txt` with a real change every 10 lines.
fn whitespace_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let lines = |f: &dyn Fn(usize) -> String| (0..300).map(f).collect::<Vec<_>>().join("\n") + "\n";
    repo.write("a.txt", lines(&|i| format!("alpha {i}")).as_bytes());
    repo.write("b.txt", lines(&|i| format!("beta {i}")).as_bytes());
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write(
        "a.txt",
        lines(&|i| match i {
            150 => "alpha 150 changed".to_owned(),
            i if i % 10 == 5 => format!("alpha  {i}   "),
            i => format!("alpha {i}"),
        })
        .as_bytes(),
    );
    repo.write(
        "b.txt",
        lines(&|i| {
            if i % 10 == 5 {
                format!("beta {i} changed")
            } else {
                format!("beta {i}")
            }
        })
        .as_bytes(),
    );
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

#[gpui_kit::test]
fn toggle_whitespace_recomputes_hunks_keeps_anchors(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = whitespace_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let rows = |shell: &mut Shell| viewport.read_with(shell.cx, |v, _| v.debug().visible_rows);

    // a.txt shows its whitespace-only changes.
    let a_rows = rows(&mut shell);
    assert!(
        a_rows.iter().any(|r| r.contains("- alpha 5")),
        "{a_rows:#?}"
    );
    // Scroll into b.txt, a few rows below its header.
    viewport.update(shell.cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx: 1,
                side: polygloss_diff::Side::New,
                line: 44,
            },
            cx,
        )
    });
    draw(shell.cx);
    let before = (
        viewport.read_with(shell.cx, |v, _| v.anchor()),
        rows(&mut shell),
    );
    assert_eq!(before.0.file_idx, 1);
    assert!(matches!(before.0.row, RowKey::Line { .. }));

    shell.cx.simulate_keystrokes("w");
    draw(shell.cx);
    assert!(viewport.read_with(shell.cx, |v, _| v.options().diff.ignore_whitespace));
    let after = (
        viewport.read_with(shell.cx, |v, _| v.anchor()),
        rows(&mut shell),
    );
    assert_eq!(after, before, "a.txt shrank above; b.txt did not move");

    // a.txt's hunks were recomputed: only the real change is left.
    viewport.update(shell.cx, |v, cx| v.scroll_to(ScrollTarget::File(0), cx));
    draw(shell.cx);
    let a_rows = rows(&mut shell);
    assert!(
        !a_rows.iter().any(|r| r.contains("- alpha 5")),
        "{a_rows:#?}"
    );
    assert!(
        a_rows.iter().any(|r| r.contains("+ alpha 150 changed")),
        "{a_rows:#?}"
    );

    // `w` again shows them again.
    shell.cx.simulate_keystrokes("w");
    draw(shell.cx);
    wait_until(shell.cx, |cx| {
        viewport.read_with(cx, |v, _| {
            v.debug()
                .visible_rows
                .iter()
                .any(|r| r.contains("- alpha 5"))
        })
    });
}
