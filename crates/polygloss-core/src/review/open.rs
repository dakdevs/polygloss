//! `Core` and review open orchestration: repos, reviews, diffs, iterations, prune (T1.12).
//!
//! Rules (plan T1.12, design §3–§5, §7):
//!
//! - Every open upserts the repo row (`common_dir` unique, design §4.3) and the
//!   review row (unique `(repo_id, key)`; a commit is a degenerate `commit:<oid>`
//!   review, OQ-2). Reopening bumps `updated_at` and un-archives the review.
//! - Commit and compare opens record a new iteration only when the tree pair
//!   (`diff_id`) differs from the latest iteration's (`pinned_by = open`, or the
//!   request's `pin`, e.g. `refresh`).
//! - Live opens snapshot the worktree and create the review but no iteration unless
//!   `pin` is set. `diffs` and `file_changes` rows are written when a diff is first
//!   pinned or opened as commit/compare, and served from the table afterwards (no
//!   `diff-tree`, no `check-attr`).
//! - Every mutation appends its event in the same transaction (except un-archiving
//!   on reopen: §7.3 has no event kind for it).
//! - Snapshot refs vs prune: `Snapshotter::delete_unreferenced_refs` deletes every
//!   snapshot ref missing from the set it is given, so a pin that created its ref
//!   but has not committed its iteration yet would lose it. Pins (from
//!   `Snapshotter::pin` until the iteration commits) and prunes (from reading the
//!   referenced set until the refs are deleted) therefore hold one exclusive
//!   advisory lock per repo, `<data_dir>/locks/repo-<sha256(common_dir)[..16]>.lock`
//!   (`File::lock`, so it works across the app, CLI and MCP processes; waits are
//!   bounded by `GUARD_TIMEOUT`).
//! - Stale prunes (OQ-34) select candidates with one read, then check the rules
//!   again inside each delete transaction, so a review another process reopened,
//!   drafted on or asked about in between is kept.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use polygloss_diff::{FileChange, ObjectFormat, Oid};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::git::{
    Git, GitError, HeadSpec, LiveState, RepoInfo, Resolution, ResolveWarning, ResolvedSide,
    ReviewKind, Since, Snapshotter, Source, classify, default_branch, discover, list_changes,
    resolve,
};
use crate::ids::{DiffId, DiffIdPrefix, diff_id, new_uuid, review_key};
use crate::paths::DataPaths;
use crate::review::CoreError;
use crate::review::models::{
    AWAITING_YOU_SQL, HAS_DRAFTS_SQL, IterationInfo, OpenRequest, OpenedDiff, PinnedBy, load_files,
    path_from_db, path_to_db, read_iteration, store_diff,
};
use crate::store::events::{Actor, EventKind, NewEvent, append_event, now_ms};
use crate::store::{Store, StoreError};

const DAY_MS: i64 = 86_400_000;

/// How long a pin or prune waits for the per-repo guard before failing. A prune
/// holds it for its delete transaction and one `update-ref`; a pin for copying the
/// state's new objects, which takes seconds only for very large dirty states.
const GUARD_TIMEOUT: Duration = Duration::from_secs(120);
const GUARD_POLL: Duration = Duration::from_millis(20);

/// The shared core: the store, the data paths and the snapshotter. Cheap to clone;
/// clones share the store's writer connection.
#[derive(Clone, Debug)]
pub struct Core {
    pub store: Store,
    pub paths: DataPaths,
    pub snapshots: Arc<Snapshotter>,
}

/// A review row as the prune and pin paths need it.
struct ReviewRow {
    repo_id: i64,
    common_dir: PathBuf,
    key: String,
    kind: String,
    since: Option<String>,
    worktree_path: Option<String>,
}

/// What a prune did to one review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pruned {
    Yes,
    /// No such review (anymore).
    Gone,
    /// It no longer qualifies for `prune_stale` (touched, drafted on, awaiting you,
    /// or its repo is gone).
    Kept,
}

/// Provenance and reason of a new iteration.
struct NewIteration<'a> {
    review_id: &'a str,
    diff_id: &'a DiffId,
    base: &'a ResolvedSide,
    head: Option<&'a ResolvedSide>,
    snapshot_ref: Option<&'a str>,
    pinned_by: PinnedBy,
    actor: &'a Actor,
}

impl Core {
    /// Opens the core at the default paths (`DataPaths::resolve`, honoring
    /// `POLYGLOSS_DATA_DIR`).
    pub fn open_default() -> Result<Core, CoreError> {
        Core::with_paths(DataPaths::resolve()?)
    }

    /// Opens (creating and migrating) the store at `paths`; scratch stores go in
    /// `paths.scratch_dir`.
    pub fn with_paths(paths: DataPaths) -> Result<Core, CoreError> {
        let store = Store::open(&paths)?;
        let snapshots = Arc::new(Snapshotter::new(paths.scratch_dir.clone()));
        Ok(Core {
            store,
            paths,
            snapshots,
        })
    }

