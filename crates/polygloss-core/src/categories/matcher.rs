//! [`Categorizer`]: compiled category patterns and the match order of design
//! §11.15.
//!
//! Every pattern line becomes one or more `globset` globs: braces expand first
//! (geld's `expandBraces`, plus `\` escapes; a `{` without its `}` stays a
//! literal), then each alternative is normalized on its own:
//!
//! | Form           | Glob                                                     |
//! | -------------- | -------------------------------------------------------- |
//! | `name`         | `**/name`                                                |
//! | `dir/`         | `**/dir/**` (a directory only, OQ-45; never a file)      |
//! | `/dir/`        | `dir/**`                                                 |
//! | `a/b`, `/name` | as written, from the repo root                           |
//!
//! compiled with `literal_separator` (`*` and `?` stay in one segment) and
//! `backslash_escape`. All globs live in one `GlobSet`, tagged with their
//! category and line, so one pass over a path answers every category.

use std::borrow::Cow;
use std::collections::HashSet;
use std::fmt;

use globset::{Candidate, Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use polygloss_diff::{FileChange, GeneratedAttr};

use super::catalog::BUILTINS;
use super::{
    BuiltinCategory, BuiltinDef, CUSTOM_ICONS, CategoriesConfig, CategoriesError, CategoryId,
    CategoryInfo, Explain, Source, Verdict,
};
use crate::git::attrs::legacy_attr;

/// Most patterns one category may list (before brace expansion).
pub(super) const MAX_PATTERNS: usize = 500;
/// Most alternatives one pattern's braces may expand to.
const MAX_ALTERNATIVES: usize = 64;
/// [`Verdict::pattern`] of an attribute verdict.
const ATTRIBUTE: &str = "linguist-generated";

/// Classifies paths into categories with one settings snapshot. `Send + Sync`.
pub struct Categorizer {
    /// Compiled categories: the enabled ones and Generated (always, for
    /// [`Categorizer::is_generated`]).
    slots: Vec<Slot>,
    /// Match order: enabled custom categories, the attribute step (when
    /// Generated is enabled), enabled built-ins.
    steps: Vec<Step>,
    generated: usize,
    set: GlobSet,
    /// Per glob of `set`, by index.
    tags: Vec<Tag>,
    enabled: Vec<CategoryInfo>,
    warnings: Vec<String>,
}

struct Slot {
    id: CategoryId,
    /// Groups' patterns, then extras, in order; rescues keep their `!`.
    lines: Vec<Line>,
}

struct Line {
    pattern: String,
    source: Source,
}

#[derive(Clone, Copy)]
struct Tag {
    slot: usize,
    line: usize,
    rescue: bool,
}

#[derive(Clone, Copy)]
enum Step {
    Slot(usize),
    Attribute,
}

/// One pattern line, compiled: `(line, rescue, globs)`.
type Compiled = (Line, bool, Vec<Glob>);

/// Per-path match state, reused across paths.
#[derive(Default)]
struct Scratch {
    matches: Vec<usize>,
    /// Per slot: the first matching `[rescue, include]` line.
    first: Vec<[Option<usize>; 2]>,
    /// `(slot, line)` of each rescue that skipped a category in the last
    /// [`Categorizer::decide`].
    rescued: Vec<(usize, usize)>,
}

/// A decision: the slot, and the line (`None` for the attribute step).
type Hit = (usize, Option<usize>);

impl Categorizer {
    /// Compiles `cfg` and the legacy `diff.generated_patterns`. A pattern in
    /// `cfg` that does not compile (enabled or not), a bad or duplicate custom
    /// id, an empty name or too many patterns is an error; a legacy pattern
    /// that does not compile is dropped into [`Categorizer::warnings`].
    pub fn new(
        cfg: &CategoriesConfig,
        legacy_generated: &[String],
    ) -> Result<Categorizer, CategoriesError> {
        let mut compiled: Vec<(CategoryId, Vec<Compiled>)> = Vec::new();
        let mut steps = Vec::new();
        let mut enabled = Vec::new();
        let mut warnings = Vec::new();

        let mut ids = HashSet::new();
        let mut unknown_icons = Vec::new();
        for custom in &cfg.custom {
            if !valid_custom_id(&custom.id) {
                return Err(CategoriesError::BadCustomId(custom.id.clone()));
            }
            if BUILTINS.iter().any(|d| d.key == custom.id) || !ids.insert(custom.id.as_str()) {
                return Err(CategoriesError::DuplicateId(custom.id.clone()));
            }
            if custom.name.trim().is_empty() {
                return Err(CategoriesError::EmptyName(custom.id.clone()));
            }
            let id = CategoryId::Custom(custom.id.clone());
            let lines = compile_user(&id, &custom.patterns, Source::Custom)?;
            let icon = match custom.icon.as_str() {
                "" => "tag",
                icon if CUSTOM_ICONS.contains(&icon) => icon,
                unknown => {
                    if !unknown_icons.contains(&unknown) {
                        unknown_icons.push(unknown);
                        warnings.push(format!(
                            "categories.custom: unknown icon {unknown:?}; using \"tag\""
                        ));
                    }
                    "tag"
                }
            };
            if custom.enabled {
                steps.push(Step::Slot(compiled.len()));
                enabled.push(CategoryInfo {
                    id: id.clone(),
                    title: custom.name.clone(),
                    icon: icon.to_owned(),
                    builtin: None,
                });
                compiled.push((id, lines));
            }
        }

        if cfg.generated.enabled {
            steps.push(Step::Attribute);
        }
        let mut generated = 0;
        for def in &BUILTINS {
            let settings = cfg.get(def.id);
            let id = CategoryId::Builtin(def.id);
            // Checked even when the category is off: a bad pattern anywhere
            // makes the section invalid.
            let extras = compile_user(&id, &settings.patterns, Source::Extra)?;
            let is_generated = def.id == BuiltinCategory::Generated;
            if !settings.enabled && !is_generated {
                continue;
            }
            let mut lines = compile_groups(def, &settings.disabled_groups)?;
            lines.extend(extras);
            if is_generated {
                generated = compiled.len();
                lines.extend(compile_legacy(legacy_generated, &mut warnings));
            }
            if settings.enabled {
                steps.push(Step::Slot(compiled.len()));
                enabled.push(CategoryInfo {
                    id: id.clone(),
                    title: def.title.to_owned(),
                    icon: def.icon.to_owned(),
                    builtin: Some(def),
                });
            }
            compiled.push((id, lines));
        }

        // One set over every glob, each tagged with its slot and line, added in
        // slot and line order (`scan` relies on it).
        let mut set = GlobSetBuilder::new();
        let mut tags = Vec::new();
        let mut slots = Vec::with_capacity(compiled.len());
        for (slot, (id, lines)) in compiled.into_iter().enumerate() {
            let mut kept = Vec::with_capacity(lines.len());
            for (line, (text, rescue, globs)) in lines.into_iter().enumerate() {
                for glob in globs {
                    set.add(glob);
                    tags.push(Tag { slot, line, rescue });
                }
                kept.push(text);
            }
            slots.push(Slot { id, lines: kept });
        }
        let set = set.build().map_err(|e| CategoriesError::Pattern {
            category: "categories".to_owned(),
            pattern: e.glob().unwrap_or_default().to_owned(),
            message: e.kind().to_string(),
        })?;
        Ok(Categorizer {
            slots,
            steps,
            generated,
            set,
            tags,
            enabled,
            warnings,
        })
    }

    /// The enabled categories, in match order.
    pub fn enabled(&self) -> &[CategoryInfo] {
        &self.enabled
    }

    /// The category of `path`, or `None` when it stays in the main list.
    pub fn categorize(
        &self,
        path: &str,
        attr: GeneratedAttr,
        stored_generated: bool,
    ) -> Option<CategoryId> {
        let hit = self.decide(
            path.as_bytes(),
            attr,
            stored_generated,
            &mut Scratch::default(),
        );
        hit.map(|(slot, _)| self.slots[slot].id.clone())
    }

    /// Whether `path` is generated (shown behind "Load diff"): the attribute
    /// when set or unset, else the Generated patterns minus its rescues,
    /// whether or not the category is enabled.
    pub fn is_generated(&self, path: &str, attr: GeneratedAttr, stored_generated: bool) -> bool {
        let path = path.as_bytes();
        match recover(path, attr, stored_generated) {
            GeneratedAttr::Set => true,
            GeneratedAttr::Unset => false,
            GeneratedAttr::Unspecified | GeneratedAttr::Unknown => {
                let mut scratch = Scratch::default();
                self.scan(path, &mut scratch);
                matches!(scratch.first[self.generated], [None, Some(_)])
            }
        }
    }

    /// The category of `path` with the pattern and source that decided, or
    /// the rescues that skipped categories.
    pub fn explain(&self, path: &str, attr: GeneratedAttr, stored_generated: bool) -> Explain {
        let mut scratch = Scratch::default();
        let hit = self.decide(path.as_bytes(), attr, stored_generated, &mut scratch);
        let Some((slot, line)) = hit else {
            let rescued_by = scratch
                .rescued
                .iter()
                .map(|&(slot, line)| {
                    let slot = &self.slots[slot];
                    (slot.id.clone(), slot.lines[line].pattern.clone())
                })
                .collect();
            return Explain::Uncategorized { rescued_by };
        };
        let slot = &self.slots[slot];
        Explain::Matched(match line {
            Some(line) => Verdict {
                category: slot.id.clone(),
                pattern: slot.lines[line].pattern.clone(),
                source: slot.lines[line].source,
            },
            None => Verdict {
                category: slot.id.clone(),
                pattern: ATTRIBUTE.to_owned(),
                source: Source::Attribute,
            },
        })
    }

    /// Each file's category by its display path (raw bytes for a non-UTF-8
    /// path), stored attribute and `generated` bit; index = position.
    pub fn categorize_files(&self, files: &[FileChange]) -> Vec<Option<CategoryId>> {
        let mut scratch = Scratch::default();
        files
            .iter()
            .map(|f| {
                let hit = self.decide(&raw_path(f), f.generated_attr, f.generated, &mut scratch);
                hit.map(|(slot, _)| self.slots[slot].id.clone())
            })
            .collect()
    }

    /// Dropped legacy patterns and unknown custom icons, for callers to log
    /// once.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// Walks the match order; rescues met on the way land in
    /// `scratch.rescued`.
    fn decide(
        &self,
        path: &[u8],
        attr: GeneratedAttr,
        stored_generated: bool,
        scratch: &mut Scratch,
    ) -> Option<Hit> {
        let attr = recover(path, attr, stored_generated);
        self.scan(path, scratch);
        scratch.rescued.clear();
        for step in &self.steps {
            let slot = match *step {
                Step::Attribute if attr == GeneratedAttr::Set => {
                    return Some((self.generated, None));
                }
                Step::Attribute => continue,
                Step::Slot(slot) => slot,
            };
            // Set was decided at the attribute step; Unset skips Generated.
            if slot == self.generated && attr != GeneratedAttr::Unspecified {
                continue;
            }
            match scratch.first[slot] {
                [Some(rescue), _] => scratch.rescued.push((slot, rescue)),
                [None, Some(line)] => return Some((slot, Some(line))),
                [None, None] => {}
            }
        }
        None
    }

    fn scan(&self, path: &[u8], scratch: &mut Scratch) {
        self.set
            .matches_candidate_into(&Candidate::from_bytes(path), &mut scratch.matches);
        scratch.first.clear();
        scratch.first.resize(self.slots.len(), [None, None]);
        // Globs were added slot by slot in line order, and matches are sorted,
        // so the first hit per slot and role is its earliest line.
        for &i in &scratch.matches {
            let tag = self.tags[i];
            scratch.first[tag.slot][usize::from(!tag.rescue)].get_or_insert(tag.line);
        }
    }
}

impl fmt::Debug for Categorizer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let enabled: Vec<String> = self.enabled.iter().map(|i| i.id.to_string()).collect();
        f.debug_struct("Categorizer")
            .field("enabled", &enabled)
            .field("globs", &self.tags.len())
            .field("warnings", &self.warnings)
            .finish()
    }
}

