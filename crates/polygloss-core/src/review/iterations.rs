//! A review's iterations and "Changes since last review" (T3.12, design §11.4,
//! OQ-9).
//!
//! - [`Core::iteration_entries`]: every iteration with its trees and why it
//!   was pinned, for the app's iteration picker.
//! - [`Core::open_iteration`]: an earlier iteration as an [`OpenedDiff`], read
//!   from the store (`iterations`, `diffs`, `file_changes`); nothing is
//!   resolved or snapshotted again.
//! - [`Core::last_submission`] and [`Core::open_changes_since`]: the diff from
//!   the head tree of the iteration the last submission was made against to
//!   the head the review shows now. Both trees are durable (a submission pins
//!   a live state first, design §5.2, and the current head must be pinned or a
//!   commit), so this is a pinned diff: its `diffs` and `file_changes` rows are
//!   stored like an iteration's, and threads can be created on it (new side
//!   only, OQ-9: the app blocks old-side comments). A rebased base shows up as
//!   noise until range-diff (post-v1).

use polygloss_diff::{ObjectFormat, Oid};
use rusqlite::{Connection, OptionalExtension, Row};

use crate::git::ResolvedSide;
use crate::ids::{DiffId, diff_id};
use crate::review::models::{IterationInfo, OpenedDiff, PinnedBy, load_files, store_diff};
use crate::review::open::review_exists;
use crate::review::submit::Verdict;
use crate::review::{Core, CoreError};
use crate::store::StoreError;
use crate::store::events::now_ms;

/// One iteration with what the picker shows about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IterationEntry {
    pub info: IterationInfo,
    pub pinned_by: PinnedBy,
    /// `iterations.created_at`, Unix ms.
    pub created_at: i64,
    pub base_tree: Oid,
    pub head_tree: Oid,
    /// Provenance: the base commit and ref it was resolved from.
    pub base_commit: Option<Oid>,
    pub base_ref: Option<String>,
    /// The head commit (`None` for a pinned live state).
    pub head_commit: Option<Oid>,
}

/// The latest submission of a review and the iteration it was made against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastSubmission {
    pub submission_id: String,
    pub verdict: Verdict,
    /// `submitted_at`, Unix ms.
    pub submitted_at: i64,
    pub iteration: IterationEntry,
}

const ENTRY_COLUMNS: &str = "i.id, i.seq, i.diff_id, i.snapshot_ref, i.pinned_by, i.created_at, \
     d.base_tree, d.head_tree, i.base_commit, i.base_ref, i.head_commit";

/// Reads [`ENTRY_COLUMNS`] starting at column `at`.
fn read_entry(r: &Row, at: usize, fmt: ObjectFormat) -> Result<IterationEntry, StoreError> {
    let bad =
        |what: &str, value: &str| StoreError::Integrity(format!("iteration {what} {value:?}"));
    let oid = |i: usize| -> Result<Oid, StoreError> {
        let s: String = r.get(at + i)?;
        Oid::parse(&s, fmt).map_err(|_| bad("tree", &s))
    };
    let opt_oid = |i: usize| -> Result<Option<Oid>, StoreError> {
        r.get::<_, Option<String>>(at + i)?
            .map(|s| Oid::parse(&s, fmt).map_err(|_| bad("commit", &s)))
            .transpose()
    };
    let diff: String = r.get(at + 2)?;
    let pinned_by: String = r.get(at + 4)?;
    Ok(IterationEntry {
        info: IterationInfo {
            id: r.get(at)?,
            seq: r.get(at + 1)?,
            diff_id: DiffId::parse(&diff).map_err(|_| bad("diff id", &diff))?,
            snapshot_ref: r.get(at + 3)?,
        },
        pinned_by: PinnedBy::parse(&pinned_by).ok_or_else(|| bad("pinned_by", &pinned_by))?,
        created_at: r.get(at + 5)?,
        base_tree: oid(6)?,
        head_tree: oid(7)?,
        base_commit: opt_oid(8)?,
        base_ref: r.get(at + 9)?,
        head_commit: opt_oid(10)?,
    })
}

/// Reads `s.id, s.verdict, s.submitted_at`, then [`ENTRY_COLUMNS`].
fn read_submission(r: &Row, fmt: ObjectFormat) -> Result<LastSubmission, StoreError> {
    let verdict: String = r.get(1)?;
    Ok(LastSubmission {
        submission_id: r.get(0)?,
        verdict: Verdict::parse(&verdict)
            .ok_or_else(|| StoreError::Integrity(format!("verdict {verdict:?}")))?,
        submitted_at: r.get(2)?,
        iteration: read_entry(r, 3, fmt)?,
    })
}

/// The object format of the review's repo.
fn review_format(conn: &Connection, review_id: &str) -> Result<Option<ObjectFormat>, StoreError> {
    let fmt: Option<String> = conn
        .query_row(
            "SELECT p.object_format FROM reviews r JOIN repos p ON p.id = r.repo_id \
             WHERE r.id = ?1",
            [review_id],
            |r| r.get(0),
        )
        .optional()?;
    fmt.map(|f| {
        ObjectFormat::from_name(&f)
            .ok_or_else(|| StoreError::Integrity(format!("repo object format {f:?}")))
    })
    .transpose()
}

