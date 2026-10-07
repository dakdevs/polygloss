//! The app socket (T4.1, design §13.2, §13.3): JSON Lines requests and
//! responses, the peer uid check, the socket's location and modes, stale
//! socket replacement and the test-only `debug_state` op.
//!
//! Every test runs under `Sandbox::isolate()`, so `DataPaths::resolve()` puts
//! the socket in a per-test temp data dir. Servers here use stand-in handlers;
//! the app's handler is tested in `polygloss-app`'s `tests/app/ipc.rs`.

use std::ffi::OsString;
use std::io::{BufRead as _, BufReader, Write as _};
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _, PermissionsExt as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use polygloss_core::ipc::{
    IpcClient, IpcError, Op, PROTOCOL_VERSION, ServerConfig, ServerHandle, codes, current_uid,
    serve, serve_at, serve_with, server::IDLE_TIMEOUT, server::check_peer, socket_path,
    socket_path_with,
};
use polygloss_core::paths::{DataPaths, SOCKET_PATH_MAX};
use polygloss_core::testing::Sandbox;
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(5);

fn paths() -> DataPaths {
    DataPaths::resolve().expect("sandbox paths")
}

/// The ops a [`recording`] handler has seen.
type Seen = Arc<Mutex<Vec<Op>>>;

/// A handler that records every op and answers `{"handled": <op name>}`.
fn recording() -> (
    Seen,
    impl Fn(Op) -> Result<Value, IpcError> + Send + 'static,
) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let handler = move |op: Op| {
        let name = op.name();
        s.lock().unwrap().push(op);
        Ok(json!({ "handled": name }))
    };
    (seen, handler)
}

fn config(allow_debug: bool) -> ServerConfig {
    ServerConfig {
        allow_debug,
        expected_uid: current_uid(),
        idle_timeout: IDLE_TIMEOUT,
    }
}

fn client(paths: &DataPaths) -> IpcClient {
    IpcClient::connect(paths)
        .expect("connect")
        .expect("the server listens")
}

/// Sends raw `lines` and reads one response line per line.
fn raw(path: &Path, lines: &[&str]) -> Vec<Value> {
    let mut stream = UnixStream::connect(path).expect("connect");
    stream.set_read_timeout(Some(T)).unwrap();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut out = Vec::new();
    for line in lines {
        stream.write_all(line.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
        let mut resp = String::new();
        reader.read_line(&mut resp).expect("a response line");
        out.push(serde_json::from_str(&resp).expect("a JSON response"));
    }
    out
}

fn mode(path: &Path) -> u32 {
    std::fs::symlink_metadata(path)
        .unwrap()
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn ipc_hello_roundtrip() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    let (seen, handler) = recording();
    let _server = serve(&paths, handler).expect("serve");

    let mut c = client(&paths);
    let hello = c
        .call(
            Op::Hello {
                client: "test".into(),
            },
            T,
        )
        .expect("hello");
    assert_eq!(hello["app"], "polygloss");
    assert_eq!(hello["version"], polygloss_core::VERSION);
    assert_eq!(hello["pid"], std::process::id());
    assert_eq!(hello["protocol"], PROTOCOL_VERSION);
    // `hello` is the server's own; the handler sees the other ops, and one
    // connection carries several requests.
    let r = c.call(Op::StoreChanged { seq: 42 }, T).expect("nudge");
    assert_eq!(r, json!({ "handled": "store_changed" }));
    let r = c
        .call(
            Op::Open {
                review_id: Some("r1".into()),
                diff_id: None,
                activate: true,
            },
            T,
        )
        .expect("open");
    assert_eq!(r, json!({ "handled": "open" }));
    assert_eq!(
        *seen.lock().unwrap(),
        [
            Op::StoreChanged { seq: 42 },
            Op::Open {
                review_id: Some("r1".into()),
                diff_id: None,
                activate: true
            }
        ]
    );

    // The wire format of design §13.3.
    let resp = raw(
        &socket_path(&paths),
        &[r#"{"v":1,"id":7,"op":"hello","client":"raw"}"#],
    );
    assert_eq!(resp[0]["id"], 7);
    assert_eq!(resp[0]["ok"], true);
    assert_eq!(resp[0]["result"]["protocol"], 1);
}

#[test]
fn ipc_handler_errors_are_returned_with_code_and_message() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    let _server = serve(&paths, |_| {
        Err(IpcError::new(codes::NOT_FOUND, "no review r9"))
    })
    .expect("serve");
    let err = client(&paths)
        .call(
            Op::Open {
                review_id: Some("r9".into()),
                diff_id: None,
                activate: false,
            },
            T,
        )
        .unwrap_err();
    assert_eq!(err, IpcError::new(codes::NOT_FOUND, "no review r9"));
}

#[test]
fn ipc_unknown_op_returns_error_code() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    let (seen, handler) = recording();
    let _server = serve(&paths, handler).expect("serve");

    let resp = raw(
        &socket_path(&paths),
        &[
            r#"{"v":1,"id":3,"op":"exec","cmd":"rm -rf /"}"#,
            r#"{"v":2,"id":4,"op":"hello","client":"x"}"#,
            r#"{"v":1,"id":5,"op":"focus","line":"twelve"}"#,
            "not json",
            // The connection survives all of the above.
            r#"{"v":1,"id":6,"op":"store_changed","seq":1}"#,
        ],
    );
    let codes_of: Vec<(Value, Value, Value)> = resp
        .iter()
        .map(|r| (r["id"].clone(), r["ok"].clone(), r["error"]["code"].clone()))
        .collect();
    assert_eq!(
        codes_of,
        [
            (json!(3), json!(false), json!("unknown_op")),
            (json!(4), json!(false), json!("unsupported_version")),
            (json!(5), json!(false), json!("bad_request")),
            (json!(0), json!(false), json!("bad_request")),
            (json!(6), json!(true), Value::Null),
        ]
    );
    assert!(
        resp[0]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("exec")
    );
    assert_eq!(*seen.lock().unwrap(), [Op::StoreChanged { seq: 1 }]);
}

