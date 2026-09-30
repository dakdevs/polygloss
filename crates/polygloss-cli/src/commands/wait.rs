//! `polygloss wait [--session <id>] [--timeout <s>]` (`--session` is the global
//! flag): the Stop-hook waiter
//! (design §16.3, T4.8). The plugin runs it as an `asyncRewake` command hook, so
//! its exit code is the whole protocol:
//!
//! - **0**: nothing to report: no open review is assigned to the session, the
//!   listening window ended (about 30 s before the hook's timeout, so Claude
//!   Code never kills it), or a newer waiter for the session replaced this one.
//! - **2**: a review assigned to the session was submitted; the summary is on
//!   stderr (Claude Code shows it to the agent and wakes the session).
//! - **1**: an error (message on stderr).
//!
//! Steps (§16.3):
//!
//! 1. Session: a non-empty `--session`, else the hook's stdin JSON
//!    `session_id`, else `CLAUDE_CODE_SESSION_ID`. The plugin passes
//!    `--session "$CLAUDE_CODE_SESSION_ID"`, which Claude Code does not document
//!    for hooks, so an empty value falls through to stdin. The session is
//!    upserted with our owner pid (the agent host: our parent, past any shell
//!    the hook ran in), which links a drifted id to its canonical session
//!    (§16.4); an existing row keeps its client name.
//! 2. Pending submissions (after `last_woken_seq`) are reported at once; else,
//!    with no open review assigned, exit 0.
//! 3. Register in `waiters`. A replaced older waiter notices on its next check
//!    and exits 0; if it is still alive after [`REPLACE_GRACE`] and it provably
//!    is that waiter (its executable is `polygloss-cli` and it started before
//!    its row was written, so a process that reused its pid never qualifies,
//!    `process::is_waiter`), it gets `SIGTERM`. A waiter that gets `SIGTERM`
//!    (or `SIGINT`/`SIGHUP`) stops listening, removes its row and exits 0.
//! 4. Poll the event feed (`data_version`, [`POLL`]) for `review.submitted`;
//!    `polygloss_mcp::wake` decides which concern the session.
//! 5. On a hit: set `last_woken_seq`, print the summary, exit 2.
//! 6. Exit 0 [`DEADLINE_MARGIN`] before the deadline (`--timeout`, else
//!    `POLYGLOSS_WAIT_TIMEOUT_S` (OQ-P14), else 3600 s).

use std::io::{IsTerminal as _, Write as _};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Context as _, anyhow, bail};
use clap::Args;
use polygloss_core::process;
use polygloss_core::review::{Core, SessionInfo};
use polygloss_core::store::events::{EventFeed, EventFilter, EventKind, now_ms};
use polygloss_mcp::wake::{self, Wake};
use serde_json::Value;

use crate::cli::GlobalArgs;

/// The session id variable (design §15.1).
pub const SESSION_ENV: &str = "CLAUDE_CODE_SESSION_ID";
/// Overrides the default timeout (OQ-P14).
pub const TIMEOUT_ENV: &str = "POLYGLOSS_WAIT_TIMEOUT_S";
/// The hook's `timeout` in the plugin (design §16.3, OQ-12).
pub const DEFAULT_TIMEOUT_S: u64 = 3600;
/// How long before the deadline the waiter gives up (§16.3 step 6).
pub const DEADLINE_MARGIN: Duration = Duration::from_secs(30);
/// Event feed poll interval (a `PRAGMA data_version` read when nothing changed).
pub const POLL: Duration = Duration::from_millis(250);
/// How often the waiter checks it is still the session's registered waiter.
pub const OWNER_CHECK: Duration = Duration::from_secs(1);
/// How long a replaced waiter gets to exit on its own before `SIGTERM`.
pub const REPLACE_GRACE: Duration = Duration::from_secs(2);
/// How long to wait for the hook's JSON on stdin (Claude Code may leave the
/// pipe open, so the waiter never waits for EOF).
pub const STDIN_WAIT: Duration = Duration::from_secs(2);
/// The executable a replaced waiter must be before it is signaled.
pub const WAITER_EXECUTABLE: &str = "polygloss-cli";
/// Client name for a session the waiter sees first (the MCP server records
/// the real `clientInfo` name when it starts).
pub const UNKNOWN_CLIENT: &str = "agent";
/// Test-only (with `POLYGLOSS_TEST=1`, OQ-P4): pins the owner pid (`0` = none),
/// because every waiter a test spawns shares the test runner as its parent.
pub const OWNER_PID_ENV: &str = "POLYGLOSS_WAIT_OWNER_PID";
/// Shells a hook command may run in; the owner pid is the process above them.
const SHELLS: [&str; 8] = ["sh", "bash", "zsh", "dash", "ksh", "fish", "tcsh", "csh"];

