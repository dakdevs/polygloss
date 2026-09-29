//! Comment markdown (design §8.5, §8.7, §19 "Agent markdown"): every
//! comment body, human or agent, renders through gpui-kit's
//! `gpui_kit::base::TextView` (markdown-rs inside) with
//!
//! 1. raw HTML shown as literal text, never rendered ([`sanitize`]: a
//!    `MarkdownPlugin` for block and inline `mdast::Node::Html`, plus a
//!    pre-pass for the formatting tags gpui-kit pairs before plugins run),
//! 2. images turned into links and every image source replaced by a bundled
//!    placeholder, so nothing is fetched ([`sanitize::prepare`],
//!    [`sanitize::image_source`]),
//! 3. links opened only for `http`, `https` and `mailto` ([`open_link`]),
//! 4. fenced code highlighted with lumis ([`code_blocks`]),
//! 5. ` ```suggestion ` blocks as a mini-diff of the anchored new-side lines
//!    ([`suggestion`]; plain code on old-side and file anchors).
//!
//! Threads render bodies with [`render_markdown`] (plus
//! [`suggestion::with_suggestions`]); the composer's preview (T3.10) uses the
//! same function.

pub mod code_blocks;
pub mod sanitize;
pub mod suggestion;

use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{Hash as _, Hasher as _};

use gpui_kit::base::{TextView, TextViewState};
use gpui_kit::{App, AppContext as _, ElementId, Entity, SharedString};

pub use sanitize::{link_allowed, open_link};

use crate::settings::SettingsStore;

/// Nothing to register: every text view is configured where it is built
/// ([`configure`]), since gpui-kit re-installs its global text defaults on
/// every theme change and would drop a global highlighter (library-choices
/// §5).
pub fn init(_cx: &mut App) {}

/// A comment body as a text view: [`sanitize::prepare`]d (cached per body)
/// and [`configure`]d.
pub fn render_markdown(id: impl Into<ElementId>, body: &str, cx: &mut App) -> TextView {
    configure(TextView::markdown(id, prepared(body)), cx)
}

/// A text view state holding `body`, sanitized like [`render_markdown`]'s;
/// render it with `configure(TextView::new(&state), cx)`.
pub fn markdown_state(body: &str, cx: &mut App) -> Entity<TextViewState> {
    let body = prepared(body);
    cx.new(|cx| TextViewState::markdown(&body, cx))
}

/// Applies the comment rules to `view`: raw HTML as text, the image
/// placeholder, the link policy and lumis code highlighting.
pub fn configure(view: TextView, cx: &mut App) -> TextView {
    view.plugin(sanitize::HtmlBlockAsText)
        .plugin(sanitize::HtmlInlineAsText)
        .image_source(sanitize::image_source)
        .on_link_click(|url, _, _, cx| open_link(url, cx))
        .shared_code_block_highlighter(code_blocks::highlighter(cx))
}

/// The code font family and size (settings `buffer_font`), for code the
/// thread blocks draw themselves (snippets, suggestions).
pub fn code_font(cx: &App) -> (SharedString, f32) {
    match cx.try_global::<SettingsStore>() {
        Some(store) => {
            let font = &store.settings().buffer_font;
            (font.family.clone().into(), font.size)
        }
        None => (crate::theme::fonts::LILEX_FAMILY.into(), 13.0),
    }
}

/// Bodies prepared so far, by content (thread blocks render every frame).
const PREPARED_CAP: usize = 1024;

thread_local! {
    static PREPARED: RefCell<HashMap<u64, (String, SharedString)>> = RefCell::new(HashMap::new());
}

/// [`sanitize::prepare`] with a small per-thread cache.
fn prepared(body: &str) -> SharedString {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut h);
    let key = h.finish();
    PREPARED.with(|cache| {
        let mut cache = cache.borrow_mut();
        if let Some((src, out)) = cache.get(&key)
            && src == body
        {
            return out.clone();
        }
        if cache.len() >= PREPARED_CAP {
            cache.clear();
        }
        let out: SharedString = sanitize::prepare(body).into();
        cache.insert(key, (body.to_owned(), out.clone()));
        out
    })
}
