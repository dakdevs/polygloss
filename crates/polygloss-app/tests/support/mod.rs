//! Helpers shared by `polygloss-app`'s test binaries (`app` and `e2e`). Each
//! includes this directory with `#[path = "../support/mod.rs"] mod support;`
//! (the only way two test binaries can share a module; the files keep their
//! snake_case names, ADR-0018).
//!
//! - [`harness`]: the `e2e` binary's libtest-compatible, main-thread runner.
//! - [`screenshot`]: the clean-room screenshot baseline runner.
//! - fixture repos for the shell, the provider and the screenshots.
//!
//! Every test starts with `let _sb = Sandbox::isolate();` (plan "Test
//! hygiene"): a temp `HOME`, data, config and cache dir and an empty global
//! git config for the whole process (nextest runs one test per process).

#![allow(dead_code)]

pub mod harness;
pub mod motion;
pub mod screenshot;

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{Core, OpenRequest, OpenedDiff};
use polygloss_core::store::events::Actor;
pub use polygloss_core::testing::{FixtureRepo, Sandbox};

/// Points `HOME` at the directory holding `repo` (the fixture's own temp
/// dir, never the real home), so the toolbar's repo block reads `~` under
/// the repo's name and screenshots never show a temp path. Call it after
/// `Sandbox::isolate()`, before the app starts.
pub fn home_above(repo: &Path) {
    let root = repo.parent().expect("a fixture repo has a parent dir");
    // SAFETY: one test per process, before the app starts any thread.
    unsafe { std::env::set_var("HOME", root) };
}
use polygloss_diff::ObjectFormat;

/// `src/config.rs` at `base`.
pub const CONFIG_RS_BASE: &str = r#"use std::collections::HashMap;

/// A parsed `key = value` config file.
pub struct Config {
    entries: HashMap<String, String>,
}

impl Config {
    pub fn parse(text: &str) -> Config {
        let mut entries = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                entries.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        Config { entries }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|v| v.as_str())
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}
"#;

/// `src/config.rs` at `head`: `HashMap` → `BTreeMap` and a new `is_empty`,
/// with the middle of `parse` unchanged (a gap row between two hunks).
pub const CONFIG_RS_HEAD: &str = r#"use std::collections::BTreeMap;

/// A parsed `key = value` config file, sorted by key.
pub struct Config {
    entries: BTreeMap<String, String>,
}

impl Config {
    pub fn parse(text: &str) -> Config {
        let mut entries = BTreeMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                entries.insert(key.trim().to_string(), value.trim().to_string());
            }
        }
        Config { entries }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}
"#;

const GREET_TS_BASE: &str = r#"import { readFileSync } from "node:fs";

export function loadGreeting(path: string): string {
  const text = readFileSync(path, "utf8");
  return text.trim();
}

export function greet(name: string): string {
  return "Hello, " + name + "!";
}
"#;

const GREET_TS_HEAD: &str = r#"import { readFileSync } from "node:fs";

export function loadGreeting(path: string, fallback = "Hello"): string {
  try {
    return readFileSync(path, "utf8").trim();
  } catch {
    return fallback;
  }
}

export function greet(name: string, greeting = "Hello"): string {
  return `${greeting}, ${name}!`;
}
"#;

const MAIN_RS_HEAD: &str = r#"mod config;

fn main() {
    let text = std::fs::read_to_string("app.conf").unwrap_or_default();
    let config = config::Config::parse(&text);
    if config.is_empty() {
        eprintln!("app.conf has no settings");
    }
    println!("{} settings", config.len());
}
"#;

/// A repo with tags `base` and `head` (head is base's only child): an
/// ordinary code change in diff order `src/config.rs` (modified Rust),
/// `src/greet.ts` (modified TypeScript) and `src/main.rs` (added).
pub fn code_change_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/config.rs", CONFIG_RS_BASE.as_bytes());
    repo.write("src/greet.ts", GREET_TS_BASE.as_bytes());
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("src/config.rs", CONFIG_RS_HEAD.as_bytes());
    repo.write("src/greet.ts", GREET_TS_HEAD.as_bytes());
    repo.write("src/main.rs", MAIN_RS_HEAD.as_bytes());
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

