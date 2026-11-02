//! Migration of a format 8 cache (plan T16), and, until plan T15 removes
//! them, the in-place migrations of older `index-state.json` versions.
//!
//! A format 8 host kept its cache in `index-state.json` (plus a saved
//! partial pass in `index-catch-up.json`) and published a hand-written
//! `index.db` with another `application_id`. The first write-capable open
//! by a format 9 host that has something to index migrates it, once:
//!
//! 1. `db::open` clears the legacy `index.db` (sidecars first) and creates
//!    the format 9 schema.
//! 2. One transaction imports a usable v8 JSON cache: its events become the
//!    "previous winners" in `brauser_events`, its dirty set becomes
//!    `dirty_artifacts`, its containers and container files become
//!    `brauser_group_state.rolled_up` and `file_path`, and every other
//!    artifact's `render_sha256` is the render the v8 state derives. A JSON
//!    cache that is absent, of another version, oversized or invalid imports
//!    nothing. The same transaction begins the staged rebuild and records
//!    `brauser_meta.legacy_import` as `imported` or `fallback`.
//! 3. The staged rebuild runs from the logs, one bounded pass per call
//!    (reads are unavailable meanwhile). After an import it marks dirty only
//!    renders that changed; after the fallback, only artifacts whose file is
//!    missing (`rebuild::Finish::Missing`), so user edits are never
//!    overwritten.
//! 4. Once the rebuild has committed, the legacy JSON files are removed and
//!    `legacy_import` becomes `done`. A crash before that repeats only what
//!    has not committed.
//!
//! A pre-format-9 host that later renames its snapshot over `index.db`
//! leaves a foreign file again, so the next open migrates again and the
//! logs recover anything written in between.

use rusqlite::{Transaction, params};

use super::*;

/// The `brauser_meta` key recording the migration's progress.
const LEGACY_KEY: &str = "legacy_import";
/// The JSON cache format a format 8 host wrote.
const LEGACY_VERSION: u8 = 8;
/// The legacy JSON files the migration removes once it has committed.
const LEGACY_FILES: [&str; 2] = ["index-state.json", "index-catch-up.json"];

/// A format 8 `index-state.json`, read only for the fields the import
/// uses. It is never written.
#[derive(Deserialize)]
struct LegacyState {
    version: u8,
    root: String,
    root_identity: String,
    log_dir: String,
    artifacts_dir: String,
    events: BTreeMap<String, CachedEvent>,
    #[serde(default)]
    positions: BTreeMap<String, (PathBuf, u64)>,
    #[serde(default)]
    sequence: BTreeMap<String, u64>,
    #[serde(default)]
    next_sequence: u64,
    #[serde(default)]
    adapters: String,
    #[serde(default)]
    dirty: BTreeSet<String>,
    #[serde(default)]
    containers: BTreeSet<String>,
    #[serde(default)]
    container_files: BTreeMap<String, PathBuf>,
}

impl LegacyState {
    fn has_content(&self) -> bool {
        !(self.events.is_empty() && self.dirty.is_empty() && self.container_files.is_empty())
    }
}

/// What the migration still has to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Legacy {
    /// Nothing: no legacy cache, or it is already migrated.
    None,
    /// The import (or fallback) committed; the staged rebuild finishes it
    /// with this dirty rule.
    Rebuild(rebuild::Finish),
    /// The rebuild committed; only the cleanup is left.
    Cleanup,
}

fn progress(value: Option<&str>, rebuilding: bool) -> Legacy {
    match value {
        None | Some("done") => Legacy::None,
        Some(_) if !rebuilding => Legacy::Cleanup,
        Some("fallback") => Legacy::Rebuild(rebuild::Finish::Missing),
        Some(_) => Legacy::Rebuild(rebuild::Finish::Changed),
    }
}

/// Whether the store holds nothing yet: no event and no log watermark.
fn store_is_empty(tx: &Transaction<'_>) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT NOT EXISTS (SELECT 1 FROM brauser_events)
           AND NOT EXISTS (SELECT 1 FROM brauser_logs)",
        [],
        |row| row.get(0),
    )?)
}

