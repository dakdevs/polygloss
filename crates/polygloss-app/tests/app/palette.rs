//! Command palette, cheat sheet and view toggles (T3.2, design §11.4, §11.6,
//! §11.8, §11.9).

use gpui_kit::component::WindowExt as _;
use gpui_kit::{TestAppContext, px};
use polygloss_app::keymap::{KeymapStore, actions};
use polygloss_app::palette::view_toggles::{self, LayoutChoices};
use polygloss_app::palette::{cheat_sheet, command, key_cap};
use polygloss_app::settings::SettingsStore;
use polygloss_core::git::Source;
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;
use polygloss_diff::rows::Layout;
use polygloss_viewport::{LayoutMode, RowKey, ScrollTarget};

use crate::keymap::wait_until;
use crate::shell::{Shell, bounds, compare_req, draw, painted, start};
use crate::support::motion::{advance, requested_frames};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

/// A gpui-kit dialog or one of our overlays (⌘K, ⌘P, `?`) is open.
fn has_dialog(shell: &mut Shell) -> bool {
    shell
        .cx
        .update(|window, cx| window.has_active_dialog(cx) || polygloss_app::overlay::is_open(cx))
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
    // Every registered action, once, in registry order; "Check for updates"
    // only with an updater, which a test binary never has (T5.3).
    let names: Vec<&str> = rows.iter().map(|r| r.action).collect();
    let registry: Vec<&str> = actions::ACTIONS
        .iter()
        .map(|a| a.name)
        .filter(|&name| name != polygloss_app::updates::ACTION_NAME)
        .collect();
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
    assert_eq!(
        groups,
        [
            "Diff",
            "File tree",
            "Comment",
            "Threads",
            "Review",
            "File categories",
            "Window"
        ]
    );

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

#[test]
fn key_caps_spell_escape_out() {
    let label = |k: &str| key_cap::key_label(&gpui_kit::Keystroke::parse(k).unwrap());
    // `⎋` reads like a reload arrow: Escape is `Esc`, modifiers first.
    assert_eq!(label("escape"), "Esc");
    assert_eq!(label("cmd-escape"), "⌘Esc");
    assert_eq!(label("shift-escape"), "⇧Esc");
    // Everything else as gpui-kit's `Kbd` spells it.
    assert_eq!(label("cmd-k"), "⌘K");
    assert_eq!(label("cmd-shift-enter"), "⇧⌘⏎");
    assert_eq!(label("down"), "↓");
    assert_eq!(command::format_keys("escape"), "Esc");
    assert_eq!(command::format_keys("g g"), "G G");
    assert!(!command::format_keys("escape").contains('⎋'));
}

#[gpui_kit::test]
fn cheat_sheet_shows_esc_as_text(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    shell.cx.simulate_keystrokes("?");
    draw(shell.cx);
    let sheet = shell
        .cx
        .update(|_, cx| cheat_sheet::current(cx))
        .expect("the cheat sheet is open");
    // The composer's Cancel is bound to Escape…
    let sections = sheet.read_with(shell.cx, |s, _| s.sections().to_vec());
    let cancel = sections
        .iter()
        .flat_map(|(_, rows)| rows)
        .find(|r| r.action == "composer::Cancel")
        .expect("composer::Cancel is listed");
    assert_eq!(cancel.keys, ["escape"]);
    // …and its key cap is drawn with the label `Esc`: the palette's own cap,
    // not gpui-kit's `Kbd` (`kbd:escape`, drawn `⎋`).
    assert!(shell.cx.debug_bounds("key-cap:escape=Esc").is_some());
    assert!(shell.cx.debug_bounds("kbd:escape").is_none());
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
        [
            "Diff",
            "File tree",
            "Comment",
            "Threads",
            "Review",
            "Window"
        ]
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

/// ⌘1 … ⌘8 share one cheat-sheet row, as design §11.9 lists them (eight
/// rows would push the sheet past a 1280×800 window); rebinding one of them
/// lists all eight again, each with its own keys.
#[test]
fn cheat_sheet_folds_the_review_numbers() {
    use polygloss_app::keymap::{UserBinding, resolve};
    let window = |user: &[UserBinding]| {
        cheat_sheet::sections(&resolve(user))
            .into_iter()
            .find(|(h, _)| *h == "Window")
            .expect("a Window section")
            .1
    };
    let rows = window(&[]);
    let numbered: Vec<_> = rows
        .iter()
        .filter(|r| r.action.starts_with("window::ActivateTab"))
        .collect();
    assert_eq!(numbered.len(), 2, "{numbered:?}");
    assert_eq!(numbered[0].title, "Show review 1 to 8");
    let keys: Vec<String> = (1..=8).map(|n| format!("cmd-{n}")).collect();
    assert_eq!(numbered[0].keys, keys);
    assert!(numbered[0].span);
    assert_eq!(
        (
            numbered[1].action,
            numbered[1].title,
            numbered[1].keys.clone()
        ),
        (
            "window::ActivateTab9",
            "Show last review",
            vec!["cmd-9".to_owned()]
        )
    );

    let rebound = window(&[UserBinding {
        keys: "cmd-shift-3".into(),
        action: Some("window::ActivateTab3"),
        context: Some("Window".into()),
    }]);
    let titles: Vec<&str> = rebound
        .iter()
        .filter(|r| r.action.starts_with("window::ActivateTab"))
        .map(|r| r.title)
        .collect();
    assert_eq!(titles.len(), 9, "{titles:?}");
    assert!(rebound.iter().all(|r| !r.span));
}

#[gpui_kit::test]
fn layout_toggle_is_segmented_and_follows_auto(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let selected = |shell: &mut Shell| {
        let marker = bounds(shell.cx, "layout-selected");
        ["layout-split", "layout-unified"]
            .into_iter()
            .find(|s| bounds(shell.cx, s).contains(&marker.center()))
            .expect("the marker is in a segment")
    };
    // Two segments side by side in one track: split, then unified.
    let (track, split, unified) = (
        bounds(shell.cx, "layout-toggle"),
        bounds(shell.cx, "layout-split"),
        bounds(shell.cx, "layout-unified"),
    );
    assert!(split.right() <= unified.left());
    assert!(track.contains(&split.center()) && track.contains(&unified.center()));
    // Automatic: this window is too narrow to split.
    assert_eq!(selected(&mut shell), "layout-unified");
    // A wide window splits, and the toggle follows.
    crate::shell::resize_window(&mut shell, 2800., 900.);
    assert_eq!(
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).effective_layout()),
        Layout::Split
    );
    assert_eq!(selected(&mut shell), "layout-split");
    assert_eq!(
        tab.read_with(shell.cx, |t, _| view_toggles::layout_choice(t)),
        None
    );
    // A click on a segment chooses it.
    crate::shell::click(shell.cx, "layout-unified");
    assert_eq!(
        tab.read_with(shell.cx, |t, _| view_toggles::layout_choice(t)),
        Some(LayoutMode::Unified)
    );
    assert_eq!(selected(&mut shell), "layout-unified");
    crate::shell::click(shell.cx, "layout-split");
    assert_eq!(
        tab.read_with(shell.cx, |t, _| view_toggles::layout_choice(t)),
        Some(LayoutMode::Split)
    );
    assert_eq!(selected(&mut shell), "layout-split");
}

