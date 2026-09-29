//! Actions, default bindings and `keymap.json` (T3.2, design §11.9, §18,
//! ADR-0025).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::{KeyContext, Keystroke, TestAppContext, VisualTestContext};
use polygloss_app::keymap::defaults::DEFAULT_BINDINGS;
use polygloss_app::keymap::{self, KeymapStore, actions};

use crate::shell::{compare_req, draw, start};
use crate::support::{Sandbox, code_change_repo};

/// Design §11.9, transcribed: `(keys, action, context)`. "Viewport, tree"
/// rows are one row per context; the provisional macOS additions follow.
const DESIGN_11_9: &[(&str, &str, &str)] = &[
    ("j", "viewport::CursorDown", "Viewport"),
    ("k", "viewport::CursorUp", "Viewport"),
    ("down", "viewport::CursorDown", "Viewport"),
    ("up", "viewport::CursorUp", "Viewport"),
    ("shift-down", "viewport::ExtendSelectionDown", "Viewport"),
    ("shift-up", "viewport::ExtendSelectionUp", "Viewport"),
    ("n", "viewport::NextFile", "Viewport"),
    ("p", "viewport::PrevFile", "Viewport"),
    ("n", "tree::NextFile", "Tree"),
    ("p", "tree::PrevFile", "Tree"),
    ("]", "viewport::NextChange", "Viewport"),
    ("[", "viewport::PrevChange", "Viewport"),
    ("v", "viewport::ToggleViewed", "Viewport"),
    ("v", "tree::ToggleViewed", "Tree"),
    ("c", "viewport::Comment", "Viewport"),
    ("cmd-enter", "composer::SaveDraft", "Composer"),
    (".", "viewport::NextOpenThread", "Viewport"),
    (",", "viewport::PrevOpenThread", "Viewport"),
    ("e", "viewport::ExpandContext", "Viewport"),
    ("E", "viewport::ExpandFile", "Viewport"),
    ("s", "viewport::ToggleLayout", "Viewport"),
    ("w", "viewport::ToggleWhitespace", "Viewport"),
    ("R", "tab::Refresh", "Tab"),
    ("o", "viewport::OpenInEditor", "Viewport"),
    ("cmd-p", "window::FileFinder", "Window"),
    ("cmd-k", "window::CommandPalette", "Window"),
    ("cmd-o", "window::OpenFlow", "Window"),
    ("cmd-f", "tab::Find", "Tab"),
    ("cmd-shift-enter", "tab::SubmitReview", "Tab"),
    ("?", "window::CheatSheet", "Window"),
    // Provisional macOS additions.
    ("escape", "composer::Cancel", "Composer"),
    ("cmd-c", "viewport::Copy", "Viewport"),
    ("cmd-w", "window::CloseTab", ""),
    ("cmd-}", "window::NextTab", ""),
    ("cmd-{", "window::PrevTab", ""),
    ("ctrl-tab", "window::NextTab", ""),
    ("ctrl-shift-tab", "window::PrevTab", ""),
    ("cmd-,", "window::OpenSettings", "Window"),
    ("cmd-q", "window::Quit", ""),
    ("cmd-m", "window::Minimize", ""),
];

/// The key context stack focus has in `context` (outermost first).
pub fn stack(context: &str) -> Vec<KeyContext> {
    let names: &[&str] = match context {
        "" | "Window" => &["Window"],
        "Tab" => &["Window", "Tab"],
        "Home" => &["Window", "Home"],
        "Viewport" => &["Window", "Tab", "Viewport"],
        "Tree" => &["Window", "Tab", "Tree"],
        "Composer" => &["Window", "Tab", "Viewport", "Composer"],
        "TreeFilter" => &["Window", "Tab", "Tree", "Input"],
        other => panic!("no stack for {other}"),
    };
    names
        .iter()
        .map(|n| KeyContext::parse(n).unwrap())
        .collect()
}

/// The action GPUI's keymap runs for `keys` with focus in `context`.
pub fn action_for(cx: &mut VisualTestContext, keys: &str, context: &str) -> Option<String> {
    let strokes: Vec<Keystroke> = keys
        .split_whitespace()
        .map(|k| Keystroke::parse(k).unwrap())
        .collect();
    let stack = stack(context);
    cx.update(|_, cx| {
        let keymap = cx.key_bindings();
        let keymap = keymap.borrow();
        let (bindings, _) = keymap.bindings_for_input(&strokes, &stack);
        bindings.first().map(|b| b.action().name().to_owned())
    })
}