/// Import a usable v8 cache into an empty store (migration step 2).
fn import(
    tx: &Transaction<'_>,
    adapters: Option<&Document>,
    fingerprint: &str,
    artifacts_dir: &str,
    state: &LegacyState,
) -> Result<()> {
    let mut containers = Containers::new(adapters);
    let mut stored = Vec::with_capacity(state.events.len());
    for (id, event) in &state.events {
        let (_, artifact_id) = event_identity(event);
        let position = state.positions.get(id);
        let sequence = state.sequence.get(id).copied();
        let order_key = ord::order_key(
            event_time(event),
            position.map(|(path, line)| (path.as_path(), *line)),
            sequence,
            id,
        );
        let container_id = store::container_of(&mut containers, event);
        let body = store::body_of(event)?;
        tx.prepare_cached(
            "INSERT INTO brauser_events(event_id, artifact_id, kind, order_key, local_date,
               log_path, log_line, sequence, container_id, has_refs, body, body_sha256)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        )?
        .execute(params![
            id,
            artifact_id,
            store::event_kind(event),
            order_key,
            event_time(event).date().to_string(),
            position.and_then(|(path, _)| catchup::portable_path(path)),
            position.map(|(_, line)| store::sql_int(*line)),
            sequence.map(store::sql_int),
            container_id,
            store::has_refs(event),
            body,
            store::sha256(body.as_bytes()),
        ])?;
        stored.push(store::StoredEvent {
            artifact_id,
            container_id,
            order_key,
            event: event.clone(),
        });
    }
    stored.sort_by(|a, b| a.order_key.cmp(&b.order_key));
    // The rows the v8 state derives, once. Their renders are what each
    // clean file holds, unless the v8 cache derived them under another
    // adapter document, whose renders this host cannot reproduce.
    let derived = store::fold_events(adapters, &stored);
    let same_adapters = state.adapters == fingerprint;
    // Under another document the v8 render is unknown, so, as a format 8
    // host's own adapter change did (`adapter_dependent_files`), every
    // roll-up and every artifact whose render depends on the document at
    // all (it differs from the render with none) is dirty.
    let plain = (!same_adapters).then(|| store::fold_events(None, &stored));
    let depends_on_adapters = |id: &str, row: &ArtifactRow| {
        plain.as_ref().is_some_and(|plain| {
            plain
                .artifacts
                .get(id)
                .map(|plain| render_artifact(artifacts_dir, plain))
                != Some(render_artifact(artifacts_dir, row))
        })
    };
    let mut dirty = state.dirty.clone();
    let file_path = |id: &str| {
        state
            .container_files
            .get(id)
            .and_then(|path| catchup::portable_path(path))
    };
    for (id, row) in &derived.artifacts {
        let rolled_up = row.rolled_up || state.containers.contains(id);
        if !same_adapters && (rolled_up || depends_on_adapters(id, row)) {
            dirty.insert(id.clone());
        }
        let render = (same_adapters && !dirty.contains(id))
            .then(|| rebuild::render_digest(artifacts_dir, row));
        tx.prepare_cached(
            "INSERT INTO brauser_group_state(artifact_id, latest_key, latest_at, why_at,
               rolled_up, file_path, render_sha256)
             VALUES (?1, x'', '', NULL, ?2, ?3, ?4)",
        )?
        .execute(params![id, rolled_up, file_path(id), render])?;
    }
    // A container file whose container no longer derives a record.
    for id in state.container_files.keys() {
        if derived.artifacts.contains_key(id) {
            continue;
        }
        if let Some(path) = file_path(id) {
            tx.prepare_cached(
                "INSERT INTO brauser_group_state(artifact_id, latest_key, latest_at, why_at,
                   rolled_up, file_path)
                 VALUES (?1, x'', '', NULL, 1, ?2)",
            )?
            .execute(params![id, path])?;
        }
    }
    store::mark_dirty(tx, &dirty)?;
    store::set_meta(tx, "next_sequence", &state.next_sequence.to_string())
}

impl ArtifactIndex {
    fn legacy_file(&self, name: &str) -> PathBuf {
        self.cache_dir.join(name)
    }

    fn legacy_files_exist(&self) -> bool {
        LEGACY_FILES
            .iter()
            .any(|name| fs::symlink_metadata(self.legacy_file(name)).is_ok())
    }

