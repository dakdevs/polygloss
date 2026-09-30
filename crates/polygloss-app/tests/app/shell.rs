//! The app shell (T3.1, design §11.1, §11.7, §5.3): one window with tabs,
//! Home first, one tab per review, the review tab's layout (toolbar, banner
//! strip, file tree | viewport | threads panel), the binary write-back, the
//! launch arguments and reopening after the last window closed.

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use futures::FutureExt as _;
use gpui_kit::{Entity, SharedString, TestAppContext, VisualTestContext};
use polygloss_app::review_tab::{self, BannerKind, ReviewTab, open_review};
use polygloss_app::startup::{self, LaunchArgs};
use polygloss_app::tabs::TabItem;
use polygloss_app::window::{self, MainWindow};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::{FileKind, ObjectFormat};

use crate::support::{FixtureRepo, Sandbox, code_change_repo, strings};

/// A running app in the sandbox: gpui-kit, every feature, the key bindings
/// and the main window with its Home tab.
pub struct Shell<'a> {
    pub core: Core,
    pub main: Entity<MainWindow>,
    pub cx: &'a mut VisualTestContext,
}

/// Starts the app the way `Polygloss` does (without the platform's run
/// loop). Call after `Sandbox::isolate()`.
pub fn start(cx: &mut TestAppContext) -> Shell<'_> {
    let core = Core::open_default().expect("open the sandbox store");
    let c = core.clone();
    let (window, main) = cx.update(|cx| {
        startup::init(c, cx);
        // Tests never start a real editor (T3.16): `tests/app/editor.rs`
        // installs a recording spawner; anything else fails loudly.
        polygloss_app::editor::set_host(
            polygloss_app::editor::EditorHost::new(Arc::new(NoEditors), || None),
            cx,
        );
        // Nor install the CLI anywhere (T5.4): `tests/app/install_cli.rs`
        // installs a recording installer.
        polygloss_app::install_cli::set_installer(
            polygloss_app::install_cli::CliInstaller::new(
                || None,
                |target: &std::path::Path| panic!("a test tried to install the CLI: {target:?}"),
            ),
            cx,
        );
        window::open_main_window(cx).expect("open the main window")
    });
    let vcx = VisualTestContext::from_window(window, cx).into_mut();
    draw(vcx);
    Shell {
        core,
        main,
        cx: vcx,
    }
}

/// The editor spawner of tests that did not install their own.
struct NoEditors;

impl polygloss_platform::editor::Spawner for NoEditors {
    fn spawn(&self, argv: &[std::ffi::OsString]) -> std::io::Result<()> {
        panic!("a test tried to start an editor: {argv:?}");
    }
}

/// Lets background work finish and draws a few frames.
pub fn draw(cx: &mut VisualTestContext) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
    }
    cx.run_until_parked();
}

/// `refs/tags/base..refs/tags/head` of [`code_change_repo`]. Full names: on
/// a case-insensitive disk `head` alone is `HEAD`.
pub fn compare_req(repo: &Path) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/tags/head".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

fn commit_req(repo: &Path, rev: &str) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Commit { rev: rev.into() },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

impl Shell<'_> {
    /// Opens `req` through `open_review` and waits for it.
    pub fn open(&mut self, req: OpenRequest) -> anyhow::Result<Entity<ReviewTab>> {
        let task = self.cx.update(|window, cx| open_review(req, window, cx));
        draw(self.cx);
        task.now_or_never().expect("the open finished")
    }

    /// `(tab count, active index)`.
    pub fn tabs(&mut self) -> (usize, usize) {
        self.main
            .read_with(self.cx, |m, _| (m.tabs().len(), m.tabs().active()))
    }

    pub fn tab(&mut self, ix: usize) -> TabItem {
        self.main
            .read_with(self.cx, |m, _| m.tabs().get(ix).cloned())
            .unwrap_or_else(|| panic!("no tab {ix}"))
    }

    pub fn active_review(&mut self) -> Option<Entity<ReviewTab>> {
        let (_, active) = self.tabs();
        self.tab(active).review().cloned()
    }
}

#[gpui_kit::test]
fn home_is_first_tab(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    assert_eq!(shell.tabs(), (1, 0));
    assert!(matches!(shell.tab(0), TabItem::Home(_)));

    shell
        .open(compare_req(repo.path()))
        .expect("open the review");
    assert_eq!(shell.tabs(), (2, 1));
    assert!(matches!(shell.tab(0), TabItem::Home(_)));
    assert!(matches!(shell.tab(1), TabItem::Review(_)));
    // Home cannot be closed; it has no close button.
    let closable = shell.main.read_with(shell.cx, |m, _| {
        (m.tabs().closable(0), m.tabs().closable(1))
    });
    assert_eq!(closable, (false, true));
}

