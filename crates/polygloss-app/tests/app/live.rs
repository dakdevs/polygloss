//! Live mode (T3.11, design §10, §5.2, §11.7, ADR-0008, ADR-0009): the
//! watcher and its filter, the banner that never moves the view, `R`
//! swapping the new state in with the scroll anchor line-mapped, collapsed
//! files, revealed context and Viewed kept, the base picker, Snapshot,
//! compare reviews watching their refs, and `index.lock` retries.
//!
//! These tests use real FSEvents: the watcher thread only sends into a
//! channel that the app polls every `WATCHER_POLL` of GPUI's fake clock,
//! so [`wait_until`] sleeps a little real time and advances the clock.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::IndexPath;
use gpui_kit::{Entity, SharedString};
use polygloss_app::keymap::actions::tab as tab_actions;
use polygloss_app::live::recompute::{self, Shown, Target};
use polygloss_app::live::watcher::{PathFilter, Verdict};
use polygloss_app::live::{self, base_picker, refresh as live_refresh};
use polygloss_app::review_tab::{BannerKind, ReviewTab};
use polygloss_core::git::{CompareMode, RepoInfo, Since, Source, discover};
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;
use polygloss_diff::{
    FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, ObjectFormat, Oid, Side,
};
use polygloss_viewport::{DiffProvider, FileFlags, RowKey, ScrollAnchor, ScrollTarget};

use crate::shell::{Shell, draw, start};
use crate::support::{FixtureRepo, Sandbox};

fn lines(f: impl Fn(usize) -> String, n: usize) -> String {
    (0..n).map(f).collect()
}

/// `src/a.rs` at the base: 120 one-line functions.
fn a_base() -> String {
    lines(|i| format!("fn a_{i}() {{}}\n"), 120)
}

/// `src/a.rs` in the working tree: lines 20..80 edited, `extra` lines on
/// top.
fn a_edited(extra: usize) -> String {
    let top = lines(|i| format!("// added {i}\n"), extra);
    let body = lines(
        |i| {
            if (20..80).contains(&i) {
                format!("fn a_{i}() {{ edited(); }}\n")
            } else {
                format!("fn a_{i}() {{}}\n")
            }
        },
        120,
    );
    top + &body
}

/// A repo on branch `feature` (from `main`): `src/b.rs` changed and
/// committed on `feature`, `src/a.rs` edited in the working tree, and a
/// `.gitignore` for `target/` and `*.log`. The live diff against the merge
/// base shows, in order, `src/a.rs` and `src/b.rs`.
fn live_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write(".gitignore", b"target/\n*.log\n");
    repo.write("notes.md", b"# Notes\n");
    repo.write("src/a.rs", a_base().as_bytes());
    repo.write(
        "src/b.rs",
        lines(|i| format!("fn b_{i}() {{}}\n"), 120).as_bytes(),
    );
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write(
        "src/b.rs",
        lines(
            |i| {
                if i == 50 {
                    "fn b_50() { changed(); }\n".to_owned()
                } else {
                    format!("fn b_{i}() {{}}\n")
                }
            },
            120,
        )
        .as_bytes(),
    );
    repo.commit("feature work");
    repo.write("src/a.rs", a_edited(0).as_bytes());
    repo
}