/// `polygloss wait` arguments. `--session` is the global flag
/// ([`GlobalArgs::session`]): default the hook's stdin `session_id`, then
/// `$CLAUDE_CODE_SESSION_ID`; empty means unset.
#[derive(Debug, Clone, Args)]
pub struct WaitArgs {
    /// The hook's timeout in seconds; the waiter exits 30 s before it
    /// (default `$POLYGLOSS_WAIT_TIMEOUT_S`, else 3600).
    #[arg(long, value_name = "SECONDS")]
    pub timeout: Option<u64>,
}

/// How a wait ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// Exit 0.
    Nothing,
    /// Exit 2 with this summary on stderr.
    Woken(String),
}

/// Runs the waiter and maps the outcome to the §16.3 exit codes: 0 and 2 are
/// returned (2 with the summary written to stderr); an error is exit 1 with
/// `polygloss: <message>` on stderr (printed by `main`).
pub fn run(args: WaitArgs, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    match wait(args, global.session.as_deref())? {
        Outcome::Nothing => Ok(ExitCode::SUCCESS),
        Outcome::Woken(text) => {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err, "{text}");
            let _ = err.flush();
            Ok(ExitCode::from(2))
        }
    }
}

fn wait(args: WaitArgs, session_flag: Option<&str>) -> anyhow::Result<Outcome> {
    let env = |k: &str| std::env::var(k).ok();
    let timeout = timeout_secs(args.timeout, env)?;
    let session = resolve_session(session_flag, read_hook_input, env).ok_or_else(|| {
        anyhow!(
            "no session id: pass --session <id>, pipe the Stop hook's JSON on stdin, \
                 or set {SESSION_ENV}"
        )
    })?;
    let budget = listen_budget(timeout);
    let started = Instant::now();

    let core = Core::open_default().context("opening the Polygloss database")?;
    record_session(&core, &session)?;
    let last_woken = core.last_woken_seq(&session)?;
    let mut feed = EventFeed::open(
        &core.paths,
        last_woken,
        EventFilter {
            kinds: Some(vec![EventKind::ReviewSubmitted]),
            ..EventFilter::default()
        },
    )
    .context("opening the event feed")?;

    // Step 2: anything already pending is reported at once.
    let pending = feed.poll()?;
    if let Some(text) = report(&core, &session, &pending)? {
        return Ok(Outcome::Woken(text));
    }
    if core.assigned_open_reviews(&session)?.is_empty() || budget.is_zero() {
        return Ok(Outcome::Nothing);
    }

    // Step 3: register, replacing an older waiter. From here on a termination
    // signal ends the wait normally, so the row is removed on the way out.
    process::catch_termination();
    let pid = i32::try_from(std::process::id()).context("pid out of range")?;
    let deadline_at =
        now_ms().saturating_add(i64::try_from(budget.as_millis()).unwrap_or(i64::MAX));
    let _registration = Registration::new(&core, &session, pid, deadline_at)?;

    // Steps 4-6.
    let mut last_owner_check = Instant::now();
    loop {
        let elapsed = started.elapsed();
        if elapsed >= budget || process::termination_requested() {
            return Ok(Outcome::Nothing);
        }
        std::thread::sleep(POLL.min(budget - elapsed));
        if process::termination_requested() {
            return Ok(Outcome::Nothing);
        }
        if last_owner_check.elapsed() >= OWNER_CHECK {
            last_owner_check = Instant::now();
            if core.waiter_pid(&session)? != Some(pid) {
                // Replaced by a newer waiter for this session.
                return Ok(Outcome::Nothing);
            }
        }
        let events = feed.poll()?;
        if events.is_empty() {
            continue;
        }
        if core.waiter_pid(&session)? != Some(pid) {
            return Ok(Outcome::Nothing);
        }
        if let Some(text) = report(&core, &session, &events)? {
            return Ok(Outcome::Woken(text));
        }
    }
}