    /// Resolves `req.source`, snapshots live worktrees, and records the repo, the
    /// review and (for commit/compare, or live with `pin`) the iteration.
    pub fn open(&self, req: &OpenRequest) -> Result<OpenedDiff, CoreError> {
        let repo = discover(&req.worktree)?;
        let res = resolve(&repo, &req.worktree, &req.source)?;
        let branch =
            default_branch_from(&req.source, &res).or_else(|| self.default_branch_of(&repo));
        let fmt = res.object_format;
        match &res.head {
            HeadSpec::Tree(head) => {
                let id = diff_id(fmt, &res.base.tree, &head.tree);
                let files =
                    self.files_or_compute(&repo, fmt, &id, &res.base.tree, &head.tree, None)?;
                let pinned_by = req.pin.unwrap_or(PinnedBy::Open);
                let now = now_ms();
                let (repo_id, review_id, iteration) = self.store.write(|tx| {
                    let repo_id = upsert_repo(tx, &repo, branch.as_ref(), now)?;
                    let review_id = upsert_review(tx, repo_id, &res, None, req, now)?;
                    store_diff(tx, &id, fmt, &res.base.tree, &head.tree, &files, now)?;
                    let iteration = match latest_iteration(tx, &review_id)? {
                        Some(it) if it.diff_id == id => it,
                        _ => insert_iteration(
                            tx,
                            &NewIteration {
                                review_id: &review_id,
                                diff_id: &id,
                                base: &res.base,
                                head: Some(head),
                                snapshot_ref: None,
                                pinned_by,
                                actor: &req.actor,
                            },
                            now,
                        )?,
                    };
                    Ok((repo_id, review_id, iteration))
                })?;
                Ok(OpenedDiff {
                    repo,
                    repo_id,
                    review_id,
                    review_key: res.review_key.clone(),
                    kind: res.kind,
                    iteration: Some(iteration),
                    diff_id: id,
                    base: res.base.clone(),
                    head_tree: head.tree.clone(),
                    head_commit: head.commit.clone(),
                    files,
                    live: None,
                    warnings: res.warnings.clone(),
                })
            }
            HeadSpec::Worktree => {
                let state = self.snapshots.snapshot(&repo, &req.worktree)?;
                let id = diff_id(fmt, &res.base.tree, &state.head_tree);
                let now = now_ms();
                let (repo_id, review_id) = self.store.write(|tx| {
                    let repo_id = upsert_repo(tx, &repo, branch.as_ref(), now)?;
                    let review_id = upsert_review(tx, repo_id, &res, Some(&state), req, now)?;
                    Ok((repo_id, review_id))
                })?;
                let (files, iteration) = match req.pin {
                    Some(by) => {
                        let fixed_base = matches!(
                            req.source,
                            Source::Live {
                                since: Since::Commit(_)
                            }
                        );
                        let (it, files) = self.pin_state(
                            &repo, &review_id, &res.base, fixed_base, &state, by, &req.actor,
                        )?;
                        (files, Some(it))
                    }
                    None => {
                        let files = self.files_or_compute(
                            &repo,
                            fmt,
                            &id,
                            &res.base.tree,
                            &state.head_tree,
                            Some(&state),
                        )?;
                        let latest = self.store.read(|c| latest_iteration(c, &review_id))?;
                        (files, latest.filter(|it| it.diff_id == id))
                    }
                };
                Ok(OpenedDiff {
                    repo,
                    repo_id,
                    review_id,
                    review_key: res.review_key.clone(),
                    kind: res.kind,
                    iteration,
                    diff_id: id,
                    base: res.base.clone(),
                    head_tree: state.head_tree.clone(),
                    head_commit: None,
                    files,
                    live: Some(state),
                    warnings: res.warnings.clone(),
                })
            }
        }
    }

    /// Pins `live` (a snapshot of the review's worktree) and records it as an
    /// iteration of the live review `review_id`, unless the latest iteration already
    /// shows the same diff (then that one is returned). `Conflict` when the review
    /// is not live, `live` is from another worktree, or the worktree is no longer
    /// on the review's branch. The base is resolved again from the review's `since`
    /// (a `LiveState` carries no base), so for `since=merge-base` a default branch
    /// that moved after the snapshot changes the pinned diff; callers holding the
    /// displayed `OpenedDiff.base` should use [`Core::pin_live_on_base`].
    pub fn pin_live(
        &self,
        review_id: &str,
        live: &LiveState,
        by: PinnedBy,
        actor: &Actor,
    ) -> Result<IterationInfo, CoreError> {
        let (repo, since) = self.live_review_repo(review_id, live)?;
        let res = resolve(
            &repo,
            &live.worktree,
            &Source::Live {
                since: since.clone(),
            },
        )?;
        let fixed_base = matches!(since, Since::Commit(_));
        let (it, _) = self.pin_state(&repo, review_id, &res.base, fixed_base, live, by, actor)?;
        Ok(it)
    }

    /// [`Core::pin_live`] against an explicit base side (the `OpenedDiff.base` the
    /// caller displayed), so a base that moved since the snapshot cannot change the
    /// pinned diff.
    pub fn pin_live_on_base(
        &self,
        review_id: &str,
        base: &ResolvedSide,
        live: &LiveState,
        by: PinnedBy,
        actor: &Actor,
    ) -> Result<IterationInfo, CoreError> {
        let (repo, since) = self.live_review_repo(review_id, live)?;
        let fixed_base = matches!(since, Since::Commit(_));
        let (it, _) = self.pin_state(&repo, review_id, base, fixed_base, live, by, actor)?;
        Ok(it)
    }