#[gpui_kit::test]
fn opening_same_review_twice_focuses_existing_tab(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let first = shell.open(compare_req(repo.path())).unwrap();
    let other = shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));
    assert_ne!(first, other);

    let again = shell.open(compare_req(repo.path())).unwrap();
    assert_eq!(again, first, "the existing tab, not a new one");
    assert_eq!(shell.tabs(), (3, 1));
    assert_eq!(shell.active_review(), Some(first));
}

#[gpui_kit::test]
fn close_tab_cmd_w(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let first = shell.open(compare_req(repo.path())).unwrap();
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));

    shell.cx.simulate_keystrokes("cmd-w");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 1));
    assert_eq!(shell.active_review(), Some(first));
    // ⌘W on Home while a review is open does nothing: closing the window
    // would lose the review tab.
    shell.cx.simulate_keystrokes("cmd-{");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 0));
    shell.cx.simulate_keystrokes("cmd-w");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 0));
    assert_eq!(shell.cx.windows().len(), 1);
    shell.cx.simulate_keystrokes("cmd-}");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 1));
    shell.cx.simulate_keystrokes("cmd-w");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (1, 0));
    // ⌘W on Home alone closes the window (the app keeps running).
    shell.cx.simulate_keystrokes("cmd-w");
    shell.cx.run_until_parked();
    assert!(shell.cx.windows().is_empty());
}

#[gpui_kit::test]
fn next_prev_tab_shortcuts(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    shell.open(compare_req(repo.path())).unwrap();
    shell
        .open(commit_req(repo.path(), "refs/tags/head"))
        .unwrap();
    assert_eq!(shell.tabs(), (3, 2));
    let active = |shell: &mut Shell, keys: &str| {
        shell.cx.simulate_keystrokes(keys);
        draw(shell.cx);
        shell.tabs().1
    };
    // ⌘⇧] / ⌘⇧[ (macOS: ⌘} / ⌘{) and ⌃Tab / ⌃⇧Tab, wrapping around.
    assert_eq!(active(&mut shell, "cmd-}"), 0);
    assert_eq!(active(&mut shell, "cmd-}"), 1);
    assert_eq!(active(&mut shell, "cmd-{"), 0);
    assert_eq!(active(&mut shell, "cmd-{"), 2);
    assert_eq!(active(&mut shell, "ctrl-tab"), 0);
    assert_eq!(active(&mut shell, "ctrl-shift-tab"), 2);
}

#[gpui_kit::test]
fn last_window_closed_app_keeps_running_and_reopens(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    shell.cx.update(|window, _| window.remove_window());
    shell.cx.run_until_parked();
    let (windows, state) = cx.update(|cx| {
        (
            cx.windows().len(),
            cx.has_global::<polygloss_app::app_state::AppState>(),
        )
    });
    assert_eq!(windows, 0);
    assert!(state, "the app and its state live on");
    assert!(cx.update(|cx| window::main_window(cx)).is_none());

    // Clicking the Dock icon (`on_reopen`) opens the window again, Home first.
    cx.update(window::reopen);
    let (_, main) = cx
        .update(|cx| window::main_window(cx))
        .expect("the window is back");
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    let (len, home) = main.read_with(cx, |m, _| {
        (
            m.tabs().len(),
            matches!(m.tabs().get(0), Some(TabItem::Home(_))),
        )
    });
    assert_eq!((len, home), (1, true));
    // A second reopen with the window open does nothing.
    cx.update(window::reopen);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
}

