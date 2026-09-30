//! `polygloss://` URLs (T4.2, design §13.5): parsing, formatting, percent
//! encoding of paths, and resolving a URL to the review tab it opens.
//!
//! The resolve tests run under `Sandbox::isolate()` (temp `HOME`, data dir and
//! git config). Only nextest (one process per test) is supported.

use polygloss_core::git::{CompareMode, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, CoreError, NewThread, OpenRequest, Subject, ThreadKind,
};
use polygloss_core::store::events::Actor;
use polygloss_core::testing::{FixtureRepo, Sandbox};
use polygloss_core::urls::{
    PolyglossUrl, UrlDiff, UrlError, UrlFocus, UrlTarget, format_url, parse_url,
};
use polygloss_core::{DiffId, ObjectFormat};
use polygloss_diff::Side;

const DIFF: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
const REVIEW: &str = "01926a5e-7b1c-7cde-9f00-0123456789ab";
const THREAD: &str = "01926a5e-7b1c-7cde-9f00-ba9876543210";

fn diff(path: Option<&str>, side: Option<Side>, line: Option<u32>) -> PolyglossUrl {
    PolyglossUrl::Diff {
        diff_id: DIFF.into(),
        path: path.map(str::to_owned),
        side,
        line,
    }
}

#[test]
fn url_roundtrip_all_forms() {
    let cases = [
        (diff(None, None, None), format!("polygloss://diff/{DIFF}")),
        (
            diff(Some("src/main.rs"), None, None),
            format!("polygloss://diff/{DIFF}?path=src/main.rs"),
        ),
        (
            diff(Some("src/main.rs"), Some(Side::New), Some(42)),
            format!("polygloss://diff/{DIFF}?path=src/main.rs&side=new&line=42"),
        ),
        (
            diff(Some("a.txt"), Some(Side::Old), Some(1)),
            format!("polygloss://diff/{DIFF}?path=a.txt&side=old&line=1"),
        ),
        (
            diff(Some("a.txt"), None, Some(7)),
            format!("polygloss://diff/{DIFF}?path=a.txt&line=7"),
        ),
        (
            PolyglossUrl::Review(REVIEW.into()),
            format!("polygloss://review/{REVIEW}"),
        ),
        (
            PolyglossUrl::Thread(THREAD.into()),
            format!("polygloss://thread/{THREAD}"),
        ),
    ];
    for (url, text) in cases {
        assert_eq!(format_url(&url), text, "format {url:?}");
        assert_eq!(parse_url(&text).unwrap(), url, "parse {text}");
    }

    // `side` and `line` need a path: without one they are dropped, so every
    // formatted URL parses (as the diff alone).
    for (side, line) in [
        (Some(Side::Old), None),
        (None, Some(3)),
        (Some(Side::New), Some(3)),
    ] {
        let text = format_url(&diff(None, side, line));
        assert_eq!(text, format!("polygloss://diff/{DIFF}"));
        assert_eq!(parse_url(&text).unwrap(), diff(None, None, None));
    }
}

#[test]
fn url_parse_is_lenient_where_links_vary() {
    // Scheme and host are case-insensitive, ids are normalized to lowercase, a
    // trailing slash and a fragment are ignored, parameters may come in any order.
    let upper = format!(
        "POLYGLOSS://Diff/{}/?line=3&side=old&path=x.rs#frag",
        DIFF.to_uppercase()
    );
    assert_eq!(
        parse_url(&upper).unwrap(),
        diff(Some("x.rs"), Some(Side::Old), Some(3))
    );
    assert_eq!(
        parse_url(&format!("polygloss://review/{}", REVIEW.to_uppercase())).unwrap(),
        PolyglossUrl::Review(REVIEW.into())
    );
    // The MCP resource form (§15.3) works as a deep link to the review.
    assert_eq!(
        parse_url(&format!("polygloss://review/{REVIEW}/threads")).unwrap(),
        PolyglossUrl::Review(REVIEW.into())
    );
    // Unknown parameters are ignored (newer links, older app).
    assert_eq!(
        parse_url(&format!("polygloss://diff/{DIFF}?utm=x")).unwrap(),
        diff(None, None, None)
    );
}

