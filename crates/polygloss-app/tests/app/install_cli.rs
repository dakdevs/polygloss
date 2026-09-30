//! GPUI tests of Install CLI (T5.4, design §21): the palette command and the
//! app menu item run [`install_cli::CliInstaller`]. Every test installs a
//! recording installer; nothing is linked outside the sandbox and no
//! administrator prompt appears.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui_kit::{OwnedMenuItem, TestAppContext};
use polygloss_app::install_cli::{self, CliInstaller};
use polygloss_app::keymap::actions;
use polygloss_platform::install::{INSTALL_LINK, InstallError};

use crate::keymap::wait_until;
use crate::shell::{Shell, draw, start};
use crate::support::Sandbox;

type Calls = Arc<Mutex<Vec<PathBuf>>>;

/// Installs a fake: the bundle's CLI is `target`, and each install records
/// its target and answers with `answer()`.
fn fake(
    shell: &mut Shell,
    target: Option<PathBuf>,
    answer: impl Fn() -> Result<(), InstallError> + Send + Sync + 'static,
) -> Calls {
    let calls: Calls = Arc::default();
    let seen = calls.clone();
    shell.cx.update(|_, cx| {
        install_cli::set_installer(
            CliInstaller::new(
                move || target.clone(),
                move |path: &Path| {
                    seen.lock().unwrap().push(path.to_path_buf());
                    answer()
                },
            ),
            cx,
        );
    });
    calls
}

fn toasts(shell: &mut Shell) -> Vec<String> {
    shell.main.read_with(shell.cx, |m, _| {
        m.toasts().iter().map(|t| t.to_string()).collect()
    })
}

/// ⌘K, "install cli", ⏎.
fn run_from_palette(shell: &mut Shell) {
    shell.cx.simulate_keystrokes("cmd-k");
    draw(shell.cx);
    shell.cx.simulate_input("install cli");
    draw(shell.cx);
    shell.cx.simulate_keystrokes("enter");
    draw(shell.cx);
}

#[gpui_kit::test]
fn install_cli_is_a_palette_action_and_an_app_menu_item(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    let info = actions::find("window::InstallCli").expect("registered");
    assert_eq!(info.title, "Install CLI");
    let items: Vec<String> = shell.cx.update(|_, cx| {
        cx.get_menus().expect("a menu bar")[0]
            .items
            .iter()
            .filter_map(|item| match item {
                OwnedMenuItem::Action { name, action, .. } => {
                    Some(format!("{name} → {}", action.name()))
                }
                _ => None,
            })
            .collect()
    });
    assert!(
        items.contains(&"Install CLI… → window::InstallCli".to_owned()),
        "{items:?}"
    );
}

#[gpui_kit::test]
fn install_cli_links_the_bundled_cli_and_confirms(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let target = PathBuf::from("/Applications/Polygloss.app/Contents/MacOS/polygloss-cli");
    let calls = fake(&mut shell, Some(target.clone()), || Ok(()));
    run_from_palette(&mut shell);
    wait_until(shell.cx, |cx| !shell_toasts(cx, &shell.main).is_empty());
    assert_eq!(*calls.lock().unwrap(), [target]);
    let toasts = toasts(&mut shell);
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(toasts[0].contains(INSTALL_LINK), "{toasts:?}");
}

#[gpui_kit::test]
fn install_cli_in_an_unbundled_build_explains_and_links_nothing(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let calls = fake(&mut shell, None, || panic!("nothing to install"));
    shell
        .cx
        .update(|window, cx| window.dispatch_action(Box::new(actions::window::InstallCli), cx));
    draw(shell.cx);
    assert!(calls.lock().unwrap().is_empty());
    let toasts = toasts(&mut shell);
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(toasts[0].contains("Polygloss.app"), "{toasts:?}");
}

#[gpui_kit::test]
fn install_cli_failures_toast_and_cancel_is_quiet(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let target = PathBuf::from("/Applications/Polygloss.app/Contents/MacOS/polygloss-cli");
    fake(&mut shell, Some(target.clone()), || {
        Err(InstallError::Occupied(PathBuf::from(INSTALL_LINK)))
    });
    run_from_palette(&mut shell);
    wait_until(shell.cx, |cx| !shell_toasts(cx, &shell.main).is_empty());
    let toasts = toasts(&mut shell);
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(
        toasts[0].contains("Could not install") && toasts[0].contains("not a link"),
        "{toasts:?}"
    );

    // Cancelling the password prompt shows nothing new.
    let calls = fake(&mut shell, Some(target), || Err(InstallError::Cancelled));
    shell
        .cx
        .update(|window, cx| window.dispatch_action(Box::new(actions::window::InstallCli), cx));
    wait_until(shell.cx, |_| !calls.lock().unwrap().is_empty());
    draw(shell.cx);
    assert_eq!(self::toasts(&mut shell).len(), 1);
}

fn shell_toasts(
    cx: &mut gpui_kit::VisualTestContext,
    main: &gpui_kit::Entity<polygloss_app::window::MainWindow>,
) -> Vec<String> {
    main.read_with(cx, |m, _| {
        m.toasts().iter().map(|t| t.to_string()).collect()
    })
}
