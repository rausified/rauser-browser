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
/// The format 8 snapshot writer's `application_id`: 'BRWR'.
pub(crate) const LEGACY_APPLICATION_ID: u32 = 0x4252_5752;
/// `PRAGMA user_version`: the cache format, 9.
pub(crate) const SCHEMA_VERSION: i64 = super::STATE_VERSION as i64;
/// How long one statement waits for another connection's lock.
pub(crate) const BUSY_TIMEOUT: Duration = Duration::from_millis(10_000);
/// Retries of a busy `BEGIN IMMEDIATE` within one busy timeout.
pub(crate) const BEGIN_RETRIES: u32 = 3;
const JOURNAL_SIZE_LIMIT: i64 = 8 * 1024 * 1024;
/// Sidecars removed, in this order, before a legacy main file.
const SIDECARS: [&str; 3] = ["-wal", "-shm", "-journal"];

/// A test's hook into [`open_with`].
#[cfg(test)]
type OpenHook = Option<Box<dyn FnOnce(&Path)>>;

#[cfg(test)]
thread_local! {
    /// Busy errors in this thread that outlasted every wait and retry and
    /// became [`DbError::Busy`]. The multi-process acceptance expects none.
    pub(crate) static BUSY_SURFACED: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static BUSY_TIMEOUT_OVERRIDE: std::cell::Cell<Option<Duration>> =
        const { std::cell::Cell::new(None) };
    /// Run once, just before this thread's next SQLite open of `index.db`.
    pub(crate) static BEFORE_SQLITE_OPEN: std::cell::RefCell<OpenHook> =
        const { std::cell::RefCell::new(None) };
}

/// [`BUSY_TIMEOUT`], or a test's shorter one for this thread's connections.
fn busy_timeout() -> Duration {
    #[cfg(test)]
    if let Some(timeout) = BUSY_TIMEOUT_OVERRIDE.with(std::cell::Cell::get) {
        return timeout;
    }
    BUSY_TIMEOUT
}

/// Shortens the busy timeout of connections this thread opens until it is
/// dropped, so a test can hold a lock without waiting ten seconds per call.
#[cfg(test)]
pub(crate) struct BusyTimeout(Option<Duration>);

#[cfg(test)]
impl BusyTimeout {
    pub(crate) fn set(timeout: Duration) -> Self {
        Self(BUSY_TIMEOUT_OVERRIDE.with(|cell| cell.replace(Some(timeout))))
    }
}

#[cfg(test)]
impl Drop for BusyTimeout {
    fn drop(&mut self) {
        BUSY_TIMEOUT_OVERRIDE.with(|cell| cell.set(self.0));
    }
}

/// A busy or locked database that outlasted the waits: never SQLite's text.
fn busy() -> anyhow::Error {
    #[cfg(test)]
    BUSY_SURFACED.with(|count| count.set(count.get() + 1));
    DbError::Busy.into()
}

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

/// [`DbError::Busy`]'s text, which a native response may carry.
pub(crate) const BUSY_MESSAGE: &str =
    "artifact history is unavailable: another process is updating the index";

impl fmt::Display for DbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy => f.write_str(BUSY_MESSAGE),
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
    /// Not ours and either not in WAL mode or the format 8 writer's own
    /// ('BRWR'): safe to remove with its sidecars.
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

