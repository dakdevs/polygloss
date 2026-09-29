//! Events and the `data_version` change feed (T1.11, design §7.1, §7.3, ADR-0015).
//!
//! Every test starts with `Sandbox::isolate()`, so `DataPaths::resolve()` points at a
//! per-test temp data dir. `feed_sees_commit_from_other_process` re-runs this test
//! binary as a writer child (`child_process_entry`, a no-op unless
//! `POLYGLOSS_EVENTS_CHILD` is set).

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use polygloss_core::paths::DataPaths;
use polygloss_core::store::Store;
use polygloss_core::store::events::{
    Actor, ActorKind, Event, EventFeed, EventFilter, EventKind, NewEvent, append_event,
    events_since,
};
use polygloss_core::testing::Sandbox;
use serde_json::json;

// ---------------------------------------------------------------------------
// Helpers

fn open_store() -> (DataPaths, Store) {
    let paths = DataPaths::resolve().unwrap();
    let store = Store::open(&paths).unwrap();
    (paths, store)
}

fn event(kind: EventKind, review_id: Option<&str>) -> NewEvent {
    NewEvent {
        kind,
        review_id: review_id.map(str::to_owned),
        diff_id: None,
        thread_id: None,
        comment_id: None,
        actor: Actor::human(),
        payload: serde_json::Value::Null,
    }
}

fn append(store: &Store, e: &NewEvent) -> i64 {
    store.write(|tx| append_event(tx, e)).unwrap()
}

fn all_events(store: &Store, filter: &EventFilter) -> Vec<Event> {
    store
        .read(|conn| events_since(conn, 0, filter, 10_000))
        .unwrap()
}

fn kinds(events: &[Event]) -> Vec<EventKind> {
    events.iter().map(|e| e.kind).collect()
}

