//! Recomputing a live or compare review in the background (design §10
//! "Debounce" and "Banner"): an unpinned snapshot of the worktree (§5.1;
//! nothing is written to the repo or the store), `diff-tree` against the
//! base, and a comparison of the blob pairs with what the tab shows. A
//! compare review resolves its two refs again (OQ-27). The result only
//! feeds the banner; nothing changes on screen until the user refreshes
//! (ADR-0009).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use polygloss_core::git::{
    CompareMode, Git, HeadSpec, RepoInfo, ReviewKind, Since, SnapshotError, Snapshotter, Source,
    list_changes, resolve,
};
use polygloss_core::ids::{DiffId, diff_id};
use polygloss_core::review::OpenedDiff;
use polygloss_diff::{FileChange, Oid};

/// What a review tab recomputes: its source, resolved where it was opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub repo: RepoInfo,
    /// Where git runs: the live worktree, else the repo's worktree (or git
    /// dir for a bare repo).
    pub worktree: PathBuf,
    pub source: Source,
    pub kind: ReviewKind,
    /// The review key shown; a live worktree now on another branch
    /// resolves to another key.
    pub review_key: String,
}

impl Target {
    /// The target of `opened`; `None` for a commit review (it never
    /// moves) or a key this build cannot read.
    pub fn of(opened: &OpenedDiff) -> Option<Target> {
        let source = source_of(opened)?;
        let worktree = match (&opened.live, &opened.repo.toplevel) {
            (Some(live), _) => live.worktree.clone(),
            (None, Some(top)) => top.clone(),
            (None, None) => opened.repo.git_dir.clone(),
        };
        Some(Target {
            repo: opened.repo.clone(),
            worktree,
            source,
            kind: opened.kind,
            review_key: opened.review_key.clone(),
        })
    }
}

/// The source `opened` was opened from, read back from its review key
/// (design §4.2): `worktree:…#since=<base>` or `compare:<base>...<head>` /
/// `compare:<base>..<head>`. `None` for commit reviews.
pub fn source_of(opened: &OpenedDiff) -> Option<Source> {
    match opened.kind {
        ReviewKind::Commit => None,
        ReviewKind::Live => Some(Source::Live {
            since: since_of(&opened.review_key)?,
        }),
        ReviewKind::Compare => {
            let spec = opened.review_key.strip_prefix("compare:")?;
            // Ref names never contain "..": the first "..." (else "..")
            // separates the two sides.
            let (base, head, mode) = match spec.split_once("...") {
                Some((b, h)) => (b, h, CompareMode::ThreeDot),
                None => {
                    let (b, h) = spec.split_once("..")?;
                    (b, h, CompareMode::Direct)
                }
            };
            Some(Source::Compare {
                base: base.to_owned(),
                head: head.to_owned(),
                mode,
            })
        }
    }
}

/// The base of a live review key (`…#since=merge-base|HEAD|<oid>`).
pub fn since_of(review_key: &str) -> Option<Since> {
    let (_, since) = review_key.rsplit_once("#since=")?;
    Some(match since {
        "merge-base" => Since::MergeBase,
        "HEAD" => Since::Head,
        oid => Since::Commit(oid.to_owned()),
    })
}

/// What the tab shows, as the recompute compares it.
#[derive(Debug, Clone)]
pub struct Shown {
    pub diff_id: DiffId,
    pub base_tree: Oid,
    pub files: Arc<Vec<FileChange>>,
}

impl Shown {
    pub fn of(opened: &OpenedDiff) -> Shown {
        Shown {
            diff_id: opened.diff_id.clone(),
            base_tree: opened.base.tree.clone(),
            files: opened.files.clone(),
        }
    }
}

/// A newer state than the one shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Newer {
    pub diff_id: DiffId,
    /// Files whose status or blobs differ from what is shown (design §10).
    pub files_changed: usize,
    /// The base tree changed (HEAD moved with `since=HEAD`, or the merge
    /// base moved).
    pub base_moved: bool,
    /// The live worktree is on another branch now (its key names it): the
    /// refresh opens that review instead.
    pub other_review: Option<String>,
}

/// The result of one recompute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing changed for this review.
    Same,
    Newer(Newer),
    /// The user's `index.lock` exists (git is writing the index): retry on
    /// the next debounce (design §5.3).
    IndexLocked,
}

/// Recomputes `target` and compares it with `shown` (background thread:
/// spawns git).
pub fn recompute(
    snapshots: &Snapshotter,
    target: &Target,
    shown: &Shown,
) -> anyhow::Result<Outcome> {
    let res = resolve(&target.repo, &target.worktree, &target.source)?;
    let fmt = res.object_format;
    let cwd = target
        .repo
        .toplevel
        .clone()
        .unwrap_or_else(|| target.repo.git_dir.clone());
    let (head_tree, git) = match &res.head {
        HeadSpec::Tree(head) => (head.tree.clone(), Git::new(cwd)),
        HeadSpec::Worktree => match snapshots.snapshot(&target.repo, &target.worktree) {
            Ok(state) => {
                let git = snapshots.diff_env(&state, Git::new(cwd));
                (state.head_tree, git)
            }
            Err(SnapshotError::IndexLocked) => return Ok(Outcome::IndexLocked),
            Err(e) => return Err(e.into()),
        },
    };
    let other_review = (res.review_key != target.review_key).then(|| res.review_key.clone());
    let id = diff_id(fmt, &res.base.tree, &head_tree);
    if id == shown.diff_id && other_review.is_none() {
        return Ok(Outcome::Same);
    }
    let files = list_changes(&git, fmt, &res.base.tree, &head_tree)?;
    Ok(Outcome::Newer(Newer {
        diff_id: id,
        files_changed: changed_files(&shown.files, &files),
        base_moved: res.base.tree != shown.base_tree,
        other_review,
    }))
}

/// Files whose status or blobs differ between `shown` and `now` (by path;
/// a file in only one of them counts).
pub fn changed_files(shown: &[FileChange], now: &[FileChange]) -> usize {
    let key = |f: &FileChange| (f.status, f.old_blob.clone(), f.new_blob.clone());
    let by_path = |files: &[FileChange]| -> HashMap<String, _> {
        files
            .iter()
            .map(|f| (f.display_path().to_owned(), key(f)))
            .collect()
    };
    let (a, b) = (by_path(shown), by_path(now));
    let changed_or_gone = a.iter().filter(|(p, k)| b.get(*p) != Some(k)).count();
    let added = b.keys().filter(|p| !a.contains_key(*p)).count();
    changed_or_gone + added
}

/// The banner text for `newer` (design §10, §11.7).
pub fn banner_text(kind: ReviewKind, newer: &Newer) -> String {
    if newer.other_review.is_some() {
        return "The worktree switched to another branch".to_owned();
    }
    match kind {
        ReviewKind::Compare => "New iteration available".to_owned(),
        _ => {
            let files = match newer.files_changed {
                1 => "1 file changed".to_owned(),
                n => format!("{n} files changed"),
            };
            if newer.base_moved {
                format!("Base moved · {files}")
            } else {
                files
            }
        }
    }
}
