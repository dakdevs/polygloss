//! The hidden `Polygloss --dump-keymap --json` and `--dump-settings --json`
//! (T5.5): the data `docs/user-guide.md` is checked against
//! (`tests/scripts/docs.test.ts`).

use std::path::Path;
use std::process::{Command, Output};

use polygloss_app::dump;
use polygloss_app::keymap::actions::ACTIONS;
use polygloss_app::keymap::defaults::DEFAULT_BINDINGS;
use polygloss_app::settings::model::Settings;
use serde_json::Value;

/// `Polygloss <args>` with a throwaway HOME, config, data dir and git config.
fn run_sandboxed(root: &Path, args: &[&str]) -> Output {
    let home = root.join("home");
    std::fs::create_dir_all(&home).expect("create sandbox HOME");
    let git_config = root.join("gitconfig");
    std::fs::write(&git_config, "").expect("write empty gitconfig");
    Command::new(env!("CARGO_BIN_EXE_Polygloss"))
        .args(args)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("POLYGLOSS_DATA_DIR", root.join("data"))
        .env("GIT_CONFIG_GLOBAL", &git_config)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .expect("run Polygloss")
}

fn stdout_json(out: &Output) -> Value {
    assert!(
        out.status.success(),
        "exit status {:?}, stderr {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("stdout is one JSON document")
}

#[test]
fn keymap_dump_lists_every_default_binding_in_order() {
    let dump = dump::keymap_json();
    let bindings = dump["bindings"].as_array().expect("bindings array");
    assert_eq!(bindings.len(), DEFAULT_BINDINGS.len());
    for (b, &(keys, action, context)) in bindings.iter().zip(DEFAULT_BINDINGS) {
        assert_eq!(b["keys"], keys);
        assert_eq!(b["action"], action);
        let context = (!context.is_empty()).then_some(context);
        assert_eq!(b["context"].as_str(), context, "{keys} {action}");
        let title = ACTIONS.iter().find(|a| a.name == action).unwrap().title;
        assert_eq!(b["title"], title);
    }
}

#[test]
fn keymap_dump_labels_keys_like_the_cheat_sheet() {
    let dump = dump::keymap_json();
    let label = |keys: &str| {
        dump["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["keys"] == keys)
            .unwrap_or_else(|| panic!("no binding {keys}"))["label"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(label("escape"), "Esc");
    assert_eq!(label("cmd-k"), "⌘K");
    // macOS order: ⌃⌥⇧⌘.
    assert_eq!(label("cmd-shift-enter"), "⇧⌘⏎");
    assert_eq!(label("j"), "J");
}

#[test]
fn keymap_dump_lists_every_action() {
    let dump = dump::keymap_json();
    let actions = dump["actions"].as_array().expect("actions array");
    let names: Vec<&str> = actions
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    let expected: Vec<&str> = ACTIONS.iter().map(|a| a.name).collect();
    assert_eq!(names, expected);
    for (a, info) in actions.iter().zip(ACTIONS) {
        assert_eq!(a["title"], info.title);
    }
}

#[test]
fn settings_dump_is_the_defaults() {
    let dump = dump::settings_json();
    let parsed: Settings = serde_json::from_value(dump.clone()).expect("a settings value");
    assert_eq!(parsed, Settings::default());
    // Keys without a value by default are still named (as `null`).
    assert_eq!(dump["editor"]["command"], Value::Null);
    assert_eq!(dump["storage"]["prune_reviews_after_days"], Value::Null);
    assert_eq!(dump["buffer_font"]["ligatures"], false);
}

#[test]
fn settings_keys_flatten_to_dotted_leaves() {
    let keys = dump::settings_keys();
    for key in [
        "theme.mode",
        "buffer_font.size",
        "buffer_font.ligatures",
        "diff.style.indicators",
        "diff.generated_patterns",
        "editor.command",
        "updates.automatic_checks",
    ] {
        assert!(keys.iter().any(|(k, _)| k == key), "missing {key}");
    }
    // Sections are not leaves.
    assert!(!keys.iter().any(|(k, _)| k == "diff" || k == "diff.style"));
}

#[test]
fn app_dumps_the_keymap_and_settings_as_json_without_a_data_dir() {
    let root = tempfile::tempdir().expect("temp dir");
    let keymap = stdout_json(&run_sandboxed(root.path(), &["--dump-keymap", "--json"]));
    assert_eq!(keymap, dump::keymap_json());
    let settings = stdout_json(&run_sandboxed(root.path(), &["--dump-settings", "--json"]));
    assert_eq!(settings, dump::settings_json());
    // A dump opens no store, lock, log or window.
    assert!(
        !root.path().join("data").exists(),
        "the data dir was created"
    );
}

#[test]
fn dump_flags_need_json_and_stay_out_of_the_usage() {
    let root = tempfile::tempdir().expect("temp dir");
    let out = run_sandboxed(root.path(), &["--dump-keymap"]);
    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("--dump-keymap needs --json"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!polygloss_app::startup::USAGE.contains("dump"));
}

/// The ```jsonc examples of `docs/user-guide.md`, in order.
fn user_guide_examples() -> Vec<String> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/user-guide.md");
    let guide = std::fs::read_to_string(&path).expect("read docs/user-guide.md");
    guide
        .split("```jsonc\n")
        .skip(1)
        .map(|rest| rest.split("\n```").next().unwrap().to_owned())
        .collect()
}

#[test]
fn user_guide_keymap_and_settings_examples_parse() {
    let examples = user_guide_examples();
    // A keymap.json example (a list), then two settings.json ones: the file
    // categories' and the Settings section's.
    let (keymaps, settings): (Vec<&String>, Vec<&String>) = examples
        .iter()
        .partition(|e| e.trim_start().starts_with('['));
    assert_eq!(
        (keymaps.len(), settings.len()),
        (1, 2),
        "a keymap.json and two settings.json examples"
    );
    let bindings = polygloss_app::keymap::file::parse(keymaps[0])
        .unwrap_or_else(|e| panic!("the keymap.json example: {e}"));
    assert!(
        bindings.iter().any(|b| b.action.is_none()),
        "it unbinds a key"
    );
    assert!(
        bindings.iter().any(|b| b.context.is_none()),
        "it binds everywhere"
    );
    for example in &settings {
        let parsed =
            Settings::parse(example).unwrap_or_else(|e| panic!("a settings.json example: {e}"));
        assert_ne!(parsed, Settings::default());
    }
    // The File categories example says what its prose says.
    let categories = Settings::parse(settings[0]).unwrap().categories;
    assert_eq!(categories.tests.disabled_groups, ["snapshots"]);
    assert_eq!(categories.tests.patterns, ["spec/", "!spec/support/"]);
    assert_eq!(categories.generated.disabled_groups, ["build-output"]);
    assert!(categories.docs.enabled);
    assert_eq!(categories.custom[0].name, "Design tokens");
}
