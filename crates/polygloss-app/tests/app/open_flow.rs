//! The open flow (T3.5, design §11.3): ⌘O → repo (recent repos ranked with
//! nucleo, or Browse…) → source (Live with a base picker, Commit from a
//! virtualized fuzzy log, Branch compare with base/head pickers, three-dot by
//! default, a direct toggle and an optional label) → a review tab, or the tab
//! already showing that review.

use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use gpui_kit::component::IndexPath;
use gpui_kit::component::WindowExt as _;
use gpui_kit::{Entity, Focusable as _, ScrollStrategy, TestAppContext};
use polygloss_app::open_flow::source_step::{COMMIT_PAGE, SourceMode, SourceStep};
use polygloss_app::open_flow::{self, FlowStep, OpenFlow, ranking};
use polygloss_core::git::{CompareMode, ReviewKind, Since, Source};
use polygloss_core::review::{Core, ReviewFilter};
use polygloss_diff::ObjectFormat;

use crate::shell::{Shell, draw, start};
use crate::support::{FixtureRepo, Sandbox};

/// Opens the flow with ⌘O.
fn open(shell: &mut Shell) -> Entity<OpenFlow> {
    shell.cx.simulate_keystrokes("cmd-o");
    draw(shell.cx);
    assert!(
        shell.cx.update(|window, cx| window.has_active_dialog(cx)),
        "⌘O opens the flow as a dialog"
    );
    shell
        .cx
        .update(|_, cx| open_flow::current(cx))
        .expect("the open flow is open")
}

/// The repo rows' display names, best first.
fn repo_names(shell: &mut Shell, flow: &Entity<OpenFlow>) -> Vec<String> {
    let list = flow.read_with(shell.cx, |f, _| f.repos().clone());
    list.read_with(shell.cx, |l, _| {
        l.delegate().matches().map(|r| r.name.to_string()).collect()
    })
}

fn search_repos(shell: &mut Shell, flow: &Entity<OpenFlow>, query: &str) -> Vec<String> {
    let list = flow.read_with(shell.cx, |f, _| f.repos().clone());
    shell
        .cx
        .update(|window, cx| list.update(cx, |l, cx| l.set_query(query, window, cx)));
    draw(shell.cx);
    repo_names(shell, flow)
}

/// Chooses `path` in the repo step and waits for the source step.
fn choose(shell: &mut Shell, flow: &Entity<OpenFlow>, path: &Path) -> Entity<SourceStep> {
    let path = path.to_path_buf();
    shell
        .cx
        .update(|window, cx| flow.update(cx, |f, cx| f.choose_repo(path, window, cx)));
    draw(shell.cx);
    flow.read_with(shell.cx, |f, _| f.source().cloned())
        .expect("the source step shows")
}

fn set_mode(shell: &mut Shell, source: &Entity<SourceStep>, mode: SourceMode) {
    shell
        .cx
        .update(|window, cx| source.update(cx, |s, cx| s.set_mode(mode, window, cx)));
    draw(shell.cx);
}

/// Adds a `repos` row for `worktree` (a directory with a `.git` dir), opened
/// at `at` (Unix ms).
fn add_repo_row(core: &Core, worktree: &Path, at: i64) {
    std::fs::create_dir_all(worktree.join(".git")).unwrap();
    let common = worktree.join(".git");
    let name = worktree.file_name().unwrap().to_string_lossy().into_owned();
    core.store
        .write(|tx| {
            tx.execute(
                "INSERT INTO repos (common_dir, display_name, object_format, default_branch, \
                 created_at, last_opened_at) VALUES (?1, ?2, 'sha1', NULL, ?3, ?3)",
                (common.to_str().unwrap(), name, at),
            )?;
            Ok(())
        })
        .unwrap();
}