#[test]
fn url_rejects_unknown_host_and_bad_ids() {
    let unknown = parse_url(&format!("polygloss://commit/{DIFF}")).unwrap_err();
    assert!(matches!(unknown, UrlError::UnknownTarget(ref h) if h == "commit"));

    for bad in [
        "https://example.com/diff/x".to_owned(),
        "polygloss:diff/abc".to_owned(),
        "diff/abc".to_owned(),
        String::new(),
    ] {
        assert!(
            matches!(parse_url(&bad), Err(UrlError::NotPolyglossUrl(_))),
            "{bad:?}"
        );
    }

    for bad in [
        "polygloss://diff/".to_owned(),
        "polygloss://diff/abc".to_owned(),
        format!("polygloss://diff/{}", &DIFF[..63]),
        format!("polygloss://diff/{DIFF}0"),
        format!("polygloss://diff/{}g", &DIFF[..63]),
        "polygloss://review/not-a-uuid".to_owned(),
        "polygloss://review/".to_owned(),
        "polygloss://thread/1234".to_owned(),
        format!("polygloss://thread/{THREAD}/extra"),
        format!("polygloss://review/{REVIEW}/other"),
    ] {
        assert!(
            matches!(parse_url(&bad), Err(UrlError::BadId { .. })),
            "{bad:?}: {:?}",
            parse_url(&bad)
        );
    }

    for bad in [
        format!("polygloss://diff/{DIFF}?path=a&side=left&line=1"),
        format!("polygloss://diff/{DIFF}?path=a&line=0"),
        format!("polygloss://diff/{DIFF}?path=a&line=-1"),
        format!("polygloss://diff/{DIFF}?path=a&line=x"),
        format!("polygloss://diff/{DIFF}?path=a&line=99999999999"),
        format!("polygloss://diff/{DIFF}?path="),
        format!("polygloss://diff/{DIFF}?path=a&path=b"),
        // A line needs a path; so does a side.
        format!("polygloss://diff/{DIFF}?line=3"),
        format!("polygloss://diff/{DIFF}?side=new"),
        // Broken escapes and non-UTF-8 bytes.
        format!("polygloss://diff/{DIFF}?path=a%2"),
        format!("polygloss://diff/{DIFF}?path=a%zz"),
        format!("polygloss://diff/{DIFF}?path=a%+1"),
        format!("polygloss://diff/{DIFF}?path=%ff%fe"),
        // Review and thread links take no focus.
        format!("polygloss://review/{REVIEW}?path=a"),
    ] {
        assert!(
            matches!(parse_url(&bad), Err(UrlError::BadParam { .. })),
            "{bad:?}: {:?}",
            parse_url(&bad)
        );
    }
}

#[test]
fn url_percent_encodes_paths() {
    let hostile = "dir with space/naïve \"q\" #1 ?&=+%.rs";
    let url = diff(Some(hostile), Some(Side::New), Some(2));
    let text = format_url(&url);
    assert_eq!(
        text,
        format!(
            "polygloss://diff/{DIFF}?path=dir%20with%20space/na%C3%AFve%20%22q%22%20%231%20%3F%26%3D%2B%25.rs&side=new&line=2"
        )
    );
    assert!(text.is_ascii());
    assert_eq!(parse_url(&text).unwrap(), url);

    // Control characters (tabs, newlines) and escapes that decode to `/` roundtrip.
    let odd = "a\tb\nc/%2F";
    let url = diff(Some(odd), None, None);
    let text = format_url(&url);
    assert!(!text.contains(['\t', '\n']), "{text:?}");
    assert_eq!(parse_url(&text).unwrap(), url);

    // `+` is a literal plus (never a form-encoded space); lowercase hex decodes.
    assert_eq!(
        parse_url(&format!("polygloss://diff/{DIFF}?path=a+b%c3%a9")).unwrap(),
        diff(Some("a+bé"), None, None)
    );
}

