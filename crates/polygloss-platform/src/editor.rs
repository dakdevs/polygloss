//! Open in editor: detecting the user's editor, `editor.command` templates
//! and launching (design §11.13, ADR-0021, OQ-21).
//!
//! - [`detect`] looks for Zed, Cursor and VS Code (a CLI shim on the search
//!   path, else the CLI inside the app bundle), then `$VISUAL`, then
//!   `$EDITOR` (provisional order, §11.13).
//! - [`EditorCommand::from_template`] reads an `editor.command` template
//!   such as `zed {path}:{line}` or `code -g {path}:{line}`: words split like
//!   a shell would (quotes and backslashes group them) but nothing is ever
//!   expanded or run by a shell.
//! - [`EditorCommand::launch`] hands the argv to a [`Spawner`] (injectable,
//!   so tests never start a real editor). Terminal editors (vim, nano, …)
//!   run in the default terminal app through a temporary, self-deleting
//!   `.command` file (OQ-21), the only place a shell sees the arguments, each
//!   single-quoted.

use std::ffi::{OsStr, OsString};
use std::io::{self, Write as _};
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

/// Starts a program from an argv (`argv[0]` is the program). The app uses
/// [`ProcessSpawner`]; tests record the argv instead.
pub trait Spawner: Send + Sync {
    /// Starts `argv` detached; returns once it started (never waits for it
    /// to end).
    fn spawn(&self, argv: &[OsString]) -> io::Result<()>;
}

/// Starts real processes: no shell, stdio closed, reaped on a thread.
pub struct ProcessSpawner;

impl Spawner for ProcessSpawner {
    fn spawn(&self, argv: &[OsString]) -> io::Result<()> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "empty editor command"))?;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        // Editor CLIs return quickly; reap them so no zombie stays behind.
        std::thread::Builder::new()
            .name("polygloss-editor-wait".into())
            .spawn(move || {
                let _ = child.wait();
            })?;
        Ok(())
    }
}

/// `open`, which starts `.command` files in the default terminal app and
/// [`EditorCommand::system_default`].
const OPEN: &str = "/usr/bin/open";

/// Editors that need a terminal (by program name): they run through a
/// `.command` file.
const TERMINAL_EDITORS: &[&str] = &[
    "vi", "vim", "nvim", "nano", "pico", "emacs", "micro", "hx", "helix", "kak", "joe", "ne", "mg",
    "jed",
];

/// An editor as an argv template: words holding `{path}` and `{line}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorCommand {
    words: Vec<OsString>,
    terminal: bool,
}

impl EditorCommand {
    /// An editor from argv words (`{path}`/`{line}` placeholders); a
    /// command without `{path}` gets it appended. Whether it needs a terminal
    /// follows from the program's name.
    pub fn new(mut words: Vec<OsString>) -> EditorCommand {
        if !words.is_empty() && !words.iter().any(|w| contains(w, b"{path}")) {
            words.push("{path}".into());
        }
        let terminal = words
            .first()
            .map(|p| TERMINAL_EDITORS.contains(&basename(p).as_str()))
            .unwrap_or(false);
        EditorCommand { words, terminal }
    }

    /// Reads an `editor.command` template (`zed {path}:{line}`): split into
    /// words at unquoted whitespace; `'…'` is literal, `"…"` honors `\"` and
    /// `\\`, and `\` outside quotes escapes the next character. Nothing else
    /// is interpreted.
    pub fn from_template(t: &str) -> EditorCommand {
        EditorCommand::new(split_words(t).into_iter().map(OsString::from).collect())
    }

    /// The editor macOS opens plain text with (`open -t`), for when nothing
    /// was detected or configured. It cannot go to a line.
    pub fn system_default() -> EditorCommand {
        EditorCommand::new(vec![OPEN.into(), "-t".into(), "{path}".into()])
    }

    /// This command with a bare program name (no `/`) replaced by the first
    /// executable of that name in `search_path`, if any: an app started
    /// from the Finder has a minimal `PATH` (see [`DetectEnv::from_process`]).
    pub fn resolved(mut self, search_path: &[PathBuf]) -> EditorCommand {
        if let Some(program) = self.words.first_mut()
            && !program.as_bytes().contains(&b'/')
            && let Some(found) = program
                .to_str()
                .and_then(|name| find_on_path(name, search_path))
        {
            *program = found.into_os_string();
        }
        self
    }

