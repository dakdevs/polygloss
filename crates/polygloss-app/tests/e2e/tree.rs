//! Screenshots of T3.6 (design §11.5): the file tree with compacted
//! folders and every row badge (Viewed checkboxes, including a partly
//! viewed folder, status letters, +/− counts, open threads, the agent badge
//! and the "changed since viewed" dot), next to the diff.

use std::sync::Arc;

use gpui_kit::{px, size};
use polygloss_app::review_tab::open_review;
use polygloss_app::tabs::TabItem;
use polygloss_app::tree::file_tree;
use polygloss_app::{startup, window};
use polygloss_core::git::{CompareMode, Source};
use polygloss_core::review::{Core, OpenRequest};
use polygloss_core::store::events::Actor;
use polygloss_diff::ObjectFormat;
use polygloss_viewport::FileFlags;

use crate::support::harness::Test;
use crate::support::screenshot::{self, WINDOW_HEIGHT, WINDOW_WIDTH, assert_screenshot};
use crate::support::{CONFIG_RS_BASE, CONFIG_RS_HEAD, FixtureRepo, Sandbox};

pub const TESTS: &[Test] = &crate::tests![e2e_tree_badges];

/// Frames drawn at most while waiting for loads, counts and highlights.
const MAX_FRAMES: usize = 30;

const ROW_RS_BASE: &str = "pub fn render(label: &str) -> String {\n    label.to_string()\n}\n";
const ROW_RS_HEAD: &str = "pub fn render(label: &str, depth: usize) -> String {\n    format!(\"{}{label}\", \" \".repeat(depth * 2))\n}\n";
const LIB_RS_BASE: &str = "pub mod store;\npub mod git;\n";
const LIB_RS_HEAD: &str = "pub mod events;\npub mod git;\npub mod store;\n";
const FILTERS_RS: &str = "/// Which files the tree shows.\n#[derive(Default)]\npub struct Filters {\n    pub unviewed: bool,\n    pub query: String,\n}\n";
const INTRO_MD: &str =
    "# Getting started\n\nOpen a review with ⌘O, then press `n` for the next file.\n";
const BUILD_SH: &str = "#!/bin/sh\nset -eu\nmake all\n";
const PREFS_RS_BASE: &str = "/// Settings read at startup.\npub struct Settings {\n    pub theme: String,\n    pub font_size: u32,\n    pub tab_width: u32,\n    pub wrap: bool,\n}\n";
const PREFS_RS_HEAD: &str = "/// Preferences read at startup.\npub struct Settings {\n    pub theme: String,\n    pub font_size: u32,\n    pub tab_width: u32,\n    pub wrap: bool,\n}\n";

/// A repo with tags `base` and `head`; the diff, in order: `README.md`
/// (M), `crates/app/src/tree/filters.rs` (A), `crates/app/src/tree/row.rs`
/// (M), `crates/core/src/lib.rs` (M), `docs/guide/intro.md` (A),
/// `scripts/old-build.sh` (D), `src/config.rs` (M), `src/prefs.rs` (R from
/// `src/settings.rs`, one line changed).
fn badges_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("README.md", b"# app\n\nA small app.\n");
    repo.write("crates/app/src/tree/row.rs", ROW_RS_BASE.as_bytes());
    repo.write("crates/core/src/lib.rs", LIB_RS_BASE.as_bytes());
    repo.write("scripts/old-build.sh", BUILD_SH.as_bytes());
    repo.write("src/config.rs", CONFIG_RS_BASE.as_bytes());
    repo.write("src/settings.rs", PREFS_RS_BASE.as_bytes());
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("README.md", b"# app\n\nA small app with a file tree.\n");
    repo.write("crates/app/src/tree/filters.rs", FILTERS_RS.as_bytes());
    repo.write("crates/app/src/tree/row.rs", ROW_RS_HEAD.as_bytes());
    repo.write("crates/core/src/lib.rs", LIB_RS_HEAD.as_bytes());
    repo.write("docs/guide/intro.md", INTRO_MD.as_bytes());
    std::fs::remove_file(repo.path().join("scripts/old-build.sh")).unwrap();
    repo.write("src/config.rs", CONFIG_RS_HEAD.as_bytes());
    std::fs::remove_file(repo.path().join("src/settings.rs")).unwrap();
    repo.write("src/prefs.rs", PREFS_RS_HEAD.as_bytes());
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

