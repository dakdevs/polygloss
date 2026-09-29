//! Polygloss diff model. The leaf crate: no git, IO, SQLite or GPUI.
//!
//! Line numbers are 0-based `u32` everywhere in this crate; `polygloss-core`
//! converts them to the 1-based anchors of the store, MCP and JSON surfaces.
#![forbid(unsafe_code)]

pub mod hunks;
pub mod line_map;
pub mod lines;
pub mod options;
pub mod rows;
pub mod types;
pub mod unified_text;
pub mod whitespace;
pub mod word;

pub use types::{
    FileChange, FileKind, FileStatus, GitPath, Mode, ObjectFormat, Oid, OidError, Side,
};
