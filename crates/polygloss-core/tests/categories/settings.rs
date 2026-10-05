//! The `categories` section: validation, unknown keys, partial objects,
//! defaults, labels and chips (design §11.15).

use super::*;

#[test]
fn invalid_patterns_and_custom_ids_are_errors() {
    let err = |value: serde_json::Value| {
        Categorizer::new(&config(value), &[]).expect_err("invalid categories")
    };
    match err(json!({ "tests": { "patterns": ["[abc"] } })) {
        CategoriesError::Pattern {
            category, pattern, ..
        } => assert_eq!((category.as_str(), pattern.as_str()), ("tests", "[abc")),
        other => panic!("{other:?}"),
    }
    // A rescue, and a disabled category, are checked too.
    assert!(matches!(
        err(json!({ "docs": { "enabled": false, "patterns": ["![x"] } })),
        CategoriesError::Pattern { category, .. } if category == "docs"
    ));
    let custom = |id: &str, name: &str| json!({ "custom": [{ "id": id, "name": name, "patterns": ["x/"] }] });
    let long_ok = format!("a{}", "b".repeat(40));
    let too_long = format!("a{}", "b".repeat(41));
    assert!(Categorizer::new(&config(custom(&long_ok, "N")), &[]).is_ok());
    assert!(Categorizer::new(&config(custom("a-1", "N")), &[]).is_ok());
    for bad in ["Bad", "1abc", "-a", "a_b", "", "a b", too_long.as_str()] {
        assert_eq!(
            err(custom(bad, "N")),
            CategoriesError::BadCustomId(bad.into()),
            "{bad:?}"
        );
    }
    assert_eq!(
        err(json!({ "custom": [
            { "id": "dup", "name": "A" },
            { "id": "dup", "name": "B" },
        ] })),
        CategoriesError::DuplicateId("dup".into())
    );
    assert_eq!(
        err(custom("tests", "Mine")),
        CategoriesError::DuplicateId("tests".into())
    );
    assert_eq!(
        err(custom("x", "  ")),
        CategoriesError::EmptyName("x".into())
    );
    assert!(matches!(
        err(json!({ "custom": [{ "id": "t", "name": "T", "patterns": ["{a"] , "enabled": false }, { "id": "u", "name": "U", "patterns": ["src/[z"] }] })),
        CategoriesError::Pattern { category, pattern, .. } if category == "custom:u" && pattern == "src/[z"
    ));
}

#[test]
fn too_many_patterns_is_an_error() {
    let patterns = |n: usize| -> Vec<String> { (0..n).map(|i| format!("p{i}.x")).collect() };
    assert!(
        Categorizer::new(
            &config(json!({ "tests": { "patterns": patterns(500) } })),
            &[]
        )
        .is_ok()
    );
    assert_eq!(
        Categorizer::new(
            &config(json!({ "tests": { "patterns": patterns(501) } })),
            &[]
        )
        .err(),
        Some(CategoriesError::TooManyPatterns("tests".into()))
    );
    assert_eq!(
        Categorizer::new(
            &config(json!({ "custom": [{ "id": "t", "name": "T", "patterns": patterns(501) }] })),
            &[]
        )
        .err(),
        Some(CategoriesError::TooManyPatterns("custom:t".into()))
    );
}

#[test]
fn unknown_custom_icon_falls_back_to_tag() {
    let c = categorizer(json!({ "custom": [
        { "id": "a", "name": "A", "patterns": ["a/"] },
        { "id": "b", "name": "B", "patterns": ["b/"], "icon": "sparkles" },
        { "id": "c", "name": "C", "patterns": ["c/"], "icon": "sparkles" },
        { "id": "d", "name": "D", "patterns": ["d/"], "icon": "layers" },
    ] }));
    let icons: Vec<&str> = c
        .enabled()
        .iter()
        .take(4)
        .map(|i| i.icon.as_str())
        .collect();
    assert_eq!(icons, ["tag", "tag", "tag", "layers"]);
    // One warning per unknown value.
    assert_eq!(c.warnings().len(), 1, "{:?}", c.warnings());
    assert!(c.warnings()[0].contains("sparkles"));
    assert_eq!(
        CUSTOM_ICONS,
        [
            "tag",
            "layers",
            "package",
            "book-open",
            "wrench",
            "flask-conical",
            "file-cog",
            "bot",
            "languages",
            "folder",
            "file"
        ]
    );
}

