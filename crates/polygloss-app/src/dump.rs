//! The hidden `Polygloss --dump-keymap --json` and `--dump-settings --json`
//! (plan T5.5): the default key bindings, every action, and the default
//! `settings.json` as JSON on stdout. `docs/user-guide.md` documents exactly
//! these, and `tests/scripts/docs.test.ts` holds it to them.
//!
//! A dump runs before anything else in [`crate::startup::main`]: it opens no
//! store, lock, log or window and reads no config file (it prints the
//! built-in defaults, not the user's overrides).

use std::process::ExitCode;

use serde_json::{Value, json};

use crate::keymap::actions::ACTIONS;
use crate::keymap::defaults::DEFAULT_BINDINGS;
use crate::keymap::file::parse_keys;
use crate::palette::key_cap::key_label;
use crate::settings::model::Settings;

/// The flags this module answers.
const DUMP_FLAGS: [&str; 2] = ["--dump-keymap", "--dump-settings"];

/// `{bindings: [{keys, label, action, title, context}], actions: [{name,
/// title}]}`: [`DEFAULT_BINDINGS`] in cheat-sheet order (`keys` in
/// `keymap.json` spelling, `label` as the cheat sheet draws it, `context`
/// `null` for bindings that apply everywhere) and every action of the
/// registry in palette order.
pub fn keymap_json() -> Value {
    let title = |action: &str| ACTIONS.iter().find(|a| a.name == action).map(|a| a.title);
    let bindings: Vec<Value> = DEFAULT_BINDINGS
        .iter()
        .map(|&(keys, action, context)| {
            json!({
                "keys": keys,
                "label": keys_label(keys),
                "action": action,
                "title": title(action),
                "context": (!context.is_empty()).then_some(context),
            })
        })
        .collect();
    let actions: Vec<Value> = ACTIONS
        .iter()
        .map(|a| json!({ "name": a.name, "title": a.title }))
        .collect();
    json!({ "bindings": bindings, "actions": actions })
}

/// `keys` as key caps (`cmd-shift-enter` → `⌘⇧⏎`); strokes of a sequence
/// are separated by a space.
fn keys_label(keys: &str) -> String {
    match parse_keys(keys) {
        Ok(strokes) => strokes.iter().map(key_label).collect::<Vec<_>>().join(" "),
        Err(_) => keys.to_owned(),
    }
}

/// The default `settings.json`, every key present (`null` where the default
/// is "unset").
pub fn settings_json() -> Value {
    serde_json::to_value(Settings::default()).expect("the settings model serializes")
}

/// Every settings key as a dotted path (`diff.style.wrap`) with its default,
/// in model order. Objects are sections, everything else (arrays and `null`
/// included) is a key.
pub fn settings_keys() -> Vec<(String, Value)> {
    fn walk(prefix: &str, value: &Value, out: &mut Vec<(String, Value)>) {
        match value {
            Value::Object(map) => {
                for (key, value) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    walk(&path, value, out);
                }
            }
            leaf => out.push((prefix.to_owned(), leaf.clone())),
        }
    }
    let mut out = Vec::new();
    walk("", &settings_json(), &mut out);
    out
}

/// Answers a dump flag in `args`, if there is one: the JSON on stdout and
/// exit 0, or a usage error (exit 2) without `--json` or with arguments it
/// does not take. `None`: not a dump; start the app.
pub fn run(args: &[String]) -> Option<ExitCode> {
    let flag = args.iter().find(|a| DUMP_FLAGS.contains(&a.as_str()))?;
    let others: Vec<&String> = args
        .iter()
        .filter(|a| *a != flag && *a != "--json")
        .collect();
    let err = |message: String| {
        eprintln!("Polygloss: {message}");
        Some(ExitCode::from(2))
    };
    if !args.iter().any(|a| a == "--json") {
        return err(format!("{flag} needs --json"));
    }
    if let Some(other) = others.first() {
        return err(format!("{flag} takes only --json, not {other:?}"));
    }
    let value = match flag.as_str() {
        "--dump-keymap" => keymap_json(),
        _ => settings_json(),
    };
    let text = serde_json::to_string_pretty(&value).expect("JSON values serialize");
    println!("{text}");
    Some(ExitCode::SUCCESS)
}