#[cfg(test)]
thread_local! {
    /// How many times this thread opened `index.db` (tests of reuse).
    pub(crate) static OPENS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn open_with(
    cache_dir: &Path,
    mode: Mode,
    remove: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> Result<Option<Connection>> {
    #[cfg(test)]
    OPENS.with(|opens| opens.set(opens.get() + 1));
    // A file that vanishes between its detection and the SQLite open is
    // decided again, so a missing one is only ever created under the lock.
    let mut vanished = 0;
    loop {
        match open_once(cache_dir, mode, remove)? {
            Opened::Vanished if vanished < 3 => vanished += 1,
            Opened::Vanished => {
                return Err(DbError::Unavailable(format!(
                    "{} keeps disappearing while it is opened",
                    db_path(cache_dir).display()
                ))
                .into());
            }
            Opened::Missing => return Ok(None),
            Opened::Connection(conn) => return Ok(Some(conn)),
        }
    }
}

enum Opened {
    Connection(Connection),
    /// `IfExists`: no database.
    Missing,
    /// `ReadWrite`: the file was gone when SQLite came to open it.
    Vanished,
}

fn open_once(
    cache_dir: &Path,
    mode: Mode,
    remove: &mut dyn FnMut(&Path) -> io::Result<()>,
) -> Result<Opened> {
    let path = db_path(cache_dir);
    // Held from deciding to create or replace the file until it is in WAL
    // with its schema, so another host never sees it half made.
    let mut replacing: Option<fs::File> = None;
    let state = match mode {
        Mode::IfExists => match detect_legacy(&path).map_err(|error| unavailable(&path, error))? {
            FileState::Missing | FileState::Empty => return Ok(Opened::Missing),
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
                //
                // Under the lock the main file's header alone decides: every
                // format 9 host writes its schema and `application_id` in
                // rollback mode before switching to WAL, all while holding
                // this lock, so a header that is not ours is never a fresh
                // schema in flight. No SQLite connection is opened first: it
                // would replay a stale `-wal` left beside a file renamed over
                // ours (a pre-format-9 host's snapshot), read our page 1 from
                // it, and never clear the file.
                replacing = Some(open_lock(cache_dir)?);
                let state = detect_legacy(&path).map_err(|error| unavailable(&path, error))?;
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
    // `index.db` itself. Never SQLITE_OPEN_CREATE: SQLite would create a
    // file that vanished since its detection (a concurrent `discard`, the
    // user clearing the cache) at 0644 and outside the lock; the file exists
    // here, pre-created owner-only under the lock when it was missing.
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    #[cfg(test)]
    if let Some(hook) = BEFORE_SQLITE_OPEN.with(|hook| hook.borrow_mut().take()) {
        hook(&path);
    }
    let mut conn = match Connection::open_with_flags(&path, flags) {
        Ok(conn) => conn,
        Err(error) => {
            if detect_legacy(&path).is_ok_and(|state| state == FileState::Missing) {
                return Ok(match mode {
                    Mode::IfExists => Opened::Missing,
                    Mode::ReadWrite => Opened::Vanished,
                });
            }
            return Err(sqlite(error));
        }
    };
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
    Ok(Opened::Connection(conn))
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

/// Windows refuses to open, create or delete a name while another handle
/// holds it without delete sharing (SQLite's own) or after its delete while
/// some handle is still open (delete pending, reported as access denied).
/// Between hosts opening one cache at once these clear within moments: a
/// header read closes, a deleted legacy file's last handle goes.
fn transient(error: &io::Error) -> bool {
    #[cfg(windows)]
    {
        const ERROR_ACCESS_DENIED: i32 = 5;
        const ERROR_SHARING_VIOLATION: i32 = 32;
        const ERROR_LOCK_VIOLATION: i32 = 33;
        const ERROR_DELETE_PENDING: i32 = 303;
        matches!(
            error.raw_os_error(),
            Some(
                ERROR_ACCESS_DENIED
                    | ERROR_SHARING_VIOLATION
                    | ERROR_LOCK_VIOLATION
                    | ERROR_DELETE_PENDING
            )
        )
    }
    #[cfg(not(windows))]
    {
        let _ = error;
        false
    }
}

/// Runs `op` again while it fails with an error `transient` accepts, within
/// the busy timeout; the error that outlasts it (a real permission problem)
/// is returned.
fn retry_io<T>(
    transient: fn(&io::Error) -> bool,
    mut op: impl FnMut() -> io::Result<T>,
) -> io::Result<T> {
    let started = std::time::Instant::now();
    loop {
        match op() {
            Err(error) if transient(&error) && started.elapsed() < busy_timeout() => {
                std::thread::sleep(Duration::from_millis(10));
            }
            result => return result,
        }
    }
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
    match retry_io(transient, || options.open(path)) {
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error),
    }
}

/// The per-connection pragmas other than the journal mode.
fn configure(conn: &Connection) -> rusqlite::Result<()> {
    conn.busy_timeout(busy_timeout())?;
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
    // An existing WAL file stays WAL: reading the mode takes no lock, while
    // setting it can wait out the busy timeout behind another host's writer.
    let current = conn
        .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
        .unwrap_or_default();
    if current.eq_ignore_ascii_case("wal") {
        return Ok(());
    }
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
                if started.elapsed() >= busy_timeout() {
                    return Err(busy());
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
/// hosts opening for the first time at once converge on one schema. A file
/// whose header already reads as this format needs no write lock: the
/// schema and both pragmas commit together, so they are never seen apart,
/// and waiting behind another connection's write here would spend a read's
/// whole busy budget before its catch-up even begins.
fn ensure_schema(conn: &mut Connection) -> Result<()> {
    let (application_id, user_version) = identity(conn).map_err(sqlite)?;
    if application_id != 0 || user_version != 0 {
        return check_identity(application_id, user_version);
    }
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
/// connection open (so stale WAL frames beside it are never replayed).
/// Windows: a sharing violation or a delete pending is waited out.
pub(crate) fn detect_legacy(path: &Path) -> io::Result<FileState> {
    retry_io(transient, || detect_once(path))
}

fn detect_once(path: &Path) -> io::Result<FileState> {
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
    // Another host may delete the file between the stat and the open (on
    // Windows a delete pending makes that window wide): it is then missing,
    // and a ReadWrite open goes through the locked create path.
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(FileState::Missing),
        Err(error) => return Err(error),
    };
    file.take(100).read_to_end(&mut header)?;
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
    // The format 8 snapshot writer's own file, even if an external SQL
    // consumer switched it to WAL: this cache's to replace.
    if application_id == LEGACY_APPLICATION_ID {
        return Ok(FileState::Legacy);
    }
    // Bytes 18-19 are the write and read format versions: 2 means WAL.
    if header[18] == 2 || header[19] == 2 {
        return Ok(FileState::ForeignWal);
    }
    Ok(FileState::Legacy)
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
        match retry_io(transient, || remove(&target)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(unavailable(&target, error)),
        }
    }
    match remove(path) {
        Ok(()) => Ok(Cleared::Removed),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Cleared::Removed),
        // Windows: already deleted by someone else and pending until its
        // last handle closes; waited out by the detection.
        Err(_) if detect_legacy(path).is_ok_and(|state| state == FileState::Missing) => {
            Ok(Cleared::Removed)
        }
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
        conn.busy_timeout(busy_timeout())?;
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
    #[cfg(test)]
    let changes = conn.total_changes();
    let tx = begin_immediate(conn)?;
    let value = f(&tx).map_err(map_busy)?;
    tx.commit().map_err(sqlite)?;
    #[cfg(test)]
    {
        super::TRANSACTIONS.with(|count| count.set(count.get() + 1));
        let written = conn.total_changes() - changes;
        super::ROWS_WRITTEN.with(|count| count.set(count.get() + written));
    }
    Ok(value)
}

#[cfg(test)]
thread_local! {
    /// `BEGIN IMMEDIATE` attempts on this thread that spent half the busy
    /// timeout or more in SQLite's busy handler (tests of single waits).
    pub(crate) static BEGIN_WAITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// `BEGIN IMMEDIATE`, retried for a busy answer only while the busy timeout
/// has not yet run out overall. The retries cover a busy answer SQLite gives
/// without waiting (a snapshot that moved, WAL recovery); they never wait
/// the full timeout again, so a busy lock surfaces after about one timeout,
/// well inside the extension's 30 s request timeout.
fn begin_immediate(conn: &Connection) -> Result<Transaction<'_>> {
    let started = std::time::Instant::now();
    let mut attempt = 0;
    loop {
        let tried = std::time::Instant::now();
        let result = Transaction::new_unchecked(conn, TransactionBehavior::Immediate);
        #[cfg(test)]
        if tried.elapsed() >= busy_timeout() / 2 {
            BEGIN_WAITS.with(|waits| waits.set(waits.get() + 1));
        }
        match result {
            Ok(tx) => return Ok(tx),
            // Only an answer that came back without waiting is retried. One
            // that already spent most of the busy timeout in SQLite's
            // handler is a held lock: retrying it would wait the timeout
            // again (the handler can give up just short of it).
            Err(error)
                if is_busy(&error)
                    && attempt < BEGIN_RETRIES
                    && tried.elapsed() < busy_timeout() / 2
                    && started.elapsed() < busy_timeout() =>
            {
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

/// Whether SQLite reported the database itself as damaged anywhere in
/// `error`'s chain (`SQLITE_CORRUPT`, `SQLITE_NOTADB`).
pub(crate) fn is_corrupt(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .and_then(rusqlite::Error::sqlite_error_code)
            .is_some_and(|code| {
                matches!(code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
            })
    })
}

/// Delete a damaged `index.db` and its sidecars (sidecars first), under the
/// lock that serializes creating and replacing it, so the next
/// write-capable open creates a fresh one. Where the file cannot be deleted
/// (Windows: another host holds it), its tables are dropped in place, which
/// a badly damaged file may refuse; history then stays unavailable.
pub(crate) fn discard(cache_dir: &Path) -> Result<()> {
    let _replacing = open_lock(cache_dir)?;
    let path = db_path(cache_dir);
    match detect_legacy(&path).map_err(|error| unavailable(&path, error))? {
        FileState::Ours | FileState::Legacy | FileState::Empty => {
            clear_legacy(&path, &mut |target| fs::remove_file(target))?;
            Ok(())
        }
        FileState::Missing => Ok(()),
        FileState::Directory | FileState::NotRegular | FileState::ForeignWal => Err(
            DbError::Unavailable(format!("{} is not this cache's database", path.display())).into(),
        ),
    }
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
    if busy { self::busy() } else { error }
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

    /// A current store deleted between its detection and the SQLite open
    /// (a concurrent `discard`, or the user clearing the cache) is created
    /// again owner-only through the locked branch, never by SQLite at 0644.
    #[cfg(unix)]
    #[test]
    fn a_store_that_vanishes_before_its_open_is_recreated_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        drop(open(dir.path(), Mode::ReadWrite).unwrap().unwrap());
        assert_eq!(
            detect_legacy(&db_path(dir.path())).unwrap(),
            FileState::Ours
        );
        let cache = dir.path().to_path_buf();
        BEFORE_SQLITE_OPEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_: &Path| discard(&cache).unwrap()));
        });
        let conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        assert!(BEFORE_SQLITE_OPEN.with(|hook| hook.borrow().is_none()));
        assert_eq!(pragma_i64(&conn, "user_version"), 9);
        let mode = fs::metadata(db_path(dir.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// A store that vanishes before an `IfExists` open reads as no store.
    #[test]
    fn a_store_that_vanishes_before_an_if_exists_open_is_none() {
        let dir = TempDir::new().unwrap();
        drop(open(dir.path(), Mode::ReadWrite).unwrap().unwrap());
        let cache = dir.path().to_path_buf();
        BEFORE_SQLITE_OPEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |_: &Path| discard(&cache).unwrap()));
        });
        assert!(open(dir.path(), Mode::IfExists).unwrap().is_none());
        assert!(!db_path(dir.path()).exists());
    }

    /// A transient refusal is retried until it clears; one that outlasts the
    /// busy timeout, or any other error, is returned.
    #[test]
    fn transient_io_errors_are_retried_within_the_busy_timeout() {
        let _timeout = BusyTimeout::set(Duration::from_millis(200));
        let interrupted = |error: &io::Error| error.kind() == io::ErrorKind::Interrupted;
        let mut calls = 0;
        let value = retry_io(interrupted, || {
            calls += 1;
            if calls < 4 {
                Err(io::Error::from(io::ErrorKind::Interrupted))
            } else {
                Ok(calls)
            }
        })
        .unwrap();
        assert_eq!(value, 4);

        let started = std::time::Instant::now();
        let error = retry_io(interrupted, || -> io::Result<()> {
            Err(io::Error::from(io::ErrorKind::Interrupted))
        })
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Interrupted);
        assert!(started.elapsed() >= Duration::from_millis(200));

        let mut calls = 0;
        let error = retry_io(interrupted, || -> io::Result<()> {
            calls += 1;
            Err(io::Error::from(io::ErrorKind::NotFound))
        })
        .unwrap_err();
        assert_eq!((error.kind(), calls), (io::ErrorKind::NotFound, 1));
    }

    /// Windows: the refusals another handle causes are transient, a missing
    /// file is not.
    #[cfg(windows)]
    #[test]
    fn windows_sharing_and_delete_pending_refusals_are_transient() {
        for code in [5, 32, 33, 303] {
            assert!(transient(&io::Error::from_raw_os_error(code)), "{code}");
        }
        assert!(!transient(&io::Error::from_raw_os_error(2)));
        assert!(!transient(&io::Error::from(io::ErrorKind::NotFound)));
    }

    /// Windows: a name another handle still holds after its delete (delete
    /// pending) refuses every open and create with access denied until that
    /// handle closes, so detection and the pre-create wait it out.
    #[cfg(windows)]
    #[test]
    fn a_delete_pending_index_db_is_waited_out() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        for step in ["detect", "precreate"] {
            fs::write(&path, b"legacy bytes").unwrap();
            // std shares read, write and delete, so the delete succeeds and
            // leaves the name pending until this handle closes.
            let holder = fs::File::open(&path).unwrap();
            fs::remove_file(&path).unwrap();
            let release = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(150));
                drop(holder);
            });
            match step {
                "detect" => assert_eq!(detect_legacy(&path).unwrap(), FileState::Missing),
                _ => {
                    precreate(&path).unwrap();
                    assert_eq!(detect_legacy(&path).unwrap(), FileState::Empty);
                    fs::remove_file(&path).unwrap();
                }
            }
            release.join().unwrap();
        }
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
                    // Every wait ended in the lock.
                    assert_eq!(BUSY_SURFACED.with(std::cell::Cell::get), 0);
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
    fn a_busy_error_that_outlasts_the_retries_is_counted() {
        let dir = TempDir::new().unwrap();
        let mut holder = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let mut waiter = open(dir.path(), Mode::IfExists).unwrap().unwrap();
        waiter.busy_timeout(Duration::from_millis(5)).unwrap();
        let before = BUSY_SURFACED.with(std::cell::Cell::get);
        // Waits that end in a lock are not counted.
        write_txn(&mut holder, |_| Ok(())).unwrap();
        write_txn(&mut waiter, |_| Ok(())).unwrap();
        assert_eq!(BUSY_SURFACED.with(std::cell::Cell::get), before);
        let tx = Transaction::new_unchecked(&holder, TransactionBehavior::Immediate).unwrap();
        write_txn(&mut waiter, |_| Ok(())).unwrap_err();
        assert_eq!(BUSY_SURFACED.with(std::cell::Cell::get), before + 1);
        // An already mapped error is not counted twice.
        let mapped = map_busy(write_txn(&mut waiter, |_| Ok(())).unwrap_err());
        assert!(matches!(db_error(&mapped), DbError::Busy));
        assert_eq!(BUSY_SURFACED.with(std::cell::Cell::get), before + 2);
        tx.rollback().unwrap();
    }

    #[test]
    fn a_wal_switch_waits_out_another_connections_read_lock() {
        let dir = TempDir::new().unwrap();
        let conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        // A file left in rollback mode (a host that fell back to DELETE).
        conn.pragma_update(None, "journal_mode", "DELETE").unwrap();
        drop(conn);
        let reader = Connection::open(db_path(dir.path())).unwrap();
        reader.execute("BEGIN", []).unwrap();
        reader
            .query_row("SELECT count(*) FROM brauser_meta", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        let hold = Duration::from_millis(300);
        let release = std::thread::spawn(move || {
            std::thread::sleep(hold);
            reader.execute("COMMIT", []).unwrap();
        });
        // The switch to WAL needs the file to itself; it waits for the
        // reader to finish instead of falling back to DELETE.
        let started = std::time::Instant::now();
        let conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let journal: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert_eq!(journal, "wal");
        assert!(started.elapsed() >= hold);
        release.join().unwrap();
        assert_eq!(BUSY_SURFACED.with(std::cell::Cell::get), 0);
    }

    #[test]
    fn opening_a_current_store_takes_no_write_lock() {
        let dir = TempDir::new().unwrap();
        let holder = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let _short = BusyTimeout::set(Duration::from_millis(50));
        let tx = Transaction::new_unchecked(&holder, TransactionBehavior::Immediate).unwrap();
        // The schema already exists, so checking it needs no BEGIN
        // IMMEDIATE behind another connection's write.
        let mut conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let rows: i64 = read_txn(&mut conn, |tx| {
            Ok(tx.query_row("SELECT count(*) FROM brauser_meta", [], |row| row.get(0))?)
        })
        .unwrap();
        assert_eq!(rows, 0);
        tx.rollback().unwrap();
    }

    #[test]
    fn a_held_write_lock_surfaces_busy_within_about_one_busy_timeout() {
        let dir = TempDir::new().unwrap();
        let holder = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let timeout = Duration::from_millis(1_000);
        let _short = BusyTimeout::set(timeout);
        let mut waiter = open(dir.path(), Mode::IfExists).unwrap().unwrap();
        let tx = Transaction::new_unchecked(&holder, TransactionBehavior::Immediate).unwrap();
        let started = std::time::Instant::now();
        let waits_before = BEGIN_WAITS.with(std::cell::Cell::get);
        let error = write_txn(&mut waiter, |_| Ok(())).unwrap_err();
        let waited = started.elapsed();
        assert!(matches!(db_error(&error), DbError::Busy), "{error:#}");
        // The held lock is waited out exactly once.
        assert_eq!(BEGIN_WAITS.with(std::cell::Cell::get) - waits_before, 1);
        // Retries never multiply the busy timeout: a read's catch-up must
        // give up well inside the extension's 30 s request timeout.
        assert!(waited >= timeout, "{waited:?}");
        // The count above is the exact check; this wall-clock bound only
        // catches a multiplied timeout, with room for a slow CI runner.
        assert!(waited < timeout * 4, "{waited:?}");
        tx.rollback().unwrap();
    }

    #[test]
    fn a_test_can_shorten_the_busy_timeout_of_its_own_connections() {
        let dir = TempDir::new().unwrap();
        let holder = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let _short = BusyTimeout::set(Duration::from_millis(20));
        let mut waiter = open(dir.path(), Mode::IfExists).unwrap().unwrap();
        assert_eq!(pragma_i64(&waiter, "busy_timeout"), 20);
        let tx = Transaction::new_unchecked(&holder, TransactionBehavior::Immediate).unwrap();
        let started = std::time::Instant::now();
        write_txn(&mut waiter, |_| Ok(())).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(5));
        tx.rollback().unwrap();
        drop(_short);
        let again = open(dir.path(), Mode::IfExists).unwrap().unwrap();
        assert_eq!(pragma_i64(&again, "busy_timeout"), 10_000);
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

    /// A format 8 snapshot as its hand-written writer left it: 64 KiB
    /// pages, application_id 'BRWR', a rollback journal.
    fn write_v8_snapshot(path: &Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "PRAGMA page_size = 65536;
             PRAGMA application_id = 1112692562;
             CREATE TABLE artifacts(artifact_id TEXT, legacy TEXT);
             INSERT INTO artifacts VALUES ('a', 'old');",
        )
        .unwrap();
        drop(conn);
        let header = fs::read(path).unwrap();
        assert_eq!((header[18], header[19]), (1, 1));
        assert_eq!(&header[68..72], b"BRWR");
    }

    #[test]
    fn a_stale_wal_beside_a_snapshot_renamed_over_ours_is_never_replayed() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        // A format 9 host killed with uncheckpointed frames, page 1 among
        // them (a schema change rewrites it).
        let conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        conn.execute_batch("CREATE TABLE stale_frames(x); INSERT INTO stale_frames VALUES (1);")
            .unwrap();
        let wal = fs::read(sidecar(&path, "-wal")).unwrap();
        let shm = fs::read(sidecar(&path, "-shm")).unwrap();
        assert!(!wal.is_empty());
        drop(conn);
        // A pre-format-9 host then renames its snapshot over index.db,
        // leaving the old sidecars beside it.
        for suffix in SIDECARS {
            let _ = fs::remove_file(sidecar(&path, suffix));
        }
        fs::remove_file(&path).unwrap();
        write_v8_snapshot(&path);
        fs::write(sidecar(&path, "-wal"), &wal).unwrap();
        fs::write(sidecar(&path, "-shm"), &shm).unwrap();
        assert_eq!(detect_legacy(&path).unwrap(), FileState::Legacy);

        let conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        let check: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(check, "ok");
        assert_eq!(pragma_i64(&conn, "application_id"), 0x6272_7772);
        assert_eq!(pragma_i64(&conn, "page_size"), 4096);
        let stale: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name = 'stale_frames'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0);
        assert!(!columns(&conn, "artifacts").contains(&"legacy".to_string()));
    }

    #[test]
    fn a_format_8_snapshot_switched_to_wal_is_still_replaced() {
        let dir = TempDir::new().unwrap();
        let path = db_path(dir.path());
        write_v8_snapshot(&path);
        // An external SQL consumer turned on WAL.
        let consumer = Connection::open(&path).unwrap();
        let mode: String = consumer
            .pragma_update_and_check(None, "journal_mode", "WAL", |row| row.get(0))
            .unwrap();
        assert_eq!(mode, "wal");
        drop(consumer);
        let header = fs::read(&path).unwrap();
        assert_eq!((header[18], header[19]), (2, 2));
        assert_eq!(detect_legacy(&path).unwrap(), FileState::Legacy);

        let error = open(dir.path(), Mode::IfExists).unwrap_err();
        assert!(matches!(db_error(&error), DbError::Legacy), "{error:#}");
        let conn = open(dir.path(), Mode::ReadWrite).unwrap().unwrap();
        assert_eq!(pragma_i64(&conn, "application_id"), 0x6272_7772);
        assert_eq!(pragma_i64(&conn, "user_version"), 9);
        assert!(!columns(&conn, "artifacts").contains(&"legacy".to_string()));
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
