//! The staged in-place rebuild and the adapter change (plan T10).
//!
//! One mechanism serves `brauser reindex`, a shrunk or removed log, a lost
//! or unusable JSON cache and (plan T16) migration. No database file is
//! renamed or deleted on this path, so it is safe on Windows:
//!
//! 1. [`begin`] sets `brauser_meta.rebuild = 'staging'` and a fresh
//!    `rebuild_id`, then creates and clears the `brauser_stage_*` tables.
//!    The previous winners stay readable in `brauser_events`.
//! 2. Each [`pass`] reads at most one catch-up pass's bound of logs into the
//!    stage tables, through the same watermark machinery as the live
//!    catch-up (`catchup::Target::Stage`), and commits. Progress therefore
//!    resumes across calls and processes; a staged log that shrinks or
//!    vanishes restarts the stage.
//! 3. [`finalize`], in one transaction: a body whose SHA-256 matches the
//!    live `brauser_events` row wins (the previous cache winner, by join),
//!    otherwise the earliest `order_key`; `brauser_events`, `brauser_logs`
//!    and `brauser_gaps` are replaced; every group is re-derived one at a
//!    time, so memory stays bounded per group; vanished container files
//!    (`brauser_group_state.file_path` and owned roll-up files on disk) are
//!    adopted so regeneration removes them; the stage tables are dropped.
//!
//! [`adapter_change`] runs once per adapter document change: it recomputes
//! every event's container, re-derives every group and marks dirty only the
//! artifacts whose rendered file changes.

use rusqlite::{OptionalExtension, Transaction, params};

use super::*;

/// A staged body per event ID is kept for at most this many distinct bodies.
const MAX_STAGED_BODIES: i64 = 8;

/// The stage tables, created by [`begin`] and dropped by [`finalize`].
const STAGE_TABLES: [&str; 4] = [
    "brauser_stage_events",
    "brauser_stage_logs",
    "brauser_stage_gaps",
    "brauser_stage_winners",
];

const STAGE_SCHEMA: [&str; 4] = [
    "CREATE TABLE IF NOT EXISTS brauser_stage_events(
       event_id TEXT NOT NULL, artifact_id TEXT NOT NULL,
       kind TEXT NOT NULL CHECK(kind IN ('activity','why','legacy','summary')),
       order_key BLOB NOT NULL, local_date TEXT NOT NULL,
       log_path TEXT, log_line INTEGER, sequence INTEGER,
       container_id TEXT,
       has_refs INTEGER NOT NULL DEFAULT 0,
       body TEXT NOT NULL,
       body_sha256 BLOB NOT NULL,
       PRIMARY KEY(event_id, body_sha256)) WITHOUT ROWID",
    "CREATE TABLE IF NOT EXISTS brauser_stage_logs(
       path TEXT NOT NULL, purpose TEXT NOT NULL CHECK(purpose IN ('events','gaps')),
       bytes INTEGER NOT NULL, modified_ns INTEGER NOT NULL,
       parsed_bytes INTEGER NOT NULL, parsed_lines INTEGER NOT NULL, tail_sha256 BLOB NOT NULL,
       PRIMARY KEY(path, purpose)) WITHOUT ROWID",
    "CREATE TABLE IF NOT EXISTS brauser_stage_gaps(
       gap_id TEXT PRIMARY KEY, marker TEXT NOT NULL, log_path TEXT, log_line INTEGER,
       start_date TEXT, end_date TEXT) WITHOUT ROWID",
    "CREATE TABLE IF NOT EXISTS brauser_stage_winners(
       event_id TEXT PRIMARY KEY, body_sha256 BLOB NOT NULL) WITHOUT ROWID",
];

/// The derived tables a finalize or an adapter change clears before it
/// re-derives every group. `brauser_group_state` keeps its regeneration
/// columns, so it is replaced group by group instead.
const DERIVED_TABLES: [&str; 5] = [
    "artifacts",
    "visits",
    "edges",
    "url_provenance",
    "brauser_artifact_days",
];

/// What a finished rebuild marks dirty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Finish {
    /// `brauser reindex`: every artifact, so regeneration rewrites every
    /// file (intentionally replacing user edits to owned files).
    All,
    /// Any other rebuild: only artifacts whose rendered file differs from
    /// what it was believed to hold, or that gained a logged body the store
    /// did not have, so unchanged files keep user edits.
    Changed,
}

