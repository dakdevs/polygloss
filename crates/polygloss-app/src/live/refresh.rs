//! Refresh (design §10 "Apply" and "Refresh", ADR-0009): only when the user
//! clicks the banner or presses `R`. The tab opens its source again
//! (`Core::open`: a new snapshot for live, the refs again for compare,
//! which records the new iteration with `pinned_by = refresh`), then swaps
//! the new state into the same viewport ([`DiffViewport::set_provider`])
//! keeping the scroll anchor by line mapping (path + line through
//! [`LineMap`]), collapsed files, revealed context and the Viewed marks of
//! unchanged files. Unchanged files keep their loaded rows, so they do not
//! flash. Features that hold per-file state listen for [`DiffRefreshed`].
//!
//! [`DiffViewport::set_provider`]: polygloss_viewport::DiffViewport::set_provider

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::{Context, EventEmitter};
use polygloss_core::ids::DiffId;
use polygloss_diff::line_map::{LineMap, Mapped};
use polygloss_diff::{FileChange, Oid, Side};
use polygloss_viewport::{
    BlockAnchor, DiffProvider, DiffViewport, FileFlags, RowKey, ScrollAnchor,
};

use crate::review_tab::ReviewTab;

/// Emitted by a [`ReviewTab`] once a refresh swapped a new diff in: the tab's
/// `opened` and viewport show it now. `file_map[i]` is where old file `i`
/// is in the new list (`None`: gone). The viewport's flags are already
/// remapped (Viewed kept for unchanged files); host blocks (threads) were
/// dropped and are the listener's to place again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRefreshed {
    pub old_diff_id: DiffId,
    pub file_map: Arc<[Option<u32>]>,
}

impl EventEmitter<DiffRefreshed> for ReviewTab {}

/// How the files of two states correspond.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FileMap {
    /// Old index → new index (`None`: not in the new state).
    pub old_to_new: Vec<Option<u32>>,
    /// New index → old index, for files that did not change at all (their
    /// loaded data is reused).
    pub carry: Vec<Option<u32>>,
}

/// Matches files by path: the new file showing the same path, else the new
/// file renamed from it.
pub fn map_files(old: &[FileChange], new: &[FileChange]) -> FileMap {
    let mut by_path: HashMap<&str, u32> = HashMap::new();
    for (i, f) in new.iter().enumerate() {
        by_path.insert(f.display_path(), i as u32);
    }
    let mut renamed_from: HashMap<&str, u32> = HashMap::new();
    for (i, f) in new.iter().enumerate() {
        if let Some(p) = &f.old_path
            && f.new_path.is_some()
        {
            renamed_from.entry(p.text.as_str()).or_insert(i as u32);
        }
    }
    let mut map = FileMap {
        old_to_new: Vec::with_capacity(old.len()),
        carry: vec![None; new.len()],
    };
    for (j, f) in old.iter().enumerate() {
        let path = f.display_path();
        let to = by_path
            .get(path)
            .or_else(|| renamed_from.get(path))
            .copied();
        if let Some(i) = to
            && same_change(f, &new[i as usize])
        {
            map.carry[i as usize] = Some(j as u32);
        }
        map.old_to_new.push(to);
    }
    map
}

fn same_change(a: &FileChange, b: &FileChange) -> bool {
    a.status == b.status
        && a.old_path == b.old_path
        && a.new_path == b.new_path
        && a.old_mode == b.old_mode
        && a.new_mode == b.new_mode
        && a.old_blob == b.old_blob
        && a.new_blob == b.new_blob
        && a.kind == b.kind
        && a.generated == b.generated
}

/// What the view keeps across a refresh, read from it just before.
#[derive(Debug, Clone, PartialEq)]
pub struct Kept {
    pub anchor: ScrollAnchor,
    pub collapsed: Vec<u32>,
    /// Revealed old-side line ranges per file.
    pub expansions: Vec<(u32, Vec<[u32; 2]>)>,
    pub flags: Vec<FileFlags>,
}

