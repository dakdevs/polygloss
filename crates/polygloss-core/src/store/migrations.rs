//! Schema migrations under a file lock; `schema-v<N>.sql` is migration N (T1.10, T6.1,
//! design §7.2).
//!
//! [`migrate_locked`] holds an exclusive `std::fs::File::lock` on
//! `polygloss.db.lock` while it reads `user_version`, backs up an existing database
//! with `VACUUM INTO 'polygloss.db.bak-v<N>'`, and runs rusqlite_migration. The
//! writer connection uses `BEGIN IMMEDIATE`, which rusqlite_migration respects.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use rusqlite_migration::{M, Migrations};

use super::{BootstrapReport, StoreError, ensure_private_file};
use crate::paths::DataPaths;

/// The schema version this build migrates to.
pub const LATEST_VERSION: u32 = 2;

static MIGRATIONS: &[M<'static>] = &[
    M::up(include_str!("schema-v1.sql")),
    M::up(include_str!("schema-v2.sql")),
];

/// The production migration list (index + 1 = `user_version`).
pub fn migrations() -> Migrations<'static> {
    Migrations::from_slice(MIGRATIONS)
}

/// Migrates `conn` to the latest version of `migrations` while holding the
/// migration lock. An existing database (version > 0) that needs migrating is
/// backed up first; a database newer than `migrations` is left untouched and
/// reported as [`StoreError::Migration`].
pub(crate) fn migrate_locked(
    conn: &mut Connection,
    paths: &DataPaths,
    migrations: &Migrations<'_>,
) -> Result<BootstrapReport, StoreError> {
    let lock = ensure_private_file(&paths.db_lock)?;
    lock.lock()
        .map_err(|e| StoreError::io("lock", &paths.db_lock, e))?;

    let before = user_version(conn)?;
    let pending = migrations.pending_migrations(conn)?;
    let backup = if before > 0 && pending > 0 {
        Some(backup(conn, paths, before)?)
    } else {
        None
    };
    migrations.to_latest(conn)?;
    let after = user_version(conn)?;

    // Dropping `lock` closes the file and releases the lock; unlock explicitly
    // anyway so the release point is obvious.
    let _ = lock.unlock();
    tracing::debug!(before, after, ?backup, "store migrated");
    Ok(BootstrapReport {
        version_before: before,
        version_after: after,
        backup,
    })
}

fn user_version(conn: &Connection) -> Result<u32, StoreError> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// `VACUUM INTO` a private temp file next to the database, then rename it to
/// `polygloss.db.bak-v<version>` (replacing an older backup of the same version).
fn backup(conn: &Connection, paths: &DataPaths, version: u32) -> Result<PathBuf, StoreError> {
    let name = |suffix: &str| -> PathBuf {
        let mut file = paths.db.file_name().unwrap_or_default().to_os_string();
        file.push(format!(".bak-v{version}{suffix}"));
        paths.db.with_file_name(file)
    };
    let target = name("");
    let tmp = name(".tmp");
    remove_if_exists(&tmp)?;
    // VACUUM INTO accepts an existing empty file, so pre-create it `0600`: the
    // backup is never readable by others, not even briefly.
    ensure_private_file(&tmp)?;
    let tmp_str = tmp
        .to_str()
        .ok_or_else(|| StoreError::Integrity(format!("non-UTF-8 backup path {tmp:?}")))?;
    if let Err(e) = conn.execute("VACUUM INTO ?1", [tmp_str]) {
        let _ = fs::remove_file(&tmp);
        return Err(e.into());
    }
    fs::rename(&tmp, &target).map_err(|e| StoreError::io("rename", &tmp, e))?;
    Ok(target)
}

fn remove_if_exists(path: &Path) -> Result<(), StoreError> {
    match fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
            Err(StoreError::io("remove", path, e))
        }
        _ => Ok(()),
    }
}