/// Where the rebuilt rows came from, recorded in `brauser_meta`.
pub(super) struct Location<'a> {
    pub(super) storage: &'a StorageConfig,
    pub(super) root_identity: &'a str,
    pub(super) fingerprint: &'a str,
}

/// The SHA-256 of an artifact's rendered file, as `render_sha256` stores it.
pub(super) fn render_digest(artifacts_dir: &str, row: &ArtifactRow) -> Vec<u8> {
    store::sha256(render_artifact(artifacts_dir, row).as_bytes())
}

fn stage_exists(tx: &Transaction<'_>) -> Result<bool> {
    let count: i64 = tx.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name IN
           ('brauser_stage_events', 'brauser_stage_logs', 'brauser_stage_gaps',
            'brauser_stage_winners')",
        [],
        |row| row.get(0),
    )?;
    Ok(count == STAGE_TABLES.len() as i64)
}

/// Start a staged rebuild from nothing: set the flag and a fresh
/// `rebuild_id`, and create (or empty) the stage tables.
pub(super) fn begin(tx: &Transaction<'_>) -> Result<()> {
    catchup::request_rebuild(tx)?;
    store::set_meta(tx, "rebuild_id", &Uuid::new_v4().to_string())?;
    for sql in STAGE_SCHEMA {
        tx.execute(sql, [])?;
    }
    for table in STAGE_TABLES {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    Ok(())
}

/// Resume the staged rebuild in progress, or begin one: when none is
/// pending, or one was only requested (a shrunk log) with no stage yet.
fn ensure_stage(tx: &Transaction<'_>) -> Result<()> {
    if catchup::rebuilding(tx)? && stage_exists(tx)? {
        return Ok(());
    }
    begin(tx)
}

/// One bounded pass of logs into the stage tables. Returns whether every
/// log is staged, so [`finalize`] may run.
pub(super) fn pass(
    tx: &Transaction<'_>,
    storage: &StorageConfig,
    root_identity: &str,
) -> Result<bool> {
    ensure_stage(tx)?;
    let mut sink = |tx: &Transaction<'_>, event: CachedEvent, position: (PathBuf, u64)| {
        stage_event(tx, &event, &position)
    };
    for attempt in 0..2 {
        match catchup::catch_up_into(
            tx,
            catchup::Target::Stage,
            storage,
            root_identity,
            &mut sink,
        )? {
            catchup::CatchUp::Caught { complete, .. } => return Ok(complete),
            // A log staged earlier shrank or vanished: what was staged from
            // it may be gone, so the stage starts again.
            catchup::CatchUp::Rebuilding if attempt == 0 => begin(tx)?,
            catchup::CatchUp::Rebuilding => break,
        }
    }
    bail!("artifact history is unavailable: a log changed during the rebuild")
}

/// Stage one logged body at its position. The same body seen again keeps
/// its earliest position; a new body for an ID that already has
/// `MAX_STAGED_BODIES` is dropped (it could never win).
fn stage_event(tx: &Transaction<'_>, event: &CachedEvent, position: &(PathBuf, u64)) -> Result<()> {
    let (id, artifact_id) = event_identity(event);
    let body = store::body_of(event)?;
    let sha = store::sha256(body.as_bytes());
    let (known, present): (i64, i64) = tx
        .prepare_cached(
            "SELECT count(*), coalesce(sum(body_sha256 = ?2), 0)
             FROM brauser_stage_events WHERE event_id = ?1",
        )?
        .query_row(params![id, sha], |row| Ok((row.get(0)?, row.get(1)?)))?;
    if present == 0 && known >= MAX_STAGED_BODIES {
        return Ok(());
    }
    let (path, line) = position;
    let order_key = ord::order_key(event_time(event), Some((path.as_path(), *line)), None, &id);
    tx.prepare_cached(
        "INSERT INTO brauser_stage_events(event_id, artifact_id, kind, order_key, local_date,
           log_path, log_line, body, body_sha256)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
         ON CONFLICT(event_id, body_sha256) DO UPDATE SET order_key = excluded.order_key,
           log_path = excluded.log_path, log_line = excluded.log_line
         WHERE excluded.order_key < brauser_stage_events.order_key",
    )?
    .execute(params![
        id,
        artifact_id,
        store::event_kind(event),
        order_key,
        event_time(event).date().to_string(),
        catchup::portable_path(path),
        store::sql_int(*line),
        body,
        sha,
    ])?;
    Ok(())
}

