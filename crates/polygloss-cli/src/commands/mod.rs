//! Subcommand handlers (design §14). The human commands (`live`, `show`,
//! `compare`, `open`, `snapshot`) open a review through `Core::open` as the
//! human actor, then show it in the app (`app::show`, unless `--no-open`) and
//! return a [`Report`]; `mcp`, `wait` and the JSON CLI (`json`) have their own
//! output rules.

pub mod app;
pub mod compare;
pub mod json;
pub mod live;
pub mod mcp;
pub mod open;
pub mod show;
pub mod snapshot;
pub mod wait;

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use polygloss_core::git::{ResolvedSide, Since};
use polygloss_core::ipc::Op;
use polygloss_core::review::{Core, CoreError, OpenRequest, OpenedDiff};
use polygloss_core::urls::{PolyglossUrl, format_url};
use serde_json::{Value, json};

use crate::cli::GlobalArgs;
use crate::output::{CliError, Report};
use app::AppShown;

/// The store at the default paths (`POLYGLOSS_DATA_DIR` honored).
pub fn core() -> Result<Core, CliError> {
    Ok(Core::open_default()?)
}

/// The directory a command works in: `path`, else `--repo`, else the current
/// directory, made absolute.
pub fn work_dir(path: Option<&Path>, global: &GlobalArgs) -> Result<PathBuf, CliError> {
    let dir = match path.or(global.repo.as_deref()) {
        Some(p) => p.to_path_buf(),
        None => std::env::current_dir()
            .map_err(|e| CliError::internal(format!("cannot read the current directory: {e}")))?,
    };
    std::path::absolute(&dir)
        .map_err(|e| CliError::internal(format!("cannot resolve {}: {e}", dir.display())))
}

/// `--since` as a live base: `merge-base` (default), `HEAD` or a revision.
pub fn parse_since(since: Option<&str>) -> Since {
    match since.map(str::trim) {
        None | Some("") | Some("merge-base") => Since::MergeBase,
        Some("HEAD") => Since::Head,
        Some(rev) => Since::Commit(rev.to_owned()),
    }
}

/// The link that reopens what was opened: the diff when an iteration of the
/// review shows it (it is stored, so `polygloss://diff/<id>` resolves), else
/// the review (an unpinned live state is not stored).
pub fn url_for(opened: &OpenedDiff) -> String {
    let url = if opened.iteration.is_some() {
        PolyglossUrl::Diff {
            diff_id: opened.diff_id.as_str().to_owned(),
            path: None,
            side: None,
            line: None,
        }
    } else {
        PolyglossUrl::Review(opened.review_id.clone())
    };
    format_url(&url)
}

/// Opens `req` and, unless `--no-open`, shows the review in the app.
pub fn open_and_show(global: &GlobalArgs, req: &OpenRequest) -> Result<Report, CliError> {
    let core = core()?;
    let opened = core.open(req)?;
    let url = url_for(&opened);
    let app = if global.no_open {
        nudge(&core);
        AppShown::Skipped
    } else {
        let op = Op::Open {
            review_id: Some(opened.review_id.clone()),
            diff_id: None,
            activate: true,
        };
        app::show(&core.paths, op).map_err(|e| recorded_anyway(e, &url))?
    };
    let label = review_label(&core, &opened.review_id)?;
    Ok(opened_report(&opened, label.as_deref(), &url, app))
}

/// An `app_unavailable` error after the review was recorded says where it is.
pub fn recorded_anyway(mut err: CliError, url: &str) -> CliError {
    if err.code == "app_unavailable" {
        let _ = write!(err.message, " (the review was recorded: {url})");
    }
    err
}

/// Best-effort `store_changed` to a running app.
pub fn nudge(core: &Core) {
    if let Ok(seq) = latest_seq(core) {
        app::nudge(&core.paths, seq);
    }
}

fn latest_seq(core: &Core) -> Result<i64, CliError> {
    Ok(core
        .store
        .read(|c| Ok(c.query_row("SELECT COALESCE(MAX(seq), 0) FROM events", [], |r| r.get(0))?))
        .map_err(CoreError::from)?)
}

