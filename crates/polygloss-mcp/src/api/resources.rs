//! MCP resources (design §15.3): markdown views of reviews, thread digests,
//! threads and diffs under `polygloss://` URIs, offered as resource templates
//! and @-mentionable in Claude Code.
//!
//! - `resources/list` = the reviews assigned to the caller's session, then the
//!   [`RECENT_REVIEWS`] most recently active, without duplicates.
//! - Agents never see drafts here either: the thread views are `get_thread`'s
//!   and `list_threads`' (open threads, positions relative to the review's
//!   latest iteration).
//! - An unknown URI or id is `not_found` (the server answers `-32002`).

use std::fmt::Write as _;

use polygloss_core::objects::BlobReader;
use polygloss_core::review::{PositionState, ReviewFilter, ReviewSummary, ThreadView};
use polygloss_diff::hunks::diff_blobs;
use polygloss_diff::options::DiffOptions;
use polygloss_diff::{FileChange, FileKind, FileStatus};

use crate::api::get_thread::{GetThreadRequest, GetThreadResult, get_thread};
use crate::api::list_threads::{ListThreadsRequest, Query};
use crate::api::shapes::{PAGE_BUDGET, ThreadSummary, timestamp};
use crate::api::thread_summary::DiffContext;
use crate::context::ApiContext;
use crate::errors::ApiError;

/// The URI scheme.
pub const SCHEME: &str = "polygloss://";
/// The MIME type of every resource.
pub const MIME: &str = "text/markdown";
/// How many recent reviews `resources/list` adds to the assigned ones.
pub const RECENT_REVIEWS: u32 = 20;
/// Files whose line counts the diff resource computes.
pub const COUNTED_FILES: usize = 200;

/// A listed resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceEntry {
    pub uri: String,
    pub name: String,
    pub title: String,
    pub description: String,
}

/// A resource template (RFC 6570).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceTemplateEntry {
    pub uri_template: String,
    pub name: String,
    pub title: String,
    pub description: String,
}

/// The four §15.3 templates.
pub fn resource_templates() -> Vec<ResourceTemplateEntry> {
    let t = |uri: &str, name: &str, title: &str, description: &str| ResourceTemplateEntry {
        uri_template: uri.to_owned(),
        name: name.to_owned(),
        title: title.to_owned(),
        description: description.to_owned(),
    };
    vec![
        t(
            "polygloss://review/{review_id}",
            "review",
            "Polygloss review",
            "Status, iterations, last verdict and summary, and counts of a review.",
        ),
        t(
            "polygloss://review/{review_id}/threads",
            "review-threads",
            "Open threads of a review",
            "Digest of a review's open threads: anchor, last comment, suggestion flag.",
        ),
        t(
            "polygloss://thread/{thread_id}",
            "thread",
            "Polygloss thread",
            "A full review thread: anchor, code, diff hunk and every comment.",
        ),
        t(
            "polygloss://diff/{diff_id}",
            "diff",
            "Polygloss diff",
            "The file list of a diff with statuses and line counts.",
        ),
    ]
}

/// The reviews assigned to the caller, then the most recent ones.
pub fn list_resources(ctx: &ApiContext) -> Result<Vec<ResourceEntry>, ApiError> {
    let mut rows = ctx.core.review_summaries(&ReviewFilter {
        assigned_session: Some(ctx.session_id.clone()),
        ..ReviewFilter::default()
    })?;
    for r in ctx.core.review_summaries(&ReviewFilter {
        limit: Some(RECENT_REVIEWS),
        ..ReviewFilter::default()
    })? {
        if !rows.iter().any(|x| x.review_id == r.review_id) {
            rows.push(r);
        }
    }
    Ok(rows
        .into_iter()
        .map(|r| {
            let title = review_title(&r);
            ResourceEntry {
                uri: format!("{SCHEME}review/{}", r.review_id),
                name: title.clone(),
                title,
                description: format!(
                    "{} review in {} · {} · {} open threads",
                    r.kind.as_str(),
                    r.repo_display,
                    r.status,
                    r.open_threads
                ),
            }
        })
        .collect())
}