/// One `artifacts` row as stored, to tell whether a re-derive changed it.
fn stored_row(tx: &Transaction<'_>, id: &str) -> Result<Option<Vec<rusqlite::types::Value>>> {
    Ok(tx
        .prepare_cached(
            "SELECT artifact_type, site, title, url, first_seen, last_seen, focused_secs,
               background_secs, opens, days_opened, last_mode, why
             FROM artifacts WHERE artifact_id = ?1",
        )?
        .query_row([id], |row| {
            (0..12)
                .map(|column| row.get::<_, rusqlite::types::Value>(column))
                .collect()
        })
        .optional()?)
}

fn ids(tx: &Transaction<'_>, sql: &str) -> Result<BTreeSet<String>> {
    Ok(tx
        .prepare(sql)?
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Every group that has rows or a state row now.
fn existing_groups(tx: &Transaction<'_>) -> Result<BTreeSet<String>> {
    ids(
        tx,
        "SELECT artifact_id FROM artifacts UNION SELECT artifact_id FROM brauser_group_state",
    )
}

/// Every group the stored events derive: each artifact and each container.
fn event_groups(tx: &Transaction<'_>) -> Result<BTreeSet<String>> {
    ids(
        tx,
        "SELECT artifact_id FROM brauser_events
         UNION SELECT container_id FROM brauser_events WHERE container_id IS NOT NULL",
    )
}

fn clear_derived(tx: &Transaction<'_>) -> Result<()> {
    for table in DERIVED_TABLES {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    Ok(())
}

/// The render digest a group's file is believed to hold: the one
/// regeneration stored, else the render of its current rows.
fn believed_render(
    tx: &Transaction<'_>,
    adapters: Option<&Document>,
    artifacts_dir: &str,
    group: &str,
) -> Result<Option<Vec<u8>>> {
    let stored: Option<Option<Vec<u8>>> = tx
        .prepare_cached("SELECT render_sha256 FROM brauser_group_state WHERE artifact_id = ?1")?
        .query_row([group], |row| row.get(0))
        .optional()?;
    if let Some(Some(sha)) = stored {
        return Ok(Some(sha));
    }
    let events = store::group_events(tx, group)?;
    Ok(store::fold_events(adapters, &events)
        .artifacts
        .get(group)
        .map(|row| render_digest(artifacts_dir, row)))
}

fn set_location(tx: &Transaction<'_>, location: &Location<'_>) -> Result<()> {
    let storage = location.storage;
    store::set_meta(tx, "root", &storage.root)?;
    store::set_meta(tx, "root_identity", location.root_identity)?;
    store::set_meta(tx, "log_dir", &storage.log_dir)?;
    store::set_meta(tx, "artifacts_dir", &storage.artifacts_dir)?;
    store::set_meta(tx, "adapters", location.fingerprint)
}

/// Finish a staged rebuild whose every log is staged (see the module
/// documentation). `known_files` are owned roll-up files found on disk, by
/// artifact ID, relative to the home folder.
pub(super) fn finalize(
    tx: &Transaction<'_>,
    adapters: Option<&Document>,
    location: &Location<'_>,
    finish: Finish,
    known_files: &BTreeMap<String, PathBuf>,
) -> Result<()> {
    let artifacts_dir = &location.storage.artifacts_dir;
    // What each existing file is believed to hold, before the events change.
    let mut believed = BTreeMap::new();
    if finish == Finish::Changed {
        for group in existing_groups(tx)? {
            let digest = believed_render(tx, adapters, artifacts_dir, &group)?;
            believed.insert(group, digest);
        }
    }
    // Winners: the previous cache's body when it is still logged, else the
    // body at the earliest log position, never the earliest claimed
    // instant (a later marker can claim any time). Every staged key has a
    // position, which follows the 16-byte instant (`ord::order_key`).
    tx.execute(
        "INSERT OR IGNORE INTO brauser_stage_winners(event_id, body_sha256)
         SELECT s.event_id, s.body_sha256 FROM brauser_stage_events s
           JOIN brauser_events e ON e.event_id = s.event_id AND e.body_sha256 = s.body_sha256",
        [],
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO brauser_stage_winners(event_id, body_sha256)
         SELECT s.event_id, s.body_sha256 FROM brauser_stage_events s
         WHERE substr(s.order_key, 17) = (SELECT min(substr(t.order_key, 17))
                                          FROM brauser_stage_events t
                                          WHERE t.event_id = s.event_id)",
        [],
    )?;
    // Artifacts that gain a body the previous store did not have are marked
    // too, as a cache rebuilt from nothing marks every artifact it ingests.
    let mut dirty = BTreeSet::new();
    if finish == Finish::Changed {
        dirty.extend(ids(
            tx,
            "SELECT DISTINCT s.artifact_id FROM brauser_stage_events s
               JOIN brauser_stage_winners w
                 ON w.event_id = s.event_id AND w.body_sha256 = s.body_sha256
             WHERE NOT EXISTS (SELECT 1 FROM brauser_events e
                               WHERE e.event_id = s.event_id
                                 AND e.body_sha256 = s.body_sha256)",
        )?);
    }
    tx.execute("DELETE FROM brauser_events", [])?;
    let mut containers = Containers::new(adapters);
    {
        let mut winners = tx.prepare(
            "SELECT s.event_id, s.artifact_id, s.kind, s.order_key, s.local_date, s.log_path,
               s.log_line, s.body, s.body_sha256
             FROM brauser_stage_events s
               JOIN brauser_stage_winners w
                 ON w.event_id = s.event_id AND w.body_sha256 = s.body_sha256",
        )?;
        let mut rows = winners.query([])?;
        while let Some(row) = rows.next()? {
            let body: String = row.get(7)?;
            let event = store::parse_body(&body)?;
            tx.prepare_cached(
                "INSERT INTO brauser_events(event_id, artifact_id, kind, order_key, local_date,
                   log_path, log_line, sequence, container_id, has_refs, body, body_sha256)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, NULL, ?8, ?9, ?10, ?11)",
            )?
            .execute(params![
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                store::container_of(&mut containers, &event),
                store::has_refs(&event),
                body,
                row.get::<_, Vec<u8>>(8)?,
            ])?;
        }
    }
    tx.execute("DELETE FROM brauser_logs WHERE purpose = 'events'", [])?;
    tx.execute(
        "INSERT INTO brauser_logs SELECT * FROM brauser_stage_logs WHERE purpose = 'events'",
        [],
    )?;
    tx.execute("DELETE FROM brauser_gaps", [])?;
    tx.execute(
        "INSERT INTO brauser_gaps SELECT * FROM brauser_stage_gaps",
        [],
    )?;
    // Re-derive every group, one at a time.
    clear_derived(tx)?;
    let mut groups = event_groups(tx)?;
    groups.extend(ids(tx, "SELECT artifact_id FROM brauser_group_state")?);
    for group in &groups {
        let row = store::rederive_group(tx, adapters, group)?;
        let changed = match finish {
            Finish::All => row.is_some(),
            Finish::Changed => {
                let digest = row.as_ref().map(|row| render_digest(artifacts_dir, row));
                believed.get(group).cloned().flatten() != digest
            }
        };
        if changed {
            dirty.insert(group.clone());
        }
    }
    // A group with no row that still names its file (kept by `write_group`)
    // is dirty, so regeneration removes that file. So is an owned roll-up
    // file on disk whose container no longer derives a record.
    dirty.extend(ids(
        tx,
        "SELECT artifact_id FROM brauser_group_state
         WHERE file_path IS NOT NULL
           AND artifact_id NOT IN (SELECT artifact_id FROM artifacts)",
    )?);
    for (id, path) in known_files {
        let Some(path) = catchup::portable_path(path) else {
            continue;
        };
        if stored_row(tx, id)?.is_some() {
            continue;
        }
        tx.prepare_cached(
            "INSERT INTO brauser_group_state(artifact_id, latest_key, latest_at, why_at,
               rolled_up, file_path)
             VALUES (?1, x'', '', NULL, 1, ?2)
             ON CONFLICT(artifact_id) DO UPDATE SET
               file_path = coalesce(brauser_group_state.file_path, excluded.file_path)",
        )?
        .execute(params![id, path])?;
        dirty.insert(id.clone());
    }
    store::mark_dirty(tx, &dirty)?;
    for table in STAGE_TABLES {
        tx.execute(&format!("DROP TABLE IF EXISTS {table}"), [])?;
    }
    store::set_meta(tx, "rebuild", "none")?;
    set_location(tx, location)
}

