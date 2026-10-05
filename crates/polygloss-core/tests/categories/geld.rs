//! The oracle: `geld_glob_and_matcher_vectors` ports geld's own
//! `packages/core/src/{glob,matcher}.test.ts` at commit
//! `5b8ce0e470fcd2b2538da6c700d7d96197f57de3` (MIT, Copyright (c) 2026 Brandon
//! McConnell, credited in `NOTICE`) as path → verdict tables, read through a
//! custom category (glob vectors) or geld's `DEFAULT_SETTINGS` (Tests on,
//! everything else off; only Tests membership compared). Cases left out, and
//! why:
//!
//! - `globToRegExp(…, { caseSensitive: false })`: matching is always
//!   case-sensitive here.
//! - `compileGlobs` normalizing `./tests/a.ts` and `tests\a.ts`: git paths
//!   never start with `./` and `\` is an escape here.
//! - `tests/` matching the file `tests`: a trailing slash matches only a
//!   directory component here (OQ-45, gitignore semantics; geld also
//!   matches a file of that name).
//! - `expandBraces` returning its expansion: checked through matching
//!   instead (the expander is private).
//! - The master switch (`enabled: false`), repo-scoped `[owner/repo]`
//!   patterns (OQ-51) and the change kinds ("trivial", "large"): not ported.
//! - The custom category's `paintbrush` icon and nouns: custom icons come
//!   from design §11.15's allow-list and nouns from the name.
//! - A leading `/`: geld strips it (any depth); here it anchors at the root
//!   (OQ-45), pinned by `inner_and_leading_slash_anchor_at_the_root`.

use super::*;

