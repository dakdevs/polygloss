//! Files whose body is not code rows, or not yet (design §6.4, §12.3):
//!
//! | File        | Body                                                        |
//! | ----------- | ----------------------------------------------------------- |
//! | Binary      | `Binary file · 12.0 KB → 14.2 KB` (sizes from `blob_size`)  |
//! | Submodule   | one row, `abc1234 → def5678`                                |
//! | Generated   | `Generated file` with "Load diff"                           |
//! | Large       | `Large diff · 20,125 changed lines` with "Load diff"        |
//! | Mode only   | nothing (the header's mode badge says it all)               |
//! | LFS pointer | the pointer as text; the header gets an `LFS` badge         |
//! | Load error  | `Could not load this file: …`                               |
//!
//! Blob sizes are read on the background executor, never on the main thread;
//! the label shows "Binary file" until they arrive.

use std::collections::{HashMap, HashSet};

use gpui_kit::{AppContext as _, Context, SharedString, Task};
use polygloss_diff::{FileChange, FileKind, Oid};

use crate::controls::ControlAction;
use crate::document::FileState;
use crate::materialize::MaterializedFile;
use crate::paint_rows::Painter;
use crate::view::DiffViewport;

/// What a body without code rows shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BodyLabel {
    pub text: SharedString,
    /// Followed by a "Load diff" link (large and generated files).
    pub load_diff: bool,
}

impl BodyLabel {
    pub(crate) fn plain(text: impl Into<SharedString>) -> BodyLabel {
        BodyLabel {
            text: text.into(),
            load_diff: false,
        }
    }
}

/// Blob sizes in bytes; `None` for a missing side or a failed read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BlobSizes {
    old: Option<u64>,
    new: Option<u64>,
}

/// Per-file facts for special bodies and header badges.
#[derive(Default)]
pub(crate) struct Specials {
    sizes: HashMap<u32, BlobSizes>,
    size_reads: HashMap<u32, Task<()>>,
    /// Files seen to hold a git-lfs pointer (kept after eviction).
    lfs: HashSet<u32>,
    /// Large or generated files the user asked to see.
    load_requested: HashSet<u32>,
}

impl Specials {
    pub(crate) fn load_requested(&self, f: u32) -> bool {
        self.load_requested.contains(&f)
    }

    pub(crate) fn is_lfs(&self, f: u32) -> bool {
        self.lfs.contains(&f)
    }

    /// Remembers whether materialized file `f` is a git-lfs pointer.
    pub(crate) fn note_lfs(&mut self, f: u32, file: &MaterializedFile) {
        if is_lfs_pointer(&file.new_text) || is_lfs_pointer(&file.old_text) {
            self.lfs.insert(f);
        }
    }

    /// The body of a file drawn from metadata alone (binary, submodule, a
    /// generated file not loaded on request), or `None` for code rows.
    pub(crate) fn body_label(&self, f: u32, change: &FileChange) -> Option<BodyLabel> {
        match change.kind {
            FileKind::Binary => Some(BodyLabel::plain(binary_label(
                change,
                self.sizes.get(&f).copied(),
            ))),
            FileKind::Submodule => Some(BodyLabel::plain(submodule_label(change))),
            _ if change.generated && !self.load_requested(f) => Some(BodyLabel {
                text: SharedString::new_static("Generated file"),
                load_diff: true,
            }),
            _ => None,
        }
    }
}

/// Whether a file's body comes from its blobs: text and symlinks whose
/// content changed, generated files only once requested. Binary, submodule
/// and content-equal changes are drawn from metadata alone.
pub(crate) fn needs_blobs(change: &FileChange, load_requested: bool) -> bool {
    matches!(change.kind, FileKind::Text | FileKind::Symlink)
        && (!change.generated || load_requested)
        && change.old_blob != change.new_blob
}

/// The placeholder of a diff over the "Load diff" threshold.
pub(crate) fn large_label(changed: u32) -> BodyLabel {
    BodyLabel {
        text: format!("Large diff · {} changed lines", group_thousands(changed)).into(),
        load_diff: true,
    }
}

fn binary_label(change: &FileChange, sizes: Option<BlobSizes>) -> String {
    const BINARY: &str = "Binary file";
    let Some(sizes) = sizes else {
        return BINARY.to_owned();
    };
    let old = (!change.old_blob.is_zero()).then_some(sizes.old);
    let new = (!change.new_blob.is_zero()).then_some(sizes.new);
    match (old, new) {
        (Some(Some(o)), Some(Some(n))) => {
            format!("{BINARY} · {} → {}", format_size(o), format_size(n))
        }
        (None, Some(Some(n))) => format!("{BINARY} · {}", format_size(n)),
        (Some(Some(o)), None) => format!("{BINARY} · {}", format_size(o)),
        _ => BINARY.to_owned(),
    }
}

