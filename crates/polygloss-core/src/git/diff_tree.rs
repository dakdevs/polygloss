//! `diff-tree -z --raw` parsing into `FileChange`s (T1.3, design §6.1, §6.2).
//!
//! Git produces the file list and rename pairing; we only parse it. The flags pin
//! everything that could vary with user config (`-M50% -l1000`, no external diff
//! or textconv, full ids), and `-z` keeps every path byte exact: spaces, quotes,
//! tabs, newlines and non-UTF-8 bytes (RF1). Non-UTF-8 paths become escaped
//! `GitPath`s (OQ-25).
//!
//! `parse_raw_z` sets `kind` from the modes only (submodule `160000`, symlink
//! `120000`, else text); `attrs::classify` then applies attributes and the
//! generated list, and the NUL-byte rule runs when a blob is first read (T1.4).

use std::ffi::OsStr;

use polygloss_diff::{
    FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, Mode, ObjectFormat, Oid,
};

use crate::git::runner::{Git, GitError};

/// Malformed `diff-tree -z --raw` output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("diff-tree output at byte {offset}: {reason}")]
pub struct ParseError {
    /// Byte offset of the record (or field) that failed.
    pub offset: usize,
    pub reason: String,
}

impl From<ParseError> for GitError {
    fn from(e: ParseError) -> GitError {
        GitError::Parse(e.to_string())
    }
}

/// Lists the changed files between two trees, in git's output order (`idx`).
///
/// Runs `diff-tree -r -z --raw -M50% -l1000 --no-ext-diff --no-textconv
/// --full-index <base> <head>`. Copies are never detected (no `-C`). Rename
/// settings come only from these flags, never from `diff.renames` or
/// `diff.renameLimit`. `kind` is mode-based and `generated` is false until
/// `attrs::classify` runs.
pub fn list_changes(
    git: &Git,
    fmt: ObjectFormat,
    base_tree: &Oid,
    head_tree: &Oid,
) -> Result<Vec<FileChange>, GitError> {
    let args = [
        "diff-tree",
        "-r",
        "-z",
        "--raw",
        "-M50%",
        "-l1000",
        "--no-ext-diff",
        "--no-textconv",
        "--full-index",
        base_tree.as_str(),
        head_tree.as_str(),
    ];
    let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
    let out = git.output(&args)?;
    Ok(parse_raw_z(&out, fmt)?)
}

/// Parses `diff-tree -r -z --raw` output between two trees. Each record is
/// `:<old_mode> <new_mode> <old_oid> <new_oid> <status>[<score>]` NUL `<path>` NUL,
/// with a second path (source then destination) for renames. Copies (`C`),
/// unmerged (`U`) and unknown (`X`) statuses are errors: they cannot come from the
/// flags `list_changes` uses.
pub fn parse_raw_z(bytes: &[u8], fmt: ObjectFormat) -> Result<Vec<FileChange>, ParseError> {
    let mut fields = Fields { bytes, pos: 0 };
    let mut out = Vec::new();
    while fields.pos < bytes.len() {
        let start = fields.pos;
        let header = fields
            .next()
            .ok_or_else(|| err(start, "unterminated record header"))?;
        let rec = parse_header(header, fmt).map_err(|reason| err(start, &reason))?;
        let mut path = || {
            let at = fields.pos;
            match fields.next() {
                Some(p) if !p.is_empty() => Ok(GitPath::from_bytes(p)),
                Some(_) => Err(err(at, "empty path")),
                None => Err(err(at, "missing path")),
            }
        };
        let first = path()?;
        let (old_path, new_path) = match rec.status {
            FileStatus::Added => (None, Some(first)),
            FileStatus::Deleted => (Some(first), None),
            FileStatus::Renamed => (Some(first), Some(path()?)),
            FileStatus::Modified | FileStatus::TypeChanged => (Some(first.clone()), Some(first)),
        };
        let idx = u32::try_from(out.len()).map_err(|_| err(start, "too many files"))?;
        out.push(FileChange {
            idx,
            status: rec.status,
            kind: kind_for(rec.old_mode, rec.new_mode),
            old_path,
            new_path,
            old_mode: rec.old_mode,
            new_mode: rec.new_mode,
            old_blob: rec.old_blob,
            new_blob: rec.new_blob,
            similarity: rec.similarity,
            generated: false,
            generated_attr: GeneratedAttr::Unspecified,
        });
    }
    Ok(out)
}

