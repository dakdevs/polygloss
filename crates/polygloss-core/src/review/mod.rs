//! The review domain: reviews, iterations, threads, drafts, submissions, Viewed,
//! view state, sessions and carry-forward (T1.12–T1.15). Converts the 0-based lines
//! of `polygloss-diff` to the 1-based anchors of the store and agent surfaces.

pub mod carry_forward;
pub mod models;
pub mod open;
pub mod sessions;
pub mod submit;
pub mod suggestions;
pub mod summary;
pub mod threads;
pub mod view_state;
pub mod viewed;
