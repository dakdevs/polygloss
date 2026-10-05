//! The Files segment's footer (T6.11, design §11.5): "Total: +X −Y" from
//! the viewport's counts, "…" until they are all in, grouped by thousands,
//! following `CountsUpdated`.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{
    AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, WindowOptions, div, px,
};
use polygloss_app::keymap::actions::viewport as viewport_actions;
use polygloss_app::tree::FileTree;
use polygloss_diff::{FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, ObjectFormat, Oid};
use polygloss_viewport::{DiffProvider, DiffViewport, ViewportOptions};

use super::{lines, settle_counts};
use crate::shell::{bounds, compare_req, painted, start};
use crate::support::{FixtureRepo, Sandbox};
use crate::toolbar::shows;

/// "Total: +A −D" summed from `git diff --numstat <args>` of `repo`, the
/// footer's oracle (counts below 1,000, so no grouping).
fn numstat_total(repo: &FixtureRepo, args: &[&str]) -> String {
    let mut argv = vec!["diff", "--numstat"];
    argv.extend_from_slice(args);
    argv.extend(["refs/tags/base", "refs/tags/head"]);
    let (mut added, mut removed) = (0u64, 0u64);
    for line in repo.git(&argv).lines() {
        let mut cols = line.split('\t');
        // Binary files print `-`: they have no lines.
        added += cols.next().unwrap().parse::<u64>().unwrap_or(0);
        removed += cols.next().unwrap().parse::<u64>().unwrap_or(0);
    }
    assert!(added + removed < 1000, "the oracle does not group digits");
    format!("Total: +{added} −{removed}")
}

/// A repo with tags `base` and `head`: `a.rs` reindented only, `b.rs` with
/// one line changed and one added.
fn whitespace_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("a.rs", b"fn a() {\n    one();\n    two();\n}\n");
    repo.write("b.rs", b"fn b() {\n    one();\n}\n");
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("a.rs", b"fn a() {\n        one();\n        two();\n}\n");
    repo.write("b.rs", b"fn b() {\n    uno();\n    dos();\n}\n");
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

#[gpui_kit::test]
fn footer_totals_follow_counts_updated(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = whitespace_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    settle_counts(&mut shell, &tab, 2);
    let shown = numstat_total(&repo, &[]);
    let hidden = numstat_total(&repo, &["-w"]);
    assert_ne!(shown, hidden, "the fixture changes whitespace");
    assert!(shows(shell.cx, "tree-footer", &shown), "{shown}");

    // Hiding whitespace counts every file again; the footer follows the
    // new counts as they land (`CountsUpdated`), with no window refresh.
    shell.cx.dispatch_action(viewport_actions::ToggleWhitespace);
    shell.cx.run_until_parked();
    let ignored = tab.read_with(shell.cx, |t, cx| {
        t.viewport.read(cx).options().diff.ignore_whitespace
    });
    assert!(ignored);
    assert!(shows(shell.cx, "tree-footer", &hidden), "{hidden}");
}

/// `big.txt`: 3 lines replaced by 1,204.
fn thousands_repo() -> FixtureRepo {
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("big.txt", lines("old", 3).as_bytes());
    repo.commit("base");
    repo.git(&["tag", "base"]);
    repo.write("big.txt", lines("new", 1204).as_bytes());
    repo.commit("head");
    repo.git(&["tag", "head"]);
    repo
}

#[gpui_kit::test]
fn footer_groups_thousands(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = thousands_repo();
    let mut shell = start(cx);
    let tab = shell.open(compare_req(repo.path())).unwrap();
    settle_counts(&mut shell, &tab, 1);
    // Hand-written: 1,204 lines added and 3 removed, grouped by thousands
    // in the footer and in the row.
    assert!(shows(shell.cx, "tree-footer", "Total: +1,204 −3"));
    assert!(painted(shell.cx, "tree-stats-0: +1,204 −3").is_some());
    // The footer closes the sidebar.
    let footer = bounds(shell.cx, "tree-footer: Total: +1,204 −3");
    let pane = bounds(shell.cx, "file-tree-pane");
    assert_eq!(footer.bottom(), pane.bottom());
}

/// Blobs held in memory.
struct MemBlobs {
    files: Arc<Vec<FileChange>>,
    blobs: HashMap<Oid, Arc<[u8]>>,
}

