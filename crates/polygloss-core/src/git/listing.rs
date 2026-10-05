//! Ref and commit listings for the open flow (T3.5, design §11.3): the branches,
//! remote branches and tags a compare can name ([`list_refs`]), and the commit
//! log a commit review is picked from ([`list_commits`], paged). The header card
//! (design §11.6) reads one commit ([`commit_details`]) and a compare's commits
//! ([`range_commits`]).
//!
//! Both read git's output as NUL-separated fields, so subjects and names come
//! back exactly (tabs, quotes, non-ASCII). `for-each-ref` has no `-z`: each
//! field ends with `%00` and git ends each record with a newline, which no ref
//! name can hold (git-check-ref-format), so the newline is dropped from the next
//! record's first field. Nothing here writes to the repo.

use std::ffi::OsStr;

use polygloss_diff::{ObjectFormat, Oid};
use serde::{Deserialize, Serialize};

use crate::git::runner::{Git, GitError};

/// What kind of ref a [`RefInfo`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefKind {
    /// `refs/heads/*`.
    Branch,
    /// `refs/remotes/*`.
    RemoteBranch,
    /// `refs/tags/*`.
    Tag,
}

impl RefKind {
    fn from_name(name: &str) -> Option<(RefKind, &str)> {
        [
            ("refs/heads/", RefKind::Branch),
            ("refs/remotes/", RefKind::RemoteBranch),
            ("refs/tags/", RefKind::Tag),
        ]
        .into_iter()
        .find_map(|(prefix, kind)| name.strip_prefix(prefix).map(|short| (kind, short)))
    }
}

/// A branch, remote branch or tag that points (after peeling tags) at a
/// commit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RefInfo {
    /// The full name (`refs/heads/feature/login`); what a compare stores.
    pub name: String,
    /// The name people write (`feature/login`, `origin/main`, `v1.2.0`).
    pub short_name: String,
    pub kind: RefKind,
    /// The commit it points at (annotated tags peeled).
    pub commit: Oid,
    /// That commit's committer date, Unix seconds.
    pub committed_at: i64,
    /// That commit's subject line.
    pub subject: String,
    /// The branch HEAD is on.
    pub is_head: bool,
}

/// One commit of the log.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommitInfo {
    pub oid: Oid,
    pub tree: Oid,
    /// First parent first; empty for a root commit.
    pub parents: Vec<Oid>,
    /// The author's name (`%an`, no mailmap).
    pub author: String,
    /// Author date, Unix seconds.
    pub authored_at: i64,
    pub subject: String,
}

/// A commit as the header card shows it (design §11.6).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CommitDetails {
    pub oid: Oid,
    pub subject: String,
    /// `%an`, no mailmap.
    pub author_name: String,
    /// `%ae` as written (the avatar's color lowercases it).
    pub author_email: String,
    /// Committer date, Unix seconds.
    pub committed_at: i64,
}

const REF_FORMAT: &str = concat!(
    "--format=",
    "%(refname)%00",
    "%(objecttype)%00",
    "%(objectname)%00",
    "%(*objecttype)%00",
    "%(*objectname)%00",
    "%(committerdate:unix)%00",
    "%(*committerdate:unix)%00",
    "%(subject)%00",
    "%(*subject)%00",
    "%(HEAD)%00",
    "%(symref)%00",
);
const REF_FIELDS: usize = 11;

/// Every branch, remote branch and tag that names a commit: branches first,
/// then remote branches, then tags, the most recently committed first inside
/// each kind (ties by name). Symbolic refs (`origin/HEAD`), refs whose name is
/// not UTF-8 and tags of anything but a commit (or of a tag of a tag) are left
/// out. A repo without refs lists nothing.
pub fn list_refs(git: &Git) -> Result<Vec<RefInfo>, GitError> {
    let out = git.output(&[
        OsStr::new("for-each-ref"),
        OsStr::new(REF_FORMAT),
        OsStr::new("refs/heads/"),
        OsStr::new("refs/remotes/"),
        OsStr::new("refs/tags/"),
    ])?;
    let mut refs = parse_refs(&out)?;
    refs.sort_by(|a, b| {
        (a.kind, std::cmp::Reverse(a.committed_at), &a.name).cmp(&(
            b.kind,
            std::cmp::Reverse(b.committed_at),
            &b.name,
        ))
    });
    Ok(refs)
}

