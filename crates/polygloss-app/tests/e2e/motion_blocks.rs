//! The block and prelude motions in the full app (T7.14, ADR-0030 M7, M8,
//! M15), drawn by Metal: each one's filmstrip, Full over Reduced
//! (`support::filmstrip`), cropped to the surface that moves.
//!
//! - `thread-chip`: a pointer opens an agent's note from its chip (M7):
//!   the card replaces the chip and is revealed from the block's top edge,
//!   the comment below riding; Reduced fades the card in at its height.
//! - `thread-chip-collapse`: its Collapse button swaps the chip in at once
//!   and closes the vacated height (M7); Reduced snaps.
//! - `commits`: "Show commits" (M8) reveals the list, the file card below
//!   riding; Reduced fades only the list in (the card's top part never
//!   fades: checked).
//! - `commits-hide`: "Hide commits" closes the card over the list, which
//!   stays painted under the curtain (its commit frame matches the open
//!   card's list within the screenshot budget: checked); Reduced fades the
//!   list out in the held frame, then the height snaps.
//! - `snapshot`: the Snapshot button's camera becomes a check scaled from
//!   0.9 (M15); Reduced fades it in unscaled.
//!
//! The harness names these tests `motion_blocks::<test>`. A headless window
//! has no debug selectors: the chip's click lands at a point computed from
//! the viewport's debug frame and the layout tokens (checked by its effect);
//! the other motions start through the functions their buttons call, with
//! the pointer's initiator. Durations are written here from ADR-0030's
//! tokens and the fixtures' heights (`tests/app/threads/chip_motion.rs`,
//! `tests/app/header_card.rs`).

use std::sync::Arc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::{Entity, HeadlessAppContext, point, px, size};
use image::{GenericImage as _, RgbaImage};
use polygloss_app::motion::{Initiator, MotionPolicy, tokens};
use polygloss_app::review_tab::{ReviewTab, header, open_review};
use polygloss_app::space::{edge, height, stroke};
use polygloss_app::tabs::TabItem;
use polygloss_app::threads::{self, ReviewThreads, block, placement};
use polygloss_app::{live, startup, window};
use polygloss_core::git::{CompareMode, Since, Source};
use polygloss_core::objects::BlobReader;
use polygloss_core::review::{
    Author, AuthorKind, Core, NewThread, OpenRequest, Subject, ThreadKind,
};
use polygloss_core::store::events::Actor;
use polygloss_diff::{ObjectFormat, Side};

use crate::motion_viewport::{App, click_at, ms};
use crate::support::filmstrip::{
    FRAMES, GUTTER, GUTTER_COLOR, assert_filmstrip, filmstrip, frame_of,
};
use crate::support::harness::Test;
use crate::support::screenshot::{self, SCALE};
use crate::support::{FixtureRepo, Sandbox, code_change_repo};

pub const TESTS: &[Test] = &[
    Test::new(
        "motion_blocks::e2e_motion_thread_chip",
        e2e_motion_thread_chip,
    ),
    Test::new(
        "motion_blocks::e2e_motion_thread_chip_collapse",
        e2e_motion_thread_chip_collapse,
    ),
    Test::new("motion_blocks::e2e_motion_commits", e2e_motion_commits),
    Test::new(
        "motion_blocks::e2e_motion_commits_hide",
        e2e_motion_commits_hide,
    ),
    Test::new("motion_blocks::e2e_motion_snapshot", e2e_motion_snapshot),
];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 40;
/// The window: wide enough for the header card's natural layout.
const WINDOW: (f32, f32) = (1100.0, 640.0);
/// `reveal(105)`: the note's card is 105 pt taller than its chip.
const CHIP_OPEN_MS: f64 = 160.5;
/// `exit(160.5)`.
const CHIP_CLOSE_MS: f64 = 120.0;
/// `reveal(93)`: three commits' list.
const SHOW_MS: f64 = 159.3;
/// `exit(159.3)`.
const HIDE_MS: f64 = 120.0;
/// The header card without its list, and three commits' list.
const CARD: f32 = 56.0;
const LIST: f32 = 93.0;

