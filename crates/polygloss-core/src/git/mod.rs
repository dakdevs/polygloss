//! The git layer: the system git CLI for revisions, structure and snapshots
//! (design §6.1, ADR-0014). Every call goes through `runner`.

pub mod attrs;
pub mod diff_tree;
pub mod listing;
pub mod repo;
pub mod resolve;
pub mod runner;
pub mod snapshot;
pub mod version;

pub use repo::{RepoInfo, discover};
pub use resolve::{
    CompareMode, HeadSpec, Resolution, ResolveError, ResolveWarning, ResolvedSide, ReviewKind,
    Since, Source, default_branch, resolve,
};
pub use runner::{Git, GitError, GitOutput, git_binary};
pub use version::{GitVersion, check_version};
