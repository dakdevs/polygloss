//! GPUI tests of T4.1: the app side of the socket (design §13.3, §13.4).
//! Socket ops reach the main thread through `ipc::IpcBridge` (here polled,
//! as under GPUI's test scheduler foreign threads may not wake the app) and
//! `ipc::handle` carries them out: `open` shows a review's tab, `focus` puts
//! the cursor on a line or thread without ever starting an editor,
//! `store_changed` nudges the store feed, `debug_state` reports the tabs.
//!
//! The single-instance tests hold `app.lock` in this process and run a
//! stand-in app on the sandbox's socket, so a real `Polygloss` second
//! instance hands its argv over and exits before it would open a window.

use std::ffi::OsString;
use std::io;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures::FutureExt as _;
use gpui_kit::Entity;
use polygloss_app::app_state::WATCHER_POLL;
use polygloss_app::editor::{self, EditorHost};
use polygloss_app::feed::StoreFeed;
use polygloss_app::ipc::single_instance::{self, Claim};
use polygloss_app::ipc::{self, IpcServer};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::window;
use polygloss_core::ipc::{IpcClient, IpcError, Op, ServerConfig, codes, serve_with};
use polygloss_core::objects::BlobReader;
use polygloss_core::paths::DataPaths;
use polygloss_core::review::{Author, AuthorKind, NewThread, OpenedDiff, Subject, ThreadKind};
use polygloss_diff::Side;
use polygloss_platform::editor::Spawner;
use polygloss_viewport::CursorPos;
use serde_json::{Value, json};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{Sandbox, code_change_repo};

/// Runs `op` through `ipc::handle` and waits for its result.
fn run(shell: &mut Shell, op: Op) -> Result<Value, IpcError> {
    let task = shell.cx.update(|_, cx| ipc::handle(op, cx));
    draw(shell.cx);
    task.now_or_never().expect("the op finished")
}

/// Records every editor launch (there must be none).
#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<Vec<OsString>>>,
}

impl Spawner for Recorder {
    fn spawn(&self, argv: &[OsString]) -> io::Result<()> {
        self.calls.lock().unwrap().push(argv.to_vec());
        Ok(())
    }
}

fn cursor(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<CursorPos> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor())
}

fn review_tabs(shell: &mut Shell) -> Vec<String> {
    shell.main.read_with(shell.cx, |m, cx| {
        m.tabs()
            .items()
            .iter()
            .filter_map(|i| i.review())
            .map(|t| t.read(cx).review_id.clone())
            .collect()
    })
}

/// Opens the fixture's compare review in the store only (no tab), as the
/// CLI or `polygloss mcp` would.
fn stored_review(shell: &Shell, repo: &std::path::Path) -> OpenedDiff {
    shell
        .core
        .open(&compare_req(repo))
        .expect("open in the store")
}

