//! The rmcp stdio server (design §15.1): the thirteen §15.2 tools as one-line
//! delegations to `api::*`, the §15.4 instructions, session bookkeeping and the
//! mapping of API results to tool results.
//!
//! Rules:
//!
//! - stdout is the JSON-RPC channel; nothing here prints (logging goes to stderr,
//!   set up by the CLI).
//! - Startup never launches the app or connects to it. `initialize` opens the
//!   store and upserts the session (`CLAUDE_CODE_SESSION_ID` or `pg-<uuidv7>`, the
//!   parent pid as `owner_pid`, `clientInfo` name and version).
//! - Every tool call runs its `api::*` function on tokio's blocking pool with an
//!   [`ApiContext`]. The client's roots are fetched with `roots/list` on the first
//!   call of a tool that needs a repo default (`open_diff`; inside that call, as
//!   SEP-2260 requires from 2026-07-28) when the client declared the capability,
//!   and again after `notifications/roots/list_changed`. Tools that name their
//!   review, thread or diff never wait for that round trip.
//! - Results carry `structuredContent` and the same JSON as text; errors are
//!   `isError: true` results with `{code, message}` ([`tool_result`]).
//! - Resources (§15.3) are markdown from `api::resources`; an unknown URI or id
//!   is JSON-RPC error `-32002` (resource not found) with `data.code`.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use polygloss_core::review::Core;
use polygloss_platform::launch::Launcher;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, Implementation, InitializeRequestParams, InitializeResult,
    ListResourceTemplatesResult, ListResourcesResult, MetaObject, PaginatedRequestParams,
    ProgressNotificationParam, ReadResourceRequestParams, ReadResourceResponse, ReadResourceResult,
    Resource, ResourceContents, ResourceTemplate, ServerCapabilities, ServerConfig,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{
    ErrorData, Peer, RoleServer, ServerHandler, ServiceExt, tool, tool_handler, tool_router,
};
use serde::Serialize;
use serde_json::json;

use crate::api::{self, WaitControl};
use crate::context::{ApiContext, root_uri_to_path};
use crate::errors::{ApiError, ApiErrorCode};
use crate::session;

/// The server name in `serverInfo`.
pub const SERVER_NAME: &str = "polygloss";

/// The server instructions (design §15.4, verbatim).
pub const INSTRUCTIONS: &str = include_str!("instructions.md");

/// Environment variable that turns on `--channel` (OQ-33).
pub const CHANNEL_ENV: &str = "POLYGLOSS_MCP_CHANNEL";

/// The `_meta` key that keeps a tool out of Claude Code's deferred tool search.
pub const ALWAYS_LOAD_META: &str = "anthropic/alwaysLoad";

/// How long the first tool call waits for the client's `roots/list` answer.
const ROOTS_TIMEOUT: Duration = Duration::from_secs(2);

/// `polygloss mcp` options.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ServeOptions {
    /// Opt in to `claude/channel` push (`--channel` or `POLYGLOSS_MCP_CHANNEL=1`).
    pub channel: bool,
}

impl ServeOptions {
    /// Options from the `--channel` flag and an environment lookup.
    pub fn from_flag_and_env(channel_flag: bool, env: impl Fn(&str) -> Option<String>) -> Self {
        let env_on = env(CHANNEL_ENV).is_some_and(|v| v.trim() == "1");
        ServeOptions {
            channel: channel_flag || env_on,
        }
    }
}

/// `_meta` with `anthropic/alwaysLoad: true` (design §15.1 "Meta").
pub fn always_load() -> MetaObject {
    let mut meta = MetaObject::new();
    meta.insert(ALWAYS_LOAD_META.to_owned(), json!(true));
    meta
}

/// An API result as a tool result: the value as `structuredContent` plus the same
/// JSON as text, or an `isError: true` result whose content is `{code, message}`.
pub fn tool_result<T: Serialize>(r: Result<T, ApiError>) -> CallToolResult {
    let value = r.and_then(|v| {
        serde_json::to_value(v).map_err(|e| ApiError::internal(format!("encoding result: {e}")))
    });
    match value {
        Ok(v) => CallToolResult::structured(v),
        Err(e) => CallToolResult::structured_error(json!({
            "code": e.code,
            "message": e.message,
        })),
    }
}

