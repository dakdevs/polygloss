//! Binary and generated detection from attributes and the built-in list (T1.3,
//! design §6.1, §6.4, OQ-P3).
//!
//! Attributes come from the **head tree**, never the worktree, via one batched
//! `git check-attr -z --stdin --source=<head_tree> binary diff linguist-generated`
//! (git ≥ 2.40). The user's `$GIT_DIR/info/attributes` and global
//! `core.attributesFile` still apply, as in git. On git 2.39 (no `--source`) only
//! the built-in generated list, the caller's extra patterns and the NUL-byte rule
//! (at first blob read, T1.4) remain, and a one-time notice is logged.
//!
//! Rules:
//! - `kind`: a text file becomes `Binary` when `binary` is set or `diff` is unset
//!   (`-diff`). Symlinks and submodules keep their kind.
//! - `generated_attr`: `linguist-generated` / `linguist-generated=true` is `Set`,
//!   `-linguist-generated` / `linguist-generated=false` is `Unset`, anything else
//!   (including git without `--source`) is `Unspecified` (T6.1, design §11.15).
//! - `generated`: `Set` sets it and `Unset` clears it (both override the lists, as
//!   on GitHub); otherwise it is set when the path matches `BUILTIN_GENERATED` or
//!   an extra pattern (`diff.generated_patterns`). Store v1 kept only this bit.
//!
//! Patterns (built-in and extra) are a gitignore-like subset matched against the raw
//! path bytes: no `/` → match the file name; with a `/` → match the whole path (a
//! leading `/` is ignored). `*` and `?` never cross `/`; `**` crosses `/`, and
//! `**/` also matches no directory at all. There are no character classes.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::PathBuf;
use std::sync::{Mutex, Once, PoisonError};

use polygloss_diff::{FileChange, FileKind, GeneratedAttr, Oid};

use crate::git::runner::{Git, GitError, git_binary};
use crate::git::version::{GitVersion, check_version};

/// Built-in generated-file patterns (design §6.4): lockfiles by name, minified and
/// source-map files, protobuf output.
///
/// Frozen: store v1 rows kept only the `generated` bit computed with this list, and
/// [`legacy_attr`] recovers their attribute from it. Changing it would change how
/// stored v1 rows read.
pub const BUILTIN_GENERATED: &[&str] = &[
    "package-lock.json",
    "yarn.lock",
    "pnpm-lock.yaml",
    "bun.lock",
    "bun.lockb",
    "Cargo.lock",
    "Gemfile.lock",
    "poetry.lock",
    "composer.lock",
    "go.sum",
    "*.min.js",
    "*.min.css",
    "*.map",
    "*.pb.go",
    "*_pb2.py",
];

/// The attributes `classify` reads.
const ATTRS: [&str; 3] = ["binary", "diff", "linguist-generated"];

/// Classifies `changes` in place: `Binary` kind from attributes and `generated`
/// from attributes, `BUILTIN_GENERATED` and `extra_generated`. Paths are looked up
/// by the new path (the old one for deletions) against `head_tree`'s attributes.
///
/// `git` must run at the repository's top level: `check-attr` resolves paths
/// relative to its working directory. Spawns no git for an empty `changes`, and at
/// most one `check-attr` otherwise.
pub fn classify(
    git: &Git,
    head_tree: &Oid,
    changes: &mut [FileChange],
    extra_generated: &[String],
) -> Result<(), GitError> {
    if changes.is_empty() {
        return Ok(());
    }
    let attrs = if tree_attributes_supported()? {
        Some(read_attrs(git, head_tree, changes)?)
    } else {
        None
    };
    for change in changes.iter_mut() {
        let path = lookup_path(change);
        let found = attrs.as_ref().and_then(|m| m.get(&path));
        if change.kind == FileKind::Text && found.is_some_and(|a| a.binary) {
            change.kind = FileKind::Binary;
        }
        let explicit = found.and_then(|a| a.generated);
        change.generated_attr = match explicit {
            Some(true) => GeneratedAttr::Set,
            Some(false) => GeneratedAttr::Unset,
            None => GeneratedAttr::Unspecified,
        };
        change.generated = explicit.unwrap_or_else(|| {
            BUILTIN_GENERATED.iter().any(|p| pattern_matches(p, &path))
                || extra_generated.iter().any(|p| pattern_matches(p, &path))
        });
    }
    Ok(())
}

/// The attribute of a row stored before v2, from its `generated` bit (design §11.15).
/// v1 stored "the attribute if specified, else `BUILTIN_GENERATED`" (both v1 callers
/// of [`classify`] passed no extra patterns), so a bit that disagrees with the list
/// was the attribute; a bit that agrees with it is ambiguous. `path` is the raw
/// path (a `&str`, or the bytes of a non-UTF-8 one).
pub fn legacy_attr(path: impl AsRef<[u8]>, generated: bool) -> GeneratedAttr {
    let path = path.as_ref();
    let listed = BUILTIN_GENERATED.iter().any(|p| pattern_matches(p, path));
    match (generated, listed) {
        (false, true) => GeneratedAttr::Unset,
        (true, false) => GeneratedAttr::Set,
        _ => GeneratedAttr::Unspecified,
    }
}