/// An unknown (pre-v2) attribute recovered from the stored v1 bit.
fn recover(path: &[u8], attr: GeneratedAttr, stored_generated: bool) -> GeneratedAttr {
    match attr {
        GeneratedAttr::Unknown => legacy_attr(path, stored_generated),
        attr => attr,
    }
}

/// The display path's raw bytes: the new path, or the old one for deletions.
fn raw_path(file: &FileChange) -> Cow<'_, [u8]> {
    match file.new_path.as_ref().or(file.old_path.as_ref()) {
        Some(p) if p.escaped => Cow::Owned(p.to_bytes()),
        Some(p) => Cow::Borrowed(p.text.as_bytes()),
        None => Cow::Borrowed(&[]),
    }
}

fn valid_custom_id(id: &str) -> bool {
    let b = id.as_bytes();
    (1..=41).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
}

/// A user's pattern list (extras or a custom category's), compiled.
fn compile_user(
    id: &CategoryId,
    patterns: &[String],
    source: Source,
) -> Result<Vec<Compiled>, CategoriesError> {
    if patterns.len() > MAX_PATTERNS {
        return Err(CategoriesError::TooManyPatterns(id.to_string()));
    }
    compile_lines(id, patterns.iter().map(|p| (p.as_str(), source)))
}

