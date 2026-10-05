//! `polygloss://` URLs in the app (T4.2, design §13.5): the system's
//! `application:openURLs:` (and `polygloss://` launch arguments) open the
//! review tab a URL names and focus its file, line or thread; a thread in
//! a closed category section opens it and its sidebar panel (T6.15), also
//! through the socket's `focus`.
//!
//! The platform callback only feeds a [`UrlInbox`]; these tests feed the
//! inbox or call [`urls::open_url`] directly.

use std::sync::Arc;

use gpui_kit::Entity;
use polygloss_app::CoreDiffProvider;
use polygloss_app::iterations::{self, Showing};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::startup::LaunchArgs;
use polygloss_app::tabs::TabItem;
use polygloss_app::threads;
use polygloss_app::urls::{self, UrlInbox};
use polygloss_core::DiffId;
use polygloss_core::git::{Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, NewThread, OpenRequest, OpenedDiff, Subject, ThreadKind,
};
use polygloss_core::store::events::Actor;
use polygloss_core::urls::{PolyglossUrl, UrlFocus, format_url};
use polygloss_diff::Side;
use polygloss_viewport::CursorPos;

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox, code_change_repo, strings};

/// Opens `url` and waits for it; the task's result.
fn open(shell: &mut Shell, url: &str) -> anyhow::Result<()> {
    let task = shell.cx.update(|_, cx| urls::open_url(url, cx));
    draw(shell.cx);
    futures::FutureExt::now_or_never(task).expect("the URL open finished")
}

fn active_review_id(shell: &mut Shell) -> Option<String> {
    let tab = shell.active_review()?;
    Some(tab.read_with(shell.cx, |t, _| t.review_id.clone()))
}

fn cursor(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<CursorPos> {
    tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor())
}

fn go_home(shell: &mut Shell) {
    shell
        .main
        .update_in(shell.cx, |m, window, cx| m.activate_tab(0, window, cx));
    assert!(matches!(shell.tab(0), TabItem::Home(_)));
}

/// An agent note on 1-based line `line` of `src/config.rs` (new side).
fn agent_note(shell: &Shell, opened: &OpenedDiff, line: u32) -> String {
    let blobs = BlobReader::open(&opened.repo).unwrap();
    shell
        .core
        .create_thread(
            &NewThread {
                review_id: opened.review_id.clone(),
                diff_id: opened.diff_id.clone(),
                subject: Subject::Line {
                    path: "src/config.rs".into(),
                    side: Side::New,
                    start_line: line,
                    line,
                },
                kind: ThreadKind::Note,
                body_md: "Parsing starts here.".into(),
                author: Author {
                    kind: AuthorKind::Agent,
                    name: "claude-code".into(),
                    session_id: None,
                },
            },
            &blobs,
        )
        .unwrap()
}

fn review_url(id: &str) -> String {
    format_url(&PolyglossUrl::Review(id.to_owned()))
}

#[gpui_kit::test]
fn url_review_focuses_its_open_tab(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let review_id = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    go_home(&mut shell);

    open(&mut shell, &review_url(&review_id)).unwrap();

    assert_eq!(shell.tabs(), (2, 1), "the open tab, not a second one");
    assert_eq!(active_review_id(&mut shell), Some(review_id));
}

#[gpui_kit::test]
fn url_diff_opens_review_and_focuses_line(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    // Opened elsewhere (the CLI or an agent): the app has no tab for it.
    let opened = shell.core.open(&compare_req(repo.path())).unwrap();
    assert_eq!(shell.tabs(), (1, 0));

    let url = format_url(&PolyglossUrl::Diff {
        diff_id: opened.diff_id.to_string(),
        path: Some("src/config.rs".into()),
        side: Some(Side::New),
        line: Some(5),
    });
    open(&mut shell, &url).unwrap();

    assert_eq!(shell.tabs(), (2, 1));
    assert_eq!(
        active_review_id(&mut shell).as_deref(),
        Some(opened.review_id.as_str())
    );
    let tab = shell.active_review().unwrap();
    let file_idx = opened
        .files
        .iter()
        .position(|f| f.display_path() == "src/config.rs")
        .unwrap() as u32;
    assert_eq!(
        cursor(&mut shell, &tab),
        Some(CursorPos {
            file_idx,
            side: Side::New,
            line: 4,
            range_start: None,
        }),
        "1-based line 5 is row 4"
    );
}