#[gpui_kit::test]
fn ipc_open_request_opens_tab(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let opened = stored_review(&shell, repo.path());
    assert!(review_tabs(&mut shell).is_empty());

    // The real socket: `serve_app` binds the sandbox's socket; a client on
    // another thread waits while the (polled) bridge hands the op over.
    shell.cx.update(|_, cx| ipc::serve_app(cx));
    let socket = shell
        .cx
        .update(|_, cx| cx.global::<IpcServer>().path().map(|p| p.to_path_buf()))
        .expect("the server listens");
    let paths = DataPaths::resolve().unwrap();
    assert_eq!(socket, polygloss_core::ipc::socket_path(&paths));
    let call = |shell: &mut Shell, op: Op| -> Result<Value, IpcError> {
        let paths = paths.clone();
        let client = std::thread::spawn(move || {
            IpcClient::connect(&paths)
                .expect("connect")
                .expect("the app listens")
                .call(op, Duration::from_secs(20))
        });
        for _ in 0..200 {
            if client.is_finished() {
                break;
            }
            shell.cx.executor().advance_clock(WATCHER_POLL);
            draw(shell.cx);
            std::thread::sleep(Duration::from_millis(5));
        }
        client.join().expect("the client thread")
    };

    let r = call(
        &mut shell,
        Op::Open {
            review_id: Some(opened.review_id.clone()),
            diff_id: None,
            activate: true,
        },
    )
    .expect("open");
    assert_eq!(
        r,
        json!({
            "status": "opened",
            "review_id": opened.review_id,
            "diff_id": opened.diff_id.as_str(),
        })
    );
    assert_eq!(
        review_tabs(&mut shell),
        std::slice::from_ref(&opened.review_id)
    );
    assert_eq!(shell.tabs(), (2, 1));

    // Back to Home, then the same review by diff id: its tab is focused,
    // not opened again.
    shell.cx.simulate_keystrokes("cmd-{");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 0));
    let r = call(
        &mut shell,
        Op::Open {
            review_id: None,
            diff_id: Some(opened.diff_id.as_str().to_owned()),
            activate: false,
        },
    )
    .expect("open by diff id");
    assert_eq!(r["status"], "focused");
    assert_eq!(shell.tabs(), (2, 1));

    // Unknown reviews and diffs are errors with codes.
    let err = call(
        &mut shell,
        Op::Open {
            review_id: Some("no-such-review".into()),
            diff_id: None,
            activate: false,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::NOT_FOUND, "{err}");
    let err = call(
        &mut shell,
        Op::Open {
            review_id: None,
            diff_id: Some("0".repeat(64)),
            activate: false,
        },
    )
    .unwrap_err();
    assert_eq!(err.code, codes::NOT_FOUND, "{err}");
    assert_eq!(shell.tabs(), (2, 1));
}

#[gpui_kit::test]
fn ipc_open_by_diff_id_opens_its_latest_review(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let opened = stored_review(&shell, repo.path());
    let activations = |shell: &mut Shell| shell.cx.update(|_, cx| window::activation_requests(cx));
    let before = activations(&mut shell);

    let r = run(
        &mut shell,
        Op::Open {
            review_id: None,
            diff_id: Some(opened.diff_id.as_str().to_owned()),
            activate: false,
        },
    )
    .expect("open by diff id");
    assert_eq!(r["status"], "opened");
    assert_eq!(r["review_id"], json!(opened.review_id));
    assert_eq!(review_tabs(&mut shell), [opened.review_id]);
    assert_eq!(
        activations(&mut shell),
        before,
        "activate: false stays behind"
    );

    // No target: the app only comes forward.
    let r = run(
        &mut shell,
        Op::Open {
            review_id: None,
            diff_id: None,
            activate: true,
        },
    )
    .unwrap();
    assert_eq!(r, json!({ "status": "activated" }));
    assert_eq!(activations(&mut shell), before + 1);
    let state = run(&mut shell, Op::DebugState).unwrap();
    assert_eq!(state["activations"], json!(before + 1));
}