fn live_req(worktree: &Path, since: Since) -> OpenRequest {
    OpenRequest {
        worktree: worktree.to_path_buf(),
        source: Source::Live { since },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

fn compare_req(worktree: &Path) -> OpenRequest {
    OpenRequest {
        worktree: worktree.to_path_buf(),
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

/// Opens `req` and starts its watcher (tests have no frame loop to run the
/// tab's next-frame callback), then gives FSEvents a moment to start.
fn open_watched(shell: &mut Shell, req: OpenRequest) -> Entity<ReviewTab> {
    let tab = shell.open(req).expect("open the review");
    shell.cx.update(|window, cx| {
        window.simulate_next_frame(cx);
    });
    draw(shell.cx);
    let watching = tab.read_with(shell.cx, |t, _| {
        live::live(t).is_some_and(|l| l.watcher().is_some())
    });
    assert!(watching, "the watcher started");
    std::thread::sleep(Duration::from_millis(300));
    tab
}

/// Lets real time pass (FSEvents, the debouncer) and advances GPUI's clock
/// (the watcher poll, retries) until `cond` holds.
fn wait_until(shell: &mut Shell, what: &str, mut cond: impl FnMut(&mut Shell) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        draw(shell.cx);
        if cond(shell) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
        shell.cx.executor().advance_clock(Duration::from_millis(50));
    }
}

/// Lets `real` time pass while the app runs (to see that nothing happens).
fn idle(shell: &mut Shell, real: Duration) {
    let end = Instant::now() + real;
    while Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
        shell.cx.executor().advance_clock(Duration::from_millis(50));
        draw(shell.cx);
    }
}

fn banners(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<(BannerKind, SharedString)> {
    let strip = tab.read_with(shell.cx, |t, _| t.banners.clone());
    strip.read_with(shell.cx, |b, _| b.banners())
}

fn banner(shell: &mut Shell, tab: &Entity<ReviewTab>, kind: BannerKind) -> Option<String> {
    banners(shell, tab)
        .into_iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, t)| t.to_string())
}

fn recomputes(shell: &mut Shell, tab: &Entity<ReviewTab>) -> u64 {
    tab.read_with(shell.cx, |t, _| live::live(t).map_or(0, |l| l.recomputes()))
}

fn busy(shell: &mut Shell, tab: &Entity<ReviewTab>) -> bool {
    tab.read_with(shell.cx, |t, _| live::live(t).is_some_and(|l| l.busy()))
}

fn diff_id(shell: &mut Shell, tab: &Entity<ReviewTab>) -> String {
    tab.read_with(shell.cx, |t, _| t.opened.diff_id.as_str().to_owned())
}

fn paths(shell: &mut Shell, tab: &Entity<ReviewTab>) -> Vec<String> {
    tab.read_with(shell.cx, |t, _| {
        t.opened
            .files
            .iter()
            .map(|f| f.display_path().to_owned())
            .collect()
    })
}

/// The viewport's anchor and painted rows.
fn view(
    shell: &mut Shell,
    tab: &Entity<ReviewTab>,
) -> (polygloss_viewport::ScrollAnchor, Vec<String>) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.read_with(shell.cx, |v, _| (v.anchor(), v.debug().visible_rows))
}

/// Code rows' text (a unified row is `old new marker text`).
fn texts(rows: &[String]) -> Vec<String> {
    rows.iter()
        .filter(|r| !r.starts_with("=="))
        .map(|r| r.get(14..).unwrap_or(r).to_owned())
        .collect()
}

/// Presses `R` in the tab and waits for the refresh to land.
fn refresh(shell: &mut Shell, tab: &Entity<ReviewTab>) {
    shell.cx.simulate_keystrokes("shift-r");
    wait_until(shell, "the refresh", |s| !busy(s, tab));
}

fn scroll_to_line(shell: &mut Shell, tab: &Entity<ReviewTab>, file_idx: u32, line: u32) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| {
        v.scroll_to(
            ScrollTarget::Line {
                file_idx,
                side: Side::New,
                line,
            },
            cx,
        )
    });
    draw(shell.cx);
}

#[gpui_kit::test]
fn watcher_change_shows_banner_without_shifting_view(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, live_req(repo.path(), Since::MergeBase));
    assert_eq!(paths(&mut shell, &tab), ["src/a.rs", "src/b.rs"]);
    assert!(banners(&mut shell, &tab).is_empty());
    scroll_to_line(&mut shell, &tab, 0, 40);
    let before = view(&mut shell, &tab);
    let shown = diff_id(&mut shell, &tab);

    // An agent writes five lines above what is on screen.
    repo.write("src/a.rs", a_edited(5).as_bytes());
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::LiveChanges).is_some()
    });
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::LiveChanges).as_deref(),
        Some("1 file changed")
    );
    // Nothing moved: same state, anchor and rows until `R`.
    assert_eq!(diff_id(&mut shell, &tab), shown);
    assert_eq!(view(&mut shell, &tab), before);
    idle(&mut shell, Duration::from_millis(300));
    assert_eq!(view(&mut shell, &tab), before);

    refresh(&mut shell, &tab);
    assert_ne!(diff_id(&mut shell, &tab), shown);
    assert!(banners(&mut shell, &tab).is_empty(), "the banner is gone");
}

