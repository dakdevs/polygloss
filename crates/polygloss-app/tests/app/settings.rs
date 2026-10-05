//! `settings.json` (T3.1, design §18, OQ-19): defaults, parsing, hot reload
//! and keeping the last good settings when the file is invalid.

use std::time::{Duration, Instant};

use gpui_kit::TestAppContext;
use polygloss_app::settings::model::{
    DiffAlgorithm, IndicatorStyle, LayoutSetting, ThemeMode, WordDiffSetting,
};
use polygloss_app::settings::{Settings, SettingsStore};
use polygloss_viewport::{Indicators, LayoutMode};

use crate::shell::{compare_req, draw, start};
use crate::support::{Sandbox, code_change_repo};

#[test]
fn buffer_font_ligatures_setting_reaches_the_viewport_font() {
    // Off by default: the code font turns contextual alternates and
    // standard ligatures off, so `->` is drawn as typed.
    let off = Settings::default().viewport_options();
    assert!(!off.ligatures);
    let features = polygloss_viewport::code_font_features(off.ligatures);
    assert_eq!(
        features.tag_value_list(),
        [("calt".to_owned(), 0), ("liga".to_owned(), 0)]
    );
    assert_eq!(features.is_calt_enabled(), Some(false));
    let font = polygloss_viewport::code_font("Lilex", off.ligatures);
    assert_eq!(font.family.as_ref(), "Lilex");
    assert_eq!(font.features, features);
    // `"ligatures": true` leaves the font's own defaults alone.
    let on = Settings::parse(r#"{ "buffer_font": { "ligatures": true } }"#)
        .unwrap()
        .viewport_options();
    assert!(on.ligatures);
    assert!(
        polygloss_viewport::code_font_features(on.ligatures)
            .tag_value_list()
            .is_empty()
    );
}

#[test]
fn settings_defaults_match_design_table() {
    let s = Settings::default();
    assert_eq!(s.theme.mode, ThemeMode::System);
    assert_eq!(s.theme.light, "Polygloss Light");
    assert_eq!(s.theme.dark, "Polygloss Dark");
    assert_eq!(s.buffer_font.family, "Lilex");
    assert_eq!(s.buffer_font.size, 13.0);
    assert!(!s.buffer_font.ligatures, "ligatures are off by default");
    assert_eq!(s.diff.layout, LayoutSetting::Auto);
    assert_eq!(s.diff.split_min_columns, 160);
    assert_eq!(s.diff.word_diff, WordDiffSetting::Word);
    assert_eq!(s.diff.algorithm, DiffAlgorithm::Myers);
    assert!(!s.diff.hide_whitespace);
    assert!(s.diff.style.backgrounds);
    assert_eq!(s.diff.style.indicators, IndicatorStyle::Bars);
    assert!(!s.diff.style.wrap);
    assert_eq!(s.diff.large_file_changed_lines, 20_000);
    assert!(s.diff.generated_patterns.is_empty());
    assert!(s.diff.renames);
    assert_eq!(s.diff.rename_threshold, 50);
    assert_eq!(s.editor.command, None);
    assert!(!s.agent_notes.hidden);
    assert!(s.notifications.enabled);
    assert_eq!(s.storage.prune_reviews_after_days, None);
    assert_eq!(s.updates.automatic_checks, None);

    // An empty object is the defaults; the design table's JSON spellings
    // parse; unknown keys are ignored and missing ones keep their default.
    assert_eq!(Settings::parse("{}").unwrap(), s);
    assert_eq!(Settings::parse("  ").unwrap(), s);
    let parsed = Settings::parse(
        r#"{
          // comments and trailing commas are fine, as in Zed's settings
          "theme": { "mode": "dark" },
          "buffer_font": { "size": 15 },
          "diff": {
            "layout": "unified",
            "word_diff": "off",
            "algorithm": "histogram",
            "style": { "indicators": "+-", "wrap": true },
            "generated_patterns": ["*.gen.ts"],
          },
          "editor": { "command": "zed {path}:{line}" },
          "storage": { "prune_reviews_after_days": 30 },
          "someday": { "unknown": true },
        }"#,
    )
    .unwrap();
    assert_eq!(parsed.theme.mode, ThemeMode::Dark);
    assert_eq!(parsed.theme.light, "Polygloss Light");
    assert_eq!(parsed.buffer_font.size, 15.0);
    assert_eq!(parsed.buffer_font.family, "Lilex");
    assert_eq!(parsed.diff.layout, LayoutSetting::Unified);
    assert_eq!(parsed.diff.word_diff, WordDiffSetting::Off);
    assert_eq!(parsed.diff.algorithm, DiffAlgorithm::Histogram);
    assert_eq!(parsed.diff.style.indicators, IndicatorStyle::PlusMinus);
    assert!(parsed.diff.style.wrap && parsed.diff.style.backgrounds);
    assert_eq!(parsed.diff.generated_patterns, ["*.gen.ts"]);
    assert_eq!(parsed.editor.command.as_deref(), Some("zed {path}:{line}"));
    assert_eq!(parsed.storage.prune_reviews_after_days, Some(30));

    // The viewport options they map to.
    let opts = parsed.viewport_options();
    assert_eq!(opts.layout, LayoutMode::Unified);
    assert_eq!(opts.code_font_size, 15.0);
    assert_eq!(opts.word_diff, None);
    assert_eq!(opts.style.indicators, Indicators::PlusMinus);
    assert!(opts.style.wrap);
    let defaults = Settings::default().viewport_options();
    assert_eq!(defaults.layout, LayoutMode::Auto);
    assert_eq!(defaults.split_min_columns, 160);
    assert_eq!(defaults.code_font, "Lilex");
    assert_eq!(defaults.style.indicators, Indicators::Bars);
    assert!(!defaults.ligatures);
    assert_eq!(defaults.large_file_changed_lines, 20_000);

    // Wrong types or unknown enum values make the file invalid.
    for bad in [
        r#"{ "diff": { "layout": "sideways" } }"#,
        r#"{ "buffer_font": { "size": "big" } }"#,
        r#"["not", "an", "object"]"#,
        r#"{ "theme": "#,
    ] {
        assert!(Settings::parse(bad).is_err(), "{bad}");
    }
    // So do values out of range.
    for (bad, names) in [
        (r#"{ "buffer_font": { "size": 0 } }"#, "buffer_font.size"),
        (r#"{ "buffer_font": { "size": -3 } }"#, "buffer_font.size"),
        (
            r#"{ "diff": { "split_min_columns": 0 } }"#,
            "diff.split_min_columns",
        ),
        (
            r#"{ "diff": { "rename_threshold": 101 } }"#,
            "diff.rename_threshold",
        ),
    ] {
        let err = Settings::parse(bad).expect_err(bad).to_string();
        assert!(err.contains(names), "{bad}: {err}");
    }
    for edge in [
        r#"{ "buffer_font": { "size": 0.5 } }"#,
        r#"{ "diff": { "split_min_columns": 1, "rename_threshold": 100 } }"#,
        r#"{ "diff": { "rename_threshold": 0 } }"#,
    ] {
        assert!(Settings::parse(edge).is_ok(), "{edge}");
    }
}

/// Runs the app until `done` holds (the watcher reports from its own
/// thread), for at most 10 s.
fn wait_until(
    cx: &mut gpui_kit::VisualTestContext,
    mut done: impl FnMut(&mut gpui_kit::VisualTestContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done(cx) {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(20));
        // The app polls the watcher's channel on a timer; move the test
        // clock with real time.
        cx.executor().advance_clock(Duration::from_millis(50));
        draw(cx);
    }
}

#[gpui_kit::test]
fn settings_invalid_json_keeps_previous_and_toasts(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let file = sb.config_dir().join("polygloss/settings.json");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, r#"{ "buffer_font": { "size": 16 } }"#).unwrap();
    let shell = start(cx);
    let size = |cx: &mut gpui_kit::VisualTestContext| {
        cx.update(|_, cx| SettingsStore::global(cx).settings().buffer_font.size)
    };
    assert_eq!(size(shell.cx), 16.0);

    std::fs::write(&file, r#"{ "buffer_font": { "size": 18 "#).unwrap();
    shell.cx.update(|_, cx| SettingsStore::reload(cx));
    draw(shell.cx);
    assert_eq!(size(shell.cx), 16.0, "the last good settings stay");
    let error = shell.cx.update(|_, cx| {
        SettingsStore::global(cx)
            .last_error()
            .map(|e| e.to_string())
    });
    let error = error.expect("the error is kept");
    assert!(error.contains("settings.json"), "{error}");
    // …and shown as a toast in the window.
    let toasts = shell.main.read_with(shell.cx, |m, _| m.toasts().to_vec());
    assert!(
        toasts.iter().any(|t| t.contains("settings.json")),
        "{toasts:?}"
    );
    let shown = shell.cx.update(|window, cx| {
        use gpui_kit::component::WindowExt as _;
        window.notifications(cx).len()
    });
    assert!(shown > 0, "a gpui-kit notification is up");

    // Fixing the file applies it and clears the error.
    std::fs::write(&file, r#"{ "buffer_font": { "size": 12 } }"#).unwrap();
    shell.cx.update(|_, cx| SettingsStore::reload(cx));
    assert_eq!(size(shell.cx), 12.0);
    assert!(
        shell
            .cx
            .update(|_, cx| SettingsStore::global(cx).last_error().is_none())
    );
}

#[gpui_kit::test]
fn settings_hot_reload_changes_font_size(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    let file = sb.config_dir().join("polygloss/settings.json");
    let mut shell = start(cx);
    // The app created its config dir and watches it before any file exists.
    assert!(file.parent().unwrap().is_dir());
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let font_size = |cx: &mut gpui_kit::VisualTestContext| {
        viewport.read_with(cx, |v, _| v.options().code_font_size)
    };
    assert_eq!(font_size(shell.cx), 13.0);

    // Written by an editor while the app runs: no reload call, the watcher
    // notices.
    std::fs::write(&file, r#"{ "buffer_font": { "size": 17 } }"#).unwrap();
    wait_until(shell.cx, |cx| font_size(cx) == 17.0);
    assert_eq!(
        shell
            .cx
            .update(|_, cx| SettingsStore::global(cx).settings().buffer_font.size),
        17.0
    );
    std::fs::write(&file, r#"{ "buffer_font": { "size": 11 } }"#).unwrap();
    wait_until(shell.cx, |cx| font_size(cx) == 11.0);
}

#[test]
fn jsonc_comments_and_trailing_commas_are_stripped_outside_strings() {
    use polygloss_app::settings::model::strip_jsonc;
    let text = "{\n  // a comment, with a comma,\n  \"a\": \"x // not a comment, \",\n  /* block\n */ \"b\": [1, 2,],\n}";
    let v: serde_json::Value = serde_json::from_str(&strip_jsonc(text)).unwrap();
    assert_eq!(v["a"], "x // not a comment, ");
    assert_eq!(v["b"], serde_json::json!([1, 2]));
    assert_eq!(strip_jsonc("\"a\\\"//b\""), "\"a\\\"//b\"");
    // Line numbers survive, so serde's errors point at the right line.
    assert_eq!(strip_jsonc(text).lines().count(), text.lines().count());
}

/// The app's production path (`startup::run` sets
/// `app_state::WatchersWakeTheApp`): a task awaiting the watcher's channel
/// sleeps until the watcher's own thread sends, no polling. GPUI's test
/// scheduler forbids such wakes, so this runs the receiver on a plain
/// executor.
#[test]
fn settings_watcher_wakes_an_awaiting_task() {
    use futures::StreamExt as _;
    use polygloss_app::settings::loader;

    let sb = Sandbox::isolate();
    let dir = sb.config_dir().join("polygloss");
    std::fs::create_dir_all(&dir).unwrap();
    let (watcher, mut rx) = loader::watch(&dir).expect("watch the config dir");
    let (woke_tx, woke_rx) = std::sync::mpsc::channel();
    let waiter = std::thread::spawn(move || {
        let woke = futures::executor::block_on(rx.next());
        let _ = woke_tx.send(woke);
        rx
    });
    // Other files in the dir are ignored; the settings file wakes it.
    std::fs::write(dir.join("other.json"), "{}").unwrap();
    std::thread::sleep(Duration::from_millis(400));
    assert!(woke_rx.try_recv().is_err(), "woken by another file");
    std::fs::write(dir.join("settings.json"), "{}").unwrap();
    let woke = woke_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("the watcher woke the task");
    assert_eq!(woke, Some(()));
    // Dropping the watcher closes the channel, which ends the app's task.
    let mut rx = waiter.join().unwrap();
    drop(watcher);
    assert_eq!(
        futures::executor::block_on(async {
            while rx.next().await.is_some() {}
            loader::take_changes(&mut rx)
        }),
        None
    );
}
