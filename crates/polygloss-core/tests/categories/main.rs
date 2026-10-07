//! File categories (T6.4, design §11.15, ADR-0028): pattern forms, match
//! order, the `linguist-generated` attribute, rescues, settings, labels, the
//! catalog port and the lenient settings reader. Every expected category and
//! flag is written by hand from the design, the card or geld's tests
//! (`geld.rs`).

mod files;
mod geld;
mod order;
mod patterns;
mod settings;

use polygloss_core::categories::{
    BUILTINS, BuiltinCategory, CUSTOM_ICONS, CategoriesConfig, CategoriesError, Categorizer,
    CategoryId, Explain, Source, Verdict, unknown_groups, unknown_keys,
};
use polygloss_core::paths::DataPaths;
use polygloss_core::settings::{categories_config, categories_config_in};
use polygloss_core::testing::Sandbox;
use polygloss_diff::testing::assert_ratio_below;
use polygloss_diff::{FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, ObjectFormat, Oid};
use serde_json::json;

use GeneratedAttr::{Set, Unknown, Unset, Unspecified};

// ---------------------------------------------------------------- helpers

fn config(value: serde_json::Value) -> CategoriesConfig {
    serde_json::from_value(value).expect("categories config deserializes")
}

fn categorizer(value: serde_json::Value) -> Categorizer {
    Categorizer::new(&config(value), &[]).expect("categorizer builds")
}

/// Every built-in off: only custom categories can match.
fn all_builtins_off() -> serde_json::Value {
    json!({
        "tests": { "enabled": false },
        "generated": { "enabled": false },
        "vendored": { "enabled": false },
        "agents": { "enabled": false },
        "docs": { "enabled": false },
        "tooling": { "enabled": false },
        "stories": { "enabled": false },
    })
}

/// A categorizer with one custom category `t` holding `patterns` and every
/// built-in off, so a pattern's own rules decide alone.
fn only(patterns: &[&str]) -> Categorizer {
    let mut value = all_builtins_off();
    value["custom"] = json!([{ "id": "t", "name": "T", "patterns": patterns }]);
    categorizer(value)
}

fn hit(patterns: &[&str], path: &str) -> bool {
    only(patterns)
        .categorize(path, Unspecified, false)
        .is_some()
}

/// The category of `path` as its settings/agent spelling ("tests",
/// "custom:t"), attribute unspecified.
fn cat(c: &Categorizer, path: &str) -> Option<String> {
    cat_with(c, path, Unspecified, false)
}

fn cat_with(c: &Categorizer, path: &str, attr: GeneratedAttr, bit: bool) -> Option<String> {
    c.categorize(path, attr, bit).map(|id| id.to_string())
}

fn enabled_ids(c: &Categorizer) -> Vec<String> {
    c.enabled().iter().map(|i| i.id.to_string()).collect()
}

fn assert_hits(patterns: &[&str], yes: &[&str], no: &[&str]) {
    let c = only(patterns);
    for path in yes {
        assert!(
            c.categorize(path, Unspecified, false).is_some(),
            "{patterns:?} should match {path:?}"
        );
    }
    for path in no {
        assert!(
            c.categorize(path, Unspecified, false).is_none(),
            "{patterns:?} should not match {path:?}"
        );
    }
}

fn file(path: &[u8], attr: GeneratedAttr, bit: bool) -> FileChange {
    FileChange {
        idx: 0,
        status: FileStatus::Modified,
        old_path: Some(GitPath::from_bytes(path)),
        new_path: Some(GitPath::from_bytes(path)),
        old_mode: None,
        new_mode: None,
        old_blob: Oid::zero(ObjectFormat::Sha1),
        new_blob: Oid::zero(ObjectFormat::Sha1),
        similarity: None,
        kind: FileKind::Text,
        generated: bit,
        generated_attr: attr,
    }
}
