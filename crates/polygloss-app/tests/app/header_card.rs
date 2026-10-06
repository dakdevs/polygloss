//! The header card (T6.13, design §11.6, ADR-0027): the prelude above the
//! first file card, per review kind. A commit's avatar (an initial on a
//! color from FNV-1a of the lowercased email), subject, author, relative
//! time and SHA; a compare's label or refs, its commit count and a "Show
//! commits" list; a live review's branch, base and Snapshot. Stats sit in
//! the right cluster before the trailing item; the card scrolls with the
//! diff and keeps the top of the document while it loads or grows.

use std::path::Path;

use gpui_kit::{Entity, Hsla, TestAppContext, rgb};
use polygloss_app::review_tab::{ReviewTab, header};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_highlight::Appearance;

use crate::shell::{Shell, bounds, click, commit_req, compare_req, draw, painted, start};
use crate::support::{FixtureRepo, Sandbox};
use crate::toolbar::shows;

/// The fixture's commit dates start here (Unix seconds); commit `n` is `n`
/// minutes later.
const EPOCH: i64 = 1_767_225_600;
const HOUR_MS: i64 = 3_600_000;

/// `main`: `a.txt` ("one") and `b.txt` ("x", "y"). Then, by Ada Lovelace
/// (`Ada@Example.com`), "Parse configs": 120 lines added to `a.txt` and
/// `y` removed from `b.txt`, committed at `EPOCH + 60`.
fn authored_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    repo.write("b.txt", b"x\ny\n");
    repo.commit("base");
    let more: String = (0..120).map(|i| format!("line {i}\n")).collect();
    repo.write("a.txt", format!("one\n{more}").as_bytes());
    repo.write("b.txt", b"x\n");
    repo.git(&["add", "-A"]);
    repo.git(&[
        "commit",
        "-q",
        "--author",
        "Ada Lovelace <Ada@Example.com>",
        "-m",
        "Parse configs",
    ]);
    repo
}

/// Pins the card's clock at `ms` (Unix ms) and redraws.
fn pin_clock(shell: &mut Shell, ms: i64) {
    shell.cx.update(|_, cx| header::set_clock(move || ms, cx));
    draw(shell.cx);
}

fn short(repo: &FixtureRepo, rev: &str) -> String {
    repo.git(&["rev-parse", "--short=7", rev])
}

fn scroll_top(shell: &mut Shell, tab: &Entity<ReviewTab>) -> f64 {
    tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).document().scroll_top()
    })
}

#[test]
fn avatar_color_is_fnv1a_of_the_lowercased_email() {
    // Eight players, told apart by hue.
    let players: Vec<Hsla> = (0..8)
        .map(|i| gpui_kit::hsla(i as f32 / 8.0, 0.5, 0.5, 1.0))
        .collect();
    // FNV-1a 32 of "ada@example.com" is 0xbef5cfd2 (≡ 2 mod 8); of
    // "fixture@polygloss.invalid" 0x525790a7 (≡ 7).
    assert_eq!(
        header::avatar_color("Ada@Example.com", &players, Appearance::Light),
        players[2]
    );
    // An ASCII case flip changes bit 5 of a byte, which never reaches the
    // low three bits of FNV-1a; `Σ` → `σ` (CE A3 → CF 83) does: FNV-1a 32 of
    // "Σofia@example.gr" is 0x258de3c6 (≡ 6), of "σofia@example.gr"
    // 0x332513ad (≡ 5).
    assert_eq!(
        header::avatar_color("Σofia@example.gr", &players, Appearance::Dark),
        players[5],
        "hashed lowercased"
    );
    assert_eq!(
        header::avatar_color("fixture@polygloss.invalid", &players, Appearance::Light),
        players[7]
    );
    // Fewer than eight players (Pierre has one): fixed hues per appearance
    // (research: redesign reference).
    let one = &players[..1];
    assert_eq!(
        header::avatar_color("ada@example.com", one, Appearance::Light),
        Hsla::from(rgb(0x3f8f62))
    );
    assert_eq!(
        header::avatar_color("ada@example.com", one, Appearance::Dark),
        Hsla::from(rgb(0x6cc08f))
    );
    assert_eq!(
        header::avatar_color("fixture@polygloss.invalid", &[], Appearance::Light),
        Hsla::from(rgb(0x6b7280))
    );

    assert_eq!(header::initial("ada Lovelace"), "A");
    assert_eq!(header::initial("  émile"), "É");
    assert_eq!(header::initial("_x9"), "X");
    assert_eq!(header::initial("42 Bot"), "4");
    assert_eq!(header::initial("--"), "?");
}

