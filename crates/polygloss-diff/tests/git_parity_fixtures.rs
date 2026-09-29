//! Hunk parity with the system git on the committed fixtures (T1.6, design §6.3).
//!
//! Every case under `fixtures/parity/<case>/{old,new}` is diffed by us and by
//! `git diff --no-index -U3 --inter-hunk-context=1 --diff-algorithm=myers
//! --indent-heuristic`; the unified bodies must be identical once git's file
//! headers and hunk-header function context are stripped. Git runs under a temp
//! `HOME`, an empty `GIT_CONFIG_GLOBAL` and `GIT_CONFIG_NOSYSTEM=1` (plan "Test
//! hygiene"), set on the child only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use polygloss_diff::hunks::diff_blobs;
use polygloss_diff::options::{Algorithm, DiffOptions};
use polygloss_diff::unified_text::{strip_git_headers, unified_text};

const CASES: &[&str] = &[
    "indent-heuristic-slider",
    "crlf",
    "no-trailing-newline",
    "whitespace-only",
    "large-insert",
    "moved-block",
    "empty-to-content",
    "content-to-empty",
    "unicode",
    "blank-line-multimatch",
];

fn fixture_dir(case: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/parity")
        .join(case)
}

struct GitSandbox {
    dir: tempfile::TempDir,
}

impl GitSandbox {
    fn new() -> GitSandbox {
        let dir = tempfile::tempdir().expect("temp dir");
        fs::write(dir.path().join("gitconfig"), "").expect("empty gitconfig");
        GitSandbox { dir }
    }

    /// `git diff --no-index` of two files with the parity flags plus `extra`.
    fn diff(&self, old: &Path, new: &Path, extra: &[&str]) -> String {
        let home = self.dir.path();
        let out = Command::new("git")
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("GIT_CONFIG_GLOBAL", home.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("LC_ALL", "C")
            .current_dir(home)
            .args([
                "-c",
                "core.quotePath=false",
                "diff",
                "--no-index",
                "--no-color",
            ])
            .args([
                "--no-ext-diff",
                "--no-textconv",
                "-U3",
                "--inter-hunk-context=1",
            ])
            .args(extra)
            .arg(old)
            .arg(new)
            .output()
            .expect("run git");
        // `--no-index` exits 1 when the files differ.
        assert!(
            matches!(out.status.code(), Some(0 | 1)),
            "git diff failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        strip_git_headers(&String::from_utf8_lossy(&out.stdout))
    }
}

/// Cases whose unified text differs from git's, with both texts.
fn mismatches(opts: &DiffOptions, git_flags: &[&str]) -> Vec<(String, String, String)> {
    let git = GitSandbox::new();
    let mut out = Vec::new();
    for case in CASES {
        let dir = fixture_dir(case);
        let old = fs::read(dir.join("old")).expect("read old");
        let new = fs::read(dir.join("new")).expect("read new");
        let ours = unified_text(&diff_blobs(&old, &new, opts), &old, &new);
        let theirs = git.diff(&dir.join("old"), &dir.join("new"), git_flags);
        if ours != theirs {
            out.push((case.to_string(), ours, theirs));
        }
    }
    out
}

fn assert_parity(opts: &DiffOptions, git_flags: &[&str]) {
    let bad = mismatches(opts, git_flags);
    for (case, ours, git) in &bad {
        eprintln!("--- {case}: ours ---\n{ours}--- {case}: git ---\n{git}");
    }
    let names: Vec<&str> = bad.iter().map(|(c, _, _)| c.as_str()).collect();
    assert!(names.is_empty(), "hunks differ from git on: {names:?}");
}

#[test]
fn hunks_match_git_on_parity_fixtures() {
    assert_parity(
        &DiffOptions::default(),
        &["--diff-algorithm=myers", "--indent-heuristic"],
    );
}

#[test]
fn hunks_ignore_whitespace_matches_git_w_on_parity_fixtures() {
    let opts = DiffOptions {
        ignore_whitespace: true,
        ..DiffOptions::default()
    };
    assert_parity(
        &opts,
        &["--diff-algorithm=myers", "--indent-heuristic", "-w"],
    );
}

#[test]
fn hunks_histogram_matches_git_on_parity_fixtures() {
    let opts = DiffOptions {
        algorithm: Algorithm::Histogram,
        ..DiffOptions::default()
    };
    assert_parity(&opts, &["--diff-algorithm=histogram", "--indent-heuristic"]);
}

#[test]
fn every_parity_fixture_is_listed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/parity");
    let mut on_disk: Vec<String> = fs::read_dir(&root)
        .expect("fixtures/parity")
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    on_disk.sort();
    let mut listed: Vec<String> = CASES.iter().map(|c| c.to_string()).collect();
    listed.sort();
    assert_eq!(on_disk, listed);
}

#[test]
fn strip_git_headers_drops_file_headers_and_function_context() {
    let git = "diff --git a/old b/new\nindex 1..2 100644\n--- a/old\n+++ b/new\n\
               @@ -1,2 +1,2 @@ fn main() {\n a\n-b\n+c\n@@ -9 +9 @@\n-x\n+y\n";
    assert_eq!(
        strip_git_headers(git),
        "@@ -1,2 +1,2 @@\n a\n-b\n+c\n@@ -9 +9 @@\n-x\n+y\n"
    );
    assert_eq!(strip_git_headers(""), "");
}
