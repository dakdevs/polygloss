//! Install CLI (design §21, ADR-0019, T5.4): `window::InstallCli`, in the
//! command palette and the app menu, links `/usr/local/bin/polygloss` to the
//! running bundle's `polygloss-cli` for DMG installs
//! ([`polygloss_platform::install::install_cli`], with an administrator
//! prompt when needed). It runs off the UI thread (the prompt waits for the
//! user) and ends in a toast; a cancelled prompt shows nothing. An unbundled
//! build (`cargo run`) has no bundled CLI and only explains that.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, AsyncApp, Global};
use polygloss_platform::install::{self, INSTALL_LINK, InstallError};

use crate::keymap::actions::window::InstallCli;

type Target = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;
type Install = Arc<dyn Fn(&Path) -> Result<(), InstallError> + Send + Sync>;

/// How Install CLI finds the CLI and links it (a GPUI global): the running
/// bundle and the real installer by default; tests install a recorder.
pub struct CliInstaller {
    /// The bundle's `polygloss-cli`, or `None` outside a bundle.
    pub target: Target,
    /// Links [`INSTALL_LINK`] to the target.
    pub install: Install,
}

impl CliInstaller {
    pub fn new(
        target: impl Fn() -> Option<PathBuf> + Send + Sync + 'static,
        install: impl Fn(&Path) -> Result<(), InstallError> + Send + Sync + 'static,
    ) -> CliInstaller {
        CliInstaller {
            target: Arc::new(target),
            install: Arc::new(install),
        }
    }

    /// The CLI next to this executable when it runs from `Polygloss.app`,
    /// and [`install::install_cli`].
    pub fn system() -> CliInstaller {
        CliInstaller::new(
            || {
                let exe = std::env::current_exe().ok()?;
                install::bundled_cli(&exe)
            },
            install::install_cli,
        )
    }
}

impl Global for CliInstaller {}

/// Replaces how the CLI is found and linked.
pub fn set_installer(installer: CliInstaller, cx: &mut App) {
    cx.set_global(installer);
}

/// Registers the action app-wide (the menu item works without a window).
/// Its App menu item is in `window::init`, next to Settings….
pub fn init(cx: &mut App) {
    if !cx.has_global::<CliInstaller>() {
        set_installer(CliInstaller::system(), cx);
    }
    cx.on_action(|_: &InstallCli, cx| run(cx));
}

/// Links the CLI and reports the outcome in a toast.
pub fn run(cx: &mut App) {
    let installer = cx.global::<CliInstaller>();
    let (target, install) = (installer.target.clone(), installer.install.clone());
    let Some(cli) = target() else {
        // Deferred: the action may be dispatched from inside the window.
        cx.defer(|cx| {
            toast(
                Err(
                    "Install CLI works from Polygloss.app; this build is not an app bundle.".into(),
                ),
                cx,
            )
        });
        return;
    };
    let task = cx.background_spawn(async move { install(&cli) });
    cx.spawn(async move |cx: &mut AsyncApp| {
        let message = match task.await {
            Ok(()) => Ok(format!(
                "Installed the polygloss command at {INSTALL_LINK}."
            )),
            Err(InstallError::Cancelled) => return,
            Err(err) => {
                tracing::warn!("install cli: {err}");
                Err(format!("Could not install the polygloss command: {err}"))
            }
        };
        cx.update(|cx| toast(message, cx));
    })
    .detach();
}

/// A success (`Ok`) or error toast in the main window, if it is open.
fn toast(message: Result<String, String>, cx: &mut App) {
    let Some((handle, main)) = crate::window::main_window(cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        main.update(cx, |main, cx| match message {
            Ok(text) => main.toast_success(text.into(), window, cx),
            Err(text) => main.toast_error(text.into(), window, cx),
        })
    });
}
