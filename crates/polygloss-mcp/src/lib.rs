//! Polygloss agent API (transport-agnostic) and the rmcp stdio MCP server
//! (design §15). Never links GPUI or lumis.
//!
//! - [`api`]: one blocking function per MCP tool, shared with the JSON CLI.
//! - [`server`]: the rmcp tool router, instructions and result mapping;
//!   [`serve_stdio`] runs it.
//! - [`context`], [`session`], [`errors`], [`paging`]: what every tool shares.
//! - [`channel`]: opt-in `claude/channel` push (T4.13).
//! - [`wake`]: which submissions wake a session, and the text (`polygloss wait`,
//!   T4.8; the channel push reuses it).
#![forbid(unsafe_code)]

pub mod api;
pub mod channel;
pub mod context;
pub mod errors;
pub mod paging;
pub mod server;
pub mod session;
pub mod wake;

pub use context::{ApiContext, nudge_app};
pub use errors::{ApiError, ApiErrorCode};
pub use paging::{Cursor, PAGE_MAX_CHARS, decode_cursor, encode_cursor, fit_page};
pub use server::{PolyglossServer, ServeOptions, serve_stdio, tool_result};