/// What the head tree's attributes say about one path.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Attrs {
    binary: bool,
    /// `None` when `linguist-generated` is unspecified (the lists decide).
    generated: Option<bool>,
}

fn lookup_path(change: &FileChange) -> Vec<u8> {
    change
        .new_path
        .as_ref()
        .or(change.old_path.as_ref())
        .map(|p| p.to_bytes())
        .unwrap_or_default()
}

fn read_attrs(
    git: &Git,
    head_tree: &Oid,
    changes: &[FileChange],
) -> Result<HashMap<Vec<u8>, Attrs>, GitError> {
    let source = format!("--source={head_tree}");
    let mut args: Vec<&OsStr> = ["check-attr", "-z", "--stdin", source.as_str()]
        .into_iter()
        .map(OsStr::new)
        .collect();
    args.extend(ATTRS.iter().map(OsStr::new));
    let mut stdin = Vec::new();
    for change in changes {
        stdin.extend_from_slice(&lookup_path(change));
        stdin.push(0);
    }
    let out = git.output_stdin(&args, &stdin)?;
    parse_check_attr_z(&out)
}

/// Parses `check-attr -z` output: `<path> NUL <attr> NUL <info> NUL` triples, where
/// info is `set`, `unset`, `unspecified` or a value.
fn parse_check_attr_z(out: &[u8]) -> Result<HashMap<Vec<u8>, Attrs>, GitError> {
    let mut map: HashMap<Vec<u8>, Attrs> = HashMap::new();
    if out.is_empty() {
        return Ok(map);
    }
    let body = out
        .strip_suffix(b"\0")
        .ok_or_else(|| GitError::Parse("check-attr output does not end with NUL".into()))?;
    let fields: Vec<&[u8]> = body.split(|&b| b == 0).collect();
    let (triples, rest) = fields.as_chunks::<3>();
    if !rest.is_empty() {
        return Err(GitError::Parse(format!(
            "check-attr output has {} fields, not path/attr/info triples",
            fields.len()
        )));
    }
    for [path, attr, info] in triples {
        let entry = map.entry(path.to_vec()).or_default();
        match (*attr, *info) {
            (b"binary", b"set") | (b"diff", b"unset") => entry.binary = true,
            (b"linguist-generated", b"set" | b"true") => entry.generated = Some(true),
            (b"linguist-generated", b"unset" | b"false") => entry.generated = Some(false),
            _ => {}
        }
    }
    Ok(map)
}

/// Whether the running git has `check-attr --source` (2.40+, OQ-P3). Cached per
/// git binary; logs a one-time notice when it does not.
fn tree_attributes_supported() -> Result<bool, GitError> {
    static CACHE: Mutex<Option<(PathBuf, bool)>> = Mutex::new(None);
    let bin = git_binary();
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some((cached_bin, supported)) = cache.as_ref()
        && *cached_bin == bin
    {
        return Ok(*supported);
    }
    let (supported, found) = match check_version() {
        Ok(v) => (v.at_least(2, 40), v),
        Err(GitError::TooOld { found, .. }) => (false, found),
        Err(e) => return Err(e),
    };
    if !supported {
        notice_once(found);
    }
    *cache = Some((bin, supported));
    Ok(supported)
}

fn notice_once(found: GitVersion) {
    static NOTICE: Once = Once::new();
    NOTICE.call_once(|| {
        tracing::warn!(
            "git {found} has no `check-attr --source` (needs 2.40): `binary`, `-diff` and \
             `linguist-generated` attributes are ignored; using the built-in generated list \
             and NUL-byte detection only"
        );
    });
}

/// Matches one generated pattern against a raw repo-relative path (see module docs).
fn pattern_matches(pattern: &str, path: &[u8]) -> bool {
    let pattern = pattern.strip_prefix('/').unwrap_or(pattern).as_bytes();
    let target = if pattern.contains(&b'/') {
        path
    } else {
        path.rsplit(|&b| b == b'/').next().unwrap_or(path)
    };
    glob(&tokens(pattern), target)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tok {
    Lit(u8),
    /// `?`: one byte, not `/`.
    One,
    /// `*`: any run without `/`.
    Star,
    /// `**` not followed by `/`: any run.
    Any,
    /// `**/`: nothing, or any run ending in `/`.
    Dirs,
}

fn tokens(p: &[u8]) -> Vec<Tok> {
    let mut out = Vec::with_capacity(p.len());
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            b'*' if p.get(i + 1) == Some(&b'*') => {
                if p.get(i + 2) == Some(&b'/') {
                    out.push(Tok::Dirs);
                    i += 3;
                } else {
                    out.push(Tok::Any);
                    i += 2;
                }
                // Further stars add nothing.
                while p.get(i) == Some(&b'*') {
                    i += 1;
                }
            }
            b'*' => {
                out.push(Tok::Star);
                i += 1;
            }
            b'?' => {
                out.push(Tok::One);
                i += 1;
            }
            c => {
                out.push(Tok::Lit(c));
                i += 1;
            }
        }
    }
    out
}

