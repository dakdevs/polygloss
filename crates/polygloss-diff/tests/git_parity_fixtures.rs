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
    "kept-brace-tie-break",
    "removal-slides-to-addition",
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

/// Two files of the same numbered blocks of distinct lines (1 to 40 lines each);
/// `new` swaps `near` pairs of blocks up to 40 apart and `far` pairs anywhere.
/// Every line occurs once in each file, so git's cleanup keeps them all and Myers
/// runs past its cost limit, where its good-snake split and its cost cutoff pick
/// the alignment.
fn shuffled_blocks(seed: u64, blocks: usize, near: usize, far: usize) -> (Vec<u8>, Vec<u8>) {
    let mut state = seed;
    let mut below = |n: usize| {
        // splitmix64
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) % n as u64) as usize
    };
    const LENS: [usize; 10] = [1, 2, 3, 5, 8, 13, 21, 25, 30, 40];
    let text: Vec<String> = (0..blocks)
        .map(|b| {
            (0..LENS[below(LENS.len())])
                .map(|j| format!("blk{b}_{j} = value({b}, {j});\n"))
                .collect()
        })
        .collect();
    let mut order: Vec<usize> = (0..blocks).collect();
    for _ in 0..near {
        let i = below(blocks);
        order.swap(i, (i + 1 + below(40)).min(blocks - 1));
    }
    for _ in 0..far {
        order.swap(below(blocks), below(blocks));
    }
    let new: String = order.iter().map(|&b| text[b].as_str()).collect();
    (text.concat().into_bytes(), new.into_bytes())
}

#[test]
fn hunks_match_git_past_the_myers_cost_limit() {
    // (seed, blocks, near, far). 3,000 blocks are about 45,000 lines a side: the
    // cost limit is 512 d-steps, so both the good-snake split (past 256) and the
    // cost cutoff decide splits. 1,500 blocks keep the limit at 256, so only the
    // cutoff does. Undoing any one of `myers_core`'s three heuristic fixes fails
    // the first two cases; the last three each fail without exactly one: the edit
    // cost counting from 1, the mid-diagonal score, the forward snake check.
    let cases = [
        (1, 3000, 300, 30),
        (2, 3000, 300, 30),
        (1, 1500, 300, 30),
        (13, 3000, 200, 0),
        (1, 3000, 20, 0),
    ];
    let git = GitSandbox::new();
    let (old_path, new_path) = (git.dir.path().join("old"), git.dir.path().join("new"));
    let mut bad = Vec::new();
    for case @ (seed, blocks, near, far) in cases {
        let (old, new) = shuffled_blocks(seed, blocks, near, far);
        fs::write(&old_path, &old).expect("write old");
        fs::write(&new_path, &new).expect("write new");
        let ours = unified_text(&diff_blobs(&old, &new, &DiffOptions::default()), &old, &new);
        let theirs = git.diff(
            &old_path,
            &new_path,
            &["--diff-algorithm=myers", "--indent-heuristic"],
        );
        if ours != theirs {
            let first_difference = ours.lines().zip(theirs.lines()).position(|(a, b)| a != b);
            bad.push((case, first_difference));
        }
    }
    assert!(
        bad.is_empty(),
        "hunks differ from git on (case, first differing output line): {bad:?}"
    );
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
