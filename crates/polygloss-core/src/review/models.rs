//! Plain row and view models shared by the review modules (T1.12).
//!
//! Also the row mappings every review module needs: `file_changes` rows to and from
//! [`FileChange`], and paths to and from the store's TEXT columns (design §7.1: git
//! paths are UTF-8 TEXT; non-UTF-8 bytes are kept in git's C-quoted form, OQ-25).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use polygloss_diff::{FileChange, FileKind, FileStatus, GitPath, Mode, ObjectFormat, Oid};
use rusqlite::{Connection, Row, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::git::{LiveState, RepoInfo, ResolveWarning, ResolvedSide, ReviewKind, Source};
use crate::ids::DiffId;
use crate::store::StoreError;
use crate::store::events::Actor;

/// Why an iteration was recorded (`iterations.pinned_by`, design §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PinnedBy {
    /// A commit or compare open (the first, or after a ref moved).
    Open,
    /// A commit or compare refresh from the "New iteration available" banner.
    Refresh,
    /// A human draft or comment on a live diff.
    Comment,
    /// An agent needed a durable id (`open_diff`, `create_comment` on live).
    Agent,
    /// The explicit Snapshot command.
    Manual,
    /// Submit review.
    Submit,
    /// `request_rereview` on a live review.
    Rereview,
}

impl PinnedBy {
    /// The `iterations.pinned_by` value.
    pub fn as_str(&self) -> &'static str {
        match self {
            PinnedBy::Open => "open",
            PinnedBy::Refresh => "refresh",
            PinnedBy::Comment => "comment",
            PinnedBy::Agent => "agent",
            PinnedBy::Manual => "manual",
            PinnedBy::Submit => "submit",
            PinnedBy::Rereview => "rereview",
        }
    }

    /// Inverse of [`PinnedBy::as_str`].
    pub fn parse(s: &str) -> Option<PinnedBy> {
        [
            PinnedBy::Open,
            PinnedBy::Refresh,
            PinnedBy::Comment,
            PinnedBy::Agent,
            PinnedBy::Manual,
            PinnedBy::Submit,
            PinnedBy::Rereview,
        ]
        .into_iter()
        .find(|p| p.as_str() == s)
    }
}

/// What to open: a source in a worktree (or any path inside a repo for commit and
/// compare sources).
#[derive(Debug, Clone)]
pub struct OpenRequest {
    /// Where to resolve the source. For live sources, the worktree under review
    /// (any path inside it).
    pub worktree: PathBuf,
    pub source: Source,
    /// Display label such as `PR #123`; never part of the key. `None` keeps the
    /// stored label.
    pub label: Option<String>,
    /// Commit/compare: why a new iteration would be recorded (default `Open`; pass
    /// `Refresh` from the refresh banner). Live: pin the state and record an
    /// iteration with this reason; `None` leaves the live state unpinned.
    pub pin: Option<PinnedBy>,
    /// Who opened it (recorded on `review.created` / `iteration.created`).
    pub actor: Actor,
}

/// One iteration of a review (design §7.2 `iterations`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IterationInfo {
    /// `iterations.id`.
    pub id: i64,
    /// 1-based within the review.
    pub seq: u32,
    pub diff_id: DiffId,
    /// `refs/polygloss/snapshots/<tree>` for pinned live states.
    pub snapshot_ref: Option<String>,
}

/// The result of [`Core::open`](crate::review::Core::open).
#[derive(Debug, Clone)]
pub struct OpenedDiff {
    pub repo: RepoInfo,
    pub repo_id: i64,
    pub review_id: String,
    pub review_key: String,
    pub kind: ReviewKind,
    /// The review's latest iteration when it shows this diff: always for commit and
    /// compare opens; for live opens only when pinned (by this open or earlier with
    /// the same trees).
    pub iteration: Option<IterationInfo>,
    pub diff_id: DiffId,
    pub base: ResolvedSide,
    pub head_tree: Oid,
    /// The head commit; `None` for live diffs (the head is the working tree).
    pub head_commit: Option<Oid>,
    /// The file list in `diff-tree` order, from `file_changes` when stored.
    pub files: Arc<Vec<FileChange>>,
    /// The live state (possibly unpinned) for live opens.
    pub live: Option<LiveState>,
    pub warnings: Vec<ResolveWarning>,
}

/// SQL predicate (over a `reviews` row aliased `r`) for "awaiting you" (design §17,
/// §11.2): re-review requested, or an open agent question without a published,
/// undeleted human reply. Shared with review summaries (T1.14).
pub(crate) const AWAITING_YOU_SQL: &str = "(r.status = 'rereview_requested' OR EXISTS (\
     SELECT 1 FROM threads t WHERE t.review_id = r.id AND t.kind = 'question' \
     AND t.status = 'open' AND NOT EXISTS (\
       SELECT 1 FROM comments c WHERE c.thread_id = t.id AND c.author_kind = 'human' \
       AND c.published_at IS NOT NULL AND c.deleted_at IS NULL)))";

