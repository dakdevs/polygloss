//! Pattern forms: slashes, stars, braces, classes, escapes (design §11.15).

use super::*;

#[test]
fn no_slash_matches_the_name_at_any_depth() {
    assert_hits(
        &["Makefile"],
        &["Makefile", "a/Makefile", "a/b/c/Makefile"],
        &["Makefile.am", "a/Makefile/x.rs", "aMakefile", "makefile"],
    );
    assert_hits(
        &["*.snap"],
        &["a.snap", "deep/nested/__snapshots__/a.snap"],
        &["a.snapshot", "a.snap/b.rs"],
    );
}

#[test]
fn trailing_slash_matches_only_a_directory_at_any_depth() {
    // OQ-45 (orchestrator decision): gitignore semantics, so a file named
    // `test` is never matched by `test/` (geld would match it).
    assert_hits(
        &["test/"],
        &["test/x.rs", "a/test/x.rs", "a/b/test/c/d.rs"],
        &[
            "bin/test",
            "scripts/test",
            "test",
            "a/tests/x.rs",
            "my-test/a.ts",
            "src/testsuite/a.ts",
        ],
    );
    // An inner slash still matches at any depth (geld's directory rule).
    assert_hits(
        &[".github/workflows/"],
        &[".github/workflows/ci.yml", "sub/.github/workflows/a/b.yml"],
        &[
            ".github/workflows",
            ".github/workflows-old/ci.yml",
            ".github/ci.yml",
        ],
    );
    // Wildcards in a directory pattern.
    assert_hits(
        &["*.Tests/"],
        &["src/App.Tests/UnitTest1.cs"],
        &["src/App.Tests", "src/AppTests/UnitTest1.cs"],
    );
}

#[test]
fn inner_and_leading_slash_anchor_at_the_root() {
    assert_hits(
        &["src/gen/*.rs"],
        &["src/gen/a.rs"],
        &["x/src/gen/a.rs", "src/gen/sub/a.rs", "src/gen/a.ts"],
    );
    assert_hits(
        &["/build/"],
        &["build/a.js", "build/x/y.js"],
        &["pkg/build/a.js", "build", "builder/a.js"],
    );
    assert_hits(&["/Makefile"], &["Makefile"], &["a/Makefile"]);
}

#[test]
fn single_star_stays_in_a_segment_double_star_spans() {
    assert_hits(&["src/*.ts"], &["src/a.ts"], &["src/sub/a.ts", "a.ts"]);
    assert_hits(
        &["src/**/*.ts"],
        &["src/a.ts", "src/x/y/a.ts"],
        &["lib/a.ts", "x/src/a.ts"],
    );
    assert_hits(
        &["docs/**"],
        &["docs/guide/index.md", "docs/a"],
        &["docs", "x/docs/a"],
    );
    assert_hits(&["a/**/b"], &["a/b", "a/x/y/b"], &["ab", "a/b/c"]);
    assert_hits(
        &["file?.ts"],
        &["file1.ts", "x/fileA.ts"],
        &["file12.ts", "file.ts"],
    );
    assert_hits(&["a?b"], &["axb"], &["a/b"]);
    // `**` inside a segment is a plain `*`, as in geld.
    assert_hits(&["a**b.rs"], &["axyb.rs"], &["a/x/b.rs"]);
}

#[test]
fn braces_nest_and_classes_match() {
    assert_hits(&["*.{js,ts}"], &["a.js", "x/a.ts"], &["a.rs", "a.{js,ts}"]);
    assert_hits(
        &["{a,b}.{x,{y,z}}"],
        &["a.x", "a.y", "a.z", "b.x", "b.y", "b.z"],
        &["a.w", "c.x", "ab.x"],
    );
    assert_hits(&["file[0-9].ts"], &["file7.ts"], &["fileA.ts", "file77.ts"]);
    assert_hits(&["file[!0-9].ts"], &["fileA.ts"], &["file7.ts"]);
    // A `{` without its `}` is a literal, as in geld.
    assert_hits(&["a{b"], &["a{b", "x/a{b"], &["ab", "a"]);
    // `\` escapes: the braces never expand.
    assert_hits(&[r"\{a,b\}"], &["{a,b}", "x/{a,b}"], &["a", "b"]);
}

#[test]
fn braces_expand_before_the_slash_rules() {
    assert_hits(
        &["{generated/,*.pb.ts}"],
        &["a/generated/x.ts", "generated/y.rs", "b/c.pb.ts"],
        &["generated", "a/generated.ts", "b/c.pb.tsx"],
    );
}

#[test]
fn more_than_64_alternatives_is_an_error() {
    let list = |n: usize| {
        let alts: Vec<String> = (0..n).map(|i| format!("f{i}")).collect();
        format!("{{{}}}.rs", alts.join(","))
    };
    let with = |pattern: &str| {
        let mut value = all_builtins_off();
        value["custom"] = json!([{ "id": "t", "name": "T", "patterns": [pattern] }]);
        Categorizer::new(&config(value), &[])
    };
    let ok = with(&list(64)).expect("64 alternatives build");
    assert!(ok.categorize("f63.rs", Unspecified, false).is_some());
    for pattern in [list(65), "{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}{a,b}".to_owned()] {
        match with(&pattern) {
            Err(CategoriesError::Pattern {
                category,
                pattern: p,
                message,
            }) => {
                assert_eq!(category, "custom:t");
                assert_eq!(p, pattern);
                assert!(message.contains("64"), "{message}");
            }
            other => panic!("{pattern}: expected a Pattern error, got {other:?}"),
        }
    }
}