/// Mode-based kind: a submodule on either side wins over a symlink, which wins
/// over text. A type change keeps the special kind, so its header shows the badge.
fn kind_for(old: Option<Mode>, new: Option<Mode>) -> FileKind {
    let modes = [old, new];
    if modes.iter().flatten().any(Mode::is_submodule) {
        FileKind::Submodule
    } else if modes.iter().flatten().any(Mode::is_symlink) {
        FileKind::Symlink
    } else {
        FileKind::Text
    }
}

struct Header {
    status: FileStatus,
    old_mode: Option<Mode>,
    new_mode: Option<Mode>,
    old_blob: Oid,
    new_blob: Oid,
    similarity: Option<u8>,
}

fn parse_header(header: &[u8], fmt: ObjectFormat) -> Result<Header, String> {
    // ASCII only, so slicing the status below can never split a character.
    let text = std::str::from_utf8(header)
        .ok()
        .filter(|t| t.is_ascii())
        .ok_or_else(|| "record header is not ASCII".to_owned())?;
    let body = text
        .strip_prefix(':')
        .ok_or_else(|| format!("record header {text:?} does not start with ':'"))?;
    let parts: Vec<&str> = body.split(' ').collect();
    let [old_mode, new_mode, old_oid, new_oid, status] = parts[..] else {
        return Err(format!("record header {text:?} does not have 5 fields"));
    };
    let mode = |s: &str| -> Result<Option<Mode>, String> {
        let m = Mode::parse_octal(s).ok_or_else(|| format!("bad mode {s:?}"))?;
        Ok((m.0 != 0).then_some(m))
    };
    let oid = |s: &str| Oid::parse(s, fmt).map_err(|e| e.to_string());
    let (letter, score) = status.split_at(status.len().min(1));
    let status_kind = letter
        .bytes()
        .next()
        .and_then(FileStatus::from_raw)
        .ok_or_else(|| format!("unsupported status {status:?}"))?;
    let similarity = match (status_kind, score) {
        (FileStatus::Renamed, s) => Some(
            s.parse::<u8>()
                .ok()
                .filter(|v| *v <= 100 && s.len() == 3)
                .ok_or_else(|| format!("bad rename score {status:?}"))?,
        ),
        (_, "") => None,
        (_, _) => return Err(format!("unexpected score in status {status:?}")),
    };
    let h = Header {
        status: status_kind,
        old_mode: mode(old_mode)?,
        new_mode: mode(new_mode)?,
        old_blob: oid(old_oid)?,
        new_blob: oid(new_oid)?,
        similarity,
    };
    let sides_ok = match h.status {
        FileStatus::Added => h.old_mode.is_none() && h.new_mode.is_some(),
        FileStatus::Deleted => h.old_mode.is_some() && h.new_mode.is_none(),
        _ => h.old_mode.is_some() && h.new_mode.is_some(),
    };
    if !sides_ok {
        return Err(format!("modes do not match status in {text:?}"));
    }
    Ok(h)
}

fn err(offset: usize, reason: &str) -> ParseError {
    ParseError {
        offset,
        reason: reason.to_owned(),
    }
}

