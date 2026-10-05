//! File tree and file finder (T3.6, design §11.5, §11.8): compacted
//! directories, row checkboxes, filters, the nucleo fuzzy filter, tree ↔
//! viewport sync, ⌘P and the 13k-file build budget.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::component::WindowExt as _;
use gpui_kit::{AppContext as _, Entity, Focusable as _, TestAppContext};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::tree::filters::{self, StatusFilter, TreeFilters};
use polygloss_app::tree::model::{ItemId, NodeKind, TreeModel};
use polygloss_app::tree::{FileTree, FileTreeEvent, file_tree, finder};
use polygloss_diff::{FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, ObjectFormat, Oid};
use polygloss_viewport::{DiffProvider, DiffViewport, FileFlags, ScrollTarget, ViewportOptions};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{FixtureRepo, Sandbox};

/// `n` lines of numbered text, so every file is taller than the window
/// and a jump can bring any file to the top.
fn lines(tag: &str, n: usize) -> String {
    (1..=n).map(|i| format!("{tag} line {i}\n")).collect()
}

/// A repo with tags `base` and `head`; the diff, in order:
/// 0 `README.md` (M), 1 `config.toml` (A), 2 `docs/guide/intro.md` (A),
/// 3 `src/app/ui/button.rs` (M), 4 `src/app/ui/menu.rs` (D),
/// 5 `src/lib.rs` (M), 6 `src/new_name.rs` (R from `src/old_name.rs`).
fn tree_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("README.md", lines("readme", 60).as_bytes());
    repo.write("src/app/ui/button.rs", lines("button", 60).as_bytes());
    repo.write("src/app/ui/menu.rs", lines("menu", 60).as_bytes());
    repo.write("src/lib.rs", lines("lib", 60).as_bytes());
    repo.write("src/old_name.rs", lines("renamed", 60).as_bytes());
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("README.md", (lines("readme", 60) + "more\n").as_bytes());
    repo.write("config.toml", lines("config", 60).as_bytes());
    repo.write("docs/guide/intro.md", lines("intro", 60).as_bytes());
    repo.write(
        "src/app/ui/button.rs",
        (lines("button", 60) + "fn more() {}\n").as_bytes(),
    );
    std::fs::remove_file(repo.path().join("src/app/ui/menu.rs")).unwrap();
    repo.write(
        "src/lib.rs",
        (String::from("// head\n") + &lines("lib", 60)).as_bytes(),
    );
    std::fs::rename(
        repo.path().join("src/old_name.rs"),
        repo.path().join("src/new_name.rs"),
    )
    .unwrap();
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

const PATHS: [&str; 7] = [
    "README.md",
    "config.toml",
    "docs/guide/intro.md",
    "src/app/ui/button.rs",
    "src/app/ui/menu.rs",
    "src/lib.rs",
    "src/new_name.rs",
];

struct Opened<'a> {
    shell: Shell<'a>,
    tab: Entity<ReviewTab>,
    tree: Entity<FileTree>,
}

fn open_tree<'a>(cx: &'a mut TestAppContext, repo: &FixtureRepo) -> Opened<'a> {
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    draw(shell.cx);
    let tree = tab.read_with(shell.cx, |t, _| file_tree(t).cloned().expect("a file tree"));
    let paths: Vec<String> = tab.read_with(shell.cx, |t, _| {
        t.opened
            .files
            .iter()
            .map(|f| f.display_path().to_owned())
            .collect()
    });
    assert_eq!(paths, PATHS, "the fixture's diff order");
    Opened { shell, tab, tree }
}

fn rows(o: &mut Opened) -> Vec<(String, usize)> {
    o.tree.read_with(o.shell.cx, |t, cx| {
        t.rows(cx).into_iter().map(|r| (r.label, r.depth)).collect()
    })
}

fn row_labels(o: &mut Opened) -> Vec<String> {
    rows(o).into_iter().map(|(l, _)| l).collect()
}

fn shown_paths(o: &mut Opened) -> Vec<&'static str> {
    o.tree.read_with(o.shell.cx, |t, _| {
        t.model()
            .file_order()
            .iter()
            .map(|&i| PATHS[i as usize])
            .collect()
    })
}

fn click(o: &mut Opened, selector: &'static str) {
    let at = o
        .shell
        .cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("no element {selector}"))
        .center();
    o.shell.cx.simulate_click(at, gpui_kit::Modifiers::none());
    draw(o.shell.cx);
}