/// Runs the app until `done` holds (watchers report from their thread), for
/// at most 10 s.
pub fn wait_until(
    cx: &mut VisualTestContext,
    mut done: impl FnMut(&mut VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(cx) {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(20));
        cx.executor().advance_clock(Duration::from_millis(50));
        draw(cx);
    }
}

/// Actions handled by [`record`], in order.
pub type Log = Rc<RefCell<Vec<String>>>;

/// Logs the name of action `A` whenever a key dispatches it, whether a
/// feature handles it (T3.8's viewport actions) or not: an app-wide
/// fallback handler takes it otherwise (GPUI tries the next binding when an
/// action is left unhandled), and a keystroke observer logs it once.
pub fn record<A: gpui_kit::Action>(cx: &mut VisualTestContext, log: &Log) {
    let log = log.clone();
    cx.update(|_, cx| {
        cx.on_action(|_: &A, _| {});
        cx.observe_keystrokes(move |event, _, _| {
            if let Some(action) = &event.action
                && action.name() == A::name_for_type()
            {
                log.borrow_mut().push(action.name().to_owned());
            }
        })
        .detach();
    });
}

fn keymap_file(sb: &Sandbox) -> std::path::PathBuf {
    let file = sb.config_dir().join("polygloss/keymap.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    file
}

#[gpui_kit::test]
fn default_bindings_match_design_11_9(cx: &mut TestAppContext) {
    // The table itself.
    let norm = |rows: &[(&str, &str, &str)]| {
        let mut rows: Vec<(String, String, String)> = rows
            .iter()
            .map(|(k, a, c)| {
                let keys = Keystroke::parse(k).unwrap().unparse();
                (keys, a.to_string(), c.to_string())
            })
            .collect();
        rows.sort();
        rows
    };
    assert_eq!(norm(DEFAULT_BINDINGS), norm(DESIGN_11_9));
    // Every action of the table is in the registry, and every registry
    // entry builds the action it names.
    for (_, action, _) in DEFAULT_BINDINGS {
        assert!(
            actions::find(action).is_some(),
            "{action} is not registered"
        );
    }
    for info in actions::ACTIONS {
        assert_eq!((info.build)().name(), info.name);
    }

    // Installed in GPUI's keymap: each key runs its action where §11.9 says.
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    for (keys, action, context) in DESIGN_11_9 {
        assert_eq!(
            action_for(shell.cx, keys, context).as_deref(),
            Some(*action),
            "{keys} in {context:?}"
        );
    }
    // Viewport keys do nothing on Home (no Viewport context).
    assert_eq!(action_for(shell.cx, "j", "Window"), None);

    // Live: focus lands in the review tab's viewport, so its keys resolve.
    shell.open(compare_req(repo.path())).unwrap();
    let seen = Log::default();
    record::<actions::viewport::CursorDown>(shell.cx, &seen);
    record::<actions::viewport::NextChange>(shell.cx, &seen);
    record::<actions::tab::Refresh>(shell.cx, &seen);
    shell.cx.simulate_keystrokes("j down ] R");
    draw(shell.cx);
    assert_eq!(
        *seen.borrow(),
        [
            "viewport::CursorDown",
            "viewport::CursorDown",
            "viewport::NextChange",
            "tab::Refresh"
        ]
    );
}

/// Home (T3.4) and the registry (T3.2) share `tab::AssignToSession`: one
/// declaration (GPUI panics on two), bound to `a` on Home, while Home's own
/// keys and the window-wide defaults resolve side by side.
#[gpui_kit::test]
fn home_keys_and_registry_share_assign_to_session(cx: &mut TestAppContext) {
    use std::any::TypeId;

    assert_eq!(
        TypeId::of::<polygloss_app::home::AssignToSession>(),
        TypeId::of::<actions::tab::AssignToSession>()
    );
    let info = actions::find("tab::AssignToSession").expect("registered");
    assert!((info.build)().partial_eq(&polygloss_app::home::AssignToSession));

    let _sb = Sandbox::isolate();
    let shell = start(cx);
    for (keys, action) in [
        ("a", "tab::AssignToSession"),
        ("j", "home::SelectNext"),
        ("k", "home::SelectPrev"),
        ("e", "home::Archive"),
        ("m", "home::ToggleMute"),
        ("?", "window::CheatSheet"),
        ("cmd-k", "window::CommandPalette"),
    ] {
        assert_eq!(
            action_for(shell.cx, keys, "Home").as_deref(),
            Some(action),
            "{keys} on Home"
        );
    }
}

#[gpui_kit::test]
fn keymap_override_rebinds_and_null_unbinds(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    let file = keymap_file(&sb);
    std::fs::write(
        &file,
        r#"[
          // Zed-like sections; comments and trailing commas are fine.
          { "context": "Viewport", "bindings": { "j": "viewport::NextFile", "k": null, "shift-v": null, } },
          { "bindings": { "cmd-shift-p": "window::CommandPalette" } },
        ]"#,
    )
    .unwrap();
    let mut shell = start(cx);
    assert!(
        shell
            .cx
            .update(|_, cx| KeymapStore::global(cx).last_error().is_none())
    );
    assert_eq!(
        action_for(shell.cx, "j", "Viewport").as_deref(),
        Some("viewport::NextFile")
    );
    assert_eq!(action_for(shell.cx, "k", "Viewport"), None, "unbound");
    assert_eq!(
        action_for(shell.cx, "up", "Viewport").as_deref(),
        Some("viewport::CursorUp"),
        "other keys of the action stay"
    );
    assert_eq!(
        action_for(shell.cx, "cmd-shift-p", "Tab").as_deref(),
        Some("window::CommandPalette")
    );
    // n in the tree was not touched.
    assert_eq!(
        action_for(shell.cx, "n", "Tree").as_deref(),
        Some("tree::NextFile")
    );
    // The hints follow: CursorDown is now ↓ only, NextFile is n (then j).
    let hints = shell.cx.update(|_, cx| {
        let r = KeymapStore::global(cx).resolved();
        let keys =
            |a: &str| -> Vec<String> { r.bindings_for(a).iter().map(|b| b.keys.clone()).collect() };
        (
            keys("viewport::CursorDown"),
            keys("viewport::NextFile"),
            keys("viewport::CursorUp"),
        )
    });
    assert_eq!(hints.0, ["down"]);
    assert_eq!(hints.1, ["n", "j"]);
    assert_eq!(hints.2, ["up"]);

    // Live keys.
    shell.open(compare_req(repo.path())).unwrap();
    let seen = Log::default();
    record::<actions::viewport::CursorDown>(shell.cx, &seen);
    record::<actions::viewport::CursorUp>(shell.cx, &seen);
    record::<actions::viewport::NextFile>(shell.cx, &seen);
    shell.cx.simulate_keystrokes("j k");
    draw(shell.cx);
    assert_eq!(*seen.borrow(), ["viewport::NextFile"]);

    // Hot reload: an empty keymap brings the defaults back.
    std::fs::write(&file, "[]").unwrap();
    wait_until(shell.cx, |cx| {
        action_for(cx, "j", "Viewport").as_deref() == Some("viewport::CursorDown")
    });
    assert_eq!(
        action_for(shell.cx, "k", "Viewport").as_deref(),
        Some("viewport::CursorUp")
    );
    assert_eq!(action_for(shell.cx, "cmd-shift-p", "Tab"), None);
    // gpui-kit's own bindings survived the reinstall (its inputs still edit).
    let input_bindings = shell.cx.update(|_, cx| {
        cx.key_bindings()
            .borrow()
            .bindings()
            .filter(|b| b.meta().is_none())
            .count()
    });
    assert!(input_bindings > 0);
}

