//! The M2 gate shell (`Polygloss --gate …`): argument parsing, the window it
//! opens over a real repo, and its error exits. T3.1 deletes the gate shell
//! and this module with it.

use std::path::PathBuf;
use std::process::Command;

use gpui_kit::{Modifiers, TestAppContext, VisualTestContext, point, px};
use polygloss_app::gate_shell::{self, GateArgs};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::review::Core;
use polygloss_diff::rows::Layout;
use polygloss_highlight::Appearance;
use polygloss_viewport::{ControlAction, LayoutMode, ViewportTheme};

use crate::support::{Sandbox, code_change_repo, strings};

fn parse(args: &[&str]) -> Result<GateArgs, String> {
    GateArgs::parse(&strings(args))
}

fn gate(repo: &str, source: Source) -> GateArgs {
    GateArgs {
        repo: PathBuf::from(repo),
        source,
        layout: LayoutMode::Auto,
        appearance: Appearance::Light,
    }
}

/// A unified row as `ViewportDebug::visible_rows` prints it (1-based numbers).
fn unified(old: Option<u32>, new: Option<u32>, marker: char, text: &str) -> String {
    let n = |v: Option<u32>| v.map_or(String::new(), |v| v.to_string());
    format!("{:>5} {:>5} {} {}", n(old), n(new), marker, text)
}

#[test]
fn gate_args_parse_every_source() {
    assert_eq!(
        parse(&["--repo", "/r", "--compare", "main", "topic"]).unwrap(),
        gate(
            "/r",
            Source::Compare {
                base: "main".into(),
                head: "topic".into(),
                mode: CompareMode::ThreeDot,
            }
        )
    );
    assert_eq!(
        parse(&["--direct", "--compare", "v6.10", "v6.11", "--repo", "/r"]).unwrap(),
        gate(
            "/r",
            Source::Compare {
                base: "v6.10".into(),
                head: "v6.11".into(),
                mode: CompareMode::Direct,
            }
        )
    );
    assert_eq!(
        parse(&["--repo", "/r", "--commit", "HEAD~1"]).unwrap(),
        gate(
            "/r",
            Source::Commit {
                rev: "HEAD~1".into()
            }
        )
    );
    assert_eq!(
        parse(&["--repo", "/r", "--live"]).unwrap(),
        gate(
            "/r",
            Source::Live {
                since: Since::MergeBase
            }
        )
    );
}

#[test]
fn gate_args_pin_layout_and_theme() {
    let args = parse(&[
        "--repo", "/r", "--live", "--layout", "unified", "--theme", "dark",
    ])
    .unwrap();
    assert_eq!(args.layout, LayoutMode::Unified);
    assert_eq!(args.appearance, Appearance::Dark);
    assert_eq!(
        parse(&["--repo", "/r", "--live", "--layout", "split"])
            .unwrap()
            .layout,
        LayoutMode::Split
    );

    let opts = gate_shell::viewport_options(&args);
    assert_eq!(opts.layout, LayoutMode::Unified);
    let dark = ViewportTheme::pierre(Appearance::Dark);
    assert_eq!(opts.theme.background, dark.background);
    assert_eq!(opts.theme.syntax_id(), dark.syntax_id());
    // Everything else is the viewport's default.
    let defaults = polygloss_viewport::ViewportOptions::default();
    assert_eq!(opts.code_font, defaults.code_font);
    assert_eq!(opts.code_font_size, defaults.code_font_size);
    assert_eq!(opts.syntax, defaults.syntax);
    assert_eq!(opts.word_diff, defaults.word_diff);
}

#[test]
fn gate_args_reject_incomplete_or_conflicting_input() {
    for bad in [
        &["--compare", "a", "b"][..],
        &["--repo", "/r"],
        &["--repo"],
        &["--repo", "/r", "--compare", "a"],
        &["--repo", "/r", "--commit", "a", "--live"],
        &["--repo", "/r", "--live", "--direct"],
        &["--repo", "/r", "--live", "--layout", "diagonal"],
        &["--repo", "/r", "--live", "--theme", "sepia"],
        &["--repo", "/r", "--live", "--frobnicate"],
        &["--repo", "/r", "--repo", "/s", "--live"],
    ] {
        assert!(parse(bad).is_err(), "{bad:?} must be rejected");
    }
}

