//! Open in editor (design §11.13, ADR-0021) and `⌘,` (open `settings.json`,
//! §11.9 provisional).
//!
//! `o` (`viewport::OpenInEditor`) opens the cursor's line (the first line of
//! a range; without a cursor, the file at the top at its first line); the
//! file header's ⋯ › "Open in editor" (`ViewportEvent::OpenInEditor`) opens
//! that file. [`target::resolve`] picks the file on disk at the line-mapped
//! position or a read-only blob copy. The editor is `editor.command` when
//! set, else the detected one (`polygloss_platform::editor::detect`: Zed,
//! Cursor, VS Code, `$VISUAL`, `$EDITOR`), else macOS's default text editor
//! (`open -t`, which cannot go to a line). It is started as an argv, never
//! through a shell; terminal editors run through a `.command` file (OQ-21).
//! Everything but reading the settings runs off the UI thread; failures
//! show an error toast. Opening an editor is human-only (MCP `focus` only
//! scrolls Polygloss).
//!
//! Owned by T3.16. T3.1 already calls [`init`] (from `features::init`) and
//! [`attach`] (for every new review tab).

pub mod target;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::{App, AppContext as _, AsyncApp, Context, Global, Subscription, Window};
use polygloss_core::review::OpenedDiff;
use polygloss_diff::Side;
use polygloss_platform::editor::{self as platform, EditorCommand, ProcessSpawner, Spawner};
use polygloss_viewport::ViewportEvent;

use crate::app_state::AppState;
use crate::keymap::actions::{viewport, window as window_actions};
use crate::keymap::handlers;
use crate::review_tab::ReviewTab;
use crate::settings::{Settings, SettingsStore};
use crate::window::MainWindow;

/// How editors are found and started (a GPUI global): the real process
/// spawner and detection by default; tests install a recording spawner.
pub struct EditorHost {
    pub spawner: Arc<dyn Spawner>,
    /// Auto-detection, used when `editor.command` is not set.
    pub detect: Arc<dyn Fn() -> Option<EditorCommand> + Send + Sync>,
}

impl EditorHost {
    pub fn new(
        spawner: Arc<dyn Spawner>,
        detect: impl Fn() -> Option<EditorCommand> + Send + Sync + 'static,
    ) -> EditorHost {
        EditorHost {
            spawner,
            detect: Arc::new(detect),
        }
    }

    /// Real processes and [`platform::detect`].
    pub fn system() -> EditorHost {
        EditorHost::new(Arc::new(ProcessSpawner), platform::detect)
    }
}

impl Global for EditorHost {}

/// Replaces how editors are found and started.
pub fn set_host(host: EditorHost, cx: &mut App) {
    cx.set_global(host);
}

/// Registers `o` on review tabs and `⌘,` on the main window.
pub fn init(cx: &mut App) {
    if !cx.has_global::<EditorHost>() {
        set_host(EditorHost::system(), cx);
    }
    handlers::on_action(
        cx,
        |tab: &mut ReviewTab, _: &viewport::OpenInEditor, _, cx| {
            let (file_idx, side, line) = cursor_target(tab, cx);
            open_in_editor(tab, file_idx, side, line, cx);
        },
    );
    handlers::on_action(
        cx,
        |_: &mut MainWindow, _: &window_actions::OpenSettings, _, cx| {
            open_settings(cx);
        },
    );
}