fn top_file(o: &mut Opened) -> u32 {
    o.tab
        .read_with(o.shell.cx, |t, cx| t.viewport.read(cx).anchor().file_idx)
}

fn selected_row(o: &mut Opened) -> Option<ItemId> {
    o.tree.read_with(o.shell.cx, |t, cx| {
        t.tree_state()
            .read(cx)
            .selected_item()
            .and_then(|i| ItemId::parse(&i.id))
    })
}

#[test]
fn tree_model_compacts_and_keeps_diff_order() {
    let model = TreeModel::build(PATHS.iter().enumerate().map(|(i, p)| (i as u32, *p)));
    let names = |ids: &[usize]| -> Vec<String> {
        ids.iter().map(|&i| model.node(i).name.clone()).collect()
    };
    assert_eq!(
        names(model.roots()),
        ["README.md", "config.toml", "docs/guide", "src"]
    );
    let docs = model.dir("docs/guide").expect("docs/guide is one node");
    assert_eq!(docs.name, "docs/guide");
    assert_eq!(docs.files, [2]);
    assert!(model.dir("docs").is_none(), "docs was compacted away");
    let src = model.dir("src").unwrap();
    assert_eq!(names(&src.children), ["app/ui", "lib.rs", "new_name.rs"]);
    assert_eq!(src.files, [3, 4, 5, 6]);
    let ui = model.dir("src/app/ui").unwrap();
    assert_eq!(names(&ui.children), ["button.rs", "menu.rs"]);
    assert_eq!(model.node(ui.children[1]).kind, NodeKind::File(4));
    assert_eq!(model.file_order(), [0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(model.dir_paths(), ["docs/guide", "src", "src/app/ui"]);
    assert_eq!(model.ancestors_of_file(4), ["src", "src/app/ui"]);
    assert!(model.ancestors_of_file(0).is_empty());
    // A chain that ends in a file is not compacted into the file.
    let single = TreeModel::build([(0, "a/b/c/d.rs")]);
    assert_eq!(single.node(single.roots()[0]).name, "a/b/c");
    // Ids round-trip.
    for id in [ItemId::Dir("src/app ui".into()), ItemId::File(12)] {
        assert_eq!(ItemId::parse(&id.to_string()), Some(id));
    }
}

#[gpui_kit::test]
fn tree_compacts_single_child_dirs(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    assert_eq!(
        rows(&mut o),
        [
            ("README.md".into(), 0),
            ("config.toml".into(), 0),
            ("docs/guide".into(), 0),
            ("intro.md".into(), 1),
            ("src".into(), 0),
            ("app/ui".into(), 1),
            ("button.rs".into(), 2),
            ("menu.rs".into(), 2),
            ("lib.rs".into(), 1),
            ("new_name.rs".into(), 1),
        ]
    );
    // Every directory starts expanded; the expansion round-trips (T3.14).
    let expanded = o.tree.read_with(o.shell.cx, |t, _| t.expanded_dirs());
    assert_eq!(expanded, ["docs/guide", "src", "src/app/ui"]);
    o.tree.update(o.shell.cx, |t, cx| {
        t.set_expanded_dirs(["src".to_string()], cx)
    });
    draw(o.shell.cx);
    assert_eq!(
        row_labels(&mut o),
        [
            "README.md",
            "config.toml",
            "docs/guide",
            "src",
            "app/ui",
            "lib.rs",
            "new_name.rs"
        ]
    );
    let expanded = o.tree.read_with(o.shell.cx, |t, _| t.expanded_dirs());
    assert_eq!(expanded, ["src"]);
    // Clicking a folder row toggles it, and the expansion follows.
    click(&mut o, "tree-row-d:src/app/ui");
    let expanded = o.tree.read_with(o.shell.cx, |t, _| t.expanded_dirs());
    assert_eq!(expanded, ["src", "src/app/ui"]);
}

#[gpui_kit::test]
fn tree_checkbox_click_does_not_toggle_folder(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    let events: Rc<RefCell<Vec<FileTreeEvent>>> = Rc::default();
    let log = events.clone();
    let tree = o.tree.clone();
    o.shell.cx.update(|_, cx| {
        cx.subscribe(&tree, move |_, e: &FileTreeEvent, _| {
            log.borrow_mut().push(e.clone())
        })
        .detach();
    });
    let before = rows(&mut o);
    let top = top_file(&mut o);

    click(&mut o, "tree-check-d:src");
    assert_eq!(rows(&mut o), before, "the folder stayed expanded");
    assert_ne!(selected_row(&mut o), Some(ItemId::Dir("src".into())));
    assert_eq!(
        *events.borrow(),
        [FileTreeEvent::ToggleFolderViewed {
            dir: "src".into(),
            files: vec![3, 4, 5, 6],
        }]
    );

    // A file's checkbox asks to toggle Viewed and neither selects the row
    // nor scrolls the viewport.
    click(&mut o, "tree-check-f:5");
    assert_eq!(
        events.borrow().last(),
        Some(&FileTreeEvent::ToggleViewed(5))
    );
    assert_eq!(top_file(&mut o), top);
    assert_ne!(selected_row(&mut o), Some(ItemId::File(5)));

    // The checkboxes show the pushed Viewed state: `src` is partly viewed.
    o.tree.update(o.shell.cx, |t, cx| {
        let mut flags = vec![FileFlags::default(); 7];
        flags[5].viewed = true;
        t.set_file_flags(flags, cx);
    });
    draw(o.shell.cx);
    let flags = o.tree.read_with(o.shell.cx, |t, _| t.file_flags().to_vec());
    assert!(flags[5].viewed);

    // Clicking the folder's row (not its checkbox) does toggle it.
    click(&mut o, "tree-row-d:src");
    assert_eq!(
        row_labels(&mut o),
        ["README.md", "config.toml", "docs/guide", "intro.md", "src"]
    );
    assert_eq!(events.borrow().len(), 2, "no checkbox event from the row");
}

#[gpui_kit::test]
fn tree_filters_unviewed_status_extension(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    // A collapsed folder stays collapsed across filtering.
    o.tree.update(o.shell.cx, |t, cx| {
        t.set_expanded_dirs(["docs/guide".to_string(), "src/app/ui".to_string()], cx)
    });
    let mut flags = vec![FileFlags::default(); 7];
    flags[0].viewed = true;
    flags[5].viewed = true;
    flags[3].open_threads = 2;
    o.tree
        .update(o.shell.cx, |t, cx| t.set_file_flags(flags.clone(), cx));

    o.tree.update(o.shell.cx, |t, cx| t.toggle_unviewed(cx));
    draw(o.shell.cx);
    assert_eq!(
        shown_paths(&mut o),
        [
            "config.toml",
            "docs/guide/intro.md",
            "src/app/ui/button.rs",
            "src/app/ui/menu.rs",
            "src/new_name.rs"
        ]
    );
    // Filtering expands what matches.
    assert!(row_labels(&mut o).contains(&"button.rs".to_string()));
    // Marking a file viewed hides it while "unviewed" is on.
    flags[1].viewed = true;
    o.tree
        .update(o.shell.cx, |t, cx| t.set_file_flags(flags.clone(), cx));
    assert!(!shown_paths(&mut o).contains(&"config.toml"));

    // Status (added) on top of unviewed.
    o.tree
        .update(o.shell.cx, |t, cx| t.toggle_status(StatusFilter::Added, cx));
    assert_eq!(shown_paths(&mut o), ["docs/guide/intro.md"]);
    o.tree.update(o.shell.cx, |t, cx| {
        t.toggle_status(StatusFilter::Deleted, cx)
    });
    assert_eq!(
        shown_paths(&mut o),
        ["docs/guide/intro.md", "src/app/ui/menu.rs"]
    );
    // After a filter change the tree compacts what is left: `src/app/ui`.
    assert_eq!(
        row_labels(&mut o),
        ["docs/guide", "intro.md", "src/app/ui", "menu.rs"]
    );

    // Extension alone.
    let tree = o.tree.clone();
    o.shell.cx.update(|window, cx| {
        tree.update(cx, |t, cx| {
            let mut f = TreeFilters::default();
            f.extensions.insert("rs".into());
            t.set_filters(f, window, cx);
        })
    });
    assert_eq!(
        shown_paths(&mut o),
        [
            "src/app/ui/button.rs",
            "src/app/ui/menu.rs",
            "src/lib.rs",
            "src/new_name.rs"
        ]
    );
    // Has comments.
    o.shell.cx.update(|window, cx| {
        tree.update(cx, |t, cx| {
            t.clear_filters(window, cx);
            t.toggle_has_comments(cx);
        })
    });
    assert_eq!(shown_paths(&mut o), ["src/app/ui/button.rs"]);
    draw(o.shell.cx);
    let count = o.shell.cx.debug_bounds("tree-count");
    assert!(count.is_some());

    // Clearing brings every file back with the user's own expansion.
    o.shell
        .cx
        .update(|window, cx| tree.update(cx, |t, cx| t.clear_filters(window, cx)));
    draw(o.shell.cx);
    assert_eq!(shown_paths(&mut o), PATHS);
    assert_eq!(
        row_labels(&mut o),
        ["README.md", "config.toml", "docs/guide", "intro.md", "src"]
    );

    // The extension list and the extension rule.
    let files = o.tab.read_with(o.shell.cx, |t, _| t.opened.files.clone());
    assert_eq!(
        filters::extensions(&files),
        [("rs".into(), 4), ("md".into(), 2), ("toml".into(), 1)]
    );
    assert_eq!(filters::extension("a/b.TXT"), "txt");
    assert_eq!(filters::extension(".gitignore"), "");
    assert_eq!(filters::extension("dir.d/Makefile"), "");
}

/// The tree walks files depth-first, the viewport (and the tree's "file
/// above/below" checks) in diff order; they agree only while every
/// directory's files are contiguous in git's output. Pin that for names
/// that sort around `/` and for renames across directories.
#[gpui_kit::test]
fn tree_order_matches_diff_order(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    let body = |tag: &str| lines(tag, 20);
    for p in [
        "foo.rs",
        "foo/a.rs",
        "foo0.rs",
        "foo-bar.rs",
        "lib/old.rs",
        "zeta/x.rs",
        "a/keep.rs",
    ] {
        repo.write(p, body(p).as_bytes());
    }
    repo.commit("base");
    repo.git(&["tag", "base"]);
    for p in ["foo.rs", "foo/a.rs", "foo0.rs", "foo-bar.rs", "a/keep.rs"] {
        repo.write(p, (body(p) + "changed\n").as_bytes());
    }
    for (from, to) in [("lib/old.rs", "foo/moved.rs"), ("zeta/x.rs", "b/x.rs")] {
        std::fs::create_dir_all(repo.path().join(to).parent().unwrap()).unwrap();
        std::fs::rename(repo.path().join(from), repo.path().join(to)).unwrap();
    }
    repo.commit("head");
    repo.git(&["tag", "head"]);

    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    draw(shell.cx);
    let (renames, order, n) = tab.read_with(shell.cx, |t, cx| {
        let files = &t.opened.files;
        let renames = files
            .iter()
            .filter(|f| f.status == FileStatus::Renamed)
            .count();
        let tree = file_tree(t).expect("a file tree").read(cx);
        (
            renames,
            tree.model().file_order().to_vec(),
            files.len() as u32,
        )
    });
    assert_eq!(renames, 2, "the fixture has two renames");
    assert_eq!(order, (0..n).collect::<Vec<_>>());
}

#[gpui_kit::test]
fn tree_keeps_selected_dir_across_flag_updates(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    // "Unviewed" rebuilds the tree on every flags push.
    o.tree.update(o.shell.cx, |t, cx| t.toggle_unviewed(cx));
    draw(o.shell.cx);
    click(&mut o, "tree-row-d:src");
    assert_eq!(selected_row(&mut o), Some(ItemId::Dir("src".into())));

    let mut flags = vec![FileFlags::default(); 7];
    flags[0].viewed = true;
    o.tree
        .update(o.shell.cx, |t, cx| t.set_file_flags(flags, cx));
    draw(o.shell.cx);
    assert!(!shown_paths(&mut o).contains(&"README.md"));
    assert_eq!(selected_row(&mut o), Some(ItemId::Dir("src".into())));
    let dir = o.tree.read_with(o.shell.cx, |t, _| t.selected_dir());
    assert_eq!(dir, Some(("src".to_string(), vec![3, 4, 5, 6])));
    // The folder the user collapsed stays collapsed.
    assert_eq!(
        row_labels(&mut o),
        ["config.toml", "docs/guide", "intro.md", "src"]
    );
}

#[gpui_kit::test]
fn tree_fuzzy_filter_uses_nucleo(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    let set_query = |o: &mut Opened, q: &str| {
        let q = q.to_owned();
        let tree = o.tree.clone();
        o.shell
            .cx
            .update(|window, cx| tree.update(cx, |t, cx| t.set_query(&q, window, cx)));
        draw(o.shell.cx);
    };
    // A subsequence, not a substring: nucleo's fuzzy match.
    set_query(&mut o, "nwnm");
    assert_eq!(shown_paths(&mut o), ["src/new_name.rs"]);
    // Path-aware: `ui/b` finds button.rs under app/ui.
    set_query(&mut o, "ui/b");
    assert_eq!(shown_paths(&mut o), ["src/app/ui/button.rs"]);
    // Smart case: an uppercase letter makes the query case-sensitive.
    set_query(&mut o, "readme");
    assert_eq!(shown_paths(&mut o), ["README.md"]);
    set_query(&mut o, "Readme");
    assert!(shown_paths(&mut o).is_empty());
    draw(o.shell.cx);
    assert!(o.shell.cx.debug_bounds("tree-clear-filters").is_some());
    set_query(&mut o, "");
    assert_eq!(shown_paths(&mut o), PATHS);

    // Typing in the filter box filters, and its letters are not tree keys.
    let input = o
        .tree
        .read_with(o.shell.cx, |t, _| t.filter_input().clone());
    o.shell.cx.update(|window, cx| {
        let focus = input.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
    });
    o.shell.cx.simulate_input("intro");
    draw(o.shell.cx);
    assert_eq!(shown_paths(&mut o), ["docs/guide/intro.md"]);
    let query = o
        .tree
        .read_with(o.shell.cx, |t, _| t.filters().query.clone());
    assert_eq!(query, "intro");
}

#[gpui_kit::test]
fn tree_select_scrolls_viewport(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    assert_eq!(top_file(&mut o), 0);

    click(&mut o, "tree-row-f:3");
    assert_eq!(top_file(&mut o), 3);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(3)));
    let highlighted = o.tree.read_with(o.shell.cx, |t, _| t.highlighted_file());
    assert_eq!(highlighted, Some(3));

    // The click focused the tree: its keys move through the files.
    o.shell.cx.simulate_keystrokes("down");
    draw(o.shell.cx);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(4)));
    assert_eq!(top_file(&mut o), 4);
    o.shell.cx.simulate_keystrokes("n");
    draw(o.shell.cx);
    assert_eq!(top_file(&mut o), 5);
    o.shell.cx.simulate_keystrokes("p p");
    draw(o.shell.cx);
    assert_eq!(top_file(&mut o), 3);
    // `p` from the first file wraps to the last, skipping folders.
    o.tree.update(o.shell.cx, |t, cx| t.select_file(0, cx));
    draw(o.shell.cx);
    o.shell.cx.simulate_keystrokes("p");
    draw(o.shell.cx);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(6)));

    // `n` into a collapsed folder expands it.
    o.tree.update(o.shell.cx, |t, cx| {
        t.set_expanded_dirs(["src".to_string()], cx);
        t.select_file(2, cx);
    });
    draw(o.shell.cx);
    o.shell.cx.simulate_keystrokes("n");
    draw(o.shell.cx);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(3)));
    let expanded = o.tree.read_with(o.shell.cx, |t, _| t.expanded_dirs());
    assert_eq!(expanded, ["docs/guide", "src", "src/app/ui"]);
}