fn parse_refs(out: &[u8]) -> Result<Vec<RefInfo>, GitError> {
    let mut fields: Vec<&[u8]> = out.split(|&b| b == 0).collect();
    // The output ends with the last record's newline after its last NUL.
    match fields.pop() {
        Some(rest) if rest.iter().all(|&b| b == b'\n') => {}
        _ => return Err(GitError::Parse("for-each-ref: truncated output".into())),
    }
    if !fields.len().is_multiple_of(REF_FIELDS) {
        return Err(GitError::Parse(format!(
            "for-each-ref: {} fields is not a multiple of {REF_FIELDS}",
            fields.len()
        )));
    }
    let mut refs = Vec::with_capacity(fields.len() / REF_FIELDS);
    for record in fields.chunks(REF_FIELDS) {
        let name = record[0].strip_prefix(b"\n").unwrap_or(record[0]);
        let Ok(name) = std::str::from_utf8(name) else {
            tracing::debug!(
                "skipping a ref whose name is not UTF-8: {}",
                String::from_utf8_lossy(name)
            );
            continue;
        };
        let Some((kind, short)) = RefKind::from_name(name) else {
            continue;
        };
        if !record[10].is_empty() {
            continue; // a symref such as refs/remotes/origin/HEAD
        }
        let (commit, date, subject) = match (record[1], record[3]) {
            (b"commit", _) => (record[2], record[5], record[7]),
            (b"tag", b"commit") => (record[4], record[6], record[8]),
            _ => continue,
        };
        let commit = parse_oid(commit)?;
        refs.push(RefInfo {
            name: name.to_owned(),
            short_name: short.to_owned(),
            kind,
            commit,
            committed_at: parse_i64(date)?,
            subject: String::from_utf8_lossy(subject).into_owned(),
            is_head: kind == RefKind::Branch && record[9] == b"*",
        });
    }
    Ok(refs)
}

const LOG_FORMAT: &str = "--format=%H%x00%T%x00%P%x00%an%x00%at%x00%s";
const LOG_FIELDS: usize = 6;

/// The commits reachable from HEAD in `git log` order (newest first), skipping
/// the first `skip` and returning at most `limit`. A repo whose HEAD has no
/// commits yet lists nothing.
pub fn list_commits(git: &Git, skip: u32, limit: u32) -> Result<Vec<CommitInfo>, GitError> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let skip = format!("--skip={skip}");
    let limit = format!("--max-count={limit}");
    let args = [
        OsStr::new("log"),
        OsStr::new("-z"),
        OsStr::new("--no-show-signature"),
        OsStr::new("--no-notes"),
        OsStr::new(LOG_FORMAT),
        OsStr::new(&skip),
        OsStr::new(&limit),
        OsStr::new("--end-of-options"),
        OsStr::new("HEAD"),
        OsStr::new("--"),
    ];
    let out = git.run(&args)?;
    if !out.success() {
        if is_unborn(git)? {
            return Ok(Vec::new());
        }
        return Err(GitError::Failed {
            args: "log".into(),
            code: out.code,
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        });
    }
    parse_log(&out.stdout)
}

const DETAILS_FORMAT: &str = "--format=%H%x00%s%x00%an%x00%ae%x00%ct";
const DETAILS_FIELDS: usize = 5;

/// The commit `rev` names. An id that names no commit is an error.
pub fn commit_details(git: &Git, rev: &Oid) -> Result<CommitDetails, GitError> {
    let out = git.output(&details_args(&["-1", "--no-walk"], rev.as_str()))?;
    parse_details(&out)?
        .pop()
        .ok_or_else(|| GitError::Parse(format!("log: no commit {rev}")))
}

/// The newest `limit` commits of `base..head` (on `head` and not on `base`),
/// newest first, and how many the range holds in all.
pub fn range_commits(
    git: &Git,
    base: &Oid,
    head: &Oid,
    limit: u32,
) -> Result<(Vec<CommitDetails>, u32), GitError> {
    let range = format!("{base}..{head}");
    let count = git.output(&[
        OsStr::new("rev-list"),
        OsStr::new("--count"),
        OsStr::new("--end-of-options"),
        OsStr::new(&range),
        OsStr::new("--"),
    ])?;
    let total = u32::try_from(parse_i64(&count)?)
        .map_err(|_| GitError::Parse("rev-list --count: out of range".into()))?;
    if limit == 0 || total == 0 {
        return Ok((Vec::new(), total));
    }
    let max = format!("--max-count={limit}");
    let commits = parse_details(&git.output(&details_args(&[&max], &range))?)?;
    Ok((commits, total))
}

/// `log -z` of `rev` in [`DETAILS_FORMAT`], with `flags`.
fn details_args<'a>(flags: &[&'a str], rev: &'a str) -> Vec<&'a OsStr> {
    let mut args = vec![
        OsStr::new("log"),
        OsStr::new("-z"),
        OsStr::new("--no-show-signature"),
        OsStr::new("--no-notes"),
        OsStr::new(DETAILS_FORMAT),
    ];
    args.extend(flags.iter().map(|f| OsStr::new(*f)));
    args.extend([
        OsStr::new("--end-of-options"),
        OsStr::new(rev),
        OsStr::new("--"),
    ]);
    args
}