#[gpui_kit::test]
fn ipc_focus_scrolls_to_line_and_spawns_no_editor(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let recorder = Arc::new(Recorder::default());
    let spawner: Arc<dyn Spawner> = recorder.clone();
    shell
        .cx
        .update(|_, cx| editor::set_host(EditorHost::new(spawner, || None), cx));
    let opened = stored_review(&shell, repo.path());

    // The tab is not open: focus opens it, then puts the cursor on line 5
    // of src/main.rs (the third file) on the new side.
    let r = run(
        &mut shell,
        Op::Focus {
            review_id: Some(opened.review_id.clone()),
            diff_id: None,
            path: Some("src/main.rs".into()),
            side: Some(Side::New),
            line: Some(5),
            thread_id: None,
        },
    )
    .expect("focus");
    assert_eq!(r["status"], "focused");
    let tab = shell.active_review().expect("the review tab is active");
    assert_eq!(
        cursor(&mut shell, &tab),
        Some(CursorPos {
            file_idx: 2,
            side: Side::New,
            line: 4,
            range_start: None,
        })
    );
    let state = run(&mut shell, Op::DebugState).unwrap();
    assert_eq!(
        state["tabs"][0]["cursor"],
        json!({ "path": "src/main.rs", "side": "new", "line": 5 })
    );

    // Side defaults to new; the open tab is reused.
    run(
        &mut shell,
        Op::Focus {
            review_id: None,
            diff_id: Some(opened.diff_id.as_str().to_owned()),
            path: Some("src/config.rs".into()),
            side: None,
            line: Some(3),
            thread_id: None,
        },
    )
    .expect("focus by diff id");
    assert_eq!(
        cursor(&mut shell, &tab),
        Some(CursorPos {
            file_idx: 0,
            side: Side::New,
            line: 2,
            range_start: None,
        })
    );
    assert_eq!(review_tabs(&mut shell).len(), 1);

    // A thread: the cursor goes to its line.
    let blobs = BlobReader::open(&opened.repo).unwrap();
    let thread_id = shell
        .core
        .create_thread(
            &NewThread {
                review_id: opened.review_id.clone(),
                diff_id: opened.diff_id.clone(),
                subject: Subject::Line {
                    path: "src/config.rs".into(),
                    side: Side::New,
                    start_line: 6,
                    line: 6,
                },
                kind: ThreadKind::Comment,
                body_md: "Why a BTreeMap?".into(),
                author: Author {
                    kind: AuthorKind::Agent,
                    name: "claude-code".into(),
                    session_id: None,
                },
            },
            &blobs,
        )
        .expect("create a thread");
    shell.cx.update(|_, cx| polygloss_app::feed::nudge(cx));
    draw(shell.cx);
    run(
        &mut shell,
        Op::Focus {
            review_id: None,
            diff_id: None,
            path: None,
            side: None,
            line: None,
            thread_id: Some(thread_id),
        },
    )
    .expect("focus a thread");
    assert_eq!(
        cursor(&mut shell, &tab),
        Some(CursorPos {
            file_idx: 0,
            side: Side::New,
            line: 5,
            range_start: None,
        })
    );

    // Bad requests.
    let focus = |path: Option<&str>, line: Option<u32>| Op::Focus {
        review_id: Some(opened.review_id.clone()),
        diff_id: None,
        path: path.map(str::to_owned),
        side: None,
        line,
        thread_id: None,
    };
    let code = |shell: &mut Shell, op: Op| run(shell, op).unwrap_err().code;
    assert_eq!(
        code(&mut shell, focus(Some("nope.rs"), Some(1))),
        codes::NOT_FOUND
    );
    assert_eq!(
        code(&mut shell, focus(Some("src/main.rs"), Some(0))),
        codes::BAD_REQUEST
    );
    assert_eq!(code(&mut shell, focus(None, Some(3))), codes::BAD_REQUEST);
    let no_target = Op::Focus {
        review_id: None,
        diff_id: None,
        path: Some("src/main.rs".into()),
        side: None,
        line: Some(1),
        thread_id: None,
    };
    assert_eq!(code(&mut shell, no_target), codes::BAD_REQUEST);

    assert!(
        recorder.calls.lock().unwrap().is_empty(),
        "focus never starts an editor"
    );
}

#[gpui_kit::test]
fn ipc_store_changed_nudges_feed(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let polls = |shell: &mut Shell| shell.cx.update(|_, cx| StoreFeed::global(cx).stats().polls);
    let before = polls(&mut shell);
    // No clock advance: only the nudge makes the feed poll.
    let r = run(&mut shell, Op::StoreChanged { seq: 12 }).unwrap();
    assert_eq!(r, json!({ "status": "nudged" }));
    assert_eq!(polls(&mut shell), before + 1);
}

