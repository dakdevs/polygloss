//! Shared shapes of design §15.2: the request shapes (`Source`, `Anchor`, sides
//! and filters) and the result shapes every read shares (`ThreadSummary`,
//! `Position`), plus the wire conventions:
//!
//! - Times (`at`, `created_at`, `edited_at`, `updated_at`) are RFC 3339 UTC
//!   strings with milliseconds, like GitHub's API: [`timestamp`].
//! - Comment bodies over [`BODY_MAX_CHARS`] characters are cut there and
//!   flagged `truncated: true` ([`truncate_chars`]); excerpts are one line of at
//!   most [`EXCERPT_MAX_CHARS`] characters ([`excerpt`]).

use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::review::{AuthorKind, PositionState, ThreadKind, ThreadStatus};
use polygloss_diff::Side;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Comment bodies (and submission summaries) longer than this many characters
/// are truncated and flagged (design §15.2 `get_thread`, **Provisional**).
pub const BODY_MAX_CHARS: usize = 20_000;

/// What one page of a list (or one resource) may use of the
/// [`PAGE_MAX_CHARS`](crate::paging::PAGE_MAX_CHARS) budget, leaving room for
/// the fields around the list.
pub const PAGE_BUDGET: usize = crate::paging::PAGE_MAX_CHARS - 1_000;

/// `last_comment.excerpt` length in characters, ellipsis included.
pub const EXCERPT_MAX_CHARS: usize = 300;

/// Where a thread shows in the diff it is listed against (§15.2 `Position`):
/// the review's latest iteration, or the `diff_id` the caller passed. `path` is
/// set only when the file is shown under another name (a rename); lines are
/// 1-based and absent for file and review threads, absent threads, and outdated
/// threads on a side without lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PositionOut {
    pub state: PositionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

/// Who started a thread (`created_by`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CreatedBy {
    pub kind: AuthorKind,
    pub name: String,
}

/// The newest visible comment of a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LastComment {
    pub author_kind: AuthorKind,
    pub author_name: String,
    /// One line, at most [`EXCERPT_MAX_CHARS`] characters.
    pub excerpt: String,
    pub at: String,
}

/// A thread as lists show it (§15.2 `ThreadSummary`). `path`, `side`,
/// `start_line` and `line` are the anchor as created; `position` is where it
/// shows now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadSummary {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub review_id: Option<String>,
    pub kind: ThreadKind,
    /// `line`, `file` or `review`.
    pub subject: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<SideParam>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub position: PositionOut,
    pub status: ThreadStatus,
    pub created_by: CreatedBy,
    /// Visible comments, a deleted root's placeholder not counted.
    pub comment_count: u32,
    /// Some comment has a ```` ```suggestion ```` block for the anchored
    /// new-side lines.
    pub has_suggestion: bool,
    pub last_comment: LastComment,
    pub updated_at: String,
}

/// Unix milliseconds as RFC 3339 UTC with milliseconds
/// (`2026-09-29T10:11:12.345Z`).
pub fn timestamp(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        tod / 3600,
        tod % 3600 / 60,
        tod % 60
    )
}

/// `days` since 1970-01-01 as a proleptic Gregorian date (Howard Hinnant's
/// `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// `s` cut to at most `max` characters, and whether it was cut.
pub fn truncate_chars(s: &str, max: usize) -> (String, bool) {
    match s.char_indices().nth(max) {
        Some((i, _)) => (s[..i].to_owned(), true),
        None => (s.to_owned(), false),
    }
}

/// A one-line excerpt: whitespace runs collapsed to one space, cut to
/// [`EXCERPT_MAX_CHARS`] characters with a final `…` when longer.
pub fn excerpt(body: &str) -> String {
    let line = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let (cut, truncated) = truncate_chars(&line, EXCERPT_MAX_CHARS);
    if !truncated {
        return cut;
    }
    let (mut short, _) = truncate_chars(&cut, EXCERPT_MAX_CHARS - 1);
    short.push('…');
    short
}