/// Renders the resource at `uri` as markdown.
pub fn read_resource(ctx: &ApiContext, uri: &str) -> Result<String, ApiError> {
    let unknown = || ApiError::not_found(format!("unknown resource: {uri}"));
    let rest = uri
        .get(..SCHEME.len())
        .filter(|s| s.eq_ignore_ascii_case(SCHEME))
        .map(|_| &uri[SCHEME.len()..])
        .ok_or_else(unknown)?;
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
    match parts.as_slice() {
        [kind, id] if kind.eq_ignore_ascii_case("review") => review_md(ctx, &id.to_lowercase()),
        [kind, id, sub]
            if kind.eq_ignore_ascii_case("review") && sub.eq_ignore_ascii_case("threads") =>
        {
            threads_md(ctx, &id.to_lowercase())
        }
        [kind, id] if kind.eq_ignore_ascii_case("thread") => thread_md(ctx, &id.to_lowercase()),
        [kind, id] if kind.eq_ignore_ascii_case("diff") => diff_md(ctx, id),
        _ => Err(unknown()),
    }
}

fn review_title(r: &ReviewSummary) -> String {
    r.label.clone().unwrap_or_else(|| r.key.clone())
}

fn summary_of(ctx: &ApiContext, review_id: &str) -> Result<ReviewSummary, ApiError> {
    ctx.core
        .review_summary(review_id)?
        .ok_or_else(|| ApiError::not_found(format!("review not found: {review_id}")))
}

fn review_md(ctx: &ApiContext, review_id: &str) -> Result<String, ApiError> {
    let r = summary_of(ctx, review_id)?;
    let mut md = format!("# Review: {}\n\n", review_title(&r));
    let _ = writeln!(md, "- Review id: `{}`", r.review_id);
    let _ = writeln!(
        md,
        "- Kind: {} · Repo: `{}`",
        r.kind.as_str(),
        r.repo_path.display()
    );
    let _ = writeln!(md, "- Status: {}", r.status);
    match &r.latest_diff_id {
        Some(d) => {
            let _ = writeln!(md, "- Iterations: {} (latest diff `{d}`)", r.iterations);
        }
        None => {
            let _ = writeln!(md, "- Iterations: {}", r.iterations);
        }
    }
    let _ = writeln!(md, "- Viewed: {} / {} files", r.viewed_done, r.viewed_total);
    let _ = writeln!(
        md,
        "- Open threads: {} ({} questions)",
        r.open_threads, r.open_questions
    );
    if let Some(s) = &r.assigned_session {
        let _ = writeln!(md, "- Assigned session: `{s}`");
    }
    let _ = writeln!(md, "- Updated: {}", timestamp(r.updated_at));
    if let Some(s) = &r.last_submission {
        let _ = write!(
            md,
            "\n## Last submission\n\n**{}** at {}\n",
            s.verdict.as_str(),
            timestamp(s.at)
        );
        if !s.summary_md.trim().is_empty() {
            let _ = write!(md, "\n{}\n", clip(&s.summary_md));
        }
    }
    if let Some((summary, at)) = &r.rereview {
        let _ = write!(
            md,
            "\n## Re-review requested ({})\n\n{}\n",
            timestamp(*at),
            clip(summary)
        );
    }
    let _ = write!(
        md,
        "\nOpen threads: {SCHEME}review/{}/threads\n",
        r.review_id
    );
    Ok(md)
}

fn threads_md(ctx: &ApiContext, review_id: &str) -> Result<String, ApiError> {
    let r = summary_of(ctx, review_id)?;
    let query = Query::resolve(
        ctx,
        &ListThreadsRequest {
            review_id: Some(review_id.to_owned()),
            ..ListThreadsRequest::default()
        },
    )?;
    let threads = query.threads(ctx)?;
    let mut md = format!(
        "# Open threads: {}\n\n{} open threads.\n\n",
        review_title(&r),
        threads.len()
    );
    let mut shown = 0;
    for chunk in threads.chunks(50) {
        let refs: Vec<&ThreadView> = chunk.iter().collect();
        for s in query.summaries(ctx, &refs)? {
            let item = digest_item(&s);
            if md.len() + item.len() > PAGE_BUDGET {
                let _ = write!(
                    md,
                    "\n…{} more; call list_threads(review_id=\"{review_id}\") for all of them.\n",
                    threads.len() - shown
                );
                return Ok(md);
            }
            md.push_str(&item);
            shown += 1;
        }
    }
    Ok(md)
}

/// `path:line` (or `path:start-line`, with the side when old) of a summary.
fn location(path: Option<&str>, old: bool, start: Option<u32>, line: Option<u32>) -> String {
    let Some(path) = path else {
        return "review".to_owned();
    };
    let mut s = path.to_owned();
    match (start, line) {
        (Some(a), Some(b)) if a != b => {
            let _ = write!(s, ":{a}-{b}");
        }
        (_, Some(b)) => {
            let _ = write!(s, ":{b}");
        }
        _ => {}
    }
    if old {
        s.push_str(" (old side)");
    }
    s
}