#[gpui_kit::test]
fn display_menu_has_wrap_and_agent_notes(cx: &mut TestAppContext) {
    use actions::{tab as tab_actions, viewport as v};
    use polygloss_app::palette::MenuEntry;
    use polygloss_core::review::ThreadKind;
    use polygloss_diff::Side;

    use crate::threads::{agent, create, line, reload};

    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let entries = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            polygloss_app::features::display_menu_entries(t, cx)
        })
    };
    let toggles = || {
        vec![
            MenuEntry::check("Automatic layout", true, v::LayoutAuto),
            MenuEntry::check("Hide whitespace", false, v::ToggleWhitespace),
            MenuEntry::check("Wrap lines", false, v::ToggleWrap),
            MenuEntry::Separator,
            MenuEntry::check("Word diff", true, v::WordDiffWord),
            MenuEntry::check("Character diff", false, v::WordDiffChar),
            MenuEntry::check("No inline highlights", false, v::WordDiffOff),
        ]
    };
    // No agent notes: no notes item.
    assert_eq!(entries(&mut shell), toggles());

    for (n, body) in [(1, "First note."), (3, "Second note.")] {
        let subject = line("src/config.rs", Side::New, n, n);
        create(&mut shell, &tab, subject, ThreadKind::Note, body, agent());
    }
    reload(&mut shell, &tab);
    let with_notes = |label: &str| {
        let mut all = toggles();
        all.push(MenuEntry::Separator);
        all.push(MenuEntry::action(label, tab_actions::ToggleAgentNotes));
        all
    };
    assert_eq!(entries(&mut shell), with_notes("Hide agent notes"));
    shell.cx.dispatch_action(tab_actions::ToggleAgentNotes);
    draw(shell.cx);
    assert_eq!(entries(&mut shell), with_notes("Show agent notes (2)"));

    // The items say what is on: wrap toggled from the palette.
    shell.cx.dispatch_action(v::ToggleWrap);
    draw(shell.cx);
    assert!(entries(&mut shell).contains(&MenuEntry::check("Wrap lines", true, v::ToggleWrap)));
    // The button that opens it is painted.
    assert!(painted(shell.cx, "view-options").is_some());
}

