//! Match order: custom categories, the attribute step, built-ins, rescues,
//! Generated and the v1 recovery (design §11.15).

use super::*;

#[test]
fn rescue_skips_only_its_category() {
    let c = categorizer(json!({
        "tests": { "patterns": ["!tests/fixtures/"] },
        "stories": { "enabled": true },
    }));
    // Rescued from Tests; Stories' `fixtures/` takes it.
    assert_eq!(cat(&c, "tests/fixtures/x.json"), Some("stories".into()));
    assert_eq!(cat(&c, "tests/other.rs"), Some("tests".into()));
}

#[test]
fn custom_categories_match_before_builtins() {
    let c = categorizer(json!({
        "custom": [{ "id": "tokens", "name": "Design tokens", "patterns": ["tokens/", "*.test.tsx"] }],
    }));
    assert_eq!(enabled_ids(&c), ["custom:tokens", "tests", "generated"]);
    assert_eq!(cat(&c, "src/Button.test.tsx"), Some("custom:tokens".into()));
    assert_eq!(cat(&c, "tokens/colors.json"), Some("custom:tokens".into()));
    assert_eq!(cat(&c, "src/a.test.ts"), Some("tests".into()));
    assert_eq!(cat(&c, "src/index.ts"), None);
}

#[test]
fn linguist_generated_set_wins_after_custom() {
    let defaults = categorizer(json!({}));
    assert_eq!(
        cat_with(&defaults, "src/a.test.ts", Set, true),
        Some("generated".into())
    );
    assert!(defaults.is_generated("src/a.test.ts", Set, true));

    let custom = categorizer(json!({
        "custom": [{ "id": "src", "name": "Sources", "patterns": ["src/"] }],
    }));
    assert_eq!(
        cat_with(&custom, "src/a.test.ts", Set, true),
        Some("custom:src".into())
    );
    assert!(custom.is_generated("src/a.test.ts", Set, true));
}

#[test]
fn set_attribute_ignores_generated_rescues() {
    let c = categorizer(json!({ "generated": { "patterns": ["!x.pb.go"] } }));
    assert_eq!(cat_with(&c, "x.pb.go", Set, true), Some("generated".into()));
    assert!(c.is_generated("x.pb.go", Set, true));
    // Without the attribute the rescue works.
    assert_eq!(cat_with(&c, "x.pb.go", Unspecified, false), None);
    assert!(!c.is_generated("x.pb.go", Unspecified, false));
}

#[test]
fn disabled_generated_lets_a_set_file_fall_through() {
    let c = categorizer(json!({ "generated": { "enabled": false } }));
    assert_eq!(cat_with(&c, "a.test.ts", Set, true), Some("tests".into()));
    assert!(c.is_generated("a.test.ts", Set, true));
    assert_eq!(cat_with(&c, "src/main.rs", Set, true), None);
    assert!(c.is_generated("src/main.rs", Set, true));
}

#[test]
fn unset_attribute_is_never_generated() {
    let defaults = categorizer(json!({}));
    assert_eq!(cat_with(&defaults, "Cargo.lock", Unset, false), None);
    assert!(!defaults.is_generated("Cargo.lock", Unset, false));

    let docs = categorizer(json!({ "docs": { "enabled": true } }));
    assert_eq!(
        cat_with(&docs, "docs/Cargo.lock", Unset, false),
        Some("docs".into())
    );
    assert!(!docs.is_generated("docs/Cargo.lock", Unset, false));
    // Unspecified, the same file is Generated (Generated comes before Docs).
    assert_eq!(
        cat_with(&docs, "docs/Cargo.lock", Unspecified, false),
        Some("generated".into())
    );
}

