//! Screenshots of T3.5, the open flow (⌘O) over Home: the recent repos
//! (`e2e_open_flow_repos`), a repo's commit log (`e2e_open_flow_commits`)
//! and a branch compare with its label (`e2e_open_flow_compare`). The repos
//! live under the sandbox's `HOME`, so their paths read `~/…`, and the
//! history is written with fixed dates and authors, so ids and relative
//! times are the same on every run.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use gpui_kit::{AnyWindowHandle, Entity, HeadlessAppContext, px, size};
use polygloss_app::keymap::actions::window as window_actions;
use polygloss_app::open_flow::source_step::{SourceMode, SourceStep};
use polygloss_app::open_flow::{self, OpenFlow};
use polygloss_app::{startup, window};
use polygloss_core::review::Core;

use crate::support::Sandbox;
use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};

pub const TESTS: &[Test] = &crate::tests![
    e2e_open_flow_repos,
    e2e_open_flow_commits,
    e2e_open_flow_compare
];

/// The screenshot's "now": 2026-09-29 12:00 UTC.
const NOW_S: i64 = 1_790_683_200;
const HOUR_S: i64 = 60 * 60;

/// Frames drawn at most while the flow reads a repo.
const MAX_FRAMES: usize = 20;

/// `(subject, author, hours before NOW)`, oldest first.
const HISTORY: &[(&str, &str, i64)] = &[
    ("Initial commit", "Ada Lovelace", 30 * 24),
    ("Add the config parser", "Ada Lovelace", 26 * 24),
    ("Parse quoted values", "Grace Hopper", 21 * 24),
    ("Document app.conf in the README", "Ada Lovelace", 15 * 24),
    ("Release 0.3.0", "Ada Lovelace", 12 * 24),
    ("Keep config entries sorted by key", "Grace Hopper", 9 * 24),
    (
        "Reject duplicate keys with a clear error",
        "Grace Hopper",
        6 * 24,
    ),
    ("Add is_empty to Config", "Ada Lovelace", 4 * 24 + 5),
    (
        "Warn when app.conf has no settings",
        "Katherine Johnson",
        2 * 24 + 3,
    ),
    (
        "Fix parser crash on a trailing backslash",
        "Grace Hopper",
        30,
    ),
    ("Load the greeting with a fallback", "Katherine Johnson", 7),
    (
        "Open flow: rank recent repos with nucleo",
        "Ada Lovelace",
        2,
    ),
];

fn git(dir: &Path, args: &[&str], stdin: Option<&str>) {
    let mut child = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Ada Lovelace")
        .env("GIT_AUTHOR_EMAIL", "ada@polygloss.invalid")
        .env("GIT_COMMITTER_NAME", "Ada Lovelace")
        .env("GIT_COMMITTER_EMAIL", "ada@polygloss.invalid")
        .env("GIT_AUTHOR_DATE", format!("@{NOW_S} +0000"))
        .env("GIT_COMMITTER_DATE", format!("@{NOW_S} +0000"))
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .spawn()
        .expect("spawn git");
    if let Some(input) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
    }
    assert!(child.wait().unwrap().success(), "git {args:?}");
}

/// A repo at `dir` with [`HISTORY`] on `main`, `feature/open-flow` checked
/// out at the tip, `main` three commits behind, `origin/main` (the default
/// branch) and a tag.
fn make_repo(dir: &Path) {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"], None);
    let mut stream = String::new();
    for (i, (subject, author, hours)) in HISTORY.iter().enumerate() {
        let at = NOW_S - hours * HOUR_S;
        let email = author.to_lowercase().replace(' ', ".");
        let body = format!("change {i}\n");
        stream.push_str(&format!(
            "commit refs/heads/main\n\
             author {author} <{email}@polygloss.invalid> {at} +0000\n\
             committer {author} <{email}@polygloss.invalid> {at} +0000\n\
             data {}\n{subject}\nM 644 inline notes.txt\ndata {}\n{body}\n",
            subject.len(),
            body.len()
        ));
    }
    git(dir, &["fast-import", "--quiet"], Some(&stream));
    git(
        dir,
        &["checkout", "-q", "-f", "-b", "feature/open-flow", "main"],
        None,
    );
    git(dir, &["branch", "-f", "main", "main~3"], None);
    git(dir, &["tag", "v0.3.0", "main~2"], None);
    git(
        dir,
        &["update-ref", "refs/remotes/origin/main", "main"],
        None,
    );
    git(
        dir,
        &[
            "symbolic-ref",
            "refs/remotes/origin/HEAD",
            "refs/remotes/origin/main",
        ],
        None,
    );
}