fn position_text(s: &ThreadSummary) -> String {
    let p = &s.position;
    let state = match p.state {
        PositionState::Exact => "exact",
        PositionState::Moved => "moved",
        PositionState::Outdated => "outdated",
        PositionState::Absent => "absent",
    };
    match (p.state, &p.path, p.line) {
        (PositionState::Exact, None, _) | (PositionState::Absent, ..) => state.to_owned(),
        (_, path, Some(line)) => format!(
            "{state} → {}:{line}",
            path.as_deref().or(s.path.as_deref()).unwrap_or_default()
        ),
        (_, Some(path), None) => format!("{state} → {path}"),
        (_, None, None) => state.to_owned(),
    }
}

fn digest_item(s: &ThreadSummary) -> String {
    let old = matches!(s.side, Some(crate::api::shapes::SideParam::Old));
    let mut item = format!(
        "- **{}** `{}` ({}) · {} · by {}\n",
        s.kind.as_str(),
        location(s.path.as_deref(), old, s.start_line, s.line),
        position_text(s),
        s.status.as_str(),
        s.created_by.name
    );
    let _ = writeln!(
        item,
        "  - last: {} ({}): {}{}",
        s.last_comment.author_name,
        s.last_comment.author_kind.as_str(),
        s.last_comment.excerpt,
        if s.has_suggestion {
            " · has suggestion"
        } else {
            ""
        }
    );
    let _ = writeln!(item, "  - {SCHEME}thread/{}", s.thread_id);
    item
}

fn thread_md(ctx: &ApiContext, thread_id: &str) -> Result<String, ApiError> {
    // `not_found` for drafts too: `get_thread`'s visibility.
    let t: GetThreadResult = get_thread(
        ctx,
        GetThreadRequest {
            thread_id: thread_id.to_owned(),
            ..GetThreadRequest::default()
        },
    )?;
    let s = &t.summary;
    let old = matches!(s.side, Some(crate::api::shapes::SideParam::Old));
    let mut md = format!(
        "# {} on {}\n\n",
        capitalize(s.kind.as_str()),
        location(s.path.as_deref(), old, s.start_line, s.line)
    );
    let _ = writeln!(md, "- Thread id: `{}`", s.thread_id);
    if let Some(r) = &s.review_id {
        let _ = writeln!(md, "- Review: `{r}` ({SCHEME}review/{r})");
    }
    let _ = writeln!(md, "- Status: {}", s.status.as_str());
    let _ = writeln!(md, "- Position: {}", position_text(s));
    let _ = writeln!(
        md,
        "- Started by: {} ({})",
        s.created_by.name,
        s.created_by.kind.as_str()
    );
    if let Some(r) = &t.resolved_by {
        let _ = writeln!(
            md,
            "- Resolved by: {} ({}) at {}",
            r.name.as_deref().unwrap_or("?"),
            r.kind.as_str(),
            r.at
        );
    }
    if let Some(code) = &t.anchor.original_snippet {
        let _ = write!(md, "\n## Code when commented\n\n{}\n", fenced(code, ""));
    }
    if let Some(code) = &t.anchor.current_snippet
        && Some(code) != t.anchor.original_snippet.as_ref()
    {
        let _ = write!(md, "\n## Code now\n\n{}\n", fenced(code, ""));
    }
    if let Some(hunk) = &t.anchor.diff_hunk {
        let _ = write!(md, "\n## Diff hunk\n\n{}\n", fenced(hunk, "diff"));
    }
    md.push_str("\n## Comments\n");
    for c in &t.comments {
        let _ = write!(
            md,
            "\n### {} ({}) · {}{}\n\n",
            c.author_name,
            c.author_kind.as_str(),
            c.created_at,
            if c.edited_at.is_some() {
                " · edited"
            } else {
                ""
            }
        );
        if c.deleted {
            md.push_str("_Comment deleted._\n");
        } else {
            md.push_str(c.body_md.trim_end());
            md.push('\n');
            if c.truncated {
                md.push_str("\n_(truncated)_\n");
            }
        }
    }
    // `get_thread` already paged the comments to fit the budget.
    if let Some(cursor) = &t.next_cursor {
        let _ = write!(
            md,
            "\n…more comments follow; call get_thread(thread_id=\"{}\", cursor=\"{cursor}\") \
             (its next_cursor) for the rest.\n",
            s.thread_id
        );
    }
    Ok(md)
}

