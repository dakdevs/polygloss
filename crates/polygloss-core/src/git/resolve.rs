//! Source resolution to base and head trees, default branch and review keys (T1.2, design §3).
//!
//! Every source becomes `{object_format, base, head}` before anything else runs
//! (design §3). Revisions go through `rev-parse --verify --end-of-options`, so an
//! input starting with `-` is never an option. Commit objects are read raw
//! (`cat-file commit`), so a shallow boundary or a missing parent is reported as
//! `ObjectsMissing` instead of being mistaken for a root commit. Nothing here can
//! fetch (see `runner`).

use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};

use polygloss_diff::{ObjectFormat, Oid};
use serde::{Deserialize, Serialize};

use crate::git::repo::{RepoInfo, path_output};
use crate::git::runner::{Git, GitError};
use crate::ids::review_key;

/// The base of a live review (design §3, §4.2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Since {
    /// `merge-base(HEAD, <default branch>)`, the default (ADR-0008).
    #[default]
    MergeBase,
    Head,
    /// A fixed revision; keyed by its commit OID.
    Commit(String),
}

/// Three-dot (merge-base) or direct (two-dot) compare.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareMode {
    #[default]
    ThreeDot,
    Direct,
}

/// What the user or agent asked to review (design §3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Live {
        since: Since,
    },
    Commit {
        rev: String,
    },
    Compare {
        base: String,
        head: String,
        mode: CompareMode,
    },
}

/// One resolved side. `commit` is the commit whose tree this is (`None` for the
/// empty tree); `ref_name` is the full ref name of the requested input when it was
/// a ref (provenance only, never part of `diff_id`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ResolvedSide {
    pub tree: Oid,
    pub commit: Option<Oid>,
    pub ref_name: Option<String>,
}

/// The head of a diff: a tree, or the working tree (snapshot taken by T1.5).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadSpec {
    Tree(ResolvedSide),
    Worktree,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewKind {
    Live,
    Compare,
    Commit,
}

impl ReviewKind {
    /// `live` | `compare` | `commit` (the `reviews.kind` column, design §7.2).
    pub fn as_str(&self) -> &'static str {
        match self {
            ReviewKind::Live => "live",
            ReviewKind::Compare => "compare",
            ReviewKind::Commit => "commit",
        }
    }
}

/// Notices shown with a resolved diff (design §3 fallbacks).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ResolveWarning {
    /// Live review in a repo without commits: the base is the empty tree (OQ-P5).
    UnbornHead,
    /// Live `since=merge-base` found no default branch (OQ-5); the base is HEAD.
    NoDefaultBranch,
    /// Live `since=merge-base`: HEAD shares no history with the default branch
    /// (OQ-P5); the base is HEAD.
    NoMergeBase { default_branch: String },
    /// Live `since=merge-base` in a shallow clone: no merge base within the fetched
    /// history (the real one is most likely beyond the shallow boundary); the base
    /// is HEAD.
    MergeBaseBeyondShallow { default_branch: String },
    /// Several merge bases; git's first one is used (design §3, provisional).
    MultipleMergeBases { count: usize, used: Oid },
}

impl fmt::Display for ResolveWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolveWarning::UnbornHead => {
                f.write_str("HEAD has no commits yet; showing every file against the empty tree")
            }
            ResolveWarning::NoDefaultBranch => f.write_str(
                "no default branch found (origin/HEAD, origin/main, origin/master, main, master); showing changes since HEAD",
            ),
            ResolveWarning::NoMergeBase { default_branch } => write!(
                f,
                "HEAD shares no history with {default_branch}; showing changes since HEAD"
            ),
            ResolveWarning::MergeBaseBeyondShallow { default_branch } => write!(
                f,
                "no merge base with {default_branch} within this shallow clone's history; showing changes since HEAD (fetch more history to diff against the merge base)"
            ),
            ResolveWarning::MultipleMergeBases { count, used } => {
                write!(f, "{count} merge bases found; using {}", used.short())
            }
        }
    }
}

/// A source resolved to trees, with its review key (design §4.2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub object_format: ObjectFormat,
    pub base: ResolvedSide,
    pub head: HeadSpec,
    pub kind: ReviewKind,
    pub review_key: String,
    pub warnings: Vec<ResolveWarning>,
}