#[gpui_kit::test]
fn toggle_wrap_reaches_the_viewport_options(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let wrap = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).options().style.wrap)
    };
    assert!(!wrap(&mut shell), "off by default (diff.style.wrap)");
    shell.cx.dispatch_action(actions::viewport::ToggleWrap);
    draw(shell.cx);
    assert!(wrap(&mut shell));
    assert_eq!(
        tab.read_with(shell.cx, |t, _| view_toggles::overrides(t).wrap),
        Some(true)
    );
    // A settings reload keeps the tab's choice (it is an override on top).
    let settings = sb.config_dir().join("polygloss/settings.json");
    std::fs::write(&settings, r#"{ "buffer_font": { "size": 14 } }"#).unwrap();
    shell.cx.update(|_, cx| SettingsStore::reload(cx));
    draw(shell.cx);
    let size = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).options().code_font_size
    });
    assert_eq!(size, 14.0, "the reload applied");
    assert!(wrap(&mut shell), "kept over the reload");
    shell.cx.dispatch_action(actions::viewport::ToggleWrap);
    draw(shell.cx);
    assert!(!wrap(&mut shell));
}

// ---------------------------------------------------------------- T7.6
// Overlay geometry (ADR-0031: one overlay frame, one row ladder, one header
// edge). Expected heights are AppKit's ladder, written by hand.

/// Twelve commits, then twenty new files in the worktree: the palette, the
/// finder and the base picker each have more rows than fit.
fn crowded_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    for i in 0..12 {
        repo.write("log.txt", format!("{i}\n").as_bytes());
        repo.commit(&format!("Step {i}"));
    }
    for i in 0..20 {
        repo.write(&format!("src/file_{i:02}.rs"), b"fn f() {}\n");
    }
    repo
}