    /// The review's iterations in `seq` order (empty for an unpinned live review).
    pub fn iterations(&self, review_id: &str) -> Result<Vec<IterationInfo>, CoreError> {
        let its = self.store.read(|c| {
            if !review_exists(c, review_id)? {
                return Ok(None);
            }
            let mut stmt = c.prepare_cached(
                "SELECT id, seq, diff_id, snapshot_ref FROM iterations \
                 WHERE review_id = ?1 ORDER BY seq",
            )?;
            let rows = stmt
                .query_map([review_id], |r| Ok(read_iteration(r)))?
                .map(|r| r.map_err(StoreError::from).and_then(|x| x))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some(rows))
        })?;
        its.ok_or_else(|| CoreError::not_found("review", review_id))
    }

    /// The stored file list of a diff (`None` for diffs never pinned or opened as
    /// commit/compare).
    pub fn files_for_diff(
        &self,
        diff_id: &DiffId,
    ) -> Result<Option<Arc<Vec<FileChange>>>, CoreError> {
        Ok(self.store.read(|c| load_files(c, diff_id))?.map(Arc::new))
    }

    /// Resolves a diff id or unique prefix (8+ hex chars) and finds a repo that has
    /// both of its trees (design §4.1): the repo containing `cwd`, then repos whose
    /// iterations reference the diff, then every known repo (most recently opened
    /// first), each checked with `git cat-file -e`. Repos whose directory is gone
    /// are skipped.
    pub fn find_repo_for_diff(
        &self,
        id_or_prefix: &str,
        cwd: Option<&Path>,
    ) -> Result<(RepoInfo, DiffId), CoreError> {
        let prefix = DiffIdPrefix::parse(id_or_prefix)?;
        let (matches, iteration_repos, all_repos) = self.store.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id, object_format, base_tree, head_tree FROM diffs \
                 WHERE substr(id, 1, ?1) = ?2 ORDER BY id LIMIT 20",
            )?;
            let matches = stmt
                .query_map(
                    params![prefix.as_str().len() as i64, prefix.as_str()],
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, String>(3)?,
                        ))
                    },
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let iteration_repos = match matches.as_slice() {
                [(id, ..)] => strings(
                    c,
                    "SELECT p.common_dir FROM repos p \
                     WHERE p.id IN (SELECT v.repo_id FROM reviews v \
                       JOIN iterations i ON i.review_id = v.id WHERE i.diff_id = ?1) \
                     ORDER BY p.last_opened_at DESC",
                    [id],
                )?,
                _ => Vec::new(),
            };
            let all_repos = strings(
                c,
                "SELECT common_dir FROM repos ORDER BY last_opened_at DESC",
                [],
            )?;
            Ok((matches, iteration_repos, all_repos))
        })?;
        let (id, fmt, base, head) = match matches.len() {
            0 => return Err(CoreError::not_found("diff", prefix.as_str())),
            1 => matches.into_iter().next().expect("one match"),
            _ => {
                return Err(CoreError::Ambiguous {
                    prefix: prefix.as_str().to_owned(),
                    matches: matches.into_iter().map(|m| m.0).collect(),
                });
            }
        };
        let bad = |what: &str| StoreError::Integrity(format!("diff {id}: bad {what}"));
        let diff = DiffId::parse(&id).map_err(|_| bad("id"))?;
        let fmt = ObjectFormat::from_name(&fmt).ok_or_else(|| bad("object format"))?;
        let base = Oid::parse(&base, fmt).map_err(|_| bad("base tree"))?;
        let head = Oid::parse(&head, fmt).map_err(|_| bad("head tree"))?;

        let mut seen = HashSet::new();
        if let Some(repo) = cwd.and_then(|p| discover(p).ok()) {
            seen.insert(repo.common_dir.clone());
            if repo.object_format == fmt && has_objects(&repo, &[&base, &head])? {
                return Ok((repo, diff));
            }
        }
        // Deduped before `open_known_repo`, which spawns git.
        for dir in iteration_repos.iter().chain(&all_repos) {
            let dir = path_from_db(dir);
            if !seen.insert(dir.clone()) {
                continue;
            }
            let Some(repo) = open_known_repo(&dir) else {
                continue;
            };
            if repo.object_format == fmt && has_objects(&repo, &[&base, &head])? {
                return Ok((repo, diff));
            }
        }
        Err(CoreError::RepoNotFound(diff.to_string()))
    }

    /// Archives the review (hidden from recents; reopening un-archives it) and
    /// appends `review.archived`. Archiving an archived review does nothing.
    pub fn archive_review(&self, review_id: &str, actor: &Actor) -> Result<(), CoreError> {
        let found = self.store.write(|tx| {
            let row: Option<(String, String, Option<i64>)> = tx
                .query_row(
                    "SELECT key, kind, archived_at FROM reviews WHERE id = ?1",
                    [review_id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((key, kind, archived_at)) = row else {
                return Ok(false);
            };
            if archived_at.is_some() {
                return Ok(true);
            }
            tx.execute(
                "UPDATE reviews SET archived_at = ?2 WHERE id = ?1",
                params![review_id, now_ms()],
            )?;
            append_event(
                tx,
                &review_event(
                    EventKind::ReviewArchived,
                    review_id,
                    actor,
                    json!({ "key": key, "kind": kind }),
                ),
            )?;
            Ok(true)
        })?;
        if found {
            Ok(())
        } else {
            Err(CoreError::not_found("review", review_id))
        }
    }

    /// Deletes the review and everything hanging off it (iterations, submissions,
    /// assignment, drafts, threads and their comments; OQ-24), then the diffs no
    /// iteration or thread references anymore, then the repo's snapshot refs nothing
    /// references anymore. Appends `review.archived` with `"pruned": true` (the
    /// review is gone for good; waiters wake as on archive, OQ-11). An orphaned
    /// review (its repo directory is gone) loses its rows only.
    pub fn prune_review(&self, review_id: &str) -> Result<(), CoreError> {
        match self.prune(review_id, None)? {
            Pruned::Yes => Ok(()),
            Pruned::Gone | Pruned::Kept => Err(CoreError::not_found("review", review_id)),
        }
    }

    /// Prunes reviews whose `updated_at` is older than `older_than_days` before
    /// `now_ms` (`storage.prune_reviews_after_days`, OQ-34), except orphaned reviews
    /// (repo directory gone), reviews with drafts (unpublished comments or a
    /// non-empty Submit dialog draft) and reviews awaiting you. Returns the pruned
    /// ids, oldest first. A review that fails to prune is logged and skipped. The
    /// rules are checked again inside each review's delete transaction, so one that
    /// was touched after the candidates were read is kept.
    pub fn prune_stale(&self, older_than_days: u32, now_ms: i64) -> Result<Vec<String>, CoreError> {
        let cutoff = now_ms.saturating_sub(i64::from(older_than_days) * DAY_MS);
        let candidates: Vec<(String, String)> = self.store.read(|c| {
            let mut stmt = c.prepare(&format!(
                "SELECT r.id, p.common_dir FROM reviews r JOIN repos p ON p.id = r.repo_id \
                 WHERE r.updated_at < ?1 AND NOT {HAS_DRAFTS_SQL} AND NOT {AWAITING_YOU_SQL} \
                 ORDER BY r.updated_at, r.id"
            ))?;
            let rows = stmt
                .query_map([cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        after_prune_candidates();
        let mut pruned = Vec::new();
        for (id, common_dir) in candidates {
            if !path_from_db(&common_dir).exists() {
                continue;
            }
            // The rules are checked again when deleting: the review may have been
            // reopened, drafted on or asked about since the candidates were read.
            match self.prune(&id, Some(cutoff)) {
                Ok(Pruned::Yes) => pruned.push(id),
                Ok(Pruned::Gone | Pruned::Kept) => {}
                Err(e) => tracing::warn!(review_id = %id, error = %e, "prune_stale: skipped"),
            }
        }
        Ok(pruned)
    }

    /// [`Core::prune_review`]. With `stale_before`, the review is deleted only when,
    /// inside the delete transaction, it still qualifies for [`Core::prune_stale`]
    /// (`updated_at < stale_before`, no drafts, not awaiting you, repo on disk).
    fn prune(&self, review_id: &str, stale_before: Option<i64>) -> Result<Pruned, CoreError> {
        let Some(row) = self.store.read(|c| review_row(c, review_id))? else {
            return Ok(Pruned::Gone);
        };
        let repo = open_known_repo(&row.common_dir);
        if repo.is_none() && stale_before.is_some() {
            return Ok(Pruned::Kept);
        }
        // Held from reading `referenced` until the refs are deleted (module docs).
        let _guard = repo.as_ref().map(|r| self.repo_guard(r)).transpose()?;
        let (outcome, referenced) = self.store.write(|tx| {
            let outcome = delete_review(tx, review_id, stale_before)?;
            if outcome != Pruned::Yes {
                return Ok((outcome, None));
            }
            append_event(
                tx,
                &review_event(
                    EventKind::ReviewArchived,
                    review_id,
                    &Actor::system(),
                    json!({ "key": row.key, "kind": row.kind, "pruned": true }),
                ),
            )?;
            Ok((outcome, Some(referenced_refs(tx, row.repo_id)?)))
        })?;
        if let (Some(repo), Some(referenced)) = (&repo, &referenced) {
            self.snapshots.delete_unreferenced_refs(repo, referenced)?;
        }
        Ok(outcome)
    }

    /// Pins `state` under the repo guard and records (or reuses) its iteration.
    #[allow(clippy::too_many_arguments)]
    fn pin_state(
        &self,
        repo: &RepoInfo,
        review_id: &str,
        base: &ResolvedSide,
        fixed_base: bool,
        state: &LiveState,
        by: PinnedBy,
        actor: &Actor,
    ) -> Result<(IterationInfo, Arc<Vec<FileChange>>), CoreError> {
        let fmt = repo.object_format;
        let id = diff_id(fmt, &base.tree, &state.head_tree);
        // Held from the ref update until the iteration naming it commits.
        let _guard = self.repo_guard(repo)?;
        let snapshot_ref = self.snapshots.pin(repo, state)?;
        if fixed_base {
            self.snapshots.pin_tree(repo, &base.tree)?;
        }
        pause_after_pin();
        let files =
            self.files_or_compute(repo, fmt, &id, &base.tree, &state.head_tree, Some(state))?;
        let now = now_ms();
        let it = self.store.write(|tx| {
            store_diff(tx, &id, fmt, &base.tree, &state.head_tree, &files, now)?;
            match latest_iteration(tx, review_id)? {
                Some(it) if it.diff_id == id => Ok(it),
                _ => insert_iteration(
                    tx,
                    &NewIteration {
                        review_id,
                        diff_id: &id,
                        base,
                        head: None,
                        snapshot_ref: Some(&snapshot_ref),
                        pinned_by: by,
                        actor,
                    },
                    now,
                ),
            }
        })?;
        Ok((it, files))
    }

    /// Checks that `review_id` is a live review of `live.worktree` and returns its
    /// repo and base.
    fn live_review_repo(
        &self,
        review_id: &str,
        live: &LiveState,
    ) -> Result<(RepoInfo, Since), CoreError> {
        let row = self
            .store
            .read(|c| review_row(c, review_id))?
            .ok_or_else(|| CoreError::not_found("review", review_id))?;
        if row.kind != ReviewKind::Live.as_str() {
            return Err(CoreError::Conflict(format!(
                "review {review_id} is a {} review, not live",
                row.kind
            )));
        }
        if row.worktree_path.as_deref() != Some(path_to_db(&live.worktree).as_str()) {
            return Err(CoreError::Conflict(format!(
                "the live state of {} does not belong to review {review_id}",
                live.worktree.display()
            )));
        }
        let repo = discover(&live.worktree)?;
        if repo.common_dir != row.common_dir {
            return Err(CoreError::Conflict(format!(
                "{} is no longer a worktree of {}",
                live.worktree.display(),
                row.common_dir.display()
            )));
        }
        let since_key = row.since.as_deref().unwrap_or("merge-base");
        // The key names the branch; after a `git checkout` of another branch the
        // worktree belongs to another live review (checked now, not at snapshot
        // time: `LiveState` records no branch).
        let branch = current_branch(&live.worktree)?;
        let branch = branch
            .as_deref()
            .map(|r| r.strip_prefix("refs/heads/").unwrap_or(r));
        if review_key::live(&live.worktree, branch, since_key) != row.key {
            return Err(CoreError::Conflict(format!(
                "{} is now on {}, not the branch of review {review_id} ({})",
                live.worktree.display(),
                branch.unwrap_or("a detached HEAD"),
                row.key
            )));
        }
        let since = match since_key {
            "merge-base" => Since::MergeBase,
            "HEAD" => Since::Head,
            oid => Since::Commit(oid.to_owned()),
        };
        Ok((repo, since))
    }

    /// The stored file list of `id`, or `diff-tree` + `classify` when not stored.
    fn files_or_compute(
        &self,
        repo: &RepoInfo,
        fmt: ObjectFormat,
        id: &DiffId,
        base: &Oid,
        head: &Oid,
        live: Option<&LiveState>,
    ) -> Result<Arc<Vec<FileChange>>, CoreError> {
        if let Some(files) = self.files_for_diff(id)? {
            return Ok(files);
        }
        let cwd = repo
            .toplevel
            .clone()
            .unwrap_or_else(|| repo.git_dir.clone());
        let mut git = Git::new(cwd);
        if let Some(state) = live {
            git = self.snapshots.diff_env(state, git);
        }
        let mut files = list_changes(&git, fmt, base, head)?;
        // `check-attr` needs a worktree top level; bare repos keep mode-based kinds.
        if repo.toplevel.is_some() {
            classify(&git, head, &mut files, &[])?;
        }
        Ok(Arc::new(files))
    }

    /// The repo's default branch for `repos.default_branch` (the OQ-5 chain):
    /// `Some(Some(name))` to store, `Some(None)` when there is none, `None` (keep the
    /// stored value) when git failed otherwise.
    fn default_branch_of(&self, repo: &RepoInfo) -> Option<Option<String>> {
        let cwd = repo
            .toplevel
            .clone()
            .unwrap_or_else(|| repo.git_dir.clone());
        match default_branch(&Git::new(cwd)) {
            Ok((name, _)) => Some(Some(name)),
            Err(GitError::NoDefaultBranch) => Some(None),
            Err(e) => {
                tracing::warn!(error = %e, "default branch lookup failed");
                None
            }
        }
    }

    /// The exclusive per-repo pin/prune guard (module docs).
    fn repo_guard(&self, repo: &RepoInfo) -> Result<File, CoreError> {
        let dir = self.paths.data_dir.join("locks");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .map_err(|e| StoreError::io("create", &dir, e))?;
        let hash = hex::encode(Sha256::digest(repo.common_dir.as_os_str().as_bytes()));
        let path = dir.join(format!("repo-{}.lock", &hash[..16]));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&path)
            .map_err(|e| StoreError::io("open", &path, e))?;
        // Bounded, so a pin or prune stuck in git cannot block the other forever.
        let timeout = guard_timeout();
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(file),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(GUARD_POLL);
                }
                Err(TryLockError::WouldBlock) => {
                    let e = io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!(
                            "another pin or prune of {} held this lock for over {} s",
                            repo.common_dir.display(),
                            timeout.as_secs()
                        ),
                    );
                    return Err(StoreError::io("lock", &path, e).into());
                }
                Err(TryLockError::Error(e)) => return Err(StoreError::io("lock", &path, e).into()),
            }
        }
    }
}