struct State {
    opts: ServeOptions,
    session_id: String,
    launcher: Arc<dyn Launcher + Send + Sync>,
    core: Mutex<Option<Core>>,
    /// `clientInfo` name and version, once known.
    client: Mutex<Option<(String, Option<String>)>>,
    session_recorded: AtomicBool,
    /// `None` until fetched, and again after `roots/list_changed`.
    roots: tokio::sync::Mutex<Option<Vec<PathBuf>>>,
    /// Cancel flags of `wait_for_review` calls, set when the server stops so
    /// a pending wait never outlives stdin (the runtime waits for blocking tasks).
    waits: Mutex<Vec<Weak<AtomicBool>>>,
}

/// The Polygloss MCP server. Cheap to clone.
#[derive(Clone)]
pub struct PolyglossServer {
    tool_router: ToolRouter<PolyglossServer>,
    state: Arc<State>,
}

impl PolyglossServer {
    /// A server for session `session_id` that launches the app through `launcher`.
    pub fn new(
        opts: ServeOptions,
        session_id: String,
        launcher: Arc<dyn Launcher + Send + Sync>,
    ) -> PolyglossServer {
        PolyglossServer {
            tool_router: Self::tool_router(),
            state: Arc::new(State {
                opts,
                session_id,
                launcher,
                core: Mutex::new(None),
                client: Mutex::new(None),
                session_recorded: AtomicBool::new(false),
                roots: tokio::sync::Mutex::new(None),
                waits: Mutex::new(Vec::new()),
            }),
        }
    }

    /// This process's session id.
    pub fn session_id(&self) -> &str {
        &self.state.session_id
    }

    /// Cancels every pending `wait_for_review` (the server is stopping).
    pub fn cancel_waits(&self) {
        let waits =
            std::mem::take(&mut *self.state.waits.lock().unwrap_or_else(|p| p.into_inner()));
        for flag in waits.iter().filter_map(Weak::upgrade) {
            flag.store(true, Ordering::SeqCst);
        }
    }

    /// Registers a wait's cancel flag for [`PolyglossServer::cancel_waits`].
    fn track_wait(&self, ctl: &WaitControl) {
        let mut waits = self.state.waits.lock().unwrap_or_else(|p| p.into_inner());
        waits.retain(|w| w.strong_count() > 0);
        waits.push(Arc::downgrade(&ctl.cancel_flag()));
    }

    /// The core, opening the store on first use (blocking).
    fn core_blocking(state: &State) -> Result<Core, ApiError> {
        let mut slot = state.core.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(core) = slot.as_ref() {
            return Ok(core.clone());
        }
        let core = Core::open_default()
            .map_err(|e| ApiError::internal(format!("opening the Polygloss store: {e}")))?;
        *slot = Some(core.clone());
        Ok(core)
    }