#[gpui_kit::test]
fn gate_shell_opens_fixture_compare(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let args = parse(&[
        "--repo",
        &repo.path().to_string_lossy(),
        "--compare",
        "base",
        "head",
        "--direct",
        "--layout",
        "unified",
        "--theme",
        "dark",
    ])
    .unwrap();
    let core = Core::open_default().expect("open the sandbox store");
    let opened = gate_shell::open_diff(&core, &args).expect("open the fixture compare");
    assert_eq!(opened.files.len(), 3);

    let window = cx
        .update(|cx| {
            gate_shell::init(cx);
            gate_shell::open_window(opened, &args, cx)
        })
        .expect("open the gate window");
    let viewport = window
        .update(cx, |shell, _, _| shell.viewport().clone())
        .expect("gate window is open");
    let cx = VisualTestContext::from_window(*window, cx).into_mut();
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update(|window, _| window.refresh());
    }
    cx.run_until_parked();

    let (debug, layout, background) = viewport.read_with(cx, |v, _| {
        (
            v.debug(),
            v.effective_layout(),
            v.options().theme.background,
        )
    });
    assert_eq!(layout, Layout::Unified);
    assert_eq!(
        background,
        ViewportTheme::pierre(Appearance::Dark).background
    );
    assert_eq!(debug.visible_rows[0], "== src/config.rs");
    for row in [
        unified(Some(1), None, '-', "use std::collections::HashMap;"),
        unified(None, Some(1), '+', "use std::collections::BTreeMap;"),
        unified(Some(2), Some(2), ' ', ""),
    ] {
        assert!(
            debug.visible_rows.contains(&row),
            "{row:?} not in {:#?}",
            debug.visible_rows
        );
    }
    assert!(debug.styled_rows > 0, "syntax tokens were painted");

    // The header's ⋯ menu (a gpui-kit `PopupMenu`) opens: `init` set gpui-kit
    // up.
    let (x, y, w, h) = debug
        .controls
        .iter()
        .find(|c| c.action == ControlAction::Menu(0))
        .map(|c| c.bounds)
        .expect("the first header's menu button is painted");
    let center = point(px(x + w / 2.0), px(y + h / 2.0));
    cx.simulate_click(center, Modifiers::default());
    cx.run_until_parked();
    let menu = viewport.read_with(cx, |v, _| v.debug().menu);
    assert_eq!(menu.map(|m| m.file_idx), Some(0));
}

fn polygloss() -> Command {
    Command::new(env!("CARGO_BIN_EXE_Polygloss"))
}

#[test]
fn gate_shell_reports_open_errors_before_opening_a_window() {
    let sb = Sandbox::isolate();
    let not_a_repo = sb.home().join("not-a-repo");
    std::fs::create_dir_all(&not_a_repo).unwrap();
    let out = polygloss()
        .arg("--gate")
        .arg("--repo")
        .arg(&not_a_repo)
        .args(["--commit", "HEAD"])
        .output()
        .expect("run Polygloss");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.starts_with("Polygloss --gate: "), "stderr: {stderr}");
    assert!(
        stderr.contains(&*not_a_repo.to_string_lossy()),
        "stderr: {stderr}"
    );

    let repo = code_change_repo();
    let out = polygloss()
        .arg("--gate")
        .arg("--repo")
        .arg(repo.path())
        .args(["--commit", "no-such-rev"])
        .output()
        .expect("run Polygloss");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr: {stderr}");
    assert!(stderr.contains("no-such-rev"), "stderr: {stderr}");
}

#[test]
fn gate_shell_usage_errors_exit_2() {
    let _sb = Sandbox::isolate();
    for args in [
        &["--gate", "--repo", "/r"][..],
        &["--gate"],
        &[],
        &["--frobnicate"],
    ] {
        let out = polygloss().args(args).output().expect("run Polygloss");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{args:?}: {stderr}");
        assert!(
            stderr.contains("usage: Polygloss --gate --repo <path>"),
            "{args:?}: {stderr}"
        );
        assert!(out.stdout.is_empty());
    }
}