#[gpui_kit::test]
fn viewport_scroll_highlights_tree_row(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(0)));

    let scroll = |o: &mut Opened, target: ScrollTarget| {
        o.tab.update(o.shell.cx, |t, cx| {
            t.viewport.update(cx, |v, cx| v.scroll_to(target, cx))
        });
        draw(o.shell.cx);
    };
    scroll(&mut o, ScrollTarget::File(5));
    assert_eq!(selected_row(&mut o), Some(ItemId::File(5)));
    let highlighted = o.tree.read_with(o.shell.cx, |t, _| t.highlighted_file());
    assert_eq!(highlighted, Some(5));
    // Scrolling within the file keeps the row.
    o.tab.update(o.shell.cx, |t, cx| {
        t.viewport.update(cx, |v, cx| v.scroll_by(40.0, cx))
    });
    draw(o.shell.cx);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(5)));

    // Choosing the file already at the top (clicking the highlighted row)
    // leaves no stale mark: scrolling up into the file above highlights it.
    scroll(&mut o, ScrollTarget::File(3));
    click(&mut o, "tree-row-f:3");
    assert_eq!(top_file(&mut o), 3);
    o.tab.update(o.shell.cx, |t, cx| {
        t.viewport.update(cx, |v, cx| v.scroll_by(-40.0, cx))
    });
    draw(o.shell.cx);
    assert_eq!(top_file(&mut o), 2);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(2)));
    let highlighted = o.tree.read_with(o.shell.cx, |t, _| t.highlighted_file());
    assert_eq!(highlighted, Some(2));

    // Near the end the viewport cannot bring the last file (a pure rename,
    // header only) to its top; the tree keeps marking the chosen file while
    // it is on screen, then follows the top file again.
    click(&mut o, "tree-row-f:6");
    o.tab.update(o.shell.cx, |t, cx| {
        t.viewport.update(cx, |v, cx| v.scroll_by(-40.0, cx))
    });
    draw(o.shell.cx);
    assert!(top_file(&mut o) < 6);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(6)));
    scroll(&mut o, ScrollTarget::File(2));
    assert_eq!(selected_row(&mut o), Some(ItemId::File(2)));

    // A file inside a collapsed folder marks the folder.
    o.tree.update(o.shell.cx, |t, cx| {
        t.set_expanded_dirs(["docs/guide".to_string()], cx)
    });
    scroll(&mut o, ScrollTarget::File(3));
    assert_eq!(selected_row(&mut o), Some(ItemId::Dir("src".into())));
    let highlighted = o.tree.read_with(o.shell.cx, |t, _| t.highlighted_file());
    assert_eq!(highlighted, Some(3));
    // Scrolling the viewport never expands the tree.
    let expanded = o.tree.read_with(o.shell.cx, |t, _| t.expanded_dirs());
    assert_eq!(expanded, ["docs/guide"]);
}

