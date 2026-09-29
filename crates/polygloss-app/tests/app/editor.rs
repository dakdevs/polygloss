//! GPUI tests of T3.16: open in editor (design §11.13, ADR-0021) and `⌘,`
//! (open `settings.json`, §11.9 provisional). Every launch goes to a
//! recording spawner ([`editor::EditorHost`]); no real editor starts.

use std::ffi::OsString;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gpui_kit::{Entity, OwnedMenuItem};
use polygloss_app::editor::{self, EditorHost};
use polygloss_app::review_tab::ReviewTab;
use polygloss_app::settings::{Settings, SettingsStore};
use polygloss_core::git::{Since, Source};
use polygloss_core::review::OpenRequest;
use polygloss_core::store::events::Actor;
use polygloss_diff::{ObjectFormat, Side};
use polygloss_platform::editor::{EditorCommand, Spawner};
use polygloss_viewport::{CursorPos, ViewportEvent};

use crate::shell::{Shell, compare_req, draw, start};
use crate::support::{CONFIG_RS_BASE, CONFIG_RS_HEAD, FixtureRepo, Sandbox, code_change_repo};

/// Records every argv instead of starting anything; fails when told to.
#[derive(Default)]
struct Recorder {
    calls: Mutex<Vec<Vec<OsString>>>,
    fail: bool,
}

impl Spawner for Recorder {
    fn spawn(&self, argv: &[OsString]) -> io::Result<()> {
        self.calls.lock().unwrap().push(argv.to_vec());
        if self.fail {
            return Err(io::Error::new(io::ErrorKind::NotFound, "no such editor"));
        }
        Ok(())
    }
}

impl Recorder {
    fn calls(&self) -> Vec<Vec<OsString>> {
        self.calls.lock().unwrap().clone()
    }
}

/// Installs `recorder` as the spawner, with nothing auto-detected, and the
/// `editor.command` template `fake-editor {path}:{line}` (unless
/// `template` is `None`).
fn install(shell: &mut Shell, recorder: &Arc<Recorder>, template: Option<&str>) {
    let spawner: Arc<dyn Spawner> = recorder.clone();
    let template = template.map(str::to_owned);
    shell.cx.update(|_, cx| {
        editor::set_host(EditorHost::new(spawner, || None), cx);
        let mut settings = Settings::default();
        settings.editor.command = template;
        SettingsStore::set(settings, cx);
    });
}

/// The `(path, line)` of a single `fake-editor {path}:{line}` launch.
fn launched(recorder: &Recorder) -> (PathBuf, u32) {
    let calls = recorder.calls();
    assert_eq!(calls.len(), 1, "one launch: {calls:?}");
    let argv = &calls[0];
    assert_eq!(argv.len(), 2, "{argv:?}");
    assert_eq!(argv[0], "fake-editor");
    let arg = argv[1].to_str().expect("a UTF-8 argument");
    let (path, line) = arg.rsplit_once(':').expect("{path}:{line}");
    (PathBuf::from(path), line.parse().expect("a line number"))
}

fn set_cursor(shell: &mut Shell, tab: &Entity<ReviewTab>, file_idx: u32, side: Side, line: u32) {
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |v, cx| {
        v.set_cursor(
            Some(CursorPos {
                file_idx,
                side,
                line,
                range_start: None,
            }),
            cx,
        )
    });
    draw(shell.cx);
}

fn press(shell: &mut Shell, keys: &str) {
    shell.cx.simulate_keystrokes(keys);
    draw(shell.cx);
}

fn toasts(shell: &mut Shell) -> Vec<String> {
    shell.main.read_with(shell.cx, |m, _| {
        m.toasts().iter().map(|t| t.to_string()).collect()
    })
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).expect("canonicalize")
}

/// The blob copy `<cache>/blobs/<oid>/<basename>` must be read-only and
/// hold `content`.
fn assert_blob_copy(sb: &Sandbox, path: &Path, oid: &str, name: &str, content: &str) {
    let want = sb
        .home()
        .join("Library/Caches/polygloss/blobs")
        .join(oid)
        .join(name);
    assert_eq!(path, want);
    assert_eq!(std::fs::read_to_string(path).unwrap(), content);
    let mode = std::fs::metadata(path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o444, "{mode:o}");
}