#[test]
fn ipc_rejects_foreign_uid() {
    let _sb = Sandbox::isolate();
    // The check itself.
    assert!(check_peer(501, 501).is_ok());
    let err = check_peer(502, 501).unwrap_err();
    assert_eq!(err.code, codes::FORBIDDEN);

    // A server that expects another uid than ours (the injected uid) turns
    // our connection away before reading a request.
    let paths = paths();
    let (seen, handler) = recording();
    let foreign = ServerConfig {
        allow_debug: true,
        expected_uid: current_uid().wrapping_add(1),
        idle_timeout: IDLE_TIMEOUT,
    };
    let _server = serve_with(&paths, foreign, handler).expect("serve");
    let err = client(&paths)
        .call(Op::StoreChanged { seq: 1 }, T)
        .unwrap_err();
    assert_eq!(err.code, codes::FORBIDDEN, "{err}");
    assert!(seen.lock().unwrap().is_empty());
}

/// `DataPaths` whose data dir is `dir`.
fn paths_at(dir: &Path) -> DataPaths {
    let home = std::env::var_os("HOME").unwrap();
    DataPaths::resolve_with(|k| match k {
        "HOME" => Some(home.clone()),
        "POLYGLOSS_DATA_DIR" => Some(OsString::from(dir)),
        _ => None,
    })
    .unwrap()
}

/// A data dir under the sandbox whose socket path is longer than
/// `sun_path` allows.
fn long_data_dir(sb: &Sandbox) -> PathBuf {
    let dir = sb
        .data_dir()
        .join("d".repeat(SOCKET_PATH_MAX + 1 - "/polygloss.sock".len()));
    assert!(dir.join("polygloss.sock").as_os_str().len() > SOCKET_PATH_MAX);
    dir
}

