//! Keyboard-only and accessibility pass (T5.6, OQ-23): every action has a
//! key or a palette entry and does something where it belongs, `Tab` /
//! `⇧Tab` cycle the review tab's panes (tree → viewport → threads panel →
//! composer) with a visible focus ring, and `Esc` closes every popover and
//! dialog, handing the keyboard back.

use gpui_kit::component::WindowExt as _;
use gpui_kit::{Entity, FocusHandle, VisualTestContext};
use polygloss_app::composer::{self, ComposerKey};
use polygloss_app::iterations;
use polygloss_app::keyboard::{self, Pane};
use polygloss_app::keymap::actions::{self, tab as tab_actions};
use polygloss_app::live;
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::threads;
use polygloss_app::tree;
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, NewThread, OpenRequest, Subject, ThreadKind, ThreadStatus, Viewer,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_viewport::CursorPos;

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

fn author(kind: AuthorKind) -> Author {
    Author {
        kind,
        name: match kind {
            AuthorKind::Human => "you".into(),
            AuthorKind::Agent => "claude-code".into(),
        },
        session_id: None,
    }
}

fn keys(shell: &mut Shell, keys: &str) {
    shell.cx.simulate_keystrokes(keys);
    draw(shell.cx);
}

fn painted(cx: &mut VisualTestContext, selector: &str) -> bool {
    cx.debug_bounds(Box::leak(selector.to_owned().into_boxed_str()))
        .is_some()
}

fn create(
    shell: &mut Shell,
    tab: &Entity<ReviewTab>,
    subject: Subject,
    body: &str,
    by: AuthorKind,
) -> String {
    let (review_id, diff_id, repo) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.diff_id.clone(),
            t.opened.repo.clone(),
        )
    });
    let blobs = BlobReader::open(&repo).expect("open the object store");
    let kind = match by {
        AuthorKind::Human => ThreadKind::Comment,
        AuthorKind::Agent => ThreadKind::Question,
    };
    let id = shell
        .core
        .create_thread(
            &NewThread {
                review_id,
                diff_id,
                subject,
                kind,
                body_md: body.into(),
                author: author(by),
            },
            &blobs,
        )
        .expect("create the thread");
    tab.update(shell.cx, threads::reload);
    draw(shell.cx);
    id
}

fn new_line(path: &str, line: u32) -> Subject {
    Subject::Line {
        path: path.into(),
        side: Side::New,
        start_line: line,
        line,
    }
}

/// The cursor on 0-based new `line` of file `file_idx`, the keyboard in the
/// diff.
fn cursor_at(shell: &mut Shell, tab: &Entity<ReviewTab>, file_idx: u32, line: u32) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| {
        v.set_cursor(
            Some(CursorPos {
                file_idx,
                side: Side::New,
                line,
                range_start: None,
            }),
            cx,
        )
    });
    focus_viewport(shell, tab);
}

fn focus_viewport(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    let focus = tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    focus_on(shell, &focus);
}

fn focus_on(shell: &mut Shell, focus: &FocusHandle) {
    shell.cx.update(|window, cx| window.focus(focus, cx));
    draw(shell.cx);
}

