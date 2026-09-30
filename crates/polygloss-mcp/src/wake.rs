//! Wake-ups for an agent session (design §16.3; T4.8 `polygloss wait`, and the
//! opt-in `claude/channel` push of T4.13): which `review.submitted` events
//! concern a session, and the summary text the agent is woken with.
//!
//! Rules ([`wakes_for`]):
//!
//! - Only `review.submitted` events count (OQ-11: submissions only, never plain
//!   agent replies or resolves), for any verdict (an approval wakes too).
//! - The review must be assigned to the session's canonical session now
//!   (`sessions.canonical_id`, §16.4), so a drifted id sees its root's reviews,
//!   and the submission must come after that assignment began
//!   (`Core::assignment_seq`): a session that opens a review with an old
//!   verdict is not woken with it (T5.9).
//! - A submission the agent already answered with `request_rereview` (a later
//!   `review.rereview_requested` on the same review) is skipped, so a new
//!   session opening a long-lived live review is not woken with old feedback.
//! - One wake per review: the latest of its submissions in `events`.
//!
//! The caller filters by `seq` (`sessions.last_woken_seq` for the waiter).

use polygloss_core::review::{Core, CoreError, ReviewSummary, Verdict};
use polygloss_core::store::events::{Event, EventFilter, EventKind, events_since};

/// The longest submission summary quoted in a wake-up; longer ones are cut.
pub const MAX_SUMMARY_CHARS: usize = 4_000;

/// One submission that should wake the session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wake {
    pub review_id: String,
    pub submission_id: String,
    pub verdict: Verdict,
    /// The `review.submitted` event's `seq`.
    pub seq: i64,
    /// The summary for the agent ([`wake_text`]).
    pub text: String,
}

/// The wakes for `session_id` among `events` (any events; only
/// `review.submitted` ones count), in `seq` order. See the module rules.
pub fn wakes_for(core: &Core, session_id: &str, events: &[Event]) -> Result<Vec<Wake>, CoreError> {
    let root = core.canonical_session(session_id)?;
    // The latest submission per review, in order of first appearance.
    let mut latest: Vec<&Event> = Vec::new();
    for e in events
        .iter()
        .filter(|e| e.kind == EventKind::ReviewSubmitted)
    {
        let Some(review_id) = e.review_id.as_deref() else {
            continue;
        };
        match latest
            .iter_mut()
            .find(|l| l.review_id.as_deref() == Some(review_id))
        {
            Some(slot) if slot.seq < e.seq => *slot = e,
            Some(_) => {}
            None => latest.push(e),
        }
    }

    let mut wakes = Vec::new();
    for e in latest {
        let review_id = e.review_id.as_deref().unwrap_or_default();
        let assigned = match core.assigned_session(review_id) {
            Ok(s) => s,
            // Pruned since the event was written.
            Err(CoreError::NotFound { .. }) => continue,
            Err(err) => return Err(err),
        };
        if assigned.is_none_or(|s| s.id != root) || answered_by_rereview(core, review_id, e.seq)? {
            continue;
        }
        if core
            .assignment_seq(review_id, &root)?
            .is_some_and(|since| e.seq < since)
        {
            continue;
        }
        let Some(review) = core.review_summary(review_id)? else {
            continue;
        };
        let submission_id = e.payload["submission_id"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let payload_verdict = e.payload["verdict"].as_str().and_then(Verdict::parse);
        // The review's latest submission is this event's unless another one
        // landed since; its text is what the agent should read.
        let (verdict, summary_md) = match &review.last_submission {
            Some(s) if s.submission_id == submission_id || payload_verdict.is_none() => {
                (s.verdict, s.summary_md.as_str())
            }
            _ => (payload_verdict.unwrap_or(Verdict::Comment), ""),
        };
        wakes.push(Wake {
            review_id: review_id.to_owned(),
            submission_id,
            verdict,
            seq: e.seq,
            text: wake_text(&review, verdict, summary_md),
        });
    }
    wakes.sort_by_key(|w| w.seq);
    Ok(wakes)
}

/// Whether the review has a `review.rereview_requested` after `seq`.
fn answered_by_rereview(core: &Core, review_id: &str, seq: i64) -> Result<bool, CoreError> {
    let filter = EventFilter {
        review_ids: Some(vec![review_id.to_owned()]),
        agent_visible_only: false,
        kinds: Some(vec![EventKind::ReviewRereviewRequested]),
    };
    Ok(!core
        .store
        .read(|c| events_since(c, seq, &filter, 1))?
        .is_empty())
}

/// The verdict as a person would say it.
pub fn verdict_words(v: Verdict) -> &'static str {
    match v {
        Verdict::RequestChanges => "request changes",
        Verdict::Comment => "comment",
        Verdict::Approve => "approve",
    }
}