#[test]
fn socket_path_falls_back_to_tmpdir_when_too_long() {
    let sb = Sandbox::isolate();
    let short = paths();
    assert_eq!(socket_path(&short), short.socket);

    // A short $TMPDIR (macOS's own, under /var/folders, leaves too little
    // room for the fallback name).
    let tmp = tempfile::Builder::new()
        .prefix("pg-")
        .tempdir_in("/tmp")
        .unwrap();
    let tmpdir = tmp.path().to_path_buf();
    let long = paths_at(&long_data_dir(&sb));
    assert!(long.socket_path_too_long());
    let fallback = socket_path_with(&long, Some(&tmpdir));
    let dir = tmpdir.join(format!("polygloss-{}", current_uid()));
    assert_eq!(fallback.parent(), Some(dir.as_path()));
    let name = fallback.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("polygloss-") && name.ends_with(".sock") && name.len() == 31,
        "{name}"
    );
    assert!(fallback.as_os_str().len() <= SOCKET_PATH_MAX);
    // Another long data dir gets another socket; the same one the same.
    let other = paths_at(&long_data_dir(&sb).with_extension("x"));
    assert_ne!(socket_path_with(&other, Some(&tmpdir)), fallback);
    assert_eq!(socket_path_with(&long, Some(&tmpdir)), fallback);
    // No usable $TMPDIR: /tmp.
    for bad in [None, Some(Path::new("relative"))] {
        let p = socket_path_with(&long, bad);
        assert!(p.starts_with("/tmp"), "{}", p.display());
    }
    let too_long_tmp = PathBuf::from(format!("/{}", "t".repeat(SOCKET_PATH_MAX)));
    assert!(socket_path_with(&long, Some(&too_long_tmp)).starts_with("/tmp"));

    // The server and the client agree on it: serving with a long data dir
    // binds in the process's $TMPDIR (pointed at the short temp dir).
    // SAFETY: nextest runs each test in its own process; no other thread
    // reads the environment yet.
    unsafe { std::env::set_var("TMPDIR", &tmpdir) };
    let (_, handler) = recording();
    let server = serve(&long, handler).expect("serve at the fallback");
    assert_eq!(server.path(), socket_path(&long));
    assert!(server.path().starts_with(&tmpdir));
    assert!(!long.socket.exists());
    let fallback_dir = server.path().parent().unwrap();
    assert_eq!(mode(fallback_dir), 0o700);
    assert_eq!(
        std::fs::metadata(fallback_dir).unwrap().uid(),
        current_uid()
    );
    let hello = client(&long)
        .call(Op::Hello { client: "t".into() }, T)
        .unwrap();
    assert_eq!(hello["protocol"], 1);
    let bound = server.path().to_path_buf();
    drop(server);
    assert!(!bound.exists(), "the socket file is removed on drop");
}

#[test]
fn socket_dir_0700_socket_0600() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    // The data dir does not exist yet: the server creates it 0700.
    std::fs::remove_dir_all(&paths.data_dir).unwrap();
    let (_, handler) = recording();
    let server = serve(&paths, handler).expect("serve");
    assert_eq!(server.path(), paths.socket);
    assert_eq!(mode(&paths.data_dir), 0o700);
    assert_eq!(mode(&paths.socket), 0o600);
    assert!(
        std::fs::symlink_metadata(&paths.socket)
            .unwrap()
            .file_type()
            .is_socket()
    );
    drop(server);

    // An existing, looser data dir is tightened.
    std::fs::set_permissions(&paths.data_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let (_, handler) = recording();
    let _server = serve(&paths, handler).expect("serve again");
    assert_eq!(mode(&paths.data_dir), 0o700);
    assert_eq!(mode(&paths.socket), 0o600);
}

#[test]
fn stale_socket_replaced_after_probe() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    // A crashed app leaves its socket file behind: nothing accepts on it.
    drop(UnixListener::bind(&paths.socket).expect("bind a soon-stale socket"));
    assert!(paths.socket.exists());
    assert!(
        IpcClient::connect(&paths).unwrap().is_none(),
        "stale = no app"
    );

    let (_, handler) = recording();
    let server = serve(&paths, handler).expect("replaces the stale socket");
    let hello = client(&paths)
        .call(Op::Hello { client: "t".into() }, T)
        .unwrap();
    assert_eq!(hello["pid"], std::process::id());

    // A live server is not displaced: the probe doubles as the
    // single-instance check.
    let (_, handler) = recording();
    let err = serve(&paths, handler).unwrap_err();
    assert_eq!(err.code, codes::ALREADY_RUNNING, "{err}");
    assert!(client(&paths).call(Op::StoreChanged { seq: 1 }, T).is_ok());
    drop(server);
}