fn pane_of(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Option<Pane> {
    shell.cx.update(|window, cx| {
        let t = tab.read(cx);
        keyboard::focused_pane(t, window, cx)
    })
}

fn has_dialog(shell: &mut Shell) -> bool {
    shell.cx.update(|window, cx| window.has_active_dialog(cx))
}

/// Gives `pane` the keyboard as `Tab` does.
fn focus_pane(shell: &mut Shell, tab: &Entity<ReviewTab>, pane: &Pane) {
    tab.update_in(shell.cx, |t, window, cx| {
        keyboard::focus_pane(t, pane, window, cx)
    });
    draw(shell.cx);
}

fn panel_focus(shell: &mut Shell, tab: &Entity<ReviewTab>) -> FocusHandle {
    tab.read_with(shell.cx, |t, cx| {
        threads::threads(t)
            .expect("the tab has threads")
            .read(cx)
            .panel_focus()
            .clone()
    })
}

fn composer_focus(shell: &mut Shell, tab: &Entity<ReviewTab>, key: &ComposerKey) -> FocusHandle {
    tab.read_with(shell.cx, |t, cx| {
        composer::composer(t, key, cx)
            .expect("the composer is open")
            .read(cx)
            .focus_target(cx)
    })
}

/// A line composer on `config.rs` new line 5 (0-based 4), opened with `c`.
fn open_line_composer(shell: &mut Shell, tab: &Entity<ReviewTab>) -> ComposerKey {
    cursor_at(shell, tab, 0, 4);
    keys(shell, "c");
    let key = ComposerKey::Line {
        path: "src/config.rs".into(),
        side: Side::New,
        start_line: 5,
        line: 5,
    };
    assert!(
        tab.read_with(shell.cx, |t, cx| composer::composer(t, &key, cx).is_some()),
        "c opened the composer"
    );
    key
}

/// Where each registry action is dispatched from in the test: its
/// namespace's pane.
fn target_for(namespace: &str, composer_key: &ComposerKey) -> Pane {
    match namespace {
        "tree" => Pane::Tree,
        "composer" => Pane::Composer(composer_key.clone()),
        "threads" => Pane::Threads,
        _ => Pane::Viewport,
    }
}

/// Our action namespaces (gpui-kit's are its own business).
const OUR_NAMESPACES: &[&str] = &[
    "viewport",
    "tree",
    "composer",
    "threads",
    "tab",
    "window",
    "home",
    "open_flow",
    "submit",
    "find",
];

#[gpui_kit::test]
fn every_action_has_binding_or_palette_entry(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    create(
        &mut shell,
        &tab,
        new_line("src/config.rs", 2),
        "Why this?",
        AuthorKind::Agent,
    );
    let composer_key = open_line_composer(&mut shell, &tab);

    // Every action of ours GPUI knows has a key or a palette row: nothing
    // is reachable by the mouse alone.
    let names: Vec<&'static str> = shell.cx.update(|_, cx| cx.all_action_names().to_vec());
    let mut unreachable = Vec::new();
    for name in names {
        let Some((namespace, _)) = name.split_once("::") else {
            continue;
        };
        if !OUR_NAMESPACES.contains(&namespace) {
            continue;
        }
        let in_palette = actions::find(name).is_some();
        let bound = shell.cx.update(|_, cx| {
            let action = cx.build_action(name, None).expect("builds");
            !cx.key_bindings()
                .borrow()
                .bindings_for_action(&*action)
                .next()
                .is_none()
        });
        if !in_palette && !bound {
            unreachable.push(name);
        }
    }
    assert!(
        unreachable.is_empty(),
        "no key and no palette entry: {unreachable:?}"
    );

    // The keyboard reaches what the mouse did alone before T5.6.
    for name in [
        "tab::FocusNextPane",
        "tab::FocusPrevPane",
        "tab::ChooseIteration",
        "viewport::FileMenu",
        "viewport::ToggleCollapse",
        "tree::FocusFilter",
        "tree::FilterMenu",
        "threads::SelectNext",
        "threads::SelectPrev",
        "threads::Open",
        "threads::Reply",
        "threads::ToggleResolved",
        "threads::EditComment",
        "threads::DeleteComment",
    ] {
        assert!(actions::find(name).is_some(), "{name} is in the palette");
    }

    // Every palette action does something where it belongs: a handler on
    // the dispatch path from its pane (or app-wide). An action nobody
    // handles would be a dead palette row.
    let mut dead = Vec::new();
    for info in actions::ACTIONS {
        let pane = target_for(info.namespace(), &composer_key);
        focus_pane(&mut shell, &tab, &pane);
        assert_eq!(pane_of(&mut shell, &tab), Some(pane), "{}", info.name);
        let action = (info.build)();
        let available = shell.cx.update(|window, cx| {
            window.is_action_available(&*action, cx) || cx.is_action_available(&*action)
        });
        if !available {
            dead.push(info.name);
        }
    }
    assert!(dead.is_empty(), "no handler in a review tab: {dead:?}");
}

#[gpui_kit::test]
fn focus_cycles_between_panes(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let key = open_line_composer(&mut shell, &tab);
    shell.cx.simulate_input("typed");
    draw(shell.cx);
    let text = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            composer::composer(t, &key, cx).map(|c| c.read(cx).text(cx))
        })
    };
    assert_eq!(text(&mut shell).as_deref(), Some("typed"));

    // From the tree: Tab visits viewport, threads panel, composer, and
    // wraps around to the tree.
    focus_pane(&mut shell, &tab, &Pane::Tree);
    assert_eq!(pane_of(&mut shell, &tab), Some(Pane::Tree));
    assert!(painted(shell.cx, "focus-ring-tree"));
    assert!(!painted(shell.cx, "focus-ring-viewport"));
    let expected = [
        Pane::Viewport,
        Pane::Threads,
        Pane::Composer(key.clone()),
        Pane::Tree,
    ];
    for want in &expected {
        keys(&mut shell, "tab");
        assert_eq!(pane_of(&mut shell, &tab).as_ref(), Some(want), "tab");
    }
    // Each pane shows it has the keyboard, and only that pane.
    keys(&mut shell, "tab");
    assert!(painted(shell.cx, "focus-ring-viewport"));
    assert!(!painted(shell.cx, "focus-ring-tree"));
    keys(&mut shell, "tab");
    assert!(painted(shell.cx, "focus-ring-threads"));
    assert!(!painted(shell.cx, "focus-ring-viewport"));
    keys(&mut shell, "tab");
    assert!(painted(shell.cx, &format!("composer-focus-ring-{key}")));
    assert!(!painted(shell.cx, "focus-ring-threads"));
    // Tab leaves the composer's text alone (no indent typed into it).
    assert_eq!(text(&mut shell).as_deref(), Some("typed"));

    // ⇧Tab goes the other way.
    for want in [Pane::Threads, Pane::Viewport, Pane::Tree] {
        keys(&mut shell, "shift-tab");
        assert_eq!(pane_of(&mut shell, &tab), Some(want), "shift-tab");
    }
    keys(&mut shell, "shift-tab");
    assert_eq!(pane_of(&mut shell, &tab), Some(Pane::Composer(key.clone())));
    assert_eq!(text(&mut shell).as_deref(), Some("typed"));

    // The tree's filter box is part of the tree's stop: `/` goes there, and
    // Tab goes on to the viewport.
    focus_pane(&mut shell, &tab, &Pane::Tree);
    keys(&mut shell, "/");
    let filter_focused = tab.read_with(shell.cx, |t, cx| {
        tree::file_tree(t).unwrap().read(cx).filter_focus(cx)
    });
    assert!(
        shell
            .cx
            .update(|window, _| filter_focused.is_focused(window))
    );
    keys(&mut shell, "tab");
    assert_eq!(pane_of(&mut shell, &tab), Some(Pane::Viewport));

    // A hidden threads panel is skipped; without composers the cycle is
    // tree ⇄ viewport.
    tab.update(shell.cx, |t, cx| t.toggle_threads_panel(cx));
    let composer_focus = composer_focus(&mut shell, &tab, &key);
    focus_on(&mut shell, &composer_focus);
    keys(&mut shell, "escape");
    assert!(tab.read_with(shell.cx, |t, cx| composer::composer(t, &key, cx).is_none()));
    focus_viewport(&mut shell, &tab);
    keys(&mut shell, "tab");
    assert_eq!(pane_of(&mut shell, &tab), Some(Pane::Tree));
    keys(&mut shell, "tab");
    assert_eq!(pane_of(&mut shell, &tab), Some(Pane::Viewport));
    keys(&mut shell, "shift-tab");
    assert_eq!(pane_of(&mut shell, &tab), Some(Pane::Tree));
}