#[gpui_kit::test]
fn keymap_invalid_file_keeps_previous(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let file = keymap_file(&sb);
    std::fs::write(
        &file,
        r#"[{ "context": "Viewport", "bindings": { "j": "viewport::NextFile" } }]"#,
    )
    .unwrap();
    let shell = start(cx);
    assert_eq!(
        action_for(shell.cx, "j", "Viewport").as_deref(),
        Some("viewport::NextFile")
    );

    for (bad, says) in [
        (
            r#"[{ "bindings": { "j": "viewport::NoSuchAction" } }]"#,
            "unknown action",
        ),
        (
            r#"[{ "bindings": { "ctrl-x-y": "viewport::Comment" } }]"#,
            "keystroke",
        ),
        (
            r#"[{ "context": "Viewport &&", "bindings": { "x": "viewport::Comment" } }]"#,
            "context",
        ),
        (r#"[{ "bindings": { "j": "#, "EOF"),
    ] {
        std::fs::write(&file, bad).unwrap();
        shell.cx.update(|_, cx| KeymapStore::reload(cx));
        draw(shell.cx);
        assert_eq!(
            action_for(shell.cx, "j", "Viewport").as_deref(),
            Some("viewport::NextFile"),
            "the previous bindings stay after {bad}"
        );
        let error = shell
            .cx
            .update(|_, cx| KeymapStore::global(cx).last_error().map(|e| e.to_string()))
            .expect("the error is kept");
        assert!(error.contains("keymap.json"), "{error}");
        assert!(error.contains(says), "{bad}: {error}");
    }
    // Each error was shown in the window.
    let toasts = shell.main.read_with(shell.cx, |m, _| m.toasts().to_vec());
    let shown = toasts
        .iter()
        .filter(|t| t.contains("Invalid key bindings") && t.contains("keymap.json"))
        .count();
    assert_eq!(shown, 4, "{toasts:?}");

    // Fixed: applied, error cleared.
    std::fs::write(&file, "[]").unwrap();
    shell.cx.update(|_, cx| KeymapStore::reload(cx));
    assert_eq!(
        action_for(shell.cx, "j", "Viewport").as_deref(),
        Some("viewport::CursorDown")
    );
    assert!(
        shell
            .cx
            .update(|_, cx| KeymapStore::global(cx).last_error().is_none())
    );
}

#[gpui_kit::test]
fn keymap_invalid_at_launch_uses_defaults_and_toasts(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    std::fs::write(keymap_file(&sb), "{ not json").unwrap();
    let shell = start(cx);
    assert_eq!(
        action_for(shell.cx, "j", "Viewport").as_deref(),
        Some("viewport::CursorDown")
    );
    let toasts = shell.main.read_with(shell.cx, |m, _| m.toasts().to_vec());
    assert!(
        toasts.iter().any(|t| t.contains("keymap.json")),
        "{toasts:?}"
    );
}

#[gpui_kit::test]
fn single_letter_keys_inactive_in_composer_context(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    // A user's own single-letter binding follows the same rule.
    std::fs::write(
        keymap_file(&sb),
        r#"[{ "context": "Viewport", "bindings": { "x": "viewport::Comment" } }]"#,
    )
    .unwrap();
    let shell = start(cx);
    for keys in ["j", "k", "c", "s", "w", "E", "R", "?", "x", ".", "]"] {
        assert_eq!(
            action_for(shell.cx, keys, "Composer"),
            None,
            "{keys} while composing"
        );
    }
    // …while the composer's own keys work there, and the letters still work
    // in the viewport around it.
    assert_eq!(
        action_for(shell.cx, "cmd-enter", "Composer").as_deref(),
        Some("composer::SaveDraft")
    );
    assert_eq!(
        action_for(shell.cx, "escape", "Composer").as_deref(),
        Some("composer::Cancel")
    );
    assert_eq!(
        action_for(shell.cx, "x", "Viewport").as_deref(),
        Some("viewport::Comment")
    );
    assert_eq!(
        action_for(shell.cx, "cmd-k", "Composer").as_deref(),
        Some("window::CommandPalette"),
        "modified keys still work while composing"
    );
    // Any text field counts: the tree's filter box (a gpui-kit input).
    for keys in ["n", "p", "v", "?"] {
        assert_eq!(
            action_for(shell.cx, keys, "TreeFilter"),
            None,
            "{keys} in a text field"
        );
    }
    assert!(keymap::is_text_key("shift-r") && keymap::is_text_key("?"));
    assert!(!keymap::is_text_key("cmd-k") && !keymap::is_text_key("down"));
}

#[test]
fn keymap_file_parses_zed_shape_and_rejects_unknown_actions() {
    use polygloss_app::keymap::file::parse;
    let parsed = parse(
        r#"[
          { "context": "Viewport", "bindings": { "j": "viewport::CursorDown", "shift-v": null } },
          { "bindings": { "cmd-k": "window::CommandPalette" } }
        ]"#,
    )
    .unwrap();
    let got: Vec<(&str, Option<&str>, Option<&str>)> = parsed
        .iter()
        .map(|b| (b.keys.as_str(), b.action, b.context.as_deref()))
        .collect();
    assert_eq!(
        got,
        [
            ("j", Some("viewport::CursorDown"), Some("Viewport")),
            ("shift-v", None, Some("Viewport")),
            ("cmd-k", Some("window::CommandPalette"), None),
        ]
    );
    assert!(parse("").unwrap().is_empty());
    assert!(parse(r#"[{ "bindings": { "j": "nope::Nope" } }]"#).is_err());
    assert!(parse(r#"{ "bindings": {} }"#).is_err(), "not an array");
    // `E` and `shift-e` are the same keys: a user binding replaces the default.
    let resolved = keymap::resolve(
        &parse(r#"[{ "context": "Viewport", "bindings": { "E": "viewport::Comment" } }]"#).unwrap(),
    );
    assert_eq!(resolved.bindings_for("viewport::ExpandFile").len(), 0);
    assert_eq!(
        resolved
            .bindings_for("viewport::Comment")
            .iter()
            .map(|b| b.keys.as_str())
            .collect::<Vec<_>>(),
        ["c", "E"]
    );
}