#[gpui_kit::test]
fn file_finder_jumps_to_file(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    let files = o.tab.read_with(o.shell.cx, |t, _| t.opened.files.clone());
    // Ranking: blank lists everything in diff order; a query ranks by
    // nucleo's score and drops what does not match.
    assert_eq!(finder::rank(&files, ""), [0, 1, 2, 3, 4, 5, 6]);
    assert_eq!(finder::rank(&files, "lib").first(), Some(&5));
    let mut ui = finder::rank(&files, "srcui");
    ui.sort();
    assert_eq!(ui, [3, 4]);
    assert!(finder::rank(&files, "zzz").is_empty());

    o.shell.cx.simulate_keystrokes("cmd-p");
    draw(o.shell.cx);
    assert!(o.shell.cx.update(|window, cx| window.has_active_dialog(cx)));
    let finder = o
        .shell
        .cx
        .update(|_, cx| finder::current(cx))
        .expect("the finder is open");
    let listed = finder.read_with(o.shell.cx, |s, _| s.delegate().matches().to_vec());
    assert_eq!(listed, [0, 1, 2, 3, 4, 5, 6]);

    o.shell.cx.simulate_input("button");
    draw(o.shell.cx);
    let (listed, selected) = finder.read_with(o.shell.cx, |s, _| {
        (
            s.delegate().matches().to_vec(),
            s.delegate().selected_file(),
        )
    });
    assert_eq!(listed, [3]);
    assert_eq!(selected, Some(3));
    o.shell.cx.simulate_keystrokes("enter");
    draw(o.shell.cx);
    assert!(!o.shell.cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(top_file(&mut o), 3);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(3)));
    // The diff has the keyboard.
    let tab = o.tab.clone();
    let focused = o
        .shell
        .cx
        .update(|window, cx| tab.read(cx).viewport_focus().is_focused(window));
    assert!(focused);

    // Escape closes it without moving.
    o.shell.cx.simulate_keystrokes("cmd-p");
    draw(o.shell.cx);
    o.shell.cx.simulate_input("readme");
    draw(o.shell.cx);
    o.shell.cx.simulate_keystrokes("escape");
    draw(o.shell.cx);
    assert!(!o.shell.cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(top_file(&mut o), 3);
}