#[gpui_kit::test]
fn commit_header_shows_avatar_subject_author_and_sha(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = authored_repo();
    let mut shell = start(cx);
    shell.open(commit_req(repo.path(), "HEAD")).unwrap();
    pin_clock(&mut shell, (EPOCH + 60) * 1000 + 2 * HOUR_MS);

    let card = bounds(shell.cx, "header-card");
    for part in ["header-avatar", "header-sha", "header-stats"] {
        let b = bounds(shell.cx, part);
        assert!(card.contains(&b.center()), "{part} is in the card");
    }
    assert!(shows(shell.cx, "header-avatar", "A"));
    assert!(shows(shell.cx, "header-title", "Parse configs"));
    assert!(shows(
        shell.cx,
        "header-byline",
        "Ada Lovelace committed 2h ago"
    ));
    assert!(shows(shell.cx, "header-sha", &short(&repo, "HEAD")));
    // The avatar leads, the SHA ends the row.
    assert!(bounds(shell.cx, "header-avatar").right() <= bounds(shell.cx, "header-title").left());
    assert!(bounds(shell.cx, "header-sha").right() <= card.right());
    // A commit has no commit list and no Snapshot.
    assert!(painted(shell.cx, "header-commits-toggle").is_none());
    assert!(painted(shell.cx, "live-snapshot").is_none());
}

#[gpui_kit::test]
fn relative_time_uses_the_app_clock(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = authored_repo();
    let mut shell = start(cx);
    shell.open(commit_req(repo.path(), "HEAD")).unwrap();
    let committed = (EPOCH + 60) * 1000;
    pin_clock(&mut shell, committed + 30_000);
    assert!(shows(
        shell.cx,
        "header-byline",
        "Ada Lovelace committed just now"
    ));
    pin_clock(&mut shell, committed + 3 * 24 * HOUR_MS);
    assert!(shows(
        shell.cx,
        "header-byline",
        "Ada Lovelace committed 3d ago"
    ));
}

#[gpui_kit::test]
fn header_stats_sit_before_the_sha(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = authored_repo();
    let mut shell = start(cx);
    shell.open(commit_req(repo.path(), "HEAD")).unwrap();
    // `git diff --numstat HEAD~ HEAD`: a.txt 120 0, b.txt 0 1.
    assert!(shows(shell.cx, "header-stats", "2 files · +120 −1"));
    let stats = bounds(shell.cx, "header-stats");
    let sha = bounds(shell.cx, "header-sha");
    assert!(stats.right() <= sha.left(), "{stats:?} before {sha:?}");
    assert!(
        stats.top() < sha.bottom() && sha.top() < stats.bottom(),
        "on one row"
    );
    assert!(
        bounds(shell.cx, "header-title").right() <= stats.left(),
        "the right cluster"
    );
    // Nothing categorized: no breakdown to point at.
    let line = "Without categorized files: 2 files · +120 −1";
    assert_eq!(
        crate::categories::tooltip_lines(&mut shell, "header-stats: 2 files · +120 −1", &[line]),
        [false]
    );
}

#[gpui_kit::test]
fn fresh_open_shows_the_header_card(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = authored_repo();
    let mut shell = start(cx);
    let tab = shell.open(commit_req(repo.path(), "HEAD")).unwrap();
    let card = bounds(shell.cx, "header-card");
    let viewport = bounds(shell.cx, "viewport-pane");
    assert_eq!(card.top(), viewport.top(), "right under the banner strip");
    assert_eq!(scroll_top(&mut shell, &tab), 0.0);
    // The first file card follows it, below the card gap.
    let prelude = tab
        .read_with(shell.cx, |t, cx| {
            t.viewport.read(cx).document().prelude_height()
        })
        .expect("the card is the prelude");
    assert!(prelude > 40.0, "a card's height: {prelude}");
}

#[gpui_kit::test]
fn header_card_scrolls_with_the_diff(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = authored_repo();
    let mut shell = start(cx);
    let tab = shell.open(commit_req(repo.path(), "HEAD")).unwrap();
    let before = bounds(shell.cx, "header-card");
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| v.scroll_by(25.0, cx));
    draw(shell.cx);
    let after = bounds(shell.cx, "header-card");
    assert_eq!(after.top(), before.top() - gpui_kit::px(25.));
    assert_eq!(scroll_top(&mut shell, &tab), 25.0);
    // Scrolled past it, the card is gone from the frame.
    viewport.update(shell.cx, |v, cx| v.scroll_by(800.0, cx));
    draw(shell.cx);
    assert!(painted(shell.cx, "header-card").is_none());
}