/// The catalog patterns of `def`'s groups not in `disabled`.
fn compile_groups(
    def: &'static BuiltinDef,
    disabled: &[String],
) -> Result<Vec<Compiled>, CategoriesError> {
    let groups = def
        .groups
        .iter()
        .filter(|g| !disabled.iter().any(|d| d == g.key));
    let lines = groups.flat_map(|g| {
        let source = Source::BuiltIn { group: g.key };
        g.patterns.iter().map(move |p| (*p, source))
    });
    compile_lines(&CategoryId::Builtin(def.id), lines)
}

/// `diff.generated_patterns`, compiled; one that does not compile is
/// dropped with a warning, never an error.
fn compile_legacy(patterns: &[String], warnings: &mut Vec<String>) -> Vec<Compiled> {
    let mut out = Vec::new();
    for raw in patterns {
        match compile_line(raw, Source::Extra) {
            Ok(line) => out.extend(line),
            Err(message) => warnings.push(format!(
                "diff.generated_patterns: {raw:?} does not compile ({message}); ignored"
            )),
        }
    }
    out
}

fn compile_lines<'a>(
    id: &CategoryId,
    lines: impl Iterator<Item = (&'a str, Source)>,
) -> Result<Vec<Compiled>, CategoriesError> {
    let mut out = Vec::new();
    for (raw, source) in lines {
        let line = compile_line(raw, source).map_err(|message| CategoriesError::Pattern {
            category: id.to_string(),
            pattern: raw.trim().to_owned(),
            message,
        })?;
        out.extend(line);
    }
    Ok(out)
}

