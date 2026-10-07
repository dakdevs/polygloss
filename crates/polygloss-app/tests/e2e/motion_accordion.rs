//! The accordion's panel switch in the full app (T7.13, ADR-0030 M9), drawn
//! by Metal: its filmstrip, Full over Reduced (`support::filmstrip`). A
//! header's click (its handler's call, `FileTree::open_panel_by` with
//! `Initiator::Pointer`) opens Tests below the open Changes: Changes' body
//! shrinks from its header while Tests' header glides up with its body
//! hanging from it, and both chevrons turn; under Reduce Motion the switch
//! is instant and Tests' list fades in.
//!
//! The harness names this test `motion_accordion::e2e_motion_accordion`.
//! Fixture: a commit adding `lib/util.rs`, `src/{app,ui}.rs` (Changes),
//! `src/app.test.rs`, `tests/ui.rs` (Tests) and `Cargo.lock` (Generated),
//! in the smallest main window. The swap lasts `reveal(H)` for the body's
//! height H, read from the tree.

use std::sync::Arc;

use gpui_kit::{AnyWindowHandle, Entity, HeadlessAppContext, px, size};
use polygloss_app::motion::{self, Initiator, MotionPolicy, tokens};
use polygloss_app::review_tab::{ReviewTab, open_review};
use polygloss_app::space::layout;
use polygloss_app::tabs::TabItem;
use polygloss_app::tree::panels::PanelKey;
use polygloss_app::tree::{FileTree, file_tree};
use polygloss_app::window::MainWindow;
use polygloss_app::{startup, window};
use polygloss_core::git::Source;
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;

use crate::support::filmstrip::{assert_filmstrip, filmstrip};
use crate::support::harness::Test;
use crate::support::screenshot;
use crate::support::{FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &[Test::new(
    "motion_accordion::e2e_motion_accordion",
    e2e_motion_accordion,
)];

/// Frames drawn at most while waiting for loads and highlights.
const MAX_FRAMES: usize = 40;

/// Changes: `lib/util.rs`, `src/app.rs`, `src/ui.rs`; Tests:
/// `src/app.test.rs`, `tests/ui.rs`; Generated: `Cargo.lock`.
fn panels_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    crate::support::home_above(repo.path());
    repo.write("notes.txt", b"notes\n");
    repo.commit("notes");
    for (path, body) in [
        ("Cargo.lock", "version = 4\n"),
        ("lib/util.rs", "pub fn util() {}\n"),
        ("src/app.rs", "pub fn app() {}\n"),
        ("src/app.test.rs", "#[test]\nfn app() {}\n"),
        ("src/ui.rs", "pub fn ui() {}\n"),
        ("tests/ui.rs", "#[test]\nfn ui() {}\n"),
    ] {
        repo.write(path, body.as_bytes());
    }
    repo.commit("add the app");
    repo
}

/// The app and the main window. `cx` is the last field: fields drop in
/// order, and the app checks for leaked entity handles when it drops.
struct App {
    window: AnyWindowHandle,
    main: Entity<MainWindow>,
    cx: HeadlessAppContext,
}

impl App {
    fn open(width: f32, height: f32) -> App {
        let core = Core::open_default().expect("open the sandbox store");
        let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
        let (window, main) = cx.update(|cx| {
            startup::init(core, cx);
            window::open_main_window_sized(size(px(width), px(height)), cx)
                .expect("open the main window")
        });
        screenshot::park_pointer(&mut cx, window);
        screenshot::draw(&mut cx, window);
        App { window, main, cx }
    }

    /// Opens the commit at `repo`'s HEAD and draws until its rows are
    /// highlighted.
    fn open_commit(&mut self, repo: &FixtureRepo) -> Entity<ReviewTab> {
        let req = OpenRequest {
            worktree: repo.path().to_path_buf(),
            source: Source::Commit { rev: "HEAD".into() },
            label: None,
            pin: None,
            actor: Actor::human(),
        };
        let _task = self
            .cx
            .update_window(self.window, |_, w, cx| open_review(req, w, cx))
            .expect("the window is open");
        let main = self.main.clone();
        for _ in 0..MAX_FRAMES {
            self.draw();
            let tab = self.cx.update(|cx| {
                main.read(cx)
                    .tabs()
                    .get(1)
                    .and_then(TabItem::review)
                    .cloned()
            });
            let Some(tab) = tab else { continue };
            let viewport = self.cx.update(|cx| tab.read(cx).viewport.clone());
            let d = self.cx.update(|cx| viewport.read(cx).debug());
            if !d.visible_rows.is_empty()
                && !d.visible_rows.iter().any(|r| r == "Loading…")
                && d.styled_rows > 0
            {
                for _ in 0..3 {
                    self.draw();
                }
                return tab;
            }
        }
        panic!("the review tab never settled");
    }

    fn draw(&mut self) {
        screenshot::draw(&mut self.cx, self.window);
    }
}

/// `tree`'s panel keys, top to bottom.
fn panel_keys(cx: &mut HeadlessAppContext, tree: &Entity<FileTree>) -> Vec<PanelKey> {
    cx.update(|cx| {
        (tree.read(cx).panels().panels().iter())
            .map(|p| p.key().clone())
            .collect()
    })
}

fn e2e_motion_accordion() {
    let _sb = Sandbox::isolate();
    let repo = panels_repo();
    let (width, height) = layout::WINDOW_MIN;
    let mut app = App::open(width, height);
    let tab = app.open_commit(&repo);
    let tree = app
        .cx
        .update(|cx| file_tree(tab.read(cx)).cloned().expect("a file tree"));
    let keys = panel_keys(&mut app.cx, &tree);
    let names: Vec<String> = keys.iter().map(ToString::to_string).collect();
    assert_eq!(names, ["changes", "tests", "generated"]);
    let open = {
        let tree = tree.clone();
        move |cx: &mut HeadlessAppContext| {
            cx.update(|cx| tree.read(cx).panels().open_key().cloned())
        }
    };
    assert_eq!(open(&mut app.cx).as_ref(), Some(&keys[0]), "Changes open");
    let h = app.cx.update(|cx| tree.read(cx).panels().body_height());
    assert!(h > 0.0, "the body was laid out");
    let window = app.window;
    let switch = {
        let (tree, tests, open) = (tree.clone(), keys[1].clone(), open.clone());
        move |cx: &mut HeadlessAppContext| {
            cx.update_window(window, |_, w, cx| {
                tree.update(cx, |t, cx| {
                    t.open_panel_by(&tests, Initiator::Pointer, w, cx)
                })
            })
            .expect("the window is open");
            assert_eq!(open(cx).as_ref(), Some(&tests), "Tests opened");
        }
    };
    let full = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Full,
        tokens::reveal(h, h),
        switch.clone(),
    );
    // Back to Changes at once, between the rows.
    app.cx
        .update(|cx| motion::set_override(Some(MotionPolicy::Off), cx));
    let changes = keys[0].clone();
    app.cx
        .update(|cx| tree.update(cx, |t, cx| t.open_panel(&changes, cx)));
    for _ in 0..3 {
        app.draw();
    }
    assert_eq!(open(&mut app.cx).as_ref(), Some(&keys[0]));
    let reduced = filmstrip(
        &mut app.cx,
        window,
        MotionPolicy::Reduced,
        tokens::QUICK,
        switch,
    );
    assert_filmstrip("accordion", full, reduced);
}