/// `main` with `a.txt`; branch `feature` (checked out) with `n` commits
/// "step 1" … "step n", each by Ada Lovelace or, every third, Grace Hopper
/// (`grace@example.org`). Commit `step i` is fixture commit `i + 1`.
fn commits_repo(n: usize) -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"0\n");
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    for i in 1..=n {
        repo.write("a.txt", format!("{i}\n").as_bytes());
        repo.commit(&format!("step {i}"));
        let author = if i % 3 == 0 {
            "Grace Hopper <grace@example.org>"
        } else {
            "Ada Lovelace <ada@example.com>"
        };
        repo.git(&["commit", "-q", "--amend", "--no-edit", "--author", author]);
    }
    repo
}

fn compare(repo: &Path, mode: CompareMode, label: Option<&str>) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Compare {
            base: "refs/heads/main".into(),
            head: "refs/heads/feature".into(),
            mode,
        },
        label: label.map(str::to_owned),
        pin: None,
        actor: Actor::human(),
    }
}

#[gpui_kit::test]
fn compare_header_counts_and_lists_commits(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = commits_repo(52);
    let mut shell = start(cx);
    shell
        .open(compare(repo.path(), CompareMode::ThreeDot, None))
        .unwrap();
    // `step 52` is fixture commit 53; five minutes later.
    pin_clock(&mut shell, (EPOCH + 53 * 60) * 1000 + 5 * 60_000);

    assert!(shows(shell.cx, "header-title", "main\u{2026}feature"));
    assert!(shows(
        shell.cx,
        "header-byline",
        "52 commits · Ada Lovelace committed 5m ago"
    ));
    assert!(shows(shell.cx, "header-stats", "1 file · +1 −1"));
    // No SHA and no Snapshot in a compare's card; no list until asked.
    assert!(painted(shell.cx, "header-sha").is_none());
    assert!(painted(shell.cx, "live-snapshot").is_none());
    assert!(painted(shell.cx, "header-commit-0").is_none());

    click(shell.cx, "header-commits-toggle");
    // Newest first, 50 of them, then the rest as a count.
    assert!(shows(shell.cx, "header-commit", "step 52"));
    assert!(shows(shell.cx, "header-commit", "step 3"));
    assert!(!shows(shell.cx, "header-commit", "step 2"), "past the 50");
    assert!(shows(shell.cx, "header-commits-more", "and 2 more"));
    let first = bounds(shell.cx, "header-commit-0");
    let second = bounds(shell.cx, "header-commit-1");
    assert!(first.bottom() <= second.top(), "newest on top");
    // Hidden again.
    click(shell.cx, "header-commits-toggle");
    assert!(painted(shell.cx, "header-commit-0").is_none());

    // A label names the review instead of its refs.
    shell
        .open(compare(
            repo.path(),
            CompareMode::Direct,
            Some("Speed up parsing"),
        ))
        .unwrap();
    assert!(shows(shell.cx, "header-title", "Speed up parsing"));
}

/// `main`: `a.txt`. Branch `feature` (checked out): `b.txt` committed, and
/// `a.txt` edited in the working tree.
fn live_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"one\n");
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("b.txt", b"feature\n");
    repo.commit("feature work");
    repo.write("a.txt", b"one\ntwo\n");
    repo
}

