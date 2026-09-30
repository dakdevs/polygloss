//! `polygloss://` URLs (T4.2, design §13.5): parsing, formatting, and resolving
//! a URL to the review tab it opens.
//!
//! | URL                                                 | Opens                                        |
//! | --------------------------------------------------- | -------------------------------------------- |
//! | `polygloss://diff/<diff_id>`                        | The diff, in its most recent review          |
//! | `polygloss://diff/<diff_id>?path=…&side=new&line=…` | The same, focused on a file or a line        |
//! | `polygloss://review/<review_id>`                    | A review tab (also `…/<review_id>/threads`)  |
//! | `polygloss://thread/<thread_id>`                    | The thread's review tab, focused on it       |
//!
//! - Ids: a full 64-hex `diff_id`, UUID review and thread ids (hyphenated).
//!   Scheme, target and ids are case-insensitive; ids are normalized to
//!   lowercase.
//! - `line` is 1-based (design §8.1) and needs `path`; `side` (`old` | `new`,
//!   default `new`) needs `path` too.
//! - [`format_url`] percent-encodes every byte of `path` except the RFC 3986
//!   unreserved characters and `/`, so the URL is plain ASCII; `+` is always a
//!   literal plus. Unknown query parameters are ignored, a fragment too.
//!   MCP resource URIs (§15.3) use the same forms: `review/<id>/threads`
//!   parses as the review.

use polygloss_diff::Side;
use rusqlite::OptionalExtension as _;

use crate::ids::DiffId;
use crate::review::{Core, CoreError};

/// The URL scheme (registered as `CFBundleURLTypes` in the bundle).
pub const SCHEME: &str = "polygloss";

/// A parsed `polygloss://` URL. Ids are normalized (lowercase), but only
/// checked for shape, not for existence ([`Core::resolve_url`] does that).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolyglossUrl {
    Diff {
        diff_id: String,
        path: Option<String>,
        side: Option<Side>,
        /// 1-based.
        line: Option<u32>,
    },
    Review(String),
    Thread(String),
}

/// Why a string is not a valid `polygloss://` URL.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UrlError {
    #[error("not a polygloss:// URL: {0:?}")]
    NotPolyglossUrl(String),
    #[error("unknown polygloss:// target {0:?} (expected diff, review or thread)")]
    UnknownTarget(String),
    #[error("invalid {kind} id {id:?}")]
    BadId { kind: &'static str, id: String },
    #[error("invalid URL parameter {name:?}: {reason}")]
    BadParam { name: String, reason: String },
}

/// Parses a `polygloss://` URL (see the module docs for the accepted forms).
pub fn parse_url(s: &str) -> Result<PolyglossUrl, UrlError> {
    let not_ours = || UrlError::NotPolyglossUrl(s.to_owned());
    let (scheme, rest) = s.split_once("://").ok_or_else(not_ours)?;
    if !scheme.eq_ignore_ascii_case(SCHEME) {
        return Err(not_ours());
    }
    let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (target, ids) = path.split_once('/').unwrap_or((path, ""));
    let ids = ids.strip_suffix('/').unwrap_or(ids);
    let params = Params::parse(query);
    match target.to_ascii_lowercase().as_str() {
        "diff" => {
            let diff_id =
                DiffId::parse(&ids.to_ascii_lowercase()).map_err(|_| UrlError::BadId {
                    kind: "diff",
                    id: ids.to_owned(),
                })?;
            let path = params.get("path")?;
            let side = params
                .raw("side")?
                .map(|v| match v {
                    "old" => Ok(Side::Old),
                    "new" => Ok(Side::New),
                    _ => Err(bad_param("side", "expected old or new")),
                })
                .transpose()?;
            let line = params
                .raw("line")?
                .map(|v| match v.parse::<u32>() {
                    Ok(n) if n >= 1 => Ok(n),
                    _ => Err(bad_param("line", "expected a line number from 1")),
                })
                .transpose()?;
            if path.as_deref() == Some("") {
                return Err(bad_param("path", "empty"));
            }
            if path.is_none() {
                for (name, given) in [("line", line.is_some()), ("side", side.is_some())] {
                    if given {
                        return Err(bad_param(name, "needs a path"));
                    }
                }
            }
            Ok(PolyglossUrl::Diff {
                diff_id: diff_id.to_string(),
                path,
                side,
                line,
            })
        }
        "review" => {
            params.no_focus()?;
            // `review/<id>/threads` is the MCP resource of the review's threads.
            let id = ids.strip_suffix("/threads").unwrap_or(ids);
            Ok(PolyglossUrl::Review(uuid_id("review", id)?))
        }
        "thread" => {
            params.no_focus()?;
            Ok(PolyglossUrl::Thread(uuid_id("thread", ids)?))
        }
        other => Err(UrlError::UnknownTarget(other.to_owned())),
    }
}

/// Formats `u` as a `polygloss://` URL; [`parse_url`] reads it back unchanged.
pub fn format_url(u: &PolyglossUrl) -> String {
    match u {
        PolyglossUrl::Diff {
            diff_id,
            path,
            side,
            line,
        } => {
            let mut s = format!("{SCHEME}://diff/{diff_id}");
            let mut sep = '?';
            let mut push = |name: &str, value: &str| {
                s.push(sep);
                s.push_str(name);
                s.push('=');
                s.push_str(value);
                sep = '&';
            };
            if let Some(path) = path {
                push("path", &percent_encode(path));
            }
            if let Some(side) = side {
                push("side", side_str(*side));
            }
            if let Some(line) = line {
                push("line", &line.to_string());
            }
            s
        }
        PolyglossUrl::Review(id) => format!("{SCHEME}://review/{id}"),
        PolyglossUrl::Thread(id) => format!("{SCHEME}://thread/{id}"),
    }
}

