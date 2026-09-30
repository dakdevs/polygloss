//! What every `api::*` call runs with: the core, the caller's session and name,
//! the app launcher and the client's roots (design §15.1), plus the two app-facing
//! helpers every tool shares: the default repo and the `store_changed` nudge.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use polygloss_core::git;
use polygloss_core::ipc::client::IpcClient;
use polygloss_core::ipc::protocol::Op;
use polygloss_core::review::{Author, AuthorKind, Core};
use polygloss_core::store::events::{Actor, ActorKind};
use polygloss_platform::launch::Launcher;

/// The environment variable Claude Code sets to the project root for spawned
/// MCP servers.
pub const PROJECT_DIR_ENV: &str = "CLAUDE_PROJECT_DIR";

/// How long the best-effort `store_changed` nudge may take.
pub const NUDGE_TIMEOUT: Duration = Duration::from_millis(200);

/// The context of one API call. Cheap to clone.
#[derive(Clone)]
pub struct ApiContext {
    pub core: Core,
    /// This process's session id (`CLAUDE_CODE_SESSION_ID` or `pg-<uuidv7>`), as
    /// upserted into `sessions`; core follows `canonical_id` where it matters.
    pub session_id: String,
    /// MCP `clientInfo.name` (e.g. `claude-code`); for the JSON CLI, `--agent`.
    pub client_name: String,
    /// Launches the app for UI side effects (`open_diff` with `show`, `focus`,
    /// `request_rereview`). Never used at startup or by the nudge.
    pub launcher: Arc<dyn Launcher + Send + Sync>,
    /// The client's `roots/list` directories (empty when the client has none).
    pub roots: Vec<PathBuf>,
}

impl std::fmt::Debug for ApiContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApiContext")
            .field("session_id", &self.session_id)
            .field("client_name", &self.client_name)
            .field("roots", &self.roots)
            .finish_non_exhaustive()
    }
}

impl ApiContext {
    /// The caller as a comment author: an agent named after the client, in this
    /// session.
    pub fn author(&self) -> Author {
        Author {
            kind: AuthorKind::Agent,
            name: self.client_name.clone(),
            session_id: Some(self.session_id.clone()),
        }
    }

    /// The caller as an event actor.
    pub fn actor(&self) -> Actor {
        Actor {
            kind: ActorKind::Agent,
            name: Some(self.client_name.clone()),
            session_id: Some(self.session_id.clone()),
        }
    }

    /// The repo a call works on (design §15.1 "Repo default"): `repo` when given,
    /// else the first root that is inside a git worktree, else
    /// `$CLAUDE_PROJECT_DIR`, else the current directory.
    pub fn default_repo(&self, repo: Option<&str>) -> PathBuf {
        let project_dir = std::env::var_os(PROJECT_DIR_ENV).map(PathBuf::from);
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        resolve_repo(repo, &self.roots, project_dir.as_deref(), &cwd)
    }
}

/// [`ApiContext::default_repo`] with its inputs explicit. A relative `repo` is
/// taken relative to `cwd`; an empty `project_dir` counts as unset.
pub fn resolve_repo(
    repo: Option<&str>,
    roots: &[PathBuf],
    project_dir: Option<&Path>,
    cwd: &Path,
) -> PathBuf {
    if let Some(repo) = repo.map(str::trim).filter(|r| !r.is_empty()) {
        return cwd.join(repo);
    }
    if let Some(root) = roots.iter().find(|r| is_worktree(r)) {
        return root.clone();
    }
    match project_dir {
        Some(dir) if !dir.as_os_str().is_empty() => cwd.join(dir),
        _ => cwd.to_path_buf(),
    }
}

/// Whether `path` is inside a git worktree (not a bare repo or a git dir).
pub fn is_worktree(path: &Path) -> bool {
    git::discover(path).is_ok_and(|r| r.toplevel.is_some())
}

/// The local path of a `file://` root URI (percent-decoded); `None` for other
/// schemes or remote hosts.
pub fn root_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///abs` (empty host) or `file://localhost/abs`.
    let path = if rest.starts_with('/') {
        rest
    } else {
        rest.strip_prefix("localhost")?
    };
    if !path.starts_with('/') {
        return None;
    }
    let bytes = percent_decode(path)?;
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

fn percent_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    Some(out)
}

/// After a write: if the app is running, tell it to read events now
/// (`store_changed`, design §13.3). Best effort and bounded by [`NUDGE_TIMEOUT`];
/// never launches the app (§15.1 "App dependency").
pub fn nudge_app(ctx: &ApiContext, seq: i64) {
    match IpcClient::connect(&ctx.core.paths) {
        Ok(Some(mut client)) => {
            if let Err(e) = client.call(Op::StoreChanged { seq }, NUDGE_TIMEOUT) {
                tracing::debug!("store_changed nudge failed: {e}");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::debug!("store_changed nudge: cannot reach the app: {e}"),
    }
}