/// The wakes among `events` that are still newer than `last_woken_seq` (another
/// waiter may have reported some meanwhile); records the highest seq and
/// returns the combined summary.
fn report(
    core: &Core,
    session: &str,
    events: &[polygloss_core::store::events::Event],
) -> anyhow::Result<Option<String>> {
    if events.is_empty() {
        return Ok(None);
    }
    let last_woken = core.last_woken_seq(session)?;
    let wakes: Vec<Wake> = wake::wakes_for(core, session, events)?
        .into_iter()
        .filter(|w| w.seq > last_woken)
        .collect();
    let Some(max_seq) = wakes.iter().map(|w| w.seq).max() else {
        return Ok(None);
    };
    core.set_last_woken_seq(session, max_seq)?;
    Ok(Some(wake::combined_text(&wakes)))
}

/// Upserts the session with our owner pid, keeping an existing row's client.
fn record_session(core: &Core, session: &str) -> anyhow::Result<()> {
    let existing = core.session(session)?;
    let owner = match test_owner_override(|k| std::env::var(k).ok()) {
        Some(pinned) => pinned,
        None => owner_pid(),
    };
    let info = match existing {
        Some(s) => SessionInfo {
            owner_pid: owner.or(s.owner_pid),
            cwd: None,
            ..s
        },
        None => SessionInfo {
            id: session.to_owned(),
            client_name: UNKNOWN_CLIENT.to_owned(),
            client_version: None,
            owner_pid: owner,
            cwd: std::env::current_dir().ok(),
        },
    };
    core.upsert_session(&info)?;
    Ok(())
}

/// This waiter's row in `waiters`; removed on drop if it is still ours.
struct Registration<'a> {
    core: &'a Core,
    session: String,
    pid: i32,
}

impl<'a> Registration<'a> {
    fn new(core: &'a Core, session: &str, pid: i32, deadline_at: i64) -> anyhow::Result<Self> {
        if let Some(old) = core.register_waiter(session, pid, deadline_at)? {
            retire_replaced_waiter(old.pid, old.started_at);
        }
        Ok(Registration {
            core,
            session: session.to_owned(),
            pid,
        })
    }
}

impl Drop for Registration<'_> {
    fn drop(&mut self) {
        if let Err(err) = self.core.remove_waiter(&self.session, self.pid) {
            warn(&format!("could not remove the waiter row: {err}"));
        }
    }
}

