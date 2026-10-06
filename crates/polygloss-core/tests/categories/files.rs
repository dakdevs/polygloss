//! Files (raw non-UTF-8 paths), the lenient settings reader and the
//! 13k-path scaling check.

use super::*;

#[test]
fn escaped_non_utf8_paths_never_panic() {
    let c = categorizer(json!({ "docs": { "enabled": true } }));
    let files = [
        file(b"caf\xe9/a.test.ts", Unspecified, false),
        file(b"\xff", Unknown, true),
        file(b"\xff\xfe/docs/x\xe9.md", Unknown, false),
        file(b"src/main.rs", Unspecified, false),
        file(b"Cargo.lock", Unknown, false),
        // Listed in v1's list by its raw bytes, bit 0: recovered Unset.
        file(b"caf\xe9/Cargo.lock", Unknown, false),
        file(b"caf\xe9/Cargo.lock", Unspecified, false),
    ];
    assert!(files[0].new_path.as_ref().unwrap().escaped);
    let got: Vec<Option<String>> = c
        .categorize_files(&files)
        .into_iter()
        .map(|id| id.map(|id| id.to_string()))
        .collect();
    // Raw bytes are matched, not the quoted form; a v1 bit on an unlisted
    // path recovers as Set, bit 0 on a listed one as Unset.
    assert_eq!(
        got,
        [
            Some("tests".to_owned()),
            Some("generated".to_owned()),
            Some("docs".to_owned()),
            None,
            None,
            None,
            Some("generated".to_owned()),
        ]
    );
    // The escaped text itself never panics either.
    for f in &files {
        let path = f.display_path();
        let _ = c.categorize(path, f.generated_attr, f.generated);
        let _ = c.is_generated(path, f.generated_attr, f.generated);
        let _ = c.explain(path, f.generated_attr, f.generated);
    }
}

#[test]
fn categories_config_in_falls_back_to_defaults_with_a_warning() {
    let defaults = CategoriesConfig::default();
    // Blank and missing: defaults, quietly.
    for text in [
        "",
        "  \n",
        "{}",
        "// nothing\n{ \"theme\": { \"mode\": \"dark\" } }",
    ] {
        let got = categories_config_in(text);
        assert_eq!(got.config, defaults, "{text:?}");
        assert!(got.legacy_generated.is_empty());
        assert_eq!(got.warning, None, "{text:?}");
    }
    // A valid section, JSONC allowed, with the legacy patterns.
    let got = categories_config_in(
        "{\n  // docs too\n  \"categories\": { \"docs\": { \"enabled\": true, }, },\n  \"diff\": { \"generated_patterns\": [\"*.out\", \"!x.out\"] },\n}",
    );
    assert!(got.config.docs.enabled);
    assert_eq!(got.legacy_generated, ["*.out", "!x.out"]);
    assert_eq!(got.warning, None);
    // Invalid: wrong type, a bad pattern, a bad id, unparseable JSON.
    for text in [
        r#"{"categories": {"tests": {"enabled": "yes"}}}"#,
        r#"{"categories": {"tests": {"patterns": ["[abc"]}}}"#,
        r#"{"categories": {"custom": [{"id": "Bad", "name": "B"}]}}"#,
        r#"{"categories": {"docs": {"enabled": true}}"#,
    ] {
        let got = categories_config_in(text);
        assert_eq!(got.config, defaults, "{text}");
        let warning = got.warning.unwrap_or_else(|| panic!("{text}: a warning"));
        assert!(warning.starts_with("settings.json: "), "{warning}");
    }
    // An invalid legacy pattern is not an invalid section.
    let got = categories_config_in(
        r#"{"categories": {"docs": {"enabled": true}}, "diff": {"generated_patterns": ["[abc"]}}"#,
    );
    assert!(got.config.docs.enabled);
    assert_eq!(got.warning, None);
}

#[test]
fn categories_config_reads_the_sandboxed_settings_file() {
    let sb = Sandbox::isolate();
    let paths = DataPaths::resolve().unwrap();
    assert!(paths.config_dir.starts_with(sb.home()));
    // Missing file: defaults, no warning.
    let got = categories_config(&paths);
    assert_eq!(got.config, CategoriesConfig::default());
    assert_eq!(got.warning, None);
    std::fs::create_dir_all(&paths.config_dir).unwrap();
    std::fs::write(
        paths.config_dir.join("settings.json"),
        r#"{"categories": {"docs": {"enabled": true}}}"#,
    )
    .unwrap();
    assert!(categories_config(&paths).config.docs.enabled);
}

#[test]
fn categorizing_13k_paths_with_every_category_on_scales_linearly() {
    // A Linux-sized tree (~13k paths) with every built-in and every group on.
    let dirs = [
        "drivers/net/ethernet/intel",
        "fs/ext4",
        "arch/arm64/kernel",
        "tools/testing/selftests/net",
        "Documentation/admin-guide",
        "include/linux",
        "sound/soc/codecs",
        "net/ipv4",
        "scripts/kconfig",
        "rust/kernel",
        "samples/bpf",
        ".github/workflows",
    ];
    let names = [
        "main.c",
        "core.h",
        "Makefile",
        "Kconfig",
        "index.rst",
        "a_test.c",
        "util.rs",
        "README",
        "x.py",
        "build.sh",
    ];
    let paths: Vec<String> = (0..13_000)
        .map(|i| {
            let dir = dirs[i % dirs.len()];
            let name = names[(i / dirs.len()) % names.len()];
            format!("{dir}/sub{}/{i}-{name}", i % 97)
        })
        .collect();
    let mut cfg = config(json!({
        "vendored": { "enabled": true }, "agents": { "enabled": true }, "docs": { "enabled": true },
        "tooling": { "enabled": true }, "stories": { "enabled": true },
        "generated": { "disabled_groups": [] },
    }));
    cfg.custom =
        serde_json::from_value(json!([{ "id": "k", "name": "Kconfig", "patterns": ["Kconfig*"] }]))
            .unwrap();
    let files: Vec<FileChange> = paths
        .iter()
        .map(|p| file(p.as_bytes(), Unspecified, false))
        .collect();

    let c = Categorizer::new(&cfg, &[]).unwrap();
    let got = c.categorize_files(&files);
    assert_eq!(got.len(), files.len());
    // Spot checks, by hand: `tools/testing/selftests/net/sub3/3-main.c` is
    // Tests (`testing/`), `Documentation/admin-guide/sub52/52-index.rst` is
    // Docs (`*.rst`), `fs/ext4/sub1/1-main.c` is nothing.
    let id = |i: usize| got[i].as_ref().map(|c| c.to_string());
    assert_eq!(id(3).as_deref(), Some("tests"));
    assert_eq!(id(52).as_deref(), Some("docs"));
    assert_eq!(id(1), None);
    // One pass per path: 10x the paths cost about 10x; 100x if each path's
    // cost grew with the number of paths.
    assert_ratio_below(
        "categorizing, 1.3k -> 13k paths",
        30.0,
        [&files[..1_300], &files[..]],
        |files| c.categorize_files(files),
    );
}

#[test]
fn categorizer_is_send_and_sync() {
    // One categorizer is shared by every review tab and background
    // re-partition (T6.14), so this must keep compiling.
    fn shared<T: Send + Sync>() {}
    shared::<Categorizer>();
}