impl DiffProvider for MemBlobs {
    fn object_format(&self) -> ObjectFormat {
        ObjectFormat::Sha1
    }
    fn files(&self) -> Arc<Vec<FileChange>> {
        self.files.clone()
    }
    fn load_blob(&self, oid: &Oid) -> anyhow::Result<Arc<[u8]>> {
        self.blobs
            .get(oid)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no blob {oid:?}"))
    }
    fn blob_size(&self, oid: &Oid) -> anyhow::Result<u64> {
        Ok(self.load_blob(oid)?.len() as u64)
    }
}

/// The blob id made of `c` repeated.
fn oid(c: char) -> Oid {
    Oid::parse(&c.to_string().repeat(40), ObjectFormat::Sha1).unwrap()
}

/// A text file at `path` from blob `old` to `new` (`None`: that side is
/// missing).
fn text_change(idx: u32, path: &str, old: Option<Oid>, new: Option<Oid>) -> FileChange {
    let zero = Oid::zero(ObjectFormat::Sha1);
    let at = Some(GitPath::from_bytes(path.as_bytes()));
    FileChange {
        idx,
        status: match (&old, &new) {
            (None, _) => FileStatus::Added,
            (_, None) => FileStatus::Deleted,
            _ => FileStatus::Modified,
        },
        old_path: old.as_ref().and(at.clone()),
        new_path: new.as_ref().and(at),
        old_mode: None,
        new_mode: None,
        old_blob: old.unwrap_or_else(|| zero.clone()),
        new_blob: new.unwrap_or(zero),
        similarity: None,
        kind: FileKind::Text,
        generated: false,
        generated_attr: GeneratedAttr::Unspecified,
    }
}

/// A tree beside its diff, as a review tab lays them out.
struct TreeBeside {
    tree: Entity<FileTree>,
    viewport: Entity<DiffViewport>,
}

impl Render for TreeBeside {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .child(div().w(px(280.)).h_full().child(self.tree.clone()))
            .child(div().flex_1().h_full().child(self.viewport.clone()))
    }
}

#[gpui_kit::test]
fn footer_shows_an_ellipsis_until_counted(cx: &mut TestAppContext) {
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    // Hand-counted: `a.txt` one line changed and one added (+2 −1),
    // `b.txt` added with two lines (+2 −0), `logo.png` binary (no lines).
    let blobs: HashMap<Oid, Arc<[u8]>> = HashMap::from([
        (oid('a'), Arc::from(&b"one\ntwo\nthree\n"[..])),
        (oid('b'), Arc::from(&b"one\n2\nthree\nfour\n"[..])),
        (oid('c'), Arc::from(&b"x\ny\n"[..])),
    ]);
    let mut png = text_change(2, "logo.png", Some(oid('d')), Some(oid('e')));
    png.kind = FileKind::Binary;
    let files = Arc::new(vec![
        text_change(0, "a.txt", Some(oid('a')), Some(oid('b'))),
        text_change(1, "b.txt", None, Some(oid('c'))),
        png,
    ]);
    let provider: Arc<dyn DiffProvider> = Arc::new(MemBlobs {
        files: files.clone(),
        blobs,
    });
    let (window, _) = shell
        .cx
        .update(|_, cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let viewport = cx
                    .new(|cx| DiffViewport::new(provider, ViewportOptions::default(), window, cx));
                let tree = cx.new(|cx| FileTree::new(files, viewport.clone(), window, cx));
                cx.new(|_| TreeBeside { tree, viewport })
            })
        })
        .expect("open the window");
    let cx = VisualTestContext::from_window(window, &shell.cx.cx).into_mut();
    // A frame before any background work ran: nothing is counted.
    cx.update(|window, _| window.refresh());
    assert!(shows(cx, "tree-footer", "Total: …"));
    assert!(painted(cx, "tree-stats-0: +2 −1").is_none());
    // The counts land: the footer and the rows show them; the binary file
    // adds nothing and is never waited for.
    cx.run_until_parked();
    assert!(shows(cx, "tree-footer", "Total: +4 −1"));
    assert!(painted(cx, "tree-stats-0: +2 −1").is_some());
    assert!(painted(cx, "tree-stats-1: +2 −0").is_some());
}
