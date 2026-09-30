//! Opt-in `claude/channel` push (design §16.4, OQ-33). The server already calls
//! both hooks; T4.13 implements them. Until then `--channel` changes nothing:
//! no capability is declared and nothing is sent.

use polygloss_core::review::Core;
use rmcp::model::ServerCapabilities;
use rmcp::{Peer, RoleServer};

use crate::server::ServeOptions;

/// Adds `experimental["claude/channel"]` to `caps` when `opts.channel` is set.
pub fn declare(_opts: &ServeOptions, _caps: &mut ServerCapabilities) {}

/// Called once the client is initialized: with `opts.channel`, watch the event feed
/// for submissions of reviews assigned to `session_id` and push one
/// `notifications/claude/channel` each through `peer`.
pub fn start(_opts: &ServeOptions, _core: &Core, _session_id: &str, _peer: &Peer<RoleServer>) {}