#[gpui_kit::test]
fn file_finder_reselects_after_no_match(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = tree_repo();
    let mut o = open_tree(cx, &repo);
    let open = |o: &mut Opened| {
        o.shell.cx.simulate_keystrokes("cmd-p");
        draw(o.shell.cx);
        o.shell
            .cx
            .update(|_, cx| finder::current(cx))
            .expect("the finder is open")
    };
    let selected = |o: &mut Opened, finder: &finder::Finder| {
        finder.read_with(o.shell.cx, |s, _| {
            (s.selected_index().is_some(), s.delegate().selected_file())
        })
    };

    // A typo matches nothing (the list draws empty); fixing it with
    // backspace must select the first match again, so Enter jumps.
    let finder = open(&mut o);
    o.shell.cx.simulate_input("buttonz");
    draw(o.shell.cx);
    assert!(finder.read_with(o.shell.cx, |s, _| s.delegate().matches().is_empty()));
    o.shell.cx.simulate_keystrokes("backspace");
    draw(o.shell.cx);
    assert_eq!(selected(&mut o, &finder), (true, Some(3)));
    o.shell.cx.simulate_keystrokes("enter");
    draw(o.shell.cx);
    assert!(!o.shell.cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(top_file(&mut o), 3);

    // The same after replacing the whole query: `zzz`, then `lib`.
    let finder = open(&mut o);
    o.shell.cx.simulate_input("zzz");
    draw(o.shell.cx);
    assert_eq!(selected(&mut o, &finder).1, None);
    o.shell.cx.simulate_keystrokes("cmd-a");
    o.shell.cx.simulate_input("lib");
    draw(o.shell.cx);
    assert_eq!(selected(&mut o, &finder), (true, Some(5)));
    o.shell.cx.simulate_keystrokes("enter");
    draw(o.shell.cx);
    assert!(!o.shell.cx.update(|window, cx| window.has_active_dialog(cx)));
    assert_eq!(top_file(&mut o), 5);
    assert_eq!(selected_row(&mut o), Some(ItemId::File(5)));
}

/// Blobs are never read: the tree only needs the file list.
struct ListOnly(Arc<Vec<FileChange>>);

impl DiffProvider for ListOnly {
    fn object_format(&self) -> ObjectFormat {
        ObjectFormat::Sha1
    }
    fn files(&self) -> Arc<Vec<FileChange>> {
        self.0.clone()
    }
    fn load_blob(&self, _: &Oid) -> anyhow::Result<Arc<[u8]>> {
        Ok(Arc::from(&b""[..]))
    }
    fn blob_size(&self, _: &Oid) -> anyhow::Result<u64> {
        Ok(0)
    }
}

/// `n` changed files spread over nested directories, in git's path order.
fn synthetic_files(n: usize) -> Vec<FileChange> {
    let zero = Oid::zero(ObjectFormat::Sha1);
    let mut paths: Vec<String> = (0..n)
        .map(|i| {
            format!(
                "drivers/d{:02}/sub{:02}/deep/part{:02}/file{i:05}.c",
                i % 37,
                i % 11,
                i % 5
            )
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
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

#[gpui_kit::test]
fn tree_builds_13k_files_under_200ms(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    let files = Arc::new(synthetic_files(13_000));
    let (tree, elapsed) = shell.cx.update(|window, cx| {
        let provider: Arc<dyn DiffProvider> = Arc::new(ListOnly(files.clone()));
        let viewport =
            cx.new(|cx| DiffViewport::new(provider, ViewportOptions::default(), window, cx));
        let start = Instant::now();
        let tree = cx.new(|cx| FileTree::new(files.clone(), viewport, window, cx));
        (tree, start.elapsed())
    });
    eprintln!("13k-file tree built in {elapsed:?}");
    assert!(
        elapsed < Duration::from_millis(200),
        "building the tree took {elapsed:?}"
    );
    let (order, rows) = tree.read_with(shell.cx, |t, cx| {
        (t.model().file_order().len(), t.rows(cx).len())
    });
    assert_eq!(order, 13_000);
    // Every file and folder is a row (all expanded).
    let dirs = tree.read_with(shell.cx, |t, _| t.expanded_dirs().len());
    assert_eq!(rows, 13_000 + dirs);
    // Filtering all 13k with nucleo stays interactive too.
    let start = Instant::now();
    shell
        .cx
        .update(|window, cx| tree.update(cx, |t, cx| t.set_query("d07part3", window, cx)));
    let filtered = start.elapsed();
    eprintln!("13k-file fuzzy filter in {filtered:?}");
    assert!(
        filtered < Duration::from_millis(400),
        "filtering took {filtered:?}"
    );
}