#[gpui_kit::test]
fn review_tab_has_toolbar_banner_strip_and_three_panes(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let bounds = |shell: &mut Shell, name: &'static str| {
        shell
            .cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not painted"))
    };
    let tab_bar = bounds(&mut shell, "tab-bar");
    let toolbar = bounds(&mut shell, "review-toolbar");
    let banners = bounds(&mut shell, "banner-strip");
    let tree = bounds(&mut shell, "file-tree-pane");
    let viewport = bounds(&mut shell, "viewport-pane");
    let threads = bounds(&mut shell, "threads-pane");
    // Top to bottom: tab bar, toolbar, banner strip, then the panes.
    assert!(tab_bar.bottom() <= toolbar.top());
    assert!(toolbar.bottom() <= banners.top());
    assert!(banners.bottom() <= viewport.top());
    assert!(banners.size.height > gpui_kit::px(0.), "reserved height");
    // Left to right: file tree | viewport | threads panel.
    assert!(tree.right() <= viewport.left());
    assert!(viewport.right() <= threads.left());
    assert!(viewport.size.width > tree.size.width);
    assert_eq!(tree.top(), viewport.top());
    assert_eq!(threads.top(), viewport.top());

    // The threads panel toggles; the viewport takes its room.
    shell
        .cx
        .dispatch_action(polygloss_app::review_tab::panes::ToggleThreadsPanel);
    draw(shell.cx);
    assert!(!tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
    assert!(shell.cx.debug_bounds("threads-pane").is_none());
    let wider = bounds(&mut shell, "viewport-pane");
    assert!(wider.size.width > viewport.size.width);
    shell
        .cx
        .dispatch_action(polygloss_app::review_tab::panes::ToggleThreadsPanel);
    draw(shell.cx);
    assert!(tab.read_with(shell.cx, |t, _| t.threads_panel_visible()));
}

#[gpui_kit::test]
fn banner_strip_never_changes_viewport_anchor(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| v.scroll_by(90.0, cx));
    draw(shell.cx);
    let before = (
        viewport.read_with(shell.cx, |v, _| (v.anchor(), v.debug().visible_rows)),
        shell.cx.debug_bounds("viewport-pane"),
    );

    let banners = tab.read_with(shell.cx, |t, _| t.banners.clone());
    banners.update(shell.cx, |b, cx| {
        b.set(
            BannerKind::LiveChanges,
            "3 files changed".into(),
            Box::new(polygloss_app::review_tab::panes::ToggleThreadsPanel),
            cx,
        );
        b.set(
            BannerKind::AgentReplies,
            "claude-code replied to 2 threads".into(),
            Box::new(polygloss_app::review_tab::panes::ToggleThreadsPanel),
            cx,
        );
    });
    draw(shell.cx);
    let shown: Vec<(BannerKind, SharedString)> = banners.read_with(shell.cx, |b, _| b.banners());
    assert_eq!(
        shown,
        [
            (BannerKind::LiveChanges, "3 files changed".into()),
            (
                BannerKind::AgentReplies,
                "claude-code replied to 2 threads".into()
            ),
        ]
    );
    let with_banners = (
        viewport.read_with(shell.cx, |v, _| (v.anchor(), v.debug().visible_rows)),
        shell.cx.debug_bounds("viewport-pane"),
    );
    assert_eq!(with_banners, before, "nothing moved");

    banners.update(shell.cx, |b, cx| {
        b.clear(BannerKind::LiveChanges, cx);
        b.clear(BannerKind::AgentReplies, cx);
    });
    draw(shell.cx);
    assert!(banners.read_with(shell.cx, |b, _| b.banners()).is_empty());
    let cleared = (
        viewport.read_with(shell.cx, |v, _| (v.anchor(), v.debug().visible_rows)),
        shell.cx.debug_bounds("viewport-pane"),
    );
    assert_eq!(cleared, before);
}

#[gpui_kit::test]
fn toolbar_and_banner_buttons_reach_the_tab_without_focus(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let visible = |shell: &mut Shell| tab.read_with(shell.cx, |t, _| t.threads_panel_visible());
    let click = |shell: &mut Shell, name: &'static str| {
        // Nothing focused: an action dispatched from the focused element
        // would never reach the tab.
        shell.cx.update(|window, cx| window.blur(cx));
        let at = shell
            .cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not painted"))
            .center();
        shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
        draw(shell.cx);
    };
    assert!(visible(&mut shell));
    click(&mut shell, "toggle-threads-panel");
    assert!(!visible(&mut shell), "the toolbar toggle hid the panel");

    let banners = tab.read_with(shell.cx, |t, _| t.banners.clone());
    banners.update(shell.cx, |b, cx| {
        b.set(
            BannerKind::NewIteration,
            "New iteration available".into(),
            Box::new(polygloss_app::review_tab::panes::ToggleThreadsPanel),
            cx,
        );
    });
    draw(shell.cx);
    click(&mut shell, "banner-button-0");
    assert!(visible(&mut shell), "the banner's action reached the tab");
}