/// Re-derive the store for a changed adapter document, once: recompute
/// every event's container, re-derive every group (mentions resolve in the
/// fold) and mark dirty only the artifacts whose rendered file changes.
/// With a stored `render_sha256` that is exact; with none (a file not yet
/// regenerated by a format 9 host) a file counts as changed when its row
/// changed, it is or was a roll-up, or its render depends on the adapters
/// at all. Returns every dirty artifact afterwards. A store already on
/// `fingerprint` is left alone.
pub(super) fn adapter_change(
    tx: &Transaction<'_>,
    adapters: Option<&Document>,
    fingerprint: &str,
    artifacts_dir: &str,
) -> Result<BTreeSet<String>> {
    let marked = || ids(tx, "SELECT artifact_id FROM dirty_artifacts");
    if store::meta(tx, "adapters")?.as_deref() == Some(fingerprint) {
        return marked();
    }
    struct Before {
        row: Option<Vec<rusqlite::types::Value>>,
        rolled_up: bool,
        render: Option<Vec<u8>>,
    }
    let mut before = BTreeMap::new();
    for group in existing_groups(tx)? {
        let (rolled_up, render): (bool, Option<Vec<u8>>) = tx
            .prepare_cached(
                "SELECT rolled_up, render_sha256 FROM brauser_group_state WHERE artifact_id = ?1",
            )?
            .query_row([&group], |row| Ok((row.get(0)?, row.get(1)?)))
            .optional()?
            .unwrap_or((false, None));
        let row = stored_row(tx, &group)?;
        before.insert(
            group,
            Before {
                row,
                rolled_up,
                render,
            },
        );
    }
    // 1. Containers follow the document.
    let mut containers = Containers::new(adapters);
    let mut moved = Vec::new();
    {
        let mut statement = tx.prepare(
            "SELECT event_id, container_id, body FROM brauser_events
             WHERE kind IN ('activity', 'legacy')",
        )?;
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            let old: Option<String> = row.get(1)?;
            let event = store::parse_body(&row.get::<_, String>(2)?)?;
            let new = store::container_of(&mut containers, &event);
            if new != old {
                moved.push((row.get::<_, String>(0)?, new));
            }
        }
    }
    for (id, container) in moved {
        tx.prepare_cached("UPDATE brauser_events SET container_id = ?2 WHERE event_id = ?1")?
            .execute(params![id, container])?;
    }
    // 2 and 3. Every group, including those that lose their rows.
    clear_derived(tx)?;
    let mut groups = event_groups(tx)?;
    groups.extend(before.keys().cloned());
    let mut dirty = BTreeSet::new();
    for group in &groups {
        let row = store::rederive_group(tx, adapters, group)?;
        let digest = row.as_ref().map(|row| render_digest(artifacts_dir, row));
        let changed = match before.get(group) {
            Some(Before {
                render: Some(sha), ..
            }) => digest.as_ref() != Some(sha),
            Some(Before {
                row: old,
                rolled_up,
                ..
            }) => {
                *rolled_up
                    || row.as_ref().is_some_and(|row| row.rolled_up)
                    || *old != stored_row(tx, group)?
                    || digest != plain_render(tx, artifacts_dir, group)?
            }
            None => row.is_some(),
        };
        if changed {
            dirty.insert(group.clone());
        }
    }
    store::set_meta(tx, "adapters", fingerprint)?;
    store::mark_dirty(tx, &dirty)?;
    marked()
}