#[gpui_kit::test]
fn o_on_new_side_maps_line_to_disk_content(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    // Since the review's head, three lines were added at the top of the
    // file on disk and `get` was rewritten.
    let edited = format!(
        "// one\n// two\n// three\n{}",
        CONFIG_RS_HEAD.replace("map(String::as_str)", "map(|v| v.as_str())")
    );
    repo.write("src/config.rs", edited.as_bytes());
    let mut shell = start(cx);
    let recorder = Arc::new(Recorder::default());
    install(&mut shell, &recorder, Some("fake-editor {path}:{line}"));
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let on_disk = canonical(&repo.path().join("src/config.rs"));

    // New line 8 (`pub fn parse`) is unchanged: 3 lines further down on
    // disk, 1-based for the editor.
    set_cursor(&mut shell, &tab, 0, Side::New, 8);
    press(&mut shell, "o");
    assert_eq!(launched(&recorder), (on_disk.clone(), 12));

    // A line changed on disk opens at the nearest line.
    let get = CONFIG_RS_HEAD
        .lines()
        .position(|l| l.contains("map(String::as_str)"))
        .unwrap() as u32;
    recorder.calls.lock().unwrap().clear();
    set_cursor(&mut shell, &tab, 0, Side::New, get);
    press(&mut shell, "o");
    assert_eq!(launched(&recorder), (on_disk, get + 3 + 1));
    assert!(toasts(&mut shell).is_empty(), "{:?}", toasts(&mut shell));
}

#[gpui_kit::test]
fn o_on_old_side_opens_readonly_blob_copy(cx: &mut gpui_kit::TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let recorder = Arc::new(Recorder::default());
    install(&mut shell, &recorder, Some("fake-editor {path}:{line}"));
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let old_blob = repo.oid("refs/tags/base:src/config.rs");

    set_cursor(&mut shell, &tab, 0, Side::Old, 4);
    press(&mut shell, "o");
    let (path, line) = launched(&recorder);
    assert_eq!(line, 5, "old-side lines are not mapped");
    assert_blob_copy(&sb, &path, old_blob.as_str(), "config.rs", CONFIG_RS_BASE);

    // Again: the existing copy is reused.
    recorder.calls.lock().unwrap().clear();
    press(&mut shell, "o");
    assert_eq!(launched(&recorder), (path, 5));
}

#[gpui_kit::test]
fn o_on_deleted_file_opens_blob_copy(cx: &mut gpui_kit::TestAppContext) {
    let sb = Sandbox::isolate();
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("keep.txt", b"keep\n");
    repo.write("gone.txt", b"first\nsecond\nthird\n");
    repo.commit("base");
    repo.git(&["tag", "base"]);
    std::fs::remove_file(repo.path().join("gone.txt")).unwrap();
    repo.write("keep.txt", b"keep\nmore\n");
    repo.write("new.txt", b"alpha\nbeta\n");
    repo.commit("head");
    repo.git(&["tag", "head"]);
    // The added file is gone from disk too: its new-side blob opens.
    std::fs::remove_file(repo.path().join("new.txt")).unwrap();
    let gone_blob = repo.oid("refs/tags/base:gone.txt");
    let new_blob = repo.oid("refs/tags/head:new.txt");

    let mut shell = start(cx);
    let recorder = Arc::new(Recorder::default());
    install(&mut shell, &recorder, Some("fake-editor {path}:{line}"));
    let tab = shell.open(compare_req(repo.path())).unwrap();
    let files: Vec<String> = tab.read_with(shell.cx, |t, _| {
        t.opened
            .files
            .iter()
            .map(|f| f.display_path().to_owned())
            .collect()
    });
    assert_eq!(files, ["gone.txt", "keep.txt", "new.txt"]);

    set_cursor(&mut shell, &tab, 0, Side::Old, 1);
    press(&mut shell, "o");
    let (path, line) = launched(&recorder);
    assert_eq!(line, 2);
    assert_blob_copy(
        &sb,
        &path,
        gone_blob.as_str(),
        "gone.txt",
        "first\nsecond\nthird\n",
    );

    recorder.calls.lock().unwrap().clear();
    set_cursor(&mut shell, &tab, 2, Side::New, 1);
    press(&mut shell, "o");
    let (path, line) = launched(&recorder);
    assert_eq!(line, 2);
    assert_blob_copy(&sb, &path, new_blob.as_str(), "new.txt", "alpha\nbeta\n");
}

#[gpui_kit::test]
fn live_review_opens_the_review_worktree(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    // (No `head` tag: on a case-insensitive disk it shadows `HEAD`.)
    let repo = FixtureRepo::init(ObjectFormat::Sha1);
    repo.write("src/main.rs", b"fn main() {}\n");
    repo.commit("base");
    let linked = repo.add_worktree("linked");
    std::fs::write(linked.join("src/main.rs"), "// live\nfn main() {}\n").unwrap();
    // The main worktree differs: the linked one is the review's.
    repo.write("src/main.rs", b"// main\n\n\nfn main() {}\n");
    let mut shell = start(cx);
    let recorder = Arc::new(Recorder::default());
    install(&mut shell, &recorder, Some("fake-editor {path}:{line}"));
    let tab = shell
        .open(OpenRequest {
            worktree: linked.clone(),
            source: Source::Live { since: Since::Head },
            label: None,
            pin: None,
            actor: Actor::human(),
        })
        .unwrap();
    let idx = tab.read_with(shell.cx, |t, _| {
        t.opened
            .files
            .iter()
            .position(|f| f.display_path() == "src/main.rs")
            .expect("src/main.rs changed") as u32
    });
    set_cursor(&mut shell, &tab, idx, Side::New, 1);
    press(&mut shell, "o");
    assert_eq!(
        launched(&recorder),
        (canonical(&linked.join("src/main.rs")), 2)
    );
}

