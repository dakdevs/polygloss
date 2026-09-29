//! Data paths, store bootstrap, schema v1 and migrations (T1.10, design §7.1, §7.2, §13.2).
//!
//! Isolation: every test first calls `isolate_process()`, which points `HOME`,
//! `POLYGLOSS_DATA_DIR`, `XDG_CONFIG_HOME` and the git config at a per-process temp
//! dir, so nothing here can reach the real `~/Library` or `~/.config`. Tests then
//! build their own `DataPaths` from a fresh temp dir with `DataPaths::resolve_with`,
//! so they stay independent even when a runner shares one process between tests.
//! (T1.2 owns `polygloss_core::testing::Sandbox`; later tasks can switch to it.)
//!
//! The multi-process tests re-run this test binary as children (`child_process_entry`,
//! a no-op unless `POLYGLOSS_STORE_CHILD` is set).

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use polygloss_core::paths::{DataPaths, PathsError, SOCKET_PATH_MAX, socket_path_fits};
use polygloss_core::store::migrations::{LATEST_VERSION, migrations};
use polygloss_core::store::{Store, StoreError};
use rusqlite::{Connection, OptionalExtension};
use rusqlite_migration::{M, Migrations};

// ---------------------------------------------------------------------------
// Isolation helpers

/// Points the process env at a throwaway sandbox (once per process).
fn isolate_process() -> &'static Path {
    static ROOT: OnceLock<tempfile::TempDir> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = tempfile::Builder::new()
            .prefix("polygloss-store-test-")
            .tempdir()
            .unwrap();
        let home = root.path().join("home");
        let data = root.path().join("data");
        let config = home.join(".config");
        let cache = home.join(".cache");
        for dir in [&home, &data, &config, &cache] {
            fs::create_dir_all(dir).unwrap();
        }
        let git_config = root.path().join("gitconfig");
        fs::write(&git_config, "").unwrap();
        // SAFETY: integration tests may mutate process env (plan M1 conventions); this
        // runs once, before any test reads the env.
        unsafe {
            std::env::set_var("HOME", &home);
            std::env::set_var("POLYGLOSS_DATA_DIR", &data);
            std::env::set_var("XDG_CONFIG_HOME", &config);
            std::env::set_var("XDG_CACHE_HOME", &cache);
            std::env::set_var("GIT_CONFIG_GLOBAL", &git_config);
            std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        }
        root
    })
    .path()
}

/// A fresh temp root plus `DataPaths` for `<root>/data/nested` and `<root>/home`.
fn fresh_paths() -> (tempfile::TempDir, DataPaths) {
    isolate_process();
    let root = tempfile::Builder::new()
        .prefix("polygloss-store-")
        .tempdir()
        .unwrap();
    let paths = paths_for(root.path());
    (root, paths)
}