/// A repo whose `main` has `n` commits (`git fast-import`), commit `i` at
/// minute `i`, subject `subject(i)`; the worktree is checked out.
fn long_history(n: usize, subject: impl Fn(usize) -> String) -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let mut stream = String::new();
    for i in 0..n {
        let msg = subject(i);
        let body = format!("line {i}\n");
        stream.push_str(&format!(
            "commit refs/heads/main\ncommitter Polygloss Fixture <f@polygloss.invalid> {} +0000\n\
             data {}\n{msg}\nM 644 inline file.txt\ndata {}\n{body}\n",
            1_767_225_600 + i * 60,
            msg.len(),
            body.len()
        ));
    }
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(repo.path())
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stream.as_bytes())
        .unwrap();
    assert!(child.wait().unwrap().success());
    repo.git(&["reset", "-q", "--hard"]);
    repo
}

/// `main` with two commits, a remote-tracking `origin/main` (and
/// `origin/HEAD`), and `feature` checked out two commits ahead of it.
fn feature_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/lib.rs", b"pub fn one() -> u32 {\n    1\n}\n");
    repo.commit("Initial commit");
    repo.write("README.md", b"# app\n");
    let main = repo.commit("Add a readme");
    repo.git(&["update-ref", "refs/remotes/origin/main", main.as_str()]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    repo.branch("feature");
    repo.checkout("feature");
    repo.write(
        "src/lib.rs",
        b"pub fn one() -> u32 {\n    1\n}\n\npub fn two() -> u32 {\n    2\n}\n",
    );
    repo.commit("Add two");
    repo.write("src/main.rs", b"fn main() {}\n");
    repo.commit("Add a binary");
    repo
}

#[test]
fn ranking_prefers_path_segment_starts() {
    let paths = [
        "/Users/d/work/api-gateway",
        "/Users/d/src/pierre-diffs",
        "/Users/d/src/polygloss",
    ];
    assert_eq!(ranking::rank_paths("", &paths), [0, 1, 2]);
    assert_eq!(ranking::rank_paths("pgl", &paths)[0], 2);
    assert_eq!(ranking::rank_paths("src/pd", &paths), [1]);
    assert!(ranking::rank_paths("zzz", &paths).is_empty());
    assert_eq!(
        ranking::rank_text("fxprsr", &["Tweak module", "Fix parser crash"]),
        [1]
    );
}

#[gpui_kit::test]
fn open_flow_recent_repos_ranked_by_nucleo(cx: &mut TestAppContext) {
    let sb = Sandbox::isolate();
    let mut shell = start(cx);
    let home = sb.home().to_path_buf();
    add_repo_row(&shell.core, &home.join("src/polygloss"), 1_000);
    add_repo_row(&shell.core, &home.join("src/pierre-diffs"), 2_000);
    add_repo_row(&shell.core, &home.join("work/api-gateway"), 3_000);
    // The most recent one is gone from disk: not offered.
    add_repo_row(&shell.core, &home.join("gone"), 4_000);
    std::fs::remove_dir_all(home.join("gone")).unwrap();

    let flow = open(&mut shell);
    assert_eq!(flow.read_with(shell.cx, |f, _| f.step()), FlowStep::Repo);
    // No query: most recently opened first.
    assert_eq!(
        repo_names(&mut shell, &flow),
        ["api-gateway", "pierre-diffs", "polygloss"]
    );
    // Rows show where the repo is, with the home directory as `~`.
    let list = flow.read_with(shell.cx, |f, _| f.repos().clone());
    let detail = list.read_with(shell.cx, |l, _| {
        l.delegate().matches().next().unwrap().detail.to_string()
    });
    assert_eq!(detail, "~/work/api-gateway");

    // Typing ranks with nucleo's path matching.
    assert_eq!(search_repos(&mut shell, &flow, "pgl")[0], "polygloss");
    assert_eq!(search_repos(&mut shell, &flow, "src/pd"), ["pierre-diffs"]);
    assert_eq!(
        search_repos(&mut shell, &flow, "src"),
        ["pierre-diffs", "polygloss"]
    );
    assert!(search_repos(&mut shell, &flow, "zzz").is_empty());
    // "Browse…" stays available whatever the query.
    let browse = list.read_with(shell.cx, |l, _| l.delegate().has_browse_row());
    assert!(browse);

    // Enter chooses the best match; these are not git repositories, so the
    // flow says so and stays on the repo step.
    search_repos(&mut shell, &flow, "api");
    shell.cx.simulate_keystrokes("enter");
    draw(shell.cx);
    let (step, error) = flow.read_with(shell.cx, |f, _| (f.step(), f.error().cloned()));
    assert_eq!(step, FlowStep::Repo);
    let error = error.expect("an error").to_string();
    assert!(
        error.contains("not a git repository") && error.contains("api-gateway"),
        "{error}"
    );
}