#[test]
fn unknown_attribute_is_recovered_from_the_v1_bit() {
    let defaults = categorizer(json!({}));
    let rescued = categorizer(json!({ "generated": { "patterns": ["!gen.txt", "!Cargo.lock"] } }));
    // (categorizer, path, stored bit, expected category, expected generated)
    let cases: [(&Categorizer, &str, bool, Option<&str>, bool); 7] = [
        // Bit 0, listed: Unset, as v1 showed it.
        (&defaults, "Cargo.lock", false, None, false),
        // Bit 1, unlisted: Set.
        (&defaults, "gen.txt", true, Some("generated"), true),
        (&rescued, "gen.txt", true, Some("generated"), true),
        (&defaults, "a.test.ts", true, Some("generated"), true),
        // Bit 1, listed: Unspecified, so patterns decide and a rescue works.
        (&defaults, "Cargo.lock", true, Some("generated"), true),
        (&rescued, "Cargo.lock", true, None, false),
        // Bit 0, unlisted: Unspecified.
        (&defaults, "src/a.test.ts", false, Some("tests"), false),
    ];
    for (c, path, bit, category, generated) in cases {
        assert_eq!(
            cat_with(c, path, Unknown, bit).as_deref(),
            category,
            "{path} bit {bit}"
        );
        assert_eq!(
            c.is_generated(path, Unknown, bit),
            generated,
            "{path} bit {bit}"
        );
    }
}

#[test]
fn disabled_categories_and_groups_are_skipped() {
    let no_unit = categorizer(json!({ "tests": { "disabled_groups": ["unit"] } }));
    assert_eq!(cat(&no_unit, "src/a.test.ts"), None);
    assert_eq!(cat(&no_unit, "tests/x.rs"), Some("tests".into()));

    let no_tests = categorizer(json!({ "tests": { "enabled": false, "patterns": ["*.golden"] } }));
    assert_eq!(cat(&no_tests, "tests/x.rs"), None);
    assert_eq!(cat(&no_tests, "a.golden"), None);
    assert_eq!(enabled_ids(&no_tests), ["generated"]);

    let custom_off = categorizer(json!({
        "custom": [{ "id": "t", "name": "T", "patterns": ["tokens/"], "enabled": false }],
    }));
    assert_eq!(cat(&custom_off, "tokens/a.json"), None);
    assert_eq!(enabled_ids(&custom_off), ["tests", "generated"]);

    let no_lockfiles = categorizer(json!({ "generated": { "disabled_groups": ["lockfiles"] } }));
    assert_eq!(cat(&no_lockfiles, "Cargo.lock"), None);
    assert!(!no_lockfiles.is_generated("Cargo.lock", Unspecified, false));
    assert_eq!(cat(&no_lockfiles, "app.min.js"), Some("generated".into()));
}

#[test]
fn build_output_is_off_by_default() {
    let defaults = categorizer(json!({}));
    assert_eq!(cat(&defaults, "dist/app.js"), None);
    assert!(!defaults.is_generated("dist/app.js", Unspecified, false));
    assert!(!defaults.is_generated("pkg/out/x.o", Unspecified, false));

    let on = categorizer(json!({ "generated": { "disabled_groups": [] } }));
    for path in ["dist/app.js", "pkg/build/x.js", "out/a.txt"] {
        assert_eq!(cat(&on, path), Some("generated".into()), "{path}");
        assert!(on.is_generated(path, Unspecified, false), "{path}");
    }
}

#[test]
fn legacy_generated_patterns_extend_generated() {
    let c = Categorizer::new(&config(json!({})), &["*.out".into(), "gen/".into()]).unwrap();
    assert_eq!(cat(&c, "a/b.out"), Some("generated".into()));
    assert!(c.is_generated("a/b.out", Unspecified, false));
    assert_eq!(cat(&c, "x/gen/y.rs"), Some("generated".into()));
    assert_eq!(
        c.explain("a/b.out", Unspecified, false),
        Explain::Matched(Verdict {
            category: CategoryId::Builtin(BuiltinCategory::Generated),
            pattern: "*.out".into(),
            source: Source::Extra,
        })
    );
    assert!(c.warnings().is_empty());
}

#[test]
fn legacy_bang_lines_are_generated_rescues() {
    let c = Categorizer::new(&config(json!({})), &["!Cargo.lock".into()]).unwrap();
    assert_eq!(cat(&c, "Cargo.lock"), None);
    assert!(!c.is_generated("Cargo.lock", Unspecified, false));
    assert_eq!(cat(&c, "yarn.lock"), Some("generated".into()));
}