#[gpui_kit::test]
fn threads_panel_works_from_the_keyboard(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let question = create(
        &mut shell,
        &tab,
        new_line("src/config.rs", 2),
        "Why this?",
        AuthorKind::Agent,
    );
    let later = create(
        &mut shell,
        &tab,
        new_line("src/main.rs", 3),
        "And this?",
        AuthorKind::Agent,
    );
    let focus = panel_focus(&mut shell, &tab);
    focus_on(&mut shell, &focus);
    let selected = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            threads::threads(t)
                .unwrap()
                .read(cx)
                .selected()
                .map(str::to_owned)
        })
    };
    // j / k walk the rows; the selected row is marked.
    keys(&mut shell, "j");
    assert_eq!(selected(&mut shell).as_deref(), Some(question.as_str()));
    assert!(painted(
        shell.cx,
        &format!("threads-panel-selected-{question}")
    ));
    keys(&mut shell, "j");
    assert_eq!(selected(&mut shell).as_deref(), Some(later.as_str()));
    keys(&mut shell, "k");
    assert_eq!(selected(&mut shell).as_deref(), Some(question.as_str()));

    // x resolves the selected thread, at once.
    keys(&mut shell, "x");
    assert_eq!(
        shell.core.thread(&question, Viewer::Agent).unwrap().status,
        ThreadStatus::Resolved
    );
    // r opens its reply composer with the keyboard in it.
    keys(&mut shell, "r");
    let reply = ComposerKey::Reply {
        thread_id: question.clone(),
    };
    let reply_focus = composer_focus(&mut shell, &tab, &reply);
    assert!(shell.cx.update(|window, _| reply_focus.is_focused(window)));
    shell.cx.simulate_input("Fixed");
    keys(&mut shell, "cmd-enter");
    let t = shell.core.thread(&question, Viewer::Human).unwrap();
    assert_eq!(t.comments.last().map(|c| c.body_md.as_str()), Some("Fixed"));

    // Enter jumps into the diff: the cursor on the thread's line, the
    // keyboard in the viewport.
    // (Resolved, `question` moved below `later`, into RESOLVED.)
    focus_on(&mut shell, &focus);
    keys(&mut shell, "up");
    assert_eq!(selected(&mut shell).as_deref(), Some(later.as_str()));
    keys(&mut shell, "enter");
    assert_eq!(pane_of(&mut shell, &tab), Some(Pane::Viewport));
    let cursor = tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).cursor());
    // `main.rs` line 3 is 0-based line 2.
    assert_eq!(cursor.map(|c| (c.file_idx, c.line)), Some((2, 2)));

    // From the diff the palette's thread actions act on the thread the
    // cursor is on: e edits (here: nothing of ours), Delete asks first.
    let own = create(
        &mut shell,
        &tab,
        new_line("src/config.rs", 6),
        "Mine",
        AuthorKind::Human,
    );
    focus_on(&mut shell, &focus);
    let row = tab.read_with(shell.cx, |t, cx| {
        threads::panel::rows(threads::threads(t).unwrap().read(cx), cx)
            .iter()
            .position(|(id, _)| *id == own)
            .unwrap()
    });
    tab.update(shell.cx, |t, cx| {
        threads::threads(t)
            .unwrap()
            .update(cx, |m, cx| m.select_row(row, cx))
    });
    draw(shell.cx);
    keys(&mut shell, "e");
    let edit = ComposerKey::Edit {
        comment_id: shell.core.thread(&own, Viewer::Human).unwrap().comments[0]
            .id
            .clone(),
    };
    assert!(tab.read_with(shell.cx, |t, cx| composer::composer(t, &edit, cx).is_some()));
    keys(&mut shell, "escape");
    focus_on(&mut shell, &focus);
    shell
        .cx
        .update(|window, cx| window.dispatch_action(Box::new(actions::threads::DeleteComment), cx));
    draw(shell.cx);
    // A draft goes at once (published comments ask first).
    assert!(!has_dialog(&mut shell));
    let gone = match shell.core.thread(&own, Viewer::Human) {
        Err(_) => true,
        Ok(t) => t.comments.iter().all(|c| c.deleted),
    };
    assert!(gone, "the draft is deleted");
}

