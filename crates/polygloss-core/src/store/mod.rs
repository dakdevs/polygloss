//! The SQLite store: bootstrap, WAL, `BEGIN IMMEDIATE` writes and reads (T1.10, design §7.1).
//!
//! One database shared by the app, the CLI, `polygloss mcp` and `polygloss wait`
//! (ADR-0015). [`Store::open`] creates the data dir (`0700`) and the database file
//! (`0600`), applies the §7.1 bootstrap pragmas in order, then migrates while
//! holding an exclusive `std::fs::File::lock` on `polygloss.db.lock`, so concurrent
//! first launches run exactly one migration.

pub mod events;
pub mod migrations;

use std::fs::{self, DirBuilder, OpenOptions, Permissions};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use rusqlite::{Connection, ErrorCode, Transaction, TransactionBehavior};
use rusqlite_migration::Migrations;

use crate::paths::DataPaths;

/// `busy_timeout` for every connection (design §7.1).
pub const BUSY_TIMEOUT: Duration = Duration::from_millis(5000);

/// `journal_size_limit` in bytes (64 MB, provisional).
pub const JOURNAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

/// How long to keep retrying the switch to WAL while a fresh file reports
/// `SQLITE_BUSY` (it can do so without calling the busy handler).
const WAL_RETRY_FOR: Duration = Duration::from_secs(10);

/// Errors from the store and from closures run inside it.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A SQLite error.
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// A migration failed, or the database is newer than this build.
    #[error("migration: {0}")]
    Migration(#[from] Box<rusqlite_migration::Error>),
    /// A filesystem operation on the data dir failed.
    #[error("{op} {}: {source}", path.display())]
    Io {
        /// What was being done.
        op: &'static str,
        /// The path involved.
        path: PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
    /// The database could not be switched to WAL mode.
    #[error("could not enable WAL: {0}")]
    Wal(String),
    /// `PRAGMA quick_check` found problems (or a caller-detected inconsistency).
    #[error("integrity: {0}")]
    Integrity(String),
    /// A JSON column could not be encoded or decoded.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}

impl From<rusqlite_migration::Error> for StoreError {
    fn from(e: rusqlite_migration::Error) -> Self {
        StoreError::Migration(Box::new(e))
    }
}

impl StoreError {
    pub(crate) fn io(op: &'static str, path: &Path, source: std::io::Error) -> Self {
        StoreError::Io {
            op,
            path: path.to_path_buf(),
            source,
        }
    }
}

/// What [`Store::open`] found and did while holding the migration lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BootstrapReport {
    /// `user_version` observed under the lock, before migrating (0 = new database).
    pub version_before: u32,
    /// `user_version` after migrating.
    pub version_after: u32,
    /// The `VACUUM INTO` backup taken before migrating an existing database.
    pub backup: Option<PathBuf>,
}

/// The shared store: one writer connection behind a mutex. Cheap to clone.
#[derive(Clone)]
pub struct Store {
    conn: Arc<Mutex<Connection>>,
    report: Arc<BootstrapReport>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("report", &self.report)
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Opens (creating if needed) and migrates the store at `paths.db`.
    pub fn open(paths: &DataPaths) -> Result<Store, StoreError> {
        Self::open_with_migrations(paths, &migrations::migrations())
    }

    /// [`Store::open`] with an explicit migration list (tests use short lists).
    pub fn open_with_migrations(
        paths: &DataPaths,
        migrations: &Migrations<'_>,
    ) -> Result<Store, StoreError> {
        ensure_private_dir(&paths.data_dir)?;
        ensure_private_db(&paths.db)?;

        let mut conn = Connection::open(&paths.db)?;
        bootstrap_connection(&conn)?;
        conn.set_transaction_behavior(TransactionBehavior::Immediate);

        let report = migrations::migrate_locked(&mut conn, paths, migrations)?;
        Ok(Store {
            conn: Arc::new(Mutex::new(conn)),
            report: Arc::new(report),
        })
    }

    /// What happened during bootstrap (versions seen under the lock, backup).
    pub fn bootstrap(&self) -> &BootstrapReport {
        &self.report
    }

    /// Runs `f` in a `BEGIN IMMEDIATE` transaction; commits on `Ok`, rolls back on `Err`.
    pub fn write<T>(
        &self,
        f: impl FnOnce(&Transaction) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    /// Runs `f` inside a deferred read transaction, so its queries see one snapshot.
    pub fn read<T>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T, StoreError>,
    ) -> Result<T, StoreError> {
        let mut conn = self.lock();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Deferred)?;
        let value = f(&tx)?;
        tx.commit()?;
        Ok(value)
    }

    /// `PRAGMA quick_check`; any result other than `ok` is [`StoreError::Integrity`].
    pub fn quick_check(&self) -> Result<(), StoreError> {
        let conn = self.lock();
        let mut stmt = conn.prepare("PRAGMA quick_check")?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        if rows.len() == 1 && rows[0] == "ok" {
            Ok(())
        } else {
            Err(StoreError::Integrity(rows.join("; ")))
        }
    }

    /// `PRAGMA wal_checkpoint(PASSIVE)`: never blocks readers or writers.
    pub fn checkpoint(&self) -> Result<(), StoreError> {
        let conn = self.lock();
        conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))?;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        // A panic inside a closure drops (rolls back) its transaction first, so the
        // connection itself is still usable after poisoning.
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Applies the §7.1 bootstrap to a connection, in order: `busy_timeout=5000`,
/// `journal_mode=WAL` (retried on `SQLITE_BUSY`), `synchronous=NORMAL`,
/// `foreign_keys=ON`, then `journal_size_limit`. Every connection to the store
/// (including dedicated feed readers) goes through this.
pub fn bootstrap_connection(conn: &Connection) -> Result<(), StoreError> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    enable_wal(conn)?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.query_row(
        &format!("PRAGMA journal_size_limit = {JOURNAL_SIZE_LIMIT}"),
        [],
        |_| Ok(()),
    )?;
    Ok(())
}

