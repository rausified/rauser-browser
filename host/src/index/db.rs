//! Connection factory and format 9 schema for `index.db` (brwr-b2e, brwr-7vf).
//!
//! Every connection is opened per operation through [`open`], which applies
//! the per-connection pragmas, creates the schema on first write-capable
//! open, and clears a legacy (pre-format-9) file without ever opening it for
//! writing. Every mutation runs in [`write_txn`] (`BEGIN IMMEDIATE`, with a
//! bounded retry at BEGIN), and every read in [`read_txn`], which can never
//! be upgraded into a writer. `host/clippy.toml` forbids starting a
//! transaction or running a raw SQL batch anywhere else.
//!
//! A busy or locked database never reaches a caller as SQLite's own wording:
//! it becomes [`DbError::Busy`], whose text is the existing "artifact history
//! is unavailable" family.

// Wired into `ArtifactIndex` by the store, catch-up and query lanes.
#![cfg_attr(not(test), allow(dead_code))]
// The one module allowed to begin transactions and run SQL batches.
#![allow(clippy::disallowed_methods)]

use std::fmt;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use rusqlite::{Connection, ErrorCode, OpenFlags, Transaction, TransactionBehavior};

/// The database file inside the cache directory.
pub(crate) const DB_FILE: &str = "index.db";
/// `PRAGMA application_id`: 'brwr'.
pub(crate) const APPLICATION_ID: u32 = 0x6272_7772;
/// `PRAGMA user_version`: the cache format, 9.
pub(crate) const SCHEMA_VERSION: i64 = super::STATE_VERSION as i64;
/// How long one statement waits for another connection's lock.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Retries of `BEGIN IMMEDIATE` after the busy timeout has expired.
pub(crate) const BEGIN_RETRIES: u32 = 3;
const JOURNAL_SIZE_LIMIT: i64 = 8 * 1024 * 1024;
/// Sidecars removed, in this order, before a legacy main file.
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// The format 9 schema (DESIGN §5.5). Public tables first, then the
/// `brauser_`-prefixed internal ones. The rebuild's `brauser_stage_*` tables
/// exist only while a staged rebuild runs and are created by it.
pub(crate) const SCHEMA: &str = "\
CREATE TABLE IF NOT EXISTS artifacts(
  artifact_id TEXT NOT NULL UNIQUE, artifact_type TEXT NOT NULL, site TEXT NOT NULL,
  title TEXT NOT NULL, url TEXT NOT NULL, first_seen TEXT, last_seen TEXT,
  focused_secs INTEGER, background_secs INTEGER, opens INTEGER, days_opened INTEGER,
  last_mode TEXT, why TEXT);
CREATE INDEX IF NOT EXISTS artifacts_by_type ON artifacts(artifact_type, artifact_id);

CREATE TABLE IF NOT EXISTS visits(
  event_id TEXT PRIMARY KEY, artifact_id TEXT NOT NULL, occurred_at TEXT,
  local_date TEXT NOT NULL, focused_secs INTEGER, background_secs INTEGER,
  opens INTEGER, mode TEXT, title TEXT, url TEXT);
CREATE INDEX IF NOT EXISTS visits_by_artifact ON visits(artifact_id, occurred_at);
CREATE INDEX IF NOT EXISTS visits_by_date ON visits(local_date, artifact_id);