/// The default branch a live `since=merge-base` resolution already looked up
/// (same shape as [`Core::default_branch_of`]), so the open does not run the OQ-5
/// chain twice. `None` when the resolution did not look it up.
fn default_branch_from(source: &Source, res: &Resolution) -> Option<Option<String>> {
    if !matches!(
        source,
        Source::Live {
            since: Since::MergeBase
        }
    ) {
        return None;
    }
    for w in &res.warnings {
        match w {
            ResolveWarning::UnbornHead => return None,
            ResolveWarning::NoDefaultBranch => return Some(None),
            ResolveWarning::NoMergeBase { default_branch }
            | ResolveWarning::MergeBaseBeyondShallow { default_branch } => {
                return Some(Some(default_branch.clone()));
            }
            ResolveWarning::MultipleMergeBases { .. } => {}
        }
    }
    // A merge base was found: the base side is the default branch's.
    res.base.ref_name.clone().map(Some)
}

/// Opens a known repo by its common dir, preferring its main worktree (so
/// `toplevel` is set). `None` when it is gone or no longer that repo.
fn open_known_repo(common_dir: &Path) -> Option<RepoInfo> {
    if !common_dir.exists() {
        return None;
    }
    let main = (common_dir.file_name() == Some(OsStr::new(".git")))
        .then(|| common_dir.parent())
        .flatten();
    main.and_then(|m| discover(m).ok())
        .filter(|r| r.common_dir == common_dir)
        .or_else(|| {
            discover(common_dir)
                .ok()
                .filter(|r| r.common_dir == common_dir)
        })
}

