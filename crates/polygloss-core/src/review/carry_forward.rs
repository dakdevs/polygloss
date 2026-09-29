//! Carry-forward of thread positions via line mapping (T1.15, design §8.6,
//! ADR-0010).
//!
//! A thread keeps the anchor it was created with (`ThreadAnchor`: path, side,
//! 1-based lines of `anchor_blob`). Its position in another diff is computed by
//! mapping those lines from `anchor_blob` into the blob the diff shows on the same
//! side:
//!
//! - **Review** subjects belong to the review panel: `Exact` with no path.
//! - The file is found by new path, else by the old path of a renamed or deleted
//!   file (T1.13 stores a deleted file's thread under its old path). No such file,
//!   or no blob on the anchored side (an added file's old side, a deleted file's
//!   new side): `Absent` (threads panel only).
//! - **File** subjects: `Exact` on that file's header.
//! - **Line** subjects: `Exact` when the current blob is the anchor blob; `Moved`
//!   to the new numbers when every anchored line is in one equal region of the
//!   line diff (`LineMap`, Myers + indent heuristic, whitespace exact), even when
//!   the numbers stay the same; otherwise `Outdated` at the line where the range's
//!   last line maps (`start_line = line = nearest`), or with no lines when the
//!   current side has none (empty, binary or a submodule).
//!
//! Line numbers are 1-based here, as in the store; `LineMap` is 0-based.
//!
//! [`Core::positions`] caches results in `thread_positions` keyed by
//! `(thread, diff)` and [`CARRY_FORWARD_ENGINE_VERSION`]. A diff without a `diffs`
//! row (an unpinned live refresh) is computed without caching, since the table's
//! foreign key needs the row and live refreshes must never pin.

use std::collections::{HashMap, HashSet};

use polygloss_diff::line_map::{LineMap, MappedRange};
use polygloss_diff::{FileChange, FileKind, FileStatus, GitPath, ObjectFormat, Oid, Side};
use rusqlite::{Connection, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};

use crate::ids::DiffId;
use crate::objects::{BlobReader, is_binary};
use crate::review::threads::{parse_side, side_str, subject_from_columns};
use crate::review::{Core, CoreError, Subject, ThreadAnchor};
use crate::store::StoreError;

/// Version of the mapping engine stored with every cached position. Bump it when
/// a change to this module or to `LineMap` can change a result: rows of other
/// versions are then recomputed.
pub const CARRY_FORWARD_ENGINE_VERSION: i64 = 1;

/// `thread_positions.state` (design §8.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionState {
    /// Same lines of the same blob (also file and review subjects).
    Exact,
    /// Every anchored line is unchanged; the numbers may differ.
    Moved,
    /// An anchored line changed; shown at the nearest line with the original
    /// snippet.
    Outdated,
    /// The file (or its anchored side) is not in this diff: threads panel only.
    Absent,
}

impl PositionState {
    /// The column text.
    pub fn as_str(&self) -> &'static str {
        match self {
            PositionState::Exact => "exact",
            PositionState::Moved => "moved",
            PositionState::Outdated => "outdated",
            PositionState::Absent => "absent",
        }
    }

    /// Parses the column text.
    pub fn parse(s: &str) -> Option<PositionState> {
        match s {
            "exact" => Some(PositionState::Exact),
            "moved" => Some(PositionState::Moved),
            "outdated" => Some(PositionState::Outdated),
            "absent" => Some(PositionState::Absent),
            _ => None,
        }
    }
}

/// Where a thread shows in one diff. `path` is the file's display path in that
/// diff (so a renamed file's new name); `side` is the anchored side; lines are
/// 1-based and inclusive. Review subjects and absent threads have no path; file
/// subjects and outdated threads on a side without lines have no lines.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Position {
    pub state: PositionState,
    pub path: Option<String>,
    pub side: Option<Side>,
    pub start_line: Option<u32>,
    pub line: Option<u32>,
}

impl Position {
    fn bare(state: PositionState, path: Option<String>) -> Position {
        Position {
            state,
            path,
            side: None,
            start_line: None,
            line: None,
        }
    }
}