#[test]
fn non_socket_file_at_socket_path_is_left_alone() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    std::fs::write(&paths.socket, b"not a socket").unwrap();
    let (_, handler) = recording();
    let err = serve(&paths, handler).unwrap_err();
    assert_eq!(err.code, codes::IO, "{err}");
    assert_eq!(std::fs::read(&paths.socket).unwrap(), b"not a socket");
}

#[test]
fn connect_without_app_returns_none() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    assert!(IpcClient::connect(&paths).unwrap().is_none());
    // After a clean shutdown too.
    let (_, handler) = recording();
    let server = serve(&paths, handler).unwrap();
    server.shutdown();
    assert!(!paths.socket.exists());
    assert!(IpcClient::connect(&paths).unwrap().is_none());
}

#[test]
fn debug_state_disabled_without_test_env() {
    let _sb = Sandbox::isolate();
    // The environment decides.
    let lookup = |v: Option<&str>| {
        ServerConfig::from_lookup(move |k| {
            (k == "POLYGLOSS_TEST")
                .then(|| v.map(OsString::from))
                .flatten()
        })
        .allow_debug
    };
    assert!(!lookup(None));
    assert!(!lookup(Some("0")));
    assert!(!lookup(Some("true")));
    assert!(lookup(Some("1")));

    let paths = paths();
    let (seen, handler) = recording();
    let server = serve_with(&paths, config(false), handler).expect("serve");
    let err = client(&paths).call(Op::DebugState, T).unwrap_err();
    assert_eq!(err.code, codes::UNKNOWN_OP, "{err}");
    assert!(seen.lock().unwrap().is_empty(), "the handler never sees it");
    drop(server);

    let (seen, handler) = recording();
    let _server = serve_with(&paths, config(true), handler).expect("serve");
    let r = client(&paths).call(Op::DebugState, T).unwrap();
    assert_eq!(r, json!({ "handled": "debug_state" }));
    assert_eq!(*seen.lock().unwrap(), [Op::DebugState]);
}

#[test]
fn call_times_out_and_breaks_the_connection() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    let _server = serve(&paths, |_| {
        std::thread::sleep(Duration::from_millis(600));
        Ok(Value::Null)
    })
    .unwrap();
    let mut c = client(&paths);
    let err = c
        .call(Op::StoreChanged { seq: 1 }, Duration::from_millis(100))
        .unwrap_err();
    assert_eq!(err.code, codes::TIMEOUT, "{err}");
    let err = c.call(Op::StoreChanged { seq: 2 }, T).unwrap_err();
    assert_eq!(err.code, codes::DISCONNECTED, "{err}");
}

#[test]
fn concurrent_clients_are_all_answered() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    let (seen, handler) = recording();
    let _server = serve(&paths, handler).unwrap();
    let threads: Vec<_> = (0..8)
        .map(|i| {
            let paths = paths.clone();
            std::thread::spawn(move || {
                let mut c = client(&paths);
                for j in 0..10 {
                    c.call(Op::StoreChanged { seq: i * 100 + j }, T).unwrap();
                }
            })
        })
        .collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(seen.lock().unwrap().len(), 80);
}

#[test]
fn serve_at_binds_a_fake_app_at_any_path() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    std::fs::create_dir_all(&paths.data_dir).unwrap();
    let (seen, handler) = recording();
    let server: ServerHandle = serve_at(&paths.socket, config(false), handler).unwrap();
    client(&paths).call(Op::StoreChanged { seq: 9 }, T).unwrap();
    assert_eq!(*seen.lock().unwrap(), [Op::StoreChanged { seq: 9 }]);
    drop(server);
}

