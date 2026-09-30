//! `nudge_app` (T4.4, design §15.1 "App dependency"): after a write, a running
//! app gets one best-effort `store_changed`; a missing or silent app never
//! blocks the caller for long, and the nudge never launches the app.

use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use polygloss_core::paths::socket_path_fits;
use polygloss_core::review::Core;
use polygloss_core::testing::Sandbox;
use polygloss_mcp::context::NUDGE_TIMEOUT;
use polygloss_mcp::{ApiContext, nudge_app};
use polygloss_platform::launch::Launcher;
use serde_json::{Value, json};

/// Records every launch instead of starting anything.
#[derive(Default)]
struct RecordingLauncher {
    calls: Mutex<Vec<(Option<String>, bool)>>,
}

impl Launcher for RecordingLauncher {
    fn launch(&self, url: Option<&str>, activate: bool) -> std::io::Result<()> {
        self.calls
            .lock()
            .unwrap()
            .push((url.map(str::to_owned), activate));
        Ok(())
    }
}

fn context(launcher: Arc<RecordingLauncher>) -> ApiContext {
    ApiContext {
        core: Core::open_default().expect("open sandbox store"),
        session_id: "pg-test-session".into(),
        client_name: "tests".into(),
        launcher,
        roots: Vec::<PathBuf>::new(),
    }
}

#[test]
fn nudge_sends_one_store_changed_to_a_running_app() {
    let _sb = Sandbox::isolate();
    let launcher = Arc::new(RecordingLauncher::default());
    let ctx = context(launcher.clone());
    let socket = ctx.core.paths.socket.clone();
    assert!(socket_path_fits(&socket), "{}", socket.display());

    let listener = UnixListener::bind(&socket).unwrap();
    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let req: Value = serde_json::from_str(&line).unwrap();
        let resp = json!({ "id": req["id"], "ok": true, "result": {} });
        (&stream).write_all(format!("{resp}\n").as_bytes()).unwrap();
        // The client sends nothing else on this connection.
        let mut rest = String::new();
        let n = reader.read_line(&mut rest).unwrap_or(0);
        (req, n)
    });

    nudge_app(&ctx, 42);
    drop(ctx);
    let (req, extra) = server.join().unwrap();
    assert_eq!(req["v"], json!(1));
    assert_eq!(req["op"], json!("store_changed"));
    assert_eq!(req["seq"], json!(42));
    assert!(req["id"].is_u64(), "{req}");
    assert_eq!(extra, 0, "one request per nudge");
    assert!(launcher.calls.lock().unwrap().is_empty());
}

#[test]
fn nudge_without_an_app_neither_launches_nor_waits() {
    let _sb = Sandbox::isolate();
    let launcher = Arc::new(RecordingLauncher::default());
    let ctx = context(launcher.clone());

    // No socket at all.
    let started = Instant::now();
    nudge_app(&ctx, 1);
    // A stale socket file whose app is gone (connection refused).
    let listener = UnixListener::bind(&ctx.core.paths.socket).unwrap();
    drop(listener);
    assert!(ctx.core.paths.socket.exists());
    nudge_app(&ctx, 2);
    assert!(started.elapsed() < Duration::from_millis(150));
    assert!(launcher.calls.lock().unwrap().is_empty());
}

#[test]
fn nudge_to_a_silent_app_is_bounded_by_the_timeout() {
    let _sb = Sandbox::isolate();
    let launcher = Arc::new(RecordingLauncher::default());
    let ctx = context(launcher.clone());
    let listener = UnixListener::bind(&ctx.core.paths.socket).unwrap();
    // Accept and read, but never answer.
    let silent = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(&stream).read_line(&mut line).unwrap();
        std::thread::sleep(NUDGE_TIMEOUT * 3);
        line
    });

    let started = Instant::now();
    nudge_app(&ctx, 7);
    let took = started.elapsed();
    assert!(took >= NUDGE_TIMEOUT, "{took:?}");
    assert!(took < NUDGE_TIMEOUT * 3, "{took:?}");
    assert!(silent.join().unwrap().contains("store_changed"));
    assert!(launcher.calls.lock().unwrap().is_empty());
}