#[gpui_kit::test]
fn open_flow_browse_picks_a_folder(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let sub = repo.path().join("src");
    let mut shell = start(cx);
    let flow = open(&mut shell);
    shell
        .cx
        .update(|window, cx| flow.update(cx, |f, cx| f.browse(window, cx)));
    draw(shell.cx);
    assert!(shell.cx.did_prompt_for_paths(), "Browse… asks for a folder");
    let picked = sub.clone();
    shell.cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple);
        Some(vec![picked])
    });
    draw(shell.cx);
    // A folder inside a worktree opens that worktree.
    let source = flow
        .read_with(shell.cx, |f, _| f.source().cloned())
        .expect("the source step");
    let worktree = source.read_with(shell.cx, |s, _| s.worktree().map(Path::to_path_buf));
    assert_eq!(worktree.as_deref(), Some(repo.path()));

    // Back returns to the repo list, also from inside a text field (which
    // binds ⌘[ itself).
    set_mode(&mut shell, &source, SourceMode::Compare);
    let label = source.read_with(shell.cx, |s, _| s.label_input().clone());
    assert!(
        shell
            .cx
            .update(|window, cx| label.read(cx).focus_handle(cx).is_focused(window)),
        "Compare focuses the label field"
    );
    shell.cx.simulate_keystrokes("cmd-[");
    draw(shell.cx);
    assert_eq!(flow.read_with(shell.cx, |f, _| f.step()), FlowStep::Repo);
}

#[gpui_kit::test]
fn open_flow_commit_list_is_virtualized_and_fuzzy(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let total = COMMIT_PAGE as usize + 50;
    let special = |i: usize| match i {
        10 => "Add fuzzy finder".to_owned(),
        240 => "Fix parser crash on empty input".to_owned(),
        i => format!("Tweak module {i}"),
    };
    let repo = long_history(total, special);
    let mut shell = start(cx);
    let flow = open(&mut shell);
    let source = choose(&mut shell, &flow, repo.path());
    set_mode(&mut shell, &source, SourceMode::Commit);
    let list = source.read_with(shell.cx, |s, _| s.commits().clone());

    // The first page is loaded, and only the rows on screen are painted.
    let (loaded, exhausted) = list.read_with(shell.cx, |l, _| {
        (l.delegate().loaded().len(), l.delegate().exhausted())
    });
    assert_eq!((loaded, exhausted), (COMMIT_PAGE as usize, false));
    assert!(shell.cx.debug_bounds("open-flow-commit-0").is_some());
    let last = format!("open-flow-commit-{}", COMMIT_PAGE - 1);
    assert!(
        shell
            .cx
            .debug_bounds(Box::leak(last.into_boxed_str()))
            .is_none(),
        "rows below the fold are not painted"
    );
    // Newest first, with subject, author and date.
    let first = list.read_with(shell.cx, |l, _| l.delegate().loaded()[0].clone());
    assert_eq!(first.subject, format!("Tweak module {}", total - 1));
    assert_eq!(first.author, "Polygloss Fixture");

    // Scrolling to the end loads the next page.
    shell.cx.update(|window, cx| {
        list.update(cx, |l, cx| {
            l.scroll_to_item(
                IndexPath::new(COMMIT_PAGE as usize - 1),
                ScrollStrategy::Top,
                window,
                cx,
            )
        })
    });
    draw(shell.cx);
    let (loaded, exhausted) = list.read_with(shell.cx, |l, _| {
        (l.delegate().loaded().len(), l.delegate().exhausted())
    });
    assert_eq!((loaded, exhausted), (total, true));

    // Fuzzy search over subjects, authors and ids.
    let search = |shell: &mut Shell, query: &str| -> Vec<String> {
        shell
            .cx
            .update(|window, cx| list.update(cx, |l, cx| l.set_query(query, window, cx)));
        draw(shell.cx);
        list.read_with(shell.cx, |l, _| {
            l.delegate().matches().map(|c| c.subject.clone()).collect()
        })
    };
    assert_eq!(
        search(&mut shell, "fxprsrcrsh")[0],
        "Fix parser crash on empty input"
    );
    assert_eq!(search(&mut shell, "fzzy fndr")[0], "Add fuzzy finder");
    let oid = repo.oid("main~100");
    assert_eq!(
        search(&mut shell, &oid.as_str()[..10])[0],
        format!("Tweak module {}", total - 1 - 100)
    );

    // Enter opens the best match as a commit review.
    search(&mut shell, "fxprsrcrsh");
    shell.cx.simulate_keystrokes("enter");
    draw(shell.cx);
    assert!(
        shell.cx.update(|_, cx| open_flow::current(cx)).is_none(),
        "the flow closed"
    );
    let tab = shell.active_review().expect("a review tab");
    let (kind, key) = tab.read_with(shell.cx, |t, _| {
        (t.opened.kind, t.opened.review_key.clone())
    });
    assert_eq!(kind, ReviewKind::Commit);
    let fix = repo.oid(&format!("main~{}", total - 1 - 240));
    assert_eq!(key, format!("commit:{}", fix.as_str()));
}