CREATE TABLE IF NOT EXISTS edges(
  source_artifact_id TEXT NOT NULL, target_artifact_id TEXT NOT NULL, kind TEXT NOT NULL,
  weight INTEGER NOT NULL, last_seen TEXT NOT NULL,
  PRIMARY KEY(source_artifact_id, target_artifact_id, kind)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS edges_by_target ON edges(target_artifact_id, kind);

CREATE TABLE IF NOT EXISTS url_provenance(
  event_id TEXT NOT NULL, artifact_id TEXT NOT NULL, role TEXT NOT NULL,
  url TEXT NOT NULL, stripped_params TEXT,
  PRIMARY KEY(event_id, role)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS url_provenance_by_artifact ON url_provenance(artifact_id, role);

CREATE TABLE IF NOT EXISTS dirty_artifacts(
  artifact_id TEXT PRIMARY KEY, generation INTEGER NOT NULL) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS brauser_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS brauser_events(
  event_id TEXT PRIMARY KEY, artifact_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('activity','why','legacy','summary')),
  order_key BLOB NOT NULL UNIQUE, local_date TEXT NOT NULL,
  log_path TEXT, log_line INTEGER, sequence INTEGER,
  container_id TEXT,
  has_refs INTEGER NOT NULL DEFAULT 0,
  body TEXT NOT NULL,
  body_sha256 BLOB NOT NULL) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS brauser_events_by_artifact ON brauser_events(artifact_id, order_key);
CREATE INDEX IF NOT EXISTS brauser_events_by_container ON brauser_events(container_id, order_key)
  WHERE container_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS brauser_events_by_day ON brauser_events(kind, local_date, order_key);

CREATE TABLE IF NOT EXISTS brauser_group_state(
  artifact_id TEXT PRIMARY KEY,
  latest_key BLOB NOT NULL,
  latest_at TEXT NOT NULL,
  why_at TEXT,
  rolled_up INTEGER NOT NULL,
  file_path TEXT,
  render_sha256 BLOB) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS brauser_artifact_days(
  artifact_id TEXT NOT NULL, local_date TEXT NOT NULL,
  focused_secs INTEGER NOT NULL, background_secs INTEGER NOT NULL, opens INTEGER NOT NULL,
  own_focused_secs INTEGER NOT NULL, own_background_secs INTEGER NOT NULL,
  own_opens INTEGER NOT NULL, summaries INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(artifact_id, local_date)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS brauser_days_by_date
  ON brauser_artifact_days(local_date, focused_secs DESC, artifact_id);

CREATE TABLE IF NOT EXISTS brauser_logs(
  path TEXT NOT NULL, purpose TEXT NOT NULL CHECK(purpose IN ('events','gaps')),
  bytes INTEGER NOT NULL, modified_ns INTEGER NOT NULL,
  parsed_bytes INTEGER NOT NULL, parsed_lines INTEGER NOT NULL, tail_sha256 BLOB NOT NULL,
  PRIMARY KEY(path, purpose)) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS brauser_gaps(
  gap_id TEXT PRIMARY KEY, marker TEXT NOT NULL, log_path TEXT, log_line INTEGER,
  start_date TEXT, end_date TEXT) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS brauser_gaps_by_date ON brauser_gaps(start_date, end_date);
";

/// How [`open`] may treat the cache directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Create the directory, the file and the schema as needed, and clear a
    /// legacy file. Only for paths that are about to commit ingested data.
    ReadWrite,
    /// Open an existing format 9 database. Never creates, deletes or writes
    /// a file; anything else is reported without being touched.
    IfExists,
}

/// Index database failures that callers distinguish.
#[derive(Debug)]
pub(crate) enum DbError {
    /// Another connection held the write lock past the busy timeout and the
    /// BEGIN retries (or a statement stayed locked).
    Busy,
    /// `index.db` predates format 9; the next write-capable open replaces it.
    Legacy,
    /// The path cannot be used (a directory, a symlink, a foreign WAL-mode
    /// database, an unsupported format, or a legacy file that could not be
    /// cleared). Nothing was changed.
    Unavailable(String),
}

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str(
                "artifact history is unavailable: another process is updating the index",
            ),
            Self::Legacy => f.write_str(
                "artifact history is unavailable: index.db predates cache format 9 and is rebuilt on the next delivery",
            ),
            Self::Unavailable(reason) => {
                write!(f, "artifact history is unavailable: {reason}")
            }
        }
    }
}

impl std::error::Error for DbError {}

/// What the bytes at `index.db` are, read with no SQLite connection open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileState {
    Missing,
    /// Zero bytes: pre-created by a host that has not committed the schema.
    Empty,
    Directory,
    /// A symlink or another non-regular file.
    NotRegular,
    /// SQLite with this cache's `application_id`.
    Ours,
    /// Not ours and not in WAL mode: safe to remove with its sidecars.
    Legacy,
    /// Another application's WAL-mode database: never touched.
    ForeignWal,
}

/// How a legacy file was cleared.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cleared {
    Removed,
    /// The delete failed (Windows: a pre-upgrade host holds the file), so its
    /// tables were dropped in place.
    DroppedInPlace,
}

/// Serializes replacing a legacy `index.db` or creating a missing one.
const OPEN_LOCK: &str = "index-open.lock";

/// Take `<cache>/index-open.lock`, waiting while another host process is
/// replacing or creating `index.db`. Released when the file is dropped.
fn open_lock(cache_dir: &Path) -> Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = cache_dir.join(OPEN_LOCK);
    let file = options
        .open(&lock)
        .map_err(|error| unavailable(&lock, error))?;
    file.lock().map_err(|error| unavailable(&lock, error))?;
    Ok(file)
}

pub(crate) fn db_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(DB_FILE)
}

/// Opens `index.db` in `cache_dir`. `ReadWrite` always returns `Some`;
/// `IfExists` returns `None` when there is no database yet.
pub(crate) fn open(cache_dir: &Path, mode: Mode) -> Result<Option<Connection>> {
    open_with(cache_dir, mode, &mut |path| fs::remove_file(path))
}