// --- Resolving a URL to the tab it opens ---------------------------------------

fn req(repo: &FixtureRepo, base: &str, head: &str) -> OpenRequest {
    OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: base.into(),
            head: head.into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

/// `main` with `a.txt`; `feature` changes its line 2.
fn repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\ntwo\nthree\n");
    repo.commit("c1");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("a.txt", b"one\nTWO\nthree\n");
    repo.commit("f1");
    repo.checkout("main");
    repo
}

#[test]
fn url_resolves_review_diff_and_thread_targets() {
    let _sb = Sandbox::isolate();
    let repo = repo();
    let core = Core::open_default().unwrap();
    let opened = core.open(&req(&repo, "main", "feature")).unwrap();
    let review_id = opened.review_id.clone();
    let diff_id = opened.diff_id.clone();

    let review = core
        .resolve_url(&PolyglossUrl::Review(review_id.clone()))
        .unwrap();
    assert_eq!(
        review,
        UrlTarget {
            review_id: review_id.clone(),
            diff: None,
            focus: None,
        }
    );

    let plain = core
        .resolve_url(&PolyglossUrl::Diff {
            diff_id: diff_id.to_string(),
            path: None,
            side: None,
            line: None,
        })
        .unwrap();
    assert_eq!(plain.review_id, review_id);
    assert_eq!(
        plain.diff,
        Some(UrlDiff {
            diff_id: diff_id.clone(),
            seq: 1,
        })
    );
    assert_eq!(plain.focus, None);

    // A line defaults to the new side; a path alone focuses the file.
    let line = core
        .resolve_url(&PolyglossUrl::Diff {
            diff_id: diff_id.to_string(),
            path: Some("a.txt".into()),
            side: None,
            line: Some(2),
        })
        .unwrap();
    assert_eq!(
        line.focus,
        Some(UrlFocus::Line {
            path: "a.txt".into(),
            side: Side::New,
            line: 2,
        })
    );
    let file = core
        .resolve_url(&PolyglossUrl::Diff {
            diff_id: diff_id.to_string(),
            path: Some("a.txt".into()),
            side: None,
            line: None,
        })
        .unwrap();
    assert_eq!(
        file.focus,
        Some(UrlFocus::File {
            path: "a.txt".into()
        })
    );

    let blobs = BlobReader::open(&opened.repo).unwrap();
    let thread_id = core
        .create_thread(
            &NewThread {
                review_id: review_id.clone(),
                diff_id: diff_id.clone(),
                subject: Subject::Line {
                    path: "a.txt".into(),
                    side: Side::New,
                    start_line: 2,
                    line: 2,
                },
                kind: ThreadKind::Comment,
                body_md: "Why uppercase?".into(),
                author: Author {
                    kind: AuthorKind::Human,
                    name: "you".into(),
                    session_id: None,
                },
            },
            &blobs,
        )
        .unwrap();
    let thread = core
        .resolve_url(&PolyglossUrl::Thread(thread_id.clone()))
        .unwrap();
    assert_eq!(
        thread,
        UrlTarget {
            review_id,
            diff: None,
            focus: Some(UrlFocus::Thread(thread_id)),
        }
    );
}

#[test]
fn url_diff_opens_its_most_recent_review() {
    let _sb = Sandbox::isolate();
    let repo = repo();
    let core = Core::open_default().unwrap();
    // Two reviews show the same trees (a branch compare and the same compare by
    // commit id): the one touched last wins.
    let first = core.open(&req(&repo, "main", "feature")).unwrap();
    // `updated_at` is in milliseconds.
    let tick = || std::thread::sleep(std::time::Duration::from_millis(5));
    tick();
    let main = repo.oid("main").to_string();
    let feature = repo.oid("feature").to_string();
    let second = core.open(&req(&repo, &main, &feature)).unwrap();
    assert_eq!(first.diff_id, second.diff_id);
    assert_ne!(first.review_id, second.review_id);
    let url = PolyglossUrl::Diff {
        diff_id: first.diff_id.to_string(),
        path: None,
        side: None,
        line: None,
    };
    assert_eq!(core.resolve_url(&url).unwrap().review_id, second.review_id);

    // Opening the first one again makes it the most recent.
    tick();
    core.open(&req(&repo, "main", "feature")).unwrap();
    assert_eq!(core.resolve_url(&url).unwrap().review_id, first.review_id);
}

