//! The single integration test binary for `polygloss-viewport` (one module per
//! feature), so the GPUI-linking crate pays for one test link.

mod blocks;
mod cards;
mod cursor;
mod document;
mod file_header;
mod find;
mod headers_gaps;
mod kit;
mod pipeline;
mod provider_swap;
mod rows;
mod support;
mod theme;
mod viewport_render;