fn open_with(
    cache_dir: &Path,
    mode: Mode,
    remove: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> Result<Option<Connection>> {
    let path = db_path(cache_dir);
    // Held from deciding to create or replace the file until it is in WAL
    // with its schema, so another host never sees it half made.
    let mut replacing: Option<fs::File> = None;
    let state = match mode {
        Mode::IfExists => match detect_legacy(&path).map_err(|error| unavailable(&path, error))? {
            FileState::Missing | FileState::Empty => return Ok(None),
            FileState::Legacy => return Err(DbError::Legacy.into()),
            state => state,
        },
        Mode::ReadWrite => {
            fs::create_dir_all(cache_dir).map_err(|error| unavailable(cache_dir, error))?;
            let state = detect_legacy(&path).map_err(|error| unavailable(&path, error))?;
            if matches!(
                state,
                FileState::Legacy | FileState::Missing | FileState::Empty
            ) {
                // Replacing or creating the file is serialized across host
                // processes, and decided again under the lock: two first
                // opens of a legacy file must not both clear it, or the
                // second deletes the first's new database and its WAL
                // (SQLITE_IOERR_DELETE_NOENT), and two must not both switch
                // a new file to WAL, which SQLite answers with an immediate
                // SQLITE_BUSY. A current file never waits.
                replacing = Some(open_lock(cache_dir)?);
                let state = detect_legacy(&path).map_err(|error| unavailable(&path, error))?;
                let state = match state {
                    FileState::Legacy => confirm_legacy(&path)?,
                    state => state,
                };
                match state {
                    FileState::Legacy => {
                        if clear_legacy(&path, remove)? == Cleared::Removed {
                            precreate(&path).map_err(|error| unavailable(&path, error))?;
                        }
                        FileState::Ours
                    }
                    FileState::Missing => {
                        precreate(&path).map_err(|error| unavailable(&path, error))?;
                        FileState::Empty
                    }
                    state => state,
                }
            } else {
                state
            }
        }
    };
    match state {
        FileState::Directory => {
            return Err(DbError::Unavailable(format!("{} is a directory", path.display())).into());
        }
        FileState::NotRegular => {
            return Err(
                DbError::Unavailable(format!("{} is not a regular file", path.display())).into(),
            );
        }
        FileState::ForeignWal => {
            return Err(DbError::Unavailable(format!(
                "{} is another application's database",
                path.display()
            ))
            .into());
        }
        FileState::Missing | FileState::Empty | FileState::Ours | FileState::Legacy => {}
    }

    // Not SQLITE_OPEN_NOFOLLOW: it refuses a symlink anywhere in the path
    // (macOS's /var, a relocated home). `detect_legacy` refuses a symlink at
    // `index.db` itself.
    let mut flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if mode == Mode::ReadWrite {
        flags |= OpenFlags::SQLITE_OPEN_CREATE;
    }
    let mut conn = Connection::open_with_flags(&path, flags).map_err(sqlite)?;
    configure(&conn).map_err(sqlite)?;
    match mode {
        Mode::ReadWrite => ensure_schema(&mut conn)?,
        Mode::IfExists => {
            let (application_id, user_version) = identity(&conn).map_err(sqlite)?;
            check_identity(application_id, user_version)?;
        }
    }
    set_journal_mode(&conn)?;
    drop(replacing);
    Ok(Some(conn))
}

/// An empty format 9 store in memory, for a read over a history that has
/// nothing to commit yet: `index.db` is created only to commit ingested
/// data, so its absence after a successful catch-up means "no history".
pub(crate) fn empty() -> Result<Connection> {
    let mut conn = Connection::open_in_memory().map_err(sqlite)?;
    configure(&conn).map_err(sqlite)?;
    ensure_schema(&mut conn)?;
    Ok(conn)
}

fn unavailable(path: &Path, error: io::Error) -> anyhow::Error {
    DbError::Unavailable(format!("{}: {error}", path.display())).into()
}

fn sqlite(error: rusqlite::Error) -> anyhow::Error {
    map_busy(error.into())
}

/// Unix: owner-only before SQLite first opens it (SQLite gives its `-wal`
/// and `-shm` the main file's mode). Another host's pre-create is kept.
fn precreate(path: &Path) -> io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

/// The per-connection pragmas other than the journal mode.
fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(BUSY_TIMEOUT)?;
    conn.pragma_update(None, "foreign_keys", false)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT)?;
    Ok(())
}