/// Moves tag `head` of [`code_change_repo`] to a new commit: `src/config.rs`
/// changes again and `src/main.rs` is gone, so opening the compare records
/// iteration 2, a diff without `src/main.rs`.
fn move_head(repo: &FixtureRepo) {
    repo.git(&["checkout", "-q", "refs/tags/head"]);
    let config = std::fs::read_to_string(repo.path().join("src/config.rs")).unwrap();
    repo.write("src/config.rs", format!("// moved on\n{config}").as_bytes());
    repo.git(&["rm", "-q", "src/main.rs"]);
    repo.commit("head 2");
    repo.git(&["tag", "-f", "head"]);
}

fn diff_url(diff_id: &DiffId, path: &str, line: u32) -> String {
    format_url(&PolyglossUrl::Diff {
        diff_id: diff_id.to_string(),
        path: Some(path.to_owned()),
        side: Some(Side::New),
        line: Some(line),
    })
}

fn shown_diff(shell: &mut Shell, tab: &Entity<ReviewTab>) -> DiffId {
    tab.read_with(shell.cx, |t, _| t.opened.diff_id.clone())
}

fn main_rs_line(shell: &mut Shell, tab: &Entity<ReviewTab>, line: u32) -> Option<CursorPos> {
    let file_idx = tab.read_with(shell.cx, |t, _| {
        t.opened
            .files
            .iter()
            .find(|f| f.display_path() == "src/main.rs")
            .map(|f| f.idx)
    })?;
    Some(CursorPos {
        file_idx,
        side: Side::New,
        line,
        range_start: None,
    })
}

#[gpui_kit::test]
fn url_diff_of_an_older_iteration_shows_that_iteration(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    // An agent links iteration 1; the head then moves on (iteration 2).
    let first = shell.core.open(&compare_req(repo.path())).unwrap();
    move_head(&repo);
    let second = shell.core.open(&compare_req(repo.path())).unwrap();
    assert_eq!(first.review_id, second.review_id);
    assert_ne!(first.diff_id, second.diff_id);
    assert!(
        !second
            .files
            .iter()
            .any(|f| f.display_path() == "src/main.rs")
    );

    // No tab yet: the review opens (its current state is iteration 2), then
    // shows iteration 1, the linked diff, focused on its line.
    open(&mut shell, &diff_url(&first.diff_id, "src/main.rs", 3)).unwrap();
    assert_eq!(shell.tabs(), (2, 1));
    let tab = shell.active_review().unwrap();
    assert_eq!(shown_diff(&mut shell, &tab), first.diff_id);
    assert_eq!(
        tab.read_with(shell.cx, |t, _| iterations::showing(t)),
        Showing::Iteration(1)
    );
    assert_eq!(
        tab.read_with(shell.cx, |t, _| iterations::current(t).diff_id.clone()),
        second.diff_id,
        "the review's current state is still iteration 2"
    );
    let expected = main_rs_line(&mut shell, &tab, 2);
    assert!(expected.is_some());
    assert_eq!(
        cursor(&mut shell, &tab),
        expected,
        "1-based line 3 is row 2"
    );
    let viewport_files = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).document().files().len()
    });
    assert_eq!(
        viewport_files,
        first.files.len(),
        "the viewport shows iteration 1"
    );

    // The open tab follows links: to iteration 2 (its current state)…
    go_home(&mut shell);
    open(&mut shell, &diff_url(&second.diff_id, "src/config.rs", 1)).unwrap();
    assert_eq!(shell.tabs(), (2, 1), "the same tab");
    assert_eq!(shown_diff(&mut shell, &tab), second.diff_id);
    assert_eq!(
        tab.read_with(shell.cx, |t, _| iterations::showing(t)),
        Showing::Current
    );
    // …and back to iteration 1, where `src/main.rs` is.
    open(&mut shell, &diff_url(&first.diff_id, "src/main.rs", 1)).unwrap();
    assert_eq!(shown_diff(&mut shell, &tab), first.diff_id);
    let expected = main_rs_line(&mut shell, &tab, 0);
    assert_eq!(cursor(&mut shell, &tab), expected);
}