/// What to diff. The default is `{kind: "live"}`: the working
/// tree against the merge-base with the default branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceParam {
    /// The live working tree (including uncommitted and untracked changes).
    Live {
        /// `merge-base` (default), `HEAD`, or any revision.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        since: Option<String>,
    },
    /// One commit against its first parent.
    Commit { rev: String },
    /// Branch against branch, like a pull request.
    Compare {
        base: String,
        head: String,
        /// `three-dot` (default: changes on `head` since the merge-base) or
        /// `direct` (the two trees as they are).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mode: Option<CompareModeParam>,
    },
}

impl Default for SourceParam {
    fn default() -> SourceParam {
        SourceParam::Live { since: None }
    }
}

impl SourceParam {
    /// The core source this names.
    pub fn to_core(&self) -> Source {
        match self {
            SourceParam::Live { since } => Source::Live {
                since: match since.as_deref().map(str::trim) {
                    None | Some("") | Some("merge-base") => Since::MergeBase,
                    Some("HEAD") => Since::Head,
                    Some(rev) => Since::Commit(rev.to_owned()),
                },
            },
            SourceParam::Commit { rev } => Source::Commit { rev: rev.clone() },
            SourceParam::Compare { base, head, mode } => Source::Compare {
                base: base.clone(),
                head: head.clone(),
                mode: match mode {
                    Some(CompareModeParam::Direct) => CompareMode::Direct,
                    Some(CompareModeParam::ThreeDot) | None => CompareMode::ThreeDot,
                },
            },
        }
    }
}

/// `three-dot` or `direct`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum CompareModeParam {
    ThreeDot,
    Direct,
}

/// A diff side: `old` = base, `new` = head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SideParam {
    Old,
    New,
}

impl From<SideParam> for Side {
    fn from(s: SideParam) -> Side {
        match s {
            SideParam::Old => Side::Old,
            SideParam::New => Side::New,
        }
    }
}

impl From<Side> for SideParam {
    fn from(s: Side) -> SideParam {
        match s {
            Side::Old => SideParam::Old,
            Side::New => SideParam::New,
        }
    }
}

/// Where a comment goes: `{path}` for a file thread, or
/// `{path, side, line, start_line?}` for a line or range thread. Lines are 1-based
/// lines of the file on that side.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AnchorParam {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<SideParam>,
    /// The last (or only) line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The first line of a range; omit for a single line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
}

/// `list_threads.status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatusFilter {
    #[default]
    Open,
    Resolved,
    All,
}

/// `list_threads.author`: who started the thread.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorFilter {
    Human,
    Agent,
    #[default]
    Any,
}

/// A thread kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ThreadKindParam {
    Comment,
    Note,
    Question,
}

/// The kinds an agent may create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentThreadKind {
    /// Explains a change; needs no answer.
    Note,
    /// Asks the human for a decision.
    Question,
}

/// `list_reviews.assigned`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssignedFilter {
    #[default]
    Any,
    /// Only reviews assigned to this session.
    Me,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_rfc3339_utc_millis() {
        assert_eq!(timestamp(0), "1970-01-01T00:00:00.000Z");
        assert_eq!(timestamp(1_767_225_600_123), "2026-01-01T00:00:00.123Z");
        assert_eq!(timestamp(1_709_210_096_789), "2024-02-29T12:34:56.789Z");
        assert_eq!(timestamp(-1), "1969-12-31T23:59:59.999Z");
    }

    #[test]
    fn truncate_counts_characters_not_bytes() {
        assert_eq!(truncate_chars("héllo", 2), ("hé".to_owned(), true));
        assert_eq!(truncate_chars("héllo", 5), ("héllo".to_owned(), false));
        assert_eq!(truncate_chars("", 0), (String::new(), false));
    }

    #[test]
    fn excerpt_is_one_line_of_at_most_300_chars() {
        assert_eq!(excerpt("  a\n\n b\tc  "), "a b c");
        let long = excerpt(&"é".repeat(400));
        assert_eq!(long.chars().count(), EXCERPT_MAX_CHARS);
        assert!(long.ends_with('…'));
        let exact = "x".repeat(EXCERPT_MAX_CHARS);
        assert_eq!(excerpt(&exact), exact);
    }
}