fn diff_md(ctx: &ApiContext, id: &str) -> Result<String, ApiError> {
    let diff = ctx.core.resolve_diff_prefix(id)?;
    let files = ctx
        .core
        .files_for_diff(&diff)?
        .ok_or_else(|| ApiError::not_found(format!("diff not found: {diff}")))?;
    // Counts need the objects; without a repo the list still renders.
    let dc = DiffContext::load(ctx, &diff).ok();
    let mut md = format!("# Diff {}\n\n", diff.short());
    let _ = writeln!(md, "- Diff id: `{diff}`");
    let _ = writeln!(md, "- Files: {}\n", files.len());
    md.push_str("| Status | Path | + | - |\n| --- | --- | --- | --- |\n");
    for (i, f) in files.iter().enumerate() {
        let counts = if i < COUNTED_FILES {
            dc.as_ref().and_then(|dc| line_counts(f, dc))
        } else {
            None
        };
        let path = match (&f.old_path, f.status) {
            (Some(old), FileStatus::Renamed) => format!("{} → {}", old.text, f.display_path()),
            _ => f.display_path().to_owned(),
        };
        let (add, del) = match counts {
            Some((a, d)) => (format!("+{a}"), format!("-{d}")),
            None => ("".to_owned(), "".to_owned()),
        };
        let row = format!(
            "| {} | {} | {add} | {del} |\n",
            status_text(f),
            path.replace('|', "\\|")
        );
        if md.len() + row.len() > PAGE_BUDGET {
            let _ = write!(md, "\n…{} more files.\n", files.len() - i);
            break;
        }
        md.push_str(&row);
    }
    Ok(md)
}

fn status_text(f: &FileChange) -> &'static str {
    match f.status {
        FileStatus::Added => "added",
        FileStatus::Modified => "modified",
        FileStatus::Deleted => "deleted",
        FileStatus::Renamed => "renamed",
        FileStatus::TypeChanged => "type changed",
    }
}

/// Added and deleted lines of a text file (`None` for binaries, submodules and
/// unreadable blobs).
fn line_counts(f: &FileChange, dc: &DiffContext) -> Option<(u32, u32)> {
    line_counts_with(f, dc.blobs.as_ref()?)
}

/// [`line_counts`] reading the blobs through `blobs`.
pub(crate) fn line_counts_with(f: &FileChange, blobs: &BlobReader) -> Option<(u32, u32)> {
    if matches!(f.kind, FileKind::Binary | FileKind::Submodule) {
        return None;
    }
    let read = |blob: &polygloss_diff::Oid, absent: bool| -> Option<Vec<u8>> {
        if absent || blob.is_zero() {
            return Some(Vec::new());
        }
        blobs.read(blob).ok().map(|b| b.to_vec())
    };
    let old = read(&f.old_blob, f.status == FileStatus::Added)?;
    let new = read(&f.new_blob, f.status == FileStatus::Deleted)?;
    let fd = diff_blobs(&old, &new, &DiffOptions::default());
    Some((fd.additions, fd.deletions))
}

/// A file status as JSON results spell it: `added`, `modified`, `deleted`,
/// `renamed` or `type_changed`.
pub(crate) fn file_status(status: FileStatus) -> &'static str {
    match status {
        FileStatus::Added => "added",
        FileStatus::Modified => "modified",
        FileStatus::Deleted => "deleted",
        FileStatus::Renamed => "renamed",
        FileStatus::TypeChanged => "type_changed",
    }
}

/// `text` in a fenced code block whose fence is longer than any backtick run
/// inside it.
fn fenced(text: &str, lang: &str) -> String {
    let mut longest = 0;
    let mut run = 0;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    let fence = "`".repeat(longest.max(2) + 1);
    format!("{fence}{lang}\n{text}\n{fence}")
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// Long summaries are cut like bodies.
fn clip(s: &str) -> String {
    let (cut, truncated) =
        crate::api::shapes::truncate_chars(s.trim_end(), crate::api::shapes::BODY_MAX_CHARS);
    if truncated {
        format!("{cut}\n\n_(truncated)_")
    } else {
        cut
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fences_outgrow_backticks_inside() {
        assert_eq!(fenced("a", "diff"), "```diff\na\n```");
        assert_eq!(fenced("x ```` y", ""), "`````\nx ```` y\n`````");
    }

    #[test]
    fn locations_name_lines_ranges_and_sides() {
        assert_eq!(location(Some("a.rs"), false, Some(5), Some(5)), "a.rs:5");
        assert_eq!(
            location(Some("a.rs"), true, Some(2), Some(4)),
            "a.rs:2-4 (old side)"
        );
        assert_eq!(location(Some("a.rs"), false, None, None), "a.rs");
        assert_eq!(location(None, false, None, None), "review");
    }
}