#[gpui_kit::test]
fn ipc_debug_state_reports_tabs_banners_badge_and_events(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let state = run(&mut shell, Op::DebugState).unwrap();
    assert_eq!(state["window_open"], true);
    assert_eq!(state["tabs"], json!([]));
    assert_eq!(state["focused_tab"], Value::Null);
    assert_eq!(state["badge"], 0);
    assert_eq!(state["banners"], json!([]));

    let tab = shell.open(compare_req(repo.path())).unwrap();
    let (review_id, diff_id) = tab.read_with(shell.cx, |t, _| {
        (t.review_id.clone(), t.opened.diff_id.as_str().to_owned())
    });
    let state = run(&mut shell, Op::DebugState).unwrap();
    assert_eq!(state["focused_tab"], json!(review_id));
    let tabs = state["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0]["review_id"], json!(review_id));
    assert_eq!(tabs[0]["diff_id"], json!(diff_id));
    assert_eq!(tabs[0]["active"], true);
    assert_eq!(tabs[0]["anchor"]["path"], "src/config.rs");
    assert_eq!(tabs[0]["anchor"]["line"], 1);
    assert!(state["events_seen"].is_u64());
    // The feed's own counters, so E2E suites can tell it has opened (it
    // starts at the latest event) and never failed to read the store.
    let stats = shell.cx.update(|_, cx| StoreFeed::global(cx).stats());
    let polls = state["feed_polls"].as_u64().unwrap();
    assert!(polls > 0 && polls <= stats.polls, "{polls} vs {stats:?}");
    assert_eq!(state["feed_errors"], json!(stats.errors));
}

// ---------------------------------------------------------------------------
// Single instance

/// A stand-in app on the sandbox's socket that records every op.
fn fake_app(paths: &DataPaths) -> (Arc<Mutex<Vec<Op>>>, polygloss_core::ipc::ServerHandle) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    let server = serve_with(paths, ServerConfig::from_lookup(|_| None), move |op: Op| {
        s.lock().unwrap().push(op);
        Ok(json!({ "status": "opened" }))
    })
    .expect("serve");
    (seen, server)
}

#[test]
fn single_instance_claim_is_exclusive() {
    let _sb = Sandbox::isolate();
    let paths = DataPaths::resolve().unwrap();
    let first = single_instance::claim(&paths).unwrap();
    assert!(matches!(first, Claim::Primary(_)));
    assert!(paths.app_lock.exists());
    assert!(matches!(
        single_instance::claim(&paths).unwrap(),
        Claim::Secondary
    ));
    drop(first);
    let again = single_instance::claim(&paths).unwrap();
    assert!(matches!(again, Claim::Primary(_)));
    drop(again);

    // A live app on the socket without the lock (an older build) wins too.
    let (_, server) = fake_app(&paths);
    assert!(matches!(
        single_instance::claim(&paths).unwrap(),
        Claim::Secondary
    ));
    drop(server);
}

#[test]
fn forward_opens_the_review_and_asks_the_app_to_show_it() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let paths = DataPaths::resolve().unwrap();
    let (seen, _server) = fake_app(&paths);

    single_instance::forward(
        Some(&compare_req(repo.path())),
        &paths,
        Duration::from_secs(2),
    )
    .expect("forward");
    let core = polygloss_core::review::Core::with_paths(paths.clone()).unwrap();
    let review_id = core.open(&compare_req(repo.path())).unwrap().review_id;
    assert_eq!(
        *seen.lock().unwrap(),
        [Op::Open {
            review_id: Some(review_id),
            diff_id: None,
            activate: true
        }]
    );

    // Without argv the app is only brought forward.
    single_instance::forward(None, &paths, Duration::from_secs(2)).unwrap();
    assert_eq!(
        seen.lock().unwrap().last(),
        Some(&Op::Open {
            review_id: None,
            diff_id: None,
            activate: true
        })
    );
}

#[test]
fn forward_without_a_socket_fails_after_waiting() {
    let _sb = Sandbox::isolate();
    let paths = DataPaths::resolve().unwrap();
    let start = Instant::now();
    let err = single_instance::forward(None, &paths, Duration::from_millis(300)).unwrap_err();
    assert!(start.elapsed() >= Duration::from_millis(300));
    assert!(err.contains("does not answer"), "{err}");
}

