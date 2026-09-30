//! Id lookups against the store alone (T4.5): the agent surface resolves a
//! user-typed diff id prefix even when no repo on disk has the diff's trees
//! (listing its threads needs no objects). [`Core::find_repo_for_diff`] is the
//! variant that also finds a repo. `wait_for_review` (T4.7) reads a
//! submission by id and whether a review is archived.

use rusqlite::{OptionalExtension as _, params};

use crate::ids::{DiffId, DiffIdPrefix};
use crate::review::models::read_iteration;
use crate::review::submit::{Submission, Verdict};
use crate::review::{Core, CoreError};
use crate::store::StoreError;

/// How many matches an ambiguous prefix lists.
const MAX_MATCHES: i64 = 20;

impl Core {
    /// The stored diff whose id starts with `id_or_prefix` (8 to 64 hex chars,
    /// any case). A malformed or unknown prefix is `NotFound` (`Id` for bad
    /// syntax), one matching several diffs `Ambiguous` with the matches.
    pub fn resolve_diff_prefix(&self, id_or_prefix: &str) -> Result<DiffId, CoreError> {
        let prefix = DiffIdPrefix::parse(id_or_prefix.trim())?;
        let matches: Vec<String> = self.store.read(|c| {
            let mut stmt = c.prepare_cached(
                "SELECT id FROM diffs WHERE substr(id, 1, ?1) = ?2 ORDER BY id LIMIT ?3",
            )?;
            let rows = stmt
                .query_map(
                    params![prefix.as_str().len() as i64, prefix.as_str(), MAX_MATCHES],
                    |r| r.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        match matches.as_slice() {
            [] => Err(CoreError::not_found("diff", prefix.as_str())),
            [id] => DiffId::parse(id)
                .map_err(|_| StoreError::Integrity(format!("diff id {id:?}")).into()),
            _ => Err(CoreError::Ambiguous {
                prefix: prefix.as_str().to_owned(),
                matches,
            }),
        }
    }

    /// The submission `submission_id` with its iteration and the `seq` of its
    /// `review.submitted` event; `None` when there is no such submission.
    pub fn submission(&self, submission_id: &str) -> Result<Option<Submission>, CoreError> {
        Ok(self.store.read(|c| {
            let row = c
                .query_row(
                    "SELECT i.id, i.seq, i.diff_id, i.snapshot_ref, \
                       s.review_id, s.verdict, s.summary_md, s.comment_count, s.submitted_at, \
                       (SELECT e.seq FROM events e WHERE e.kind = 'review.submitted' \
                          AND e.review_id = s.review_id \
                          AND json_extract(e.payload, '$.submission_id') = s.id) \
                     FROM review_submissions s JOIN iterations i ON i.id = s.iteration_id \
                     WHERE s.id = ?1",
                    [submission_id],
                    |r| {
                        Ok((
                            read_iteration(r),
                            r.get::<_, String>(4)?,
                            r.get::<_, String>(5)?,
                            r.get::<_, String>(6)?,
                            r.get::<_, u32>(7)?,
                            r.get::<_, i64>(8)?,
                            r.get::<_, Option<i64>>(9)?,
                        ))
                    },
                )
                .optional()?;
            let Some((iteration, review_id, verdict, summary_md, comment_count, at, seq)) = row
            else {
                return Ok(None);
            };
            let verdict = Verdict::parse(&verdict).ok_or_else(|| {
                StoreError::Integrity(format!("submission {submission_id} verdict {verdict:?}"))
            })?;
            let seq = seq.ok_or_else(|| {
                StoreError::Integrity(format!("submission {submission_id} has no event"))
            })?;
            Ok(Some(Submission {
                id: submission_id.to_owned(),
                review_id,
                iteration: iteration?,
                verdict,
                summary_md,
                comment_count,
                submitted_at: at,
                seq,
            }))
        })?)
    }

    /// Whether review `review_id` is archived (`NotFound` when there is no such
    /// review, e.g. after it was pruned).
    pub fn review_archived(&self, review_id: &str) -> Result<bool, CoreError> {
        let archived_at: Option<Option<i64>> = self.store.read(|c| {
            Ok(c.query_row(
                "SELECT archived_at FROM reviews WHERE id = ?1",
                [review_id],
                |r| r.get(0),
            )
            .optional()?)
        })?;
        match archived_at {
            Some(at) => Ok(at.is_some()),
            None => Err(CoreError::not_found("review", review_id)),
        }
    }
}