    /// The program (`argv[0]`), if any.
    pub fn program(&self) -> Option<&OsStr> {
        self.words.first().map(OsString::as_os_str)
    }

    /// Whether the editor runs in a terminal (vim, nano, …).
    pub fn is_terminal(&self) -> bool {
        self.terminal
    }

    /// The editor's argv for `path` at 1-based `line`, placeholders
    /// substituted inside their words (never split or interpreted).
    pub fn argv(&self, path: &Path, line: u32) -> Vec<OsString> {
        let line = line.to_string();
        self.words
            .iter()
            .map(|w| {
                let bytes = replace(w.as_bytes(), b"{path}", path.as_os_str().as_bytes());
                OsString::from_vec(replace(&bytes, b"{line}", line.as_bytes()))
            })
            .collect()
    }

    /// Opens `path` at 1-based `line`: the argv through `spawner`, or, for a
    /// terminal editor, a `.command` file written to `scratch_dir` and opened
    /// with the default terminal app.
    pub fn launch(
        &self,
        path: &Path,
        line: u32,
        spawner: &dyn Spawner,
        scratch_dir: &Path,
    ) -> io::Result<()> {
        let argv = self.argv(path, line);
        if argv.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "empty editor command",
            ));
        }
        if self.terminal {
            let file = write_command_file(&argv, scratch_dir)?;
            spawner.spawn(&[OPEN.into(), file.into_os_string()])
        } else {
            spawner.spawn(&argv)
        }
    }
}

