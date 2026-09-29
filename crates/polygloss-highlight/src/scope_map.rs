//! Scope → style mapping (library-choices §4 "Colours", open question L3):
//! lumis reports nvim-treesitter capture names (`keyword.function`,
//! `punctuation.bracket.rust`, …); the active Zed theme's `syntax` map colors
//! them by **longest dotted prefix**, so any Zed theme recolors code.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use lumis::highlights::HIGHLIGHT_NAMES;

use crate::theme::{SyntaxStyle, ZedTheme};
use crate::tokens::StyleId;

/// Identifies the syntax styling of a [`SyntaxTheme`] (name, appearance and
/// every `syntax` entry) for cache keys. Stable within one build; never persist it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ThemeId(pub u64);

/// A Zed theme's `syntax` map compiled for lookups: one [`StyleId`] per key
/// (in key order, after [`StyleId::DEFAULT`]) and a precomputed style for
/// every lumis scope.
#[derive(Debug, Clone)]
pub struct SyntaxTheme {
    /// `styles[0]` is the default (empty) style.
    styles: Vec<SyntaxStyle>,
    scopes: HashMap<String, StyleId>,
    /// Style per `lumis::highlights::HIGHLIGHT_NAMES` index.
    lumis: Vec<StyleId>,
    id: ThemeId,
}

impl SyntaxTheme {
    pub fn from_zed(t: &ZedTheme) -> SyntaxTheme {
        let mut styles = vec![SyntaxStyle::default()];
        let mut scopes = HashMap::with_capacity(t.syntax.len());
        for (key, style) in t.syntax.iter().take(u16::MAX as usize) {
            scopes.insert(key.clone(), StyleId(styles.len() as u16));
            styles.push(style.clone());
        }
        let lumis = HIGHLIGHT_NAMES
            .iter()
            .map(|name| longest_prefix(&scopes, name))
            .collect();
        let mut hasher = DefaultHasher::new();
        (&t.name, t.appearance, &t.syntax).hash(&mut hasher);
        SyntaxTheme {
            styles,
            scopes,
            lumis,
            id: ThemeId(hasher.finish()),
        }
    }

    /// The style for `scope`: the `syntax` key that is its longest dotted
    /// prefix (`keyword.function.rust` → `keyword.function` → `keyword`), or
    /// [`StyleId::DEFAULT`].
    pub fn style_for_scope(&self, scope: &str) -> StyleId {
        longest_prefix(&self.scopes, scope)
    }

    /// The style behind `id`; the default style for ids this theme never issued.
    pub fn style(&self, id: StyleId) -> &SyntaxStyle {
        self.styles.get(id.0 as usize).unwrap_or(&self.styles[0])
    }

    /// Every style, indexed by `StyleId.0` (`[0]` is the default).
    pub fn styles(&self) -> &[SyntaxStyle] {
        &self.styles
    }

    pub fn id(&self) -> ThemeId {
        self.id
    }

    /// The style for lumis scope `scope_index` (an index into `HIGHLIGHT_NAMES`).
    pub(crate) fn lumis_style(&self, scope_index: usize) -> StyleId {
        self.lumis.get(scope_index).copied().unwrap_or_default()
    }
}

fn longest_prefix(scopes: &HashMap<String, StyleId>, scope: &str) -> StyleId {
    let mut candidate = scope;
    while !candidate.is_empty() {
        if let Some(&id) = scopes.get(candidate) {
            return id;
        }
        match candidate.rfind('.') {
            Some(dot) => candidate = &candidate[..dot],
            None => break,
        }
    }
    StyleId::DEFAULT
}
