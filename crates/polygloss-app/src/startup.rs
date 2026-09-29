//! Process startup: arguments, logging, the store, then the GPUI app with
//! its main window (design §11.1, §13.4).
//!
//! ```text
//! Polygloss [--repo <path> (--compare <base> <head> [--direct] | --commit <rev> | --live)]
//! Polygloss --version
//! POLYGLOSS_TEST=1 Polygloss --perf-scenario …      (test-only, see `perf`)
//! ```
//!
//! Startup follows the M2 gate (plan T2.10): a review named on the command
//! line starts opening on the background executor as soon as the app has
//! launched, while gpui-kit initializes (with named fonts, never its
//! ≈ 440 ms font scan) and the window opens; its tab appears when the open
//! is done. M4 adds the socket and `polygloss://` URLs as further sources
//! of open requests.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use gpui_kit::{AnyWindowHandle, App, Entity, Task, TaskExt as _};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::paths::DataPaths;
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;

use crate::app_state::AppState;
use crate::perf::Clock;
use crate::review_tab::{self, ReviewTab};
use crate::settings::SettingsStore;
use crate::window;

/// Printed (to stderr) with every argument error.
pub const USAGE: &str = "usage: Polygloss [--repo <path> \
    (--compare <base> <head> [--direct] | --commit <rev> | --live)]\n       Polygloss --version";

/// What `Polygloss` was asked to do at launch.
#[derive(Debug, Clone, Default)]
pub struct LaunchArgs {
    /// A review to open (`--repo` with a source).
    pub open: Option<OpenRequest>,
}

impl LaunchArgs {
    pub fn parse(args: &[String]) -> Result<LaunchArgs, String> {
        let mut repo = None;
        let mut source = None;
        let mut direct = false;
        let set = |slot: &mut Option<Source>, s: Source| {
            if slot.replace(s).is_some() {
                return Err("give only one of --compare, --commit or --live".to_owned());
            }
            Ok(())
        };
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
                    set(
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
                    set(&mut source, Source::Commit { rev })?;
                }
                "--live" => set(
                    &mut source,
                    Source::Live {
                        since: Since::MergeBase,
                    },
                )?,
                other => return Err(format!("unknown argument {other:?}")),
            }
        }
        let (repo, mut source) = match (repo, source) {
            (None, None) if !direct => return Ok(LaunchArgs::default()),
            (Some(repo), Some(source)) => (repo, source),
            (None, _) => return Err("--repo <path> is required with a source".to_owned()),
            (Some(_), None) => return Err("give one of --compare, --commit or --live".to_owned()),
        };
        if direct {
            match &mut source {
                Source::Compare { mode, .. } => *mode = CompareMode::Direct,
                _ => return Err("--direct only applies to --compare".to_owned()),
            }
        }
        Ok(LaunchArgs {
            open: Some(OpenRequest {
                worktree: repo,
                source,
                label: None,
                pin: None,
                actor: Actor::human(),
            }),
        })
    }
}

/// When startup reached its milestones.
#[derive(Debug, Clone, Copy)]
pub struct StartupMarks {
    pub app_launched: Instant,
    pub kit_initialized: Instant,
    pub window_opened: Instant,
}

/// What [`run`] hands a launch hook once the window is open.
pub struct Launched {
    pub window: AnyWindowHandle,
    /// The open of [`Launch::open`], until its tab shows (or it failed).
    pub opening: Option<Task<anyhow::Result<Entity<ReviewTab>>>>,
    pub marks: StartupMarks,
}

/// Called with the app before its window opens.
pub type AppHook = Box<dyn FnOnce(&mut App)>;

/// Called once the app has launched and the window is open (the perf
/// scenarios measure from here).
pub type LaunchHook = Box<dyn FnOnce(Launched, &mut App)>;

/// One launch of the app.
pub struct Launch {
    pub open: Option<OpenRequest>,
    pub clock: Clock,
    /// Runs before the window opens (after [`init`]).
    pub before_window: Option<AppHook>,
    pub after_launch: Option<LaunchHook>,
}

/// `Polygloss`'s `main`.
pub fn main() -> ExitCode {
    let clock = Clock::now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Polygloss {}", polygloss_core::VERSION);
        return ExitCode::SUCCESS;
    }
    if let Some((first, rest)) = args.split_first()
        && first == "--perf-scenario"
    {
        return crate::perf::main(rest, clock);
    }
    let args = match LaunchArgs::parse(&args) {
        Ok(args) => args,
        Err(err) => {
            eprintln!("Polygloss: {err}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    run(Launch {
        open: args.open,
        clock,
        before_window: None,
        after_launch: None,
    })
}

/// Everything the app sets up before its first window: [`AppState`], the
/// settings, gpui-kit (through `polygloss_viewport::kit::init_kit`, never
/// `gpui_kit::init`, T2.10.2), every feature and the menu bar.
pub fn init(core: Core, cx: &mut App) {
    AppState::set(core, cx);
    crate::settings::init(cx);
    let code_font = SettingsStore::global(cx)
        .settings()
        .buffer_font
        .family
        .clone();
    polygloss_viewport::kit::init_kit(&code_font, cx);
    crate::features::init(cx);
    window::install_menus(cx);
}

/// Runs the app until it quits.
pub fn run(launch: Launch) -> ExitCode {
    let paths = match DataPaths::resolve() {
        Ok(paths) => paths,
        Err(err) => {
            eprintln!("Polygloss: {err}");
            return ExitCode::from(1);
        }
    };
    let log_guard = crate::logging::init(&paths.logs_dir);
    let core = match Core::with_paths(paths) {
        Ok(core) => core,
        Err(err) => {
            tracing::error!("opening the store: {err}");
            eprintln!("Polygloss: {err}");
            return ExitCode::from(1);
        }
    };
    tracing::info!("Polygloss {} starting", polygloss_core::VERSION);
    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    app.on_reopen(window::reopen);
    app.run(move |cx: &mut App| {
        let app_launched = Instant::now();
        let mut log_guard = log_guard;
        cx.on_app_quit(move |_| {
            // Flushes the log.
            log_guard.take();
            async {}
        })
        .detach();
        // The open runs while the window comes up.
        AppState::set(core.clone(), cx);
        let opening = launch.open.map(|req| review_tab::start_open(req, cx));
        init(core, cx);
        let kit_initialized = Instant::now();
        if let Some(before) = launch.before_window {
            before(cx);
        }
        let window = match window::open_main_window(cx) {
            Ok((window, _)) => window,
            Err(err) => {
                tracing::error!("opening the window: {err:#}");
                eprintln!("Polygloss: could not open a window: {err:#}");
                cx.quit();
                return;
            }
        };
        let window_opened = Instant::now();
        cx.activate(true);
        let opening = opening.and_then(|opening| {
            window
                .update(cx, |_, window, cx| {
                    review_tab::finish_open(opening, window, cx)
                })
                .ok()
        });
        let launched = Launched {
            window,
            opening,
            marks: StartupMarks {
                app_launched,
                kit_initialized,
                window_opened,
            },
        };
        match launch.after_launch {
            Some(hook) => hook(launched, cx),
            None => {
                if let Some(opening) = launched.opening {
                    opening.detach_and_log_err(cx);
                }
            }
        }
    });
    ExitCode::SUCCESS
}