/// WAL, or `DELETE` with a warning where the file system refuses WAL (a
/// redirected or network folder). Set after the schema exists, so a WAL-mode
/// file always carries this cache's `application_id` in its header.
fn set_journal_mode(conn: &Connection) -> Result<()> {
    let mode = |value: &str| {
        conn.pragma_update_and_check(None, "journal_mode", value, |row| row.get::<_, String>(0))
    };
    // Switching to WAL takes an exclusive lock, and SQLite reports a
    // conflicting switch as SQLITE_BUSY at once, without the busy handler.
    // Another host's switch makes this one unnecessary, so a busy answer is
    // retried within the busy timeout until the file reads as WAL.
    let started = std::time::Instant::now();
    let refused = loop {
        match mode("WAL") {
            Ok(current) if current.eq_ignore_ascii_case("wal") => return Ok(()),
            Ok(current) => break current,
            Err(error) if is_busy(&error) => {
                let current = conn
                    .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                    .unwrap_or_default();
                if current.eq_ignore_ascii_case("wal") {
                    return Ok(());
                }
                if started.elapsed() >= BUSY_TIMEOUT {
                    return Err(DbError::Busy.into());
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => break error.to_string(),
        }
    };
    tracing::warn!(
        journal_mode = %refused,
        "index.db refused WAL; using the DELETE journal, which blocks readers during writes"
    );
    mode("DELETE").map_err(sqlite)?;
    Ok(())
}

fn identity(conn: &Connection) -> rusqlite::Result<(i64, i64)> {
    let application_id = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    let user_version = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok((application_id, user_version))
}

fn check_identity(application_id: i64, user_version: i64) -> Result<()> {
    if application_id == i64::from(APPLICATION_ID) && user_version == SCHEMA_VERSION {
        return Ok(());
    }
    Err(DbError::Unavailable(format!(
        "index.db has an unsupported format (application_id {application_id:#x}, user_version {user_version})"
    ))
    .into())
}

/// Creates the schema in a fresh file. Re-checked inside the transaction, so
/// hosts opening for the first time at once converge on one schema.
fn ensure_schema(conn: &mut Connection) -> Result<()> {
    write_txn(conn, |tx| {
        let (application_id, user_version) = identity(tx)?;
        if application_id == 0 && user_version == 0 {
            let objects: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite\\_%' ESCAPE '\\'",
                [],
                |row| row.get(0),
            )?;
            if objects == 0 {
                tx.execute_batch(SCHEMA)?;
                tx.execute_batch(&format!(
                    "PRAGMA application_id = {APPLICATION_ID}; PRAGMA user_version = {SCHEMA_VERSION};"
                ))?;
                return Ok(());
            }
        }
        check_identity(application_id, user_version)
    })
}

/// Classifies the file at `path` from its first 100 bytes, with no SQLite
/// connection open (so a foreign file's stale WAL frames are never replayed).
pub(crate) fn detect_legacy(path: &Path) -> io::Result<FileState> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(FileState::Missing),
        Err(error) => return Err(error),
    };
    let file_type = metadata.file_type();
    if file_type.is_dir() {
        return Ok(FileState::Directory);
    }
    if !file_type.is_file() {
        return Ok(FileState::NotRegular);
    }
    let mut header = Vec::with_capacity(100);
    fs::File::open(path)?.take(100).read_to_end(&mut header)?;
    if header.is_empty() {
        return Ok(FileState::Empty);
    }
    if header.len() < 100 || !header.starts_with(b"SQLite format 3\0") {
        return Ok(FileState::Legacy);
    }
    let application_id = u32::from_be_bytes([header[68], header[69], header[70], header[71]]);
    if application_id == APPLICATION_ID {
        return Ok(FileState::Ours);
    }
    // Bytes 18-19 are the write and read format versions: 2 means WAL.
    if header[18] == 2 || header[19] == 2 {
        return Ok(FileState::ForeignWal);
    }
    Ok(FileState::Legacy)
}