#[gpui_kit::test]
fn open_flow_compare_defaults_to_three_dot(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let mut shell = start(cx);
    let flow = open(&mut shell);
    let source = choose(&mut shell, &flow, repo.path());
    set_mode(&mut shell, &source, SourceMode::Compare);

    // Base: the default branch; head: the branch checked out.
    let (base, head, direct) =
        source.read_with(shell.cx, |s, cx| (s.base(cx), s.head(cx), s.direct()));
    assert_eq!(base.as_deref(), Some("refs/remotes/origin/main"));
    assert_eq!(head.as_deref(), Some("refs/heads/feature"));
    assert!(!direct, "three-dot by default");
    let req = source
        .read_with(shell.cx, |s, cx| s.request(cx))
        .expect("a complete request");
    assert_eq!(
        req.source,
        Source::Compare {
            base: "refs/remotes/origin/main".into(),
            head: "refs/heads/feature".into(),
            mode: CompareMode::ThreeDot,
        }
    );
    assert_eq!(req.label, None);
    assert_eq!(req.worktree, repo.path());

    // The pickers offer every branch, remote branch and tag.
    let offered = source.read_with(shell.cx, |s, _| s.ref_names());
    assert_eq!(
        offered,
        [
            "refs/heads/feature",
            "refs/heads/main",
            "refs/remotes/origin/main"
        ]
    );

    // ⌘⏎ opens it, from the label field that has focus.
    let label = source.read_with(shell.cx, |s, _| s.label_input().clone());
    assert!(
        shell
            .cx
            .update(|window, cx| label.read(cx).focus_handle(cx).is_focused(window))
    );
    shell.cx.simulate_keystrokes("cmd-enter");
    draw(shell.cx);
    let tab = shell.active_review().expect("a review tab");
    let key = tab.read_with(shell.cx, |t, _| t.opened.review_key.clone());
    assert_eq!(key, "compare:refs/remotes/origin/main...refs/heads/feature");
}

