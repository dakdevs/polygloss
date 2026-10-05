//! Hidden `polygloss debug categorize [--repo <path>] [--json] <path>…`
//! (design §11.15): explains the file category of each repo-relative path
//! with `settings.json` as agents read it: the category and its title, and
//! the source, group and pattern that decided, or the rescues that skipped a
//! category on the way.
//!
//! - With `--repo`, `linguist-generated` comes from that repo's HEAD tree (one
//!   `check-attr`; a bare repo has no worktree to run it in, so its paths stay
//!   unspecified). Without it every path is unspecified, so the `attribute`
//!   source needs `--repo`.
//! - Unlike `open_diff`, an invalid `categories` section is an error
//!   (`conflict`, "settings.json: …"), not the defaults.

use std::fmt::Write as _;
use std::path::Path;

use clap::Args;
use polygloss_core::categories::{Categorizer, CategoryId, Explain, Source};
use polygloss_core::git::{self, Git};
use polygloss_core::paths::DataPaths;
use polygloss_core::review::CoreError;
use polygloss_core::settings::categories_config;
use polygloss_diff::{FileChange, FileKind, FileStatus, GeneratedAttr, GitPath, Oid};
use serde::Serialize;

use crate::cli::GlobalArgs;
use crate::output::{CliError, Report};

/// `debug categorize` arguments; `--repo` and `--json` are the global flags.
#[derive(Debug, Args)]
pub struct CategorizeArgs {
    /// Repo-relative paths, as diffs show them.
    #[arg(required = true, value_name = "PATH")]
    pub paths: Vec<String>,
}

/// One path's verdict.
#[derive(Debug, Default, Serialize)]
struct Explained {
    path: String,
    /// `null` when the path stays in the main list.
    category: Option<CategoryId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    /// `built-in`, `extra`, `custom` or `attribute`.
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pattern: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    rescued_by: Vec<Rescue>,
}

/// A rescue (`!pattern`) that skipped `category`.
#[derive(Debug, Serialize)]
struct Rescue {
    category: CategoryId,
    pattern: String,
}

pub fn run(args: CategorizeArgs, global: &GlobalArgs) -> Result<Report, CliError> {
    let data = DataPaths::resolve().map_err(CoreError::from)?;
    let read = categories_config(&data);
    if let Some(warning) = read.warning {
        return Err(CliError::new("conflict", warning));
    }
    let categorizer = Categorizer::new(&read.config, &read.legacy_generated)
        .map_err(|e| CliError::new("conflict", format!("settings.json: {e}")))?;
    let attrs = match &global.repo {
        Some(repo) => head_attributes(repo, &args.paths)?,
        None => vec![GeneratedAttr::Unspecified; args.paths.len()],
    };
    let explained: Vec<Explained> = args
        .paths
        .into_iter()
        .zip(attrs)
        .map(|(path, attr)| explain(&categorizer, path, attr))
        .collect();
    let human = explained.iter().map(human_line).collect();
    let json = serde_json::to_value(&explained)
        .map_err(|e| CliError::internal(format!("encoding the verdicts: {e}")))?;
    Ok(Report { json, human })
}

fn explain(categorizer: &Categorizer, path: String, attr: GeneratedAttr) -> Explained {
    let v = match categorizer.explain(&path, attr, false) {
        Explain::Matched(v) => v,
        Explain::Uncategorized { rescued_by } => {
            let rescued_by = rescued_by
                .into_iter()
                .map(|(category, pattern)| Rescue { category, pattern })
                .collect();
            return Explained {
                path,
                rescued_by,
                ..Explained::default()
            };
        }
    };
    let (source, group) = match v.source {
        Source::BuiltIn { group } => ("built-in", Some(group)),
        Source::Extra => ("extra", None),
        Source::Custom => ("custom", None),
        Source::Attribute => ("attribute", None),
    };
    let title = categorizer
        .enabled()
        .iter()
        .find(|info| info.id == v.category)
        .map(|info| info.title.clone());
    Explained {
        path,
        category: Some(v.category),
        title,
        source: Some(source),
        group,
        pattern: Some(v.pattern),
        rescued_by: Vec::new(),
    }
}

/// `src/a.test.ts: tests (built-in group unit, pattern *.test.*)`.
fn human_line(e: &Explained) -> String {
    let mut line = format!("{}: ", e.path);
    match (&e.category, e.source, &e.pattern) {
        (Some(category), Some(source), Some(pattern)) => {
            let _ = write!(line, "{category} ({source}");
            if let Some(group) = e.group {
                let _ = write!(line, " group {group}");
            }
            let _ = write!(line, ", pattern {pattern})");
        }
        _ => {
            line.push_str("uncategorized");
            let rescues: Vec<String> = e
                .rescued_by
                .iter()
                .map(|r| format!("rescued from {} by {}", r.category, r.pattern))
                .collect();
            if !rescues.is_empty() {
                let _ = write!(line, " ({})", rescues.join(", "));
            }
        }
    }
    line.push('\n');
    line
}

/// `linguist-generated` of each path in the HEAD tree of the repo at `repo`.
fn head_attributes(repo: &Path, paths: &[String]) -> Result<Vec<GeneratedAttr>, CliError> {
    let info = git::discover(repo).map_err(CoreError::from)?;
    let Some(toplevel) = info.toplevel.clone() else {
        return Ok(vec![GeneratedAttr::Unspecified; paths.len()]);
    };
    let git = Git::new(toplevel);
    let head = crate::debug::tree_of(&git, info.object_format, "HEAD")?;
    let zero = Oid::zero(info.object_format);
    // `classify` reads attributes for file changes; these stand-ins carry
    // only the path it looks up.
    let mut changes: Vec<FileChange> = paths
        .iter()
        .map(|path| FileChange {
            idx: 0,
            status: FileStatus::Added,
            old_path: None,
            new_path: Some(GitPath::from_bytes(path.as_bytes())),
            old_mode: None,
            new_mode: None,
            old_blob: zero.clone(),
            new_blob: zero.clone(),
            similarity: None,
            kind: FileKind::Text,
            generated: false,
            generated_attr: GeneratedAttr::Unspecified,
        })
        .collect();
    git::classify(&git, &head, &mut changes, &[]).map_err(CoreError::from)?;
    Ok(changes.into_iter().map(|c| c.generated_attr).collect())
}