fn parse_details(out: &[u8]) -> Result<Vec<CommitDetails>, GitError> {
    log_fields(out, DETAILS_FIELDS)?
        .chunks(DETAILS_FIELDS)
        .map(|f| {
            Ok(CommitDetails {
                oid: parse_oid(f[0])?,
                subject: String::from_utf8_lossy(f[1]).into_owned(),
                author_name: String::from_utf8_lossy(f[2]).into_owned(),
                author_email: String::from_utf8_lossy(f[3]).into_owned(),
                committed_at: parse_i64(f[4])?,
            })
        })
        .collect()
}

/// Whether HEAD names no commit (a fresh `git init`).
fn is_unborn(git: &Git) -> Result<bool, GitError> {
    let code = git.status(&[
        OsStr::new("rev-parse"),
        OsStr::new("-q"),
        OsStr::new("--verify"),
        OsStr::new("HEAD^{commit}"),
    ])?;
    Ok(code != 0)
}

/// The fields of `log -z` output whose records have `n` fields each: `-z`
/// ends every record with a NUL, and the fields are NUL-separated.
fn log_fields(out: &[u8], n: usize) -> Result<Vec<&[u8]>, GitError> {
    if out.is_empty() {
        return Ok(Vec::new());
    }
    let body = out
        .strip_suffix(b"\0")
        .ok_or_else(|| GitError::Parse("log -z: output does not end with NUL".into()))?;
    let fields: Vec<&[u8]> = body.split(|&b| b == 0).collect();
    if !fields.len().is_multiple_of(n) {
        return Err(GitError::Parse(format!(
            "log -z: {} fields is not a multiple of {n}",
            fields.len()
        )));
    }
    Ok(fields)
}

fn parse_log(out: &[u8]) -> Result<Vec<CommitInfo>, GitError> {
    log_fields(out, LOG_FIELDS)?
        .chunks(LOG_FIELDS)
        .map(|f| {
            let parents = std::str::from_utf8(f[2])
                .map_err(|_| GitError::Parse("log: parents are not ASCII".into()))?
                .split_ascii_whitespace()
                .map(|p| parse_oid(p.as_bytes()))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(CommitInfo {
                oid: parse_oid(f[0])?,
                tree: parse_oid(f[1])?,
                parents,
                author: String::from_utf8_lossy(f[3]).into_owned(),
                authored_at: parse_i64(f[4])?,
                subject: String::from_utf8_lossy(f[5]).into_owned(),
            })
        })
        .collect()
}

/// An object id in either format (told apart by its length).
fn parse_oid(bytes: &[u8]) -> Result<Oid, GitError> {
    let text =
        std::str::from_utf8(bytes).map_err(|_| GitError::Parse("object id is not ASCII".into()))?;
    let fmt = if text.len() == ObjectFormat::Sha256.hex_len() {
        ObjectFormat::Sha256
    } else {
        ObjectFormat::Sha1
    };
    Oid::parse(text, fmt).map_err(|e| GitError::Parse(e.to_string()))
}

fn parse_i64(bytes: &[u8]) -> Result<i64, GitError> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .ok_or_else(|| {
            GitError::Parse(format!(
                "expected a number, got {:?}",
                String::from_utf8_lossy(bytes)
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";

    #[test]
    fn parse_refs_drops_record_newlines_and_symrefs() {
        let out = format!(
            "refs/heads/a b\0commit\0{A}\0\0\0100\0\0subj\0\0*\0\0\n\
             refs/remotes/o/HEAD\0commit\0{A}\0\0\0100\0\0subj\0\0 \0refs/remotes/o/main\0\n\
             refs/tags/t\0tag\0{B}\0commit\0{A}\0\0100\0tag msg\0subj\0 \0\0\n"
        );
        let refs = parse_refs(out.as_bytes()).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].name, "refs/heads/a b");
        assert!(refs[0].is_head);
        assert_eq!(refs[1].name, "refs/tags/t");
        assert_eq!(refs[1].commit.as_str(), A);
        assert_eq!(refs[1].subject, "subj");
        assert!(parse_refs(b"refs/heads/x\0commit").is_err());
    }

    #[test]
    fn parse_log_rejects_ragged_output() {
        assert!(parse_log(b"").unwrap().is_empty());
        assert!(parse_log(format!("{A}\0{B}\0\0n\0").as_bytes()).is_err());
        let one = format!("{A}\0{B}\0\0n\0\x31\0s\0");
        assert_eq!(parse_log(one.as_bytes()).unwrap()[0].authored_at, 1);
    }
}