#[test]
fn second_instance_forwards_argv_and_exits_zero() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let paths = DataPaths::resolve().unwrap();
    // This process is "the running app": it holds the lock and the socket.
    let Claim::Primary(_lock) = single_instance::claim(&paths).unwrap() else {
        panic!("the sandbox has no app yet");
    };
    let (seen, _server) = fake_app(&paths);

    // The sandbox env (HOME, POLYGLOSS_DATA_DIR, git config) is inherited.
    let mut child = Command::new(env!("CARGO_BIN_EXE_Polygloss"))
        .args(["--repo"])
        .arg(repo.path())
        .args(["--compare", "refs/tags/base", "refs/tags/head", "--direct"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start Polygloss");
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            child.kill().ok();
            panic!("the second instance did not exit");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = child.wait_with_output().unwrap();
    assert!(
        status.success(),
        "exit {status:?}; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "{seen:?}");
    let Op::Open {
        review_id: Some(review_id),
        diff_id: None,
        activate: true,
    } = &seen[0]
    else {
        panic!("an activating open of the review: {seen:?}");
    };
    let core = polygloss_core::review::Core::with_paths(paths).unwrap();
    let summary = core.review_summary(review_id).unwrap().expect("the review");
    assert_eq!(
        summary.key, "compare:refs/tags/base..refs/tags/head",
        "the argv's review"
    );
}

#[test]
fn url_op_maps_each_url_form_to_a_socket_op() {
    let diff = "a".repeat(64);
    let review = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
    let thread = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c";
    assert_eq!(
        single_instance::url_op(&format!("polygloss://review/{review}")).unwrap(),
        Op::Open {
            review_id: Some(review.into()),
            diff_id: None,
            activate: false
        }
    );
    assert_eq!(
        single_instance::url_op(&format!("polygloss://diff/{diff}")).unwrap(),
        Op::Open {
            review_id: None,
            diff_id: Some(diff.clone()),
            activate: false
        }
    );
    assert_eq!(
        single_instance::url_op(&format!(
            "polygloss://diff/{diff}?path=src/a%20b.rs&side=old&line=7"
        ))
        .unwrap(),
        Op::Focus {
            review_id: None,
            diff_id: Some(diff.clone()),
            path: Some("src/a b.rs".into()),
            side: Some(Side::Old),
            line: Some(7),
            thread_id: None
        }
    );
    assert_eq!(
        single_instance::url_op(&format!("polygloss://thread/{thread}")).unwrap(),
        Op::Focus {
            review_id: None,
            diff_id: None,
            path: None,
            side: None,
            line: None,
            thread_id: Some(thread.into())
        }
    );
    assert!(single_instance::url_op("polygloss://nope/x").is_err());
}

#[test]
fn second_instance_forwards_polygloss_urls_and_exits_zero() {
    let _sb = Sandbox::isolate();
    let paths = DataPaths::resolve().unwrap();
    let Claim::Primary(_lock) = single_instance::claim(&paths).unwrap() else {
        panic!("the sandbox has no app yet");
    };
    let (seen, _server) = fake_app(&paths);
    let review = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b";
    let thread = "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5c";

    let out = Command::new(env!("CARGO_BIN_EXE_Polygloss"))
        .arg(format!("polygloss://review/{review}"))
        .arg(format!("polygloss://thread/{thread}"))
        .stdin(Stdio::null())
        .output()
        .expect("run Polygloss");
    assert!(
        out.status.success(),
        "exit {:?}; stderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    // Only the URLs, in order: no extra bring-forward `open`.
    assert_eq!(
        *seen.lock().unwrap(),
        [
            single_instance::url_op(&format!("polygloss://review/{review}")).unwrap(),
            single_instance::url_op(&format!("polygloss://thread/{thread}")).unwrap(),
        ]
    );
}
