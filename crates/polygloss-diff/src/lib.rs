//! Polygloss diff model. The leaf crate: no git, IO, SQLite or GPUI.
//!
//! Line numbers are 0-based `u32` everywhere in this crate; `polygloss-core`
//! converts them to the 1-based anchors of the store, MCP and JSON surfaces.
#![forbid(unsafe_code)]

mod git_myers;
pub mod hunks;
pub mod line_map;
pub mod lines;
mod myers_core;
pub mod options;
pub mod rows;
#[cfg(feature = "test-support")]
pub mod testing;
pub mod types;
pub mod unified_text;
pub mod whitespace;
pub mod word;

pub use types::{
    FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, Mode, ObjectFormat, Oid, OidError,
    Side,
};