/// Gives a replaced waiter [`REPLACE_GRACE`] to exit on its own (it polls its
/// row), then sends `SIGTERM` only if the process is provably that waiter: its
/// executable is `polygloss-cli` and it started no later than its registration
/// at `registered_at` (ms), so a process that reused the pid is never signaled.
/// Runs on a detached thread so registration is not delayed.
fn retire_replaced_waiter(old: i32, registered_at: i64) {
    std::thread::spawn(move || {
        let until = Instant::now() + REPLACE_GRACE;
        while Instant::now() < until {
            if !process::is_alive(old) {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        if process::is_waiter(old, registered_at, WAITER_EXECUTABLE) {
            let _ = process::terminate(old);
        }
    });
}

/// The CLI has no tracing subscriber for `wait`: warnings go straight to stderr
/// (never on exit 2's summary path).
fn warn(msg: &str) {
    eprintln!("polygloss wait: warning: {msg}");
}

/// The agent host that owns this waiter: our parent, skipping shells the hook
/// command ran in (`sh -c …`). `None` when orphaned (reparented to launchd).
fn owner_pid() -> Option<i32> {
    let mut pid = i32::try_from(std::os::unix::process::parent_id()).ok()?;
    for _ in 0..4 {
        if pid <= 1 {
            return None;
        }
        let is_shell = process::executable_name(pid).is_some_and(|n| SHELLS.contains(&n.as_str()));
        if !is_shell {
            break;
        }
        pid = process::parent_pid(pid)?;
    }
    (pid > 1).then_some(pid)
}

/// The pinned owner pid when [`OWNER_PID_ENV`] is set in test mode:
/// `Some(None)` for `0`, `Some(Some(pid))` otherwise; `None` = not pinned.
pub fn test_owner_override(env: impl Fn(&str) -> Option<String>) -> Option<Option<i32>> {
    if env(polygloss_core::ipc::TEST_ENV).as_deref() != Some("1") {
        return None;
    }
    let pid: i32 = env(OWNER_PID_ENV)?.trim().parse().ok()?;
    Some((pid > 0).then_some(pid))
}

/// The session per §16.3 step 1: a non-empty `flag`, else the hook input's
/// `session_id` (read lazily), else the non-empty environment variable.
pub fn resolve_session(
    flag: Option<&str>,
    hook_input: impl FnOnce() -> Option<Value>,
    env: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    non_empty(flag.map(str::to_owned))
        .or_else(|| non_empty(hook_input().and_then(|v| session_from_hook_input(&v))))
        .or_else(|| non_empty(env(SESSION_ENV)))
}

/// The `session_id` of a hook's stdin JSON.
pub fn session_from_hook_input(input: &Value) -> Option<String> {
    input.get("session_id")?.as_str().map(str::to_owned)
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// The hook's JSON from stdin: the first JSON value, without waiting for EOF,
/// at most [`STDIN_WAIT`]; `None` for a terminal, empty or non-JSON stdin.
fn read_hook_input() -> Option<Value> {
    if std::io::stdin().is_terminal() {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    // Detached: if nothing arrives, the blocked reader dies with the process.
    std::thread::spawn(move || {
        let stdin = std::io::stdin().lock();
        let value = serde_json::Deserializer::from_reader(stdin)
            .into_iter::<Value>()
            .next()
            .and_then(Result::ok);
        let _ = tx.send(value);
    });
    rx.recv_timeout(STDIN_WAIT).ok().flatten()
}

/// The hook timeout in seconds: `--timeout`, else [`TIMEOUT_ENV`], else
/// [`DEFAULT_TIMEOUT_S`].
pub fn timeout_secs(
    flag: Option<u64>,
    env: impl Fn(&str) -> Option<String>,
) -> anyhow::Result<u64> {
    if let Some(t) = flag {
        return Ok(t);
    }
    match env(TIMEOUT_ENV).map(|v| v.trim().to_owned()) {
        None => Ok(DEFAULT_TIMEOUT_S),
        Some(v) if v.is_empty() => Ok(DEFAULT_TIMEOUT_S),
        Some(v) => match v.parse::<u64>() {
            Ok(t) => Ok(t),
            Err(_) => bail!("{TIMEOUT_ENV}={v:?} is not a number of seconds"),
        },
    }
}

/// How long the waiter listens: the timeout minus [`DEADLINE_MARGIN`] (zero
/// when the timeout is that short).
pub fn listen_budget(timeout_s: u64) -> Duration {
    Duration::from_secs(timeout_s).saturating_sub(DEADLINE_MARGIN)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn env_with(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| {
            pairs
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        }
    }

    #[test]
    fn session_prefers_flag_then_hook_stdin_then_env() {
        let env = env_with(&[(SESSION_ENV, "from-env")]);
        let hook = || Some(json!({ "session_id": "from-hook", "hook_event_name": "Stop" }));
        assert_eq!(
            resolve_session(Some("from-flag"), hook, &env).as_deref(),
            Some("from-flag")
        );
        assert_eq!(
            resolve_session(None, hook, &env).as_deref(),
            Some("from-hook")
        );
        assert_eq!(
            resolve_session(None, || None, &env).as_deref(),
            Some("from-env")
        );
        assert_eq!(resolve_session(None, || None, env_with(&[])), None);
    }

    #[test]
    fn empty_values_fall_through() {
        // The plugin passes `--session "$CLAUDE_CODE_SESSION_ID"`, which may be empty.
        let env = env_with(&[(SESSION_ENV, " ")]);
        let hook = || Some(json!({ "session_id": "from-hook" }));
        assert_eq!(
            resolve_session(Some(""), hook, &env).as_deref(),
            Some("from-hook")
        );
        assert_eq!(
            resolve_session(Some("  "), || Some(json!({ "session_id": "" })), &env),
            None
        );
        assert_eq!(
            resolve_session(
                None,
                || Some(json!({ "other": 1 })),
                env_with(&[(SESSION_ENV, "e")])
            )
            .as_deref(),
            Some("e")
        );
    }

    #[test]
    fn stdin_is_not_read_when_the_flag_names_the_session() {
        let read = std::cell::Cell::new(false);
        let got = resolve_session(
            Some("s"),
            || {
                read.set(true);
                None
            },
            env_with(&[]),
        );
        assert_eq!(got.as_deref(), Some("s"));
        assert!(!read.get());
    }

    #[test]
    fn timeout_prefers_flag_then_env_then_default() {
        let env = env_with(&[(TIMEOUT_ENV, "90")]);
        assert_eq!(timeout_secs(Some(45), &env).unwrap(), 45);
        assert_eq!(timeout_secs(None, &env).unwrap(), 90);
        assert_eq!(
            timeout_secs(None, env_with(&[])).unwrap(),
            DEFAULT_TIMEOUT_S
        );
        assert_eq!(
            timeout_secs(None, env_with(&[(TIMEOUT_ENV, "")])).unwrap(),
            DEFAULT_TIMEOUT_S
        );
        assert!(timeout_secs(None, env_with(&[(TIMEOUT_ENV, "soon")])).is_err());
    }

    #[test]
    fn listens_until_thirty_seconds_before_the_deadline() {
        assert_eq!(listen_budget(3600), Duration::from_secs(3570));
        assert_eq!(listen_budget(31), Duration::from_secs(1));
        assert_eq!(listen_budget(30), Duration::ZERO);
        assert_eq!(listen_budget(0), Duration::ZERO);
    }

    #[test]
    fn owner_override_only_in_test_mode() {
        let test = |v: &str| env_with(&[("POLYGLOSS_TEST", "1"), (OWNER_PID_ENV, v)]);
        assert_eq!(test_owner_override(test("0")), Some(None));
        assert_eq!(test_owner_override(test("4242")), Some(Some(4242)));
        assert_eq!(test_owner_override(test("x")), None);
        assert_eq!(
            test_owner_override(env_with(&[(OWNER_PID_ENV, "4242")])),
            None
        );
        assert_eq!(
            test_owner_override(env_with(&[("POLYGLOSS_TEST", "1")])),
            None
        );
    }

    #[test]
    fn owner_pid_is_the_parent_when_it_is_not_a_shell() {
        // The test harness is not a shell.
        let parent = i32::try_from(std::os::unix::process::parent_id()).unwrap();
        let is_shell =
            process::executable_name(parent).is_some_and(|n| SHELLS.contains(&n.as_str()));
        if !is_shell {
            assert_eq!(owner_pid(), Some(parent));
        }
    }
}