/// Writes a self-deleting `.command` script (mode `0700`) to `dir` that
/// `exec`s `argv`, every word single-quoted. Returns its path.
pub fn write_command_file(argv: &[OsString], dir: &Path) -> io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let mut script = b"#!/bin/sh\n# Opened by Polygloss (open in editor); removes itself.\nrm -f -- \"$0\"\nexec".to_vec();
    for word in argv {
        script.push(b' ');
        script.extend(shell_quote(word));
    }
    script.push(b'\n');
    loop {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = dir.join(format!("open-{}-{n}.command", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&path)
        {
            Ok(mut f) => {
                f.write_all(&script)?;
                // `mode` is filtered by the umask.
                f.set_permissions(std::fs::Permissions::from_mode(0o700))?;
                return Ok(path);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
}

/// `arg` single-quoted for `/bin/sh` (`'` becomes `'\''`); any bytes but
/// NUL survive.
pub fn shell_quote(arg: &OsStr) -> Vec<u8> {
    let mut out = vec![b'\''];
    for &b in arg.as_bytes() {
        if b == b'\'' {
            out.extend_from_slice(b"'\\''");
        } else {
            out.push(b);
        }
    }
    out.push(b'\'');
    out
}

/// Where [`detect_in`] looks.
#[derive(Debug, Clone, Default)]
pub struct DetectEnv {
    /// Directories searched for CLI shims and bare `$VISUAL`/`$EDITOR`
    /// programs, in order.
    pub search_path: Vec<PathBuf>,
    /// Directories holding `.app` bundles (`/Applications`, …).
    pub app_dirs: Vec<PathBuf>,
    /// `$VISUAL`.
    pub visual: Option<OsString>,
    /// `$EDITOR`.
    pub editor: Option<OsString>,
}

impl DetectEnv {
    /// This process's environment: `$PATH` plus the usual shim directories
    /// (an app started from the Finder has a minimal `PATH`), and
    /// `/Applications` and `~/Applications`.
    pub fn from_process() -> DetectEnv {
        let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let mut search_path: Vec<PathBuf> = var("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let extra = ["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from);
        for dir in extra
            .into_iter()
            .chain(home.iter().map(|h| h.join(".local/bin")))
        {
            if !search_path.contains(&dir) {
                search_path.push(dir);
            }
        }
        let mut app_dirs = vec![PathBuf::from("/Applications")];
        app_dirs.extend(home.map(|h| h.join("Applications")));
        DetectEnv {
            search_path,
            app_dirs,
            visual: var("VISUAL"),
            editor: var("EDITOR"),
        }
    }
}

/// A known GUI editor: its CLI shim, the CLIs inside its bundles, and the
/// arguments after the program.
struct Known {
    shim: &'static str,
    bundles: &'static [&'static str],
    args: &'static [&'static str],
}

/// Zed, Cursor, VS Code, in detection order.
const KNOWN: &[Known] = &[
    Known {
        shim: "zed",
        bundles: &[
            "Zed.app/Contents/MacOS/cli",
            "Zed Preview.app/Contents/MacOS/cli",
        ],
        args: &["{path}:{line}"],
    },
    Known {
        shim: "cursor",
        bundles: &["Cursor.app/Contents/Resources/app/bin/cursor"],
        args: &["-g", "{path}:{line}"],
    },
    Known {
        shim: "code",
        bundles: &["Visual Studio Code.app/Contents/Resources/app/bin/code"],
        args: &["-g", "{path}:{line}"],
    },
];

/// The user's editor from this process's environment ([`detect_in`] over
/// [`DetectEnv::from_process`]).
pub fn detect() -> Option<EditorCommand> {
    detect_in(&DetectEnv::from_process())
}

/// Zed, Cursor, VS Code (shim, else bundle), then `$VISUAL`, then
/// `$EDITOR`; `None` when there is none.
pub fn detect_in(env: &DetectEnv) -> Option<EditorCommand> {
    for known in KNOWN {
        let program = find_on_path(known.shim, &env.search_path).or_else(|| {
            env.app_dirs
                .iter()
                .flat_map(|dir| known.bundles.iter().map(move |b| dir.join(b)))
                .find(|p| is_executable(p))
        });
        if let Some(program) = program {
            let mut words = vec![program.into_os_string()];
            words.extend(known.args.iter().map(OsString::from));
            return Some(EditorCommand::new(words));
        }
    }
    [&env.visual, &env.editor]
        .into_iter()
        .flatten()
        .find_map(|value| env_editor(value, &env.search_path))
}

/// An editor from a `$VISUAL`/`$EDITOR` value (`nvim -p`, `code --wait`):
/// its program resolved on the search path, its own flags, then the line
/// syntax its name calls for.
fn env_editor(value: &OsStr, search_path: &[PathBuf]) -> Option<EditorCommand> {
    let mut words: Vec<OsString> = match value.to_str() {
        Some(text) => split_words(text).into_iter().map(OsString::from).collect(),
        None => vec![value.to_owned()],
    };
    let name = basename(words.first()?);
    if !words.iter().any(|w| contains(w, b"{path}")) {
        words.extend(line_args(&name).iter().map(OsString::from));
    }
    Some(EditorCommand::new(words).resolved(search_path))
}

/// How editor `name` is told to open a file at a line.
fn line_args(name: &str) -> &'static [&'static str] {
    match name {
        "vi" | "vim" | "nvim" | "gvim" | "mvim" | "nano" | "pico" | "emacs" | "emacsclient"
        | "micro" | "kak" | "joe" | "ne" | "mg" | "jed" | "bbedit" => &["+{line}", "{path}"],
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" => &["-g", "{path}:{line}"],
        "zed" | "subl" | "hx" | "helix" | "nova" => &["{path}:{line}"],
        "mate" => &["-l", "{line}", "{path}"],
        _ => &["{path}"],
    }
}

/// The first executable `name` in `dirs`.
fn find_on_path(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    dirs.iter().map(|d| d.join(name)).find(|p| is_executable(p))
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The last component of `program`, lossily.
fn basename(program: &OsStr) -> String {
    Path::new(program)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn contains(word: &OsStr, needle: &[u8]) -> bool {
    word.as_bytes().windows(needle.len()).any(|w| w == needle)
}

/// `haystack` with every `from` replaced by `to`.
fn replace(haystack: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(from) {
            out.extend_from_slice(to);
            i += from.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

/// Splits a template into words (see [`EditorCommand::from_template`]).
fn split_words(t: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = t.chars();
    while let Some(c) = chars.next() {
        match c {
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut word));
                    in_word = false;
                }
            }
            '\'' => {
                in_word = true;
                for c in chars.by_ref() {
                    if c == '\'' {
                        break;
                    }
                    word.push(c);
                }
            }
            '"' => {
                in_word = true;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some(e @ ('"' | '\\')) => word.push(e),
                            Some(other) => {
                                word.push('\\');
                                word.push(other);
                            }
                            None => word.push('\\'),
                        },
                        c => word.push(c),
                    }
                }
            }
            '\\' => {
                in_word = true;
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            }
            c => {
                in_word = true;
                word.push(c);
            }
        }
    }
    if in_word {
        words.push(word);
    }
    words
}