#[test]
fn unknown_keys_are_reported() {
    let cfg = config(json!({
        "test": { "enabled": false },
        "tests": { "enable": false, "disabled_groups": ["snapshot", "unit"] },
        "custom": [{ "id": "t", "name": "T", "colour": "red" }],
    }));
    assert_eq!(
        unknown_keys(&cfg),
        [
            "categories.test",
            "categories.tests.enable",
            "categories.custom[t].colour"
        ]
    );
    assert_eq!(unknown_groups(&cfg), ["tests/snapshot"]);
    // Nothing unknown is applied: Tests stays on.
    assert!(cfg.tests.enabled);
    assert!(unknown_keys(&CategoriesConfig::default()).is_empty());
    assert!(unknown_groups(&CategoriesConfig::default()).is_empty());
}

#[test]
fn partial_category_objects_keep_their_defaults() {
    let cfg = config(json!({
        "generated": { "patterns": ["!Cargo.lock"] },
        "tests": { "disabled_groups": ["snapshots"] },
        "docs": { "enabled": true },
    }));
    assert!(cfg.generated.enabled);
    assert_eq!(cfg.generated.disabled_groups, ["build-output"]);
    assert_eq!(cfg.generated.patterns, ["!Cargo.lock"]);
    assert!(cfg.tests.enabled);
    assert_eq!(cfg.tests.disabled_groups, ["snapshots"]);
    assert!(cfg.tests.patterns.is_empty());
    assert!(cfg.docs.enabled);
    assert!(cfg.docs.disabled_groups.is_empty());
    assert!(!cfg.vendored.enabled);
    assert_eq!(
        cfg.get(BuiltinCategory::Generated).patterns,
        ["!Cargo.lock"]
    );
    let mut toggled = cfg.clone();
    toggled.get_mut(BuiltinCategory::Vendored).enabled = true;
    assert!(toggled.vendored.enabled);
    assert_eq!(config(json!({})), CategoriesConfig::default());
    // A serialized config reads back the same.
    let text = serde_json::to_string(&cfg).unwrap();
    assert_eq!(
        serde_json::from_str::<CategoriesConfig>(&text).unwrap(),
        cfg
    );
}

#[test]
fn labels_and_chips_use_singular_and_plural() {
    let c = categorizer(json!({
        "agents": { "enabled": true },
        "custom": [{ "id": "tokens", "name": "Design tokens", "patterns": ["tokens/"] }],
    }));
    let info = |id: &str| {
        c.enabled()
            .iter()
            .find(|i| i.id.to_string() == id)
            .unwrap_or_else(|| panic!("{id} enabled"))
    };
    let tests = info("tests");
    assert_eq!(tests.label(1), "1 test file");
    assert_eq!(tests.label(12), "12 test files");
    assert_eq!(tests.chip(1), "1 test");
    assert_eq!(tests.chip(6), "6 tests");
    assert_eq!(info("generated").chip(2), "2 generated");
    assert_eq!(info("generated").label(1), "1 generated file");
    assert_eq!(info("agents").chip(1), "1 agent file");
    assert_eq!(info("agents").chip(2), "2 agent files");
    assert_eq!(info("agents").label(2), "2 agent config files");
    let tokens = info("custom:tokens");
    assert_eq!(tokens.title, "Design tokens");
    assert_eq!(tokens.label(1), "Design tokens · 1 file");
    assert_eq!(tokens.label(12), "Design tokens · 12 files");
    assert_eq!(tokens.chip(12), "12 design tokens");
}