impl Core {
    /// The review's iterations in `seq` order, with their trees
    /// (`NotFound` for an unknown review).
    pub fn iteration_entries(&self, review_id: &str) -> Result<Vec<IterationEntry>, CoreError> {
        let entries = self.store.read(|c| {
            let Some(fmt) = review_format(c, review_id)? else {
                return Ok(None);
            };
            let mut stmt = c.prepare_cached(&format!(
                "SELECT {ENTRY_COLUMNS} FROM iterations i JOIN diffs d ON d.id = i.diff_id \
                 WHERE i.review_id = ?1 ORDER BY i.seq"
            ))?;
            let rows = stmt
                .query_map([review_id], |r| Ok(read_entry(r, 0, fmt)))?
                .map(|r| r.map_err(StoreError::from).and_then(|x| x))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(rows))
        })?;
        entries.ok_or_else(|| CoreError::not_found("review", review_id))
    }

    /// The review's latest submission (by `submitted_at`) and its iteration;
    /// `None` when it was never submitted.
    pub fn last_submission(&self, review_id: &str) -> Result<Option<LastSubmission>, CoreError> {
        let last = self.store.read(|c| {
            let Some(fmt) = review_format(c, review_id)? else {
                return Ok(None);
            };
            let row = c
                .query_row(
                    &format!(
                        "SELECT s.id, s.verdict, s.submitted_at, {ENTRY_COLUMNS} \
                         FROM review_submissions s JOIN iterations i ON i.id = s.iteration_id \
                         JOIN diffs d ON d.id = i.diff_id \
                         WHERE s.review_id = ?1 ORDER BY s.submitted_at DESC, s.id DESC LIMIT 1"
                    ),
                    [review_id],
                    |r| Ok(read_submission(r, fmt)),
                )
                .optional()?
                .transpose()?;
            Ok(Some(row))
        })?;
        last.ok_or_else(|| CoreError::not_found("review", review_id))
    }

    /// Iteration `seq` of `current`'s review as an [`OpenedDiff`] (the repo and
    /// review fields are `current`'s). Its trees and files come from the store;
    /// `live` is `None` (a pinned state's objects are in the repo).
    pub fn open_iteration(&self, current: &OpenedDiff, seq: u32) -> Result<OpenedDiff, CoreError> {
        let fmt = current.repo.object_format;
        let found = self.store.read(|c| {
            if !review_exists(c, &current.review_id)? {
                return Ok(None);
            }
            let entry = c
                .query_row(
                    &format!(
                        "SELECT {ENTRY_COLUMNS} FROM iterations i JOIN diffs d ON d.id = i.diff_id \
                         WHERE i.review_id = ?1 AND i.seq = ?2"
                    ),
                    rusqlite::params![current.review_id, seq],
                    |r| Ok(read_entry(r, 0, fmt)),
                )
                .optional()?
                .transpose()?;
            let Some(entry) = entry else {
                return Ok(Some(None));
            };
            let files = load_files(c, &entry.info.diff_id)?;
            Ok(Some(Some((entry, files))))
        })?;
        let Some(found) = found else {
            return Err(CoreError::not_found("review", &current.review_id));
        };
        let Some((entry, files)) = found else {
            return Err(CoreError::not_found(
                "iteration",
                format!("{} #{seq}", current.review_id),
            ));
        };
        let files = files.ok_or_else(|| {
            CoreError::Store(StoreError::Integrity(format!(
                "iteration {seq} of {}: diff {} has no stored file list",
                current.review_id, entry.info.diff_id
            )))
        })?;
        Ok(OpenedDiff {
            repo: current.repo.clone(),
            repo_id: current.repo_id,
            review_id: current.review_id.clone(),
            review_key: current.review_key.clone(),
            kind: current.kind,
            diff_id: entry.info.diff_id.clone(),
            base: ResolvedSide {
                tree: entry.base_tree,
                commit: entry.base_commit,
                ref_name: entry.base_ref,
            },
            head_tree: entry.head_tree,
            head_commit: entry.head_commit,
            files: std::sync::Arc::new(files),
            live: None,
            warnings: Vec::new(),
            iteration: Some(entry.info),
        })
    }

    /// "Changes since last review" (module docs): the diff from the head tree
    /// of the last submission's iteration to `current`'s head, stored, as an
    /// [`OpenedDiff`] whose base is that submitted head (its commit when it had
    /// one) and whose `iteration` and `live` are `None`. `None` when the review
    /// was never submitted. `Conflict` when `current` is a live state that is not
    /// pinned (its objects may only be in the scratch store): pin it first.
    pub fn open_changes_since(
        &self,
        current: &OpenedDiff,
    ) -> Result<Option<OpenedDiff>, CoreError> {
        let pinned = current
            .iteration
            .as_ref()
            .is_some_and(|it| it.diff_id == current.diff_id);
        if current.live.is_some() && !pinned {
            return Err(CoreError::Conflict(format!(
                "the live state of review {} is not pinned; pin it before comparing it with \
                 the last review",
                current.review_id
            )));
        }
        let Some(last) = self.last_submission(&current.review_id)? else {
            return Ok(None);
        };
        let fmt = current.repo.object_format;
        let old = &last.iteration.head_tree;
        let new = &current.head_tree;
        let id = diff_id(fmt, old, new);
        let files = self.files_or_compute(&current.repo, fmt, &id, old, new, None)?;
        let now = now_ms();
        self.store
            .write(|tx| store_diff(tx, &id, fmt, old, new, &files, now))?;
        Ok(Some(OpenedDiff {
            repo: current.repo.clone(),
            repo_id: current.repo_id,
            review_id: current.review_id.clone(),
            review_key: current.review_key.clone(),
            kind: current.kind,
            iteration: None,
            diff_id: id,
            base: ResolvedSide {
                tree: old.clone(),
                commit: last.iteration.head_commit.clone(),
                ref_name: None,
            },
            head_tree: new.clone(),
            head_commit: current.head_commit.clone(),
            files,
            live: None,
            warnings: Vec::new(),
        }))
    }
}
