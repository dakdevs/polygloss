//! `polygloss open <diff_id|prefix>`: opens a diff by id or unique prefix of
//! at least 8 hex digits (design §4.1, §14). The store finds a repo that has
//! both trees (the `--repo`/cwd repo first); the app shows the diff's most
//! recent review. The report counts the diff's categorized files.

use polygloss_core::ipc::Op;
use polygloss_core::urls::{PolyglossUrl, format_url};
use serde_json::json;

use crate::cli::{GlobalArgs, OpenArgs};
use crate::output::{CliError, Report};

use super::app::{self, AppShown};

pub fn run(args: OpenArgs, global: &GlobalArgs) -> Result<Report, CliError> {
    let cwd = super::work_dir(None, global)?;
    let core = super::core()?;
    let (repo, diff) = core.find_repo_for_diff(args.diff.trim(), Some(&cwd))?;
    let review_id = core.latest_review_for_diff(diff.as_str())?;
    let files = core.files_for_diff(&diff)?.unwrap_or_default();
    let counts = super::category_counts(&core, &files)?;
    let url = format_url(&PolyglossUrl::Diff {
        diff_id: diff.as_str().to_owned(),
        path: None,
        side: None,
        line: None,
    });
    let shown = if global.no_open {
        AppShown::Skipped
    } else {
        let op = Op::Open {
            review_id: None,
            diff_id: Some(diff.as_str().to_owned()),
            activate: true,
        };
        app::show(&core.paths, op)?
    };
    let repo = repo.toplevel.unwrap_or(repo.common_dir);
    let mut json = json!({
        "review_id": review_id,
        "diff_id": diff.as_str(),
        "url": url,
        "repo": repo.to_string_lossy(),
        "app": shown.as_str(),
    });
    super::add_categories(&mut json, &counts);
    let mut human = format!(
        "Diff {}  {}\n",
        diff.short(),
        super::files_text(files.len(), &counts)
    );
    if let Some(review_id) = &review_id {
        human.push_str(&format!("  review     {review_id}\n"));
    }
    human.push_str(&format!(
        "  repo       {}\n  url        {url}\n",
        repo.display()
    ));
    match shown {
        AppShown::Opened => human.push_str("Opened in Polygloss.\n"),
        AppShown::Launched => human.push_str("Launched Polygloss.\n"),
        AppShown::Skipped => {}
    }
    Ok(Report { json, human })
}