/// A group's render from its own events with no adapter document: what its
/// file holds when nothing about it depends on the adapters.
fn plain_render(tx: &Transaction<'_>, artifacts_dir: &str, group: &str) -> Result<Option<Vec<u8>>> {
    let events = store::group_events(tx, group)?
        .into_iter()
        .filter(|stored| stored.artifact_id == group)
        .collect::<Vec<_>>();
    Ok(store::fold_events(None, &events)
        .artifacts
        .get(group)
        .map(|row| render_digest(artifacts_dir, row)))
}

impl ArtifactIndex {
    fn location<'a>(&'a self, fingerprint: &'a str) -> Location<'a> {
        Location {
            storage: &self.storage,
            root_identity: &self.root_identity,
            fingerprint,
        }
    }

    /// One bounded staged-rebuild pass, beginning a rebuild when none is in
    /// progress, and the finalize once every log is staged. Returns whether
    /// the rebuild finished. The roll-up file scan runs between the two
    /// transactions, never inside one.
    pub(super) fn rebuild_pass(&self, finish: Finish) -> Result<bool> {
        let adapters = self.adapters.as_deref();
        let fingerprint = self.adapters_fingerprint();
        // Passes touch only the stage tables, so they skip the derived-rows
        // oracle, which the finalize then runs (the live rows may still be
        // derived under an earlier adapter document until then).
        let mut conn = store::open(&self.cache_dir)?;
        let staged = db::write_txn(&mut conn, |tx| pass(tx, &self.storage, &self.root_identity))?;
        if !staged {
            return Ok(false);
        }
        let known = self.owned_rollup_files();
        db::write_txn(&mut conn, |tx| {
            // Another host finished it in between.
            if !catchup::rebuilding(tx)? {
                return Ok(true);
            }
            // Stage anything logged since; usually a stat-only check.
            if !pass(tx, &self.storage, &self.root_identity)? {
                return Ok(false);
            }
            finalize(tx, adapters, &self.location(&fingerprint), finish, &known)?;
            #[cfg(test)]
            store::oracle::check(tx, adapters)?;
            Ok(true)
        })
    }