#[gpui_kit::test]
fn refresh_keeps_scroll_anchor_via_line_mapping(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, live_req(repo.path(), Since::MergeBase));
    scroll_to_line(&mut shell, &tab, 0, 60);
    let (anchor, rows) = view(&mut shell, &tab);
    assert_eq!(
        anchor.row,
        RowKey::Line {
            side: Side::New,
            line: 60
        }
    );

    repo.write("src/a.rs", a_edited(5).as_bytes());
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::LiveChanges).is_some()
    });
    refresh(&mut shell, &tab);
    let (after, after_rows) = view(&mut shell, &tab);
    // The same line (now five lines lower in the file) is at the same place.
    assert_eq!(after.file_idx, 0);
    assert_eq!(
        after.row,
        RowKey::Line {
            side: Side::New,
            line: 65
        }
    );
    assert_eq!(after.offset_px, anchor.offset_px);
    assert_eq!(texts(&after_rows)[..10], texts(&rows)[..10]);
}

#[gpui_kit::test]
fn refresh_preserves_collapsed_expanded_viewed(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, live_req(repo.path(), Since::MergeBase));
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    // a.rs: context revealed and viewed; b.rs: viewed and collapsed.
    // Viewed marks are stored (a refresh reloads them from the store,
    // design §9); marking collapses both, so a.rs is expanded again.
    tab.update(shell.cx, |t, cx| {
        polygloss_app::viewed::set_viewed(t, &[0, 1], true, cx)
    });
    draw(shell.cx);
    viewport.update(shell.cx, |v, cx| {
        v.set_collapsed(0, false, cx);
        v.set_expansions(0, &[[0, 20]], cx);
    });
    draw(shell.cx);

    // a.rs changes again; a new file sorts between the two, so b.rs moves
    // from index 1 to 2.
    repo.write(
        "src/a.rs",
        a_edited(0)
            .replace("fn a_100() {}", "fn a_100() { again(); }")
            .as_bytes(),
    );
    repo.write("src/aa.rs", b"fn aa() {}\n");
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::LiveChanges).as_deref() == Some("2 files changed")
    });
    refresh(&mut shell, &tab);
    assert_eq!(
        paths(&mut shell, &tab),
        ["src/a.rs", "src/aa.rs", "src/b.rs"]
    );
    let (collapsed, expansions, flags) = viewport.read_with(shell.cx, |v, _| {
        (v.collapsed(), v.expansions(), v.file_flags().to_vec())
    });
    assert_eq!(collapsed, [2], "b.rs stays collapsed at its new index");
    assert_eq!(
        expansions,
        [(0, vec![[0, 20]])],
        "a.rs keeps its revealed context"
    );
    // Viewed holds for the unchanged b.rs; a.rs changed since it was viewed.
    assert!(flags[2].viewed);
    assert!(!flags[0].viewed);
    assert!(flags[0].changed_since_viewed);
    assert_eq!(flags[1], FileFlags::default());
    // The tree shows the new list with the same flags.
    let tree = tab.read_with(shell.cx, |t, _| {
        polygloss_app::tree::file_tree(t).cloned().expect("a tree")
    });
    let (labels, tree_flags) = tree.read_with(shell.cx, |t, cx| {
        (
            t.rows(cx).into_iter().map(|r| r.label).collect::<Vec<_>>(),
            t.file_flags().to_vec(),
        )
    });
    assert!(labels.iter().any(|l| l == "aa.rs"), "{labels:?}");
    assert_eq!(tree_flags, flags);
}

#[gpui_kit::test]
fn gitignored_and_objects_changes_do_not_trigger(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, live_req(repo.path(), Since::MergeBase));
    let watcher = tab.read_with(shell.cx, |t, _| {
        live::live(t)
            .and_then(|l| l.watcher().cloned())
            .expect("a watcher")
    });

    repo.write("target/debug/app", b"binary");
    repo.write("build.log", b"compiling\n");
    repo.write(".git/objects/ab/cdef0123", b"not really an object");
    repo.write(".git/logs/HEAD", b"log line\n");
    repo.write(".git/index.lock", b"");
    std::fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
    repo.write(".git/FETCH_HEAD", b"");
    idle(&mut shell, Duration::from_millis(1500));
    assert_eq!(watcher.read_with(shell.cx, |w, _| w.batches()), 0);
    assert_eq!(recomputes(&mut shell, &tab), 0);
    assert!(banners(&mut shell, &tab).is_empty());

    // A tracked file does trigger (the same watcher is alive).
    repo.write("notes.md", b"# Notes\n\nMore.\n");
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::LiveChanges).is_some()
    });
    assert!(watcher.read_with(shell.cx, |w, _| w.batches()) >= 1);
}