/// The working tree of `repo` since HEAD.
fn live_head(repo: &FixtureRepo) -> OpenRequest {
    OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Live {
            since: polygloss_core::git::Since::Head,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

/// Opens the base picker of the active live review and waits for its log.
fn open_base_picker(shell: &mut Shell) {
    shell
        .cx
        .dispatch_action(polygloss_app::keymap::actions::tab::ChooseBase);
    draw(shell.cx);
    wait_until(shell.cx, |cx| {
        cx.update(|_, cx| {
            polygloss_app::live::base_picker::current(cx)
                .is_some_and(|p| !p.read(cx).delegate().loading())
        })
    });
    draw(shell.cx);
}

fn close_overlay(shell: &mut Shell) {
    shell.cx.simulate_keystrokes("escape");
    draw(shell.cx);
    assert!(!has_dialog(shell));
}

#[gpui_kit::test]
fn pickers_share_one_frame(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = crowded_repo();
    let mut shell = start(cx);
    // gpui-kit's dialogs slide in; settled, each sits where it rests.
    shell.cx.update(|_, cx| {
        use polygloss_app::motion::{MotionPolicy, set_override};
        set_override(Some(MotionPolicy::Off), cx)
    });
    shell.open(live_head(&repo)).unwrap();

    shell.cx.simulate_keystrokes("cmd-k");
    draw(shell.cx);
    let palette = bounds(shell.cx, "command-palette");
    let last = shell.cx.update(|_, cx| {
        let p = command::current(cx).expect("the palette is open");
        p.read(cx).rows().last().unwrap().action
    });
    assert!(
        painted(shell.cx, &format!("palette-row-{last}")).is_none(),
        "the palette's list is at its maximum height"
    );
    close_overlay(&mut shell);

    shell.cx.simulate_keystrokes("cmd-p");
    draw(shell.cx);
    let finder = bounds(shell.cx, "file-finder");
    let files = shell.cx.update(|_, cx| {
        let f = polygloss_app::tree::finder::current(cx).expect("the finder is open");
        f.read(cx).delegate().matches().len()
    });
    assert!(files >= 20, "{files} files");
    assert!(
        painted(shell.cx, &format!("finder-row-{}", files - 1)).is_none(),
        "the finder's list is at its maximum height"
    );
    close_overlay(&mut shell);

    open_base_picker(&mut shell);
    let picker = bounds(shell.cx, "base-picker");
    let choices = shell.cx.update(|_, cx| {
        let p = polygloss_app::live::base_picker::current(cx).expect("the picker is open");
        p.read(cx).delegate().matches().len()
    });
    assert!(choices >= 14, "{choices} bases");
    assert!(
        painted(shell.cx, &format!("base-row-{}", choices - 1)).is_none(),
        "the base picker's list is at its maximum height"
    );
    close_overlay(&mut shell);

    // One width, one left edge, one top and one maximum height. The base
    // picker is still a gpui-kit dialog: the kit centres its popup's host,
    // then rounds the popup's offset to whole points, so a kit dialog lands
    // half a point off its top when its host's centring is (T7.12).
    for (name, other, slack) in [
        ("the finder", finder, 0.0),
        ("the base picker", picker, 0.5),
    ] {
        assert_eq!(other.size.width, palette.size.width, "{name}'s width");
        assert_eq!(other.left(), palette.left(), "{name}'s left edge");
        assert!(
            (other.top() - palette.top()).abs() <= gpui_kit::px(slack),
            "{name}'s top: {:?} against {:?}",
            other.top(),
            palette.top()
        );
        assert_eq!(
            other.size.height, palette.size.height,
            "{name}'s maximum height"
        );
    }
}

#[gpui_kit::test]
fn overlay_rows_follow_the_ladder(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = crowded_repo();
    let mut shell = start(cx);
    shell.open(live_head(&repo)).unwrap();
    let height = |shell: &mut Shell, name: &str| f32::from(bounds(shell.cx, name).size.height);

    // The palette: gpui-kit's rows (6 above and below one line) at
    // `text::UI`'s 20 pt line: 32, on the grid, though off the ladder.
    shell.cx.simulate_keystrokes("cmd-k");
    draw(shell.cx);
    let (first, second) = shell.cx.update(|_, cx| {
        let p = command::current(cx).expect("the palette is open");
        let rows = &p.read(cx).groups()[0].1;
        (rows[0].action, rows[1].action)
    });
    let top = |shell: &mut Shell, action: &str| {
        f32::from(bounds(shell.cx, &format!("palette-row-{action}")).top())
    };
    assert_eq!(
        top(&mut shell, second) - top(&mut shell, first),
        32.0,
        "a palette row"
    );
    close_overlay(&mut shell);

    // The finder: one-line rows, MD.
    shell.cx.simulate_keystrokes("cmd-p");
    draw(shell.cx);
    assert_eq!(height(&mut shell, "finder-row-0"), 28.0, "a finder row");
    close_overlay(&mut shell);

    // Two-line rows, ROW2: the base picker, the open flow's repos and
    // commits.
    open_base_picker(&mut shell);
    assert_eq!(height(&mut shell, "base-row-0"), 44.0, "a base");
    close_overlay(&mut shell);
    shell.cx.simulate_keystrokes("cmd-o");
    draw(shell.cx);
    assert_eq!(height(&mut shell, "open-flow-repo-row-0"), 44.0, "a repo");
    let flow = shell
        .cx
        .update(|_, cx| polygloss_app::open_flow::current(cx))
        .expect("the open flow is open");
    let path = repo.path().to_path_buf();
    shell.cx.update(|window, cx| {
        flow.update(cx, |f, cx| {
            f.choose_repo(
                path,
                polygloss_app::motion::Initiator::Programmatic,
                window,
                cx,
            )
        })
    });
    draw(shell.cx);
    let source = flow
        .read_with(shell.cx, |f, _| f.source().cloned())
        .expect("the source step");
    shell.cx.update(|window, cx| {
        source.update(cx, |s, cx| {
            s.set_mode(
                polygloss_app::open_flow::source_step::SourceMode::Commit,
                window,
                cx,
            )
        })
    });
    draw(shell.cx);
    assert_eq!(
        height(&mut shell, "open-flow-commit-row-0"),
        44.0,
        "a commit"
    );
    close_overlay(&mut shell);

    // The cheat sheet: a leaf result list, SM.
    shell.cx.simulate_keystrokes("?");
    draw(shell.cx);
    assert_eq!(
        height(&mut shell, "cheat-row-viewport::CursorDown"),
        24.0,
        "a cheat-sheet row"
    );
}

#[gpui_kit::test]
fn overlay_headers_and_rows_share_an_edge(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = crowded_repo();
    let mut shell = start(cx);
    shell.open(live_head(&repo)).unwrap();
    // A header's text starts on its rows' content column: their first box
    // (a check slot, an icon or the text itself).
    let left = |shell: &mut Shell, name: &str| bounds(shell.cx, name).left();

    open_base_picker(&mut shell);
    assert_eq!(
        left(&mut shell, "base-picker-header"),
        left(&mut shell, "base-choice-0"),
        "the base picker"
    );
    close_overlay(&mut shell);

    shell.cx.simulate_keystrokes("cmd-o");
    draw(shell.cx);
    assert_eq!(
        left(&mut shell, "open-flow-repo-header"),
        left(&mut shell, "open-flow-repo-0"),
        "the open flow's repositories"
    );
    close_overlay(&mut shell);

    shell.cx.simulate_keystrokes("?");
    draw(shell.cx);
    assert_eq!(
        left(&mut shell, "cheat-heading-Diff"),
        left(&mut shell, "cheat-row-viewport::CursorDown"),
        "the cheat sheet"
    );
}

#[gpui_kit::test]
fn base_picker_keeps_its_heading_when_nothing_matches(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = crowded_repo();
    let mut shell = start(cx);
    // gpui-kit's dialogs slide in; settled, each sits where it rests.
    shell.cx.update(|_, cx| {
        use polygloss_app::motion::{MotionPolicy, set_override};
        set_override(Some(MotionPolicy::Off), cx)
    });
    shell.open(live_head(&repo)).unwrap();
    open_base_picker(&mut shell);
    // The heading's place in the picker's frame.
    let place = |shell: &mut Shell| {
        let frame = bounds(shell.cx, "base-picker");
        let heading = bounds(shell.cx, "base-picker-header");
        (heading.origin - frame.origin, heading.size)
    };
    let heading = place(&mut shell);
    shell.cx.simulate_input("no such base qqzz");
    draw(shell.cx);
    let matches = shell.cx.update(|_, cx| {
        let p = polygloss_app::live::base_picker::current(cx).expect("the picker is open");
        p.read(cx).delegate().matches().len()
    });
    assert_eq!(matches, 0, "nothing matches");
    assert_eq!(place(&mut shell), heading, "the heading stays where it was");
}

#[gpui_kit::test]
fn key_caps_use_the_small_radius(cx: &mut TestAppContext) {
    use gpui_kit::Styled as _;
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    shell.cx.simulate_keystrokes("?");
    draw(shell.cx);
    // A cap as the cheat sheet draws it is at most 20 pt tall, so the height
    // rule gives it the small radius: 4.
    let cap = bounds(shell.cx, "key-cap:escape=Esc");
    assert!(cap.size.height <= gpui_kit::px(20.), "{cap:?}");
    let escape = gpui_kit::Keystroke::parse("escape").unwrap();
    let mut element = shell.cx.update(|_, cx| key_cap::key_cap(&escape, cx));
    let radii = element.style().corner_radii.clone();
    for corner in [
        radii.top_left,
        radii.top_right,
        radii.bottom_right,
        radii.bottom_left,
    ] {
        assert_eq!(corner, Some(gpui_kit::px(4.).into()));
    }
}

// ---------------------------------------------------------------- T7.12
// Instant overlays (ADR-0030 "Inherited gpui-kit motion", OQ-49): ⌘K, ⌘P
// and `?` open and close on one frame for every initiator, on Home and in
// a review. These tests set no motion override: under the Full policy a
// gpui-kit dialog slides and fades in for 250 ms, requesting a frame each
// display frame.

/// What an overlay test opens: how, the selector that is painted while it
/// is open, and rows inside it.
struct Overlay {
    name: &'static str,
    keys: &'static str,
    action: fn() -> Box<dyn gpui_kit::Action>,
    painted: &'static str,
    rows: [&'static str; 2],
    /// gpui-kit's `Command` lays its rows out again at its list's width on
    /// the frame after its first (`CommandState::measure_on_next_frame`).
    remeasures: bool,
}

const PALETTE: Overlay = Overlay {
    name: "the palette",
    keys: "cmd-k",
    action: || Box::new(actions::window::CommandPalette),
    painted: "command-palette",
    rows: [
        "palette-row-viewport::CursorDown",
        "palette-row-viewport::CursorUp",
    ],
    remeasures: true,
};
const FINDER: Overlay = Overlay {
    name: "the finder",
    keys: "cmd-p",
    action: || Box::new(actions::window::FileFinder),
    painted: "file-finder",
    rows: ["finder-row-0", "finder-row-1"],
    remeasures: false,
};
const CHEAT_SHEET: Overlay = Overlay {
    name: "the cheat sheet",
    keys: "?",
    action: || Box::new(actions::window::CheatSheet),
    painted: "cheat-heading-Diff",
    rows: [
        "cheat-row-viewport::CursorDown",
        "cheat-row-window::CheatSheet",
    ],
    remeasures: false,
};

/// The policy a gpui-kit dialog would animate under, and nothing running.
fn full_policy_at_rest(shell: &mut Shell) {
    use polygloss_app::motion::{MotionPolicy, policy};
    assert_eq!(
        shell.cx.update(|_, cx| policy(cx)),
        MotionPolicy::Full,
        "no override and Reduce Motion off"
    );
    requested_frames(shell.cx);
    assert_eq!(requested_frames(shell.cx), 0, "nothing runs before");
}

/// Opens `overlay` (its keys, or its action dispatched where the keyboard
/// is, as the menu bar does), with `previous` holding the keyboard: on the
/// commit frame it and its rows are painted where they rest and it has the
/// keyboard (its search field when `field` gives one); it requests no frame
/// but the palette's one re-measure, which moves nothing. Esc takes it off
/// on the next frame and gives the keyboard back.
fn opens_and_closes_on_one_frame(
    shell: &mut Shell,
    overlay: &Overlay,
    previous: &gpui_kit::FocusHandle,
    field: &dyn Fn(&mut Shell) -> Option<gpui_kit::FocusHandle>,
) {
    for by_action in [false, true] {
        let how = if by_action {
            "its action"
        } else {
            overlay.keys
        };
        shell.cx.update(|window, cx| window.focus(previous, cx));
        draw(shell.cx);
        full_policy_at_rest(shell);
        if by_action {
            let action = (overlay.action)();
            shell
                .cx
                .update(|window, cx| window.dispatch_action(action, cx));
        } else {
            shell.cx.simulate_keystrokes(overlay.keys);
        }
        let at_rest = |shell: &mut Shell| {
            [overlay.painted, overlay.rows[0], overlay.rows[1]].map(|name| {
                painted(shell.cx, name).unwrap_or_else(|| {
                    panic!("{} paints {name} on its first frame ({how})", overlay.name)
                })
            })
        };
        let first = at_rest(shell);
        let focused = shell
            .cx
            .update(|window, cx| previous.contains_focused(window, cx));
        assert!(!focused, "{} has the keyboard ({how})", overlay.name);
        if let Some(field) = field(shell) {
            let in_field = shell
                .cx
                .update(|window, cx| field.contains_focused(window, cx));
            assert!(
                in_field,
                "{}'s search field has the keyboard ({how})",
                overlay.name
            );
        }
        assert_eq!(
            requested_frames(shell.cx),
            usize::from(overlay.remeasures),
            "{} requests no frame after its first ({how})",
            overlay.name
        );
        // Well past gpui-kit's 250 ms entrance: still where the first frame
        // put it, and nothing left running.
        advance(shell.cx, std::time::Duration::from_millis(400));
        assert_eq!(
            at_rest(shell),
            first,
            "{} rests where its first frame put it ({how})",
            overlay.name
        );
        assert_eq!(requested_frames(shell.cx), 0, "nothing runs ({how})");

        shell.cx.simulate_keystrokes("escape");
        assert!(
            painted(shell.cx, overlay.painted).is_none(),
            "Esc takes {} off on the next frame ({how})",
            overlay.name
        );
        let back = shell.cx.update(|window, _| previous.is_focused(window));
        assert!(back, "the keyboard is back after {} ({how})", overlay.name);
        assert_eq!(requested_frames(shell.cx), 0, "nothing runs after ({how})");
    }
}

fn palette_field(shell: &mut Shell) -> Option<gpui_kit::FocusHandle> {
    use gpui_kit::Focusable as _;
    shell.cx.update(|_, cx| {
        let palette = command::current(cx).expect("the palette is open");
        let state = palette.read(cx).state().clone();
        Some(state.read(cx).focus_handle(cx))
    })
}

fn finder_field(shell: &mut Shell) -> Option<gpui_kit::FocusHandle> {
    use gpui_kit::Focusable as _;
    shell.cx.update(|_, cx| {
        let finder = polygloss_app::tree::finder::current(cx).expect("the finder is open");
        Some(finder.read(cx).focus_handle(cx))
    })
}

/// A review of `code_change_repo` with the keyboard in its diff.
fn review_with_diff_focused(shell: &mut Shell, repo: &FixtureRepo) -> gpui_kit::FocusHandle {
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let focus = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    shell.cx.update(|window, cx| window.focus(&focus, cx));
    draw(shell.cx);
    focus
}

#[gpui_kit::test]
fn palette_opens_and_closes_on_one_frame(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let diff = review_with_diff_focused(&mut shell, &repo);
    opens_and_closes_on_one_frame(&mut shell, &PALETTE, &diff, &palette_field);
}

#[gpui_kit::test]
fn finder_opens_and_closes_on_one_frame(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let diff = review_with_diff_focused(&mut shell, &repo);
    opens_and_closes_on_one_frame(&mut shell, &FINDER, &diff, &finder_field);
}

#[gpui_kit::test]
fn cheat_sheet_opens_and_closes_on_one_frame(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    // On Home…
    let home = home_focus(&mut shell);
    opens_and_closes_on_one_frame(&mut shell, &CHEAT_SHEET, &home, &|_| None);
    // …and in a review.
    let diff = review_with_diff_focused(&mut shell, &repo);
    opens_and_closes_on_one_frame(&mut shell, &CHEAT_SHEET, &diff, &|_| None);
}

fn home_focus(shell: &mut Shell) -> gpui_kit::FocusHandle {
    use gpui_kit::Focusable as _;
    shell
        .main
        .read_with(shell.cx, |m, cx| match m.tabs().active_item() {
            polygloss_app::tabs::TabItem::Home(home) => home.read(cx).focus_handle(cx),
            polygloss_app::tabs::TabItem::Review(_) => panic!("Home is showing"),
        })
}

#[gpui_kit::test]
fn palette_opens_on_home(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let home = home_focus(&mut shell);
    opens_and_closes_on_one_frame(&mut shell, &PALETTE, &home, &palette_field);
}

#[gpui_kit::test]
fn overlays_trap_focus_and_close_on_outside_click(cx: &mut TestAppContext) {
    use gpui_kit::{Modifiers, point};
    use polygloss_diff::Side;
    use polygloss_viewport::CursorPos;

    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let diff = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    let cursor = CursorPos {
        file_idx: 0,
        side: Side::New,
        line: 4,
        range_start: None,
    };
    let window = shell.cx.update(|window, _| window.viewport_size());
    // Bottom right: diff rows, outside every overlay's frame.
    let outside = point(window.width - px(40.), window.height - px(40.));

    for overlay in [&PALETTE, &FINDER, &CHEAT_SHEET] {
        let name = overlay.name;
        viewport.update(shell.cx, |v, cx| v.set_cursor(Some(cursor), cx));
        shell.cx.update(|window, cx| window.focus(&diff, cx));
        draw(shell.cx);
        shell.cx.simulate_keystrokes(overlay.keys);
        draw(shell.cx);
        let frame = bounds(shell.cx, overlay.painted);
        assert!(
            !frame.contains(&outside),
            "{name}: the test's point is outside"
        );

        // ⇥ and ⇧⇥ cycle inside it: the diff never gets the keyboard, and
        // Esc still reaches the overlay.
        for keys in ["tab", "tab", "tab", "shift-tab"] {
            shell.cx.simulate_keystrokes(keys);
            draw(shell.cx);
            let escaped = shell
                .cx
                .update(|window, cx| diff.contains_focused(window, cx));
            assert!(!escaped, "{name}: {keys} kept the keyboard inside");
            assert!(painted(shell.cx, overlay.painted).is_some(), "{name}: open");
        }
        shell.cx.simulate_keystrokes("escape");
        draw(shell.cx);
        assert!(
            painted(shell.cx, overlay.painted).is_none(),
            "{name}: Esc closed it after ⇥"
        );

        // A click inside the frame keeps it open…
        shell.cx.update(|window, cx| window.focus(&diff, cx));
        shell.cx.simulate_keystrokes(overlay.keys);
        draw(shell.cx);
        let frame = bounds(shell.cx, overlay.painted);
        let inside = frame.origin + point(px(4.), px(4.));
        shell.cx.simulate_click(inside, Modifiers::none());
        draw(shell.cx);
        assert!(
            painted(shell.cx, overlay.painted).is_some(),
            "{name}: a click inside keeps it"
        );
        // …a click outside closes it, reaches nothing beneath (the diff's
        // cursor stays) and gives the keyboard back to the diff.
        shell.cx.simulate_click(outside, Modifiers::none());
        draw(shell.cx);
        assert!(
            painted(shell.cx, overlay.painted).is_none(),
            "{name}: a click outside closed it"
        );
        assert_eq!(
            viewport.read_with(shell.cx, |v, _| v.cursor()),
            Some(cursor),
            "{name}: the click reached nothing beneath"
        );
        let back = shell.cx.update(|window, _| diff.is_focused(window));
        assert!(back, "{name}: the diff has the keyboard again");
    }
}

/// In the smallest window an overlay keeps 16 pt of it clear on every side
/// (gpui-kit's dialog margin): a frame wider than the window narrows, and
/// one that would run off the bottom moves up.
#[gpui_kit::test]
fn overlays_stay_inside_a_short_window(cx: &mut TestAppContext) {
    use polygloss_app::space::layout;

    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let (width, height) = layout::WINDOW_MIN;
    crate::shell::resize_window(&mut shell, width, height);
    for overlay in [&PALETTE, &CHEAT_SHEET] {
        shell.cx.simulate_keystrokes(overlay.keys);
        draw(shell.cx);
        let frame = bounds(shell.cx, "overlay-frame");
        let name = overlay.name;
        assert!(frame.top() >= px(16.), "{name}: {frame:?}");
        assert!(frame.bottom() <= px(height - 16.), "{name}: {frame:?}");
        assert!(frame.left() >= px(16.), "{name}: {frame:?}");
        assert!(frame.right() <= px(width - 16.), "{name}: {frame:?}");
        // Centred.
        assert_eq!(frame.left(), px(width) - frame.right(), "{name}");
        shell.cx.simulate_keystrokes("escape");
        draw(shell.cx);
    }
    // The palette (96 pt down in a tall window) moved up to fit.
    shell.cx.simulate_keystrokes("cmd-k");
    draw(shell.cx);
    let frame = bounds(shell.cx, "overlay-frame");
    assert!(frame.top() < px(96.), "{frame:?}");
    assert_eq!(frame.bottom(), px(height - 16.), "{frame:?}");
    // The cheat sheet is wider than the window less its margins.
    shell.cx.simulate_keystrokes("escape");
    shell.cx.simulate_keystrokes("?");
    draw(shell.cx);
    assert_eq!(
        bounds(shell.cx, "overlay-frame").size.width,
        px(width - 2. * 16.)
    );
}