/// `refs/heads/<branch>` HEAD of `worktree` points at, `None` when detached.
fn current_branch(worktree: &Path) -> Result<Option<String>, CoreError> {
    let args = [
        OsStr::new("symbolic-ref"),
        OsStr::new("-q"),
        OsStr::new("HEAD"),
    ];
    let out = Git::new(worktree).run(&args)?;
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

/// Whether the repo has every object (`git cat-file -e`, never fetches).
fn has_objects(repo: &RepoInfo, oids: &[&Oid]) -> Result<bool, CoreError> {
    let git = Git::new(
        repo.toplevel
            .clone()
            .unwrap_or_else(|| repo.git_dir.clone()),
    );
    for oid in oids {
        let args = [
            OsStr::new("cat-file"),
            OsStr::new("-e"),
            OsStr::new(oid.as_str()),
        ];
        if git.status(&args)? != 0 {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `basename` of the main worktree: the parent of a `.git` common dir, else the
/// common dir's name without `.git` (bare repos).
fn display_name(common_dir: &Path) -> String {
    let name = common_dir.file_name().unwrap_or_default();
    let name = if name == ".git" {
        common_dir
            .parent()
            .and_then(Path::file_name)
            .unwrap_or(name)
    } else {
        name
    };
    let name = name.to_string_lossy();
    name.strip_suffix(".git")
        .filter(|n| !n.is_empty())
        .unwrap_or(&name)
        .to_owned()
}

fn upsert_repo(
    tx: &Transaction,
    repo: &RepoInfo,
    default_branch: Option<&Option<String>>,
    now: i64,
) -> Result<i64, StoreError> {
    Ok(tx.query_row(
        "INSERT INTO repos (common_dir, display_name, object_format, default_branch, created_at, \
           last_opened_at) VALUES (?1, ?2, ?3, ?4, ?5, ?5) \
         ON CONFLICT (common_dir) DO UPDATE SET display_name = excluded.display_name, \
           object_format = excluded.object_format, last_opened_at = excluded.last_opened_at, \
           default_branch = CASE WHEN ?6 THEN excluded.default_branch ELSE default_branch END \
         RETURNING id",
        params![
            path_to_db(&repo.common_dir),
            display_name(&repo.common_dir),
            repo.object_format.as_str(),
            default_branch.cloned().flatten(),
            now,
            default_branch.is_some(),
        ],
        |r| r.get(0),
    )?)
}

/// Finds or creates the review for `res.review_key` and returns its id. A new
/// review appends `review.created`; an existing one gets `updated_at`, is
/// un-archived and takes a new label when one is given.
fn upsert_review(
    tx: &Transaction,
    repo_id: i64,
    res: &Resolution,
    live: Option<&LiveState>,
    req: &OpenRequest,
    now: i64,
) -> Result<String, StoreError> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM reviews WHERE repo_id = ?1 AND key = ?2",
            params![repo_id, res.review_key],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        tx.execute(
            "UPDATE reviews SET updated_at = ?2, archived_at = NULL, label = COALESCE(?3, label) \
             WHERE id = ?1",
            params![id, now, req.label],
        )?;
        return Ok(id);
    }
    let key = res.review_key.as_str();
    let (mut base_spec, mut head_spec, mut compare_mode, mut since, mut worktree) =
        (None, None, None, None, None);
    match &req.source {
        Source::Compare { mode, .. } => {
            let direct = *mode == crate::git::CompareMode::Direct;
            let pair = key.strip_prefix("compare:").unwrap_or(key);
            // Ref names never contain "..", so the first separator splits the key.
            if let Some((b, h)) = pair.split_once(if direct { ".." } else { "..." }) {
                base_spec = Some(b.to_owned());
                head_spec = Some(h.to_owned());
            }
            compare_mode = Some(if direct { "direct" } else { "three-dot" });
        }
        Source::Commit { .. } => {
            head_spec = key.strip_prefix("commit:").map(str::to_owned);
        }
        Source::Live { .. } => {
            since = key.rsplit_once("#since=").map(|(_, s)| s.to_owned());
            worktree = live.map(|s| path_to_db(&s.worktree));
        }
    }
    let id = new_uuid();
    tx.execute(
        "INSERT INTO reviews (id, repo_id, key, kind, label, base_spec, head_spec, compare_mode, \
           since, worktree_path, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            id,
            repo_id,
            key,
            res.kind.as_str(),
            req.label,
            base_spec,
            head_spec,
            compare_mode,
            since,
            worktree,
            now
        ],
    )?;
    append_event(
        tx,
        &review_event(
            EventKind::ReviewCreated,
            &id,
            &req.actor,
            json!({ "key": key, "kind": res.kind.as_str() }),
        ),
    )?;
    Ok(id)
}

pub(crate) fn latest_iteration(
    conn: &Connection,
    review_id: &str,
) -> Result<Option<IterationInfo>, StoreError> {
    conn.query_row(
        "SELECT id, seq, diff_id, snapshot_ref FROM iterations WHERE review_id = ?1 \
         ORDER BY seq DESC LIMIT 1",
        [review_id],
        |r| Ok(read_iteration(r)),
    )
    .optional()?
    .transpose()
}

/// Inserts the next iteration, appends `iteration.created` and bumps the review's
/// `updated_at`.
fn insert_iteration(
    tx: &Transaction,
    it: &NewIteration<'_>,
    now: i64,
) -> Result<IterationInfo, StoreError> {
    let seq: u32 = tx.query_row(
        "SELECT COALESCE(MAX(seq), 0) + 1 FROM iterations WHERE review_id = ?1",
        [it.review_id],
        |r| r.get(0),
    )?;
    tx.execute(
        "INSERT INTO iterations (review_id, seq, diff_id, base_commit, head_commit, base_ref, \
           head_ref, snapshot_ref, pinned_by, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            it.review_id,
            seq,
            it.diff_id.as_str(),
            it.base.commit.as_ref().map(Oid::as_str),
            it.head.and_then(|h| h.commit.as_ref()).map(Oid::as_str),
            it.base.ref_name,
            it.head.and_then(|h| h.ref_name.as_deref()),
            it.snapshot_ref,
            it.pinned_by.as_str(),
            now
        ],
    )?;
    let id = tx.last_insert_rowid();
    tx.execute(
        "UPDATE reviews SET updated_at = ?2 WHERE id = ?1",
        params![it.review_id, now],
    )?;
    append_event(
        tx,
        &NewEvent {
            kind: EventKind::IterationCreated,
            review_id: Some(it.review_id.to_owned()),
            diff_id: Some(it.diff_id.to_string()),
            thread_id: None,
            comment_id: None,
            actor: it.actor.clone(),
            payload: json!({
                "seq": seq,
                "diff_id": it.diff_id.as_str(),
                "pinned_by": it.pinned_by.as_str(),
            }),
        },
    )?;
    Ok(IterationInfo {
        id,
        seq,
        diff_id: it.diff_id.clone(),
        snapshot_ref: it.snapshot_ref.map(str::to_owned),
    })
}