impl Kept {
    /// Reads `viewport`. An anchor on a host block becomes the line the
    /// block sits below (blocks are placed again after the refresh). An
    /// anchor in the first shown file's lead is the top of the document
    /// (the prelude), whichever file holds it in display order: it is kept
    /// as file 0's lead, the top after the swap's identity order, which
    /// the re-partition's sections keep at the top.
    pub fn read(viewport: &DiffViewport) -> Kept {
        let doc = viewport.document();
        let mut anchor = viewport.anchor();
        if anchor.row == RowKey::Lead && anchor.file_idx == doc.top_anchor().file_idx {
            anchor.file_idx = 0;
        }
        if let RowKey::Block(id) = anchor.row {
            let metrics = doc.metrics();
            let placed = doc.blocks(anchor.file_idx).iter().find(|b| b.id == id);
            anchor = match placed.map(|b| b.anchor) {
                Some(BlockAnchor::Line { side, line }) => ScrollAnchor {
                    row: RowKey::Line { side, line },
                    offset_px: anchor.offset_px + metrics.row_height,
                    ..anchor
                },
                _ => ScrollAnchor {
                    row: RowKey::Header,
                    offset_px: anchor.offset_px + metrics.header_height,
                    ..anchor
                },
            };
        }
        Kept {
            anchor,
            collapsed: viewport.collapsed(),
            expansions: viewport.expansions(),
            flags: viewport.file_flags().to_vec(),
        }
    }
}

/// What to restore in the new state.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub map: FileMap,
    pub anchor: ScrollAnchor,
    pub collapsed: Vec<u32>,
    pub expansions: Vec<(u32, Vec<[u32; 2]>)>,
    pub flags: Vec<FileFlags>,
}

/// Maps what the view keeps onto the new files (background thread: reads
/// the blobs line mapping needs, old ones from `old_blobs`, new ones from
/// `new_blobs`).
pub fn plan(
    kept: &Kept,
    old: &[FileChange],
    new: &[FileChange],
    old_blobs: &dyn DiffProvider,
    new_blobs: &dyn DiffProvider,
) -> Plan {
    let map = map_files(old, new);
    let lines = Lines {
        old_blobs,
        new_blobs,
    };
    let new_idx = |j: u32| map.old_to_new.get(j as usize).copied().flatten();

    let anchor = map_anchor(kept.anchor, old, new, &map, &lines);
    let collapsed = kept.collapsed.iter().filter_map(|&j| new_idx(j)).collect();
    let expansions = kept
        .expansions
        .iter()
        .filter_map(|(j, ranges)| {
            let i = new_idx(*j)?;
            let (o, n) = (&old[*j as usize], &new[i as usize]);
            let ranges = if o.old_blob == n.old_blob {
                ranges.clone()
            } else {
                lines.map_ranges(&o.old_blob, &n.old_blob, ranges)?
            };
            (!ranges.is_empty()).then_some((i, ranges))
        })
        .collect();
    let mut flags = vec![FileFlags::default(); new.len()];
    for (j, f) in kept.flags.iter().enumerate() {
        let Some(i) = new_idx(j as u32) else {
            continue;
        };
        let same = map.carry[i as usize] == Some(j as u32)
            || old[j].viewed_key() == new[i as usize].viewed_key();
        flags[i as usize] = FileFlags {
            // Viewed holds exactly while the file's key is unchanged
            // (design §9); a changed file shows "changed since viewed".
            viewed: f.viewed && same,
            changed_since_viewed: if same {
                f.changed_since_viewed
            } else {
                f.viewed || f.changed_since_viewed
            },
            ..*f
        };
    }
    Plan {
        map,
        anchor,
        collapsed,
        expansions,
        flags,
    }
}