#[derive(Debug, thiserror::Error)]
pub enum ResolveError {
    #[error("unknown revision {0:?}")]
    BadRevision(String),
    #[error(
        "the two revisions have no common ancestor; use {suggestion} to compare their trees directly"
    )]
    NoMergeBase { suggestion: &'static str },
    #[error(
        "objects missing: {0}. Polygloss never fetches; fetch or unshallow the repository yourself"
    )]
    ObjectsMissing(String),
    #[error(transparent)]
    Git(#[from] GitError),
}

/// Resolves `source` in `repo`, running git in `worktree` (any worktree of the repo;
/// for live sources, the worktree under review, possibly a subdirectory of it).
pub fn resolve(
    repo: &RepoInfo,
    worktree: &Path,
    source: &Source,
) -> Result<Resolution, ResolveError> {
    let cx = Cx {
        git: Git::new(worktree),
        fmt: repo.object_format,
        shallow: repo.common_dir.join("shallow").exists(),
    };
    let resolution = match source {
        Source::Commit { rev } => cx.resolve_commit(rev)?,
        Source::Compare { base, head, mode } => cx.resolve_compare(base, head, *mode)?,
        Source::Live { since } => cx.resolve_live(since)?,
    };
    let mut trees = vec![&resolution.base.tree];
    if let HeadSpec::Tree(head) = &resolution.head {
        trees.push(&head.tree);
    }
    cx.ensure_present(&trees)?;
    Ok(resolution)
}