fn live(repo: &Path, since: Since) -> OpenRequest {
    OpenRequest {
        worktree: repo.to_path_buf(),
        source: Source::Live { since },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

fn toasts(shell: &mut Shell) -> Vec<String> {
    shell.main.read_with(shell.cx, |m, _| {
        m.toasts().iter().map(|t| t.to_string()).collect()
    })
}

#[gpui_kit::test]
fn live_header_shows_branch_base_and_snapshot(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    shell.open(live(repo.path(), Since::MergeBase)).unwrap();
    assert!(shows(shell.cx, "header-title", "Changes on feature"));
    // The merge base with `main` is `main` itself here.
    let base = short(&repo, "main");
    assert!(shows(
        shell.cx,
        "header-byline",
        &format!("vs main ({base})")
    ));
    // Merge base: both the branch's commit and the edit.
    assert!(shows(shell.cx, "header-stats", "2 files · +2 −0"));
    let stats = bounds(shell.cx, "header-stats");
    let snapshot = bounds(shell.cx, "live-snapshot");
    assert!(stats.right() <= snapshot.left(), "Snapshot ends the row");
    assert!(painted(shell.cx, "header-avatar").is_none());

    // Snapshot pins the state, then is disabled: a second click does
    // nothing.
    click(shell.cx, "live-snapshot");
    let saved = "Snapshot saved as iteration 1";
    assert_eq!(toasts(&mut shell).iter().filter(|t| *t == saved).count(), 1);
    click(shell.cx, "live-snapshot");
    assert_eq!(
        toasts(&mut shell).iter().filter(|t| *t == saved).count(),
        1,
        "disabled once pinned: {:?}",
        toasts(&mut shell)
    );
}

#[gpui_kit::test]
fn live_header_says_uncommitted_only_against_head(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    shell.open(live(repo.path(), Since::MergeBase)).unwrap();
    assert!(shows(shell.cx, "header-title", "Changes on feature"));
    shell.open(live(repo.path(), Since::Head)).unwrap();
    assert!(shows(
        shell.cx,
        "header-title",
        "Uncommitted changes on feature"
    ));
    let head = short(&repo, "HEAD");
    assert!(shows(
        shell.cx,
        "header-byline",
        &format!("vs feature ({head})")
    ));
    // Against HEAD only the edit: `a.txt` +1.
    assert!(shows(shell.cx, "header-stats", "1 file · +1 −0"));
}

#[gpui_kit::test]
fn snapshot_left_the_toolbar(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = live_repo();
    let mut shell = start(cx);
    shell.open(live(repo.path(), Since::MergeBase)).unwrap();
    let snapshot = bounds(shell.cx, "live-snapshot");
    let toolbar = bounds(shell.cx, "review-toolbar");
    let card = bounds(shell.cx, "header-card");
    assert!(snapshot.top() >= toolbar.bottom(), "not in the toolbar");
    assert!(card.contains(&snapshot.center()), "in the header card");
    // The Live pill stays in the toolbar.
    assert!(toolbar.contains(&bounds(shell.cx, "live-base").center()));
}

#[gpui_kit::test]
fn late_header_card_and_show_commits_keep_scroll_top_0(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = commits_repo(30);
    let mut shell = start(cx);
    let tab = shell
        .open(compare(repo.path(), CompareMode::ThreeDot, None))
        .unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    // Frames without the card, then it lands (as after a slow `git log`).
    viewport.update(shell.cx, |v, cx| v.set_prelude(None, cx));
    draw(shell.cx);
    assert!(painted(shell.cx, "header-card").is_none());
    let top = bounds(shell.cx, "viewport-pane").top();
    tab.update(shell.cx, header::reload);
    draw(shell.cx);
    assert_eq!(scroll_top(&mut shell, &tab), 0.0);
    assert_eq!(bounds(shell.cx, "header-card").top(), top);

    // 30 commits open below the subject; the view stays at the top.
    click(shell.cx, "header-commits-toggle");
    assert_eq!(scroll_top(&mut shell, &tab), 0.0);
    assert_eq!(bounds(shell.cx, "header-card").top(), top);
    let title = bounds(shell.cx, "header-title");
    let row = bounds(shell.cx, "header-commit-0");
    assert!(row.top() >= title.bottom(), "{row:?} below {title:?}");
    assert!(shows(shell.cx, "header-commit", "step 30"));
    assert!(painted(shell.cx, "header-commits-more").is_none(), "all 30");
}

#[gpui_kit::test]
fn a_bare_repo_commit_review_shows_the_header_card(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = authored_repo();
    // Beside the worktree, in no repository: git run in the bare repo's
    // parent finds none.
    let bare = repo.path().parent().unwrap().join("bare.git");
    repo.git(&["clone", "-q", "--bare", ".", bare.to_str().unwrap()]);
    let mut shell = start(cx);
    shell.open(commit_req(&bare, "HEAD")).unwrap();
    assert!(painted(shell.cx, "header-card").is_some());
    assert!(shows(shell.cx, "header-title", "Parse configs"));
    assert!(shows(shell.cx, "header-sha", &short(&repo, "HEAD")));
}

#[gpui_kit::test]
fn a_reload_off_screen_measures_the_card_again(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    // Three commits, the last adding 400 lines: a diff to scroll in.
    let repo = commits_repo(2);
    let long: String = (0..400).map(|i| format!("line {i}\n")).collect();
    repo.write("long.txt", long.as_bytes());
    repo.commit("long");
    let mut shell = start(cx);
    let tab = shell
        .open(compare(repo.path(), CompareMode::ThreeDot, None))
        .unwrap();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    let prelude = |shell: &mut Shell| {
        viewport
            .read_with(shell.cx, |v, _| v.document().prelude_height())
            .expect("the card is the prelude")
    };
    click(shell.cx, "header-commits-toggle");
    assert!(shows(shell.cx, "header-commit", "long"));
    let three = prelude(&mut shell);

    // Scrolled past the card, a refresh brings three more commits into the
    // open list.
    viewport.update(shell.cx, |v, cx| v.scroll_by(600.0, cx));
    draw(shell.cx);
    assert!(painted(shell.cx, "header-card").is_none());
    for i in 0..3 {
        repo.write("a.txt", format!("more {i}\n").as_bytes());
        repo.commit(&format!("more {i}"));
    }
    tab.update_in(shell.cx, polygloss_app::live::refresh_tab);
    draw(shell.cx);
    assert!(
        painted(shell.cx, "header-card").is_none(),
        "still off screen"
    );
    let off_screen = prelude(&mut shell);

    // On screen the card is laid out every frame: its height there is the
    // truth.
    viewport.update(shell.cx, |v, cx| v.scroll_by(-10_000.0, cx));
    draw(shell.cx);
    assert!(shows(shell.cx, "header-commit", "more 2"));
    let on_screen = prelude(&mut shell);
    assert!(
        on_screen > three,
        "{on_screen} > {three}: six rows, not three"
    );
    assert_eq!(off_screen, on_screen, "measured again off screen");
}

#[gpui_kit::test]
fn an_open_commit_list_goes_with_the_last_commit(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    // `main` one commit past the fork; `feature` two.
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.txt", b"0\n");
    let fork = repo.commit("base");
    repo.branch("feature");
    repo.write("b.txt", b"main\n");
    repo.commit("main 1");
    repo.checkout("feature");
    for i in 1..=2 {
        repo.write("a.txt", format!("{i}\n").as_bytes());
        repo.commit(&format!("step {i}"));
    }
    let mut shell = start(cx);
    // `main..feature`: both of `feature`'s commits.
    let tab = shell
        .open(compare(repo.path(), CompareMode::Direct, None))
        .unwrap();
    click(shell.cx, "header-commits-toggle");
    assert!(shows(shell.cx, "header-commit", "step 2"));
    assert!(painted(shell.cx, "header-commits").is_some());

    // `feature` back at the fork, behind `main`: no commit of its own, and
    // still a diff (`b.txt` removed).
    repo.git(&["update-ref", "refs/heads/feature", &fork.to_string()]);
    tab.update_in(shell.cx, polygloss_app::live::refresh_tab);
    draw(shell.cx);
    assert!(shows(shell.cx, "header-stats", "1 file · +0 −1"));
    assert!(painted(shell.cx, "header-commits-toggle").is_none());
    assert!(
        painted(shell.cx, "header-commits").is_none(),
        "no empty list left open"
    );
}

/// T6.15: the header card's stats and the tree's footer count the
/// uncategorized files only; categorized ones go to the chips.
#[gpui_kit::test]
fn footer_and_header_exclude_categorized_files(cx: &mut TestAppContext) {
    use crate::categories::{TOTALS_CATEGORIZED, numstat, totals_repo};
    let _sb = Sandbox::isolate();
    let repo = totals_repo(None);
    let (files, added, removed) = numstat(&repo, |p| !TOTALS_CATEGORIZED.contains(&p));
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    crate::tree::settle_counts(&mut shell, &tab, 5);
    draw(shell.cx);
    assert_eq!(files, 2);
    assert!(added < 1000 && removed < 1000, "the oracle does not group");
    assert!(shows(
        shell.cx,
        "tree-footer",
        &format!("Total: +{added} −{removed}")
    ));
    assert!(shows(
        shell.cx,
        "header-stats",
        &format!("{files} files · +{added} −{removed}")
    ));
    // The chips sit after the stats, before the trailing item (none for a
    // compare).
    let stats = bounds(
        shell.cx,
        &format!("header-stats: {files} files · +{added} −{removed}"),
    );
    let chips = bounds(shell.cx, "header-chips: 1 test · 2 generated");
    assert!(stats.right() <= chips.left());
}

/// `base` (a README), then "init" by Ada Lovelace adding `notes.txt`, 120
/// lines: one one-sided card with 3-digit numbers.
fn added_file_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("README.md", b"# notes\n");
    repo.commit("base");
    let lines: String = (1..=120).map(|i| format!("note {i}\n")).collect();
    repo.write("notes.txt", lines.as_bytes());
    repo.git(&["add", "-A"]);
    repo.git(&[
        "commit",
        "-q",
        "--author",
        "Ada Lovelace <ada@example.com>",
        "-m",
        "init",
    ]);
    repo
}

/// T7.7, ADR-0031 C3: the avatar and its gap put the title's box on the
/// code column of a one-sided card with 3-digit numbers (relational: both
/// measured).
#[gpui_kit::test]
fn header_card_title_lands_on_the_code_column(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = added_file_repo();
    let mut shell = start(cx);
    let tab = shell.open(commit_req(repo.path(), "HEAD")).unwrap();
    draw(shell.cx);
    let title = bounds(shell.cx, "header-title");
    let code = tab.read_with(shell.cx, |t, cx| {
        let v = t.viewport.read(cx);
        let f = v.display_order()[0];
        let card = v.card_bounds(f).expect("the file card is painted");
        // `code_x` is from the card's inner left edge, inside its border.
        card.left() + gpui_kit::px(1.) + v.code_x(f, Side::New).expect("the file")
    });
    let off = (title.left() - code).abs();
    assert!(
        off <= gpui_kit::px(0.5),
        "the title's box at {:?}, the code column at {code:?}",
        title.left()
    );
}

/// T7.7, ADR-0031 C3 and R13: `CARD_Y`, a 20 pt title line, an 18 pt
/// byline and `CARD_Y`, inside two 1 pt borders, for a commit and for a
/// compare (whose byline holds "Show commits"); the card is the viewport's
/// prelude, placed where the E2E reference test reads it.
#[gpui_kit::test]
fn header_card_is_56_tall(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = authored_repo();
    let mut shell = start(cx);
    let tab = shell.open(commit_req(repo.path(), "HEAD")).unwrap();
    draw(shell.cx);
    let card = bounds(shell.cx, "header-card");
    assert_eq!(card.size.height, gpui_kit::px(8. + 20. + 18. + 8. + 2.));
    // The viewport's debug frame places the prelude at the card's bounds
    // (viewport-relative; the first file card gives the origin).
    let (prelude, origin) = tab.read_with(shell.cx, |t, cx| {
        let v = t.viewport.read(cx);
        let d = v.debug();
        let f = v.display_order()[0];
        let file_card = v.card_bounds(f).expect("the file card is painted");
        let at = d
            .cards
            .iter()
            .find(|c| c.file_idx == f)
            .expect("in the frame");
        (
            d.prelude.expect("the prelude is placed"),
            (
                file_card.left().as_f32() - at.bounds.0,
                file_card.top().as_f32() - at.bounds.1,
            ),
        )
    });
    assert_eq!(
        (
            prelude.0 + origin.0,
            prelude.1 + origin.1,
            prelude.2,
            prelude.3
        ),
        (
            card.left().as_f32(),
            card.top().as_f32(),
            card.size.width.as_f32(),
            card.size.height.as_f32()
        )
    );

    let commits = commits_repo(3);
    shell
        .open(compare(commits.path(), CompareMode::ThreeDot, None))
        .unwrap();
    assert!(painted(shell.cx, "header-commits-toggle").is_some());
    assert_eq!(
        bounds(shell.cx, "header-card").size.height,
        gpui_kit::px(56.)
    );
}

/// T7.7: the commit list's rows are list rows (28, touching) with 20 pt
/// avatars.
#[gpui_kit::test]
fn commit_rows_follow_the_ladder(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = commits_repo(3);
    let mut shell = start(cx);
    shell
        .open(compare(repo.path(), CompareMode::ThreeDot, None))
        .unwrap();
    click(shell.cx, "header-commits-toggle");
    let rows = [0, 1, 2].map(|i| bounds(shell.cx, &format!("header-commit-{i}")));
    for row in rows {
        assert_eq!(row.size.height, gpui_kit::px(28.), "{row:?}");
    }
    assert_eq!(rows[1].top(), rows[0].bottom(), "rows touch");
    assert_eq!(rows[2].top(), rows[1].bottom(), "rows touch");
    let avatar = bounds(shell.cx, "header-commit-avatar-0");
    assert_eq!(
        (avatar.size.width, avatar.size.height),
        (gpui_kit::px(20.), gpui_kit::px(20.))
    );
    // Centred on its row.
    assert_eq!(avatar.center().y, rows[0].center().y);
}