    fn client(&self) -> Option<(String, Option<String>)> {
        self.state
            .client
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn set_client(&self, name: String, version: Option<String>) {
        *self.state.client.lock().unwrap_or_else(|p| p.into_inner()) = Some((name, version));
    }

    /// Opens the store and upserts the session (blocking). Idempotent once it
    /// succeeded.
    fn record_session_blocking(
        state: &State,
        name: &str,
        version: Option<&str>,
    ) -> Result<Core, ApiError> {
        let core = Self::core_blocking(state)?;
        if !state.session_recorded.load(Ordering::SeqCst) {
            let info = session::session_info(&state.session_id, name, version);
            session::record_session(&core, &info)?;
            state.session_recorded.store(true, Ordering::SeqCst);
        }
        Ok(core)
    }

    /// The client's roots, fetched once per `roots/list_changed`.
    async fn roots(&self, rc: &RequestContext<RoleServer>) -> Vec<PathBuf> {
        let supports_roots = rc
            .peer
            .peer_info()
            .is_some_and(|p| p.capabilities.roots.is_some());
        if !supports_roots {
            return Vec::new();
        }
        let mut slot = self.state.roots.lock().await;
        if let Some(roots) = slot.as_ref() {
            return roots.clone();
        }
        #[allow(deprecated)] // roots are deprecated by SEP-2577 but Claude Code answers them
        let fetched = tokio::time::timeout(ROOTS_TIMEOUT, rc.peer.list_roots()).await;
        let roots: Vec<PathBuf> = match fetched {
            Ok(Ok(res)) => root_paths(&res),
            Ok(Err(e)) => {
                tracing::debug!("roots/list failed: {e}");
                Vec::new()
            }
            Err(_) => {
                tracing::debug!("roots/list timed out");
                Vec::new()
            }
        };
        *slot = Some(roots.clone());
        roots
    }

    /// The context of one call: the store, the recorded session, the caller's
    /// name and, when `with_roots`, the roots (else none: the call names its
    /// review, thread or diff and needs no repo default).
    async fn context(
        &self,
        rc: &RequestContext<RoleServer>,
        with_roots: bool,
    ) -> Result<ApiContext, ApiError> {
        let (name, version) = match self.client() {
            Some(c) => c,
            None => {
                let from_meta = rc.meta.client_info();
                let c = match from_meta {
                    Some(info) => (info.name, Some(info.version)),
                    None => (session::UNKNOWN_CLIENT.to_owned(), None),
                };
                self.set_client(c.0.clone(), c.1.clone());
                c
            }
        };
        let state = self.state.clone();
        let client_name = name.clone();
        let core = tokio::task::spawn_blocking(move || {
            Self::record_session_blocking(&state, &name, version.as_deref())
        })
        .await
        .map_err(|e| ApiError::internal(format!("session setup panicked: {e}")))??;
        let roots = if with_roots {
            self.roots(rc).await
        } else {
            Vec::new()
        };
        Ok(ApiContext {
            core,
            session_id: self.state.session_id.clone(),
            client_name,
            launcher: self.state.launcher.clone(),
            roots,
        })
    }

    /// Runs `f` with a fresh context (no roots) on the blocking pool and maps
    /// the result.
    async fn run<T, F>(
        &self,
        rc: &RequestContext<RoleServer>,
        f: F,
    ) -> Result<CallToolResult, ErrorData>
    where
        T: Serialize + Send + 'static,
        F: FnOnce(&ApiContext) -> Result<T, ApiError> + Send + 'static,
    {
        self.run_in(rc, false, f).await
    }

    /// [`PolyglossServer::run`] for a tool that needs the repo default, so the
    /// context carries the client's roots.
    async fn run_with_roots<T, F>(
        &self,
        rc: &RequestContext<RoleServer>,
        f: F,
    ) -> Result<CallToolResult, ErrorData>
    where
        T: Serialize + Send + 'static,
        F: FnOnce(&ApiContext) -> Result<T, ApiError> + Send + 'static,
    {
        self.run_in(rc, true, f).await
    }

    async fn run_in<T, F>(
        &self,
        rc: &RequestContext<RoleServer>,
        with_roots: bool,
        f: F,
    ) -> Result<CallToolResult, ErrorData>
    where
        T: Serialize + Send + 'static,
        F: FnOnce(&ApiContext) -> Result<T, ApiError> + Send + 'static,
    {
        let ctx = match self.context(rc, with_roots).await {
            Ok(ctx) => ctx,
            Err(e) => return Ok(tool_result::<()>(Err(e))),
        };
        let r = tokio::task::spawn_blocking(move || f(&ctx))
            .await
            .unwrap_or_else(|e| Err(ApiError::internal(format!("tool panicked: {e}"))));
        Ok(tool_result(r))
    }

    /// Runs `f` with a fresh context on the blocking pool for a non-tool
    /// request (resources): API errors become JSON-RPC errors.
    async fn blocking<T, F>(&self, rc: &RequestContext<RoleServer>, f: F) -> Result<T, ErrorData>
    where
        T: Send + 'static,
        F: FnOnce(&ApiContext) -> Result<T, ApiError> + Send + 'static,
    {
        let ctx = self.context(rc, false).await.map_err(rpc_error)?;
        tokio::task::spawn_blocking(move || f(&ctx))
            .await
            .unwrap_or_else(|e| Err(ApiError::internal(format!("request panicked: {e}"))))
            .map_err(rpc_error)
    }
}

/// An API error as a JSON-RPC error: `not_found` is "resource not found"
/// (`-32002`), `conflict` invalid params, anything else internal; `data.code`
/// keeps the §15.1 code.
pub fn rpc_error(e: ApiError) -> ErrorData {
    let data = Some(json!({ "code": e.code }));
    match e.code {
        ApiErrorCode::NotFound => ErrorData::resource_not_found(e.message, data),
        ApiErrorCode::Conflict => ErrorData::invalid_params(e.message, data),
        _ => ErrorData::internal_error(e.message, data),
    }
}

/// The local directories of a `roots/list` answer (non-`file://` roots dropped).
#[allow(deprecated)] // roots are deprecated by SEP-2577 but Claude Code answers them
fn root_paths(res: &rmcp::model::ListRootsResult) -> Vec<PathBuf> {
    res.roots
        .iter()
        .filter_map(|r| root_uri_to_path(&r.uri))
        .collect()
}

/// A [`WaitControl`] wired to the request: cancelled with its token, and
/// reporting `notifications/progress` when the call carries a `progressToken`.
fn wait_control(rc: &RequestContext<RoleServer>) -> WaitControl {
    let ctl = match rc.meta.get_progress_token() {
        Some(token) => {
            let peer: Peer<RoleServer> = rc.peer.clone();
            let handle = tokio::runtime::Handle::current();
            WaitControl::with_progress(Box::new(move |elapsed, total| {
                let param = ProgressNotificationParam::new(token.clone(), elapsed.as_secs_f64())
                    .with_total(total.as_secs_f64())
                    .with_message("waiting for the review to be submitted");
                let peer = peer.clone();
                handle.spawn(async move {
                    if let Err(e) = peer.notify_progress(param).await {
                        tracing::debug!("progress notification failed: {e}");
                    }
                });
            }))
        }
        None => WaitControl::default(),
    };
    let flag = ctl.cancel_flag();
    let ct = rc.ct.clone();
    tokio::spawn(async move {
        ct.cancelled().await;
        flag.store(true, Ordering::SeqCst);
    });
    ctl
}

#[tool_router(router = tool_router)]
impl PolyglossServer {
    #[tool(
        description = "Show your changes to the human for review. Default source: the live working tree vs the merge-base with the default branch; use source.kind=compare for branch vs branch (add a label) or commit for one commit. Returns review_id, diff_id and the file list.",
        meta = always_load()
    )]
    async fn open_diff(
        &self,
        Parameters(req): Parameters<api::OpenDiffRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run_with_roots(&rc, move |ctx| api::open_diff(ctx, req))
            .await
    }

    #[tool(
        description = "List reviews with status, viewed counts, open threads and questions, and the last submission. Filter by repo, status or assigned=me. Paginated: pass next_cursor."
    )]
    async fn list_reviews(
        &self,
        Parameters(req): Parameters<api::ListReviewsRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::list_reviews(ctx, req)).await
    }

    #[tool(
        description = "List the submitted threads of a review (or diff) with their current positions and last comment. Default status=open. Use since=<seq> for only new activity. Paginated: pass next_cursor.",
        meta = always_load()
    )]
    async fn list_threads(
        &self,
        Parameters(req): Parameters<api::ListThreadsRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::list_threads(ctx, req)).await
    }

    #[tool(
        description = "Get one thread: anchor, original and current code snippets, the diff hunk, every comment with parsed ```suggestion blocks, and who resolved it."
    )]
    async fn get_thread(
        &self,
        Parameters(req): Parameters<api::GetThreadRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::get_thread(ctx, req)).await
    }

    #[tool(
        description = "Reply to a thread (published at once). Say what you changed; set resolve=true only when the thread is fully addressed."
    )]
    async fn reply(
        &self,
        Parameters(req): Parameters<api::ReplyRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::reply(ctx, req)).await
    }

    #[tool(description = "Resolve a thread, optionally with a closing reply.")]
    async fn resolve(
        &self,
        Parameters(req): Parameters<api::ResolveRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::resolve(ctx, req)).await
    }

    #[tool(description = "Reopen a resolved thread.")]
    async fn unresolve(
        &self,
        Parameters(req): Parameters<api::UnresolveRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::unresolve(ctx, req)).await
    }

    #[tool(
        description = "Leave a note (explains a change) or question (asks the human for a decision) on a line, range, file or the whole review. Published at once; at most ~50 per iteration."
    )]
    async fn create_comment(
        &self,
        Parameters(req): Parameters<api::CreateCommentRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::create_comment(ctx, req))
            .await
    }

    #[tool(description = "Edit one of your own comments.")]
    async fn edit_comment(
        &self,
        Parameters(req): Parameters<api::EditCommentRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::edit_comment(ctx, req)).await
    }

    #[tool(
        description = "Delete one of your own comments. A comment with replies leaves a placeholder."
    )]
    async fn delete_comment(
        &self,
        Parameters(req): Parameters<api::DeleteCommentRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::delete_comment(ctx, req))
            .await
    }

    #[tool(
        description = "Block until the human submits (or archives) the review, then return the verdict, summary and new or updated threads. Returns at once if a submission already happened after `since`. Only needed without the Polygloss plugin.",
        meta = always_load()
    )]
    async fn wait_for_review(
        &self,
        Parameters(req): Parameters<api::WaitForReviewRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let ctl = wait_control(&rc);
        self.track_wait(&ctl);
        self.run(&rc, move |ctx| api::wait_for_review(ctx, req, &ctl))
            .await
    }

    #[tool(
        description = "Ask the human to review again after addressing their comments: pins the current working tree as a new iteration and notifies them. Summarize what changed."
    )]
    async fn request_rereview(
        &self,
        Parameters(req): Parameters<api::RequestRereviewRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::request_rereview(ctx, req))
            .await
    }

    #[tool(
        description = "Point the human at a location: scrolls the Polygloss app to a file, line or thread (opening the review if needed). Never opens an editor."
    )]
    async fn focus(
        &self,
        Parameters(req): Parameters<api::FocusRequest>,
        rc: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.run(&rc, move |ctx| api::focus(ctx, req)).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for PolyglossServer {
    fn get_info(&self) -> ServerConfig {
        let mut caps = ServerCapabilities::builder()
            .enable_tools()
            .enable_resources()
            .build();
        crate::channel::declare(&self.state.opts, &mut caps);
        let mut info = InitializeResult::new(caps).with_instructions(INSTRUCTIONS.trim_end());
        info.server_info = Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION"));
        info
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        context.peer.set_peer_info(request.clone());
        let name = request.client_info.name.clone();
        let version = request.client_info.version.clone();
        self.set_client(name.clone(), Some(version.clone()));
        let state = self.state.clone();
        let recorded = tokio::task::spawn_blocking(move || {
            Self::record_session_blocking(&state, &name, Some(&version)).map(|_| ())
        })
        .await;
        // The handshake never fails over the store: tools report it instead.
        match recorded {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::warn!("recording the session failed: {e}"),
            Err(e) => tracing::warn!("recording the session panicked: {e}"),
        }
        self.negotiate_initialize(&request)
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        if !self.state.opts.channel {
            return;
        }
        // `initialize` normally opened the store; retry once if it could not.
        let state = self.state.clone();
        let core = tokio::task::spawn_blocking(move || Self::core_blocking(&state)).await;
        match core {
            Ok(Ok(core)) => crate::channel::start(
                &self.state.opts,
                &core,
                &self.state.session_id,
                &context.peer,
            ),
            Ok(Err(e)) => tracing::warn!("claude/channel disabled: {e}"),
            Err(e) => tracing::warn!("claude/channel disabled: opening the store panicked: {e}"),
        }
    }

    async fn on_roots_list_changed(&self, _context: NotificationContext<RoleServer>) {
        *self.state.roots.lock().await = None;
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let list = self
            .blocking(&context, api::resources::list_resources)
            .await?;
        Ok(ListResourcesResult::with_all_items(
            list.into_iter()
                .map(|r| {
                    Resource::new(r.uri, r.name)
                        .with_title(r.title)
                        .with_description(r.description)
                        .with_mime_type(api::resources::MIME)
                })
                .collect(),
        ))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        Ok(ListResourceTemplatesResult::with_all_items(
            api::resources::resource_templates()
                .into_iter()
                .map(|t| {
                    ResourceTemplate::new(t.uri_template, t.name)
                        .with_title(t.title)
                        .with_description(t.description)
                        .with_mime_type(api::resources::MIME)
                })
                .collect(),
        ))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResponse, ErrorData> {
        let uri = request.uri.clone();
        let md = self
            .blocking(&context, move |ctx| {
                api::resources::read_resource(ctx, &uri)
            })
            .await?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(md, request.uri).with_mime_type(api::resources::MIME),
        ])
        .into())
    }
}

/// Serves MCP over stdin/stdout until the client disconnects. Never launches or
/// contacts the app at startup.
pub async fn serve_stdio(opts: ServeOptions) -> anyhow::Result<()> {
    let launcher: Arc<dyn Launcher + Send + Sync> =
        Arc::new(polygloss_platform::launch::SystemLauncher::from_env());
    serve_stdio_with(opts, launcher).await
}

/// [`serve_stdio`] with an explicit launcher.
pub async fn serve_stdio_with(
    opts: ServeOptions,
    launcher: Arc<dyn Launcher + Send + Sync>,
) -> anyhow::Result<()> {
    let server = PolyglossServer::new(opts, session::session_id_from_env(), launcher);
    let running = server.clone().serve(rmcp::transport::stdio()).await?;
    let waited = running.waiting().await;
    // Blocking waits would keep the runtime (and the process) alive.
    server.cancel_waits();
    waited?;
    Ok(())
}