fn review_event(
    kind: EventKind,
    review_id: &str,
    actor: &Actor,
    payload: serde_json::Value,
) -> NewEvent {
    NewEvent {
        kind,
        review_id: Some(review_id.to_owned()),
        diff_id: None,
        thread_id: None,
        comment_id: None,
        actor: actor.clone(),
        payload,
    }
}

pub(crate) fn review_exists(conn: &Connection, review_id: &str) -> Result<bool, StoreError> {
    Ok(conn
        .query_row("SELECT 1 FROM reviews WHERE id = ?1", [review_id], |_| {
            Ok(())
        })
        .optional()?
        .is_some())
}

fn review_row(conn: &Connection, review_id: &str) -> Result<Option<ReviewRow>, StoreError> {
    Ok(conn
        .query_row(
            "SELECT r.repo_id, p.common_dir, r.key, r.kind, r.since, r.worktree_path \
             FROM reviews r JOIN repos p ON p.id = r.repo_id WHERE r.id = ?1",
            [review_id],
            |r| {
                Ok(ReviewRow {
                    repo_id: r.get(0)?,
                    common_dir: path_from_db(&r.get::<_, String>(1)?),
                    key: r.get(2)?,
                    kind: r.get(3)?,
                    since: r.get(4)?,
                    worktree_path: r.get(5)?,
                })
            },
        )
        .optional()?)
}

