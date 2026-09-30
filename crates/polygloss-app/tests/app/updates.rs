//! GPUI tests of T5.3: the Sparkle updater in the app (design §18, §21,
//! OQ-16). "Check for Updates…" is in the Polygloss menu and the command
//! palette only when there is an updater, which a test binary (not an app
//! bundle) never has; with one, the action asks it to check, and a set
//! `updates.automatic_checks` overrides the answer to Sparkle's prompt.
//!
//! The updater is an injected [`UpdateBackend`] that records calls: no
//! Sparkle, no network.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::{OwnedMenuItem, TestAppContext};
use polygloss_app::ipc::debug_state;
use polygloss_app::keymap::actions::window::CheckForUpdates;
use polygloss_app::palette::command;
use polygloss_app::settings::{Settings, SettingsStore};
use polygloss_app::updates::{self, UpdateBackend};

use crate::shell::{Shell, draw, start};
use crate::support::Sandbox;

/// A fake updater: records what the app asked of it.
#[derive(Default)]
struct Recorder {
    checks: RefCell<u32>,
    automatic: RefCell<Vec<bool>>,
}

impl UpdateBackend for Recorder {
    fn check_for_updates(&self) {
        *self.checks.borrow_mut() += 1;
    }

    fn set_automatic_checks(&self, on: bool) {
        self.automatic.borrow_mut().push(on);
    }

    fn state(&self) -> &'static str {
        "started"
    }
}

/// Starts the app with `recorder` as its updater.
fn start_with<'a>(recorder: &Rc<Recorder>, cx: &'a mut TestAppContext) -> Shell<'a> {
    let backend: Rc<dyn UpdateBackend> = recorder.clone();
    cx.update(|cx| updates::set_backend(Some(backend), cx));
    start(cx)
}

/// The Polygloss menu as `name → action` lines (`—` for separators).
fn app_menu(shell: &mut Shell<'_>) -> Vec<String> {
    shell.cx.update(|_, cx| {
        let menus = cx.get_menus().expect("a menu bar");
        menus[0]
            .items
            .iter()
            .map(|item| match item {
                OwnedMenuItem::Action { name, action, .. } => {
                    format!("{name} → {}", action.name())
                }
                OwnedMenuItem::Separator => "—".to_owned(),
                OwnedMenuItem::SystemMenu(m) => m.name.to_string(),
                OwnedMenuItem::Submenu(m) => m.name.to_string(),
            })
            .collect()
    })
}

/// Whether the command palette lists `window::CheckForUpdates`.
fn palette_lists_check(shell: &mut Shell<'_>) -> bool {
    shell.cx.update(|_, cx| {
        command::groups(cx)
            .iter()
            .flat_map(|(_, rows)| rows)
            .any(|row| row.action == "window::CheckForUpdates")
    })
}

fn with_automatic_checks(value: Option<bool>) -> Settings {
    let mut settings = Settings::default();
    settings.updates.automatic_checks = value;
    settings
}

#[gpui_kit::test]
fn menu_item_hidden_without_updater(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    // A test binary is not an app bundle: Sparkle never starts.
    assert!(!shell.cx.update(|_, cx| updates::available(cx)));
    assert_eq!(shell.cx.update(|_, cx| updates::state(cx)), None);
    let state = shell.cx.update(|_, cx| debug_state::snapshot(cx));
    assert_eq!(state["updater"], serde_json::Value::Null);
    let menu = app_menu(&mut shell);
    assert!(
        menu.iter().all(|item| !item.contains("Check for Updates")),
        "{menu:?}"
    );
    assert!(!palette_lists_check(&mut shell));
    // The action itself (e.g. bound in keymap.json) does nothing.
    shell.cx.dispatch_action(CheckForUpdates);
    draw(shell.cx);
}

#[gpui_kit::test]
fn menu_item_checks_for_updates_with_updater(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let recorder = Rc::new(Recorder::default());
    let mut shell = start_with(&recorder, cx);
    assert!(shell.cx.update(|_, cx| updates::available(cx)));
    assert_eq!(shell.cx.update(|_, cx| updates::state(cx)), Some("started"));
    let state = shell.cx.update(|_, cx| debug_state::snapshot(cx));
    assert_eq!(state["updater"], "started");
    // macOS order: Check for Updates… comes before Settings….
    assert_eq!(
        app_menu(&mut shell),
        [
            "Check for Updates… → window::CheckForUpdates",
            "—",
            "Settings… → window::OpenSettings",
            "—",
            "Services",
            "—",
            "Quit Polygloss → window::Quit"
        ]
    );
    assert!(palette_lists_check(&mut shell));

    shell.cx.dispatch_action(CheckForUpdates);
    draw(shell.cx);
    assert_eq!(*recorder.checks.borrow(), 1);
}

#[gpui_kit::test]
fn automatic_checks_follow_sparkle_until_set(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let recorder = Rc::new(Recorder::default());
    let shell = start_with(&recorder, cx);
    // `null` (the default): Sparkle's own first-launch prompt decides.
    assert_eq!(*recorder.automatic.borrow(), Vec::<bool>::new());

    shell
        .cx
        .update(|_, cx| SettingsStore::set(with_automatic_checks(Some(false)), cx));
    draw(shell.cx);
    assert_eq!(*recorder.automatic.borrow(), [false]);

    // Another setting changing leaves the updater alone.
    shell.cx.update(|_, cx| {
        let mut settings = with_automatic_checks(Some(false));
        settings.agent_notes.hidden = true;
        SettingsStore::set(settings, cx)
    });
    draw(shell.cx);
    assert_eq!(*recorder.automatic.borrow(), [false]);

    shell
        .cx
        .update(|_, cx| SettingsStore::set(with_automatic_checks(Some(true)), cx));
    draw(shell.cx);
    assert_eq!(*recorder.automatic.borrow(), [false, true]);

    // Back to `null`: Sparkle keeps what it has; nothing is forced.
    shell
        .cx
        .update(|_, cx| SettingsStore::set(with_automatic_checks(None), cx));
    draw(shell.cx);
    assert_eq!(*recorder.automatic.borrow(), [false, true]);
}

#[gpui_kit::test]
fn automatic_checks_setting_applies_at_launch(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let paths = polygloss_core::paths::DataPaths::resolve().unwrap();
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(
        paths.config_dir.join("settings.json"),
        r#"{ "updates": { "automatic_checks": false } }"#,
    )
    .unwrap();
    let recorder = Rc::new(Recorder::default());
    let _shell = start_with(&recorder, cx);
    assert_eq!(*recorder.automatic.borrow(), [false]);
}