/// The position of a thread anchored at `anchor` in the diff whose files are
/// `files` (design §8.6). `blobs` must read that diff's blobs (for a live diff,
/// through its scratch store) and the anchor blob. A missing blob is
/// `Objects(Missing)`.
pub fn position_of(
    anchor: &ThreadAnchor,
    files: &[FileChange],
    blobs: &BlobReader,
) -> Result<Position, CoreError> {
    let (path, side, start_line, line) = match &anchor.subject {
        Subject::Review => return Ok(Position::bare(PositionState::Exact, None)),
        Subject::File { path } => {
            return Ok(match current_file(files, path) {
                Some(f) => Position::bare(PositionState::Exact, Some(f.display_path().to_owned())),
                None => Position::bare(PositionState::Absent, None),
            });
        }
        Subject::Line {
            path,
            side,
            start_line,
            line,
        } => (path, *side, *start_line, *line),
    };
    let absent = Position::bare(PositionState::Absent, None);
    let Some(file) = current_file(files, path) else {
        return Ok(absent);
    };
    let current = match (side, file.status) {
        (Side::Old, FileStatus::Added) | (Side::New, FileStatus::Deleted) => return Ok(absent),
        (Side::Old, _) => &file.old_blob,
        (Side::New, _) => &file.new_blob,
    };
    let Some(anchor_blob) = &anchor.anchor_blob else {
        return Err(
            StoreError::Integrity(format!("line thread on {path} has no anchor blob")).into(),
        );
    };
    let at = |state: PositionState, lines: Option<(u32, u32)>| Position {
        state,
        path: Some(file.display_path().to_owned()),
        side: Some(side),
        start_line: lines.map(|l| l.0),
        line: lines.map(|l| l.1),
    };
    if current == anchor_blob {
        return Ok(at(PositionState::Exact, Some((start_line, line))));
    }
    if matches!(file.kind, FileKind::Binary | FileKind::Submodule) {
        return Ok(at(PositionState::Outdated, None));
    }
    let new = blobs.read(current)?;
    let old = blobs.read(anchor_blob)?;
    if is_binary(&new) || is_binary(&old) {
        return Ok(at(PositionState::Outdated, None));
    }
    let map = LineMap::new(&old, &new);
    Ok(
        match map.map_range(start_line.saturating_sub(1), line.saturating_sub(1)) {
            MappedRange::Moved { start, end } => {
                at(PositionState::Moved, Some((start + 1, end + 1)))
            }
            MappedRange::Outdated { .. } if map.new_len() == 0 => at(PositionState::Outdated, None),
            MappedRange::Outdated { nearest } => {
                at(PositionState::Outdated, Some((nearest + 1, nearest + 1)))
            }
        },
    )
}

/// The file change a thread on `path` lands on: by new path, else a renamed or
/// deleted file by its old path.
fn current_file<'a>(files: &'a [FileChange], path: &str) -> Option<&'a FileChange> {
    let is = |p: &Option<GitPath>| p.as_ref().is_some_and(|p| p.text == path);
    files.iter().find(|f| is(&f.new_path)).or_else(|| {
        files.iter().find(|f| {
            matches!(f.status, FileStatus::Renamed | FileStatus::Deleted) && is(&f.old_path)
        })
    })
}

impl Core {
    /// Positions of `thread_ids` in the diff `diff_id` whose files are `files`
    /// (see [`position_of`] for `blobs`). Unknown thread ids are left out of the
    /// map (a thread deleted since the caller listed it).
    ///
    /// When the diff is stored, results come from `thread_positions` rows of the
    /// current [`CARRY_FORWARD_ENGINE_VERSION`] (then `files` and `blobs` are not
    /// used), and computed results are written back; a row of an older engine is
    /// replaced, one of a newer engine (a newer build) is ignored but kept. An
    /// unstored diff (an unpinned live refresh) is computed every time and never
    /// pinned. The cache write appends no event (derived state, like view
    /// state); if it fails, the computed positions are still returned.
    pub fn positions(
        &self,
        diff_id: &DiffId,
        files: &[FileChange],
        thread_ids: &[String],
        blobs: &BlobReader,
    ) -> Result<HashMap<String, Position>, CoreError> {
        let mut seen = HashSet::new();
        let ids: Vec<&String> = thread_ids
            .iter()
            .filter(|id| seen.insert(id.as_str()))
            .collect();
        let (stored, pending, mut out) = self.store.read(|c| {
            let stored: bool = c.query_row(
                "SELECT EXISTS (SELECT 1 FROM diffs WHERE id = ?1)",
                [diff_id.as_str()],
                |r| r.get(0),
            )?;
            let mut pending = Vec::new();
            let mut out = HashMap::new();
            for id in &ids {
                if stored && let Some(p) = cached_position(c, id, diff_id)? {
                    out.insert((*id).clone(), p);
                    continue;
                }
                if let Some(anchor) = load_anchor(c, id)? {
                    pending.push(((*id).clone(), anchor));
                }
            }
            Ok((stored, pending, out))
        })?;

        let mut computed = Vec::with_capacity(pending.len());
        for (id, anchor) in pending {
            computed.push((id, position_of(&anchor, files, blobs)?));
        }
        if stored && !computed.is_empty() {
            let write = self.store.write(|tx| {
                for (id, p) in &computed {
                    cache_position(tx, id, diff_id, p)?;
                }
                Ok(())
            });
            if let Err(e) = write {
                tracing::warn!(diff_id = diff_id.as_str(), error = %e, "caching thread positions failed");
            }
        }
        out.extend(computed);
        Ok(out)
    }
}