/// A repo whose `feature` branch gains a second commit (a second
/// iteration of the compare review).
fn two_iteration_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let body: String = (0..30).map(|i| format!("fn a_{i}() {{}}\n")).collect();
    repo.write("src/a.rs", body.as_bytes());
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write(
        "src/a.rs",
        body.replace("fn a_3() {}", "fn a_3() { x(); }").as_bytes(),
    );
    repo.commit("feature 1");
    repo
}

fn compare_branches(repo: &FixtureRepo) -> OpenRequest {
    OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/heads/main".into(),
            head: "refs/heads/feature".into(),
            mode: CompareMode::ThreeDot,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

/// With the keyboard in `pane`, opens a popover or dialog with `open`
/// (keys), checks it is open with `is_open`, presses Esc, and checks it
/// closed and the keyboard went back to `pane`.
fn esc_closes(
    shell: &mut Shell,
    tab: &Entity<ReviewTab>,
    what: &str,
    pane: Pane,
    open: &str,
    is_open: &dyn Fn(&mut Shell) -> bool,
) {
    focus_pane(shell, tab, &pane);
    keys(shell, open);
    assert!(is_open(shell), "{what} opens with {open}");
    keys(shell, "escape");
    assert!(!is_open(shell), "Esc closes {what}");
    assert_eq!(
        pane_of(shell, tab),
        Some(pane),
        "the keyboard is back where it was after {what}"
    );
}

#[gpui_kit::test]
fn escape_closes_popovers_and_dialogs(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = two_iteration_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_branches(&repo)).unwrap();
    // A second iteration, so the iteration picker shows.
    repo.write("src/a.rs", b"fn a() {}\n");
    repo.commit("feature 2");
    tab.update_in(shell.cx, live::refresh_tab);
    draw(shell.cx);
    assert!(tab.read_with(shell.cx, |t, _| iterations::picker_visible(t)));
    let own = create(
        &mut shell,
        &tab,
        new_line("src/a.rs", 1),
        "Published",
        AuthorKind::Human,
    );
    let review = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    shell
        .core
        .submit_review(&review, polygloss_core::review::Verdict::Comment, "", None)
        .expect("publish the draft");
    tab.update(shell.cx, threads::reload);
    draw(shell.cx);

    let dialog = |shell: &mut Shell| has_dialog(shell);
    // Dialogs.
    let v = || Pane::Viewport;
    esc_closes(&mut shell, &tab, "the palette", v(), "cmd-k", &dialog);
    esc_closes(&mut shell, &tab, "the cheat sheet", v(), "?", &dialog);
    esc_closes(&mut shell, &tab, "the file finder", v(), "cmd-p", &dialog);
    esc_closes(&mut shell, &tab, "the open flow", v(), "cmd-o", &dialog);
    esc_closes(
        &mut shell,
        &tab,
        "the submit dialog",
        v(),
        "cmd-shift-enter",
        &dialog,
    );
    // The delete confirmation of a published comment.
    let panel = panel_focus(&mut shell, &tab);
    focus_on(&mut shell, &panel);
    keys(&mut shell, "j");
    let before = shell.core.thread(&own, Viewer::Human).unwrap();
    assert!(!before.comments[0].draft);
    shell
        .cx
        .update(|window, cx| window.dispatch_action(Box::new(actions::threads::DeleteComment), cx));
    draw(shell.cx);
    assert!(has_dialog(&mut shell), "delete asks first");
    keys(&mut shell, "escape");
    assert!(!has_dialog(&mut shell));
    assert!(!shell.core.thread(&own, Viewer::Human).unwrap().comments[0].deleted);

    // Popovers: the file header's ⋯ menu, the tree's filter menu and the
    // iteration menu, opened from the keyboard.
    cursor_at(&mut shell, &tab, 0, 3);
    let file_menu = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).menu_file().is_some())
    };
    esc_closes(&mut shell, &tab, "the file menu", v(), "m", &file_menu);
    let filter_menu = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            tree::file_tree(t).unwrap().read(cx).filter_menu_open()
        })
    };
    esc_closes(
        &mut shell,
        &tab,
        "the filter menu",
        Pane::Tree,
        "f",
        &filter_menu,
    );
    let iteration_menu =
        |shell: &mut Shell| tab.read_with(shell.cx, |t, _| iterations::menu_open(t));
    esc_closes(
        &mut shell,
        &tab,
        "the iteration menu",
        v(),
        "i",
        &iteration_menu,
    );
    // The find bar.
    let find = |shell: &mut Shell| {
        tab.read_with(shell.cx, |t, cx| {
            polygloss_app::find::find_bar(t).is_some_and(|b| b.read(cx).is_open())
        })
    };
    esc_closes(&mut shell, &tab, "the find bar", v(), "cmd-f", &find);

    // `z` folds the cursor's file, and unfolds it.
    focus_viewport(&mut shell, &tab);
    keys(&mut shell, "z");
    assert!(tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).collapsed() == vec![0]));
    keys(&mut shell, "z");
    assert!(tab.read_with(shell.cx, |t, cx| t.viewport.read(cx).collapsed().is_empty()));

    // A live tab's base picker.
    let live = FixtureRepo::init(ObjectFormat::Sha1);
    live.write("a.txt", b"one\n");
    live.commit("base");
    live.write("a.txt", b"two\n");
    let live_tab = shell
        .open(OpenRequest {
            worktree: live.path().to_path_buf(),
            source: Source::Live { since: Since::Head },
            label: None,
            pin: None,
            actor: Actor::human(),
        })
        .unwrap();
    let live_viewport = live_tab.read_with(shell.cx, |t, _| t.viewport_focus().clone());
    focus_on(&mut shell, &live_viewport);
    shell
        .cx
        .update(|window, cx| window.dispatch_action(Box::new(tab_actions::ChooseBase), cx));
    draw(shell.cx);
    assert!(has_dialog(&mut shell), "the base picker opens");
    keys(&mut shell, "escape");
    assert!(!has_dialog(&mut shell), "Esc closes the base picker");
    assert!(
        shell
            .cx
            .update(|window, cx| live_viewport.contains_focused(window, cx))
    );
    // `tab::AssignToSession` from a review tab opens the session picker.
    shell
        .cx
        .update(|window, cx| window.dispatch_action(Box::new(tab_actions::AssignToSession), cx));
    draw(shell.cx);
    assert!(has_dialog(&mut shell), "the session picker opens");
    keys(&mut shell, "escape");
    assert!(!has_dialog(&mut shell), "Esc closes the session picker");
}