#[gpui_kit::test]
fn header_menu_and_no_cursor_open_the_file(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let recorder = Arc::new(Recorder::default());
    install(&mut shell, &recorder, Some("fake-editor {path}:{line}"));
    let tab = shell.open(compare_req(repo.path())).unwrap();

    // Without a cursor, `o` opens the file at the top at its first line.
    press(&mut shell, "o");
    assert_eq!(
        launched(&recorder),
        (canonical(&repo.path().join("src/config.rs")), 1)
    );

    // The header ⋯ menu's "Open in editor" (a viewport event).
    recorder.calls.lock().unwrap().clear();
    let viewport = tab.read_with(shell.cx, |t, _| t.viewport.clone());
    viewport.update(shell.cx, |_, cx| {
        cx.emit(ViewportEvent::OpenInEditor {
            file_idx: 1,
            side: Side::New,
            line: 2,
        })
    });
    draw(shell.cx);
    assert_eq!(
        launched(&recorder),
        (canonical(&repo.path().join("src/greet.ts")), 3)
    );
}

#[gpui_kit::test]
fn editor_failures_show_a_toast(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let repo = code_change_repo();
    let mut shell = start(cx);
    let recorder = Arc::new(Recorder {
        fail: true,
        ..Recorder::default()
    });
    install(&mut shell, &recorder, Some("fake-editor {path}:{line}"));
    let tab = shell.open(compare_req(repo.path())).unwrap();
    set_cursor(&mut shell, &tab, 0, Side::New, 0);
    press(&mut shell, "o");
    assert_eq!(recorder.calls().len(), 1);
    let toasts = toasts(&mut shell);
    assert_eq!(toasts.len(), 1, "{toasts:?}");
    assert!(
        toasts[0].starts_with("Could not open the editor") && toasts[0].contains("no such editor"),
        "{toasts:?}"
    );
}

#[gpui_kit::test]
fn cmd_comma_opens_settings_file_creating_defaults(cx: &mut gpui_kit::TestAppContext) {
    let sb = Sandbox::isolate();
    let mut shell = start(cx);
    let recorder = Arc::new(Recorder::default());
    // No `editor.command`: the detected editor opens it.
    let spawner: Arc<dyn Spawner> = recorder.clone();
    shell.cx.update(|_, cx| {
        editor::set_host(
            EditorHost::new(spawner, || {
                Some(EditorCommand::from_template(
                    "detected-editor {path}:{line}",
                ))
            }),
            cx,
        )
    });
    let file = sb.config_dir().join("polygloss/settings.json");
    assert!(!file.exists());

    press(&mut shell, "cmd-,");
    let text = std::fs::read_to_string(&file).expect("settings.json created");
    assert_eq!(Settings::parse(&text).unwrap(), Settings::default());
    assert!(text.contains("\"editor\""), "every key is listed: {text}");
    let calls = recorder.calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let want = format!("{}:1", file.display());
    assert_eq!(calls[0], [OsString::from("detected-editor"), want.into()]);

    // An existing file is opened as it is.
    std::fs::write(&file, "{ \"diff\": { \"layout\": \"split\" } }\n").unwrap();
    press(&mut shell, "cmd-,");
    assert_eq!(recorder.calls().len(), 2);
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "{ \"diff\": { \"layout\": \"split\" } }\n"
    );
}

#[gpui_kit::test]
fn app_menu_has_settings_first(cx: &mut gpui_kit::TestAppContext) {
    let _sb = Sandbox::isolate();
    let shell = start(cx);
    // macOS order: Polygloss › Settings… (⌘,), then Services, then Quit.
    let items: Vec<String> = shell.cx.update(|_, cx| {
        let menus = cx.get_menus().expect("a menu bar");
        menus[0]
            .items
            .iter()
            .map(|item| match item {
                OwnedMenuItem::Action { name, action, .. } => {
                    format!("{name} → {}", action.name())
                }
                OwnedMenuItem::Separator => "—".to_owned(),
                OwnedMenuItem::SystemMenu(m) => m.name.to_string(),
                OwnedMenuItem::Submenu(m) => m.name.to_string(),
            })
            .collect()
    });
    assert_eq!(
        items,
        [
            "Settings… → window::OpenSettings",
            "—",
            "Services",
            "—",
            "Quit Polygloss → window::Quit"
        ]
    );
}