/// The cached position of `thread_id` in `diff_id` from the current engine.
fn cached_position(
    c: &Connection,
    thread_id: &str,
    diff_id: &DiffId,
) -> Result<Option<Position>, StoreError> {
    let row = c
        .prepare_cached(
            "SELECT state, path, side, start_line, line FROM thread_positions \
             WHERE thread_id = ?1 AND diff_id = ?2 AND engine_version = ?3",
        )?
        .query_row(
            params![thread_id, diff_id.as_str(), CARRY_FORWARD_ENGINE_VERSION],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<u32>>(3)?,
                    r.get::<_, Option<u32>>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((state, path, side, start_line, line)) = row else {
        return Ok(None);
    };
    let bad = |what: &str, v: &str| {
        StoreError::Integrity(format!(
            "thread_positions ({thread_id}, {diff_id}): {what} {v:?}"
        ))
    };
    Ok(Some(Position {
        state: PositionState::parse(&state).ok_or_else(|| bad("state", &state))?,
        path,
        side: side
            .map(|s| parse_side(&s).ok_or_else(|| bad("side", &s)))
            .transpose()?,
        start_line,
        line,
    }))
}

/// Writes a computed position unless a newer engine's row is there. Skips a
/// thread or diff deleted since the positions were read.
fn cache_position(
    tx: &Transaction,
    thread_id: &str,
    diff_id: &DiffId,
    p: &Position,
) -> Result<(), StoreError> {
    tx.prepare_cached(
        "INSERT INTO thread_positions \
           (thread_id, diff_id, state, path, side, start_line, line, engine_version) \
         SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8 \
         WHERE EXISTS (SELECT 1 FROM threads WHERE id = ?1) \
           AND EXISTS (SELECT 1 FROM diffs WHERE id = ?2) \
         ON CONFLICT (thread_id, diff_id) DO UPDATE SET \
           state = excluded.state, path = excluded.path, side = excluded.side, \
           start_line = excluded.start_line, line = excluded.line, \
           engine_version = excluded.engine_version \
         WHERE excluded.engine_version >= thread_positions.engine_version",
    )?
    .execute(params![
        thread_id,
        diff_id.as_str(),
        p.state.as_str(),
        p.path,
        p.side.map(side_str),
        p.start_line,
        p.line,
        CARRY_FORWARD_ENGINE_VERSION,
    ])?;
    Ok(())
}

/// The stored anchor of `thread_id`, or `None` for an unknown thread.
fn load_anchor(c: &Connection, thread_id: &str) -> Result<Option<ThreadAnchor>, StoreError> {
    let row = c
        .prepare_cached(
            "SELECT t.subject, t.path, t.side, t.start_line, t.line, t.anchor_blob, \
               t.anchor_snippet, d.object_format \
             FROM threads t JOIN diffs d ON d.id = t.origin_diff_id WHERE t.id = ?1",
        )?
        .query_row([thread_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<u32>>(3)?,
                r.get::<_, Option<u32>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, String>(7)?,
            ))
        })
        .optional()?;
    let Some((subject, path, side, start_line, line, blob, snippet, fmt)) = row else {
        return Ok(None);
    };
    let bad =
        |what: &str, v: &str| StoreError::Integrity(format!("thread {thread_id}: {what} {v:?}"));
    let subject = subject_from_columns(&subject, path, side, start_line, line)
        .map_err(|(what, v)| bad(what, &v))?;
    let fmt = ObjectFormat::from_name(&fmt).ok_or_else(|| bad("object format", &fmt))?;
    let anchor_blob = blob
        .map(|b| Oid::parse(&b, fmt).map_err(|_| bad("anchor_blob", &b)))
        .transpose()?;
    Ok(Some(ThreadAnchor {
        subject,
        anchor_blob,
        anchor_snippet: snippet,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn position_state_roundtrips_its_column_text() {
        for s in [
            PositionState::Exact,
            PositionState::Moved,
            PositionState::Outdated,
            PositionState::Absent,
        ] {
            assert_eq!(PositionState::parse(s.as_str()), Some(s));
            assert_eq!(
                serde_json::to_value(s).unwrap(),
                serde_json::Value::from(s.as_str())
            );
        }
        assert_eq!(PositionState::parse("gone"), None);
    }
}