/// The scroll anchor in the new state: the top of the document stays the
/// top; else the same line of the same file (mapped through the file's line
/// diff when its blob changed), else the file's header, else the next file
/// still there.
fn map_anchor(
    anchor: ScrollAnchor,
    old: &[FileChange],
    new: &[FileChange],
    map: &FileMap,
    lines: &Lines<'_>,
) -> ScrollAnchor {
    let j = anchor.file_idx as usize;
    // The first file's lead (the prelude and the canvas above the first
    // card) belongs to the document, not to that file: whichever file is
    // first now, the header card stays where it was.
    if j == 0 && anchor.row == RowKey::Lead {
        return anchor;
    }
    let Some(i) = map.old_to_new.get(j).copied().flatten() else {
        // The file is gone: the top of the next file that is still there
        // (else the previous one); its lead from a lead, else its header.
        let next = map.old_to_new.iter().skip(j + 1).find_map(|m| *m);
        let prev = map.old_to_new.iter().take(j).rev().find_map(|m| *m);
        return ScrollAnchor {
            file_idx: next.or(prev).unwrap_or(0),
            row: if anchor.row == RowKey::Lead {
                RowKey::Lead
            } else {
                RowKey::Header
            },
            offset_px: 0.0,
        };
    };
    let header = ScrollAnchor {
        file_idx: i,
        row: RowKey::Header,
        offset_px: 0.0,
    };
    let (o, n) = (&old[j], &new[i as usize]);
    match anchor.row {
        RowKey::Line { side, line } => {
            let blob = |f: &FileChange| match side {
                Side::Old => f.old_blob.clone(),
                Side::New => f.new_blob.clone(),
            };
            let (from, to) = (blob(o), blob(n));
            if to.is_zero() {
                return header;
            }
            let line = if from == to {
                Some(line)
            } else {
                lines.map_line(&from, &to, line)
            };
            match line {
                Some(line) => ScrollAnchor {
                    file_idx: i,
                    row: RowKey::Line { side, line },
                    offset_px: anchor.offset_px,
                },
                None => header,
            }
        }
        RowKey::Lead | RowKey::Header | RowKey::Placeholder => ScrollAnchor {
            file_idx: i,
            ..anchor
        },
        // Gaps and blocks are per state (`Kept::read` turned blocks into
        // lines).
        RowKey::Gap(_) | RowKey::Block(_) => header,
    }
}

/// Blob reads for line mapping.
struct Lines<'a> {
    old_blobs: &'a dyn DiffProvider,
    new_blobs: &'a dyn DiffProvider,
}

impl Lines<'_> {
    fn line_map(&self, from: &Oid, to: &Oid) -> Option<LineMap> {
        let read = |p: &dyn DiffProvider, oid: &Oid| -> Option<Arc<[u8]>> {
            if oid.is_zero() {
                return Some(Arc::from(&[][..]));
            }
            p.load_blob(oid)
                .inspect_err(|e| tracing::debug!("reading {oid} to map lines: {e:#}"))
                .ok()
        };
        let a = read(self.old_blobs, from)?;
        let b = read(self.new_blobs, to)?;
        Some(LineMap::new(&a, &b))
    }

    /// Where line `line` of blob `from` is in blob `to`: the same line when
    /// unchanged, else the nearest one.
    fn map_line(&self, from: &Oid, to: &Oid, line: u32) -> Option<u32> {
        let map = self.line_map(from, to)?;
        Some(match map.map_line(line) {
            Mapped::Unchanged(l) => l,
            Mapped::Changed { nearest } => nearest,
        })
    }

    /// Old-side ranges (half-open) of blob `from` in blob `to`.
    fn map_ranges(&self, from: &Oid, to: &Oid, ranges: &[[u32; 2]]) -> Option<Vec<[u32; 2]>> {
        let map = self.line_map(from, to)?;
        let at = |l: u32| match map.map_line(l) {
            Mapped::Unchanged(l) => l,
            Mapped::Changed { nearest } => nearest,
        };
        Some(
            ranges
                .iter()
                .filter(|[s, e]| s < e)
                .map(|&[s, e]| [at(s), at(e - 1).saturating_add(1)])
                .filter(|[s, e]| s < e)
                .collect(),
        )
    }
}

/// Swaps `plan`'s restored state into `tab`'s viewport (main thread),
/// right after `tab.opened` became the new state, and tells the tab's
/// features ([`DiffRefreshed`]).
pub(crate) fn apply(
    tab: &mut ReviewTab,
    old_diff_id: DiffId,
    provider: Arc<dyn DiffProvider>,
    plan: Plan,
    cx: &mut Context<ReviewTab>,
) {
    let Plan {
        map,
        anchor,
        collapsed,
        expansions,
        flags,
    } = plan;
    tab.viewport.update(cx, |v, cx| {
        v.set_provider(provider, &map.carry, cx);
        for f in collapsed {
            v.set_collapsed(f, true, cx);
        }
        for (f, ranges) in &expansions {
            v.set_expansions(*f, ranges, cx);
        }
        v.set_file_flags(flags, cx);
        v.scroll_to_anchor(anchor, cx);
    });
    cx.emit(DiffRefreshed {
        old_diff_id,
        file_map: map.old_to_new.into(),
    });
    cx.notify();
}