/// SQL predicate (over `r`) for "has drafts": an unpublished comment, or a
/// non-empty autosaved Submit dialog.
pub(crate) const HAS_DRAFTS_SQL: &str = "(EXISTS (\
     SELECT 1 FROM threads t JOIN comments c ON c.thread_id = t.id \
     WHERE t.review_id = r.id AND c.published_at IS NULL AND c.deleted_at IS NULL) \
   OR EXISTS (SELECT 1 FROM review_drafts d WHERE d.review_id = r.id \
     AND (d.summary_md <> '' OR d.verdict IS NOT NULL)))";

/// A path as store TEXT: UTF-8 as is, other bytes C-quoted (OQ-25).
pub(crate) fn path_to_db(path: &Path) -> String {
    GitPath::from_bytes(path.as_os_str().as_bytes()).text
}

/// Inverse of [`path_to_db`].
pub(crate) fn path_from_db(text: &str) -> PathBuf {
    PathBuf::from(OsStr::from_bytes(&git_path_from_db(text).to_bytes()))
}

/// A `file_changes` path column back to a [`GitPath`]. The table has no escaped
/// flag, so a value is the escaped form exactly when it is git's canonical C-quoted
/// form of bytes that are not UTF-8 (which a UTF-8 path never needs).
pub(crate) fn git_path_from_db(text: &str) -> GitPath {
    if text.len() >= 2 && text.starts_with('"') && text.ends_with('"') {
        let candidate = GitPath {
            text: text.to_owned(),
            escaped: true,
        };
        let bytes = candidate.to_bytes();
        if std::str::from_utf8(&bytes).is_err() && GitPath::from_bytes(&bytes).text == text {
            return candidate;
        }
    }
    GitPath {
        text: text.to_owned(),
        escaped: false,
    }
}

fn kind_str(kind: FileKind) -> &'static str {
    match kind {
        FileKind::Text => "text",
        FileKind::Binary => "binary",
        FileKind::Symlink => "symlink",
        FileKind::Submodule => "submodule",
    }
}

fn parse_kind(s: &str) -> Option<FileKind> {
    Some(match s {
        "text" => FileKind::Text,
        "binary" => FileKind::Binary,
        "symlink" => FileKind::Symlink,
        "submodule" => FileKind::Submodule,
        _ => return None,
    })
}

/// Inserts the `diffs` row and its `file_changes` unless the diff is stored already
/// (`files_count` set). A row without `files_count` (inserted without its file list)
/// gets `files_count` and a fresh set of `file_changes`. Returns whether it wrote
/// the file list.
pub(crate) fn store_diff(
    tx: &Transaction,
    id: &DiffId,
    fmt: ObjectFormat,
    base_tree: &Oid,
    head_tree: &Oid,
    files: &[FileChange],
    now: i64,
) -> Result<bool, StoreError> {
    let inserted = tx.execute(
        "INSERT INTO diffs (id, object_format, base_tree, head_tree, files_count, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT (id) DO UPDATE SET files_count = excluded.files_count \
         WHERE diffs.files_count IS NULL",
        params![
            id.as_str(),
            fmt.as_str(),
            base_tree.as_str(),
            head_tree.as_str(),
            files.len() as i64,
            now
        ],
    )? == 1;
    if !inserted {
        return Ok(false);
    }
    // Rows a writer left without `files_count` are not trusted.
    tx.execute("DELETE FROM file_changes WHERE diff_id = ?1", [id.as_str()])?;
    let mut stmt = tx.prepare_cached(
        "INSERT INTO file_changes (diff_id, idx, status, old_path, new_path, old_mode, new_mode, \
           old_blob, new_blob, similarity, kind, generated) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
    )?;
    for f in files {
        stmt.execute(params![
            id.as_str(),
            f.idx,
            f.status.as_letter().to_string(),
            f.old_path.as_ref().map(|p| p.text.as_str()),
            f.new_path.as_ref().map(|p| p.text.as_str()),
            f.old_mode.map(|m| m.to_string()),
            f.new_mode.map(|m| m.to_string()),
            f.old_blob.as_str(),
            f.new_blob.as_str(),
            f.similarity,
            kind_str(f.kind),
            f.generated,
        ])?;
    }
    Ok(true)
}