/// A row of design §11.15's built-in table: key, title, default, groups,
/// icon, label noun (one / many), chip noun (one / many).
type DesignRow = (
    &'static str,
    &'static str,
    bool,
    &'static [&'static str],
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

#[test]
fn defaults_match_design_11_15() {
    #[rustfmt::skip]
    let table: [DesignRow; 7] = [
        ("tests", "Tests", true, &["unit", "e2e", "directories", "snapshots", "tooling"], "flask-conical", "test file", "test files", "test", "tests"),
        ("generated", "Generated", true, &["lockfiles", "generated-code", "build-output"], "file-cog", "generated file", "generated files", "generated", "generated"),
        ("vendored", "Vendored", false, &["vendored"], "package", "vendored file", "vendored files", "vendored", "vendored"),
        ("agents", "Agent config", false, &["agents"], "bot", "agent config file", "agent config files", "agent file", "agent files"),
        ("docs", "Docs", false, &["docs"], "book-open", "documentation file", "documentation files", "doc", "docs"),
        ("tooling", "Tooling & CI", false, &["ci", "lint-format", "build-config"], "wrench", "tooling file", "tooling files", "tooling", "tooling"),
        ("stories", "Stories & fixtures", false, &["stories", "fixtures", "i18n"], "layers", "story or fixture file", "stories, fixtures & i18n files", "fixture", "fixtures"),
    ];
    assert_eq!(BUILTINS.len(), table.len());
    for (def, row) in BUILTINS.iter().zip(table) {
        let (key, title, on, groups, icon, noun, nouns, chip, chips) = row;
        let got_groups: Vec<&str> = def.groups.iter().map(|g| g.key).collect();
        assert_eq!(
            (
                def.key,
                def.title,
                def.default_enabled,
                got_groups.as_slice(),
                def.icon
            ),
            (key, title, on, groups, icon)
        );
        assert_eq!(
            (def.noun, def.noun_plural, def.chip, def.chip_plural),
            (noun, nouns, chip, chips),
            "{key}"
        );
        assert_eq!(def.id.to_string(), key);
        let expected_disabled: &[&str] = if key == "generated" {
            &["build-output"]
        } else {
            &[]
        };
        assert_eq!(def.default_disabled_groups, expected_disabled, "{key}");
    }
    let cfg = CategoriesConfig::default();
    let settings = [
        (&cfg.tests, true),
        (&cfg.generated, true),
        (&cfg.vendored, false),
        (&cfg.agents, false),
        (&cfg.docs, false),
        (&cfg.tooling, false),
        (&cfg.stories, false),
    ];
    for (s, on) in settings {
        assert_eq!(s.enabled, on);
        assert!(s.patterns.is_empty());
    }
    assert_eq!(cfg.generated.disabled_groups, ["build-output"]);
    assert!(cfg.tests.disabled_groups.is_empty());
    assert!(cfg.custom.is_empty());
    let c = Categorizer::new(&cfg, &[]).unwrap();
    assert_eq!(enabled_ids(&c), ["tests", "generated"]);
    assert!(c.warnings().is_empty());
}

#[test]
fn category_ids_are_spelled_as_agents_see_them() {
    let tests = CategoryId::Builtin(BuiltinCategory::Tests);
    let tokens = CategoryId::Custom("tokens".into());
    assert_eq!(tests.to_string(), "tests");
    assert_eq!(tokens.to_string(), "custom:tokens");
    assert_eq!(
        serde_json::to_value(&tokens).unwrap(),
        json!("custom:tokens")
    );
    assert_eq!(
        serde_json::from_value::<CategoryId>(json!("stories")).unwrap(),
        CategoryId::Builtin(BuiltinCategory::Stories)
    );
    assert_eq!("custom:tokens".parse::<CategoryId>(), Ok(tokens));
    assert!("test".parse::<CategoryId>().is_err());
    assert!(serde_json::from_value::<CategoryId>(json!("Tests")).is_err());
}