/// Re-reads a rollback-mode legacy header under SQLite's shared lock, so a
/// host committing a fresh schema at that moment is never mistaken for a
/// legacy file. Closed again before anything is removed.
fn confirm_legacy(path: &Path) -> Result<FileState> {
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let Ok(conn) = Connection::open_with_flags(path, flags) else {
        return Ok(FileState::Legacy);
    };
    conn.busy_timeout(BUSY_TIMEOUT).map_err(sqlite)?;
    let application_id =
        conn.pragma_query_value(None, "application_id", |row| row.get::<_, i64>(0));
    drop(conn);
    match application_id {
        Ok(id) if id == i64::from(APPLICATION_ID) => Ok(FileState::Ours),
        Err(error) if is_busy(&error) => Err(DbError::Busy.into()),
        _ => Ok(FileState::Legacy),
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Removes a legacy file's `-wal`, `-shm` and `-journal` first, so stale
/// frames can never be replayed into whatever takes the name next, and then
/// the file. When only the main file cannot be removed (Windows: a
/// pre-upgrade host holds it), its tables are dropped in place instead.
pub(crate) fn clear_legacy(
    path: &Path,
    remove: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> Result<Cleared> {
    for suffix in SIDECARS {
        let target = sidecar(path, suffix);
        match remove(&target) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(unavailable(&target, error)),
        }
    }
    match remove(path) {
        Ok(()) => Ok(Cleared::Removed),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Cleared::Removed),
        Err(error) => {
            tracing::warn!(%error, "removing a legacy index.db failed; dropping its tables in place");
            drop_in_place(path)?;
            Ok(Cleared::DroppedInPlace)
        }
    }
}

/// Empties a rollback-mode legacy file so the schema can be created in it.
/// Until this succeeds the database stays unavailable and the next open
/// retries.
fn drop_in_place(path: &Path) -> Result<()> {
    let attempt = || -> Result<()> {
        let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let mut conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(BUSY_TIMEOUT)?;
        write_txn(&mut conn, |tx| {
            let mut statement = tx.prepare(
                "SELECT type, name FROM sqlite_master
                 WHERE type IN ('trigger', 'view', 'table') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\'
                 ORDER BY CASE type WHEN 'trigger' THEN 0 WHEN 'view' THEN 1 ELSE 2 END, name",
            )?;
            let objects = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            for (kind, name) in objects {
                let kind = kind.to_ascii_uppercase();
                let name = name.replace('"', "\"\"");
                tx.execute_batch(&format!("DROP {kind} IF EXISTS \"{name}\";"))?;
            }
            tx.execute_batch("PRAGMA application_id = 0; PRAGMA user_version = 0;")?;
            Ok(())
        })
    };
    attempt().map_err(|error| {
        if matches!(error.downcast_ref::<DbError>(), Some(DbError::Busy)) {
            return error;
        }
        DbError::Unavailable(format!(
            "index.db from an older format could not be replaced: {error:#}"
        ))
        .into()
    })
}

/// Runs `f` in a `BEGIN IMMEDIATE` transaction and commits when it succeeds;
/// an error rolls it back. The write lock is taken up front, so no statement
/// inside ever needs to upgrade a read snapshot (the one WAL path that
/// returns busy without waiting).
pub(crate) fn write_txn<T>(
    conn: &mut Connection,
    f: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    let tx = begin_immediate(conn)?;
    let value = f(&tx).map_err(map_busy)?;
    tx.commit().map_err(sqlite)?;
    #[cfg(test)]
    super::TRANSACTIONS.with(|count| count.set(count.get() + 1));
    Ok(value)
}

fn begin_immediate(conn: &Connection) -> Result<Transaction<'_>> {
    let mut attempt = 0;
    loop {
        match Transaction::new_unchecked(conn, TransactionBehavior::Immediate) {
            Ok(tx) => return Ok(tx),
            Err(error) if is_busy(&error) && attempt < BEGIN_RETRIES => {
                attempt += 1;
                std::thread::sleep(retry_delay(attempt));
            }
            Err(error) => return Err(sqlite(error)),
        }
    }
}

/// Grows with the attempt, with jitter so retrying hosts do not stay in step.
fn retry_delay(attempt: u32) -> Duration {
    let jitter = u64::from(uuid::Uuid::new_v4().as_bytes()[0] % 40);
    Duration::from_millis(u64::from(attempt) * 20 + jitter)
}

/// Runs `f` in a read snapshot. `query_only` is on for its duration, so a
/// write inside fails instead of upgrading the snapshot into a writer.
pub(crate) fn read_txn<T>(
    conn: &mut Connection,
    f: impl FnOnce(&Transaction<'_>) -> Result<T>,
) -> Result<T> {
    conn.pragma_update(None, "query_only", true)
        .map_err(sqlite)?;
    let result = (|| {
        let tx = Transaction::new(conn, TransactionBehavior::Deferred).map_err(sqlite)?;
        let value = f(&tx)?;
        tx.commit().map_err(sqlite)?;
        Ok(value)
    })();
    let reset = conn
        .pragma_update(None, "query_only", false)
        .map_err(sqlite);
    let value = result.map_err(map_busy)?;
    reset?;
    Ok(value)
}

/// True for SQLite's busy and locked codes.
pub(crate) fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error.sqlite_error_code(),
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked)
    )
}