#[gpui_kit::test]
fn url_diff_of_an_unpinned_live_state_focuses_its_tab(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    repo.write("src/config.rs", b"// uncommitted\n");
    let mut shell = start(cx);
    let tab = shell
        .open(OpenRequest {
            worktree: repo.path().to_path_buf(),
            source: Source::Live { since: Since::Head },
            label: None,
            pin: None,
            actor: Actor::human(),
        })
        .unwrap();
    let diff_id = shown_diff(&mut shell, &tab);
    // Not pinned: the store has no iteration showing it.
    let err = shell
        .core
        .resolve_url(&PolyglossUrl::Diff {
            diff_id: diff_id.to_string(),
            path: None,
            side: None,
            line: None,
        })
        .unwrap_err();
    assert_eq!(err.code(), "not_found");
    go_home(&mut shell);

    open(&mut shell, &diff_url(&diff_id, "src/config.rs", 1)).unwrap();

    assert_eq!(shell.tabs(), (2, 1), "the tab showing it");
    let at = cursor(&mut shell, &tab).expect("the cursor on the line");
    assert_eq!((at.side, at.line), (Side::New, 0));
}

#[gpui_kit::test]
fn url_diff_path_scrolls_to_file(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let opened = shell.core.open(&compare_req(repo.path())).unwrap();
    let last = opened.files.len() as u32 - 1;
    let path = opened.files[last as usize].display_path().to_owned();

    let url = format_url(&PolyglossUrl::Diff {
        diff_id: opened.diff_id.to_string(),
        path: Some(path),
        side: None,
        line: None,
    });
    open(&mut shell, &url).unwrap();

    let tab = shell.active_review().unwrap();
    let top = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).anchor().file_idx);
    assert_eq!(top, last);
}

#[gpui_kit::test]
fn url_thread_opens_review_on_thread(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let opened = shell.core.open(&compare_req(repo.path())).unwrap();
    let thread_id = agent_note(&shell, &opened, 9);

    open(
        &mut shell,
        &format_url(&PolyglossUrl::Thread(thread_id.clone())),
    )
    .unwrap();

    assert_eq!(
        active_review_id(&mut shell).as_deref(),
        Some(opened.review_id.as_str())
    );
    let tab = shell.active_review().unwrap();
    let at = cursor(&mut shell, &tab).expect("the cursor on the thread");
    assert_eq!((at.side, at.line), (Side::New, 8));
}

#[gpui_kit::test]
fn url_thread_focus_waits_for_threads_to_load(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let opened = shell.core.open(&compare_req(repo.path())).unwrap();
    let thread_id = agent_note(&shell, &opened, 9);

    // The focus arrives with the tab, before its threads have loaded.
    let main = shell.main.clone();
    let tab = shell.cx.update(|window, cx| {
        let provider = Arc::new(
            CoreDiffProvider::open(&opened, &polygloss_app::categories::shared(cx)).unwrap(),
        );
        let tab = main.update(cx, |m, cx| {
            m.show_review(opened.clone(), provider, window, cx)
        });
        let loaded = threads::threads(tab.read(cx))
            .map(|m| m.read(cx).is_loaded())
            .unwrap();
        assert!(!loaded, "the threads load in the background");
        urls::apply_focus(&tab, UrlFocus::Thread(thread_id.clone()), window, cx).unwrap();
        tab
    });
    assert_eq!(cursor(&mut shell, &tab), None, "nothing to focus yet");
    draw(shell.cx);

    let at = cursor(&mut shell, &tab).expect("the cursor on the thread once loaded");
    assert_eq!((at.side, at.line), (Side::New, 8));
}

#[gpui_kit::test]
fn url_reopens_the_closed_window(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let shell = start(cx);
    let opened = shell.core.open(&compare_req(repo.path())).unwrap();
    // The last window closed; the app lives on (T3.1).
    shell.cx.update(|window, _| window.remove_window());
    shell.cx.run_until_parked();
    assert!(
        cx.update(|cx| polygloss_app::window::main_window(cx))
            .is_none()
    );

    let task = cx.update(|cx| urls::open_url(&review_url(&opened.review_id), cx));
    cx.run_until_parked();
    futures::FutureExt::now_or_never(task)
        .expect("the URL open finished")
        .unwrap();

    let (_, main) = cx
        .update(|cx| polygloss_app::window::main_window(cx))
        .expect("the window is back");
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    let active = main.read_with(cx, |m, cx| {
        m.tabs()
            .get(m.tabs().active())
            .and_then(|t| t.review())
            .map(|t| t.read(cx).review_id.clone())
    });
    assert_eq!(active.as_deref(), Some(opened.review_id.as_str()));
}

