//! Per-diff view state JSON v1 (T1.14, design §7.2 `view_state`, §11.12).
//!
//! Stored per `diff_id` as `{ "v": 1, "scroll_anchor": { "path", "side", "line" },
//! "collapsed": [path], "expanded": { path: [[start, end]] }, "layout": "split" |
//! "unified" | null, "tree_expanded": [dir], "composer": { key: text },
//! "threads_panel": bool, "open_sections": [category] }`. Missing fields load as
//! their defaults (a missing `tree_expanded`, `threads_panel` or `open_sections`
//! is `None`: never saved, unlike `[]` or `false`) and unknown fields are
//! ignored; a state with
//! another `v`, or one that does not parse, loads as `None` (ignored, never an
//! error). View state is UI state saved on change (debounced by the app), so it
//! appends no event.

use polygloss_diff::Side;
use polygloss_diff::rows::Layout;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::ids::DiffId;
use crate::review::{Core, CoreError};
use crate::store::events::now_ms;

/// The `view_state.state_json` version this build reads and writes.
pub const VIEW_STATE_VERSION: u32 = 1;

/// The restorable view of one diff (design §11.12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewState {
    /// Format version; [`VIEW_STATE_VERSION`]. Saving always writes this build's
    /// version.
    pub v: u32,
    /// The top visible line; never pixels.
    #[serde(default)]
    pub scroll_anchor: Option<ScrollAnchorState>,
    /// Collapsed files (display paths).
    #[serde(default)]
    pub collapsed: Vec<String>,
    /// Expanded context ranges per display path, `[start, end]` as the app
    /// records them (core stores them unchanged).
    #[serde(default)]
    pub expanded: BTreeMap<String, Vec<[u32; 2]>>,
    /// The manual split/unified choice; `None` = automatic by width.
    #[serde(default)]
    pub layout: Option<Layout>,
    /// Expanded directories of the file tree. `None` when the tree's
    /// expansion was never saved (left out of the JSON), so the tree keeps
    /// its default; `Some(vec![])` means every directory is collapsed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tree_expanded: Option<Vec<String>>,
    /// Unsaved composer text by composer key (autosave, design §8.3).
    #[serde(default)]
    pub composer: BTreeMap<String, String>,
    /// Whether the threads panel shows (design §11.1). `None` when it was
    /// never shown or hidden (left out of the JSON): the panel is hidden.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads_panel: Option<bool>,
    /// The category sections shown open (design §11.15), by category as
    /// agents spell it (`"tests"`, `"custom:tokens"`). `None` when the user
    /// never opened or closed one (left out of the JSON): the default rule
    /// decides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_sections: Option<Vec<String>>,
}

impl Default for ViewState {
    fn default() -> Self {
        ViewState {
            v: VIEW_STATE_VERSION,
            scroll_anchor: None,
            collapsed: Vec::new(),
            expanded: BTreeMap::new(),
            layout: None,
            tree_expanded: None,
            composer: BTreeMap::new(),
            threads_panel: None,
            open_sections: None,
        }
    }
}

/// A scroll position as a line, not pixels: `line` is the 1-based line of the
/// `side` blob of `path` (the store's line convention).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScrollAnchorState {
    pub path: String,
    pub side: Side,
    pub line: u32,
}

impl Core {
    /// The saved view state of `diff_id`, or `None` when there is none, it has
    /// another version, or it does not parse.
    pub fn load_view_state(&self, diff_id: &DiffId) -> Result<Option<ViewState>, CoreError> {
        let json: Option<String> = self.store.read(|c| {
            Ok(c.query_row(
                "SELECT state_json FROM view_state WHERE diff_id = ?1",
                [diff_id.as_str()],
                |r| r.get(0),
            )
            .optional()?)
        })?;
        Ok(json.and_then(|json| parse(diff_id, &json)))
    }

    /// Saves (replaces) the view state of `diff_id` as v1. `NotFound` when the
    /// diff is not stored (an unpinned live state has no `diffs` row).
    pub fn save_view_state(&self, diff_id: &DiffId, s: &ViewState) -> Result<(), CoreError> {
        let json = serde_json::to_string(&ViewState {
            v: VIEW_STATE_VERSION,
            ..s.clone()
        })
        .map_err(crate::store::StoreError::from)?;
        let saved = self.store.write(|tx| {
            if !tx
                .prepare_cached("SELECT 1 FROM diffs WHERE id = ?1")?
                .exists([diff_id.as_str()])?
            {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO view_state (diff_id, state_json, updated_at) VALUES (?1, ?2, ?3) \
                 ON CONFLICT (diff_id) DO UPDATE SET state_json = excluded.state_json, \
                   updated_at = excluded.updated_at",
                params![diff_id.as_str(), json, now_ms()],
            )?;
            Ok(true)
        })?;
        if saved {
            Ok(())
        } else {
            Err(CoreError::not_found("diff", diff_id.as_str()))
        }
    }
}

fn parse(diff_id: &DiffId, json: &str) -> Option<ViewState> {
    #[derive(Deserialize)]
    struct Version {
        v: Option<serde_json::Value>,
    }
    let version = match serde_json::from_str::<Version>(json) {
        Ok(Version { v }) => v,
        Err(e) => {
            tracing::warn!(diff_id = %diff_id, error = %e, "ignoring unreadable view state");
            return None;
        }
    };
    if version.as_ref().and_then(serde_json::Value::as_u64) != Some(u64::from(VIEW_STATE_VERSION)) {
        tracing::debug!(diff_id = %diff_id, ?version, "ignoring view state of another version");
        return None;
    }
    serde_json::from_str(json)
        .inspect_err(
            |e| tracing::warn!(diff_id = %diff_id, error = %e, "ignoring unreadable view state"),
        )
        .ok()
}