/// The text an agent is woken with for one submission: which review, the
/// verdict, open threads, the human's summary and what to call next.
pub fn wake_text(review: &ReviewSummary, verdict: Verdict, summary_md: &str) -> String {
    let title = review.label.as_deref().unwrap_or(&review.key);
    let summary = summary_md.trim();
    let summary = if summary.is_empty() {
        "(none)".to_owned()
    } else if summary.chars().count() > MAX_SUMMARY_CHARS {
        let cut: String = summary.chars().take(MAX_SUMMARY_CHARS).collect();
        format!("{cut}… (truncated)")
    } else {
        summary.to_owned()
    };
    let next = match verdict {
        Verdict::Approve => "read any remaining comments; the human approved this review",
        Verdict::RequestChanges | Verdict::Comment => {
            "read the feedback, reply to or resolve each thread, then call request_rereview when your changes are ready"
        }
    };
    format!(
        "Polygloss: the human submitted their review of \"{title}\" ({repo}).\n\
         Verdict: {verdict}\n\
         Open threads: {open}\n\
         Summary: {summary}\n\
         Next: call list_threads(review_id=\"{id}\") to {next}.",
        repo = review.repo_display,
        verdict = verdict_words(verdict),
        open = review.open_threads,
        id = review.review_id,
    )
}

/// Every wake's text, separated by a blank line.
pub fn combined_text(wakes: &[Wake]) -> String {
    wakes
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use polygloss_core::git::ReviewKind;

    use super::*;

    fn review(label: Option<&str>) -> ReviewSummary {
        ReviewSummary {
            review_id: "0199-review".into(),
            key: "compare:/repo:main...feature".into(),
            label: label.map(str::to_owned),
            kind: ReviewKind::Compare,
            repo_display: "repo".into(),
            repo_path: PathBuf::from("/repo"),
            status: "changes_requested".into(),
            iterations: 1,
            latest_diff_id: None,
            viewed_done: 0,
            viewed_total: 1,
            open_threads: 3,
            open_questions: 0,
            awaiting_you: false,
            last_submission: None,
            rereview: None,
            assigned_session: Some("s".into()),
            muted: false,
            updated_at: 0,
        }
    }

    #[test]
    fn wake_text_names_review_verdict_threads_summary_and_next_call() {
        let text = wake_text(
            &review(Some("Auth refactor")),
            Verdict::RequestChanges,
            "  Rename the helper.\n",
        );
        assert_eq!(
            text,
            "Polygloss: the human submitted their review of \"Auth refactor\" (repo).\n\
             Verdict: request changes\n\
             Open threads: 3\n\
             Summary: Rename the helper.\n\
             Next: call list_threads(review_id=\"0199-review\") to read the feedback, reply \
             to or resolve each thread, then call request_rereview when your changes are ready."
        );
    }

    #[test]
    fn wake_text_falls_back_to_the_key_and_marks_an_empty_summary() {
        let text = wake_text(&review(None), Verdict::Approve, "");
        assert!(text.contains("\"compare:/repo:main...feature\""), "{text}");
        assert!(text.contains("Verdict: approve"), "{text}");
        assert!(text.contains("Summary: (none)"), "{text}");
        assert!(text.contains("the human approved"), "{text}");
    }

    #[test]
    fn wake_text_truncates_long_summaries() {
        let long = "é".repeat(MAX_SUMMARY_CHARS + 10);
        let text = wake_text(&review(None), Verdict::Comment, &long);
        assert!(text.contains("… (truncated)"));
        assert!(text.chars().count() < MAX_SUMMARY_CHARS + 400);
    }

    #[test]
    fn verdict_words_are_plain_english() {
        assert_eq!(verdict_words(Verdict::RequestChanges), "request changes");
        assert_eq!(verdict_words(Verdict::Comment), "comment");
        assert_eq!(verdict_words(Verdict::Approve), "approve");
    }
}