/// Submodule commits in [`special_files_repo`] (short ids `a1b2c3d` → `d4e5f60`).
pub const SUBMODULE_BASE: &str = "a1b2c3d4e5f60718293a4b5c6d7e8f9012345678";
pub const SUBMODULE_HEAD: &str = "d4e5f6071829a3b4c5d6e7f8091a2b3c4d5e6f70";

/// A repo with tags `base` and `head` holding one of each special change
/// (design §6.4), in diff order: `Cargo.lock` (generated), `assets/logo.png`
/// (binary by `.gitattributes`), `current` (symlink `v1` → `v2`), `docs/guide.md` (pure rename
/// from `docs/old-guide.md`), `legacy.txt` (deleted), `scripts/build.sh`
/// (mode 100644 → 100755 only) and `vendor/lib` (submodule).
pub fn special_files_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    // Unchanged, so not in the diff; `binary` makes the PNG a binary change
    // from attributes alone.
    repo.write(".gitattributes", b"*.png binary\n");
    repo.write(
        "Cargo.lock",
        b"version = 3\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n",
    );
    repo.write(
        "assets/logo.png",
        b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x10\0\0\0\x10base",
    );
    std::os::unix::fs::symlink("v1", repo.path().join("current")).expect("create symlink");
    repo.write(
        "docs/old-guide.md",
        b"# Guide\n\nRun `scripts/build.sh`, then open `current/index.html`.\n",
    );
    repo.write(
        "legacy.txt",
        b"This file is no longer used.\nDelete it after the release.\n",
    );
    repo.write("scripts/build.sh", b"#!/bin/sh\nset -eu\nmake all\n");
    set_gitlink(&repo, "vendor/lib", SUBMODULE_BASE);
    repo.commit("base");
    repo.git(&["tag", "base"]);

    repo.write(
        "Cargo.lock",
        b"version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.2.0\"\n",
    );
    repo.write(
        "assets/logo.png",
        b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x20\0\0\0\x20head",
    );
    std::fs::remove_file(repo.path().join("current")).expect("remove symlink");
    std::os::unix::fs::symlink("v2", repo.path().join("current")).expect("create symlink");
    std::fs::rename(
        repo.path().join("docs/old-guide.md"),
        repo.path().join("docs/guide.md"),
    )
    .expect("rename guide");
    std::fs::remove_file(repo.path().join("legacy.txt")).expect("remove legacy.txt");
    std::fs::set_permissions(
        repo.path().join("scripts/build.sh"),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("chmod build.sh");
    set_gitlink(&repo, "vendor/lib", SUBMODULE_HEAD);
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

/// Stages a submodule entry (a gitlink to `commit`) at `path`. The empty
/// directory makes it an unpopulated submodule, which `git add -A` keeps.
fn set_gitlink(repo: &FixtureRepo, path: &str, commit: &str) {
    std::fs::create_dir_all(repo.path().join(path)).expect("create submodule dir");
    repo.git(&[
        "update-index",
        "--add",
        "--cacheinfo",
        &format!("160000,{commit},{path}"),
    ]);
}

/// Opens `base..head` of `repo` (direct compare) through a [`Core`] on the
/// sandbox's data dir.
pub fn open_compare(repo: &Path) -> OpenedDiff {
    open(
        repo,
        Source::Compare {
            base: "base".into(),
            head: "head".into(),
            mode: CompareMode::Direct,
        },
    )
}

/// Opens `source` in `repo` through a [`Core`] on the sandbox's data dir.
pub fn open(repo: &Path, source: Source) -> OpenedDiff {
    let core = Core::open_default().expect("open the store in the sandbox");
    core.open(&OpenRequest {
        worktree: repo.to_path_buf(),
        source,
        label: None,
        pin: None,
        actor: Actor::human(),
    })
    .expect("open the diff")
}

/// `args` as owned strings (paths through `to_string_lossy`).
pub fn strings<I, S>(args: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<std::ffi::OsStr>,
{
    args.into_iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect()
}
