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