/// A long-lived client (`polygloss mcp`, a waiter) whose connection the
/// server closed after `idle_timeout` reconnects once and succeeds (T5.9 #1).
#[test]
fn call_after_idle_timeout_reconnects() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    let (seen, handler) = recording();
    let short = ServerConfig {
        idle_timeout: Duration::from_millis(200),
        ..config(false)
    };
    let _server = serve_with(&paths, short, handler).unwrap();
    let mut c = client(&paths);
    c.call(Op::StoreChanged { seq: 1 }, T).unwrap();
    // Longer than the server's idle timeout: it closes the connection.
    std::thread::sleep(Duration::from_millis(600));
    let answer = c.call(Op::StoreChanged { seq: 2 }, T).expect("reconnects");
    assert_eq!(answer, json!({ "handled": "store_changed" }));
    // And the reconnected client keeps working.
    c.call(Op::StoreChanged { seq: 3 }, T).unwrap();
    assert_eq!(
        *seen.lock().unwrap(),
        [
            Op::StoreChanged { seq: 1 },
            Op::StoreChanged { seq: 2 },
            Op::StoreChanged { seq: 3 }
        ]
    );
}

/// A slow `open` must not hold up `store_changed` nudges from other
/// connections (T5.9 #2): handlers run concurrently.
#[test]
fn nudge_answers_while_a_slow_op_is_in_flight() {
    let _sb = Sandbox::isolate();
    let paths = paths();
    // The `open` handler is held until the nudge has its answer (or for 2T,
    // past the nudge's own timeout, if handlers ran one at a time).
    let (entered, released) = (
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    let _server = serve(&paths, {
        let (entered, released) = (entered.clone(), released.clone());
        move |op: Op| {
            if matches!(op, Op::Open { .. }) {
                entered.store(true, Ordering::SeqCst);
                let deadline = Instant::now() + 2 * T;
                while !released.load(Ordering::SeqCst) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            Ok(json!({ "handled": op.name() }))
        }
    })
    .unwrap();
    let slow = {
        let paths = paths.clone();
        std::thread::spawn(move || {
            client(&paths).call(
                Op::Open {
                    review_id: Some("r".into()),
                    diff_id: None,
                    activate: false,
                },
                T,
            )
        })
    };
    // The slow op reaches its handler first.
    let deadline = Instant::now() + T;
    while !entered.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "the slow op never reached its handler"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    client(&paths)
        .call(Op::StoreChanged { seq: 1 }, T)
        .expect("the nudge is answered while the slow op is held");
    assert!(!slow.is_finished(), "the slow op is still in flight");
    released.store(true, Ordering::SeqCst);
    slow.join().unwrap().expect("the slow op finishes too");
}

/// The client checks who owns the socket and its directory before it sends
/// anything (T5.9 #4; the `/tmp` fallback could be planted by someone else).
#[test]
fn client_verifies_socket_ownership() {
    use polygloss_core::ipc::client::check_socket_owner;
    let _sb = Sandbox::isolate();
    let paths = paths();
    let (_seen, handler) = recording();
    let _server = serve(&paths, handler).unwrap();
    let path = socket_path(&paths);
    check_socket_owner(&path, current_uid()).expect("our own socket");
    let err = check_socket_owner(&path, current_uid().wrapping_add(1)).unwrap_err();
    assert_eq!(err.code, codes::FORBIDDEN, "{err}");

    // A symlink to our socket is not a socket: refused, nothing is sent.
    let link = paths.data_dir.join("link.sock");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    let err = check_socket_owner(&link, current_uid()).unwrap_err();
    assert_eq!(err.code, codes::FORBIDDEN, "{err}");
    let err = IpcClient::connect_at(&link).unwrap_err();
    assert_eq!(err.code, codes::FORBIDDEN, "{err}");

    // A group- or world-writable directory is refused too.
    let open_dir = paths.data_dir.join("open");
    std::fs::create_dir(&open_dir).unwrap();
    std::fs::set_permissions(&open_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
    let planted = open_dir.join("p.sock");
    let _other = serve_at(&planted, config(false), |_| Ok(Value::Null)).unwrap();
    let err = IpcClient::connect_at(&planted).unwrap_err();
    assert_eq!(err.code, codes::FORBIDDEN, "{err}");
}