/// The app at [`WINDOW`] showing `req` once its rows are highlighted, under
/// the harness's Off policy, the header card's clock pinned.
fn open(req: OpenRequest) -> App {
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (window, main) = cx.update(|cx| {
        startup::init(core, cx);
        header::set_clock(|| screenshot::CLOCK_MS, cx);
        window::open_main_window_sized(size(px(WINDOW.0), px(WINDOW.1)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, window);
    let _task = cx
        .update_window(window, |_, w, cx| open_review(req, w, cx))
        .expect("the window is open");
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, window);
        let tab = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        let Some(tab) = tab else { continue };
        let viewport = cx.update(|cx| tab.read(cx).viewport.clone());
        let d = cx.update(|cx| viewport.read(cx).debug());
        if !d.visible_rows.is_empty()
            && !d.visible_rows.iter().any(|r| r == "Loading…")
            && d.styled_rows > 0
            && d.prelude.is_some()
        {
            for _ in 0..3 {
                screenshot::draw(&mut cx, window);
            }
            return App {
                cx,
                window,
                viewport,
                tab,
            };
        }
    }
    panic!("the review tab never settled");
}

impl App {
    fn redraw(&mut self) {
        screenshot::park_pointer(&mut self.cx, self.window);
        for _ in 0..3 {
            screenshot::draw(&mut self.cx, self.window);
        }
    }

    /// The window's last frame as Metal draws it.
    fn capture(&mut self) -> RgbaImage {
        self.cx
            .capture_screenshot(self.window)
            .expect("render the frame with Metal")
    }

    /// The header card's window bounds `(x, y, w, h)`, from the debug frame.
    fn prelude(&mut self) -> (f32, f32, f32, f32) {
        let (x, y, w, h) = self.debug().prelude.expect("the header card is placed");
        let o = self.origin();
        (o.0 + x, o.1 + y, w, h)
    }
}

/// Each frame of a one-row `strip` cut to `(x, y, w, h)` (window points),
/// [`GUTTER`] apart.
fn crop(strip: &RgbaImage, (x, y, w, h): (f32, f32, f32, f32)) -> RgbaImage {
    let s = SCALE as f32;
    let (x, y, w, h) = (
        (x * s) as u32,
        (y * s) as u32,
        (w * s) as u32,
        (h * s) as u32,
    );
    let n = FRAMES as u32;
    let mut out = RgbaImage::from_pixel(n * w + (n - 1) * GUTTER, h, GUTTER_COLOR);
    for i in 0..FRAMES {
        let frame = frame_of(strip, i);
        let part = image::imageops::crop_imm(&frame, x, y, w, h).to_image();
        out.copy_from(&part, i as u32 * (w + GUTTER), 0)
            .expect("the crop fits");
    }
    out
}

/// `(x, y, w, h)` of an image, cut out (device pixels from window points).
fn region(image: &RgbaImage, (x, y, w, h): (f32, f32, f32, f32)) -> RgbaImage {
    let s = SCALE as f32;
    image::imageops::crop_imm(
        image,
        (x * s) as u32,
        (y * s) as u32,
        (w * s) as u32,
        (h * s) as u32,
    )
    .to_image()
}

/// An agent's one-line note on `src/config.rs` line 1 and its comment on
/// line 5, in the compare of [`code_change_repo`]; the note's id.
fn with_note(app: &mut App) -> String {
    let core = Core::open_default().expect("the sandbox store");
    let tab = app.tab.clone();
    let (review_id, diff_id, info) = app.cx.update(|cx| {
        let t = tab.read(cx);
        (
            t.review_id.clone(),
            t.opened.diff_id.clone(),
            t.opened.repo.clone(),
        )
    });
    let blobs = BlobReader::open(&info).expect("open the object store");
    let agent = Author {
        kind: AuthorKind::Agent,
        name: "claude-code".into(),
        session_id: None,
    };
    let mut add = |line: u32, kind: ThreadKind, body: &str| {
        core.create_thread(
            &NewThread {
                review_id: review_id.clone(),
                diff_id: diff_id.clone(),
                subject: Subject::Line {
                    path: "src/config.rs".into(),
                    side: Side::New,
                    start_line: line,
                    line,
                },
                kind,
                body_md: body.into(),
                author: agent.clone(),
            },
            &blobs,
        )
        .expect("create the thread")
    };
    let note = add(1, ThreadKind::Note, "Sorted on purpose.");
    add(5, ThreadKind::Comment, "Below.");
    app.cx.update(|cx| tab.update(cx, threads::reload));
    app.redraw();
    note
}

fn compare_req(repo: &FixtureRepo) -> OpenRequest {
    OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/tags/head".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    }
}

fn model(app: &mut App) -> Entity<ReviewThreads> {
    let tab = app.tab.clone();
    app.cx
        .update(|cx| threads::threads(tab.read(cx)).cloned().expect("threads"))
}

fn expanded(app: &mut App, note: &str) -> bool {
    let m = model(app);
    app.cx.update(|cx| m.read(cx).is_expanded(note))
}

/// The window point of the note's chip (inside it, past its icon and
/// label), from its block row in the debug frame: `NESTED_Y` below the
/// row's top, half the chip's height down, `NESTED_X` in from the card's
/// inner edge.
fn chip_point(app: &mut App, note: &str) -> gpui_kit::Point<gpui_kit::Pixels> {
    let block = format!("[block {}]", placement::block_id(note).0);
    let d = app.debug();
    let i = d
        .visible_rows
        .iter()
        .position(|r| *r == block)
        .unwrap_or_else(|| panic!("{block} not in {:#?}", d.visible_rows));
    let (top, _) = d.row_bounds[i];
    let card = d.cards[0].bounds;
    let o = app.origin();
    point(
        px(o.0 + card.0 + stroke::BORDER + edge::NESTED_X + 200.0),
        px(o.1 + top + edge::NESTED_Y + height::MD / 2.0),
    )
}

/// The file card holding the blocks, with a margin, from the viewport's
/// top: what M7 moves.
fn chip_region(app: &mut App) -> (f32, f32, f32, f32) {
    let d = app.debug();
    let card = d.cards[0].bounds;
    let o = app.origin();
    (o.0 + card.0 - 12.0, o.1, card.2 + 24.0, 440.0)
}

fn e2e_motion_thread_chip() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    let mut app = open(compare_req(&repo));
    let note = with_note(&mut app);
    let window = app.window;
    let at = chip_point(&mut app, &note);
    let click = move |cx: &mut HeadlessAppContext| click_at(cx, window, at);
    let area = chip_region(&mut app);
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        ms(CHIP_OPEN_MS),
        click,
    );
    assert!(expanded(&mut app, &note), "the click landed on the chip");
    app.policy(MotionPolicy::Off);
    let m = model(&mut app);
    app.cx
        .update(|cx| m.update(cx, |m, cx| m.toggle_expanded(&note, cx)));
    app.redraw();
    assert!(!expanded(&mut app, &note));
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        click,
    );
    assert!(expanded(&mut app, &note));
    assert_filmstrip("thread-chip", crop(&full, area), crop(&reduced, area));
}