/// Deletes the review (cascading per §7.4) and then its diffs that nothing
/// references anymore. With `stale_before`, only when the review still qualifies
/// for `prune_stale` (checked in `tx`, so no other writer can change it before the
/// delete).
fn delete_review(
    tx: &Transaction,
    review_id: &str,
    stale_before: Option<i64>,
) -> Result<Pruned, StoreError> {
    if !review_exists(tx, review_id)? {
        return Ok(Pruned::Gone);
    }
    if let Some(cutoff) = stale_before {
        let still_stale: bool = tx.query_row(
            &format!(
                "SELECT EXISTS (SELECT 1 FROM reviews r WHERE r.id = ?1 AND r.updated_at < ?2 \
                 AND NOT {HAS_DRAFTS_SQL} AND NOT {AWAITING_YOU_SQL})"
            ),
            params![review_id, cutoff],
            |r| r.get(0),
        )?;
        if !still_stale {
            return Ok(Pruned::Kept);
        }
    }
    let diffs = strings(
        tx,
        "SELECT diff_id FROM iterations WHERE review_id = ?1 \
         UNION SELECT origin_diff_id FROM threads WHERE review_id = ?1",
        [review_id],
    )?;
    tx.execute("DELETE FROM reviews WHERE id = ?1", [review_id])?;
    for diff in diffs {
        tx.execute(
            "DELETE FROM diffs WHERE id = ?1 \
             AND NOT EXISTS (SELECT 1 FROM iterations WHERE diff_id = ?1) \
             AND NOT EXISTS (SELECT 1 FROM threads WHERE origin_diff_id = ?1)",
            [&diff],
        )?;
    }
    Ok(Pruned::Yes)
}

