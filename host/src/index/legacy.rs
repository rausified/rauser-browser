//! Migration of a format 8 cache (plan T16). Older `index-state.json`
//! formats are not migrated: they take the fallback (nothing imported).
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
//! 4. Once the rebuild has committed, `index-legacy-imported` records the
//!    legacy JSON files as they are and `legacy_import` becomes `done`. A
//!    crash before that repeats only what has not committed.
//!
//! The legacy JSON files are left in place. A format 8 host still running
//! the old binary (a side panel open across the upgrade) keeps catching up
//! from its own checkpoints and marks only new events dirty; had they been
//! removed, it would rebuild from nothing, mark every artifact dirty and
//! rewrite every owned file, user edits included. Files that still match
//! `index-legacy-imported` count as absent, so a format 9 host never imports
//! the same stale cache twice (after a damaged `index.db` is replaced, say).
//! A pre-format-9 host that later renames its snapshot over `index.db`
//! rewrites them too, so the next open sees a foreign file and fresh JSON,
//! migrates again, and the logs recover anything written in between.

use rusqlite::{Transaction, params};

use super::*;

/// The `brauser_meta` key recording the migration's progress.
const LEGACY_KEY: &str = "legacy_import";
/// The JSON cache format a format 8 host wrote.
const LEGACY_VERSION: u8 = 8;
/// The legacy JSON files a format 8 host writes. The migration reads them
/// and leaves them for any format 8 host still running.
const LEGACY_FILES: [&str; 2] = ["index-state.json", "index-catch-up.json"];
/// Records the legacy JSON files as they were when a migration finished, so
/// the same files are never imported again; a format 8 host that writes them
/// afterwards changes them, and they count again.
const IMPORTED_MARKER: &str = "index-legacy-imported";

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

    /// The legacy JSON files' sizes and modification times, as one line.
    fn legacy_fingerprint(&self) -> String {
        LEGACY_FILES
            .iter()
            .map(|name| match fs::symlink_metadata(self.legacy_file(name)) {
                Ok(metadata) => {
                    let modified = metadata
                        .modified()
                        .ok()
                        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |since| since.as_nanos());
                    format!("{name}:{}:{modified}", metadata.len())
                }
                Err(_) => format!("{name}:-"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Whether the legacy JSON files are exactly the ones a finished
    /// migration already imported.
    fn legacy_files_imported(&self) -> bool {
        fs::read_to_string(self.legacy_file(IMPORTED_MARKER))
            .is_ok_and(|marker| marker == self.legacy_fingerprint())
    }

    /// Whether a legacy JSON file exists that no finished migration has
    /// imported as it is now.
    fn legacy_files_exist(&self) -> bool {
        LEGACY_FILES
            .iter()
            .any(|name| fs::symlink_metadata(self.legacy_file(name)).is_ok())
            && !self.legacy_files_imported()
    }

    /// The v8 cache, if it is usable for this folder: format 8 within the
    /// 64 MiB bound, written for this root and layout, with valid evidence.
    fn usable_legacy_state(&self) -> Option<LegacyState> {
        if self.legacy_files_imported() {
            return None;
        }
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
    pub(super) fn logs_exist(&self) -> bool {
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
    /// While passes remain, history is unavailable. Returns whether this
    /// call saw a fallback rebuild (`Finish::Missing`) finish, which kept
    /// every existing file unmarked.
    pub(super) fn migrate_legacy(&self, all_passes: bool) -> Result<bool> {
        let step = self.legacy_step()?;
        self.advance_legacy(step, all_passes)
    }

    /// Advance the migration from `step`. Its rebuild only resumes: when
    /// another host has finished it since `step` was read, no new one
    /// begins. Returns whether a fallback rebuild finished.
    pub(super) fn advance_legacy(&self, step: Legacy, all_passes: bool) -> Result<bool> {
        match step {
            Legacy::None => return Ok(false),
            Legacy::Rebuild(finish) => {
                while !self.resume_rebuild_pass(finish)? {
                    if !all_passes {
                        bail!(INCOMPLETE_CATCH_UP);
                    }
                }
            }
            Legacy::Cleanup => {}
        }
        self.finish_legacy()?;
        Ok(step == Legacy::Rebuild(rebuild::Finish::Missing))
    }

    /// Migration step 7: record the legacy JSON files as imported and the
    /// migration as done, once, after the rebuild has committed. The files
    /// stay: a format 8 host still running reads them (see the module docs).
    pub(super) fn finish_legacy(&self) -> Result<()> {
        let mut conn = store::open(&self.cache_dir)?;
        db::write_txn(&mut conn, |tx| {
            if store::meta(tx, LEGACY_KEY)?.as_deref() == Some("done") {
                return Ok(());
            }
            // The only file this module writes; never a legacy JSON file.
            atomic_write(
                &self.legacy_file(IMPORTED_MARKER),
                self.legacy_fingerprint().as_bytes(),
            )
            .with_context(|| format!("recording the imported legacy cache in {IMPORTED_MARKER}"))?;
            store::set_meta(tx, LEGACY_KEY, "done")
        })
    }
}