/// The review's display label (`reviews.label`).
pub fn review_label(core: &Core, review_id: &str) -> Result<Option<String>, CliError> {
    Ok(core
        .store
        .read(|c| {
            Ok(c.query_row(
                "SELECT label FROM reviews WHERE id = ?1",
                [review_id],
                |r| r.get(0),
            )?)
        })
        .map_err(CoreError::from)?)
}

fn side_json(side: &ResolvedSide) -> Value {
    let mut v = json!({ "tree": side.tree.as_str() });
    if let Some(rev) = &side.ref_name {
        v["rev"] = json!(rev);
    }
    if let Some(commit) = &side.commit {
        v["commit"] = json!(commit.as_str());
    }
    v
}

/// The JSON and human output of an opened review.
pub fn opened_report(opened: &OpenedDiff, label: Option<&str>, url: &str, app: AppShown) -> Report {
    let repo = opened
        .repo
        .toplevel
        .clone()
        .unwrap_or_else(|| opened.repo.common_dir.clone());
    let mut head = json!({ "tree": opened.head_tree.as_str() });
    if let Some(commit) = &opened.head_commit {
        head["commit"] = json!(commit.as_str());
    }
    let warnings: Vec<String> = opened.warnings.iter().map(ToString::to_string).collect();
    let json = json!({
        "review_id": opened.review_id,
        "review_key": opened.review_key,
        "kind": opened.kind.as_str(),
        "label": label,
        "diff_id": opened.diff_id.as_str(),
        "iteration": opened.iteration.as_ref().map(|it| it.seq),
        "url": url,
        "repo": repo.to_string_lossy(),
        "base": side_json(&opened.base),
        "head": head,
        "files": opened.files.len(),
        "warnings": warnings,
        "app": app.as_str(),
    });

    let mut human = String::new();
    let what = match opened.kind.as_str() {
        "live" => "Live review",
        "commit" => "Commit review",
        _ => "Compare review",
    };
    let _ = writeln!(human, "{what} {}", opened.review_key);
    if let Some(label) = label {
        let _ = writeln!(human, "  label      {label}");
    }
    let _ = writeln!(human, "  review     {}", opened.review_id);
    let files = match opened.files.len() {
        1 => "1 file".to_owned(),
        n => format!("{n} files"),
    };
    let pinned = match &opened.iteration {
        Some(it) => format!("iteration {}", it.seq),
        None => "not pinned".to_owned(),
    };
    let _ = writeln!(
        human,
        "  diff       {}  {files}, {pinned}",
        opened.diff_id.short()
    );
    let _ = writeln!(human, "  url        {url}");
    for w in &warnings {
        let _ = writeln!(human, "  note       {w}");
    }
    match app {
        AppShown::Opened => human.push_str("Opened in Polygloss.\n"),
        AppShown::Launched => human.push_str("Launched Polygloss.\n"),
        AppShown::Skipped => {}
    }
    Report { json, human }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_parses_the_three_forms() {
        assert_eq!(parse_since(None), Since::MergeBase);
        assert_eq!(parse_since(Some("merge-base")), Since::MergeBase);
        assert_eq!(parse_since(Some("HEAD")), Since::Head);
        assert_eq!(parse_since(Some("v1.0")), Since::Commit("v1.0".to_owned()));
    }

    #[test]
    fn work_dir_prefers_path_then_repo() {
        let global = GlobalArgs {
            repo: Some("/r".into()),
            ..GlobalArgs::default()
        };
        assert_eq!(
            work_dir(Some(Path::new("/p")), &global).expect("dir"),
            PathBuf::from("/p")
        );
        assert_eq!(work_dir(None, &global).expect("dir"), PathBuf::from("/r"));
        assert_eq!(
            work_dir(None, &GlobalArgs::default()).expect("dir"),
            std::env::current_dir().expect("cwd")
        );
    }

    #[test]
    fn unavailable_errors_name_the_recorded_review() {
        let e = recorded_anyway(
            CliError::new("app_unavailable", "no app"),
            "polygloss://review/x",
        );
        assert_eq!(
            e.message,
            "no app (the review was recorded: polygloss://review/x)"
        );
        let other = recorded_anyway(CliError::new("not_found", "gone"), "u");
        assert_eq!(other.message, "gone");
    }
}
