//! Hidden `polygloss debug …` developer commands: `parity` (T1.16),
//! `categorize` (T6.9, in `debug_categorize`; `main` runs it, since it prints
//! like the human commands) and the test-only human (T4.4, `human-archive`:
//! T4.7) and agent (T4.5, `assign`: T4.8) stand-ins in `debug_human`.
//!
//! `debug parity` checks our hunks against the system git on a real range (design
//! §6.3 "Parity"): for every text modify/rename pair that `list_changes` reports
//! between two trees, it compares `unified_text` of our `diff_blobs` with
//! `git diff -U3 --inter-hunk-context=1 --diff-algorithm=myers --indent-heuristic`
//! of the same two blobs, normalized by `strip_git_headers`. `--algorithm` only
//! changes our side, so `--algorithm histogram` measures how far our Histogram
//! strays from git's Myers. `scripts/git-parity.ts` aggregates the result and
//! enforces the parity rate.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context as _, bail};
use clap::{Args, Subcommand, ValueEnum};
use polygloss_core::git::{self, Git, RepoInfo};
use polygloss_core::objects::{BlobReader, is_binary};
use polygloss_diff::hunks::diff_blobs;
use polygloss_diff::options::{Algorithm, DiffOptions};
use polygloss_diff::unified_text::{strip_git_headers, unified_text};
use polygloss_diff::{FileChange, FileKind, FileStatus, ObjectFormat, Oid};
use serde::Serialize;

use crate::cli::GlobalArgs;
use crate::{debug_categorize, debug_human};

/// Developer tools; hidden from `--help`.
#[derive(Debug, Args)]
pub struct DebugArgs {
    #[command(subcommand)]
    pub command: DebugCommand,
}

#[derive(Debug, Subcommand)]
pub enum DebugCommand {
    /// Compare our hunks with `git diff` for every text pair between two revisions.
    Parity(ParityArgs),
    /// Explain the file category of paths (settings.json, and with --repo its
    /// HEAD's `linguist-generated`).
    Categorize(debug_categorize::CategorizeArgs),
    /// Test-only: open a review as the human would.
    Seed(debug_human::SeedArgs),
    /// Test-only: add a human draft thread or reply.
    HumanComment(debug_human::HumanCommentArgs),
    /// Test-only: mark a file viewed.
    HumanViewed(debug_human::HumanViewedArgs),
    /// Test-only: submit the review with a verdict.
    HumanSubmit(debug_human::HumanSubmitArgs),
    /// Test-only: add a published agent thread or reply.
    AgentComment(debug_human::AgentCommentArgs),
    /// Test-only: record an agent session and assign the review to it.
    Assign(debug_human::AssignArgs),
    /// Test-only: archive (or prune) the review.
    HumanArchive(debug_human::HumanArchiveArgs),
}

/// `debug parity` arguments. `--repo` (any path inside the repository, required)
/// and `--json` are the global flags.
#[derive(Debug, Args)]
pub struct ParityArgs {
    /// Base revision (anything `git rev-parse` accepts that names a tree).
    #[arg(long)]
    pub base: String,
    /// Head revision.
    #[arg(long)]
    pub head: String,
    /// Our line diff algorithm. git always runs Myers.
    #[arg(long, value_enum, default_value_t = AlgorithmArg::Myers)]
    pub algorithm: AlgorithmArg,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum AlgorithmArg {
    Myers,
    Histogram,
}

impl From<AlgorithmArg> for Algorithm {
    fn from(a: AlgorithmArg) -> Algorithm {
        match a {
            AlgorithmArg::Myers => Algorithm::Myers,
            AlgorithmArg::Histogram => Algorithm::Histogram,
        }
    }
}

/// `debug parity` output: `files` text pairs compared, `identical` of them equal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParityReport {
    pub files: usize,
    pub identical: usize,
    pub mismatches: Vec<Mismatch>,
}

/// One pair whose unified text differs, with both texts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mismatch {
    pub path: String,
    pub ours: String,
    pub git: String,
}