#[test]
fn geld_glob_and_matcher_vectors() {
    // glob.test.ts: globToRegExp(pattern).test(path), through a custom category.
    #[rustfmt::skip]
    let globs: &[(&str, &str, bool)] = &[
        ("*.snap", "a.snap", true),
        ("*.snap", "deep/nested/__snapshots__/a.snap", true),
        ("*.snap", "a.snapshot", false),
        ("src/*.ts", "src/a.ts", true),
        ("src/*.ts", "src/sub/a.ts", false),
        ("src/**/*.ts", "src/a.ts", true),
        ("src/**/*.ts", "src/x/y/a.ts", true),
        ("src/**/*.ts", "lib/a.ts", false),
        ("docs/**", "docs/guide/index.md", true),
        ("tests/", "tests/a.ts", true),
        ("tests/", "packages/x/tests/deep/a.ts", true),
        ("tests/", "my-tests/a.ts", false),
        ("tests/", "src/testsuite/a.ts", false),
        ("*.Tests/", "src/App.Tests/UnitTest1.cs", true),
        ("*-snapshots/", "e2e/home.spec.ts-snapshots/home-1-chromium.png", true),
        ("file?.ts", "file1.ts", true),
        ("file?.ts", "file12.ts", false),
        ("file[0-9].ts", "file7.ts", true),
        ("file[!0-9].ts", "file7.ts", false),
        ("a.b", "a.b", true),
        ("a.b", "aXb", false),
        ("(x)+y", "src/(x)+y", true),
        ("*Test.java", "latest.java", false),
        ("*Test.java", "FooTest.java", true),
        // expandBraces, through matching.
        ("*.{js,ts}", "a.js", true),
        ("*.{js,ts}", "a.ts", true),
        ("{a,b}.{x,{y,z}}", "a.x", true),
        ("{a,b}.{x,{y,z}}", "b.z", true),
    ];
    for &(pattern, path, expected) in globs {
        assert_eq!(hit(&[pattern], path), expected, "{pattern} vs {path}");
    }

    // compileGlobs: `!` vetoes (a rescue here), blank lines and comments.
    let veto = only(&["*.spec.*", "!*.spec.yaml"]);
    assert_eq!(cat(&veto, "api.spec.ts"), Some("custom:t".into()));
    assert_eq!(cat(&veto, "openapi.spec.yaml"), None);
    let comments = only(&["", "# comment", "  *.snap  "]);
    assert_eq!(cat(&comments, "a.snap"), Some("custom:t".into()));
    assert_eq!(cat(&comments, "# comment"), None);
    // firstMatch: the first include pattern in list order.
    let first = only(&["*.snap", "tests/"]);
    let pattern_of = |path: &str| match first.explain(path, Unspecified, false) {
        Explain::Matched(v) => Some(v.pattern),
        Explain::Uncategorized { .. } => None,
    };
    assert_eq!(pattern_of("tests/a.snap").as_deref(), Some("*.snap"));
    assert_eq!(pattern_of("tests/a.ts").as_deref(), Some("tests/"));
    assert_eq!(pattern_of("src/a.ts"), None);

    // matcher.test.ts, built-in test detection under geld's DEFAULT_SETTINGS
    // (Tests on with every group, the rest off): only Tests membership.
    let geld_defaults = json!({ "generated": { "enabled": false } });
    let c = categorizer(geld_defaults.clone());
    let is_test = |path: &str| cat(&c, path).as_deref() == Some("tests");
    #[rustfmt::skip]
    let hides = [
        "src/utils.test.ts", "src/components/Button.spec.tsx",
        "packages/wxt/src/core/package-managers/__tests__/npm.test.ts",
        "packages/wxt/e2e/tests/auto-imports.test.ts", "packages/wxt/e2e/utils.ts",
        "playground/lazy-compilation/__tests__/lazy-compilation.spec.ts", "src/__mocks__/fs.ts",
        "src/__snapshots__/render.test.ts.snap", "cypress/e2e/login.cy.ts",
        "e2e/checkout.spec.ts-snapshots/checkout-1-chromium.png", "jest.config.js",
        "vitest.config.mts", "vitest.workspace.ts", "playwright.config.ts", "src/setupTests.ts",
        "test/fixtures/data.json", "tests/integration/api.int.test.ts",
        "apps/web/src/features/cart/cart.e2e.ts",
        "tests/test_models.py", "app/test_views.py", "app/models_test.py", "conftest.py",
        "pytest.ini", "tox.ini",
        "pkg/server/server_test.go", "pkg/server/testdata/golden.json",
        "spec/models/user_spec.rb", "spec/spec_helper.rb", ".rspec",
        "src/test/java/com/example/FooTest.java",
        "app/src/androidTest/java/com/example/ExampleInstrumentedTest.kt",
        "core/src/main/kotlin/FooTest.kt", "service/src/it/java/FooIT.java",
        "src/MyApp.Tests/UnitTest1.cs", "MyAppTests/MyAppTests.swift",
        "Tests/GeldTests/GeldTests.swift",
        "tests/Feature/LoginTest.php", "phpunit.xml.dist", "test/widget_test.dart",
        "test/geld_test.exs",
        "features/login.feature", "coverage/lcov.info", "scripts/deploy.bats",
    ];
    #[rustfmt::skip]
    let keeps = [
        "src/index.ts", "README.md", "package.json", "bun.lock",
        "docs/.vitepress/loaders/cli.data.ts", "src/latest.java", "src/contest/results.ts",
        "src/testimonials/list.tsx", "openapi/api.spec.yaml", "src/components/Button.stories.tsx",
        "vite.config.ts", "src/attestation.ts", "src/protester.py", "lib/testable.rb",
        "assets/latest.png", "src/manifest.json",
    ];
    for path in hides {
        assert!(is_test(path), "geld hides {path}");
    }
    for path in keeps {
        assert!(!is_test(path), "geld keeps {path}");
    }

    // Settings interplay.
    let off = categorizer(json!({
        "tests": { "enabled": false, "patterns": ["*.generated.ts"] },
        "generated": { "enabled": false },
    }));
    assert_eq!(cat(&off, "src/a.test.ts"), None);
    assert_eq!(cat(&off, "src/schema.generated.ts"), None);

    let extras = categorizer(json!({
        "generated": { "patterns": ["*.golden"] },
        "tests": { "patterns": ["*.spec.yaml"] },
    }));
    let verdict = |c: &Categorizer, path: &str| match c.explain(path, Unspecified, false) {
        Explain::Matched(v) => (v.category.to_string(), v.pattern, v.source),
        other => panic!("{path}: {other:?}"),
    };
    assert_eq!(
        verdict(&extras, "lib/a.golden"),
        ("generated".into(), "*.golden".into(), Source::Extra)
    );
    assert_eq!(
        verdict(&extras, "api.spec.yaml"),
        ("tests".into(), "*.spec.yaml".into(), Source::Extra)
    );

    let no_e2e = categorizer(json!({
        "tests": { "disabled_groups": ["e2e"] },
        "generated": { "enabled": false },
    }));
    assert_eq!(cat(&no_e2e, "cypress/e2e/login.cy.ts"), None);
    assert_eq!(cat(&no_e2e, "src/a.test.ts"), Some("tests".into()));
    let no_lockfiles = categorizer(json!({ "generated": { "disabled_groups": ["lockfiles"] } }));
    assert_eq!(cat(&no_lockfiles, "pnpm-lock.yaml"), None);
    assert_eq!(
        cat(&no_lockfiles, "dist/bundle.min.js"),
        Some("generated".into())
    );

    let mut tokens = geld_defaults.clone();
    tokens["custom"] = json!([{ "id": "tokens", "name": "Design tokens", "patterns": ["tokens/**", "*.test.tsx"] }]);
    let tokens = categorizer(tokens);
    assert_eq!(enabled_ids(&tokens), ["custom:tokens", "tests"]);
    assert_eq!(
        verdict(&tokens, "tokens/colors.json"),
        ("custom:tokens".into(), "tokens/**".into(), Source::Custom)
    );
    assert_eq!(tokens.enabled()[0].title, "Design tokens");
    assert_eq!(
        cat(&tokens, "src/Button.test.tsx"),
        Some("custom:tokens".into())
    );
    let mut tokens_off = geld_defaults.clone();
    tokens_off["custom"] = json!([{ "id": "tokens", "name": "T", "icon": "tag", "patterns": ["tokens/**"], "enabled": false }]);
    assert_eq!(cat(&categorizer(tokens_off), "tokens/colors.json"), None);

    let all = categorizer(json!({
        "vendored": { "enabled": true }, "agents": { "enabled": true }, "docs": { "enabled": true },
        "tooling": { "enabled": true }, "stories": { "enabled": true },
    }));
    for (path, expected) in [
        ("pnpm-lock.yaml", Some("generated")),
        ("vendor/lib/thing.js", Some("vendored")),
        (".cursor/rules/style.mdc", Some("agents")),
        ("CLAUDE.md", Some("agents")),
        ("README.md", Some("docs")),
        (".github/workflows/ci.yml", Some("tooling")),
        ("src/Button.stories.tsx", Some("stories")),
        ("src/index.ts", None),
    ] {
        assert_eq!(cat(&all, path).as_deref(), expected, "{path}");
    }
    assert_eq!(
        enabled_ids(&all),
        [
            "tests",
            "generated",
            "vendored",
            "agents",
            "docs",
            "tooling",
            "stories"
        ]
    );

    let explains = categorizer(json!({
        "tests": { "patterns": ["*.gen.ts", "!src/keep.test.ts"] },
        "generated": { "enabled": false },
    }));
    assert_eq!(
        verdict(&explains, "src/a.test.ts"),
        (
            "tests".into(),
            "*.test.*".into(),
            Source::BuiltIn { group: "unit" }
        )
    );
    assert_eq!(
        verdict(&explains, "src/x.gen.ts"),
        ("tests".into(), "*.gen.ts".into(), Source::Extra)
    );
    assert_eq!(
        explains.explain("src/keep.test.ts", Unspecified, false),
        Explain::Uncategorized {
            rescued_by: vec![(
                CategoryId::Builtin(BuiltinCategory::Tests),
                "!src/keep.test.ts".into()
            )]
        }
    );
    assert_eq!(
        explains.explain("src/index.ts", Unspecified, false),
        Explain::Uncategorized { rescued_by: vec![] }
    );

    let rescued = categorizer(json!({
        "docs": { "enabled": true },
        "tests": { "patterns": ["!tests/important/**"] },
        "generated": { "enabled": false },
    }));
    assert_eq!(cat(&rescued, "tests/important/keep.ts"), None);
    assert_eq!(cat(&rescued, "tests/other/hide.ts"), Some("tests".into()));
    assert_eq!(
        cat(&rescued, "tests/important/README.md"),
        Some("docs".into())
    );
}

