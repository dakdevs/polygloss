//! Repo discovery: common dir, git dir, toplevel and object format (T1.2, design §4.3).

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
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
    // Fixed-token answers first and at most one path per call, so a path containing
    // a newline is never split (paths are git's output minus its one trailing `\n`).
    let out = git.run(&[
        OsStr::new("rev-parse"),
        OsStr::new("--show-object-format"),
        OsStr::new("--is-inside-work-tree"),
        OsStr::new("--path-format=absolute"),
        OsStr::new("--git-dir"),
    ])?;
    if !out.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("not a git repository") {
            return Err(not_a_repo());
        }
        return Err(GitError::Failed {
            args: "rev-parse --git-dir".into(),
            code: out.code,
            stderr: stderr.into_owned(),
        });
    }
    let mut fields = out.stdout.splitn(3, |&b| b == b'\n');
    let (Some(format), Some(inside), Some(git_dir)) = (fields.next(), fields.next(), fields.next())
    else {
        return Err(GitError::Parse(format!(
            "rev-parse discovery output: {:?}",
            String::from_utf8_lossy(&out.stdout)
        )));
    };
    let format = String::from_utf8_lossy(format);
    let object_format = ObjectFormat::from_name(&format)
        .ok_or_else(|| GitError::Parse(format!("unknown object format {format:?}")))?;
    let git_dir = path_output(git_dir)?;
    let common_dir = path_output(&git.output(&[
        OsStr::new("rev-parse"),
        OsStr::new("--path-format=absolute"),
        OsStr::new("--git-common-dir"),
    ])?)?;
    let toplevel = if inside == b"true" {
        let top = git.output(&[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")])?;
        Some(std::fs::canonicalize(path_output(&top)?)?)
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

/// One path printed by git on its own line: the bytes minus exactly one trailing
/// `\n` (a path may itself end in, or contain, newlines; non-UTF-8 bytes are kept).
pub(crate) fn path_output(bytes: &[u8]) -> Result<PathBuf, GitError> {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    if bytes.is_empty() {
        return Err(GitError::Parse("git printed an empty path".into()));
    }
    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}
