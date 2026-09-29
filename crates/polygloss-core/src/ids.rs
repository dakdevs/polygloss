//! Identifiers: `diff_id` (design §4.1, OQ-1), id prefixes, UUIDv7 ids (§4.4,
//! OQ-P2) and review keys (§4.2, OQ-3).

use std::fmt;

use polygloss_diff::{ObjectFormat, Oid};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Hex chars shown in the UI for a `diff_id` (§4.1 "Display").
const DISPLAY_LEN: usize = 12;
/// Shortest `diff_id` prefix accepted as input (§4.1, provisional).
const MIN_PREFIX_LEN: usize = 8;
/// Full `diff_id` length: lowercase hex of a sha256.
const DIFF_ID_LEN: usize = 64;

/// Why a string is not a `DiffId` or `DiffIdPrefix`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IdError {
    #[error("diff id {0:?} is not 64 lowercase hex chars")]
    InvalidDiffId(String),
    #[error("diff id prefix needs at least {MIN_PREFIX_LEN} hex chars, got {0}")]
    PrefixTooShort(usize),
    #[error("diff id prefix {0:?} is not hex or longer than 64 chars")]
    NotHex(String),
}

/// A diff's identity: 64 lowercase hex chars (design §4.1).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DiffId(String);

/// `lowercase_hex(sha256("polygloss/diff/v1\n" + objfmt + "\n" + base_tree + "\n" + head_tree))`.
/// Depends on the two trees only: commits, refs, repos and view options never feed it.
pub fn diff_id(fmt: ObjectFormat, base_tree: &Oid, head_tree: &Oid) -> DiffId {
    let mut h = Sha256::new();
    h.update(b"polygloss/diff/v1\n");
    h.update(fmt.as_str().as_bytes());
    h.update(b"\n");
    h.update(base_tree.as_str().as_bytes());
    h.update(b"\n");
    h.update(head_tree.as_str().as_bytes());
    DiffId(hex::encode(h.finalize()))
}

impl DiffId {
    /// Validates a full stored id (64 lowercase hex chars).
    pub fn parse(s: &str) -> Result<DiffId, IdError> {
        if s.len() == DIFF_ID_LEN && is_lower_hex(s) {
            Ok(DiffId(s.to_owned()))
        } else {
            Err(IdError::InvalidDiffId(s.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The 12-char display form.
    pub fn short(&self) -> &str {
        &self.0[..DISPLAY_LEN]
    }
}

impl fmt::Display for DiffId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl TryFrom<String> for DiffId {
    type Error = IdError;

    fn try_from(s: String) -> Result<DiffId, IdError> {
        DiffId::parse(&s)
    }
}

impl From<DiffId> for String {
    fn from(id: DiffId) -> String {
        id.0
    }
}

/// A user-typed `diff_id` prefix: 8 to 64 hex chars, normalized to lower case.
/// Uniqueness is checked where ids are looked up (T1.12).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DiffIdPrefix(String);

impl DiffIdPrefix {
    pub fn parse(s: &str) -> Result<DiffIdPrefix, IdError> {
        if s.len() > DIFF_ID_LEN || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(IdError::NotHex(s.to_owned()));
        }
        if s.len() < MIN_PREFIX_LEN {
            return Err(IdError::PrefixTooShort(s.len()));
        }
        Ok(DiffIdPrefix(s.to_ascii_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn matches(&self, id: &DiffId) -> bool {
        id.as_str().starts_with(&self.0)
    }
}

/// A new UUIDv7 in hyphenated lower-case text form (§4.4, OQ-P2).
pub fn new_uuid() -> String {
    uuid::Uuid::now_v7().to_string()
}

fn is_lower_hex(s: &str) -> bool {
    s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Review keys: reviews are unique per `(repo_id, key)` (design §4.2, OQ-3).
pub mod review_key {
    use std::path::Path;

    use polygloss_diff::Oid;

    /// `worktree:<worktree>@<branch>#since=<since>`. `worktree` is the canonical
    /// worktree root; `branch` is the short branch name, `None` on a detached HEAD
    /// (`@detached`); `since` is `merge-base`, `HEAD` or a full commit OID.
    pub fn live(worktree: &Path, branch: Option<&str>, since: &str) -> String {
        format!(
            "worktree:{}@{}#since={since}",
            worktree.display(),
            branch.unwrap_or("detached")
        )
    }

    /// `compare:<base>...<head>` (three-dot) or `compare:<base>..<head>` (`direct`).
    /// Refs are full names; non-ref inputs are commit OIDs.
    pub fn compare(base_ref: &str, head_ref: &str, direct: bool) -> String {
        let dots = if direct { ".." } else { "..." };
        format!("compare:{base_ref}{dots}{head_ref}")
    }

    /// `commit:<commit_oid>` (provisional, OQ-2).
    pub fn commit(oid: &Oid) -> String {
        format!("commit:{oid}")
    }
}