/// The stored file list of a diff, or `None` when the diff is not stored.
pub(crate) fn load_files(
    conn: &Connection,
    id: &DiffId,
) -> Result<Option<Vec<FileChange>>, StoreError> {
    let meta: Option<(String, Option<i64>)> = conn
        .query_row(
            "SELECT object_format, files_count FROM diffs WHERE id = ?1",
            [id.as_str()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })?;
    let Some((fmt, Some(files_count))) = meta else {
        return Ok(None);
    };
    let fmt = ObjectFormat::from_name(&fmt)
        .ok_or_else(|| StoreError::Integrity(format!("diff {id}: object format {fmt:?}")))?;
    let mut stmt = conn.prepare_cached(
        "SELECT idx, status, old_path, new_path, old_mode, new_mode, old_blob, new_blob, \
           similarity, kind, generated \
         FROM file_changes WHERE diff_id = ?1 ORDER BY idx",
    )?;
    let files = stmt
        .query_map([id.as_str()], |r| Ok(read_file_row(r, fmt)))?
        .map(|row| row.map_err(StoreError::from).and_then(|f| f))
        .collect::<Result<Vec<_>, _>>()?;
    if files.len() as i64 != files_count {
        return Err(StoreError::Integrity(format!(
            "diff {id}: {} file_changes rows, files_count {files_count}",
            files.len()
        )));
    }
    Ok(Some(files))
}

fn read_file_row(r: &Row, fmt: ObjectFormat) -> Result<FileChange, StoreError> {
    let bad =
        |what: &str, value: &str| StoreError::Integrity(format!("file_changes {what} {value:?}"));
    let status: String = r.get(1)?;
    let mode = |i: usize| -> Result<Option<Mode>, StoreError> {
        r.get::<_, Option<String>>(i)?
            .map(|m| Mode::parse_octal(&m).ok_or_else(|| bad("mode", &m)))
            .transpose()
    };
    let oid = |i: usize| -> Result<Oid, StoreError> {
        let s: String = r.get(i)?;
        Oid::parse(&s, fmt).map_err(|_| bad("blob", &s))
    };
    let kind: String = r.get(9)?;
    Ok(FileChange {
        idx: r.get(0)?,
        status: status
            .bytes()
            .next()
            .filter(|_| status.len() == 1)
            .and_then(FileStatus::from_raw)
            .ok_or_else(|| bad("status", &status))?,
        old_path: r.get::<_, Option<String>>(2)?.map(|p| git_path_from_db(&p)),
        new_path: r.get::<_, Option<String>>(3)?.map(|p| git_path_from_db(&p)),
        old_mode: mode(4)?,
        new_mode: mode(5)?,
        old_blob: oid(6)?,
        new_blob: oid(7)?,
        similarity: r.get(8)?,
        kind: parse_kind(&kind).ok_or_else(|| bad("kind", &kind))?,
        generated: r.get(10)?,
    })
}

/// Reads an `iterations` row (`id, seq, diff_id, snapshot_ref`).
pub(crate) fn read_iteration(r: &Row) -> Result<IterationInfo, StoreError> {
    let diff: String = r.get(2)?;
    Ok(IterationInfo {
        id: r.get(0)?,
        seq: r.get(1)?,
        diff_id: DiffId::parse(&diff)
            .map_err(|_| StoreError::Integrity(format!("iteration diff id {diff:?}")))?,
        snapshot_ref: r.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinned_by_roundtrips() {
        for p in [
            PinnedBy::Open,
            PinnedBy::Refresh,
            PinnedBy::Comment,
            PinnedBy::Agent,
            PinnedBy::Manual,
            PinnedBy::Submit,
            PinnedBy::Rereview,
        ] {
            assert_eq!(PinnedBy::parse(p.as_str()), Some(p));
            assert_eq!(serde_json::to_value(p).unwrap(), p.as_str());
        }
        assert_eq!(PinnedBy::parse("nope"), None);
    }

    #[test]
    fn git_path_db_form_keeps_the_escaped_flag() {
        let escaped = GitPath::from_bytes(b"caf\xe9.txt");
        assert!(escaped.escaped);
        assert_eq!(git_path_from_db(&escaped.text), escaped);
        // UTF-8 names that merely look quoted stay literal.
        for literal in ["\"quoted\".txt", "\"a\"", "\"\\101\"", "plain.rs", "\""] {
            let p = git_path_from_db(literal);
            assert!(!p.escaped, "{literal}");
            assert_eq!(p.text, literal);
        }
    }

    #[test]
    fn repo_paths_roundtrip_through_db_text() {
        for bytes in [&b"/tmp/a b/repo/.git"[..], b"/tmp/x\xff\ny/.git"] {
            let path = Path::new(OsStr::from_bytes(bytes));
            assert_eq!(path_from_db(&path_to_db(path)), path);
        }
    }
}