/// NUL-terminated fields; a field without its terminating NUL is `None`.
struct Fields<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Fields<'a> {
    fn next(&mut self) -> Option<&'a [u8]> {
        let rest = &self.bytes[self.pos..];
        let end = rest.iter().position(|&b| b == 0)?;
        self.pos += end + 1;
        Some(&rest[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "ce013625030ba8dba906f756967f9e9ca394464a";
    const B: &str = "1c59427adc4b205a270d8f810310394962e79a8b";
    const Z: &str = "0000000000000000000000000000000000000000";

    fn rec(header: &str, paths: &[&[u8]]) -> Vec<u8> {
        let mut v = header.as_bytes().to_vec();
        v.push(0);
        for p in paths {
            v.extend_from_slice(p);
            v.push(0);
        }
        v
    }

    #[test]
    fn parse_empty_input_is_empty() {
        assert_eq!(parse_raw_z(b"", ObjectFormat::Sha1).unwrap(), vec![]);
    }

    #[test]
    fn parse_all_statuses_and_kinds() {
        let mut bytes = Vec::new();
        bytes.extend(rec(&format!(":000000 100644 {Z} {A} A"), &[b"add"]));
        bytes.extend(rec(&format!(":100644 000000 {A} {Z} D"), &[b"del"]));
        bytes.extend(rec(&format!(":100644 100755 {A} {B} M"), &[b"mod"]));
        bytes.extend(rec(
            &format!(":100644 100644 {A} {B} R087"),
            &[b"from", b"to"],
        ));
        bytes.extend(rec(&format!(":100644 120000 {A} {B} T"), &[b"tc"]));
        bytes.extend(rec(&format!(":160000 160000 {A} {B} M"), &[b"sub"]));
        let files = parse_raw_z(&bytes, ObjectFormat::Sha1).unwrap();
        assert_eq!(files.len(), 6);
        let statuses: Vec<_> = files.iter().map(|f| f.status).collect();
        assert_eq!(
            statuses,
            [
                FileStatus::Added,
                FileStatus::Deleted,
                FileStatus::Modified,
                FileStatus::Renamed,
                FileStatus::TypeChanged,
                FileStatus::Modified
            ]
        );
        assert_eq!(files[0].old_mode, None);
        assert!(files[0].old_blob.is_zero());
        assert_eq!(files[1].new_path, None);
        assert_eq!(files[3].similarity, Some(87));
        assert_eq!(files[3].old_path.as_ref().unwrap().text, "from");
        assert_eq!(files[3].new_path.as_ref().unwrap().text, "to");
        assert_eq!(files[4].kind, FileKind::Symlink);
        assert_eq!(files[5].kind, FileKind::Submodule);
        assert!(files.iter().enumerate().all(|(i, f)| f.idx as usize == i));
    }

    #[test]
    fn parse_sha256_ids() {
        let a = "a".repeat(64);
        let z = "0".repeat(64);
        let bytes = rec(&format!(":000000 100644 {z} {a} A"), &[b"x"]);
        let files = parse_raw_z(&bytes, ObjectFormat::Sha256).unwrap();
        assert_eq!(files[0].new_blob.as_str(), a);
        // The same bytes are not valid sha1 output.
        assert!(parse_raw_z(&bytes, ObjectFormat::Sha1).is_err());
    }

    #[test]
    fn parse_rejects_malformed_records() {
        let fmt = ObjectFormat::Sha1;
        let cases: Vec<(&str, Vec<u8>)> = vec![
            (
                "no colon",
                rec(&format!("100644 100644 {A} {B} M"), &[b"p"]),
            ),
            (
                "copy",
                rec(&format!(":100644 100644 {A} {B} C100"), &[b"a", b"b"]),
            ),
            (
                "unmerged",
                rec(&format!(":100644 100644 {A} {B} U"), &[b"p"]),
            ),
            (
                "bad score",
                rec(&format!(":100644 100644 {A} {B} R1x0"), &[b"a", b"b"]),
            ),
            (
                "score > 100",
                rec(&format!(":100644 100644 {A} {B} R101"), &[b"a", b"b"]),
            ),
            (
                "score on M",
                rec(&format!(":100644 100644 {A} {B} M050"), &[b"p"]),
            ),
            (
                "short oid",
                rec(&format!(":100644 100644 {} {B} M", &A[..7]), &[b"p"]),
            ),
            (
                "bad mode",
                rec(&format!(":10064x 100644 {A} {B} M"), &[b"p"]),
            ),
            (
                "add with old mode",
                rec(&format!(":100644 100644 {Z} {A} A"), &[b"p"]),
            ),
            (
                "too few fields",
                rec(&format!(":100644 {A} {B} M"), &[b"p"]),
            ),
            (
                "empty path",
                rec(&format!(":100644 100644 {A} {B} M"), &[b""]),
            ),
            (
                "missing rename dst",
                rec(&format!(":100644 100644 {A} {B} R100"), &[b"a"]),
            ),
            (
                "non-ascii status",
                rec(&format!(":100644 100644 {A} {B} \u{e9}"), &[b"p"]),
            ),
            (
                "unterminated header",
                format!(":100644 100644 {A} {B} M").into_bytes(),
            ),
            ("unterminated path", {
                let mut v = rec(&format!(":100644 100644 {A} {B} M"), &[]);
                v.extend_from_slice(b"path-without-nul");
                v
            }),
        ];
        for (name, bytes) in cases {
            let e = parse_raw_z(&bytes, fmt).expect_err(name);
            assert!(e.offset <= bytes.len(), "{name}: {e}");
        }
        // Offsets point at the failing record, not the start of the input.
        let mut two = rec(&format!(":100644 100644 {A} {B} M"), &[b"ok"]);
        let second = two.len();
        two.extend(rec(&format!(":100644 100644 {A} {B} X"), &[b"bad"]));
        assert_eq!(parse_raw_z(&two, fmt).unwrap_err().offset, second);
    }

    #[test]
    fn parse_error_maps_to_git_parse_error() {
        let e: GitError = err(3, "boom").into();
        assert!(matches!(e, GitError::Parse(ref m) if m.contains("byte 3") && m.contains("boom")));
    }
}
