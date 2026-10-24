//! SQLite state (ARCHITECTURE.md §2 ADR 0004): tokens, sessions, jobs, experiments,
//! profiles, secrets, splits and the audit chain.
//!
//! One connection lives on a dedicated thread; async code sends it closures through
//! [`Db::call`], so SQLite never blocks the async runtime and every statement is serialized
//! (SQLite has one writer anyway). Command-line subcommands open their own connection with
//! [`open_connection`] (WAL mode and a busy timeout make that safe while a server runs).
//!
//! The database file is created 0600 inside a 0700 directory before SQLite opens it, so the
//! `-wal`/`-shm` files SQLite creates inherit the same mode.

pub mod audit;
pub mod experiments;
pub mod jobs;
pub mod profiles;
pub mod secrets;
pub mod sessions;
pub mod splits;
pub mod tokens;

use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

/// Errors from the state store. Each message includes its cause, so the cause is not also
/// exposed as `source()`: printing the chain (`{:#}`) would show it twice.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("database: {0}")]
    Sqlite(rusqlite::Error),
    #[error("database file: {0}")]
    Io(std::io::Error),
    #[error("stored JSON is invalid: {0}")]
    Json(serde_json::Error),
    #[error(
        "the database was written by a newer Harness² Studio (schema {found}, this build knows {known})"
    )]
    TooNew { found: i64, known: i64 },
    #[error("the database thread has stopped")]
    Closed,
    #[error("{0}")]
    Invalid(String),
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sqlite(e)
    }
}

impl From<std::io::Error> for DbError {
    fn from(e: std::io::Error) -> Self {
        DbError::Io(e)
    }
}

impl From<serde_json::Error> for DbError {
    fn from(e: serde_json::Error) -> Self {
        DbError::Json(e)
    }
}

/// Result alias for this module.
pub type DbResult<T> = Result<T, DbError>;

/// Schema migrations, applied in order; `PRAGMA user_version` counts how many ran.
const MIGRATIONS: &[&str] = &[
    r"
CREATE TABLE tokens (
    id TEXT PRIMARY KEY,
    secret_hash BLOB NOT NULL,
    name TEXT NOT NULL,
    role TEXT NOT NULL CHECK (role IN ('viewer', 'operator', 'admin')),
    created_at INTEGER NOT NULL,
    created_by TEXT NOT NULL,
    expires_at INTEGER,
    revoked_at INTEGER,
    last_used_at INTEGER
) STRICT;

CREATE TABLE sessions (
    id_hash BLOB PRIMARY KEY,
    token_id TEXT NOT NULL REFERENCES tokens(id) ON DELETE CASCADE,
    created_at INTEGER NOT NULL,
    expires_at INTEGER NOT NULL
) STRICT;
CREATE INDEX sessions_token ON sessions(token_id);

CREATE TABLE experiments (
    name TEXT PRIMARY KEY,
    kind TEXT NOT NULL,
    description TEXT NOT NULL DEFAULT '',
    tags TEXT NOT NULL DEFAULT '[]',
    profile TEXT,
    created_at INTEGER NOT NULL,
    created_by TEXT NOT NULL,
    archived INTEGER NOT NULL DEFAULT 0
) STRICT;

CREATE TABLE profiles (
    name TEXT PRIMARY KEY,
    spec TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE secrets (
    name TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE splits (
    name TEXT PRIMARY KEY,
    info TEXT NOT NULL,
    created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE jobs (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    spec TEXT NOT NULL,
    experiment TEXT,
    status TEXT NOT NULL,
    isolation TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    started_at INTEGER,
    finished_at INTEGER,
    exit_code INTEGER,
    steps TEXT NOT NULL DEFAULT '[]',
    progress TEXT,
    result TEXT,
    error TEXT,
    log_lines INTEGER NOT NULL DEFAULT 0,
    spent_usd REAL,
    pid INTEGER,
    pid_start INTEGER,
    in_container INTEGER NOT NULL DEFAULT 0,
    argv TEXT,
    log_purged INTEGER NOT NULL DEFAULT 0
) STRICT;
CREATE INDEX jobs_status ON jobs(status, seq);
CREATE INDEX jobs_experiment ON jobs(experiment, seq);
CREATE INDEX jobs_kind ON jobs(kind, seq);

CREATE TABLE audit (
    seq INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    actor TEXT NOT NULL,
    action TEXT NOT NULL,
    target TEXT NOT NULL,
    outcome TEXT NOT NULL,
    ip TEXT,
    request_id TEXT,
    detail TEXT,
    prev_hash TEXT NOT NULL,
    hash TEXT NOT NULL
) STRICT;
",
    r"
-- The boot the recorded pid belongs to (/proc/sys/kernel/random/boot_id): start times are
-- clock ticks since boot, so a pid and start time from another boot identify nothing.
ALTER TABLE jobs ADD COLUMN pid_boot TEXT;
",
];

/// Schema version this build writes.
#[must_use]
pub fn schema_version() -> i64 {
    i64::try_from(MIGRATIONS.len()).unwrap_or(i64::MAX)
}

/// Open (creating if needed) the database at `path`: 0700 directory, 0600 file, WAL,
/// foreign keys, busy timeout, migrations.
///
/// # Errors
/// I/O, SQLite or schema-version errors.
pub fn open_connection(path: &Path) -> DbResult<Connection> {
    if let Some(dir) = path.parent() {
        crate::util::ensure_private_dir(dir)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
    {
        Ok(f) => f.set_permissions(std::fs::Permissions::from_mode(0o600))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let meta = std::fs::symlink_metadata(path)?;
            if !meta.is_file() {
                return Err(DbError::Invalid(format!(
                    "{} is not a regular file",
                    path.display()
                )));
            }
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Err(e) => return Err(e.into()),
    }
    let mut conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    conn.busy_timeout(Duration::from_secs(10))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "secure_delete", "ON")?;
    migrate(&mut conn)?;
    for suffix in ["-wal", "-shm"] {
        let mut p = path.as_os_str().to_owned();
        p.push(suffix);
        crate::util::chmod_if_exists(&PathBuf::from(p), 0o600)?;
    }
    Ok(conn)
}

fn migrate(conn: &mut Connection) -> DbResult<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let known = schema_version();
    if current > known {
        return Err(DbError::TooNew {
            found: current,
            known,
        });
    }
    let start = usize::try_from(current).unwrap_or(0);
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(start) {
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Exclusive)?;
        tx.execute_batch(sql)?;
        let version = i64::try_from(i + 1).unwrap_or(i64::MAX);
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;
    }
    Ok(())
}

type Task = Box<dyn FnOnce(&mut Connection) + Send + 'static>;

/// Async handle to the database thread. Cheap to clone.
#[derive(Clone)]
pub struct Db {
    tx: std::sync::mpsc::Sender<Task>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Db")
    }
}

