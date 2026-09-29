//! The M2 gate shell: one window with a [`DiffViewport`] over a real diff, to
//! look at the viewport on real repos before M3's review UX exists (plan
//! T2.8). T3.1 deletes this module.
//!
//! ```text
//! Polygloss --gate --repo <path> (--compare <base> <head> [--direct] | --commit <rev> | --live)
//!                  [--layout auto|split|unified] [--theme light|dark]
//! ```
//!
//! The diff is opened through [`Core::open`] (so it is recorded in the store
//! like any open) before any window exists: a bad repo or revision prints
//! `Polygloss --gate: <error>` and exits 1, bad arguments print the usage and
//! exit 2. The viewport uses [`ViewportOptions::default`] with the layout and
//! the Pierre theme pinned by the flags (default auto and Pierre Light).

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use gpui_kit::{
    App, AppContext as _, Context, Entity, IntoElement, KeyBinding, Menu, MenuItem, ParentElement,
    Render, Styled, TitlebarOptions, Window, WindowBounds, WindowHandle, WindowOptions, div, px,
    size,
};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::review::{Core, CoreError, OpenRequest, OpenedDiff};
use polygloss_core::store::events::Actor;
use polygloss_highlight::Appearance;
use polygloss_viewport::{DiffProvider, DiffViewport, LayoutMode, ViewportOptions, ViewportTheme};

use crate::provider::CoreDiffProvider;

/// Printed (to stderr) with every argument error.
pub const USAGE: &str = "usage: Polygloss --gate --repo <path> \
    (--compare <base> <head> [--direct] | --commit <rev> | --live) \
    [--layout auto|split|unified] [--theme light|dark]";

/// The gate window's initial size in points.
const WINDOW_SIZE: (f32, f32) = (1440.0, 900.0);

gpui_kit::actions!(polygloss_gate, [Quit]);

/// `Polygloss --gate` arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateArgs {
    /// Any path inside the repo (the worktree, for `--live`).
    pub repo: PathBuf,
    /// `--compare` (three-dot unless `--direct`), `--commit` or `--live`
    /// (since the merge base, the default live base).
    pub source: Source,
    pub layout: LayoutMode,
    /// The Pierre theme to pin.
    pub appearance: Appearance,
}

impl GateArgs {
    /// Parses the arguments after `--gate`.
    pub fn parse(args: &[String]) -> Result<GateArgs, String> {
        let mut repo = None;
        let mut source = None;
        let mut direct = false;
        let mut layout = LayoutMode::Auto;
        let mut appearance = Appearance::Light;
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |what: &str| {
                args.next()
                    .cloned()
                    .ok_or_else(|| format!("{arg} needs {what}"))
            };
            match arg.as_str() {
                "--repo" => {
                    if repo.replace(PathBuf::from(value("a path")?)).is_some() {
                        return Err("--repo given twice".to_owned());
                    }
                }
                "--compare" => {
                    let base = value("<base> <head>")?;
                    let head = value("<base> <head>")?;
                    set_source(
                        &mut source,
                        Source::Compare {
                            base,
                            head,
                            mode: CompareMode::ThreeDot,
                        },
                    )?;
                }
                "--direct" => direct = true,
                "--commit" => {
                    let rev = value("a revision")?;
                    set_source(&mut source, Source::Commit { rev })?;
                }
                "--live" => set_source(
                    &mut source,
                    Source::Live {
                        since: Since::MergeBase,
                    },
                )?,
                "--layout" => {
                    layout = match value("auto, split or unified")?.as_str() {
                        "auto" => LayoutMode::Auto,
                        "split" => LayoutMode::Split,
                        "unified" => LayoutMode::Unified,
                        other => return Err(format!("unknown layout {other:?}")),
                    }
                }
                "--theme" => {
                    appearance = match value("light or dark")?.as_str() {
                        "light" => Appearance::Light,
                        "dark" => Appearance::Dark,
                        other => return Err(format!("unknown theme {other:?}")),
                    }
                }
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        let repo = repo.ok_or("--repo <path> is required")?;
        let mut source = source.ok_or("give one of --compare, --commit or --live")?;
        if direct {
            match &mut source {
                Source::Compare { mode, .. } => *mode = CompareMode::Direct,
                _ => return Err("--direct only applies to --compare".to_owned()),
            }
        }
        Ok(GateArgs {
            repo,
            source,
            layout,
            appearance,
        })
    }
}

/// Sets the diff source; a second one is an error.
fn set_source(slot: &mut Option<Source>, source: Source) -> Result<(), String> {
    if slot.replace(source).is_some() {
        return Err("give only one of --compare, --commit or --live".to_owned());
    }
    Ok(())
}

/// [`ViewportOptions::default`] with the layout and Pierre theme of `args`.
pub fn viewport_options(args: &GateArgs) -> ViewportOptions {
    ViewportOptions {
        layout: args.layout,
        theme: Arc::new(ViewportTheme::pierre(args.appearance)),
        ..ViewportOptions::default()
    }
}

/// Opens the diff `args` name through `core` (recorded in the store like
/// any human open; live diffs stay unpinned).
pub fn open_diff(core: &Core, args: &GateArgs) -> Result<OpenedDiff, CoreError> {
    core.open(&OpenRequest {
        worktree: args.repo.clone(),
        source: args.source.clone(),
        label: None,
        pin: None,
        actor: Actor::human(),
    })
}

/// The gate window's root view: the viewport, filling the window.
pub struct GateShell {
    viewport: Entity<DiffViewport>,
    opened: OpenedDiff,
}

impl GateShell {
    pub fn viewport(&self) -> &Entity<DiffViewport> {
        &self.viewport
    }