/// The default branch, offline (OQ-5): `refs/remotes/origin/HEAD`, then
/// `origin/main`, `origin/master`, `main`, `master`. Returns the full ref name and
/// whether a fallback was used; `GitError::NoDefaultBranch` when none exists.
pub fn default_branch(git: &Git) -> Result<(String, bool), GitError> {
    let out = git.run(&args(&["symbolic-ref", "-q", "refs/remotes/origin/HEAD"]))?;
    match out.code {
        Some(0) => {
            let target = String::from_utf8_lossy(&out.stdout).trim_end().to_owned();
            if is_commit(git, &target)? {
                return Ok((target, false));
            }
        }
        // Exit 1: unset (or not a symbolic ref); fall back.
        Some(1) => {}
        _ => {
            return Err(GitError::Failed {
                args: "symbolic-ref -q refs/remotes/origin/HEAD".into(),
                code: out.code,
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
    }
    for candidate in [
        "refs/remotes/origin/main",
        "refs/remotes/origin/master",
        "refs/heads/main",
        "refs/heads/master",
    ] {
        if is_commit(git, candidate)? {
            return Ok((candidate.to_owned(), true));
        }
    }
    Err(GitError::NoDefaultBranch)
}

fn is_commit(git: &Git, full_ref: &str) -> Result<bool, GitError> {
    let peeled = format!("{full_ref}^{{commit}}");
    Ok(git.status(&args(&[
        "rev-parse",
        "--verify",
        "--quiet",
        "--end-of-options",
        &peeled,
    ]))? == 0)
}

fn args<'a>(a: &'a [&'a str]) -> Vec<&'a OsStr> {
    a.iter().map(OsStr::new).collect()
}

/// A raw commit object's tree and parents.
struct CommitInfo {
    tree: Oid,
    parents: Vec<Oid>,
}

enum MergeBase {
    None,
    Found { first: Oid, count: usize },
}

struct Cx {
    git: Git,
    fmt: ObjectFormat,
    shallow: bool,
}

impl Cx {
    fn resolve_commit(&self, rev: &str) -> Result<Resolution, ResolveError> {
        let commit = self.commit(rev)?;
        let info = self.read_commit(&commit, None)?;
        let base = match info.parents.first() {
            None => self.empty_side(),
            Some(parent) => ResolvedSide {
                tree: self.read_commit(parent, Some(&commit))?.tree,
                commit: Some(parent.clone()),
                ref_name: None,
            },
        };
        Ok(Resolution {
            object_format: self.fmt,
            base,
            head: HeadSpec::Tree(ResolvedSide {
                tree: info.tree,
                commit: Some(commit.clone()),
                ref_name: self.full_ref(rev)?,
            }),
            kind: ReviewKind::Commit,
            review_key: review_key::commit(&commit),
            warnings: Vec::new(),
        })
    }

    fn resolve_compare(
        &self,
        base: &str,
        head: &str,
        mode: CompareMode,
    ) -> Result<Resolution, ResolveError> {
        let base_commit = self.commit(base)?;
        let head_commit = self.commit(head)?;
        let base_ref = self.full_ref(base)?;
        let head_ref = self.full_ref(head)?;
        let mut warnings = Vec::new();
        let base_side_commit = match mode {
            CompareMode::Direct => base_commit.clone(),
            CompareMode::ThreeDot => match self.merge_base(&base_commit, &head_commit)? {
                MergeBase::Found { first, count } => {
                    if count > 1 {
                        warnings.push(ResolveWarning::MultipleMergeBases {
                            count,
                            used: first.clone(),
                        });
                    }
                    first
                }
                MergeBase::None if self.shallow => {
                    return Err(ResolveError::ObjectsMissing(format!(
                        "no merge base of {base} and {head} within this shallow clone's history"
                    )));
                }
                MergeBase::None => {
                    return Err(ResolveError::NoMergeBase {
                        suggestion: "--direct",
                    });
                }
            },
        };
        let key = review_key::compare(
            base_ref.as_deref().unwrap_or(base_commit.as_str()),
            head_ref.as_deref().unwrap_or(head_commit.as_str()),
            mode == CompareMode::Direct,
        );
        Ok(Resolution {
            object_format: self.fmt,
            base: ResolvedSide {
                tree: self.read_commit(&base_side_commit, None)?.tree,
                commit: Some(base_side_commit),
                ref_name: base_ref,
            },
            head: HeadSpec::Tree(ResolvedSide {
                tree: self.read_commit(&head_commit, None)?.tree,
                commit: Some(head_commit),
                ref_name: head_ref,
            }),
            kind: ReviewKind::Compare,
            review_key: key,
            warnings,
        })
    }

    fn resolve_live(&self, since: &Since) -> Result<Resolution, ResolveError> {
        let toplevel = self.toplevel()?;
        let branch_ref = self.head_branch()?;
        let branch = branch_ref
            .as_deref()
            .map(|r| r.strip_prefix("refs/heads/").unwrap_or(r));
        let head = self.head_commit()?;
        let mut warnings = Vec::new();

        let head_side = |commit: Oid| -> Result<ResolvedSide, ResolveError> {
            Ok(ResolvedSide {
                tree: self.read_commit(&commit, None)?.tree,
                commit: Some(commit),
                ref_name: branch_ref.clone(),
            })
        };

        let (base, since_key) = match since {
            Since::Commit(rev) => {
                let commit = self.commit(rev)?;
                let side = ResolvedSide {
                    tree: self.read_commit(&commit, None)?.tree,
                    commit: Some(commit.clone()),
                    ref_name: self.full_ref(rev)?,
                };
                (side, commit.to_string())
            }
            Since::Head => {
                let side = match head {
                    Some(commit) => head_side(commit)?,
                    None => {
                        warnings.push(ResolveWarning::UnbornHead);
                        self.empty_side()
                    }
                };
                (side, "HEAD".to_owned())
            }
            Since::MergeBase => {
                let side = match head {
                    None => {
                        warnings.push(ResolveWarning::UnbornHead);
                        self.empty_side()
                    }
                    Some(head) => self.live_merge_base(head, &head_side, &mut warnings)?,
                };
                // The key keeps the requested base even when a fallback applied, so
                // the review stays the same once the default branch appears.
                (side, "merge-base".to_owned())
            }
        };

        Ok(Resolution {
            object_format: self.fmt,
            base,
            head: HeadSpec::Worktree,
            kind: ReviewKind::Live,
            review_key: review_key::live(&toplevel, branch, &since_key),
            warnings,
        })
    }

    fn live_merge_base(
        &self,
        head: Oid,
        head_side: &dyn Fn(Oid) -> Result<ResolvedSide, ResolveError>,
        warnings: &mut Vec<ResolveWarning>,
    ) -> Result<ResolvedSide, ResolveError> {
        let default = match default_branch(&self.git) {
            Ok((name, _fallback_used)) => name,
            Err(GitError::NoDefaultBranch) => {
                warnings.push(ResolveWarning::NoDefaultBranch);
                return head_side(head);
            }
            Err(e) => return Err(e.into()),
        };
        let default_commit = self.commit(&default)?;
        match self.merge_base(&head, &default_commit)? {
            MergeBase::Found { first, count } => {
                if count > 1 {
                    warnings.push(ResolveWarning::MultipleMergeBases {
                        count,
                        used: first.clone(),
                    });
                }
                Ok(ResolvedSide {
                    tree: self.read_commit(&first, None)?.tree,
                    commit: Some(first),
                    ref_name: Some(default),
                })
            }
            // In a shallow clone the likelier cause is history beyond the boundary.
            MergeBase::None if self.shallow => {
                warnings.push(ResolveWarning::MergeBaseBeyondShallow {
                    default_branch: default,
                });
                head_side(head)
            }
            MergeBase::None => {
                warnings.push(ResolveWarning::NoMergeBase {
                    default_branch: default,
                });
                head_side(head)
            }
        }
    }

    fn empty_side(&self) -> ResolvedSide {
        ResolvedSide {
            tree: self.fmt.empty_tree(),
            commit: None,
            ref_name: None,
        }
    }

    /// `rev-parse --verify --end-of-options <rev>^{commit}`; a failure to resolve is
    /// `BadRevision`, or `ObjectsMissing` for an absent full id in a shallow or
    /// partial clone (`unresolved`).
    fn commit(&self, rev: &str) -> Result<Oid, ResolveError> {
        let peeled = format!("{rev}^{{commit}}");
        let out = self.git.run(&args(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &peeled,
        ]))?;
        if !out.success() {
            return Err(self.unresolved(rev)?);
        }
        self.parse_oid(&out.stdout)
    }

    /// Why `rev` did not resolve to a commit. In a shallow or partial clone, a
    /// full-length object id whose object is absent is missing history
    /// (`ObjectsMissing`, RF2); anything else is `BadRevision`.
    fn unresolved(&self, rev: &str) -> Result<ResolveError, ResolveError> {
        let bad = || Ok(ResolveError::BadRevision(rev.to_owned()));
        let full_hex =
            rev.len() == self.fmt.hex_len() && rev.bytes().all(|b| b.is_ascii_hexdigit());
        if !full_hex {
            return bad();
        }
        let clone = if self.shallow {
            "shallow clone"
        } else if self.is_partial()? {
            "partial clone"
        } else {
            return bad();
        };
        // `cat-file -e` never fetches here (see `runner`).
        if self.git.status(&args(&["cat-file", "-e", rev]))? == 0 {
            // Present but not a commit (a blob or tree id).
            return bad();
        }
        Ok(ResolveError::ObjectsMissing(format!(
            "commit {} is not in this {clone}",
            rev.to_ascii_lowercase()
        )))
    }

    /// Whether the repo is a partial clone: it has a promisor remote
    /// (`remote.<name>.promisor`, what current git writes) or the older
    /// `extensions.partialClone`.
    fn is_partial(&self) -> Result<bool, ResolveError> {
        let out = self.git.run(&args(&[
            "config",
            "--get-regexp",
            r"^remote\..*\.promisor$|^extensions\.partialclone$",
        ]))?;
        if !out.success() {
            // Exit 1: no such key.
            return Ok(false);
        }
        Ok(String::from_utf8_lossy(&out.stdout).lines().any(|line| {
            match line.split_once(' ') {
                Some(("extensions.partialclone", _)) => true,
                Some((_, value)) => matches!(
                    value.to_ascii_lowercase().as_str(),
                    "true" | "yes" | "on" | "1"
                ),
                // A bare `promisor` key is boolean true.
                None => true,
            }
        }))
    }

    /// The full ref name `rev` denotes (`refs/heads/…`, `refs/remotes/…`,
    /// `refs/tags/…`), or `None` for OIDs, expressions, a detached `HEAD` and
    /// ambiguous names.
    fn full_ref(&self, rev: &str) -> Result<Option<String>, ResolveError> {
        let out = self.git.run(&args(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "--symbolic-full-name",
            "--end-of-options",
            rev,
        ]))?;
        let name = String::from_utf8_lossy(&out.stdout).trim_end().to_owned();
        Ok((out.success() && name.starts_with("refs/")).then_some(name))
    }

    /// Reads a raw commit object (`cat-file commit`, no parent rewriting by
    /// shallow grafts). A missing object is `ObjectsMissing`.
    fn read_commit(&self, commit: &Oid, child: Option<&Oid>) -> Result<CommitInfo, ResolveError> {
        let out = self
            .git
            .run(&args(&["cat-file", "commit", commit.as_str()]))?;
        if !out.success() {
            let what = match child {
                Some(child) => format!("commit {commit} (first parent of {child})"),
                None => format!("commit {commit}"),
            };
            let why = if self.shallow {
                "is beyond this shallow clone's history"
            } else {
                "is not in this repository"
            };
            return Err(ResolveError::ObjectsMissing(format!("{what} {why}")));
        }
        let mut tree = None;
        let mut parents = Vec::new();
        for line in out.stdout.split(|&b| b == b'\n') {
            if line.is_empty() {
                break;
            }
            if let Some(t) = line.strip_prefix(b"tree ") {
                tree = Some(self.parse_oid(t)?);
            } else if let Some(p) = line.strip_prefix(b"parent ") {
                parents.push(self.parse_oid(p)?);
            }
        }
        let tree =
            tree.ok_or_else(|| GitError::Parse(format!("commit {commit} has no tree header")))?;
        Ok(CommitInfo { tree, parents })
    }

    /// `merge-base --all`: git's first result plus the total count; exit 1 = none.
    fn merge_base(&self, a: &Oid, b: &Oid) -> Result<MergeBase, ResolveError> {
        let out = self
            .git
            .run(&args(&["merge-base", "--all", a.as_str(), b.as_str()]))?;
        match out.code {
            Some(0) => {
                let text = String::from_utf8_lossy(&out.stdout);
                let bases: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
                let first = bases
                    .first()
                    .ok_or_else(|| GitError::Parse("merge-base printed nothing".into()))?;
                Ok(MergeBase::Found {
                    first: self.parse_oid(first.as_bytes())?,
                    count: bases.len(),
                })
            }
            Some(1) if out.stdout.is_empty() => Ok(MergeBase::None),
            _ => Err(GitError::Failed {
                args: format!("merge-base --all {a} {b}"),
                code: out.code,
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }
            .into()),
        }
    }

    /// Checks that the trees are in the object store (`cat-file --batch-check`;
    /// never fetches). The empty tree is built into git.
    fn ensure_present(&self, trees: &[&Oid]) -> Result<(), ResolveError> {
        let empty = self.fmt.empty_tree();
        let mut input = String::new();
        for tree in trees.iter().filter(|t| ***t != empty) {
            input.push_str(tree.as_str());
            input.push('\n');
        }
        if input.is_empty() {
            return Ok(());
        }
        let out = self.git.output_stdin(
            &args(&["cat-file", "--batch-check=%(objectname) %(objecttype)"]),
            input.as_bytes(),
        )?;
        for line in String::from_utf8_lossy(&out).lines() {
            match line.split_once(' ') {
                Some((_, "tree")) => {}
                Some((oid, "missing")) => {
                    return Err(ResolveError::ObjectsMissing(format!("tree {oid}")));
                }
                _ => {
                    return Err(GitError::Parse(format!("cat-file --batch-check: {line:?}")).into());
                }
            }
        }
        Ok(())
    }

    /// The canonical root of the worktree `self.git` runs in.
    fn toplevel(&self) -> Result<PathBuf, ResolveError> {
        let out = self.git.output(&args(&["rev-parse", "--show-toplevel"]))?;
        Ok(std::fs::canonicalize(path_output(&out)?).map_err(GitError::Io)?)
    }

    /// `refs/heads/<branch>` HEAD points at (possibly unborn), or `None` when detached.
    fn head_branch(&self) -> Result<Option<String>, ResolveError> {
        let out = self.git.run(&args(&["symbolic-ref", "-q", "HEAD"]))?;
        match out.code {
            Some(0) => Ok(Some(
                String::from_utf8_lossy(&out.stdout).trim_end().to_owned(),
            )),
            Some(1) => Ok(None),
            _ => Err(GitError::Failed {
                args: "symbolic-ref -q HEAD".into(),
                code: out.code,
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            }
            .into()),
        }
    }

    /// HEAD's commit, `None` on an unborn branch.
    fn head_commit(&self) -> Result<Option<Oid>, ResolveError> {
        let out = self.git.run(&args(&[
            "rev-parse",
            "--verify",
            "--quiet",
            "HEAD^{commit}",
        ]))?;
        if out.success() {
            Ok(Some(self.parse_oid(&out.stdout)?))
        } else {
            Ok(None)
        }
    }

    fn parse_oid(&self, bytes: &[u8]) -> Result<Oid, ResolveError> {
        let text = String::from_utf8_lossy(bytes);
        let text = text.trim();
        Oid::parse(text, self.fmt)
            .map_err(|e| GitError::Parse(format!("bad object id {text:?}: {e}")).into())
    }
}