/// Replaces a busy or locked SQLite error anywhere in `error`'s chain with
/// [`DbError::Busy`], so SQLite's wording never reaches a response.
pub(crate) fn map_busy(error: anyhow::Error) -> anyhow::Error {
    let busy = error
        .chain()
        .any(|cause| cause.downcast_ref::<rusqlite::Error>().is_some_and(is_busy));
    if busy { DbError::Busy.into() } else { error }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    fn pragma_i64(conn: &Connection, name: &str) -> i64 {
        conn.pragma_query_value(None, name, |row| row.get(0))
            .unwrap()
    }

    fn db_error(error: &anyhow::Error) -> &DbError {
        error
            .downcast_ref::<DbError>()
            .unwrap_or_else(|| panic!("expected a DbError, got {error:#}"))
    }

    /// A pre-format-9 rollback-journal database with application_id 0, like
    /// the hand-written snapshot writer's, holding a legacy `artifacts`.
    fn write_legacy(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE artifacts(artifact_id TEXT, legacy TEXT);
             INSERT INTO artifacts VALUES ('a', 'old');
             CREATE VIEW legacy_view AS SELECT * FROM artifacts;",
        )
        .unwrap();
        drop(conn);
        let header = fs::read(path).unwrap();
        assert_eq!((header[18], header[19]), (1, 1));
    }

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut statement = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn read_write_open_applies_the_pragmas_and_schema() {
        let dir = TempDir::new().unwrap();
        let cache = dir.path().join("cache");
        let conn = open(&cache, Mode::ReadWrite).unwrap().unwrap();
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
        assert_eq!(pragma_i64(&conn, "synchronous"), 1, "NORMAL");
        assert_eq!(pragma_i64(&conn, "busy_timeout"), 10_000);
        assert_eq!(pragma_i64(&conn, "foreign_keys"), 0);
        assert_eq!(pragma_i64(&conn, "temp_store"), 2, "MEMORY");
        assert_eq!(pragma_i64(&conn, "journal_size_limit"), 8_388_608);
        assert_eq!(pragma_i64(&conn, "page_size"), 4096);
        assert_eq!(pragma_i64(&conn, "application_id"), 0x6272_7772);
        assert_eq!(pragma_i64(&conn, "user_version"), 9);
        let mut statement = conn
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        let tables: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        drop(statement);
        assert_eq!(
            tables,
            [
                "artifacts",
                "brauser_artifact_days",
                "brauser_events",
                "brauser_gaps",
                "brauser_group_state",
                "brauser_logs",
                "brauser_meta",
                "dirty_artifacts",
                "edges",
                "url_provenance",
                "visits",
            ]
        );
        // `artifacts` stays a rowid table for FTS5's content_rowid.
        let artifacts_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name = 'artifacts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(!artifacts_sql.contains("WITHOUT ROWID"));
        assert_eq!(columns(&conn, "artifacts")[12], "why");
        drop(conn);

        // A second open sees the same schema and applies its own pragmas.
        let again = open(&cache, Mode::IfExists).unwrap().unwrap();
        assert_eq!(pragma_i64(&again, "busy_timeout"), 10_000);
        assert_eq!(pragma_i64(&again, "user_version"), 9);
    }

    #[test]
    fn if_exists_never_creates_a_file() {
        let dir = TempDir::new().unwrap();
        let cache = dir.path().join("cache");
        assert!(open(&cache, Mode::IfExists).unwrap().is_none());
        assert!(!cache.exists(), "the cache dir is never created");
        fs::create_dir(&cache).unwrap();
        assert!(open(&cache, Mode::IfExists).unwrap().is_none());
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
    }

    #[test]
    fn if_exists_leaves_garbage_and_legacy_files_byte_identical() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        for bytes in [b"not a database at all".to_vec(), vec![0x5a; 4096], {
            let mut header = b"SQLite format 3\0".to_vec();
            header.resize(8192, 0);
            header
        }] {
            fs::write(&path, &bytes).unwrap();
            let error = open(dir.path(), Mode::IfExists).unwrap_err();
            assert!(matches!(db_error(&error), DbError::Legacy), "{error:#}");
            assert_eq!(fs::read(&path).unwrap(), bytes);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        }

        fs::remove_file(&path).unwrap();
        write_legacy(&path);
        let before = fs::read(&path).unwrap();
        let error = open(dir.path(), Mode::IfExists).unwrap_err();
        assert!(matches!(db_error(&error), DbError::Legacy), "{error:#}");
        assert_eq!(fs::read(&path).unwrap(), before);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn a_directory_at_index_db_is_unavailable_and_untouched() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        fs::create_dir(&path).unwrap();
        fs::write(path.join("keep"), b"user").unwrap();
        for mode in [Mode::ReadWrite, Mode::IfExists] {
            let error = open(dir.path(), mode).unwrap_err();
            assert!(matches!(db_error(&error), DbError::Unavailable(_)));
            assert!(
                error
                    .to_string()
                    .starts_with("artifact history is unavailable")
            );
            assert_eq!(fs::read(path.join("keep")).unwrap(), b"user");
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }

    #[cfg(unix)]
    #[test]
    fn the_database_is_created_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let mode = fs::metadata(db_path(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        drop(conn);
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_at_index_db_is_unavailable_and_untouched() {
        let dir = TempDir::new().unwrap();
        let target = dir.path().join("elsewhere.db");
        fs::write(&target, b"user data").unwrap();
        std::os::unix::fs::symlink(&target, db_path(dir.path())).unwrap();
        for mode in [Mode::ReadWrite, Mode::IfExists] {
            let error = open(dir.path(), mode).unwrap_err();
            assert!(matches!(db_error(&error), DbError::Unavailable(_)));
            assert_eq!(fs::read(&target).unwrap(), b"user data");
        }
    }

    #[test]
    fn two_threads_in_begin_immediate_never_surface_busy() {
        let dir = TempDir::new().unwrap();
        let mut conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        write_txn(&mut conn, |tx| {
            tx.execute(
                "INSERT INTO brauser_meta(key, value) VALUES ('counter', '0')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        drop(conn);

        const THREADS: usize = 4;
        const ROUNDS: usize = 40;
        let barrier = Arc::new(Barrier::new(THREADS));
        let workers: Vec<_> = (0..THREADS)
            .map(|_| {
                let cache = dir.path().to_path_buf();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || -> Result<()> {
                    barrier.wait();
                    for _ in 0..ROUNDS {
                        // Connection per operation, as the index uses it.
                        let mut conn = open(&cache, Mode::ReadWrite)?.expect("read-write");
                        write_txn(&mut conn, |tx| {
                            let value: String = tx.query_row(
                                "SELECT value FROM brauser_meta WHERE key = 'counter'",
                                [],
                                |row| row.get(0),
                            )?;
                            std::thread::sleep(Duration::from_millis(1));
                            let next = value.parse::<u64>()? + 1;
                            tx.execute(
                                "UPDATE brauser_meta SET value = ?1 WHERE key = 'counter'",
                                [next.to_string()],
                            )?;
                            Ok(())
                        })?;
                        let mut reader = open(&cache, Mode::IfExists)?.expect("exists");
                        read_txn(&mut reader, |tx| {
                            tx.query_row("SELECT count(*) FROM brauser_meta", [], |row| {
                                row.get::<_, i64>(0)
                            })?;
                            Ok(())
                        })?;
                    }
                    Ok(())
                })
            })
            .collect();
        for worker in workers {
            if let Err(error) = worker.join().unwrap() {
                panic!("a worker surfaced {error:#}");
            }
        }
        let mut conn = open(dir.path(), Mode::IfExists).unwrap().unwrap();
        let value: String = read_txn(&mut conn, |tx| {
            Ok(tx.query_row(
                "SELECT value FROM brauser_meta WHERE key = 'counter'",
                [],
                |row| row.get(0),
            )?)
        })
        .unwrap();
        assert_eq!(value, (THREADS * ROUNDS).to_string());
    }

    #[test]
    fn concurrent_first_opens_converge_on_one_schema() {
        let dir = TempDir::new().unwrap();
        let barrier = Arc::new(Barrier::new(4));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let cache = dir.path().join("cache");
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    open(&cache, Mode::ReadWrite).map(|conn| conn.is_some())
                })
            })
            .collect();
        for worker in workers {
            assert!(worker.join().unwrap().unwrap());
        }
        let conn = open(&dir.path().join("cache"), Mode::IfExists)
            .unwrap()
            .unwrap();
        assert_eq!(pragma_i64(&conn, "user_version"), 9);
    }

    #[test]
    fn a_read_transaction_is_never_upgraded_into_a_writer() {
        let dir = TempDir::new().unwrap();
        let mut conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let error = read_txn(&mut conn, |tx| {
            tx.execute("INSERT INTO brauser_meta(key, value) VALUES ('k', 'v')", [])?;
            Ok(())
        })
        .unwrap_err();
        assert!(!format!("{error:#}").contains("busy"));
        // The connection is writable again afterwards, and nothing was written.
        write_txn(&mut conn, |tx| {
            let rows: i64 =
                tx.query_row("SELECT count(*) FROM brauser_meta", [], |row| row.get(0))?;
            assert_eq!(rows, 0);
            tx.execute("INSERT INTO brauser_meta(key, value) VALUES ('k', 'v')", [])?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn a_failed_write_transaction_rolls_back() {
        let dir = TempDir::new().unwrap();
        let mut conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let error = write_txn(&mut conn, |tx| -> Result<()> {
            tx.execute("INSERT INTO brauser_meta(key, value) VALUES ('k', 'v')", [])?;
            anyhow::bail!("stop")
        })
        .unwrap_err();
        assert_eq!(error.to_string(), "stop");
        let rows: i64 = read_txn(&mut conn, |tx| {
            Ok(tx.query_row("SELECT count(*) FROM brauser_meta", [], |row| row.get(0))?)
        })
        .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn a_held_write_lock_maps_to_busy_without_sqlite_wording() {
        let dir = TempDir::new().unwrap();
        let mut holder = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let mut waiter = open(dir.path(), Mode::IfExists).unwrap().unwrap();
        waiter.busy_timeout(Duration::from_millis(5)).unwrap();
        let tx = Transaction::new_unchecked(&holder, TransactionBehavior::Immediate).unwrap();
        let error = write_txn(&mut waiter, |_| Ok(())).unwrap_err();
        assert!(matches!(db_error(&error), DbError::Busy), "{error:#}");
        let text = format!("{error:#}");
        assert!(
            text.starts_with("artifact history is unavailable"),
            "{text}"
        );
        assert!(!text.contains("locked") && !text.contains("busy"), "{text}");
        tx.rollback().unwrap();
        write_txn(&mut holder, |_| Ok(())).unwrap();
    }

    #[test]
    fn busy_errors_inside_a_transaction_are_mapped() {
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_BUSY),
            Some("database is locked".into()),
        );
        assert!(is_busy(&busy));
        let mapped = map_busy(anyhow::Error::new(busy).context("reading the index"));
        assert!(matches!(db_error(&mapped), DbError::Busy));
        assert!(!format!("{mapped:#}").contains("locked"));
        let locked = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_LOCKED),
            None,
        );
        assert!(is_busy(&locked));
        assert!(!is_busy(&rusqlite::Error::QueryReturnedNoRows));
        let other = map_busy(anyhow::anyhow!("other"));
        assert_eq!(other.to_string(), "other");
    }

    #[test]
    fn detect_legacy_classifies_from_the_header_alone() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        assert_eq!(detect_legacy(&path).unwrap(), FileState::Missing);
        fs::write(&path, b"").unwrap();
        assert_eq!(detect_legacy(&path).unwrap(), FileState::Empty);
        fs::write(&path, b"garbage").unwrap();
        assert_eq!(detect_legacy(&path).unwrap(), FileState::Legacy);
        fs::remove_file(&path).unwrap();
        write_legacy(&path);
        assert_eq!(detect_legacy(&path).unwrap(), FileState::Legacy);
        fs::remove_file(&path).unwrap();
        drop(open(dir.path(), Mode::ReadWrite).unwrap());
        assert_eq!(detect_legacy(&path).unwrap(), FileState::Ours);
        fs::remove_dir_all(dir.path()).unwrap();
        fs::create_dir_all(&path).unwrap();
        assert_eq!(detect_legacy(&path).unwrap(), FileState::Directory);
    }

    #[test]
    fn legacy_detection_deletes_the_sidecars_before_the_main_file() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        write_legacy(&path);
        for suffix in SIDECARS {
            fs::write(sidecar(&path, suffix), b"stale frames").unwrap();
        }
        let mut removed = Vec::new();
        let cleared = clear_legacy(&path, &mut |target| {
            removed.push(target.to_path_buf());
            fs::remove_file(target)
        })
        .unwrap();
        assert_eq!(cleared, Cleared::Removed);
        assert_eq!(
            removed,
            [
                sidecar(&path, "-wal"),
                sidecar(&path, "-shm"),
                sidecar(&path, "-journal"),
                path.clone(),
            ]
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_read_write_open_replaces_a_legacy_file() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        write_legacy(&path);
        for suffix in SIDECARS {
            fs::write(sidecar(&path, suffix), b"stale frames").unwrap();
        }
        let conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        assert_eq!(pragma_i64(&conn, "user_version"), 9);
        assert!(!columns(&conn, "artifacts").contains(&"legacy".to_string()));
        let rows: i64 = conn
            .query_row("SELECT count(*) FROM artifacts", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 0);
        assert!(!sidecar(&path, "-journal").exists());
        drop(conn);

        // Garbage is legacy too: the cache is disposable.
        fs::remove_file(&path).unwrap();
        fs::write(&path, b"garbage").unwrap();
        let conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        assert_eq!(pragma_i64(&conn, "application_id"), 0x6272_7772);
    }

    #[test]
    fn a_legacy_file_that_cannot_be_deleted_is_dropped_in_place() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        write_legacy(&path);
        let mut refuse_main = |target: &Path| {
            if target == db_path(dir.path()) {
                Err(io::Error::new(io::ErrorKind::PermissionDenied, "held"))
            } else {
                fs::remove_file(target)
            }
        };
        let conn = open_with(dir.path(), Mode::ReadWrite, &mut refuse_main)
            .unwrap()
            .unwrap();
        assert_eq!(pragma_i64(&conn, "user_version"), 9);
        assert_eq!(pragma_i64(&conn, "application_id"), 0x6272_7772);
        let cols = columns(&conn, "artifacts");
        assert!(cols.contains(&"why".to_string()), "{cols:?}");
        assert!(!cols.contains(&"legacy".to_string()), "{cols:?}");
        let views: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type = 'view'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(views, 0);
    }

    #[test]
    fn a_foreign_wal_mode_file_is_left_alone_and_reported_unavailable() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        let foreign = Connection::open(&path).unwrap();
        let mode: String = foreign
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        foreign
            .execute_batch("CREATE TABLE theirs(x); INSERT INTO theirs VALUES (1);")
            .unwrap();
        drop(foreign);
        let before = fs::read(&path).unwrap();
        assert_eq!((before[18], before[19]), (2, 2));
        assert_eq!(detect_legacy(&path).unwrap(), FileState::ForeignWal);
        for mode in [Mode::ReadWrite, Mode::IfExists] {
            let error = open(dir.path(), mode).unwrap_err();
            assert!(
                matches!(db_error(&error), DbError::Unavailable(_)),
                "{error:#}"
            );
            assert_eq!(fs::read(&path).unwrap(), before);
            assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn an_unsupported_format_of_ours_is_unavailable() {
        let dir = TempDir::new().unwrap();
        let mut conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        write_txn(&mut conn, |tx| {
            tx.pragma_update(None, "user_version", 10)?;
            Ok(())
        })
        .unwrap();
        drop(conn);
        for mode in [Mode::ReadWrite, Mode::IfExists] {
            let error = open(dir.path(), mode).unwrap_err();
            assert!(
                matches!(db_error(&error), DbError::Unavailable(_)),
                "{error:#}"
            );
        }
    }
}