#[gpui_kit::test]
fn open_flow_direct_toggle_and_label(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    let mut shell = start(cx);
    let flow = open(&mut shell);
    let source = choose(&mut shell, &flow, repo.path());
    set_mode(&mut shell, &source, SourceMode::Compare);

    shell.cx.update(|window, cx| {
        source.update(cx, |s, cx| {
            s.set_base("refs/heads/main", window, cx);
            s.set_direct(true, cx);
        })
    });
    // The label is typed into its field; blank labels are no label.
    shell
        .cx
        .update(|window, cx| source.update(cx, |s, cx| s.set_label("   ", window, cx)));
    assert_eq!(
        source.read_with(shell.cx, |s, cx| s.request(cx).unwrap().label),
        None
    );
    shell
        .cx
        .update(|window, cx| source.update(cx, |s, cx| s.focus_label(window, cx)));
    shell
        .cx
        .update(|window, cx| source.update(cx, |s, cx| s.set_label("", window, cx)));
    shell.cx.simulate_input("PR #123");
    draw(shell.cx);
    let req = source.read_with(shell.cx, |s, cx| s.request(cx)).unwrap();
    assert_eq!(
        req.source,
        Source::Compare {
            base: "refs/heads/main".into(),
            head: "refs/heads/feature".into(),
            mode: CompareMode::Direct,
        }
    );
    assert_eq!(req.label.as_deref(), Some("PR #123"));

    // The same ref on both sides is not a compare.
    shell
        .cx
        .update(|window, cx| source.update(cx, |s, cx| s.set_head("refs/heads/main", window, cx)));
    let err = source
        .read_with(shell.cx, |s, cx| s.request(cx))
        .expect_err("same refs");
    assert!(err.contains("different"), "{err}");
    shell.cx.update(|window, cx| {
        source.update(cx, |s, cx| s.set_head("refs/heads/feature", window, cx))
    });

    // Enter in the label field opens the review, two-dot, with the label.
    shell.cx.simulate_keystrokes("enter");
    draw(shell.cx);
    let tab = shell.active_review().expect("a review tab");
    let (key, review_id) = tab.read_with(shell.cx, |t, _| {
        (t.opened.review_key.clone(), t.review_id.clone())
    });
    assert_eq!(key, "compare:refs/heads/main..refs/heads/feature");
    let summary = shell
        .core
        .review_summaries(&ReviewFilter::default())
        .unwrap()
        .into_iter()
        .find(|s| s.review_id == review_id)
        .unwrap();
    assert_eq!(summary.label.as_deref(), Some("PR #123"));
}

#[gpui_kit::test]
fn open_flow_opens_tab_or_focuses_existing(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = feature_repo();
    repo.write("src/lib.rs", b"pub fn one() -> u32 {\n    11\n}\n");
    let mut shell = start(cx);

    // Mouse-free: ⌘O, pick the repo, Live since HEAD, ⌘⏎.
    let flow = open(&mut shell);
    let source = choose(&mut shell, &flow, repo.path());
    let (mode, since) = source.read_with(shell.cx, |s, _| (s.mode(), s.since()));
    assert_eq!(mode, SourceMode::Live, "Live is the first source");
    assert_eq!(since, Since::MergeBase, "the default base");
    source.update(shell.cx, |s, cx| s.set_since(Since::Head, cx));
    shell.cx.simulate_keystrokes("cmd-enter");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 1));
    let first = shell.active_review().unwrap();
    let key = first.read_with(shell.cx, |t, _| t.opened.review_key.clone());
    assert_eq!(
        key,
        format!("worktree:{}@feature#since=HEAD", repo.path().display())
    );

    // Back on Home, the same choices focus the open tab instead of adding one.
    shell
        .main
        .update_in(shell.cx, |m, window, cx| m.activate_tab(0, window, cx));
    let flow = open(&mut shell);
    // The repo is now the most recent one: Enter picks it.
    shell.cx.simulate_keystrokes("enter");
    draw(shell.cx);
    let source = flow
        .read_with(shell.cx, |f, _| f.source().cloned())
        .expect("the source step");
    source.update(shell.cx, |s, cx| s.set_since(Since::Head, cx));
    shell.cx.simulate_keystrokes("cmd-enter");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (2, 1), "no second tab for the same review");
    assert_eq!(shell.active_review().unwrap(), first);

    // A different base is a different review: a new tab.
    let flow = open(&mut shell);
    let source = choose(&mut shell, &flow, repo.path());
    source.update(shell.cx, |s, cx| s.set_since(Since::MergeBase, cx));
    shell.cx.simulate_keystrokes("cmd-enter");
    draw(shell.cx);
    assert_eq!(shell.tabs(), (3, 2));
}