#[gpui_kit::test]
fn base_moved_banner_when_head_moves_with_since_head(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, live_req(repo.path(), Since::Head));
    assert_eq!(paths(&mut shell, &tab), ["src/a.rs"]);

    // Committing moves HEAD, so the base moves (and the diff empties).
    repo.commit("commit the edit");
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::LiveChanges).is_some()
    });
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::LiveChanges).as_deref(),
        Some("Base moved · 1 file changed")
    );
    refresh(&mut shell, &tab);
    assert!(paths(&mut shell, &tab).is_empty());
}

#[gpui_kit::test]
fn agent_commit_does_not_change_merge_base_diff(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, live_req(repo.path(), Since::MergeBase));
    let shown = diff_id(&mut shell, &tab);

    // The agent commits its work: HEAD, the index and a ref change, but the
    // branch plus uncommitted work against the merge base does not.
    repo.commit("agent: edit a.rs");
    wait_until(&mut shell, "a recompute", |s| {
        recomputes(s, &tab) >= 1 && !busy(s, &tab)
    });
    idle(&mut shell, Duration::from_millis(500));
    assert!(
        banners(&mut shell, &tab).is_empty(),
        "{:?}",
        banners(&mut shell, &tab)
    );
    assert_eq!(diff_id(&mut shell, &tab), shown);
}

#[gpui_kit::test]
fn base_picker_opens_separate_review_key(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let first = shell
        .open(live_req(repo.path(), Since::MergeBase))
        .expect("open the review");
    let key = |shell: &mut Shell, tab: &Entity<ReviewTab>| {
        tab.read_with(shell.cx, |t, _| {
            (t.review_id.clone(), t.opened.review_key.clone())
        })
    };
    let (first_id, first_key) = key(&mut shell, &first);
    assert!(first_key.ends_with("#since=merge-base"), "{first_key}");

    // The picker: the two moving bases, then the log (newest first).
    shell.cx.dispatch_action(tab_actions::ChooseBase);
    draw(shell.cx);
    let picker = shell
        .cx
        .update(|_, cx| base_picker::current(cx))
        .expect("the base picker is open");
    let rows: Vec<Since> = picker.read_with(shell.cx, |p, _| {
        p.delegate().matches().iter().map(|c| c.since()).collect()
    });
    let feature = repo.oid("HEAD");
    let base = repo.oid("main");
    assert_eq!(
        rows,
        [
            Since::MergeBase,
            Since::Head,
            Since::Commit(feature.as_str().to_owned()),
            Since::Commit(base.as_str().to_owned()),
        ]
    );
    // Enter on a fixed commit opens that review in a tab of its own.
    shell.cx.update(|window, cx| {
        picker.update(cx, |p, cx| {
            p.set_selected_index(Some(IndexPath::new(3)), window, cx)
        })
    });
    shell.cx.simulate_keystrokes("enter");
    draw(shell.cx);
    let fixed = shell.active_review().expect("a review tab");
    assert_ne!(fixed, first);
    let (fixed_id, fixed_key) = key(&mut shell, &fixed);
    assert!(
        fixed_key.ends_with(&format!("#since={base}")),
        "{fixed_key}"
    );

    // HEAD, through the same entry point the toolbar and palette use.
    let task = shell.cx.update(|window, cx| {
        base_picker::choose(&first, Since::Head, window, cx).expect("a live tab")
    });
    draw(shell.cx);
    let head = futures::FutureExt::now_or_never(task)
        .expect("the open finished")
        .expect("open since HEAD");
    let (head_id, head_key) = key(&mut shell, &head);
    assert!(head_key.ends_with("#since=HEAD"), "{head_key}");
    let ids = [&first_id, &fixed_id, &head_id];
    assert!(ids[0] != ids[1] && ids[1] != ids[2] && ids[0] != ids[2]);
    assert_eq!(shell.tabs().0, 4, "Home and three reviews");
    // The first tab still shows its own base.
    assert_eq!(key(&mut shell, &first).1, first_key);
}