fn submodule_label(change: &FileChange) -> String {
    match (change.old_blob.is_zero(), change.new_blob.is_zero()) {
        (false, false) => format!("{} → {}", change.old_blob.short(), change.new_blob.short()),
        (true, false) => format!("Submodule added at {}", change.new_blob.short()),
        (false, true) => format!("Submodule removed (was {})", change.old_blob.short()),
        (true, true) => "Submodule".to_owned(),
    }
}

/// A byte count for people: `900 B`, `12.0 KB`, `3.3 MB` (binary units, one
/// decimal).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    // Also move up when one decimal would round to 1024.0.
    while value >= 1023.95 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// `20125` → `20,125`.
fn group_thousands(n: u32) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Whether `blob` is a git-lfs pointer file: under 1 KiB, starting with the
/// spec's `version` line and holding `oid` and `size` lines (design §6.4,
/// **Provisional**).
pub fn is_lfs_pointer(blob: &[u8]) -> bool {
    const VERSIONS: [&[u8]; 2] = [
        b"version https://git-lfs.github.com/spec/",
        b"version https://hawser.github.com/spec/",
    ];
    if blob.len() >= 1024 || !VERSIONS.iter().any(|v| blob.starts_with(v)) {
        return false;
    }
    let (mut oid, mut size) = (false, false);
    for line in blob.split(|&b| b == b'\n') {
        oid |= line.starts_with(b"oid ");
        size |= line
            .strip_prefix(b"size ")
            .is_some_and(|n| !n.is_empty() && n.iter().all(u8::is_ascii_digit));
    }
    oid && size
}

impl DiffViewport {
    /// Shows file `file_idx`'s diff although it is large or generated (the
    /// "Load diff" link and menu item). A generated file loads; a large one,
    /// already loaded, gets its rows; one not loaded yet shows its rows
    /// however large it turns out to be. Nothing above it moves. Ignored for
    /// files with no diff to show (binary, submodule, content unchanged) and
    /// for a file already shown in full.
    pub fn load_diff(&mut self, file_idx: u32, cx: &mut Context<Self>) {
        let Some(change) = self.files.get(file_idx as usize) else {
            return;
        };
        let shown_in_full = !change.generated
            && match self.doc.state(file_idx) {
                FileState::Materialized(file) => {
                    file.diff.additions + file.diff.deletions <= self.opts.large_file_changed_lines
                }
                _ => false,
            };
        if !needs_blobs(change, true)
            || shown_in_full
            || !self.special.load_requested.insert(file_idx)
        {
            return;
        }
        // "Loading…" until its rows are there.
        self.labels[file_idx as usize] = None;
        self.relayout(file_idx);
        cx.notify();
    }

    /// Whether file `f` shows a "Load diff" link.
    pub(crate) fn can_load_diff(&self, f: u32) -> bool {
        self.labels
            .get(f as usize)
            .and_then(Option::as_ref)
            .is_some_and(|l| l.load_diff)
    }

    /// Starts reading binary file `f`'s blob sizes in the background, once.
    pub(crate) fn request_blob_sizes(&mut self, f: u32, cx: &mut Context<Self>) {
        let change = &self.files[f as usize];
        if change.kind != FileKind::Binary
            || self.special.sizes.contains_key(&f)
            || self.special.size_reads.contains_key(&f)
        {
            return;
        }
        let provider = self.provider.clone();
        let (old, new) = (change.old_blob.clone(), change.new_blob.clone());
        let task = cx.spawn(async move |this, cx| {
            let sizes = cx
                .background_spawn(async move {
                    let size = |oid: &Oid| {
                        if oid.is_zero() {
                            None
                        } else {
                            provider.blob_size(oid).ok()
                        }
                    };
                    BlobSizes {
                        old: size(&old),
                        new: size(&new),
                    }
                })
                .await;
            this.update(cx, |v, cx| {
                v.special.size_reads.remove(&f);
                v.special.sizes.insert(f, sizes);
                v.labels[f as usize] = v.special.body_label(f, &v.files[f as usize]);
                cx.notify();
            })
            .ok();
        });
        self.special.size_reads.insert(f, task);
    }
}

impl Painter<'_> {
    /// A body that is one message: a special file's label, "Loading…" while
    /// its data is on its way, and "Load diff" when it can be shown anyway.
    pub(crate) fn placeholder_row(&mut self, f: u32, y: f32, h: f32) {
        // No label: the file is reloading and its old one (an error, a
        // large-diff count) is stale.
        let Some(label) = self.file_labels[f as usize].clone() else {
            self.count_loading();
            self.label_at(f, "Loading…", y, h);
            return;
        };
        let right = self.label_at(f, &label.text, y, h);
        if label.load_diff {
            let x = right + 2.0 * self.geometry.advance;
            self.link(ControlAction::LoadDiff(f), "Load diff", x, y, h);
        }
    }
}
