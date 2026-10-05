//! File categories (design §11.15, ADR-0028): named sets of path patterns
//! that move supporting files (tests, generated code, docs, …) out of the main
//! list. Pure: no git, IO or GPUI. Categories are computed from the path, the
//! file's `linguist-generated` attribute and the settings, and never stored.
//!
//! - [`catalog`]: the built-in categories ported from geld (MIT).
//! - [`config`]: the `categories` settings section.
//! - [`matcher`]: [`Categorizer`], the compiled patterns and the match order.

mod catalog;
mod config;
mod matcher;

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub use catalog::{BUILTINS, BuiltinDef, PatternGroup};
pub use config::{
    CategoriesConfig, CategorySettings, CustomCategory, unknown_groups, unknown_keys,
};
pub use matcher::Categorizer;

/// The built-in categories, in match order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum BuiltinCategory {
    Tests,
    Generated,
    Vendored,
    Agents,
    Docs,
    Tooling,
    Stories,
}

impl BuiltinCategory {
    /// Its catalog entry.
    pub fn def(self) -> &'static BuiltinDef {
        &BUILTINS[self as usize]
    }

    /// The settings key (`"tests"`).
    pub fn key(self) -> &'static str {
        self.def().key
    }
}

impl fmt::Display for BuiltinCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// A category: a built-in, or a custom one by its id. Displayed and
/// serialized as agents see it: `"tests"`, `"custom:tokens"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CategoryId {
    Builtin(BuiltinCategory),
    Custom(String),
}

impl fmt::Display for CategoryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CategoryId::Builtin(b) => f.write_str(b.key()),
            CategoryId::Custom(id) => write!(f, "custom:{id}"),
        }
    }
}

impl FromStr for CategoryId {
    type Err = String;

    fn from_str(s: &str) -> Result<CategoryId, String> {
        if let Some(id) = s.strip_prefix("custom:") {
            return Ok(CategoryId::Custom(id.to_owned()));
        }
        BUILTINS
            .iter()
            .find(|d| d.key == s)
            .map(|d| CategoryId::Builtin(d.id))
            .ok_or_else(|| format!("unknown category {s:?}"))
    }
}

impl Serialize for CategoryId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for CategoryId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<CategoryId, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

/// An enabled category as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CategoryInfo {
    pub id: CategoryId,
    pub title: String,
    /// A Lucide icon name (`assets/icons/<icon>.svg`).
    pub icon: String,
    /// The catalog entry, whose nouns label a built-in; `None` for custom.
    pub builtin: Option<&'static BuiltinDef>,
}

impl CategoryInfo {
    /// "1 test file", "12 test files"; custom "Design tokens · 12 files".
    pub fn label(&self, files: usize) -> String {
        let one = files == 1;
        match self.builtin {
            Some(def) => format!("{files} {}", if one { def.noun } else { def.noun_plural }),
            None => format!(
                "{} · {files} {}",
                self.title,
                if one { "file" } else { "files" }
            ),
        }
    }

    /// "1 test", "6 tests", "2 generated"; custom "12 design tokens".
    pub fn chip(&self, files: usize) -> String {
        match self.builtin {
            Some(def) => format!(
                "{files} {}",
                if files == 1 {
                    def.chip
                } else {
                    def.chip_plural
                }
            ),
            None => format!("{files} {}", self.title.to_lowercase()),
        }
    }
}

/// The icons a custom category may use (design §11.15).
pub const CUSTOM_ICONS: &[&str] = &[
    "tag",
    "layers",
    "package",
    "book-open",
    "wrench",
    "flask-conical",
    "file-cog",
    "bot",
    "languages",
    "folder",
    "file",
];

/// Where a verdict's pattern came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A catalog pattern of this group.
    BuiltIn { group: &'static str },
    /// `categories.<key>.patterns`, or `diff.generated_patterns` for Generated.
    Extra,
    /// A custom category's patterns.
    Custom,
    /// `linguist-generated` set (or recovered as set from a v1 row).
    Attribute,
}

/// Why a path is in a category.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub category: CategoryId,
    /// The pattern as written (`"*.test.*"`), or `linguist-generated`.
    pub pattern: String,
    pub source: Source,
}

/// [`Categorizer::explain`]'s answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Explain {
    Matched(Verdict),
    /// No category; `rescued_by` lists the rescues (as written, `!` included)
    /// that skipped a category on the way, in match order.
    Uncategorized {
        rescued_by: Vec<(CategoryId, String)>,
    },
}

/// What makes a `categories` section invalid (design §11.15). `category` and
/// the ids are spelled as agents see them (`"tests"`, `"custom:tokens"`).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CategoriesError {
    #[error("categories.{category}: pattern {pattern:?} does not compile: {message}")]
    Pattern {
        category: String,
        pattern: String,
        message: String,
    },
    #[error("categories.custom: id {0:?} must match ^[a-z][a-z0-9-]{{0,40}}$")]
    BadCustomId(String),
    #[error("categories.custom: id {0:?} is used twice or is a built-in key")]
    DuplicateId(String),
    #[error("categories.custom: {0:?} has an empty name")]
    EmptyName(String),
    #[error("categories.{0}: more than {max} patterns", max = matcher::MAX_PATTERNS)]
    TooManyPatterns(String),
}