    /// The v8 cache, if it is usable for this folder: format 8 within the
    /// 64 MiB bound, written for this root and layout, with valid evidence.
    fn usable_legacy_state(&self) -> Option<LegacyState> {
        let state: LegacyState = read_bounded_json(&self.legacy_file(LEGACY_FILES[0])).ok()??;
        (state.version == LEGACY_VERSION
            && state.root == self.storage.root
            && state.root_identity == self.root_identity
            && state.log_dir == self.storage.log_dir
            && state.artifacts_dir == self.storage.artifacts_dir
            && state.events.values().all(CachedEvent::valid_provenance))
        .then_some(state)
    }

    /// Whether the log folder exists and holds anything: the migration
    /// creates `index.db` only when there is something to index.
    fn logs_exist(&self) -> bool {
        let Ok((root, _)) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))
        else {
            return true;
        };
        let Ok(log_dir) = checked_relative_dir(&self.storage.log_dir) else {
            return true;
        };
        match root.open_dir(&log_dir) {
            Ok(logs) => logs
                .entries()
                .map_or(true, |mut entries| entries.next().is_some()),
            Err(error) => error.kind() != std::io::ErrorKind::NotFound,
        }
    }

    /// Detect a legacy cache and, when one needs migrating, commit its
    /// import (or the fallback) and the start of the staged rebuild, once.
    /// Never touches a path `db::open` would refuse, and never creates
    /// `index.db` when there is nothing to index.
    pub(super) fn legacy_step(&self) -> Result<Legacy> {
        let path = db::db_path(&self.cache_dir);
        let Ok(file) = db::detect_legacy(&path) else {
            return Ok(Legacy::None);
        };
        match file {
            db::FileState::Directory | db::FileState::NotRegular | db::FileState::ForeignWal => {
                return Ok(Legacy::None);
            }
            db::FileState::Missing | db::FileState::Empty => {
                if !self.legacy_files_exist() {
                    return Ok(Legacy::None);
                }
                let content = self
                    .usable_legacy_state()
                    .is_some_and(|state| state.has_content());
                if !content && !self.logs_exist() {
                    return Ok(Legacy::None);
                }
            }
            db::FileState::Ours => {
                let Ok(Some(mut conn)) = db::open(&self.cache_dir, db::Mode::IfExists) else {
                    return Ok(Legacy::None);
                };
                let files = self.legacy_files_exist();
                let (step, empty) = db::read_txn(&mut conn, |tx| {
                    let value = store::meta(tx, LEGACY_KEY)?;
                    let step = progress(value.as_deref(), catchup::rebuilding(tx)?);
                    let empty = value.is_none() && files && store_is_empty(tx)?;
                    Ok((step, empty))
                })?;
                // A store with no record of a migration is this host's own,
                // unless it is still empty beside a legacy cache (a host
                // stopped between creating it and committing the import).
                if !empty {
                    return Ok(step);
                }
            }
            db::FileState::Legacy => {}
        }
        let adapters = self.adapters.as_deref();
        let fingerprint = self.adapters_fingerprint();
        let mut conn = store::open(&self.cache_dir)?;
        db::write_txn(&mut conn, |tx| {
            // Another host may have migrated (or begun to) in between.
            if let Some(value) = store::meta(tx, LEGACY_KEY)? {
                return Ok(progress(Some(&value), catchup::rebuilding(tx)?));
            }
            if !store_is_empty(tx)? {
                store::set_meta(tx, LEGACY_KEY, "done")?;
                return Ok(Legacy::None);
            }
            let finish = match self.usable_legacy_state() {
                Some(state) => {
                    import(
                        tx,
                        adapters,
                        &fingerprint,
                        &self.storage.artifacts_dir,
                        &state,
                    )?;
                    store::set_meta(tx, LEGACY_KEY, "imported")?;
                    rebuild::Finish::Changed
                }
                None => {
                    store::set_meta(tx, LEGACY_KEY, "fallback")?;
                    rebuild::Finish::Missing
                }
            };
            rebuild::begin(tx)?;
            Ok(Legacy::Rebuild(finish))
        })
    }

    /// Advance a pending migration by one bounded rebuild pass (all of them
    /// with `all_passes`), then clean up once the rebuild has committed.
    /// While passes remain, history is unavailable.
    pub(super) fn migrate_legacy(&self, all_passes: bool) -> Result<()> {
        let step = self.legacy_step()?;
        self.advance_legacy(step, all_passes)
    }

    /// Advance the migration from `step`. Its rebuild only resumes: when
    /// another host has finished it since `step` was read, no new one
    /// begins.
    pub(super) fn advance_legacy(&self, step: Legacy, all_passes: bool) -> Result<()> {
        match step {
            Legacy::None => return Ok(()),
            Legacy::Rebuild(finish) => {
                while !self.resume_rebuild_pass(finish)? {
                    if !all_passes {
                        bail!(INCOMPLETE_CATCH_UP);
                    }
                }
            }
            Legacy::Cleanup => {}
        }
        self.finish_legacy()
    }

    /// Migration step 7: remove the legacy JSON files and record the
    /// migration as done, once, after the rebuild has committed.
    pub(super) fn finish_legacy(&self) -> Result<()> {
        let mut conn = store::open(&self.cache_dir)?;
        let finished = db::write_txn(&mut conn, |tx| {
            if store::meta(tx, LEGACY_KEY)?.as_deref() == Some("done") {
                return Ok(false);
            }
            for name in LEGACY_FILES {
                match fs::remove_file(self.legacy_file(name)) {
                    Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                        return Err(error).with_context(|| format!("removing the legacy {name}"));
                    }
                    _ => {}
                }
            }
            store::set_meta(tx, LEGACY_KEY, "done")?;
            Ok(true)
        })?;
        drop(conn);
        // Until plan T15 removes it, the host's own JSON cache mirrors the
        // migrated store, so the next catch-up does not rebuild it again.
        if finished {
            self.write_state(&self.store_state()?)?;
        }
        Ok(())
    }
}