/// Snapshot refs the repo's remaining reviews still need: every iteration's
/// `snapshot_ref`, plus the refs named after the base and head trees of every diff
/// its iterations and threads reference (fixed `since=<commit>` bases, §5.3).
fn referenced_refs(tx: &Transaction, repo_id: i64) -> Result<HashSet<String>, StoreError> {
    let mut stmt = tx.prepare_cached(
        "WITH used(diff_id) AS ( \
           SELECT i.diff_id FROM iterations i JOIN reviews v ON v.id = i.review_id WHERE v.repo_id = ?1 \
           UNION SELECT t.origin_diff_id FROM threads t JOIN reviews v ON v.id = t.review_id \
             WHERE v.repo_id = ?1) \
         SELECT i.snapshot_ref FROM iterations i JOIN reviews v ON v.id = i.review_id \
           WHERE v.repo_id = ?1 AND i.snapshot_ref IS NOT NULL \
         UNION SELECT ?2 || d.head_tree FROM diffs d JOIN used u ON u.diff_id = d.id \
         UNION SELECT ?2 || d.base_tree FROM diffs d JOIN used u ON u.diff_id = d.id",
    )?;
    let refs = stmt
        .query_map(params![repo_id, crate::git::SNAPSHOT_REF_PREFIX], |r| {
            r.get(0)
        })?
        .collect::<Result<HashSet<String>, _>>()?;
    Ok(refs)
}

fn strings(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
) -> Result<Vec<String>, StoreError> {
    let mut stmt = conn.prepare_cached(sql)?;
    let rows = stmt
        .query_map(params, |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}

#[cfg(feature = "test-support")]
static PIN_PAUSE_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Test hook (feature `test-support`): sleep this long after a pin created its ref
/// and before its iteration commits, so tests can race a prune against it.
#[cfg(feature = "test-support")]
pub(crate) fn set_pin_pause(pause: std::time::Duration) {
    let ms = u64::try_from(pause.as_millis()).unwrap_or(u64::MAX);
    PIN_PAUSE_MS.store(ms, std::sync::atomic::Ordering::SeqCst);
}

fn pause_after_pin() {
    #[cfg(feature = "test-support")]
    {
        let ms = PIN_PAUSE_MS.load(std::sync::atomic::Ordering::SeqCst);
        if ms > 0 {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}

#[cfg(feature = "test-support")]
static GUARD_TIMEOUT_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Test hook (feature `test-support`): override [`GUARD_TIMEOUT`] (`None` restores it).
#[cfg(feature = "test-support")]
pub(crate) fn set_guard_timeout(timeout: Option<Duration>) {
    let ms = timeout.map_or(0, |t| {
        u64::try_from(t.as_millis()).unwrap_or(u64::MAX).max(1)
    });
    GUARD_TIMEOUT_MS.store(ms, std::sync::atomic::Ordering::SeqCst);
}

fn guard_timeout() -> Duration {
    #[cfg(feature = "test-support")]
    {
        let ms = GUARD_TIMEOUT_MS.load(std::sync::atomic::Ordering::SeqCst);
        if ms > 0 {
            return Duration::from_millis(ms);
        }
    }
    GUARD_TIMEOUT
}

#[cfg(feature = "test-support")]
type PruneHook = Box<dyn FnOnce() + Send>;

#[cfg(feature = "test-support")]
static PRUNE_STALE_HOOK: std::sync::Mutex<Option<PruneHook>> = std::sync::Mutex::new(None);

/// Test hook (feature `test-support`): run `hook` once in the next `prune_stale`,
/// between selecting its candidates and deleting them.
#[cfg(feature = "test-support")]
pub(crate) fn set_prune_stale_hook(hook: PruneHook) {
    *PRUNE_STALE_HOOK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(hook);
}

fn after_prune_candidates() {
    #[cfg(feature = "test-support")]
    {
        let hook = PRUNE_STALE_HOOK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(hook) = hook {
            hook();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_uses_the_main_worktree_or_bare_dir() {
        assert_eq!(display_name(Path::new("/src/app/.git")), "app");
        assert_eq!(display_name(Path::new("/srv/app.git")), "app");
        assert_eq!(display_name(Path::new("/srv/plain")), "plain");
        assert_eq!(display_name(Path::new("/srv/.git.git")), ".git");
    }
}