#[gpui_kit::test]
fn snapshot_command_pins_manual_iteration(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = shell
        .open(live_req(repo.path(), Since::MergeBase))
        .expect("open the review");
    let (review_id, head_tree, unpinned) = tab.read_with(shell.cx, |t, _| {
        (
            t.review_id.clone(),
            t.opened.head_tree.clone(),
            t.opened.iteration.is_none(),
        )
    });
    assert!(unpinned, "a live state is not pinned by opening it");

    shell.cx.dispatch_action(tab_actions::Snapshot);
    draw(shell.cx);
    let rows = shell
        .core
        .store
        .read(|c| {
            let mut stmt =
                c.prepare("SELECT seq, pinned_by FROM iterations WHERE review_id = ?1")?;
            let rows = stmt
                .query_map([&review_id], |r| {
                    Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(rows, [(1, "manual".to_owned())]);
    let seq = tab.read_with(shell.cx, |t, _| t.opened.iteration.as_ref().map(|i| i.seq));
    assert_eq!(seq, Some(1));
    let toasts = shell.main.read_with(shell.cx, |m, _| m.toasts().to_vec());
    assert!(
        toasts.iter().any(|t| t == "Snapshot saved as iteration 1"),
        "{toasts:?}"
    );
    // The state is in the repo now, kept by its snapshot ref.
    assert_eq!(
        repo.git(&[
            "rev-parse",
            &format!("refs/polygloss/snapshots/{head_tree}")
        ]),
        head_tree.as_str()
    );
}

#[gpui_kit::test]
fn compare_ref_move_shows_new_iteration_banner(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    // The working tree edit is not part of a compare.
    repo.git(&["checkout", "--", "src/a.rs"]);
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, compare_req(repo.path()));
    assert_eq!(paths(&mut shell, &tab), ["src/b.rs"]);
    let shown = diff_id(&mut shell, &tab);

    // Editing the working tree does not move a compare.
    repo.write("notes.md", b"# Notes\n\nedited\n");
    idle(&mut shell, Duration::from_millis(800));
    assert!(banners(&mut shell, &tab).is_empty());

    // A commit on the head branch moves its ref.
    repo.write("src/a.rs", a_edited(0).as_bytes());
    repo.commit("more feature work");
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::NewIteration).is_some()
    });
    assert_eq!(
        banner(&mut shell, &tab, BannerKind::NewIteration).as_deref(),
        Some("New iteration available")
    );
    assert_eq!(diff_id(&mut shell, &tab), shown, "nothing applied yet");

    refresh(&mut shell, &tab);
    assert_ne!(diff_id(&mut shell, &tab), shown);
    assert_eq!(
        paths(&mut shell, &tab),
        ["notes.md", "src/a.rs", "src/b.rs"]
    );
    let review_id = tab.read_with(shell.cx, |t, _| t.review_id.clone());
    let its = shell
        .core
        .store
        .read(|c| {
            let mut stmt = c.prepare(
                "SELECT seq, pinned_by FROM iterations WHERE review_id = ?1 ORDER BY seq",
            )?;
            let rows = stmt
                .query_map([&review_id], |r| {
                    Ok((r.get::<_, u32>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap();
    assert_eq!(its, [(1, "open".to_owned()), (2, "refresh".to_owned())]);
}

#[gpui_kit::test]
fn index_locked_retries_next_debounce(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, live_req(repo.path(), Since::MergeBase));

    // Git is writing the index: the snapshot must not copy it.
    repo.write(".git/index.lock", b"");
    repo.write("src/a.rs", a_edited(2).as_bytes());
    wait_until(&mut shell, "a blocked recompute", |s| {
        tab.read_with(s.cx, |t, _| {
            live::live(t).is_some_and(|l| l.locked_retries() >= 1)
        })
    });
    assert!(banners(&mut shell, &tab).is_empty());

    // Git finishes; the next retry sees the change.
    std::fs::remove_file(repo.path().join(".git/index.lock")).unwrap();
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::LiveChanges).is_some()
    });
    let retries = tab.read_with(shell.cx, |t, _| live::live(t).map(|l| l.locked_retries()));
    assert_eq!(retries, Some(0));
}

// ---------------------------------------------------------------- units

fn repo_info(path: &Path) -> RepoInfo {
    discover(path).expect("a repo")
}

#[test]
fn path_filter_keeps_worktree_and_git_state_drops_noise() {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let info = repo_info(repo.path());
    let scratch = repo.path().parent().unwrap().join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let live = PathFilter::new(&info, Some(repo.path()), &scratch);
    let refs = PathFilter::new(&info, None, &scratch);
    let p = |rel: &str| -> PathBuf { repo.path().join(rel) };
    let cases = [
        ("src/a.rs", Verdict::Worktree, Verdict::Drop),
        ("Cargo.lock", Verdict::Worktree, Verdict::Drop),
        (".git/HEAD", Verdict::Git, Verdict::Git),
        (".git/index", Verdict::Git, Verdict::Drop),
        (".git/refs/heads/feature", Verdict::Git, Verdict::Git),
        (".git/packed-refs", Verdict::Git, Verdict::Git),
        (".git/index.lock", Verdict::Drop, Verdict::Drop),
        (".git/refs/heads/feature.lock", Verdict::Drop, Verdict::Drop),
        (".git/objects/ab/cdef", Verdict::Drop, Verdict::Drop),
        (".git/logs/HEAD", Verdict::Drop, Verdict::Drop),
        (".git/FETCH_HEAD", Verdict::Drop, Verdict::Drop),
        ("vendor/nested/.git/HEAD", Verdict::Drop, Verdict::Drop),
    ];
    for (rel, in_live, in_refs) in cases {
        assert_eq!(live.classify(&p(rel)), in_live, "live: {rel}");
        assert_eq!(refs.classify(&p(rel)), in_refs, "refs: {rel}");
    }
    assert_eq!(live.classify(&scratch.join("objects/ab")), Verdict::Drop);
    // One root: the git dir is inside the worktree.
    assert_eq!(live.roots(), [repo.path().to_path_buf()]);
    assert_eq!(refs.roots(), [repo.path().join(".git")]);
}

#[test]
fn path_filter_watches_a_linked_worktree_and_its_common_dir() {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let wt = repo.add_worktree("other");
    let info = repo_info(&wt);
    let scratch = repo.path().parent().unwrap().join("scratch");
    let filter = PathFilter::new(&info, Some(&wt), &scratch);
    let common = repo.path().join(".git");
    assert_eq!(filter.roots(), [common.clone(), wt.clone()]);
    assert_eq!(filter.classify(&wt.join("src/a.rs")), Verdict::Worktree);
    assert_eq!(
        filter.classify(&common.join("worktrees/other/HEAD")),
        Verdict::Git
    );
    assert_eq!(
        filter.classify(&common.join("worktrees/other/index")),
        Verdict::Git
    );
    assert_eq!(
        filter.classify(&common.join("refs/heads/other")),
        Verdict::Git
    );
    // The main worktree's own state is not this review's.
    assert_eq!(filter.classify(&common.join("HEAD")), Verdict::Drop);
    assert_eq!(filter.classify(&common.join("index")), Verdict::Drop);
    assert_eq!(
        filter.classify(&repo.path().join("src/a.rs")),
        Verdict::Drop
    );
}

#[test]
fn drop_ignored_uses_check_ignore() {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let p = |rel: &str| repo.path().join(rel);
    let kept = polygloss_app::live::watcher::drop_ignored(
        repo.path(),
        vec![
            p("src/a.rs"),
            p("target/x"),
            p("a b.log"),
            p("notes.md"),
            repo.path().to_path_buf(),
        ],
    );
    assert_eq!(
        kept,
        [repo.path().to_path_buf(), p("src/a.rs"), p("notes.md")]
    );
}

#[test]
fn recompute_counts_changed_files_and_reads_sources_from_keys() {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let core = polygloss_core::review::Core::open_default().unwrap();
    let opened = core.open(&live_req(repo.path(), Since::MergeBase)).unwrap();
    assert_eq!(
        recompute::source_of(&opened),
        Some(Source::Live {
            since: Since::MergeBase
        })
    );
    let target = Target::of(&opened).expect("a live target");
    let shown = Shown::of(&opened);
    assert_eq!(
        recompute::recompute(&core.snapshots, &target, &shown).unwrap(),
        recompute::Outcome::Same
    );
    repo.write("src/a.rs", a_edited(1).as_bytes());
    repo.write("src/c.rs", b"fn c() {}\n");
    let recompute::Outcome::Newer(newer) =
        recompute::recompute(&core.snapshots, &target, &shown).unwrap()
    else {
        panic!("a newer state");
    };
    assert_eq!((newer.files_changed, newer.base_moved), (2, false));
    assert_eq!(
        recompute::banner_text(target.kind, &newer),
        "2 files changed"
    );

    let compare = core.open(&compare_req(repo.path())).unwrap();
    assert_eq!(
        recompute::source_of(&compare),
        Some(Source::Compare {
            base: "refs/heads/main".into(),
            head: "refs/heads/feature".into(),
            mode: CompareMode::ThreeDot,
        })
    );
}

#[gpui_kit::test]
fn live_refresh_keeps_the_line_below_the_header(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{IntoElement as _, Styled as _};
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, live_req(repo.path(), Since::MergeBase));
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    // On cards, below a 72 pt prelude (the header card's stand-in).
    let prelude: polygloss_viewport::RenderBlock =
        std::rc::Rc::new(|_, _| gpui_kit::div().h(gpui_kit::px(72.)).into_any_element());
    viewport.update(shell.cx, |v, cx| v.set_prelude(Some(prelude), cx));
    draw(shell.cx);
    // What is painted right below the pinned header, and the line saved for
    // it.
    let below_header = |shell: &mut Shell| {
        viewport.read_with(shell.cx, |v, _| {
            let header = v.document().metrics().header_height;
            let d = v.debug();
            let i = d.row_bounds.iter().position(|(y, _)| *y == header).unwrap();
            (v.document().top_line(), d.visible_rows[i].clone())
        })
    };
    scroll_to_line(&mut shell, &tab, 0, 60);
    let (line, row) = below_header(&mut shell);
    assert_eq!(line, Some((0, Side::New, 60)));
    assert!(row.ends_with("fn a_60() { edited(); }"), "{row}");

    // Five lines are added on top: the same line, now 65, is still right
    // below the header.
    repo.write("src/a.rs", a_edited(5).as_bytes());
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::LiveChanges).is_some()
    });
    refresh(&mut shell, &tab);
    let (line, row) = below_header(&mut shell);
    assert_eq!(line, Some((0, Side::New, 65)));
    assert!(row.ends_with("fn a_60() { edited(); }"), "{row}");
    assert_eq!(
        viewport.read_with(shell.cx, |v, _| v.document().prelude_height()),
        Some(72.0)
    );
}

