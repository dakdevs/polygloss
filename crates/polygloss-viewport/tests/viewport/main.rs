//! The single integration test binary for `polygloss-viewport` (one module per
//! feature), so the GPUI-linking crate pays for one test link.

mod blocks;
mod cursor;
mod document;
mod find;
mod headers_gaps;
mod kit;
mod pipeline;
mod provider_swap;
mod support;
mod theme;
mod viewport_render;
