//! The `categories` section of `settings.json` (design §11.15, §18).
//!
//! A built-in category object may leave keys out; each one left out keeps that
//! category's default (`"generated": { "patterns": [...] }` still has
//! `build-output` disabled). So the section deserializes through `Raw*`
//! structs of optional fields, merged over the [`BuiltinDef`] defaults, whose
//! `#[serde(flatten)]` maps also collect the keys that mean nothing
//! ([`unknown_keys`]).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::catalog::BUILTINS;
use super::{BuiltinCategory, BuiltinDef};

/// One built-in category's settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CategorySettings {
    pub enabled: bool,
    /// Group keys of this category that do not match (`"build-output"`).
    pub disabled_groups: Vec<String>,
    /// Extra patterns; `!` lines are rescues.
    pub patterns: Vec<String>,
}

impl CategorySettings {
    /// The category's defaults (design §11.15).
    pub fn defaults(category: BuiltinCategory) -> CategorySettings {
        let def = category.def();
        CategorySettings {
            enabled: def.default_enabled,
            disabled_groups: def
                .default_disabled_groups
                .iter()
                .map(|g| (*g).to_owned())
                .collect(),
            patterns: Vec::new(),
        }
    }
}

/// A user-defined category, matched before the built-ins in settings order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomCategory {
    /// `^[a-z][a-z0-9-]{0,40}$`; agents see `custom:<id>`.
    pub id: String,
    pub name: String,
    /// One of [`super::CUSTOM_ICONS`]; empty (left out) means `tag`.
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default = "enabled")]
    pub enabled: bool,
}

fn enabled() -> bool {
    true
}

/// The `categories` section. `Default` is design §11.15's table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "RawCategories")]
pub struct CategoriesConfig {
    pub tests: CategorySettings,
    pub generated: CategorySettings,
    pub vendored: CategorySettings,
    pub agents: CategorySettings,
    pub docs: CategorySettings,
    pub tooling: CategorySettings,
    pub stories: CategorySettings,
    pub custom: Vec<CustomCategory>,
    /// Keys that mean nothing, as settings paths (`categories.tests.enable`),
    /// for [`unknown_keys`]. Never serialized.
    #[serde(skip)]
    pub unknown: Vec<String>,
}

impl Default for CategoriesConfig {
    fn default() -> CategoriesConfig {
        CategoriesConfig::from(RawCategories::default())
    }
}

impl CategoriesConfig {
    pub fn get(&self, category: BuiltinCategory) -> &CategorySettings {
        match category {
            BuiltinCategory::Tests => &self.tests,
            BuiltinCategory::Generated => &self.generated,
            BuiltinCategory::Vendored => &self.vendored,
            BuiltinCategory::Agents => &self.agents,
            BuiltinCategory::Docs => &self.docs,
            BuiltinCategory::Tooling => &self.tooling,
            BuiltinCategory::Stories => &self.stories,
        }
    }

    pub fn get_mut(&mut self, category: BuiltinCategory) -> &mut CategorySettings {
        match category {
            BuiltinCategory::Tests => &mut self.tests,
            BuiltinCategory::Generated => &mut self.generated,
            BuiltinCategory::Vendored => &mut self.vendored,
            BuiltinCategory::Agents => &mut self.agents,
            BuiltinCategory::Docs => &mut self.docs,
            BuiltinCategory::Tooling => &mut self.tooling,
            BuiltinCategory::Stories => &mut self.stories,
        }
    }
}

/// Group keys in `disabled_groups` that are not groups of their category, as
/// `"<category>/<group>"`. Logged by callers, never an error.
pub fn unknown_groups(cfg: &CategoriesConfig) -> Vec<String> {
    BUILTINS
        .iter()
        .flat_map(|def| {
            cfg.get(def.id)
                .disabled_groups
                .iter()
                .filter(|g| !def.groups.iter().any(|known| known.key == g.as_str()))
                .map(|g| format!("{}/{g}", def.key))
        })
        .collect()
}

/// Keys in `categories` or in a category object that mean nothing
/// (`categories.test`, `categories.tests.enable`). Logged by callers, never an
/// error.
pub fn unknown_keys(cfg: &CategoriesConfig) -> Vec<String> {
    cfg.unknown.clone()
}

type Unknown = BTreeMap<String, serde_json::Value>;

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawCategories {
    tests: Option<RawSettings>,
    generated: Option<RawSettings>,
    vendored: Option<RawSettings>,
    agents: Option<RawSettings>,
    docs: Option<RawSettings>,
    tooling: Option<RawSettings>,
    stories: Option<RawSettings>,
    custom: Vec<RawCustom>,
    #[serde(flatten)]
    unknown: Unknown,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawSettings {
    enabled: Option<bool>,
    disabled_groups: Option<Vec<String>>,
    patterns: Option<Vec<String>>,
    #[serde(flatten)]
    unknown: Unknown,
}

#[derive(Deserialize)]
struct RawCustom {
    #[serde(flatten)]
    category: CustomCategory,
    #[serde(flatten)]
    unknown: Unknown,
}

impl From<RawCategories> for CategoriesConfig {
    fn from(raw: RawCategories) -> CategoriesConfig {
        let mut unknown: Vec<String> = raw
            .unknown
            .keys()
            .map(|k| format!("categories.{k}"))
            .collect();
        let mut merge = |def: &BuiltinDef, raw: Option<RawSettings>| {
            let mut settings = CategorySettings::defaults(def.id);
            if let Some(raw) = raw {
                unknown.extend(
                    raw.unknown
                        .keys()
                        .map(|k| format!("categories.{}.{k}", def.key)),
                );
                if let Some(enabled) = raw.enabled {
                    settings.enabled = enabled;
                }
                if let Some(groups) = raw.disabled_groups {
                    settings.disabled_groups = groups;
                }
                if let Some(patterns) = raw.patterns {
                    settings.patterns = patterns;
                }
            }
            settings
        };
        let [tests, generated, vendored, agents, docs, tooling, stories] = &BUILTINS;
        let mut config = CategoriesConfig {
            tests: merge(tests, raw.tests),
            generated: merge(generated, raw.generated),
            vendored: merge(vendored, raw.vendored),
            agents: merge(agents, raw.agents),
            docs: merge(docs, raw.docs),
            tooling: merge(tooling, raw.tooling),
            stories: merge(stories, raw.stories),
            custom: Vec::with_capacity(raw.custom.len()),
            unknown: Vec::new(),
        };
        for custom in raw.custom {
            unknown.extend(
                custom
                    .unknown
                    .keys()
                    .map(|k| format!("categories.custom[{}].{k}", custom.category.id)),
            );
            config.custom.push(custom.category);
        }
        config.unknown = unknown;
        config
    }
}
