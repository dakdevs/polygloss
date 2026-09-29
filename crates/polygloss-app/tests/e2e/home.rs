//! Screenshots of T3.4: Home with reviews in both sections
//! (`e2e_home_populated`): a re-review request and an open agent question
//! awaiting you; approved, changes-requested and muted reviews under
//! Recent, with kind badges, statuses, viewed counts, threads, agent badges
//! and relative times; the third row is selected.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::{px, size};
use polygloss_app::home::HomeView;
use polygloss_app::tabs::TabItem;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::threads::{Author, AuthorKind, NewThread, Subject, ThreadKind};
use polygloss_core::review::{
    AssignedBy, Core, OpenRequest, OpenedDiff, PinnedBy, SessionInfo, Verdict,
};
use polygloss_core::store::events::{Actor, ActorKind};
use polygloss_core::testing::FixtureRepo;
use polygloss_diff::ObjectFormat;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{CONFIG_RS_BASE, CONFIG_RS_HEAD, Sandbox};

pub const TESTS: &[Test] = &crate::tests![e2e_home_populated];

/// The screenshot's "now": 2026-09-29 12:00 UTC.
const NOW: i64 = 1_790_683_200_000;
const MIN: i64 = 60_000;
const HOUR: i64 = 60 * MIN;
const DAY: i64 = 24 * HOUR;

/// Frames drawn at most while Home loads.
const MAX_FRAMES: usize = 20;

fn repo_with_history() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/config.rs", CONFIG_RS_BASE.as_bytes());
    repo.write("README.md", b"# app\n\nReads `app.conf`.\n");
    repo.commit("Add the config parser");
    repo.git(&["tag", "v0.1.0"]);
    repo.git(&["checkout", "-q", "-b", "sorted-config"]);
    repo.write("src/config.rs", CONFIG_RS_HEAD.as_bytes());
    repo.write("README.md", b"# app\n\nReads `app.conf`, sorted by key.\n");
    repo.commit("Keep config entries sorted by key");
    repo
}

fn req(repo: &Path, source: Source, label: Option<&str>, pin: Option<PinnedBy>) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source,
        label: label.map(str::to_owned),
        pin,
        actor: Actor::human(),
    }
}

fn compare(mode: CompareMode) -> Source {
    Source::Compare {
        base: "refs/tags/v0.1.0".into(),
        head: "refs/heads/sorted-config".into(),
        mode,
    }
}

fn agent() -> Actor {
    Actor {
        kind: ActorKind::Agent,
        name: Some("claude-code".into()),
        session_id: None,
    }
}

fn agent_thread(core: &Core, opened: &OpenedDiff, kind: ThreadKind, body: &str) -> String {
    let blobs = BlobReader::open(&opened.repo).expect("blob reader");
    core.create_thread(
        &NewThread {
            review_id: opened.review_id.clone(),
            diff_id: opened.diff_id.clone(),
            subject: Subject::Review,
            kind,
            body_md: body.into(),
            author: Author {
                kind: AuthorKind::Agent,
                name: "claude-code".into(),
                session_id: None,
            },
        },
        &blobs,
    )
    .expect("create a thread")
}

fn sql(core: &Core, statement: String) {
    core.store
        .write(|tx| {
            tx.execute(&statement, ())?;
            Ok(())
        })
        .expect("update the store");
}