#[gpui_kit::test]
fn url_errors_show_in_the_window(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let mut shell = start(cx);
    let missing = review_url("01926a5e-7b1c-7cde-9f00-0123456789ab");
    for (url, says) in [
        (
            "polygloss://nope/x".to_owned(),
            "unknown polygloss:// target",
        ),
        (missing, "not found"),
    ] {
        let err = open(&mut shell, &url).unwrap_err();
        assert!(err.to_string().contains(says), "{url}: {err}");
        let shown = shell
            .main
            .read_with(shell.cx, |m, _| m.open_errors().to_vec());
        assert!(
            shown.last().is_some_and(|m| m.contains(says)),
            "{url}: {shown:?}"
        );
    }
    assert_eq!(shell.tabs(), (1, 0));
}

#[gpui_kit::test]
fn url_inbox_routes_system_urls(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let opened = shell.core.open(&compare_req(repo.path())).unwrap();

    // URLs that arrive before the app listens wait in the inbox.
    let inbox = UrlInbox::new();
    let sender = inbox.sender();
    sender
        .unbounded_send(vec![review_url(&opened.review_id)])
        .unwrap();
    shell.cx.update(|_, cx| urls::listen(inbox, cx));
    draw(shell.cx);

    assert_eq!(shell.tabs(), (2, 1));
    assert_eq!(
        active_review_id(&mut shell).as_deref(),
        Some(opened.review_id.as_str())
    );
}

#[test]
fn launch_args_accept_polygloss_urls() {
    let parse = |args: &[&str]| LaunchArgs::parse(&strings(args));
    let url = "polygloss://review/01926a5e-7b1c-7cde-9f00-0123456789ab";
    let args = parse(&[url]).unwrap();
    assert_eq!(args.urls, vec![url.to_owned()]);
    assert!(args.open.is_none());

    let args = parse(&["--repo", "/r", "--live", url]).unwrap();
    assert!(args.open.is_some());
    assert_eq!(args.urls, vec![url.to_owned()]);

    // Other bare arguments are still errors; so is a URL of another scheme.
    assert!(parse(&["/some/path"]).is_err());
    assert!(parse(&["https://example.com"]).is_err());
}

/// T6.15: a thread in a closed category section, focused through the
/// socket (`focus`) or a `polygloss://` URL, opens the section and its
/// sidebar panel, with the cursor on the thread.
#[gpui_kit::test]
fn focus_and_url_to_a_thread_in_a_closed_section_open_it_and_its_panel(
    cx: &mut gpui_kit::TestAppContext,
) {
    use polygloss_core::ipc::Op;
    let _sb = Sandbox::isolate();
    // `tests/it.rs` (5) is in the Tests section, closed by default.
    let repo = crate::categories::mixed_repo(20);
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let thread_id = crate::threads::create(
        &mut shell,
        &tab,
        crate::threads::line("tests/it.rs", Side::New, 4, 4),
        ThreadKind::Comment,
        "Does this cover the error path?",
        crate::threads::human(),
    );
    crate::threads::reload(&mut shell, &tab);
    let tests = crate::categories::section_id(&mut shell, &tab, "tests");
    let state = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            let open = t.viewport.read(cx).section_open(tests);
            let panel = polygloss_app::tree::file_tree(t)
                .and_then(|tree| tree.read(cx).panels().open_key().map(ToString::to_string));
            (open, panel)
        })
    };
    let close = |shell: &mut Shell| {
        tab.update(shell.cx, |t, cx| {
            t.viewport
                .update(cx, |v, cx| v.set_section_open(tests, false, cx))
        });
        crate::categories::go_to_file(shell, &tab, 2);
        assert_eq!(state(shell), (Some(false), Some("changes".to_owned())));
    };
    let on_thread = Some(CursorPos {
        file_idx: 5,
        side: Side::New,
        line: 3,
        range_start: None,
    });

    close(&mut shell);
    let task = shell.cx.update(|_, cx| {
        polygloss_app::ipc::handle(
            Op::Focus {
                review_id: None,
                diff_id: None,
                path: None,
                side: None,
                line: None,
                thread_id: Some(thread_id.clone()),
            },
            cx,
        )
    });
    draw(shell.cx);
    futures::FutureExt::now_or_never(task)
        .expect("the focus finished")
        .expect("focus the thread");
    assert_eq!(state(&mut shell), (Some(true), Some("tests".to_owned())));
    assert_eq!(cursor(&mut shell, &tab), on_thread);

    close(&mut shell);
    open(&mut shell, &format_url(&PolyglossUrl::Thread(thread_id))).unwrap();
    assert_eq!(state(&mut shell), (Some(true), Some("tests".to_owned())));
    assert_eq!(cursor(&mut shell, &tab), on_thread);
}