#[gpui_kit::test]
fn live_refresh_at_the_top_of_the_document_stays_there(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{IntoElement as _, Styled as _};
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    let tab = open_watched(&mut shell, live_req(repo.path(), Since::MergeBase));
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    // A 72 pt prelude (the header card's stand-in) above the first card.
    let prelude: polygloss_viewport::RenderBlock =
        std::rc::Rc::new(|_, _| gpui_kit::div().h(gpui_kit::px(72.)).into_any_element());
    viewport.update(shell.cx, |v, cx| v.set_prelude(Some(prelude), cx));
    draw(shell.cx);
    let scroll_top =
        |shell: &mut Shell| viewport.read_with(shell.cx, |v, _| v.document().scroll_top());
    assert_eq!(scroll_top(&mut shell), 0.0);

    // A file sorting before the first one appears: the review stays at the
    // top (the prelude, then the new first card), not at the lead of the
    // file that was first.
    repo.write("notes.md", b"# Notes\n\nMore.\n");
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::LiveChanges).is_some()
    });
    refresh(&mut shell, &tab);
    assert_eq!(
        paths(&mut shell, &tab),
        ["notes.md", "src/a.rs", "src/b.rs"]
    );
    assert_eq!(scroll_top(&mut shell), 0.0);

    // 30 pt into the prelude, the first file goes away: still 30 pt into
    // the prelude, not at the next card's header.
    viewport.update(shell.cx, |v, cx| v.scroll_by(30.0, cx));
    draw(shell.cx);
    assert_eq!(scroll_top(&mut shell), 30.0);
    repo.write("notes.md", b"# Notes\n");
    wait_until(&mut shell, "the banner", |s| {
        banner(s, &tab, BannerKind::LiveChanges).is_some()
    });
    refresh(&mut shell, &tab);
    assert_eq!(paths(&mut shell, &tab), ["src/a.rs", "src/b.rs"]);
    assert_eq!(scroll_top(&mut shell), 30.0);
}