impl Db {
    /// Open the database and start its thread.
    ///
    /// # Errors
    /// See [`open_connection`]; also when the thread cannot be spawned.
    pub fn open(path: &Path) -> DbResult<Db> {
        let conn = open_connection(path)?;
        let (tx, rx) = std::sync::mpsc::channel::<Task>();
        std::thread::Builder::new()
            .name("h2s-db".into())
            .spawn(move || {
                let mut conn = conn;
                while let Ok(task) = rx.recv() {
                    // A panicking closure must not take the whole store down with it; the
                    // caller sees a closed reply channel and reports an internal error.
                    let run = std::panic::AssertUnwindSafe(|| task(&mut conn));
                    if std::panic::catch_unwind(run).is_err() {
                        tracing::error!("a database task panicked");
                    }
                }
            })?;
        Ok(Db { tx })
    }

    /// Run `f` on the database thread and await its result.
    ///
    /// # Errors
    /// The closure's error, or [`DbError::Closed`] when the thread is gone.
    pub async fn call<R, F>(&self, f: F) -> DbResult<R>
    where
        R: Send + 'static,
        F: FnOnce(&mut Connection) -> DbResult<R> + Send + 'static,
    {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        let task: Task = Box::new(move |conn| {
            let _ = reply_tx.send(f(conn));
        });
        self.tx.send(task).map_err(|_| DbError::Closed)?;
        reply_rx.await.map_err(|_| DbError::Closed)?
    }

    /// `SELECT 1` within `timeout` (readiness probe).
    pub async fn ping(&self, timeout: Duration) -> bool {
        let fut = self.call(|c| Ok(c.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?));
        matches!(tokio::time::timeout(timeout, fut).await, Ok(Ok(1)))
    }
}

/// Run `f` inside an IMMEDIATE transaction (takes the write lock up front, so a read-then-
/// write sequence such as the audit chain cannot interleave with another writer).
///
/// # Errors
/// Errors from `f` (the transaction rolls back) or from commit.
pub fn write_tx<R>(
    conn: &mut Connection,
    f: impl FnOnce(&rusqlite::Transaction<'_>) -> DbResult<R>,
) -> DbResult<R> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    let r = f(&tx)?;
    tx.commit()?;
    Ok(r)
}

/// Opaque page cursors: base64url of a decimal row key.
pub mod cursor {
    /// Encode a row key.
    #[must_use]
    pub fn encode(key: i64) -> String {
        crate::util::b64url(key.to_string().as_bytes())
    }

    /// Decode a cursor; `None` when it was not produced by [`encode`].
    #[must_use]
    pub fn decode(s: &str) -> Option<i64> {
        let bytes = crate::util::b64url_decode(s)?;
        let text = std::str::from_utf8(&bytes).ok()?;
        if text.is_empty() || text.len() > 20 || !text.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        text.parse().ok()
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;

    /// A fresh database in a temp dir (kept alive by the returned guard).
    pub(crate) fn conn() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let c = open_connection(&dir.path().join("db/studio.db")).unwrap();
        (dir, c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn open_sets_modes_and_migrates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/studio.db");
        let c = open_connection(&path).unwrap();
        let v: i64 = c
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, schema_version());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let dmode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(dmode, 0o700);
        drop(c);
        // Re-opening is idempotent.
        open_connection(&path).unwrap();
    }

    #[test]
    fn refuses_newer_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("studio.db");
        let c = open_connection(&path).unwrap();
        c.pragma_update(None, "user_version", 999).unwrap();
        drop(c);
        assert!(matches!(
            open_connection(&path),
            Err(DbError::TooNew { found: 999, .. })
        ));
    }

    #[test]
    fn refuses_symlinked_db() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.db");
        std::fs::write(&real, b"").unwrap();
        let link = dir.path().join("studio.db");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert!(open_connection(&link).is_err());
    }

    #[tokio::test]
    async fn async_handle_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("studio.db")).unwrap();
        assert!(db.ping(Duration::from_secs(5)).await);
        let n: i64 = db
            .call(|c| Ok(c.query_row("SELECT 40 + 2", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert_eq!(n, 42);
        let err = db
            .call(|c| Ok(c.execute("SELECT nope FROM nowhere", [])?))
            .await;
        assert!(err.is_err());
    }

    proptest! {
        #[test]
        fn cursor_round_trips(k in 0i64..i64::MAX) {
            prop_assert_eq!(cursor::decode(&cursor::encode(k)), Some(k));
        }

        #[test]
        fn cursor_decode_never_panics(s in ".{0,40}") {
            let _ = cursor::decode(&s);
        }
    }
}
