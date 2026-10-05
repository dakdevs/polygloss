//! `polygloss snapshot [<path>]`: pins the live state of the worktree as an
//! iteration of its live review (`pinned_by = manual`, design §5.2, §14). The
//! app is not opened; a running one is nudged.

use polygloss_core::git::Source;
use polygloss_core::review::{OpenRequest, PinnedBy};
use polygloss_core::store::events::Actor;

use crate::cli::{GlobalArgs, SnapshotArgs};
use crate::output::{CliError, Report};

use super::app::AppShown;

pub fn run(args: SnapshotArgs, global: &GlobalArgs) -> Result<Report, CliError> {
    let worktree = super::work_dir(args.path.as_deref(), global)?;
    let core = super::core()?;
    let opened = core.open(&OpenRequest {
        worktree,
        source: Source::Live {
            since: super::parse_since(args.since.as_deref()),
        },
        label: None,
        pin: Some(PinnedBy::Manual),
        actor: Actor::human(),
    })?;
    super::nudge(&core);
    let url = super::url_for(&opened);
    let label = super::review_label(&core, &opened.review_id)?;
    let counts = super::category_counts(&core, &opened.files)?;
    Ok(super::opened_report(
        &opened,
        label.as_deref(),
        &url,
        AppShown::Skipped,
        &counts,
    ))
}