fn e2e_motion_thread_chip_collapse() {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    crate::support::home_above(repo.path());
    let mut app = open(compare_req(&repo));
    let note = with_note(&mut app);
    let m = model(&mut app);
    app.cx
        .update(|cx| m.update(cx, |m, cx| m.toggle_expanded(&note, cx)));
    app.redraw();
    let window = app.window;
    let area = chip_region(&mut app);
    let (m2, id) = (m.clone(), note.clone());
    let collapse = move |cx: &mut HeadlessAppContext| {
        cx.update(|cx| block::toggle_by(&m2, &id, Initiator::Pointer, cx));
        cx.run_until_parked();
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        ms(CHIP_CLOSE_MS),
        collapse.clone(),
    );
    assert!(!expanded(&mut app, &note), "collapsed");
    app.policy(MotionPolicy::Off);
    app.cx
        .update(|cx| m.update(cx, |m, cx| m.toggle_expanded(&note, cx)));
    app.redraw();
    // Reduced: the swap and the height snap (every frame the settled one).
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::MICRO,
        collapse,
    );
    assert_filmstrip(
        "thread-chip-collapse",
        crop(&full, area),
        crop(&reduced, area),
    );
}

/// `main` with `a.rs`; branch `feature` (checked out) with three commits.
fn commits_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    repo.write("a.rs", b"let a = 0;\n");
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    for i in 1..=3 {
        repo.write("a.rs", format!("let a = {i};\n").as_bytes());
        repo.commit(&format!("step {i}"));
    }
    repo
}

fn three_commits(repo: &FixtureRepo) -> App {
    open(OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/heads/main".into(),
            head: "refs/heads/feature".into(),
            mode: CompareMode::ThreeDot,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    })
}

/// The header card and the top of the card below it, with a margin.
fn commits_region(app: &mut App) -> (f32, f32, f32, f32) {
    let (x, y, w, _) = app.prelude();
    (x - 12.0, y - 8.0, w + 24.0, CARD + LIST + 12.0 + 72.0)
}

fn toggle(cx: &mut HeadlessAppContext, tab: &Entity<ReviewTab>, initiator: Initiator) {
    cx.update(|cx| tab.update(cx, |t, cx| header::toggle_commits(t, initiator, cx)));
    cx.run_until_parked();
}