/// Dynamic programming over (token, byte) suffixes: O(tokens × bytes²) at worst,
/// no backtracking blowup.
fn glob(toks: &[Tok], s: &[u8]) -> bool {
    let n = s.len();
    // next[j]: tokens[i+1..] match s[j..]; cur[j]: tokens[i..] match s[j..].
    let mut next = vec![false; n + 1];
    next[n] = true;
    for tok in toks.iter().rev() {
        let mut cur = vec![false; n + 1];
        for j in (0..=n).rev() {
            let c = s.get(j).copied();
            cur[j] = match tok {
                Tok::Lit(l) => c == Some(*l) && next[j + 1],
                Tok::One => c.is_some_and(|c| c != b'/') && next[j + 1],
                Tok::Star => next[j] || (c.is_some_and(|c| c != b'/') && cur[j + 1]),
                Tok::Any => next[j] || (c.is_some() && cur[j + 1]),
                Tok::Dirs => next[j] || (j..n).any(|k| s[k] == b'/' && next[k + 1]),
            };
        }
        next = cur;
    }
    next[0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_without_slash_matches_file_name() {
        assert!(pattern_matches("yarn.lock", b"yarn.lock"));
        assert!(pattern_matches("yarn.lock", b"web/app/yarn.lock"));
        assert!(!pattern_matches("yarn.lock", b"yarn.lock.txt"));
        assert!(!pattern_matches("yarn.lock", b"yarn.lock/x"));
        assert!(pattern_matches("*.min.js", b"dist/a.b.min.js"));
        assert!(!pattern_matches("*.min.js", b"dist/a.min.jsx"));
        assert!(pattern_matches("*_pb2.py", b"p/msg_pb2.py"));
        assert!(pattern_matches("?.map", b"x/a.map"));
        assert!(!pattern_matches("?.map", b"ab.map"));
    }

    #[test]
    fn pattern_with_slash_matches_whole_path() {
        assert!(pattern_matches("docs/api/**", b"docs/api/a/b.html"));
        assert!(!pattern_matches("docs/api/**", b"docs/index.html"));
        assert!(pattern_matches("/gen/*.ts", b"gen/a.ts"));
        assert!(!pattern_matches("gen/*.ts", b"gen/sub/a.ts"));
        assert!(!pattern_matches("gen/*.ts", b"x/gen/a.ts"));
        assert!(pattern_matches("**/gen/*.ts", b"gen/a.ts"));
        assert!(pattern_matches("**/gen/*.ts", b"x/y/gen/a.ts"));
        assert!(pattern_matches("a/**/b", b"a/b"));
        assert!(pattern_matches("a/**/b", b"a/x/y/b"));
        assert!(!pattern_matches("a/**/b", b"ab"));
        assert!(pattern_matches("a/***", b"a/x/y"));
    }

    #[test]
    fn pattern_matches_raw_non_utf8_bytes() {
        assert!(pattern_matches("*.min.js", b"caf\xe9/app\xff.min.js"));
        assert!(pattern_matches("caf?.txt", b"caf\xe9.txt"));
    }

    #[test]
    fn pattern_many_stars_is_not_exponential() {
        let path = [b'a'; 200];
        let pattern = format!("{}b", "*a".repeat(30));
        assert!(!pattern_matches(&pattern, &path));
    }

    #[test]
    fn check_attr_output_parses_set_unset_and_values() {
        let out =
            b"a.dat\0binary\0set\0a.dat\0diff\0unset\0a.dat\0linguist-generated\0unspecified\0\
b.bin\0binary\0unspecified\0b.bin\0diff\0unset\0b.bin\0linguist-generated\0false\0\
g.ts\0binary\0unspecified\0g.ts\0diff\0mark\0g.ts\0linguist-generated\0true\0\
n\nl\0binary\0unspecified\0n\nl\0diff\0unspecified\0n\nl\0linguist-generated\0unset\0";
        let map = parse_check_attr_z(out).unwrap();
        let get = |p: &[u8]| map[p];
        assert_eq!(
            get(b"a.dat"),
            Attrs {
                binary: true,
                generated: None
            }
        );
        assert_eq!(
            get(b"b.bin"),
            Attrs {
                binary: true,
                generated: Some(false)
            }
        );
        assert_eq!(
            get(b"g.ts"),
            Attrs {
                binary: false,
                generated: Some(true)
            }
        );
        assert_eq!(get(b"n\nl").generated, Some(false));
        assert!(parse_check_attr_z(b"").unwrap().is_empty());
        assert!(parse_check_attr_z(b"a\0binary\0").is_err());
        assert!(parse_check_attr_z(b"a\0binary\0set").is_err());
    }
}