#[gpui_kit::test]
fn binary_detected_writes_file_kind_back(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("data.dat", b"text for now\n");
    repo.write("notes.txt", b"one\n");
    repo.commit("base");
    repo.git(&["tag", "base"]);
    // No attribute says binary, so `diff-tree` lists it as text; the NUL is
    // found when the viewport reads the blob.
    repo.write("data.dat", b"bin\0ary\n");
    repo.write("notes.txt", b"two\n");
    repo.commit("head");
    repo.git(&["tag", "head"]);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let diff_id = tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone());
    let idx = tab.read_with(shell.cx, |t, _| {
        t.opened
            .files
            .iter()
            .position(|f| f.display_path() == "data.dat")
            .unwrap()
    });
    let kind = |core: &Core| core.files_for_diff(&diff_id).unwrap().unwrap()[idx].kind;
    draw(shell.cx);
    assert_eq!(kind(&shell.core), FileKind::Binary);
    assert_eq!(
        shell
            .core
            .files_for_diff(&diff_id)
            .unwrap()
            .unwrap()
            .iter()
            .filter(|f| f.kind == FileKind::Binary)
            .count(),
        1
    );
    // The next open sees it as binary.
    let reopened = shell.core.open(&compare_req(repo.path())).unwrap();
    assert_eq!(reopened.files[idx].kind, FileKind::Binary);
}

#[gpui_kit::test]
fn missing_objects_show_no_longer_available(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    // A review opened earlier, whose blobs are gone since (design §5.3: commit
    // and compare iterations rely on the user's objects).
    // A shallow clone for the second half, made while every object exists.
    let shallow = repo.clone_shallow();
    let core = Core::open_default().unwrap();
    let opened = core.open(&compare_req(repo.path())).unwrap();
    for f in opened.files.iter() {
        for oid in [&f.old_blob, &f.new_blob] {
            let hex = oid.to_string();
            let _ = std::fs::remove_file(
                repo.path()
                    .join(".git/objects")
                    .join(&hex[..2])
                    .join(&hex[2..]),
            );
        }
    }
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let rows = viewport.read_with(shell.cx, |v, _| v.debug().visible_rows);
    assert!(
        rows.iter()
            .any(|r| r.contains("objects no longer available")),
        "{rows:#?}"
    );

    // Objects missing at open (history beyond a shallow clone): the open
    // fails with the same wording, shown in the window, and no tab opens.
    let before = shell.tabs().0;
    let err = shell
        .open(commit_req(shallow.path(), "HEAD"))
        .expect_err("the parent is not in the clone");
    assert!(
        err.to_string().contains("objects no longer available"),
        "{err:#}"
    );
    assert_eq!(shell.tabs().0, before);
    let shown = shell
        .main
        .read_with(shell.cx, |m, _| m.open_errors().to_vec());
    assert!(
        shown
            .last()
            .is_some_and(|m| m.contains("objects no longer available")),
        "{shown:?}"
    );
}

#[test]
fn launch_args_parse_every_source() {
    let parse = |args: &[&str]| LaunchArgs::parse(&strings(args));
    assert!(parse(&[]).unwrap().open.is_none());
    let args = parse(&["--repo", "/r", "--compare", "main", "topic", "--direct"]).unwrap();
    let req = args.open.expect("a review to open");
    assert_eq!(req.worktree, Path::new("/r"));
    assert_eq!(
        req.source,
        Source::Compare {
            base: "main".into(),
            head: "topic".into(),
            mode: CompareMode::Direct,
        }
    );
    let req = parse(&["--repo", "/r", "--live"]).unwrap().open.unwrap();
    assert_eq!(
        req.source,
        Source::Live {
            since: Since::MergeBase
        }
    );
    let req = parse(&["--commit", "HEAD~1", "--repo", "/r"])
        .unwrap()
        .open
        .unwrap();
    assert_eq!(
        req.source,
        Source::Commit {
            rev: "HEAD~1".into()
        }
    );
    for bad in [
        &["--repo", "/r"][..],
        &["--live"],
        &["--repo", "/r", "--live", "--commit", "x"],
        &["--repo", "/r", "--commit", "x", "--direct"],
        &["--frobnicate"],
        &["--gate", "--repo", "/r", "--live"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?}");
    }
}

fn polygloss() -> Command {
    Command::new(env!("CARGO_BIN_EXE_Polygloss"))
}