pub fn run(args: DebugArgs, global: &GlobalArgs) -> anyhow::Result<()> {
    match args.command {
        DebugCommand::Parity(args) => {
            let repo = required_repo(global)?;
            let report = parity(&repo, &args)?;
            if global.json {
                println!("{}", serde_json::to_string(&report)?);
            } else {
                println!("files      {}", report.files);
                println!("identical  {}", report.identical);
                for m in &report.mismatches {
                    println!("mismatch   {}", m.path);
                }
            }
            Ok(())
        }
        DebugCommand::Categorize(_) => unreachable!("main runs debug categorize"),
        DebugCommand::Seed(args) => print_json(debug_human::seed(required_repo(global)?, args)?),
        DebugCommand::HumanComment(args) => print_json(debug_human::human_comment(args)?),
        DebugCommand::HumanViewed(args) => print_json(debug_human::human_viewed(args)?),
        DebugCommand::HumanSubmit(args) => print_json(debug_human::human_submit(args)?),
        DebugCommand::AgentComment(args) => print_json(debug_human::agent_comment(
            args,
            global.agent.clone(),
            global.session.clone(),
        )?),
        DebugCommand::Assign(args) => {
            print_json(debug_human::assign(args, global.session.clone())?)
        }
        DebugCommand::HumanArchive(args) => print_json(debug_human::human_archive(args)?),
    }
}

/// The global `--repo`, which `parity` and `seed` require.
fn required_repo(global: &GlobalArgs) -> anyhow::Result<PathBuf> {
    global
        .repo
        .clone()
        .ok_or_else(|| anyhow::anyhow!("--repo <PATH> is required"))
}

fn print_json(v: serde_json::Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string(&v)?);
    Ok(())
}

/// Runs the comparison. Pairs are checked on a few worker threads; the report
/// keeps `diff-tree` order.
pub fn parity(repo_path: &Path, args: &ParityArgs) -> anyhow::Result<ParityReport> {
    let repo =
        git::discover(repo_path).with_context(|| format!("opening {}", repo_path.display()))?;
    let git = Git::new(
        repo.toplevel
            .clone()
            .unwrap_or_else(|| repo.git_dir.clone()),
    );
    let fmt = repo.object_format;
    let base = tree_of(&git, fmt, &args.base)?;
    let head = tree_of(&git, fmt, &args.head)?;
    let mut changes = git::list_changes(&git, fmt, &base, &head)?;
    // `check-attr` needs a worktree; in a bare repo the NUL rule alone decides.
    if repo.toplevel.is_some() {
        git::classify(&git, &head, &mut changes, &[])?;
    }
    let pairs: Vec<FileChange> = changes.into_iter().filter(is_text_pair).collect();
    let opts = DiffOptions {
        algorithm: args.algorithm.into(),
        ..DiffOptions::default()
    };
    let outcomes = compare_all(&repo, &git, &opts, &pairs)?;

    let mut report = ParityReport {
        files: 0,
        identical: 0,
        mismatches: Vec::new(),
    };
    for outcome in outcomes.into_iter().flatten() {
        report.files += 1;
        match outcome {
            Outcome::Identical => report.identical += 1,
            Outcome::Differs(m) => report.mismatches.push(m),
        }
    }
    Ok(report)
}

/// A modify or rename between two different text blobs (mode-only changes,
/// adds, deletes, type changes, symlinks, submodules and binaries are skipped).
fn is_text_pair(c: &FileChange) -> bool {
    matches!(c.status, FileStatus::Modified | FileStatus::Renamed)
        && c.kind == FileKind::Text
        && c.old_blob != c.new_blob
}