/// Records `worktree` as a repo opened at `at_s`.
fn add_repo_row(core: &Core, worktree: &Path, at_s: i64) {
    std::fs::create_dir_all(worktree.join(".git")).unwrap();
    let name = worktree.file_name().unwrap().to_string_lossy().into_owned();
    core.store
        .write(|tx| {
            tx.execute(
                "INSERT INTO repos (common_dir, display_name, object_format, default_branch, \
                 created_at, last_opened_at) VALUES (?1, ?2, 'sha1', NULL, ?3, ?3)",
                (worktree.join(".git").to_str().unwrap(), name, at_s * 1000),
            )?;
            Ok(())
        })
        .unwrap();
}

/// The app with four recent repos (`~/src/polygloss` a real one, opened
/// last) and the open flow open on the repo step.
fn app_with_flow(cx: &mut HeadlessAppContext, home: &Path) -> (AnyWindowHandle, Entity<OpenFlow>) {
    // The dialog's entrance animation settles on its first frame, so every
    // capture shows it in place.
    cx.update(|cx| cx.set_reduce_motion(true));
    let core = Core::open_default().expect("open the sandbox store");
    make_repo(&home.join("src/polygloss"));
    add_repo_row(
        &core,
        &home.join("work/api-gateway"),
        NOW_S - 3 * 24 * HOUR_S,
    );
    add_repo_row(&core, &home.join("src/pierre-diffs"), NOW_S - 26 * HOUR_S);
    add_repo_row(&core, &home.join("oss/gpui-kit"), NOW_S - 9 * HOUR_S);
    add_repo_row(&core, &home.join("src/polygloss"), NOW_S - HOUR_S);
    let (handle, _main) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(cx, handle);
    screenshot::draw(cx, handle);
    cx.update_window(handle, |_, window, cx| {
        window.dispatch_action(Box::new(window_actions::OpenFlow), cx)
    })
    .expect("the window is open");
    for _ in 0..4 {
        screenshot::draw(cx, handle);
    }
    let flow = cx
        .update(|cx| open_flow::current(cx))
        .expect("⌘O opened the flow");
    (handle, flow)
}

/// Chooses `~/src/polygloss` and waits for the source step.
fn choose_repo(
    cx: &mut HeadlessAppContext,
    handle: AnyWindowHandle,
    flow: &Entity<OpenFlow>,
    home: &Path,
) -> Entity<SourceStep> {
    let path = home.join("src/polygloss");
    cx.update_window(handle, |_, window, cx| {
        flow.update(cx, |f, cx| f.choose_repo(path, window, cx))
    })
    .unwrap();
    for _ in 0..MAX_FRAMES {
        screenshot::draw(cx, handle);
        if let Some(source) = cx.update(|cx| flow.read(cx).source().cloned()) {
            return source;
        }
    }
    panic!("the source step never showed");
}

fn set_mode(
    cx: &mut HeadlessAppContext,
    handle: AnyWindowHandle,
    source: &Entity<SourceStep>,
    mode: SourceMode,
) {
    cx.update_window(handle, |_, window, cx| {
        source.update(cx, |s, cx| s.set_mode(mode, window, cx))
    })
    .unwrap();
    for _ in 0..4 {
        screenshot::draw(cx, handle);
    }
}

fn e2e_open_flow_repos() {
    let sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, _flow) = app_with_flow(&mut cx, sb.home());
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}

fn e2e_open_flow_commits() {
    let sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, flow) = app_with_flow(&mut cx, sb.home());
    let source = choose_repo(&mut cx, handle, &flow, sb.home());
    let commits = cx.update(|cx| source.read(cx).commits().clone());
    cx.update(|cx| {
        commits.update(cx, |l, _| {
            l.delegate_mut().set_clock(NOW_S * 1000, 0);
        })
    });
    set_mode(&mut cx, handle, &source, SourceMode::Commit);
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}

fn e2e_open_flow_compare() {
    let sb = Sandbox::isolate();
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, flow) = app_with_flow(&mut cx, sb.home());
    let source = choose_repo(&mut cx, handle, &flow, sb.home());
    set_mode(&mut cx, handle, &source, SourceMode::Compare);
    cx.update_window(handle, |_, window, cx| {
        source.update(cx, |s, cx| s.set_label("PR #42", window, cx))
    })
    .unwrap();
    for _ in 0..4 {
        screenshot::draw(&mut cx, handle);
    }
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}
