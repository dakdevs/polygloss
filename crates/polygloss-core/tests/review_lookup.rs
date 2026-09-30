//! Id lookups the agent surface needs without a repo on disk (T4.5): a diff id
//! prefix resolved against the store only, and the latest event seq; plus
//! the submission and archived lookups of `wait_for_review` (T4.7).
//!
//! Every test runs under `Sandbox::isolate()`; only nextest is supported.

use polygloss_core::ObjectFormat;
use polygloss_core::git::{Since, Source};
use polygloss_core::review::{Core, CoreError, OpenRequest, PinnedBy, Verdict};
use polygloss_core::store::events::{Actor, latest_seq};
use polygloss_core::testing::{FixtureRepo, Sandbox};

fn open_live(core: &Core, repo: &FixtureRepo) -> polygloss_core::DiffId {
    core.open(&OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Live { since: Since::Head },
        label: None,
        pin: Some(PinnedBy::Manual),
        actor: Actor::human(),
    })
    .unwrap()
    .diff_id
}

#[test]
fn resolve_diff_prefix_finds_the_stored_diff_without_its_repo() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"a\n");
    repo.commit("c1");
    repo.write("a.txt", b"b\n");
    let diff = open_live(&core, &repo);
    let upper = diff.as_str()[..10].to_ascii_uppercase();
    assert_eq!(core.resolve_diff_prefix(&upper).unwrap(), diff);
    assert_eq!(core.resolve_diff_prefix(diff.as_str()).unwrap(), diff);
    // The repo is gone: the store still knows the id.
    std::fs::remove_dir_all(repo.path()).unwrap();
    assert_eq!(core.resolve_diff_prefix(&diff.as_str()[..8]).unwrap(), diff);

    for bad in ["abc", "zzzzzzzzzz", "0000000000"] {
        let err = core.resolve_diff_prefix(bad).unwrap_err();
        assert_eq!(err.code(), "not_found", "{bad}: {err}");
    }
}

#[test]
fn resolve_diff_prefix_reports_ambiguous_prefixes() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    let ids = ["aaaaaaaa11", "aaaaaaaa22"].map(|p| format!("{p}{}", "0".repeat(54)));
    core.store
        .write(|tx| {
            for id in &ids {
                tx.execute(
                    "INSERT INTO diffs (id, object_format, base_tree, head_tree, created_at) \
                     VALUES (?1, 'sha1', ?2, ?2, 1)",
                    [id.as_str(), "4b825dc642cb6eb9a060e54bf8d69288fbee4904"],
                )?;
            }
            Ok(())
        })
        .unwrap();
    match core.resolve_diff_prefix("aaaaaaaa") {
        Err(CoreError::Ambiguous { matches, .. }) => assert_eq!(matches, ids),
        other => panic!("{other:?}"),
    }
}

#[test]
fn latest_seq_is_zero_then_the_newest_event() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    assert_eq!(core.store.read(latest_seq).unwrap(), 0);
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"a\n");
    repo.commit("c1");
    repo.write("a.txt", b"b\n");
    open_live(&core, &repo);
    let n: i64 = core
        .store
        .read(|c| Ok(c.query_row("SELECT MAX(seq) FROM events", [], |r| r.get(0))?))
        .unwrap();
    assert!(n > 0);
    assert_eq!(core.store.read(latest_seq).unwrap(), n);
}

fn live_review(core: &Core) -> (FixtureRepo, String) {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"a\n");
    repo.commit("c1");
    repo.write("a.txt", b"b\n");
    let opened = core
        .open(&OpenRequest {
            worktree: repo.path().to_path_buf(),
            source: Source::Live { since: Since::Head },
            label: None,
            pin: Some(PinnedBy::Manual),
            actor: Actor::human(),
        })
        .unwrap();
    (repo, opened.review_id)
}

#[test]
fn submission_lookup_returns_the_row_its_iteration_and_event_seq() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    let (_repo, review) = live_review(&core);
    assert_eq!(core.submission("no-such-submission").unwrap(), None);
    let first = core
        .submit_review(&review, Verdict::Comment, "first", None)
        .unwrap();
    let second = core
        .submit_review(&review, Verdict::RequestChanges, "Rename it.", None)
        .unwrap();
    let got = core.submission(&second.id).unwrap().unwrap();
    assert_eq!(got, second);
    assert_eq!(got.iteration.seq, 1);
    assert_eq!(got.summary_md, "Rename it.");
    assert!(got.seq > first.seq);
    assert_eq!(core.submission(&first.id).unwrap().unwrap(), first);
}

#[test]
fn review_archived_follows_archive_and_reopen() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    let (repo, review) = live_review(&core);
    assert!(!core.review_archived(&review).unwrap());
    core.archive_review(&review, &Actor::human()).unwrap();
    assert!(core.review_archived(&review).unwrap());
    // Reopening un-archives it.
    open_live(&core, &repo);
    assert!(!core.review_archived(&review).unwrap());
    let err = core.review_archived("no-such-review").unwrap_err();
    assert_eq!(err.code(), "not_found");
}