/// `rev^{tree}` as a full object id.
pub(crate) fn tree_of(git: &Git, fmt: ObjectFormat, rev: &str) -> anyhow::Result<Oid> {
    let spec = format!("{rev}^{{tree}}");
    let out = git
        .run(&[
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("--quiet"),
            OsStr::new("--end-of-options"),
            OsStr::new(&spec),
        ])
        .context("running git rev-parse")?;
    if !out.success() {
        bail!("revision {rev:?} does not name a tree in this repository");
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(Oid::parse(text.trim_end(), fmt)?)
}

enum Outcome {
    Identical,
    Differs(Mismatch),
}

/// Compares every pair; `None` for pairs skipped as binary by content.
fn compare_all(
    repo: &RepoInfo,
    git: &Git,
    opts: &DiffOptions,
    pairs: &[FileChange],
) -> anyhow::Result<Vec<Option<Outcome>>> {
    let reader = BlobReader::open(repo)?;
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(1, 8)
        .min(pairs.len().max(1));
    let mut results: Vec<(usize, anyhow::Result<Option<Outcome>>)> = std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(pair) = pairs.get(i) else { break };
                        done.push((i, compare_one(&reader, git, opts, pair)));
                    }
                    done
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().expect("parity worker panicked"))
            .collect()
    });
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, r)| r).collect()
}

fn compare_one(
    reader: &BlobReader,
    git: &Git,
    opts: &DiffOptions,
    pair: &FileChange,
) -> anyhow::Result<Option<Outcome>> {
    let path = pair.display_path().to_owned();
    let old = reader
        .read(&pair.old_blob)
        .with_context(|| format!("reading {path} (old)"))?;
    let new = reader
        .read(&pair.new_blob)
        .with_context(|| format!("reading {path} (new)"))?;
    if is_binary(&old) || is_binary(&new) {
        return Ok(None);
    }
    let ours = unified_text(&diff_blobs(&old, &new, opts), &old, &new);
    let theirs = git_unified(git, &pair.old_blob, &pair.new_blob)
        .with_context(|| format!("git diff for {path}"))?;
    Ok(Some(if ours == theirs {
        Outcome::Identical
    } else {
        Outcome::Differs(Mismatch {
            path,
            ours,
            git: theirs,
        })
    }))
}

/// git's hunks for two blobs with the parity flags, headers stripped. Config that
/// could still change the body (`diff.suppressBlankEmpty`) is pinned off.
fn git_unified(git: &Git, old: &Oid, new: &Oid) -> anyhow::Result<String> {
    let args = [
        "-c",
        "diff.suppressBlankEmpty=false",
        "diff",
        "-U3",
        "--inter-hunk-context=1",
        "--diff-algorithm=myers",
        "--indent-heuristic",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        old.as_str(),
        new.as_str(),
    ];
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let out = git.output(&args)?;
    Ok(strip_git_headers(&String::from_utf8_lossy(&out)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use polygloss_diff::{GeneratedAttr, GitPath, Mode};

    fn change(status: FileStatus, kind: FileKind, old: &str, new: &str) -> FileChange {
        let oid = |c: &str| Oid::parse(&c.repeat(40), ObjectFormat::Sha1).expect("oid");
        FileChange {
            idx: 0,
            status,
            old_path: Some(GitPath::from_bytes(b"a")),
            new_path: Some(GitPath::from_bytes(b"a")),
            old_mode: Some(Mode(0o100644)),
            new_mode: Some(Mode(0o100644)),
            old_blob: oid(old),
            new_blob: oid(new),
            similarity: None,
            kind,
            generated: false,
            generated_attr: GeneratedAttr::Unspecified,
        }
    }

    #[test]
    fn text_pairs_are_modified_or_renamed_text_with_different_blobs() {
        use FileKind::*;
        use FileStatus::*;
        assert!(is_text_pair(&change(Modified, Text, "1", "2")));
        assert!(is_text_pair(&change(Renamed, Text, "1", "2")));
        assert!(!is_text_pair(&change(Renamed, Text, "1", "1")));
        assert!(!is_text_pair(&change(Modified, Text, "1", "1")));
        assert!(!is_text_pair(&change(Modified, Binary, "1", "2")));
        assert!(!is_text_pair(&change(Modified, Symlink, "1", "2")));
        assert!(!is_text_pair(&change(Modified, Submodule, "1", "2")));
        assert!(!is_text_pair(&change(Added, Text, "0", "2")));
        assert!(!is_text_pair(&change(Deleted, Text, "1", "0")));
        assert!(!is_text_pair(&change(TypeChanged, Text, "1", "2")));
    }
}
