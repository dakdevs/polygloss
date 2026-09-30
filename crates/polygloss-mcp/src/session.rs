//! The agent session of one `polygloss mcp` process (design §15.1 "Session",
//! §16.4 session-id drift).
//!
//! - The id is `CLAUDE_CODE_SESSION_ID` when set (and non-empty), otherwise
//!   `pg-<uuidv7>`, fixed for the life of the process. Claude Code documents
//!   `CLAUDE_PROJECT_DIR` for spawned MCP servers but not the session id, and the
//!   id a server was spawned with does not follow `/clear` or `--resume`; the
//!   owner pid below is what links the ids (`sessions.canonical_id`).
//! - The owner pid is our parent process: the agent host that spawned us (the
//!   plugin shim `exec`s the CLI, so the shim adds no process).
//! - It is upserted with the MCP `clientInfo` name and version on `initialize`
//!   (or on the first tool call from a client that never sent one).

use polygloss_core::review::{Core, CoreError, SessionInfo};

/// The environment variable Claude Code may set to the conversation's id.
pub const SESSION_ENV: &str = "CLAUDE_CODE_SESSION_ID";

/// The client name recorded when a client sends no `clientInfo`.
pub const UNKNOWN_CLIENT: &str = "agent";

/// This process's session id from an environment lookup: the non-empty
/// `CLAUDE_CODE_SESSION_ID`, else a fresh `pg-<uuidv7>`.
pub fn session_id_from(env: impl Fn(&str) -> Option<String>) -> String {
    match env(SESSION_ENV) {
        Some(id) if !id.trim().is_empty() => id.trim().to_owned(),
        _ => format!("pg-{}", polygloss_core::new_uuid()),
    }
}

/// [`session_id_from`] over the process environment.
pub fn session_id_from_env() -> String {
    session_id_from(|k| std::env::var(k).ok())
}

/// The process that spawned this one (the agent host), if it is a real parent.
pub fn owner_pid() -> Option<i32> {
    let ppid = std::os::unix::process::parent_id();
    // Reparented to launchd/init: the host is gone.
    (ppid > 1).then(|| i32::try_from(ppid).ok()).flatten()
}

/// The session row for `id` as this process sees it.
pub fn session_info(id: &str, client_name: &str, client_version: Option<&str>) -> SessionInfo {
    SessionInfo {
        id: id.to_owned(),
        client_name: client_name.to_owned(),
        client_version: client_version.map(str::to_owned),
        owner_pid: owner_pid(),
        cwd: std::env::current_dir().ok(),
    }
}

/// Upserts the session and returns its canonical id.
pub fn record_session(core: &Core, info: &SessionInfo) -> Result<String, CoreError> {
    core.upsert_session(info)
}