fn paths_for(root: &Path) -> DataPaths {
    let vars: HashMap<&str, OsString> = HashMap::from([
        ("HOME", root.join("home").into_os_string()),
        (
            "POLYGLOSS_DATA_DIR",
            root.join("data").join("nested").into_os_string(),
        ),
    ]);
    DataPaths::resolve_with(|k| vars.get(k).cloned()).unwrap()
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

fn user_version(db: &Path) -> u32 {
    Connection::open(db)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

// ---------------------------------------------------------------------------
// Paths

#[test]
fn data_dir_env_override() {
    let root = isolate_process();

    // The real env (set by the sandbox): POLYGLOSS_DATA_DIR wins for the data dir;
    // cache, logs and config stay under the sandbox HOME / XDG_CONFIG_HOME.
    let p = DataPaths::resolve().unwrap();
    let data = root.join("data");
    let home = root.join("home");
    assert_eq!(p.data_dir, data);
    assert_eq!(p.db, data.join("polygloss.db"));
    assert_eq!(p.db_lock, data.join("polygloss.db.lock"));
    assert_eq!(p.socket, data.join("polygloss.sock"));
    assert_eq!(p.app_lock, data.join("app.lock"));
    assert_eq!(p.bin_dir, data.join("bin"));
    assert_eq!(p.config_dir, home.join(".config").join("polygloss"));
    assert!(p.cache_dir.starts_with(&home), "{:?}", p.cache_dir);
    assert!(p.logs_dir.starts_with(&home), "{:?}", p.logs_dir);

    // Without the override: the macOS Application Support layout of §13.2.
    let env = |pairs: &[(&'static str, &str)]| {
        let map: HashMap<&str, OsString> =
            pairs.iter().map(|(k, v)| (*k, OsString::from(v))).collect();
        DataPaths::resolve_with(move |k| map.get(k).cloned())
    };
    let p = env(&[("HOME", "/Users/u")]).unwrap();
    let support = PathBuf::from("/Users/u/Library/Application Support/polygloss");
    assert_eq!(p.data_dir, support);
    assert_eq!(p.db, support.join("polygloss.db"));
    assert_eq!(
        p.cache_dir,
        PathBuf::from("/Users/u/Library/Caches/polygloss")
    );
    assert_eq!(p.scratch_dir, p.cache_dir.join("scratch"));
    assert_eq!(p.blobs_dir, p.cache_dir.join("blobs"));
    assert_eq!(p.logs_dir, PathBuf::from("/Users/u/Library/Logs/polygloss"));
    assert_eq!(p.config_dir, PathBuf::from("/Users/u/.config/polygloss"));

    // Override, XDG config, and empty values are treated as unset.
    let p = env(&[
        ("HOME", "/Users/u"),
        ("POLYGLOSS_DATA_DIR", "/tmp/pg-data"),
        ("XDG_CONFIG_HOME", "/tmp/xdg"),
    ])
    .unwrap();
    assert_eq!(p.data_dir, PathBuf::from("/tmp/pg-data"));
    assert_eq!(p.db, PathBuf::from("/tmp/pg-data/polygloss.db"));
    assert_eq!(p.config_dir, PathBuf::from("/tmp/xdg/polygloss"));
    assert_eq!(
        p.cache_dir,
        PathBuf::from("/Users/u/Library/Caches/polygloss")
    );
    let p = env(&[
        ("HOME", "/Users/u"),
        ("POLYGLOSS_DATA_DIR", ""),
        ("XDG_CONFIG_HOME", ""),
    ])
    .unwrap();
    assert_eq!(p.data_dir, support);
    assert_eq!(p.config_dir, PathBuf::from("/Users/u/.config/polygloss"));

    // A relative XDG_CONFIG_HOME is ignored (XDG spec); a relative data dir is an
    // error, since processes with different cwds would split the store.
    let p = env(&[("HOME", "/Users/u"), ("XDG_CONFIG_HOME", "rel/xdg")]).unwrap();
    assert_eq!(p.config_dir, PathBuf::from("/Users/u/.config/polygloss"));
    assert!(matches!(
        env(&[("HOME", "/Users/u"), ("POLYGLOSS_DATA_DIR", "rel/data")]),
        Err(PathsError::NotAbsolute { .. })
    ));
    assert!(matches!(env(&[]), Err(PathsError::NoHome)));
    assert!(matches!(env(&[("HOME", "")]), Err(PathsError::NoHome)));
}

#[test]
fn socket_path_longer_than_104_bytes_is_flagged() {
    let root = isolate_process();
    assert_eq!(
        SOCKET_PATH_MAX, 103,
        "macOS sun_path is 104 bytes incl. NUL"
    );

    // Build socket paths of exactly 103 and 104 bytes inside a real temp dir.
    let base = tempfile::Builder::new()
        .prefix("s")
        .tempdir_in(root)
        .unwrap();
    let with_len = |len: usize| {
        let prefix = base.path().join("d");
        let fill = len - prefix.as_os_str().len() - "/polygloss.sock".len();
        let dir = PathBuf::from(format!("{}{}", prefix.display(), "x".repeat(fill)));
        fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("polygloss.sock");
        assert_eq!(sock.as_os_str().len(), len);
        sock
    };
    let fits = with_len(103);
    let too_long = with_len(104);

    assert!(socket_path_fits(&fits));
    assert!(!socket_path_fits(&too_long));
    // The flag agrees with what the kernel (via std) accepts.
    assert!(UnixListener::bind(&fits).is_ok());
    assert!(UnixListener::bind(&too_long).is_err());

    // DataPaths flags its own socket path.
    let data_dir = too_long.parent().unwrap().to_path_buf();
    let vars: HashMap<&str, OsString> = HashMap::from([
        ("HOME", OsString::from("/Users/u")),
        ("POLYGLOSS_DATA_DIR", data_dir.into_os_string()),
    ]);
    let p = DataPaths::resolve_with(|k| vars.get(k).cloned()).unwrap();
    assert_eq!(p.socket, too_long);
    assert!(p.socket_path_too_long());
    let (_root, short) = fresh_paths();
    assert!(!short.socket_path_too_long());
}

// ---------------------------------------------------------------------------
// Bootstrap

#[test]
fn store_open_creates_dir_0700_and_file_0600() {
    let (_root, paths) = fresh_paths();
    assert!(!paths.data_dir.exists());

    let store = Store::open(&paths).unwrap();
    store
        .write(|tx| {
            tx.execute(
                "INSERT INTO events (at, kind, actor_kind) VALUES (1, 'test', 'system')",
                [],
            )?;
            Ok(())
        })
        .unwrap();

    assert_eq!(mode(&paths.data_dir), 0o700);
    assert_eq!(mode(&paths.db), 0o600);
    assert_eq!(mode(&paths.db_lock), 0o600);
    for suffix in ["-wal", "-shm"] {
        let side = PathBuf::from(format!("{}{suffix}", paths.db.display()));
        assert!(side.exists(), "{side:?} missing");
        assert_eq!(mode(&side), 0o600, "{side:?}");
    }
}

#[test]
fn store_open_tightens_existing_dir_and_file() {
    let (_root, paths) = fresh_paths();
    fs::create_dir_all(&paths.data_dir).unwrap();
    fs::set_permissions(&paths.data_dir, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(&paths.db, b"").unwrap();
    fs::set_permissions(&paths.db, fs::Permissions::from_mode(0o644)).unwrap();

    Store::open(&paths).unwrap();
    assert_eq!(mode(&paths.data_dir), 0o700);
    assert_eq!(mode(&paths.db), 0o600);
}

#[test]
fn store_bootstrap_sets_wal_sync_normal_foreign_keys() {
    let (_root, paths) = fresh_paths();
    let store = Store::open(&paths).unwrap();
    store
        .read(|c| {
            let q = |p: &str| -> Result<String, rusqlite::Error> {
                c.query_row(&format!("PRAGMA {p}"), [], |r| {
                    r.get::<_, rusqlite::types::Value>(0).map(|v| match v {
                        rusqlite::types::Value::Integer(i) => i.to_string(),
                        rusqlite::types::Value::Text(t) => t,
                        other => format!("{other:?}"),
                    })
                })
            };
            assert_eq!(q("journal_mode")?, "wal");
            assert_eq!(q("synchronous")?, "1", "NORMAL");
            assert_eq!(q("foreign_keys")?, "1");
            assert_eq!(q("busy_timeout")?, "5000");
            assert_eq!(q("journal_size_limit")?, (64 * 1024 * 1024).to_string());
            assert_eq!(q("user_version")?, LATEST_VERSION.to_string());
            Ok(())
        })
        .unwrap();
    assert_eq!(LATEST_VERSION, 1);
    assert_eq!(store.bootstrap().version_before, 0);
    assert_eq!(store.bootstrap().version_after, 1);
    assert_eq!(store.bootstrap().backup, None, "no backup of an empty db");

    // Foreign keys are enforced on the store's connection.
    let err = store
        .write(|tx| {
            tx.execute(
                "INSERT INTO file_changes (diff_id, idx, status, old_blob, new_blob) \
                 VALUES ('missing', 0, 'A', 'x', 'y')",
                [],
            )?;
            Ok(())
        })
        .unwrap_err();
    assert!(
        matches!(&err, StoreError::Sqlite(e) if e.to_string().contains("FOREIGN KEY")),
        "{err:?}"
    );

    // Reopening an up-to-date store migrates nothing.
    let again = Store::open(&paths).unwrap();
    assert_eq!(again.bootstrap().version_before, 1);
    assert_eq!(again.bootstrap().backup, None);
}

#[test]
fn store_write_begins_immediate_and_rolls_back_on_error() {
    let (_root, paths) = fresh_paths();
    let store = Store::open(&paths).unwrap();
    let other = Connection::open(&paths.db).unwrap();
    other.busy_timeout(Duration::ZERO).unwrap();

    // Inside `write`, before any statement, the RESERVED lock is already held.
    store
        .write(|_tx| {
            let err = other.execute_batch("BEGIN IMMEDIATE").unwrap_err();
            assert_eq!(
                err.sqlite_error_code(),
                Some(rusqlite::ErrorCode::DatabaseBusy)
            );
            Ok(())
        })
        .unwrap();

    // An Err from the closure rolls the transaction back.
    let err = store
        .write(|tx| {
            tx.execute(
                "INSERT INTO events (at, kind, actor_kind) VALUES (1, 'rolled-back', 'system')",
                [],
            )?;
            Err::<(), _>(StoreError::Integrity("boom".into()))
        })
        .unwrap_err();
    assert!(matches!(err, StoreError::Integrity(_)));
    let n: i64 = store
        .read(|c| Ok(c.query_row("SELECT count(*) FROM events", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(n, 0);

    // The lock is released afterwards.
    other.execute_batch("BEGIN IMMEDIATE; ROLLBACK").unwrap();
}

#[test]
fn store_quick_check_and_checkpoint() {
    let (_root, paths) = fresh_paths();
    let store = Store::open(&paths).unwrap();
    store.quick_check().unwrap();
    for i in 0..50 {
        store
            .write(|tx| {
                tx.execute(
                    "INSERT INTO events (at, kind, actor_kind) VALUES (?1, 'k', 'system')",
                    [i],
                )?;
                Ok(())
            })
            .unwrap();
    }
    store.checkpoint().unwrap();
    // After a passive checkpoint with no readers, a fresh connection sees every row
    // and the checkpoint has nothing left to do.
    let c = Connection::open(&paths.db).unwrap();
    let (busy, log, done): (i64, i64, i64) = c
        .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .unwrap();
    assert_eq!((busy, log - done), (0, 0));
    let n: i64 = c
        .query_row("SELECT count(*) FROM events", [], |r| r.get(0))
        .unwrap();
    assert_eq!(n, 50);
}

#[test]
fn store_quick_check_reports_corruption() {
    let (_root, paths) = fresh_paths();
    {
        let store = Store::open(&paths).unwrap();
        store
            .write(|tx| {
                for i in 0..2000 {
                    tx.execute(
                        "INSERT INTO events (at, kind, actor_kind, payload) \
                         VALUES (?1, 'k', 'system', ?2)",
                        rusqlite::params![i, "x".repeat(200)],
                    )?;
                }
                Ok(())
            })
            .unwrap();
        store.checkpoint().unwrap();
    }
    // Scribble over the cell pointers of a page in the middle of the file: the ~30
    // schema root pages come first, the events data (about 110 pages) after them.
    let mut bytes = fs::read(&paths.db).unwrap();
    let pages = bytes.len() / 4096;
    assert!(pages > 100, "db too small: {pages} pages");
    let page = pages / 2 * 4096;
    for b in &mut bytes[page + 8..page + 2048] {
        *b = 0xA5;
    }
    fs::write(&paths.db, &bytes).unwrap();
    let wal = PathBuf::from(format!("{}-wal", paths.db.display()));
    let _ = fs::remove_file(wal);

    let store = Store::open(&paths).unwrap();
    let err = store.quick_check().unwrap_err();
    assert!(matches!(err, StoreError::Integrity(_)), "{err:?}");
}

// ---------------------------------------------------------------------------
// Schema

/// The ```sql block under "### 7.2 Schema (v1)" in docs/design.md.
fn design_schema_block() -> String {
    let design =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/design.md"))
            .unwrap();
    let start = design.find("### 7.2 Schema (v1)").expect("§7.2 heading");
    let rest = &design[start..];
    let open = rest.find("```sql\n").expect("sql fence") + "```sql\n".len();
    let close = rest[open..].find("\n```").expect("closing fence");
    rest[open..open + close].to_owned()
}

#[test]
fn schema_v1_sql_is_design_7_2_verbatim() {
    let sql = include_str!("../src/store/schema-v1.sql");
    let block = design_schema_block();
    // Only a leading `--` header may precede the copied block.
    let header_end = sql
        .find(block.lines().next().unwrap())
        .expect("block start");
    assert!(
        sql[..header_end]
            .lines()
            .all(|l| l.is_empty() || l.starts_with("--")),
        "non-comment text before the §7.2 block"
    );
    assert_eq!(sql[header_end..].trim_end(), block.trim_end());
}

#[test]
fn store_schema_matches_design() {
    let (_root, paths) = fresh_paths();
    let store = Store::open(&paths).unwrap();
    let schema = store
        .read(|c| {
            let mut stmt = c.prepare(
                "SELECT type, name, tbl_name, sql FROM sqlite_master \
                 WHERE sql IS NOT NULL ORDER BY rowid",
            )?;
            let rows = stmt.query_map([], |r| {
                Ok(format!(
                    "-- {} {} ON {}\n{};\n",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?
                ))
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?.join("\n"))
        })
        .unwrap();
    insta::assert_snapshot!(schema);
}

// ---------------------------------------------------------------------------
// Migrations

const TEST_M1: &str = "CREATE TABLE a (id INTEGER PRIMARY KEY, v TEXT NOT NULL);";
const TEST_M2: &str = "CREATE TABLE b (id INTEGER PRIMARY KEY);";

#[test]
fn store_backup_before_migration() {
    let (_root, paths) = fresh_paths();
    let one = [M::up(TEST_M1)];
    let two = [M::up(TEST_M1), M::up(TEST_M2)];

    let store = Store::open_with_migrations(&paths, &Migrations::from_slice(&one)).unwrap();
    assert_eq!(store.bootstrap().version_before, 0);
    assert_eq!(store.bootstrap().backup, None);
    store
        .write(|tx| {
            tx.execute("INSERT INTO a (v) VALUES ('before')", [])?;
            Ok(())
        })
        .unwrap();
    drop(store);

    let store = Store::open_with_migrations(&paths, &Migrations::from_slice(&two)).unwrap();
    let backup = paths.data_dir.join("polygloss.db.bak-v1");
    assert_eq!(store.bootstrap().version_before, 1);
    assert_eq!(store.bootstrap().version_after, 2);
    assert_eq!(store.bootstrap().backup.as_deref(), Some(backup.as_path()));
    assert_eq!(user_version(&paths.db), 2);

    // The backup is the pre-migration database, private like the store.
    assert_eq!(mode(&backup), 0o600);
    assert_eq!(user_version(&backup), 1);
    let b = Connection::open(&backup).unwrap();
    let v: String = b.query_row("SELECT v FROM a", [], |r| r.get(0)).unwrap();
    assert_eq!(v, "before");
    let has_b: Option<String> = b
        .query_row("SELECT name FROM sqlite_master WHERE name = 'b'", [], |r| {
            r.get(0)
        })
        .optional()
        .unwrap();
    assert_eq!(has_b, None, "backup must predate migration 2");
    let leftovers: Vec<_> = fs::read_dir(&paths.data_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn store_open_rejects_newer_schema() {
    let (_root, paths) = fresh_paths();
    Store::open(&paths).unwrap();
    Connection::open(&paths.db)
        .unwrap()
        .pragma_update(None, "user_version", 7)
        .unwrap();
    let err = Store::open(&paths).unwrap_err();
    assert!(matches!(err, StoreError::Migration(_)), "{err:?}");
    assert_eq!(user_version(&paths.db), 7, "left untouched");
    assert!(!paths.data_dir.join("polygloss.db.bak-v7").exists());
}

#[test]
fn migrations_are_valid() {
    migrations().validate().unwrap();
}

// ---------------------------------------------------------------------------
// Multi-process (RF5)

const CHILD_ENV: &str = "POLYGLOSS_STORE_CHILD";

/// Child mode for the multi-process tests; a no-op in a normal run.
#[test]
fn child_process_entry() {
    let Some(mode) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let go = PathBuf::from(std::env::var_os("POLYGLOSS_STORE_GO").unwrap());
    let deadline = Instant::now() + Duration::from_secs(30);
    while !go.exists() {
        assert!(Instant::now() < deadline, "never got the go signal");
        std::thread::sleep(Duration::from_millis(1));
    }
    let paths = DataPaths::resolve().unwrap();
    let store = Store::open(&paths).unwrap();
    match mode.to_str().unwrap() {
        "open" => {}
        "write" => {
            for i in 0..500 {
                store
                    .write(|tx| {
                        tx.execute(
                            "INSERT INTO events (at, kind, actor_kind) \
                             VALUES (?1, 'test.write', 'system')",
                            [i],
                        )?;
                        Ok(())
                    })
                    .unwrap_or_else(|e| panic!("write {i} failed: {e:?}"));
            }
        }
        other => panic!("unknown child mode {other}"),
    }
    println!(
        "CHILD-RESULT version_before={} version_after={}",
        store.bootstrap().version_before,
        store.bootstrap().version_after
    );
}

/// Spawns `n` children of this test binary in `mode`, releases them at once, and
/// returns each child's `version_before`.
fn run_children(paths: &DataPaths, root: &Path, mode: &str, n: usize) -> Vec<u32> {
    let exe = std::env::current_exe().unwrap();
    let go = root.join("go");
    let home = root.join("home");
    let children: Vec<_> = (0..n)
        .map(|_| {
            Command::new(&exe)
                .args(["--exact", "child_process_entry", "--nocapture"])
                .env(CHILD_ENV, mode)
                .env("POLYGLOSS_STORE_GO", &go)
                .env("POLYGLOSS_DATA_DIR", &paths.data_dir)
                .env("HOME", &home)
                .env("XDG_CONFIG_HOME", home.join(".config"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        })
        .collect();
    // Give every child time to start and spin on the go file.
    std::thread::sleep(Duration::from_millis(300));
    fs::write(&go, b"").unwrap();

    children
        .into_iter()
        .map(|child| {
            let out = child.wait_with_output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(
                out.status.success(),
                "child failed: {}\n{stdout}\n{}",
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
            let line = stdout
                .lines()
                .find(|l| l.starts_with("CHILD-RESULT "))
                .unwrap_or_else(|| panic!("no result line in:\n{stdout}"));
            let before = line
                .split_whitespace()
                .find_map(|kv| kv.strip_prefix("version_before="))
                .unwrap();
            before.parse().unwrap()
        })
        .collect()
}

#[test]
fn store_concurrent_first_open_migrates_once() {
    let (root, paths) = fresh_paths();
    assert!(!paths.db.exists());
    let befores = run_children(&paths, root.path(), "open", 8);
    assert_eq!(befores.len(), 8);
    assert_eq!(
        befores.iter().filter(|v| **v == 0).count(),
        1,
        "exactly one child migrates: {befores:?}"
    );
    assert!(befores.iter().all(|v| *v <= 1), "{befores:?}");
    assert_eq!(user_version(&paths.db), 1);
    let backups: Vec<_> = fs::read_dir(&paths.data_dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".bak-"))
        .collect();
    assert!(backups.is_empty(), "{backups:?}");
}

#[test]
fn store_parallel_writers_never_surface_busy() {
    let (root, paths) = fresh_paths();
    Store::open(&paths).unwrap();
    let befores = run_children(&paths, root.path(), "write", 4);
    assert_eq!(befores, vec![1; 4]);
    let n: i64 = Connection::open(&paths.db)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM events WHERE kind = 'test.write'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 4 * 500);
}
