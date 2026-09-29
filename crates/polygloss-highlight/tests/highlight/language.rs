use polygloss_highlight::{Language, guess_language};

fn id(path: &str, head: &[u8]) -> Option<&'static str> {
    guess_language(path, head).map(|l| l.id())
}

#[test]
fn guess_language_by_extension_and_shebang() {
    // Extension or well-known file name.
    assert_eq!(id("src/main.rs", b""), Some("rust"));
    assert_eq!(id("SRC/MAIN.RS", b""), Some("rust"));
    assert_eq!(id("web/app.ts", b""), Some("typescript"));
    assert_eq!(id("web/App.tsx", b""), Some("tsx"));
    assert_eq!(id("web/index.js", b""), Some("javascript"));
    assert_eq!(id("cmd/tool/main.go", b""), Some("go"));
    assert_eq!(id("docs/README.md", b""), Some("markdown"));
    assert_eq!(id("config/settings.yaml", b""), Some("yaml"));
    assert_eq!(id("Cargo.toml", b""), Some("toml"));
    assert_eq!(id("Makefile", b""), Some("make"));
    assert_eq!(id("docker/Dockerfile", b""), Some("dockerfile"));
    // Shebang when the name says nothing.
    assert_eq!(
        id("bin/tool", b"#!/usr/bin/env python3\nprint(1)\n"),
        Some("python")
    );
    assert_eq!(id("scripts/run", b"#!/bin/bash\necho hi\n"), Some("bash"));
    // The extension wins over the first line.
    assert_eq!(id("src/lib.rs", b"#![forbid(unsafe_code)]\n"), Some("rust"));
    assert_eq!(
        id("tool.py", b"#!/bin/bash\n"),
        Some("python"),
        "extension before shebang"
    );
    // Nothing to go on: plain text is None, and directory names never count.
    assert_eq!(id("notes.xyz", b"hello"), None);
    assert_eq!(id("COPYING", b"GNU General Public License\n"), None);
    // lumis files these under Markdown on purpose.
    assert_eq!(id("LICENSE", b"MIT License\n"), Some("markdown"));
    assert_eq!(id("rust/NOTES", b"plain words\n"), None);
    // A dotless file name is not an extension (lumis maps `*.tool` to Bash).
    assert_eq!(id("bin/tool", b"echo hi\n"), None);
    assert_eq!(id("rs", b""), None);
    assert_eq!(id("data.bin", &[0, 159, 146, 150, 0xff]), None);
    assert_eq!(id("", b""), None);
}

#[test]
fn language_from_name_and_ids() {
    let rust = Language::from_name("rust").unwrap();
    assert_eq!(rust.id(), "rust");
    assert_eq!(rust.name(), "Rust");
    assert_eq!(
        Language::from_name("typescript").unwrap().id(),
        "typescript"
    );
    assert_eq!(guess_language("a.rs", b""), Some(rust));
    assert_eq!(Language::from_name("plaintext"), None);
    assert_eq!(Language::from_name("no-such-language"), None);
}