    pub fn opened(&self) -> &OpenedDiff {
        &self.opened
    }
}

impl Render for GateShell {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div().flex().size_full().child(self.viewport.clone())
    }
}

/// Opens the gate window over `opened` (blobs through
/// [`CoreDiffProvider::open`]).
pub fn open_window(
    opened: OpenedDiff,
    args: &GateArgs,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<GateShell>> {
    let provider = Arc::new(CoreDiffProvider::open(&opened)?);
    open_window_with(provider, opened, args, cx)
}

fn open_window_with(
    provider: Arc<dyn DiffProvider>,
    opened: OpenedDiff,
    args: &GateArgs,
    cx: &mut App,
) -> anyhow::Result<WindowHandle<GateShell>> {
    let opts = viewport_options(args);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(
            size(px(WINDOW_SIZE.0), px(WINDOW_SIZE.1)),
            cx,
        )),
        titlebar: Some(TitlebarOptions {
            title: Some(format!("Polygloss gate · {}", opened.review_key).into()),
            ..TitlebarOptions::default()
        }),
        focus: true,
        show: true,
        ..WindowOptions::default()
    };
    cx.open_window(options, |window, cx| {
        let viewport = cx.new(|cx| DiffViewport::new(provider, opts, window, cx));
        cx.new(|_| GateShell { viewport, opened })
    })
}

/// `Polygloss --gate …`: opens the diff, then runs the app with the gate
/// window until it is closed or the app quits (⌘Q).
pub fn run(args: &[String]) -> ExitCode {
    let args = match GateArgs::parse(args) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("Polygloss --gate: {err}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let opened = match Core::open_default().and_then(|core| open_diff(&core, &args)) {
        Ok(opened) => opened,
        Err(err) => {
            eprintln!("Polygloss --gate: {err}");
            return ExitCode::from(1);
        }
    };
    for warning in &opened.warnings {
        eprintln!("Polygloss --gate: note: {warning}");
    }
    let provider: Arc<dyn DiffProvider> = match CoreDiffProvider::open(&opened) {
        Ok(provider) => Arc::new(provider),
        Err(err) => {
            eprintln!("Polygloss --gate: {err}");
            return ExitCode::from(1);
        }
    };

    gpui_kit::application().run(move |cx| {
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus([Menu {
            name: "Polygloss".into(),
            items: vec![MenuItem::action("Quit Polygloss", Quit)],
            disabled: false,
        }]);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        match open_window_with(provider, opened, &args, cx) {
            Ok(_) => cx.activate(true),
            Err(err) => {
                eprintln!("Polygloss --gate: could not open a window: {err}");
                cx.quit();
            }
        }
    });
    ExitCode::SUCCESS
}
