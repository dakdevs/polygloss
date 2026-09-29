//! Repo discovery: common dir, git dir, toplevel and object format (T1.2, design §4.3).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use polygloss_diff::ObjectFormat;

use crate::git::runner::{Git, GitError};

/// A discovered repository. All paths are canonical (realpath'd). `common_dir` is
/// the repo identity (design §4.3): every linked worktree of one clone shares it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    pub common_dir: PathBuf,
    /// This worktree's git dir (`<common_dir>/worktrees/<name>` for a linked worktree).
    pub git_dir: PathBuf,
    /// The worktree root, `None` for a bare repo or from inside a git dir.
    pub toplevel: Option<PathBuf>,
    pub object_format: ObjectFormat,
}

/// Finds the repo containing `path` (a directory or a file inside a worktree, a git
/// dir or a bare repo). Anything that is not inside a repository, including a
/// missing path, is `GitError::NotARepo`.
pub fn discover(path: &Path) -> Result<RepoInfo, GitError> {
    let not_a_repo = || GitError::NotARepo(path.to_path_buf());
    let real = std::fs::canonicalize(path).map_err(|_| not_a_repo())?;
    let dir = if real.is_dir() {
        real
    } else {
        real.parent()
            .map(Path::to_path_buf)
            .ok_or_else(not_a_repo)?
    };
    let git = Git::new(&dir);
    let out = git.run(&[
        OsStr::new("rev-parse"),
        OsStr::new("--path-format=absolute"),
        OsStr::new("--git-common-dir"),
        OsStr::new("--git-dir"),
        OsStr::new("--show-object-format"),
        OsStr::new("--is-inside-work-tree"),
    ])?;
    if !out.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not a git repository") {
            return Err(not_a_repo());
        }
        return Err(GitError::Failed {
            args: "rev-parse --git-common-dir".into(),
            code: out.code,
            stderr: stderr.into_owned(),
        });
    }
    let text = String::from_utf8(out.stdout)
        .map_err(|_| GitError::Parse("rev-parse printed a non-UTF-8 repo path".into()))?;
    let lines: Vec<&str> = text.lines().collect();
    let [common_dir, git_dir, format, inside] = lines[..] else {
        return Err(GitError::Parse(format!(
            "rev-parse discovery output: {text:?}"
        )));
    };
    let object_format = ObjectFormat::from_name(format)
        .ok_or_else(|| GitError::Parse(format!("unknown object format {format:?}")))?;
    let toplevel = if inside == "true" {
        let top = git.output(&[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")])?;
        let top = String::from_utf8(top)
            .map_err(|_| GitError::Parse("rev-parse printed a non-UTF-8 toplevel".into()))?;
        Some(std::fs::canonicalize(top.trim_end_matches('\n'))?)
    } else {
        None
    };
    Ok(RepoInfo {
        common_dir: std::fs::canonicalize(common_dir)?,
        git_dir: std::fs::canonicalize(git_dir)?,
        toplevel,
        object_format,
    })
}