/// Bring a cache (or saved catch-up progress) of an older format up to
/// `STATE_VERSION`, scheduling a one-time gap rescan. False means it must be
/// rebuilt from the logs instead.
pub(super) fn migrate_state(state: &mut IndexState) -> bool {
    if state.events.values().any(|event| !event.valid_provenance()) {
        return false;
    }
    match state.version {
        STATE_VERSION => return true,
        4 => migrate_v4_state(state),
        5 => {}
        6 | 7 => {}
        _ => return false,
    }
    if state.version <= 5 {
        migrate_v5_state(state);
    }
    state.version = STATE_VERSION;
    state.gap_logs.clear();
    state.gaps.clear();
    true
}

/// Upgrade a version 5 cache. It holds no `EventData::Summary`: #59's
/// summary events had no caller, so no log held one, and a cached event of
/// #59's shape (no record key or harness) fails to deserialize, which makes
/// the whole cache unreadable and rescanned. Version 6 changes summary
/// parse acceptance (the record key and harness, adapter artifact types)
/// and summary derivation, and the same release changed how every artifact
/// file renders (front matter through `markdown::yaml_string`, the Why line
/// through `markdown::inline`), so every artifact becomes dirty: the next
/// regeneration rewrites each file whose rendering changed and leaves the
/// rest unchanged. The checkpoints are kept, so a log a version 5 build
/// checkpointed after skipping a version 6 summary marker (a downgrade,
/// then an upgrade) is not reread until it changes; `brauser reindex`
/// recovers that summary.
pub(super) fn migrate_v5_state(state: &mut IndexState) {
    let artifacts = state
        .events
        .values()
        .map(|event| event.artifact_id().to_owned())
        .collect::<Vec<_>>();
    state.dirty.extend(artifacts);
    state.version = STATE_VERSION;
}

/// Upgrade a version 4 cache to version 5: drop activity a log rescan
/// would now skip (a title over capture's limit), and mark those artifacts
/// and every M1 visit's artifact dirty so their owned files are regenerated
/// with the v5 derivation (M1 visits count as opens).
pub(super) fn migrate_v4_state(state: &mut IndexState) {
    let dropped = state
        .events
        .iter()
        .filter(|(_, event)| {
            matches!(&event.data, EventData::Activity(event) if !logged_activity_is_bounded(event))
        })
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for id in dropped {
        if let Some(event) = state.events.remove(&id) {
            state.dirty.insert(event.artifact_id().to_owned());
        }
        state.positions.remove(&id);
        state.sequence.remove(&id);
    }
    for event in state.events.values() {
        if let EventData::Legacy(event) = &event.data {
            state.dirty.insert(event.artifact_id.clone());
        }
    }
    state.version = 5;
}