#[test]
fn invalid_legacy_patterns_are_dropped_with_a_warning() {
    let legacy = ["[abc".to_owned(), "*.out".to_owned()];
    let c = Categorizer::new(&config(json!({})), &legacy).expect("legacy patterns never fail");
    assert_eq!(cat(&c, "x.out"), Some("generated".into()));
    assert_eq!(c.warnings().len(), 1, "{:?}", c.warnings());
    assert!(c.warnings()[0].contains("[abc"), "{:?}", c.warnings());
}

#[test]
fn is_generated_ignores_the_enabled_flag() {
    let off = categorizer(json!({ "generated": { "enabled": false, "patterns": ["*.golden"] } }));
    assert_eq!(cat(&off, "Cargo.lock"), None);
    assert!(off.is_generated("Cargo.lock", Unspecified, false));
    assert!(off.is_generated("a/b.golden", Unspecified, false));
    // Disabled groups still count as disabled.
    assert!(!off.is_generated("dist/x.js", Unspecified, false));
    assert!(!off.is_generated("src/main.rs", Unspecified, false));

    let rescued =
        categorizer(json!({ "generated": { "enabled": false, "patterns": ["!Cargo.lock"] } }));
    assert!(!rescued.is_generated("Cargo.lock", Unspecified, false));
    assert!(rescued.is_generated("yarn.lock", Unspecified, false));
}

#[test]
fn explain_names_group_pattern_and_rescue() {
    let c = Categorizer::new(
        &config(json!({
            "tests": { "patterns": ["*.gen.ts", "!src/keep.test.ts", "!both/"] },
            "docs": { "enabled": true, "patterns": ["!both/"] },
            "custom": [{ "id": "tokens", "name": "Design tokens", "patterns": ["tokens/"] }],
        })),
        &[],
    )
    .unwrap();
    let tests = CategoryId::Builtin(BuiltinCategory::Tests);
    let matched = |category: CategoryId, pattern: &str, source: Source| {
        Explain::Matched(Verdict {
            category,
            pattern: pattern.into(),
            source,
        })
    };
    assert_eq!(
        c.explain("src/a.test.ts", Unspecified, false),
        matched(tests.clone(), "*.test.*", Source::BuiltIn { group: "unit" })
    );
    assert_eq!(
        c.explain("Cargo.lock", Unspecified, false),
        matched(
            CategoryId::Builtin(BuiltinCategory::Generated),
            "Cargo.lock",
            Source::BuiltIn { group: "lockfiles" }
        )
    );
    assert_eq!(
        c.explain("src/x.gen.ts", Unspecified, false),
        matched(tests.clone(), "*.gen.ts", Source::Extra)
    );
    assert_eq!(
        c.explain("tokens/a.json", Unspecified, false),
        matched(
            CategoryId::Custom("tokens".into()),
            "tokens/",
            Source::Custom
        )
    );
    let Explain::Matched(attr) = c.explain("src/main.rs", Unknown, true) else {
        panic!("a recovered Set is Generated");
    };
    assert_eq!(
        (attr.category, attr.source),
        (
            CategoryId::Builtin(BuiltinCategory::Generated),
            Source::Attribute
        )
    );
    assert_eq!(
        c.explain("src/keep.test.ts", Unspecified, false),
        Explain::Uncategorized {
            rescued_by: vec![(tests.clone(), "!src/keep.test.ts".into())]
        }
    );
    assert_eq!(
        c.explain("both/README.md", Unspecified, false),
        Explain::Uncategorized {
            rescued_by: vec![
                (tests.clone(), "!both/".into()),
                (CategoryId::Builtin(BuiltinCategory::Docs), "!both/".into()),
            ]
        }
    );
    assert_eq!(
        c.explain("src/index.ts", Unspecified, false),
        Explain::Uncategorized { rescued_by: vec![] }
    );
}