/// Where a URL points once resolved: the review whose tab shows it, and what to
/// focus there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UrlTarget {
    pub review_id: String,
    pub focus: Option<UrlFocus>,
}

/// What to scroll to in the review tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlFocus {
    /// The file's header.
    File {
        path: String,
    },
    /// Line `line` (1-based) of `side` of `path`.
    Line {
        path: String,
        side: Side,
        line: u32,
    },
    Thread(String),
}

impl Core {
    /// The review tab `url` opens: the review itself; for a diff, the most
    /// recently active review with an iteration showing it; for a thread, the
    /// thread's review. `not_found` when there is none.
    pub fn resolve_url(&self, url: &PolyglossUrl) -> Result<UrlTarget, CoreError> {
        match url {
            PolyglossUrl::Review(id) => {
                let found = self.store.read(|c| {
                    Ok(
                        c.query_row("SELECT 1 FROM reviews WHERE id = ?1", [id], |_| Ok(()))
                            .optional()?,
                    )
                })?;
                found.ok_or_else(|| CoreError::not_found("review", id))?;
                Ok(UrlTarget {
                    review_id: id.clone(),
                    focus: None,
                })
            }
            PolyglossUrl::Diff {
                diff_id,
                path,
                side,
                line,
            } => {
                let diff_id = DiffId::parse(diff_id)?;
                let review_id = self.store.read(|c| {
                    Ok(c.query_row(
                        "SELECT i.review_id FROM iterations i \
                         JOIN reviews r ON r.id = i.review_id \
                         WHERE i.diff_id = ?1 \
                         ORDER BY r.updated_at DESC, i.id DESC LIMIT 1",
                        [diff_id.as_str()],
                        |r| r.get::<_, String>(0),
                    )
                    .optional()?)
                })?;
                let review_id =
                    review_id.ok_or_else(|| CoreError::not_found("diff", diff_id.as_str()))?;
                let focus = path.clone().map(|path| match line {
                    Some(line) => UrlFocus::Line {
                        path,
                        side: side.unwrap_or(Side::New),
                        line: *line,
                    },
                    None => UrlFocus::File { path },
                });
                Ok(UrlTarget { review_id, focus })
            }
            PolyglossUrl::Thread(id) => {
                let review_id = self.store.read(|c| {
                    Ok(
                        c.query_row("SELECT review_id FROM threads WHERE id = ?1", [id], |r| {
                            r.get::<_, Option<String>>(0)
                        })
                        .optional()?,
                    )
                })?;
                let review_id = review_id
                    .flatten()
                    .ok_or_else(|| CoreError::not_found("thread", id))?;
                Ok(UrlTarget {
                    review_id,
                    focus: Some(UrlFocus::Thread(id.clone())),
                })
            }
        }
    }
}

fn side_str(side: Side) -> &'static str {
    match side {
        Side::Old => "old",
        Side::New => "new",
    }
}

fn bad_param(name: &str, reason: &str) -> UrlError {
    UrlError::BadParam {
        name: name.to_owned(),
        reason: reason.to_owned(),
    }
}

/// A hyphenated UUID, lowercased.
fn uuid_id(kind: &'static str, id: &str) -> Result<String, UrlError> {
    let lower = id.to_ascii_lowercase();
    match uuid::Uuid::try_parse(&lower) {
        Ok(u) if u.hyphenated().to_string() == lower => Ok(lower),
        _ => Err(UrlError::BadId {
            kind,
            id: id.to_owned(),
        }),
    }
}

/// The query's `name=value` pairs, still encoded.
struct Params<'a>(Vec<(&'a str, &'a str)>);

impl<'a> Params<'a> {
    fn parse(query: &'a str) -> Params<'a> {
        let pairs = query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(|p| p.split_once('=').unwrap_or((p, "")))
            .collect();
        Params(pairs)
    }

    /// The raw value of `name`; an error when it is given twice.
    fn raw(&self, name: &str) -> Result<Option<&'a str>, UrlError> {
        let mut values = self.0.iter().filter(|(k, _)| *k == name).map(|(_, v)| *v);
        let first = values.next();
        if values.next().is_some() {
            return Err(bad_param(name, "given more than once"));
        }
        Ok(first)
    }

    /// The percent-decoded value of `name`.
    fn get(&self, name: &str) -> Result<Option<String>, UrlError> {
        self.raw(name)?
            .map(|v| percent_decode(v).ok_or_else(|| bad_param(name, "bad percent-encoding")))
            .transpose()
    }

    /// Review and thread URLs take no `path`, `side` or `line`.
    fn no_focus(&self) -> Result<(), UrlError> {
        for name in ["path", "side", "line"] {
            if self.raw(name)?.is_some() {
                return Err(bad_param(name, "only diff URLs take a location"));
            }
        }
        Ok(())
    }
}

fn percent_encode(s: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0xf) as usize] as char);
        }
    }
    out
}

/// Decodes `%XX` escapes; `None` for a broken escape or bytes that are not UTF-8.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let digit = |j: usize| (*bytes.get(j)? as char).to_digit(16);
            out.push((digit(i + 1)? * 16 + digit(i + 2)?) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}
