//! Shared request shapes of design §15.2 (`Source`, `Anchor`, sides and filters).
//! T4.5 adds the result shapes (`ThreadSummary`, `Position`, …) here.

use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_diff::Side;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What to diff (§15.2 `Source`). The default is `{kind: "live"}`: the working
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

/// Where a comment goes (§15.2 `Anchor`): `{path}` for a file thread, or
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
