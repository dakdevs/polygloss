//! `polygloss [--since merge-base|HEAD|<rev>] [<path>]`: a live review of the
//! worktree at `<path>` (default `--repo`, else the cwd), against its
//! merge-base with the default branch by default (design §3, §14). The live
//! state is not pinned (the app snapshots it again when it opens the review);
//! `polygloss snapshot` pins it.

use polygloss_core::git::Source;
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;

use crate::cli::{GlobalArgs, LiveArgs};
use crate::output::{CliError, Report};

pub fn run(args: LiveArgs, global: &GlobalArgs) -> Result<Report, CliError> {
    let worktree = super::work_dir(args.path.as_deref(), global)?;
    super::open_and_show(
        global,
        &OpenRequest {
            worktree,
            source: Source::Live {
                since: super::parse_since(args.since.as_deref()),
            },
            label: None,
            pin: None,
            actor: Actor::human(),
        },
    )
}
