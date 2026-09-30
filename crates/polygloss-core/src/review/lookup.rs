//! Id lookups against the store alone (T4.5): the agent surface resolves a
//! user-typed diff id prefix even when no repo on disk has the diff's trees
//! (listing its threads needs no objects). [`Core::find_repo_for_diff`] is the
//! variant that also finds a repo.

use rusqlite::params;

use crate::ids::{DiffId, DiffIdPrefix};
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
}