fn e2e_motion_commits() {
    let _sb = Sandbox::isolate();
    let repo = commits_repo();
    let mut app = three_commits(&repo);
    let window = app.window;
    let area = commits_region(&mut app);
    let tab = app.tab.clone();
    let show = move |cx: &mut HeadlessAppContext| toggle(cx, &tab, Initiator::Pointer);
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        ms(SHOW_MS),
        show.clone(),
    );
    let (_, y, _, h) = app.prelude();
    assert_eq!(h, CARD + LIST, "the list opened");
    app.policy(MotionPolicy::Off);
    let tab = app.tab.clone();
    toggle(&mut app.cx, &tab, Initiator::Programmatic);
    app.redraw();
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        show,
    );
    // Reduced fades the list only: the card's top part is the same on
    // every frame.
    let (x, _, w, _) = app.prelude();
    let top = (x, y, w, CARD - 1.0);
    let settled = region(&frame_of(&reduced, FRAMES - 1), top);
    for i in 0..FRAMES - 1 {
        let c =
            screenshot::compare(&region(&frame_of(&reduced, i), top), &settled).expect("same size");
        assert!(
            c.within_budget(),
            "Reduced frame {i}: the card's top part changed: {c:?}"
        );
    }
    assert_filmstrip("commits", crop(&full, area), crop(&reduced, area));
}

fn e2e_motion_commits_hide() {
    let _sb = Sandbox::isolate();
    let repo = commits_repo();
    let mut app = three_commits(&repo);
    let window = app.window;
    let tab = app.tab.clone();
    toggle(&mut app.cx, &tab, Initiator::Programmatic);
    app.redraw();
    let area = commits_region(&mut app);
    let (x, y, w, h) = app.prelude();
    assert_eq!(h, CARD + LIST, "open");
    let list = (x, y + CARD, w, LIST - 1.0);
    let open_list = region(&app.capture(), list);
    let hide = move |cx: &mut HeadlessAppContext| toggle(cx, &tab, Initiator::Pointer);
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        ms(HIDE_MS),
        hide.clone(),
    );
    assert_eq!(app.prelude().3, CARD, "the list closed");
    // No pop: on the commit frame the list is where it was, under the
    // curtain.
    let c = screenshot::compare(&region(&frame_of(&full, 0), list), &open_list).expect("same size");
    assert!(c.within_budget(), "the commit frame's list moved: {c:?}");
    app.policy(MotionPolicy::Off);
    let tab = app.tab.clone();
    toggle(&mut app.cx, &tab, Initiator::Programmatic);
    app.redraw();
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::MICRO,
        hide,
    );
    assert_filmstrip("commits-hide", crop(&full, area), crop(&reduced, area));
}

fn e2e_motion_snapshot() {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    repo.write("a.rs", b"let one = 1;\n");
    repo.commit("base");
    repo.branch("feature");
    repo.checkout("feature");
    repo.write("a.rs", b"let one = 1;\nlet two = 2;\n");
    let mut app = open(OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Live {
            since: Since::MergeBase,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    });
    let window = app.window;
    let tab = app.tab.clone();
    // Snapshot's right end of the card: the button and the stats beside it.
    let (x, y, w, h) = app.prelude();
    let area = (x + w - 260.0, y - 8.0, 272.0, h + 16.0);
    let icon = |app: &mut App| {
        let tab = app.tab.clone();
        app.cx.update(|cx| header::snapshot_icon(tab.read(cx)))
    };
    // The toast it raises sits over the card's right end: dismissed, so the
    // strip shows the button.
    let save = move |cx: &mut HeadlessAppContext| {
        cx.update_window(window, |_, w, cx| {
            tab.update(cx, |t, cx| live::snapshot(t, w, cx))
        })
        .expect("the window is open");
        cx.run_until_parked();
        cx.update_window(window, |_, w, cx| w.clear_notifications(cx))
            .expect("the window is open");
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::QUICK,
        save.clone(),
    );
    assert!(icon(&mut app).is_some_and(|i| i.check), "saved: the check");
    // Enabled again by a new live state (under Off), then Reduced.
    app.policy(MotionPolicy::Off);
    repo.write("a.rs", b"let one = 1;\nlet two = 2;\nlet three = 3;\n");
    let tab = app.tab.clone();
    app.cx
        .update_window(window, |_, w, cx| {
            tab.update(cx, |t, cx| live::refresh_tab(t, w, cx))
        })
        .expect("the window is open");
    app.redraw();
    assert!(icon(&mut app).is_some_and(|i| !i.check), "the camera again");
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        save,
    );
    assert_filmstrip("snapshot", crop(&full, area), crop(&reduced, area));
}