/// Five reviews over two repos, as a week of work leaves them.
fn populate(core: &Core, app: &FixtureRepo, kit: &FixtureRepo) {
    let session = core
        .upsert_session(&SessionInfo {
            id: "session-one".into(),
            client_name: "claude-code".into(),
            client_version: None,
            owner_pid: None,
            cwd: Some(app.path().to_path_buf()),
        })
        .unwrap();

    // Awaiting you: the agent asks for a re-review of a labeled compare.
    let pr = core
        .open(&req(
            app.path(),
            compare(CompareMode::ThreeDot),
            Some("Sort config entries by key"),
            None,
        ))
        .unwrap();
    core.set_viewed(Some(&pr.review_id), &pr.files[0], true)
        .unwrap();
    core.submit_review(&pr.review_id, Verdict::RequestChanges, "Please sort.", None)
        .unwrap();
    core.assign_review(&pr.review_id, &session, AssignedBy::OpenDiff)
        .unwrap();
    core.request_rereview(&pr.review_id, "Sorted, and added tests.", &agent(), None)
        .unwrap();

    // Awaiting you: a live review with an open agent question.
    app.git(&["checkout", "-q", "-b", "feature/greeting"]);
    app.write(
        "src/greet.rs",
        b"pub fn greet(name: &str) -> String {\n    format!(\"Hello, {name}!\")\n}\n",
    );
    let live = core
        .open(&req(
            app.path(),
            Source::Live {
                since: Since::MergeBase,
            },
            None,
            Some(PinnedBy::Agent),
        ))
        .unwrap();
    agent_thread(
        core,
        &live,
        ThreadKind::Question,
        "Should `greet` trim the name?",
    );
    core.assign_review(&live.review_id, &session, AssignedBy::OpenDiff)
        .unwrap();

    // Recent: an approved commit, every file viewed.
    let head = core
        .open(&req(
            app.path(),
            Source::Commit {
                rev: "refs/heads/sorted-config".into(),
            },
            None,
            None,
        ))
        .unwrap();
    for f in head.files.iter() {
        core.set_viewed(Some(&head.review_id), f, true).unwrap();
    }
    core.submit_review(&head.review_id, Verdict::Approve, "LGTM", None)
        .unwrap();

    // Recent: changes requested, two agent comments open.
    let base = core
        .open(&req(
            kit.path(),
            Source::Commit {
                rev: "refs/tags/v0.1.0".into(),
            },
            None,
            None,
        ))
        .unwrap();
    agent_thread(
        core,
        &base,
        ThreadKind::Comment,
        "`parse` ignores `#` comments.",
    );
    agent_thread(
        core,
        &base,
        ThreadKind::Note,
        "Kept the old behavior on purpose.",
    );
    core.submit_review(&base.review_id, Verdict::RequestChanges, "", None)
        .unwrap();

    // Recent: a muted direct compare that only has comments.
    let direct = core
        .open(&req(kit.path(), compare(CompareMode::Direct), None, None))
        .unwrap();
    core.submit_review(&direct.review_id, Verdict::Comment, "A few notes.", None)
        .unwrap();
    core.set_muted(&direct.review_id, true).unwrap();

    sql(
        core,
        format!(
            "UPDATE repos SET display_name = 'gpui-kit' WHERE common_dir = '{}'",
            kit.path().join(".git").display()
        ),
    );
    sql(
        core,
        format!(
            "UPDATE repos SET display_name = 'polygloss' WHERE common_dir = '{}'",
            app.path().join(".git").display()
        ),
    );
    for (id, ago) in [
        (&pr.review_id, 12 * MIN),
        (&live.review_id, 2 * HOUR),
        (&head.review_id, 5 * HOUR),
        (&base.review_id, DAY + 3 * HOUR),
        (&direct.review_id, 4 * DAY),
    ] {
        sql(
            core,
            format!(
                "UPDATE reviews SET updated_at = {} WHERE id = '{id}'",
                NOW - ago
            ),
        );
    }
}

fn e2e_home_populated() {
    let _sb = Sandbox::isolate();
    let app = repo_with_history();
    let kit = repo_with_history();
    let core = Core::open_default().expect("open the sandbox store");
    populate(&core, &app, &kit);
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, main) = cx.update(|cx| {
        startup::init(core, cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    let home: gpui_kit::Entity<HomeView> = cx.update(|cx| match main.read(cx).tabs().get(0) {
        Some(TabItem::Home(home)) => home.clone(),
        _ => panic!("Home is the first tab"),
    });
    cx.update(|cx| home.update(cx, |h, cx| h.set_clock(|| NOW, cx)));
    let mut loaded = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        if cx.update(|cx| home.read(cx).is_loaded()) {
            loaded = true;
            break;
        }
    }
    assert!(loaded, "Home never loaded");
    screenshot::draw(&mut cx, handle);
    let (awaiting, recent) = cx.update(|cx| {
        let h = home.read(cx);
        (h.awaiting_you().len(), h.recent().len())
    });
    assert_eq!((awaiting, recent), (2, 3));
    // The keyboard selection on the third row (`j` three times).
    cx.update(|cx| {
        home.update(cx, |h, cx| {
            let id = h.rows()[2].review_id().to_owned();
            h.select(Some(&id), cx)
        })
    });
    screenshot::draw(&mut cx, handle);
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}