#[test]
fn url_diff_names_the_iteration_showing_it() {
    let _sb = Sandbox::isolate();
    let repo = repo();
    let core = Core::open_default().unwrap();
    let first = core.open(&req(&repo, "main", "feature")).unwrap();
    // `feature` moves on: opening again records iteration 2, another diff.
    repo.checkout("feature");
    repo.write("a.txt", b"one\nTWO\nTHREE\n");
    repo.commit("f2");
    let second = core.open(&req(&repo, "main", "feature")).unwrap();
    assert_eq!(first.review_id, second.review_id);
    assert_ne!(first.diff_id, second.diff_id);
    let target = |diff_id: &DiffId| {
        core.resolve_url(&PolyglossUrl::Diff {
            diff_id: diff_id.to_string(),
            path: None,
            side: None,
            line: None,
        })
        .unwrap()
    };
    for (diff_id, seq) in [(&first.diff_id, 1), (&second.diff_id, 2)] {
        let t = target(diff_id);
        assert_eq!(t.review_id, first.review_id);
        assert_eq!(
            t.diff,
            Some(UrlDiff {
                diff_id: diff_id.clone(),
                seq,
            })
        );
    }

    // Back to the first state: iteration 3 shows the first diff again, the
    // latest of the two iterations that show it.
    repo.write("a.txt", b"one\nTWO\nthree\n");
    repo.commit("f3");
    let third = core.open(&req(&repo, "main", "feature")).unwrap();
    assert_eq!(third.diff_id, first.diff_id);
    assert_eq!(target(&first.diff_id).diff.map(|d| d.seq), Some(3));
}

#[test]
fn url_diff_prefers_reviews_not_archived() {
    let _sb = Sandbox::isolate();
    let repo = repo();
    let core = Core::open_default().unwrap();
    let first = core.open(&req(&repo, "main", "feature")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));
    let main = repo.oid("main").to_string();
    let feature = repo.oid("feature").to_string();
    let second = core.open(&req(&repo, &main, &feature)).unwrap();
    assert_eq!(first.diff_id, second.diff_id);
    let url = PolyglossUrl::Diff {
        diff_id: first.diff_id.to_string(),
        path: None,
        side: None,
        line: None,
    };
    assert_eq!(core.resolve_url(&url).unwrap().review_id, second.review_id);

    // The most recent one is archived: the other one opens.
    core.archive_review(&second.review_id, &Actor::human())
        .unwrap();
    assert_eq!(core.resolve_url(&url).unwrap().review_id, first.review_id);

    // Only archived reviews show it: the most recent of them still opens.
    core.archive_review(&first.review_id, &Actor::human())
        .unwrap();
    assert_eq!(core.resolve_url(&url).unwrap().review_id, second.review_id);
}

#[test]
fn url_unknown_targets_are_not_found() {
    let _sb = Sandbox::isolate();
    let core = Core::open_default().unwrap();
    for url in [
        PolyglossUrl::Review(REVIEW.into()),
        PolyglossUrl::Thread(THREAD.into()),
        PolyglossUrl::Diff {
            diff_id: DiffId::parse(DIFF).unwrap().to_string(),
            path: None,
            side: None,
            line: None,
        },
    ] {
        let err = core.resolve_url(&url).unwrap_err();
        assert!(matches!(err, CoreError::NotFound { .. }), "{url:?}: {err}");
        assert_eq!(err.code(), "not_found");
    }
}