fn e2e_tree_badges() {
    let _sb = Sandbox::isolate();
    let repo = badges_repo();
    let core = Core::open_default().expect("open the sandbox store");
    let mut cx = screenshot::headless_app_with_assets(Arc::new(gpui_kit::assets::Assets));
    let (handle, main) = cx.update(|cx| {
        startup::init(core.clone(), cx);
        window::open_main_window_sized(size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx)
            .expect("open the main window")
    });
    screenshot::park_pointer(&mut cx, handle);
    let req = OpenRequest {
        worktree: repo.path().to_path_buf(),
        source: Source::Compare {
            base: "refs/tags/base".into(),
            head: "refs/tags/head".into(),
            mode: CompareMode::Direct,
        },
        label: None,
        pin: None,
        actor: Actor::human(),
    };
    // Viewed marks in the store (T3.7): README and filters.rs viewed (so
    // `app/src/tree` is partly viewed), and row.rs viewed by this review at
    // another blob pair (so it is "changed since viewed").
    let opened = core.open(&req).expect("open the review in the store");
    let mark = |idx: usize| {
        core.set_viewed(Some(&opened.review_id), &opened.files[idx], true)
            .expect("mark viewed");
    };
    mark(0);
    mark(1);
    let mut earlier = opened.files[2].clone();
    earlier.new_blob = earlier.old_blob.clone();
    core.set_viewed(Some(&opened.review_id), &earlier, true)
        .expect("mark an earlier pair viewed");
    let _task = cx
        .update_window(handle, |_, window, cx| open_review(req, window, cx))
        .expect("the window is open");
    let mut tab = None;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        tab = cx.update(|cx| {
            main.read(cx)
                .tabs()
                .get(1)
                .and_then(TabItem::review)
                .cloned()
        });
        if tab.is_some() {
            break;
        }
    }
    let tab = tab.expect("the review tab opened");
    // T3.7 loads the Viewed marks from the store and pushes them; push the
    // badges' state after that, so it is not overwritten.
    for _ in 0..MAX_FRAMES {
        if cx.update(|cx| polygloss_app::viewed::is_loaded(tab.read(cx))) {
            break;
        }
        screenshot::draw(&mut cx, handle);
    }
    assert!(
        cx.update(|cx| polygloss_app::viewed::is_loaded(tab.read(cx))),
        "the Viewed marks loaded"
    );
    let (tree, statuses) = cx.update(|cx| {
        let t = tab.read(cx);
        (
            file_tree(t).cloned().expect("a file tree"),
            t.opened.files.iter().map(|f| f.status).collect::<Vec<_>>(),
        )
    });
    use polygloss_diff::FileStatus::{Added as A, Deleted as D, Modified as M, Renamed as R};
    assert_eq!(
        statuses,
        [M, A, M, M, A, D, M, R],
        "all four status letters"
    );
    let files = statuses.len();
    // The store's marks, then row.rs with two open threads, one an
    // agent's, and lib.rs with one (the threads' fields, T3.9's).
    let mut flags = vec![FileFlags::default(); files];
    flags[0].viewed = true;
    flags[1].viewed = true;
    flags[2].changed_since_viewed = true;
    cx.update(|cx| assert_eq!(tree.read(cx).file_flags(), flags.as_slice()));
    flags[2].open_threads = 2;
    flags[2].agent_threads = true;
    flags[3].open_threads = 1;
    cx.update(|cx| {
        tab.update(cx, |t, cx| {
            polygloss_app::viewed::update_file_flags(t, cx, |f| {
                f[2].open_threads = 2;
                f[2].agent_threads = true;
                f[3].open_threads = 1;
            })
        });
        // The same state in the diff's file headers and the tree.
        assert_eq!(tree.read(cx).file_flags(), flags.as_slice());
        assert_eq!(
            tab.read(cx).viewport.read(cx).file_flags(),
            flags.as_slice()
        );
    });

    // Wait for the diff to paint and every file's +/− counts.
    let mut settled = false;
    for _ in 0..MAX_FRAMES {
        screenshot::draw(&mut cx, handle);
        let (debug, counted) = cx.update(|cx| {
            let v = tab.read(cx).viewport.read(cx);
            let counted = (0..files as u32).all(|i| v.file_counts(i).is_some());
            (v.debug(), counted)
        });
        if counted
            && debug.visible_rows.len() > 10
            && !debug.visible_rows.iter().any(|r| r == "Loading…")
            && debug.styled_rows > 0
        {
            settled = true;
            break;
        }
    }
    assert!(settled, "the review tab never settled");
    screenshot::draw(&mut cx, handle);
    let labels: Vec<String> = cx.update(|cx| {
        tree.read(cx)
            .rows(cx)
            .into_iter()
            .map(|r| r.label)
            .collect()
    });
    assert_eq!(
        labels,
        [
            "README.md",
            "crates",
            "app/src/tree",
            "filters.rs",
            "row.rs",
            "core/src",
            "lib.rs",
            "docs/guide",
            "intro.md",
            "scripts",
            "old-build.sh",
            "src",
            "config.rs",
            "prefs.rs",
        ]
    );
    let image = screenshot::capture(&mut cx, handle);
    assert_screenshot(&image);
}