/// Modified files at `paths`, in order.
fn modified(paths: &[&str]) -> Vec<FileChange> {
    let zero = Oid::zero(ObjectFormat::Sha1);
    paths
        .iter()
        .enumerate()
        .map(|(i, p)| FileChange {
            idx: i as u32,
            status: FileStatus::Modified,
            old_path: Some(GitPath::from_bytes(p.as_bytes())),
            new_path: Some(GitPath::from_bytes(p.as_bytes())),
            old_mode: None,
            new_mode: None,
            old_blob: zero.clone(),
            new_blob: zero.clone(),
            similarity: None,
            kind: FileKind::Text,
            generated: false,
            generated_attr: GeneratedAttr::Unspecified,
        })
        .collect()
}

/// Anchors in leads and headers map without line mapping: no blob is read.
struct NoBlobs;

impl DiffProvider for NoBlobs {
    fn object_format(&self) -> ObjectFormat {
        ObjectFormat::Sha1
    }
    fn files(&self) -> Arc<Vec<FileChange>> {
        Arc::default()
    }
    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        anyhow::bail!("blob {oid} read")
    }
    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64> {
        anyhow::bail!("blob {oid} sized")
    }
}

#[test]
fn refresh_plan_keeps_the_top_of_the_document_and_maps_leads() {
    let at = |file_idx, row, offset_px| ScrollAnchor {
        file_idx,
        row,
        offset_px,
    };
    let mapped = |anchor, old: &[&str], new: &[&str]| {
        let kept = live_refresh::Kept {
            anchor,
            collapsed: Vec::new(),
            expansions: Vec::new(),
            flags: vec![FileFlags::default(); old.len()],
        };
        live_refresh::plan(&kept, &modified(old), &modified(new), &NoBlobs, &NoBlobs).anchor
    };
    // 30 pt into the prelude stays there when a file sorting first appears
    // and when the first file goes away.
    let top = at(0, RowKey::Lead, 30.0);
    assert_eq!(mapped(top, &["b", "c"], &["a", "b", "c"]), top);
    assert_eq!(mapped(top, &["a", "b", "c"], &["b", "c"]), top);
    // Another file's lead (the canvas above its card) follows its file.
    assert_eq!(
        mapped(at(1, RowKey::Lead, 5.0), &["b", "c"], &["a", "b", "c"]),
        at(2, RowKey::Lead, 5.0)
    );
    // Its file gone, a lead becomes the next file's lead and a header the
    // next file's header.
    assert_eq!(
        mapped(at(1, RowKey::Lead, 5.0), &["a", "b", "c"], &["a", "c"]),
        at(1, RowKey::Lead, 0.0)
    );
    assert_eq!(
        mapped(at(1, RowKey::Header, 5.0), &["a", "b", "c"], &["a", "c"]),
        at(1, RowKey::Header, 0.0)
    );
}

#[gpui_kit::test]
fn live_toolbar_shows_branch_and_live_pill_opening_the_base_picker(
    cx: &mut gpui_kit::TestAppContext,
) {
    use crate::shell::{bounds, click};
    use crate::toolbar::shows;

    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    shell
        .open(live_req(repo.path(), Since::MergeBase))
        .expect("open the review");
    assert!(shows(shell.cx, "branch-pill-label", "feature"));
    assert!(shows(shell.cx, "live-base-label", "Live \u{b7} merge base"));
    assert!(bounds(shell.cx, "branch-pill").right() <= bounds(shell.cx, "live-base").left());
    // Snapshot stays in the toolbar until the header card takes it (T6.13).
    assert!(crate::shell::painted(shell.cx, "live-snapshot").is_some());

    assert!(shell.cx.update(|_, cx| base_picker::current(cx)).is_none());
    click(shell.cx, "live-base");
    assert!(
        shell.cx.update(|_, cx| base_picker::current(cx)).is_some(),
        "the Live pill opens the base picker"
    );
}