#[test]
fn catalog_group_counts_match_geld_at_5b8ce0e() {
    // docs/research/redesign-reference.md#geld, written as literals; geld's
    // generated-code (25) minus `dist/`, `build/`, `out/`, which moved into
    // build-output.
    let expected: [(&str, &str, usize); 17] = [
        ("tests", "unit", 19),
        ("tests", "e2e", 17),
        ("tests", "directories", 33),
        ("tests", "snapshots", 9),
        ("tests", "tooling", 52),
        ("generated", "lockfiles", 22),
        ("generated", "generated-code", 22),
        ("generated", "build-output", 3),
        ("vendored", "vendored", 11),
        ("agents", "agents", 29),
        ("docs", "docs", 19),
        ("tooling", "ci", 26),
        ("tooling", "lint-format", 31),
        ("tooling", "build-config", 35),
        ("stories", "stories", 5),
        ("stories", "fixtures", 9),
        ("stories", "i18n", 15),
    ];
    let got: Vec<(&str, &str, usize)> = BUILTINS
        .iter()
        .flat_map(|d| {
            d.groups
                .iter()
                .map(move |g| (d.key, g.key, g.patterns.len()))
        })
        .collect();
    assert_eq!(got, expected);
    let build_output = BUILTINS[1].groups[2].patterns;
    assert_eq!(build_output, ["dist/", "build/", "out/"]);
    let source = include_str!("../../src/categories/catalog.rs");
    assert!(source.contains("5b8ce0e470fcd2b2538da6c700d7d96197f57de3"));
    assert!(source.contains("MIT"));
}
