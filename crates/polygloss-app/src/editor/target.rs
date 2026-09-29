//! What "open in editor" opens (design §11.13, ADR-0021): the file on disk
//! at the line-mapped position, or a read-only copy of the blob. Pure file
//! work, run off the UI thread.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use polygloss_diff::line_map::{LineMap, Mapped};
use polygloss_diff::{FileChange, FileKind, GitPath, Oid, Side};
use polygloss_viewport::DiffProvider;

/// A file and a 1-based line for the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorTarget {
    pub path: PathBuf,
    pub line: u32,
}

/// Where `change`'s 0-based `line` on `side` opens:
///
/// - a new-side line of a file that is on disk in `worktree`: that file, at
///   the line [`LineMap`] maps the diff's new blob onto the disk content
///   (the nearest line when it changed since);
/// - an old-side line, a deleted file, a file missing on disk (or no
///   worktree), a symlink: `<blobs_dir>/<oid>/<basename>`, a read-only
///   (`0444`) copy of that side's blob, at the line itself.
///
/// Submodules have nothing to open.
pub fn resolve(
    change: &FileChange,
    side: Side,
    line: u32,
    worktree: Option<&Path>,
    provider: &dyn DiffProvider,
    blobs_dir: &Path,
) -> anyhow::Result<EditorTarget> {
    if change.kind == FileKind::Submodule {
        bail!("{} is a submodule", change.display_path());
    }
    if side == Side::New
        && change.kind != FileKind::Symlink
        && let (Some(new_path), Some(worktree)) = (&change.new_path, worktree)
    {
        let disk = worktree.join(os_path(new_path));
        if std::fs::metadata(&disk).is_ok_and(|m| m.is_file()) {
            let blob = provider.load_blob(&change.new_blob)?;
            let content =
                std::fs::read(&disk).with_context(|| format!("reading {}", disk.display()))?;
            let mapped = match LineMap::new(&blob, &content).map_line(line) {
                Mapped::Unchanged(n) | Mapped::Changed { nearest: n } => n,
            };
            return Ok(EditorTarget {
                path: disk,
                line: mapped + 1,
            });
        }
    }
    // The side's own blob; the other side when this one does not exist.
    let (oid, path) = match (side, &change.old_path, &change.new_path) {
        (Side::Old, Some(p), _) | (Side::New, Some(p), None) => (&change.old_blob, p),
        (_, _, Some(p)) => (&change.new_blob, p),
        (_, None, None) => bail!("the file has no path"),
    };
    let path = blob_copy(provider, oid, path, blobs_dir)?;
    Ok(EditorTarget {
        path,
        line: line + 1,
    })
}

/// A git path as a relative filesystem path (raw bytes, OQ-25).
fn os_path(path: &GitPath) -> PathBuf {
    PathBuf::from(OsString::from_vec(path.to_bytes()))
}

/// `<blobs_dir>/<oid>/<basename>`, written once (atomically, mode `0444`)
/// and reused while it has the blob's size.
pub fn blob_copy(
    provider: &dyn DiffProvider,
    oid: &Oid,
    path: &GitPath,
    blobs_dir: &Path,
) -> anyhow::Result<PathBuf> {
    let bytes = path.to_bytes();
    let base = bytes.rsplit(|&b| b == b'/').next().unwrap_or(&bytes);
    let name = if base.is_empty() {
        OsString::from("blob")
    } else {
        OsString::from_vec(base.to_vec())
    };
    let dir = blobs_dir.join(oid.as_str());
    let dest = dir.join(&name);
    let size = provider.blob_size(oid)?;
    if std::fs::symlink_metadata(&dest).is_ok_and(|m| m.is_file() && m.len() == size) {
        return Ok(dest);
    }
    let blob = provider.load_blob(oid)?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut tmp_name = OsString::from(".");
    tmp_name.push(&name);
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = dir.join(tmp_name);
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, &blob).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o444))?;
    // A stale copy (another size) is read-only; the directory is ours.
    let _ = std::fs::remove_file(&dest);
    std::fs::rename(&tmp, &dest).with_context(|| format!("writing {}", dest.display()))?;
    Ok(dest)
}