#[gpui_kit::test]
fn submit_dialog_verdicts_and_tab_work_from_the_keyboard(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    focus_viewport(&mut shell, &tab);
    keys(&mut shell, "cmd-shift-enter");
    let dialog = tab
        .read_with(shell.cx, |t, _| polygloss_app::submit::dialog(t))
        .expect("the dialog is open");
    let verdict = |shell: &mut Shell| dialog.read_with(shell.cx, |d, _| d.verdict());
    // ⌘1 / ⌘2 / ⌘3 pick the verdicts in the order the rows show them, from
    // the summary too.
    let summary = dialog.read_with(shell.cx, |d, cx| d.summary_focus(cx));
    assert!(shell.cx.update(|window, _| summary.is_focused(window)));
    keys(&mut shell, "cmd-3");
    assert_eq!(
        verdict(&mut shell),
        polygloss_core::review::Verdict::RequestChanges
    );
    keys(&mut shell, "cmd-2");
    assert_eq!(
        verdict(&mut shell),
        polygloss_core::review::Verdict::Approve
    );
    keys(&mut shell, "cmd-1");
    assert_eq!(
        verdict(&mut shell),
        polygloss_core::review::Verdict::Comment
    );
    // ⇥ leaves the summary (nothing typed into it) for the dialog's other
    // controls, and stays in the dialog; ⇧⇥ comes back.
    shell.cx.simulate_input("Summary");
    draw(shell.cx);
    keys(&mut shell, "tab");
    assert!(!shell.cx.update(|window, _| summary.is_focused(window)));
    assert!(has_dialog(&mut shell));
    assert_eq!(dialog.read_with(shell.cx, |d, cx| d.summary(cx)), "Summary");
    keys(&mut shell, "shift-tab");
    assert!(shell.cx.update(|window, _| summary.is_focused(window)));
    assert_eq!(dialog.read_with(shell.cx, |d, cx| d.summary(cx)), "Summary");
    keys(&mut shell, "escape");
    assert!(!has_dialog(&mut shell));
}