    /// Whether `index.db` has a staged rebuild in progress. Never creates
    /// the file; an unusable one counts as no rebuild.
    pub(super) fn store_rebuilding(&self) -> bool {
        let Ok(Some(mut conn)) = db::open(&self.cache_dir, db::Mode::IfExists) else {
            return false;
        };
        db::read_txn(&mut conn, catchup::rebuilding).unwrap_or(false)
    }

    /// Run the adapter change on an existing format 9 store. `None` when
    /// there is none, so the caller decides without one.
    pub(super) fn store_adapter_change(&self) -> Result<Option<BTreeSet<String>>> {
        if !matches!(db::open(&self.cache_dir, db::Mode::IfExists), Ok(Some(_))) {
            return Ok(None);
        }
        let adapters = self.adapters.as_deref();
        let fingerprint = self.adapters_fingerprint();
        let mut conn = store::open(&self.cache_dir)?;
        store::write(&mut conn, adapters, |tx| {
            adapter_change(tx, adapters, &fingerprint, &self.storage.artifacts_dir)
        })
        .map(Some)
    }

    /// The JSON cache as the store holds it, which a finished staged rebuild
    /// hands back to the JSON half until plan T15 removes it.
    pub(super) fn store_state(&self) -> Result<IndexState> {
        let mut conn = store::open(&self.cache_dir)?;
        db::read_txn(&mut conn, |tx| {
            let mut state = self.empty_state();
            let mut statement = tx.prepare(
                "SELECT event_id, body, log_path, log_line, sequence FROM brauser_events",
            )?;
            let mut rows = statement.query([])?;
            while let Some(row) = rows.next()? {
                let id: String = row.get(0)?;
                let event = store::parse_body(&row.get::<_, String>(1)?)?;
                if let (Some(path), Some(line)) = (
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ) {
                    let line = u64::try_from(line).context("negative log line in index.db")?;
                    state
                        .positions
                        .insert(id.clone(), (path.split('/').collect(), line));
                }
                if let Some(sequence) = row.get::<_, Option<i64>>(4)? {
                    let sequence =
                        u64::try_from(sequence).context("negative sequence in index.db")?;
                    state.sequence.insert(id.clone(), sequence);
                }
                state.events.insert(id, event);
            }
            state.next_sequence = store::meta_u64(tx, "next_sequence")?;
            state.logs = catchup::checkpoints(tx)?;
            state.gap_logs = state.logs.clone();
            state.gaps = catchup::gaps(tx)?;
            if let Some(adapters) = store::meta(tx, "adapters")? {
                state.adapters = adapters;
            }
            state.dirty = ids(tx, "SELECT artifact_id FROM dirty_artifacts")?;
            state.containers = ids(
                tx,
                "SELECT artifact_id FROM brauser_group_state
                 WHERE rolled_up = 1 AND artifact_id IN (SELECT artifact_id FROM artifacts)",
            )?;
            let mut files = tx.prepare(
                "SELECT artifact_id, file_path FROM brauser_group_state
                 WHERE file_path IS NOT NULL",
            )?;
            state.container_files = files
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?.split('/').collect::<PathBuf>(),
                    ))
                })?
                .collect::<rusqlite::Result<_>>()?;
            Ok(state)
        })
    }

    /// Truncate the write-ahead log after an O(history) rewrite (reindex).
    /// Best effort: a failure only leaves the WAL at its size limit.
    pub(super) fn checkpoint_wal(&self) {
        let Ok(Some(conn)) = db::open(&self.cache_dir, db::Mode::IfExists) else {
            return;
        };
        if let Err(error) = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(())) {
            tracing::warn!(%error, "could not checkpoint index.db's write-ahead log");
        }
    }
}