/// The tab's subscription to its viewport's header menu.
struct HeaderMenuOpen(#[allow(dead_code)] Subscription);

/// Opens the file of the header ⋯ menu's "Open in editor".
pub fn attach(tab: &mut ReviewTab, _window: &mut Window, cx: &mut Context<ReviewTab>) {
    let sub = cx.subscribe(
        &tab.viewport,
        |tab: &mut ReviewTab, _, event: &ViewportEvent, cx| {
            if let ViewportEvent::OpenInEditor {
                file_idx,
                side,
                line,
            } = *event
            {
                open_in_editor(tab, file_idx, side, line, cx);
            }
        },
    );
    tab.insert_extension(HeaderMenuOpen(sub));
}

/// What `o` opens: the cursor (the first line of a range), else the first
/// line of the file at the top of the viewport.
fn cursor_target(tab: &ReviewTab, cx: &App) -> (u32, Side, u32) {
    let viewport = tab.viewport.read(cx);
    if let Some(pos) = viewport.cursor() {
        return (pos.file_idx, pos.side, pos.lines().0);
    }
    let file_idx = viewport.anchor().file_idx;
    let side = match tab.opened.files.get(file_idx as usize) {
        Some(change) if change.new_path.is_none() => Side::Old,
        _ => Side::New,
    };
    (file_idx, side, 0)
}

/// The worktree whose files "open in editor" opens: the live review's,
/// else the one the review was opened from, else the repo's main worktree.
pub fn review_worktree(opened: &OpenedDiff) -> Option<PathBuf> {
    if let Some(live) = &opened.live {
        return Some(live.worktree.clone());
    }
    if let Some(top) = &opened.repo.toplevel {
        return Some(top.clone());
    }
    let common = &opened.repo.common_dir;
    (common.file_name()? == ".git")
        .then(|| common.parent().map(Path::to_path_buf))
        .flatten()
}

/// Opens 0-based `line` on `side` of file `file_idx` in the editor.
pub fn open_in_editor(
    tab: &ReviewTab,
    file_idx: u32,
    side: Side,
    line: u32,
    cx: &mut Context<ReviewTab>,
) {
    let Some(change) = tab.opened.files.get(file_idx as usize).cloned() else {
        return;
    };
    let provider = tab.viewport.read(cx).provider().clone();
    let worktree = review_worktree(&tab.opened);
    let blobs_dir = AppState::global(cx).paths.blobs_dir.clone();
    let launch = launcher(cx);
    run(cx, async move {
        let target = target::resolve(
            &change,
            side,
            line,
            worktree.as_deref(),
            provider.as_ref(),
            &blobs_dir,
        )?;
        launch(&target.path, target.line)
    });
}

/// Opens `settings.json` in the editor, first writing it with the §18
/// defaults when it does not exist.
pub fn open_settings(cx: &mut App) {
    let path = SettingsStore::global(cx).path().to_path_buf();
    let launch = launcher(cx);
    run(cx, async move {
        write_default_settings(&path)?;
        launch(&path, 1)
    });
}

/// Writes `path` with every setting at its default, unless it exists.
fn write_default_settings(path: &Path) -> anyhow::Result<()> {
    use std::io::Write as _;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(&Settings::default())?;
    let text = format!(
        "// Polygloss settings, every key at its default. Changes apply when you save.\n{json}\n"
    );
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => Ok(file.write_all(text.as_bytes())?),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(e) => Err(e.into()),
    }
}

type Launch = Box<dyn FnOnce(&Path, u32) -> anyhow::Result<()> + Send>;

/// Starts the editor on `(path, 1-based line)`: `editor.command`, else the
/// detected editor, else `open -t`. Built on the UI thread, run off it.
fn launcher(cx: &App) -> Launch {
    let template = SettingsStore::global(cx)
        .settings()
        .editor
        .command
        .clone()
        .filter(|t| !t.trim().is_empty());
    let host = cx.global::<EditorHost>();
    let (spawner, detect) = (host.spawner.clone(), host.detect.clone());
    let scratch = AppState::global(cx).paths.cache_dir.join("commands");
    Box::new(move |path, line| {
        let command = match template {
            // A bare program name is looked up where the shims live too.
            Some(t) => EditorCommand::from_template(&t)
                .resolved(&platform::DetectEnv::from_process().search_path),
            None => detect().unwrap_or_else(EditorCommand::system_default),
        };
        command.launch(path, line, spawner.as_ref(), &scratch)?;
        Ok(())
    })
}

/// Runs `work` on the background executor; an error becomes a toast in the
/// main window.
fn run(cx: &mut App, work: impl Future<Output = anyhow::Result<()>> + Send + 'static) {
    let task = cx.background_spawn(work);
    cx.spawn(async move |cx: &mut AsyncApp| {
        if let Err(err) = task.await {
            tracing::warn!("open in editor: {err:#}");
            let message = format!("Could not open the editor: {err:#}");
            cx.update(|cx| toast_error(message, cx));
        }
    })
    .detach();
}

fn toast_error(message: String, cx: &mut App) {
    let Some((handle, main)) = crate::window::main_window(cx) else {
        return;
    };
    let _ = handle.update(cx, |_, window, cx| {
        main.update(cx, |main, cx| main.toast_error(message.into(), window, cx))
    });
}
