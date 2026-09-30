//! `polygloss compare <base> <head> [--direct] [--label <text>]`: a branch
//! compare, three-dot by default (design §3, §14). The label is display only.

use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;

use crate::cli::{CompareArgs, GlobalArgs};
use crate::output::{CliError, Report};

pub fn run(args: CompareArgs, global: &GlobalArgs) -> Result<Report, CliError> {
    let worktree = super::work_dir(None, global)?;
    super::open_and_show(
        global,
        &OpenRequest {
            worktree,
            source: Source::Compare {
                base: args.base,
                head: args.head,
                mode: if args.direct {
                    CompareMode::Direct
                } else {
                    CompareMode::ThreeDot
                },
            },
            label: args.label,
            pin: None,
            actor: Actor::human(),
        },
    )
}
