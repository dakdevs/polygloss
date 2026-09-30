//! `polygloss show <rev>`: a commit against its first parent (design §3, §14).

use polygloss_core::git::Source;
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;

use crate::cli::{GlobalArgs, ShowArgs};
use crate::output::{CliError, Report};

pub fn run(args: ShowArgs, global: &GlobalArgs) -> Result<Report, CliError> {
    let worktree = super::work_dir(None, global)?;
    super::open_and_show(
        global,
        &OpenRequest {
            worktree,
            source: Source::Commit { rev: args.rev },
            label: None,
            pin: None,
            actor: Actor::human(),
        },
    )
}