/// One pattern line compiled: `None` when blank or a `#` comment; a leading
/// `!` makes it a rescue. The error says why it does not compile.
fn compile_line(raw: &str, source: Source) -> Result<Option<Compiled>, String> {
    let pattern = raw.trim();
    if pattern.is_empty() || pattern.starts_with('#') {
        return Ok(None);
    }
    let (rescue, body) = match pattern.strip_prefix('!') {
        Some(body) => (true, body),
        None => (false, pattern),
    };
    let line = Line {
        pattern: pattern.to_owned(),
        source,
    };
    Ok(Some((line, rescue, compile(body)?)))
}

/// The globs of one pattern (no `!`); the error is the reason it does not
/// compile.
fn compile(pattern: &str) -> Result<Vec<Glob>, String> {
    let alternatives = expand_braces(pattern)
        .ok_or_else(|| format!("braces expand to more than {MAX_ALTERNATIVES} alternatives"))?;
    alternatives
        .iter()
        .filter_map(|alt| normalize(&escape_braces(alt)))
        .map(|glob| {
            GlobBuilder::new(&glob)
                .literal_separator(true)
                .backslash_escape(true)
                .build()
                .map_err(|e| e.kind().to_string())
        })
        .collect()
}

/// geld's `expandBraces`, skipping `\`-escaped characters: the first `{`
/// with a matching `}` expands into its top-level alternatives, each read
/// again with the rest of the pattern; a `{` without its `}` stays as is.
/// `None` past [`MAX_ALTERNATIVES`].
fn expand_braces(pattern: &str) -> Option<Vec<String>> {
    let b = pattern.as_bytes();
    let (mut open, mut close, mut depth) = (None, None, 0usize);
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'{' => {
                open.get_or_insert(i);
                depth += 1;
            }
            b'}' if open.is_some() => {
                depth -= 1;
                if depth == 0 {
                    close = Some(i);
                    break;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let (Some(open), Some(close)) = (open, close) else {
        return Some(vec![pattern.to_owned()]);
    };
    let (prefix, suffix) = (&pattern[..open], &pattern[close + 1..]);
    let mut out = Vec::new();
    for alternative in split_alternatives(&pattern[open + 1..close]) {
        for tail in expand_braces(&format!("{alternative}{suffix}"))? {
            if out.len() == MAX_ALTERNATIVES {
                return None;
            }
            out.push(format!("{prefix}{tail}"));
        }
    }
    Some(out)
}

/// `body` split at commas outside nested braces (escapes skipped).
fn split_alternatives(body: &str) -> Vec<&str> {
    let b = body.as_bytes();
    let (mut parts, mut start, mut depth, mut i) = (Vec::new(), 0, 0i32, 0);
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'{' => depth += 1,
            b'}' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    parts.push(&body[start..]);
    parts
}

/// Braces left after expansion (unclosed ones) escaped, so globset reads them
/// as literals instead of its own alternation.
fn escape_braces(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                out.push(c);
                out.extend(chars.next());
            }
            '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// The glob for one brace-free alternative (module table); `None` when
/// nothing is left to match.
fn normalize(alternative: &str) -> Option<String> {
    let (anchored, rest) = match alternative.strip_prefix('/') {
        Some(rest) => (true, rest),
        None => (false, alternative),
    };
    let (dir, rest) = match rest.strip_suffix('/') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    if rest.is_empty() {
        return None;
    }
    let anywhere = !anchored && (dir || !rest.contains('/')) && !rest.starts_with("**/");
    let mut glob = if anywhere {
        format!("**/{rest}")
    } else {
        rest.to_owned()
    };
    if dir {
        glob.push_str("/**");
    }
    Some(glob)
}