/// Polls `feed` until it returns something or `timeout` passes.
fn poll_until_some(feed: &mut EventFeed, timeout: Duration) -> Vec<Event> {
    let deadline = Instant::now() + timeout;
    loop {
        let got = feed.poll().unwrap();
        if !got.is_empty() || Instant::now() >= deadline {
            return got;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ---------------------------------------------------------------------------
// Kinds and actors

#[test]
fn event_kind_strings_match_design() {
    let expected = [
        (EventKind::ReviewCreated, "review.created", true),
        (EventKind::ReviewArchived, "review.archived", true),
        (EventKind::IterationCreated, "iteration.created", true),
        (EventKind::ThreadCreated, "thread.created", true),
        (EventKind::CommentCreated, "comment.created", true),
        (EventKind::CommentEdited, "comment.edited", true),
        (EventKind::CommentDeleted, "comment.deleted", true),
        (EventKind::ThreadResolved, "thread.resolved", true),
        (EventKind::ThreadUnresolved, "thread.unresolved", true),
        (EventKind::ReviewSubmitted, "review.submitted", true),
        (
            EventKind::ReviewRereviewRequested,
            "review.rereview_requested",
            true,
        ),
        (EventKind::ReviewAssigned, "review.assigned", true),
        (EventKind::ViewedChanged, "viewed.changed", false),
        (EventKind::DraftChanged, "draft.changed", false),
    ];
    assert_eq!(EventKind::ALL.len(), expected.len());
    for (kind, s, visible) in expected {
        assert_eq!(kind.as_str(), s);
        assert_eq!(kind.agent_visible(), visible, "{s}");
        assert_eq!(EventKind::parse(s), Some(kind));
        assert!(EventKind::ALL.contains(&kind));
    }
    assert_eq!(EventKind::parse("test.write"), None);

    for (kind, s) in [
        (ActorKind::Human, "human"),
        (ActorKind::Agent, "agent"),
        (ActorKind::System, "system"),
    ] {
        assert_eq!(kind.as_str(), s);
        assert_eq!(ActorKind::parse(s), Some(kind));
    }
}

// ---------------------------------------------------------------------------
// append_event / events_since

#[test]
fn append_event_monotonic_seq() {
    let _sb = Sandbox::isolate();
    let (_paths, store) = open_store();

    let first = append(&store, &event(EventKind::ReviewCreated, Some("r1")));
    // Two events in one transaction.
    let (second, third) = store
        .write(|tx| {
            let a = append_event(tx, &event(EventKind::ThreadCreated, Some("r1")))?;
            let b = append_event(tx, &event(EventKind::CommentCreated, Some("r1")))?;
            Ok((a, b))
        })
        .unwrap();
    assert!(first > 0);
    assert!(first < second && second < third, "{first} {second} {third}");

    // AUTOINCREMENT: a deleted top seq is never handed out again.
    store
        .write(|tx| {
            tx.execute("DELETE FROM events WHERE seq = ?1", [third])?;
            Ok(())
        })
        .unwrap();
    let fourth = append(&store, &event(EventKind::ReviewArchived, Some("r1")));
    assert!(fourth > third, "{fourth} reused a seq <= {third}");

    let events = all_events(&store, &EventFilter::default());
    let seqs: Vec<i64> = events.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, vec![first, second, fourth]);
}

#[test]
fn append_event_roundtrips_every_field() {
    let _sb = Sandbox::isolate();
    let (_paths, store) = open_store();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;

    let new = NewEvent {
        kind: EventKind::ReviewSubmitted,
        review_id: Some("review-1".into()),
        diff_id: Some("ab".repeat(32)),
        thread_id: Some("thread-1".into()),
        comment_id: Some("comment-1".into()),
        actor: Actor {
            kind: ActorKind::Agent,
            name: Some("claude-code".into()),
            session_id: Some("session-1".into()),
        },
        payload: json!({ "submission_id": "s1", "verdict": "approve" }),
    };
    let seq = append(&store, &new);
    let null_payload = append(&store, &event(EventKind::DraftChanged, None));

    let events = all_events(&store, &EventFilter::default());
    assert_eq!(events.len(), 2);
    let got = &events[0];
    assert_eq!(got.seq, seq);
    assert!(got.at >= before, "at {} < {before}", got.at);
    assert_eq!(got.kind, new.kind);
    assert_eq!(got.review_id, new.review_id);
    assert_eq!(got.diff_id, new.diff_id);
    assert_eq!(got.thread_id, new.thread_id);
    assert_eq!(got.comment_id, new.comment_id);
    assert_eq!(got.actor, new.actor);
    assert_eq!(got.payload, new.payload);

    assert_eq!(events[1].seq, null_payload);
    assert_eq!(events[1].payload, serde_json::Value::Null);
    assert_eq!(events[1].actor, Actor::human());
    // A null payload is stored as SQL NULL, not the string "null".
    let raw: Option<String> = store
        .read(|conn| {
            Ok(conn.query_row(
                "SELECT payload FROM events WHERE seq = ?1",
                [null_payload],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(raw, None);
}

#[test]
fn events_since_respects_cursor_and_limit() {
    let _sb = Sandbox::isolate();
    let (_paths, store) = open_store();
    let seqs: Vec<i64> = (0..5)
        .map(|_| append(&store, &event(EventKind::CommentEdited, Some("r1"))))
        .collect();
    let page = store
        .read(|conn| events_since(conn, seqs[1], &EventFilter::default(), 2))
        .unwrap();
    assert_eq!(
        page.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![seqs[2], seqs[3]]
    );
    let empty = store
        .read(|conn| events_since(conn, seqs[4], &EventFilter::default(), 100))
        .unwrap();
    assert!(empty.is_empty());
}

#[test]
fn unknown_kinds_are_skipped() {
    let _sb = Sandbox::isolate();
    let (paths, store) = open_store();
    let known = append(&store, &event(EventKind::ReviewCreated, Some("r1")));
    // A kind from a newer build (or a test) must not break older readers.
    store
        .write(|tx| {
            tx.execute(
                "INSERT INTO events (at, kind, actor_kind) VALUES (1, 'future.kind', 'system')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let later = append(&store, &event(EventKind::ReviewArchived, Some("r1")));

    let events = all_events(&store, &EventFilter::default());
    assert_eq!(
        events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![known, later]
    );

    let mut feed = EventFeed::open(&paths, 0, EventFilter::default()).unwrap();
    let got = feed.poll().unwrap();
    assert_eq!(
        got.iter().map(|e| e.seq).collect::<Vec<_>>(),
        [known, later]
    );
    assert_eq!(feed.cursor(), later);
}

// ---------------------------------------------------------------------------
// Filters

#[test]
fn agent_visible_filter_excludes_viewed_and_draft() {
    let _sb = Sandbox::isolate();
    let (paths, store) = open_store();
    for kind in EventKind::ALL {
        append(&store, &event(kind, Some("r1")));
    }
    let filter = EventFilter {
        agent_visible_only: true,
        ..EventFilter::default()
    };

    let visible = all_events(&store, &filter);
    let expected: Vec<EventKind> = EventKind::ALL
        .into_iter()
        .filter(|k| !matches!(k, EventKind::ViewedChanged | EventKind::DraftChanged))
        .collect();
    assert_eq!(kinds(&visible), expected);

    let mut feed = EventFeed::open(&paths, 0, filter).unwrap();
    assert_eq!(kinds(&feed.poll().unwrap()), expected);

    // Without the filter everything is returned.
    assert_eq!(
        kinds(&all_events(&store, &EventFilter::default())),
        EventKind::ALL.to_vec()
    );
}

#[test]
fn feed_filter_by_review_ids() {
    let _sb = Sandbox::isolate();
    let (paths, store) = open_store();
    let r1a = append(&store, &event(EventKind::ReviewCreated, Some("r1")));
    append(&store, &event(EventKind::ReviewCreated, Some("r2")));
    append(&store, &event(EventKind::DraftChanged, None));
    let r1b = append(&store, &event(EventKind::ThreadCreated, Some("r1")));
    let r3 = append(&store, &event(EventKind::ReviewSubmitted, Some("r3")));

    let filter = EventFilter {
        review_ids: Some(vec!["r1".into(), "r3".into()]),
        ..EventFilter::default()
    };
    let got = all_events(&store, &filter);
    assert_eq!(
        got.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![r1a, r1b, r3]
    );

    let mut feed = EventFeed::open(
        &paths,
        0,
        EventFilter {
            review_ids: Some(vec!["r1".into()]),
            ..EventFilter::default()
        },
    )
    .unwrap();
    assert_eq!(
        feed.poll()
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        vec![r1a, r1b]
    );
    // Events for other reviews still advance the cursor past them.
    append(&store, &event(EventKind::ReviewArchived, Some("r2")));
    assert!(feed.poll().unwrap().is_empty());
    let r1c = append(&store, &event(EventKind::ReviewArchived, Some("r1")));
    assert_eq!(
        feed.poll()
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        vec![r1c]
    );

    // An empty id list matches nothing.
    let none = EventFilter {
        review_ids: Some(vec![]),
        ..EventFilter::default()
    };
    assert!(all_events(&store, &none).is_empty());
}

#[test]
fn feed_filter_by_kinds() {
    let _sb = Sandbox::isolate();
    let (paths, store) = open_store();
    append(&store, &event(EventKind::ReviewCreated, Some("r1")));
    let submitted = append(&store, &event(EventKind::ReviewSubmitted, Some("r1")));
    let archived = append(&store, &event(EventKind::ReviewArchived, Some("r2")));
    append(&store, &event(EventKind::CommentCreated, Some("r1")));

    let filter = EventFilter {
        kinds: Some(vec![EventKind::ReviewSubmitted, EventKind::ReviewArchived]),
        ..EventFilter::default()
    };
    assert_eq!(
        all_events(&store, &filter)
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        vec![submitted, archived]
    );
    let mut feed = EventFeed::open(&paths, 0, filter).unwrap();
    assert_eq!(
        feed.poll()
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        vec![submitted, archived]
    );
}

// ---------------------------------------------------------------------------
// Deletion

#[test]
fn events_survive_row_deletion() {
    let _sb = Sandbox::isolate();
    let (_paths, store) = open_store();
    let seq = store
        .write(|tx| {
            tx.execute(
                "INSERT INTO repos (id, common_dir, display_name, object_format, created_at, last_opened_at) \
                 VALUES (1, '/tmp/repo/.git', 'repo', 'sha1', 1, 1)",
                [],
            )?;
            tx.execute(
                "INSERT INTO reviews (id, repo_id, key, kind, created_at, updated_at) \
                 VALUES ('review-1', 1, 'commit:abc', 'commit', 1, 1)",
                [],
            )?;
            append_event(tx, &event(EventKind::ReviewCreated, Some("review-1")))
        })
        .unwrap();

    store
        .write(|tx| {
            tx.execute("DELETE FROM reviews WHERE id = 'review-1'", [])?;
            tx.execute("DELETE FROM repos WHERE id = 1", [])?;
            Ok(())
        })
        .unwrap();

    let reviews: i64 = store
        .read(|conn| Ok(conn.query_row("SELECT count(*) FROM reviews", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(reviews, 0);
    let events = all_events(&store, &EventFilter::default());
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, seq);
    assert_eq!(events[0].review_id.as_deref(), Some("review-1"));
}

// ---------------------------------------------------------------------------
// The feed

#[test]
fn feed_open_on_missing_db_creates_store() {
    let _sb = Sandbox::isolate();
    let paths = DataPaths::resolve().unwrap();
    assert!(!paths.db.exists());
    let mut feed = EventFeed::open(&paths, 0, EventFilter::default()).unwrap();
    assert!(paths.db.exists());
    assert_eq!(feed.latest_seq().unwrap(), 0);
    assert!(feed.poll().unwrap().is_empty());
    assert_eq!(feed.cursor(), 0);
}

#[test]
fn feed_is_read_only() {
    let _sb = Sandbox::isolate();
    let (paths, _store) = open_store();
    let feed = EventFeed::open(&paths, 0, EventFilter::default()).unwrap();
    let err = feed
        .connection()
        .execute(
            "INSERT INTO events (at, kind, actor_kind) VALUES (1, 'review.created', 'system')",
            [],
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("readonly") || err.to_string().contains("read-only"),
        "{err}"
    );
}

#[test]
fn feed_poll_is_noop_when_data_version_unchanged() {
    let _sb = Sandbox::isolate();
    let (paths, store) = open_store();
    let seq = append(&store, &event(EventKind::ReviewCreated, Some("r1")));

    let mut feed = EventFeed::open(&paths, 0, EventFilter::default()).unwrap();
    let first = feed.poll().unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].seq, seq);
    let scans = feed.scans();
    assert!(scans >= 1);

    // Nothing committed since: no query against `events` at all.
    let n = 2_000;
    let started = Instant::now();
    for _ in 0..n {
        assert!(feed.poll().unwrap().is_empty());
    }
    let per_poll = started.elapsed() / n;
    assert_eq!(
        feed.scans(),
        scans,
        "an unchanged data_version must not scan"
    );
    // Generous bound (debug build, loaded CI); the release cost is about 1 µs.
    assert!(
        per_poll < Duration::from_micros(200),
        "{per_poll:?} per poll"
    );

    // A commit elsewhere bumps data_version and triggers exactly one scan.
    let next = append(&store, &event(EventKind::ReviewArchived, Some("r1")));
    let got = feed.poll().unwrap();
    assert_eq!(got.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![next]);
    assert_eq!(feed.scans(), scans + 1);
    assert!(feed.poll().unwrap().is_empty());
    assert_eq!(feed.scans(), scans + 1);
}

#[test]
fn feed_sees_commit_from_other_connection() {
    let _sb = Sandbox::isolate();
    let (paths, store) = open_store();
    let mut feed = EventFeed::open(&paths, 0, EventFilter::default()).unwrap();
    assert!(feed.poll().unwrap().is_empty());

    // A rolled-back write is never observed.
    let err = store.write(|tx| {
        append_event(tx, &event(EventKind::ReviewCreated, Some("rolled-back")))?;
        Err::<(), _>(polygloss_core::store::StoreError::Integrity("abort".into()))
    });
    assert!(err.is_err());
    assert!(feed.poll().unwrap().is_empty());

    // A second, independent connection to the same file.
    let other = Store::open(&paths).unwrap();
    let seq = append(&other, &event(EventKind::ThreadResolved, Some("r1")));
    let got = feed.poll().unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].seq, seq);
    assert_eq!(got[0].kind, EventKind::ThreadResolved);
    assert_eq!(feed.cursor(), seq);
    assert_eq!(feed.latest_seq().unwrap(), seq);
}

#[test]
fn feed_starts_after_seq_and_pages_large_backlog() {
    let _sb = Sandbox::isolate();
    let (paths, store) = open_store();
    let seqs: Vec<i64> = store
        .write(|tx| {
            (0..2_500)
                .map(|i| {
                    let kind = if i % 2 == 0 {
                        EventKind::CommentCreated
                    } else {
                        EventKind::ViewedChanged
                    };
                    append_event(tx, &event(kind, Some("r1")))
                })
                .collect()
        })
        .unwrap();

    let after = seqs[99];
    let mut feed = EventFeed::open(&paths, after, EventFilter::default()).unwrap();
    let got = feed.poll().unwrap();
    assert_eq!(
        got.iter().map(|e| e.seq).collect::<Vec<_>>(),
        seqs[100..].to_vec()
    );
    assert_eq!(feed.cursor(), *seqs.last().unwrap());

    let mut visible = EventFeed::open(
        &paths,
        0,
        EventFilter {
            agent_visible_only: true,
            ..EventFilter::default()
        },
    )
    .unwrap();
    let got = visible.poll().unwrap();
    assert_eq!(got.len(), 1_250);
    assert!(got.iter().all(|e| e.kind == EventKind::CommentCreated));
    assert_eq!(visible.cursor(), *seqs.last().unwrap());
}

#[test]
fn feed_seek_to_latest_skips_the_backlog() {
    let _sb = Sandbox::isolate();
    let (paths, store) = open_store();
    append(&store, &event(EventKind::ReviewCreated, Some("r1")));
    let last = append(&store, &event(EventKind::CommentCreated, Some("r1")));
    let mut feed = EventFeed::open(&paths, 0, EventFilter::default()).unwrap();
    assert_eq!(feed.seek_to_latest().unwrap(), last);
    assert_eq!(feed.cursor(), last);
    // The backlog is skipped; later events still arrive.
    assert!(feed.poll().unwrap().is_empty());
    let next = append(&store, &event(EventKind::ThreadResolved, Some("r1")));
    assert_eq!(
        feed.poll()
            .unwrap()
            .iter()
            .map(|e| e.seq)
            .collect::<Vec<_>>(),
        [next]
    );
    // Never moves the cursor back.
    let mut ahead = EventFeed::open(&paths, next + 10, EventFilter::default()).unwrap();
    assert_eq!(ahead.seek_to_latest().unwrap(), next + 10);
}

// ---------------------------------------------------------------------------
// Multi-process

const CHILD_ENV: &str = "POLYGLOSS_EVENTS_CHILD";

/// Writer child for `feed_sees_commit_from_other_process`; a no-op in a normal run.
#[test]
fn child_process_entry() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let paths = DataPaths::resolve().unwrap();
    let store = Store::open(&paths).unwrap();
    let seq = store
        .write(|tx| {
            append_event(
                tx,
                &NewEvent {
                    kind: EventKind::ReviewSubmitted,
                    review_id: Some("from-child".into()),
                    diff_id: None,
                    thread_id: None,
                    comment_id: None,
                    actor: Actor {
                        kind: ActorKind::Human,
                        name: None,
                        session_id: None,
                    },
                    payload: json!({ "pid": std::process::id() }),
                },
            )
        })
        .unwrap();
    println!("CHILD-SEQ {seq}");
}

#[test]
fn feed_sees_commit_from_other_process() {
    let sb = Sandbox::isolate();
    let (paths, _store) = open_store();
    let mut feed = EventFeed::open(&paths, 0, EventFilter::default()).unwrap();
    assert!(feed.poll().unwrap().is_empty());

    let out = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "child_process_entry", "--nocapture"])
        .env(CHILD_ENV, "1")
        .env("POLYGLOSS_DATA_DIR", &paths.data_dir)
        .env("HOME", sb.home())
        .env("XDG_CONFIG_HOME", sb.config_dir())
        .env("XDG_CACHE_HOME", sb.cache_dir())
        .env("GIT_CONFIG_GLOBAL", sb.git_config_global())
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "child failed: {}\n{stdout}\n{}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    let child_seq: i64 = stdout
        .lines()
        .find_map(|l| l.strip_prefix("CHILD-SEQ "))
        .unwrap_or_else(|| panic!("no CHILD-SEQ line in:\n{stdout}"))
        .trim()
        .parse()
        .unwrap();

    let got = poll_until_some(&mut feed, Duration::from_secs(5));
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!(got[0].seq, child_seq);
    assert_eq!(got[0].review_id.as_deref(), Some("from-child"));
    assert_eq!(got[0].kind, EventKind::ReviewSubmitted);
    assert_ne!(
        got[0].payload["pid"].as_u64(),
        Some(u64::from(std::process::id()))
    );
}