#[test]
fn usage_errors_exit_2_without_a_window() {
    let _sb = Sandbox::isolate();
    for args in [&["--repo", "/r"][..], &["--frobnicate"], &["--gate"]] {
        let out = polygloss().args(args).output().expect("run Polygloss");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(stderr.contains("usage: Polygloss"), "{args:?}: {stderr}");
        assert!(out.stdout.is_empty());
    }
}

#[test]
fn perf_scenarios_need_polygloss_test() {
    let _sb = Sandbox::isolate();
    let out = polygloss()
        .env_remove("POLYGLOSS_TEST")
        .args(["--perf-scenario", "open", "--corpus", "typical", "--json"])
        .output()
        .expect("run Polygloss");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("POLYGLOSS_TEST=1"), "{stderr}");
    // With it, bad scenario arguments are usage errors too (no window).
    let out = polygloss()
        .env("POLYGLOSS_TEST", "1")
        .args(["--perf-scenario", "nope", "--corpus", "typical"])
        .output()
        .expect("run Polygloss");
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn logging_writes_a_rolling_file_in_the_logs_dir() {
    let sb = Sandbox::isolate();
    let dir = sb.home().join("Library/Logs/polygloss");
    let guard = polygloss_app::logging::init(&dir).expect("logging starts");
    tracing::info!(target: "polygloss_app", "hello from the log test");
    tracing::debug!(target: "polygloss_app", "below the default level");
    drop(guard);
    let files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(
        name.starts_with("polygloss.") && name.ends_with(".log"),
        "{name}"
    );
    let text = std::fs::read_to_string(&files[0]).unwrap();
    assert!(text.contains("hello from the log test"), "{text}");
    assert!(!text.contains("below the default level"), "{text}");
}

#[test]
fn review_titles_name_the_repo_and_both_sides() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    repo.git(&["branch", "topic", "refs/tags/head"]);
    repo.git(&["branch", "trunk", "refs/tags/base"]);
    let core = Core::open_default().unwrap();
    let name = repo
        .path()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let mut req = compare_req(repo.path());
    req.source = Source::Compare {
        base: "trunk".into(),
        head: "topic".into(),
        mode: CompareMode::ThreeDot,
    };
    let opened = core.open(&req).unwrap();
    assert_eq!(
        review_tab::title(&opened),
        format!("{name} · trunk...topic")
    );
    let base = opened.base.commit.as_ref().unwrap().short().to_string();
    let head = opened.head_commit.as_ref().unwrap().short().to_string();
    assert_eq!(
        review_tab::description(&opened),
        format!("trunk ({base}) → topic ({head}) · 3 files changed")
    );
    let commit = core.open(&commit_req(repo.path(), "topic")).unwrap();
    assert_eq!(review_tab::title(&commit), format!("{name} · {head}"));

    // Dotted refs (version tags, release branches) keep their dots: the key
    // splits at the separator, not at the last dot.
    repo.git(&["tag", "v1.2.0", "refs/tags/head"]);
    repo.git(&["branch", "release-1.2", "refs/tags/head"]);
    for (head_ref, mode, sep, shown) in [
        ("refs/tags/v1.2.0", CompareMode::ThreeDot, "...", "v1.2.0"),
        ("v1.2.0", CompareMode::Direct, "..", "v1.2.0"),
        ("release-1.2", CompareMode::Direct, "..", "release-1.2"),
    ] {
        req.source = Source::Compare {
            base: "trunk".into(),
            head: head_ref.into(),
            mode,
        };
        let opened = core.open(&req).unwrap();
        assert_eq!(
            review_tab::title(&opened),
            format!("{name} · trunk{sep}{shown}"),
            "{head_ref}"
        );
        assert_eq!(
            review_tab::description(&opened),
            format!("trunk ({base}) → {shown} ({head}) · 3 files changed"),
            "{head_ref}"
        );
    }

    // Live keys: branch names may hold `@` and `#`.
    repo.git(&["checkout", "-q", "-b", "feat@x#1", "refs/tags/head"]);
    let live = core
        .open(&OpenRequest {
            source: Source::Live { since: Since::Head },
            ..compare_req(repo.path())
        })
        .unwrap();
    assert_eq!(
        review_tab::title(&live),
        format!("{name} · feat@x#1 (working tree)")
    );
    assert_eq!(review_tab::short_ref(&"a".repeat(40)), "aaaaaaa");
    assert_eq!(
        review_tab::short_ref("refs/remotes/origin/main"),
        "origin/main"
    );
}