/// Switches the database to WAL, retrying on `SQLITE_BUSY`/`SQLITE_LOCKED`: a fresh
/// file can return BUSY without calling the busy handler (§7.1).
fn enable_wal(conn: &Connection) -> Result<(), StoreError> {
    let deadline = Instant::now() + WAL_RETRY_FOR;
    loop {
        match conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get::<_, String>(0)) {
            Ok(mode) if mode.eq_ignore_ascii_case("wal") => return Ok(()),
            Ok(mode) => return Err(StoreError::Wal(format!("journal_mode is {mode}"))),
            Err(e)
                if matches!(
                    e.sqlite_error_code(),
                    Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
                ) && Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Creates `dir` (and missing parents) with mode `0700`, and tightens it to `0700`.
fn ensure_private_dir(dir: &Path) -> Result<(), StoreError> {
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(|e| StoreError::io("create dir", dir, e))?;
    fs::set_permissions(dir, Permissions::from_mode(0o700))
        .map_err(|e| StoreError::io("chmod 0700", dir, e))
}

/// Creates the database file with mode `0600` if missing, else tightens it to
/// `0600` by path. It never opens an existing database: closing any descriptor of
/// a file drops every POSIX lock the process holds on it, so a second
/// [`Store::open`] in a process (an [`events::EventFeed`] next to a `Core`) would
/// strip the SHARED lock of its open WAL connections, and other processes could
/// then lock the database exclusively while it is in use
/// (<https://sqlite.org/howtocorrupt.html>, §2.2).
fn ensure_private_db(db: &Path) -> Result<(), StoreError> {
    let private = Permissions::from_mode(0o600);
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(db)
    {
        // A new file: no connection of this process can hold locks on it.
        Ok(file) => file.set_permissions(private),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => fs::set_permissions(db, private),
        Err(e) => return Err(StoreError::io("open", db, e)),
    }
    .map_err(|e| StoreError::io("chmod 0600", db, e))
}

/// Creates `file` with mode `0600` if missing, tightens it to `0600`, and returns it.
pub(crate) fn ensure_private_file(file: &Path) -> Result<fs::File, StoreError> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(file)
        .map_err(|e| StoreError::io("open", file, e))?;
    f.set_permissions(Permissions::from_mode(0o600))
        .map_err(|e| StoreError::io("chmod 0600", file, e))?;
    Ok(f)
}
