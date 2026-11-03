//! Rebuildable artifact index and lazily generated artifact Markdown.
//!
//! The append-only daily log is authoritative. `index.db` is a rusqlite
//! database (`store`) whose `brauser_events` hold every indexed event and
//! whose derived tables serve current and future readers; a delivery
//! catches up with the logs through the bounded `brauser_logs` watermark
//! (`catchup`) and re-derives only the groups it touches. The database may
//! be deleted; `brauser reindex` recreates it.
//! A rebuild (reindex, a lost or foreign database, a removed or shrunk log)
//! is staged in `index.db` (`rebuild.rs`): each bounded pass commits its
//! progress there, the previous rows keep serving, and the next pass
//! resumes.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions as StdOpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::SystemTime;

use adapter_templates::{Document, Ref};
use anyhow::{Context, Result, bail};
use brauser_protocol::{
    ActivityEvent, ArtifactListItem, ArtifactRefView, ArtifactView, ContainedArtifactView,
    IncomingTrailView, OverflowMarker, RecordKey, RecordKeyKind, StorageConfig, SummaryView,
    WhyEvent,
};
use cap_std::fs::Dir;
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime, UtcOffset};
use url::Url;
use uuid::Uuid;

use crate::brand::{APP_NAME, NAMESPACE};
use crate::capture::{
    activity_shape_is_valid, canonical_url, logged_activity_is_bounded, parse_event_id,
    trail_transition_is_valid,
};
use crate::markdown::{self, yaml_string};
use crate::relearn::aliases::Resolution;
use crate::summary::SummaryEvent;
use crate::url_provenance::{StoredUrl, UrlProvenance};
use crate::vault::{
    OwnedWrite, PostPublishWarning, STALE_TEMPORARY_AGE, checked_relative_dir,
    is_generated_temporary, open_selected_root, owned_frontmatter, replace_owned_file,
    sync_directory,
};

mod catchup;
mod db;
mod fold;
mod legacy;
mod ord;
mod project;
mod query;
mod rebuild;
mod regen;
mod render;
mod scan;
mod store;
#[cfg(test)]
pub(crate) mod tests;
pub(crate) use db::BUSY_MESSAGE;
/// Busy accounting for host-level tests (7vf).
#[cfg(test)]
pub(crate) use db::{BUSY_SURFACED, BusyTimeout};

use fold::*;
pub(crate) use fold::{unvisited_container, unvisited_within};
use legacy::*;
pub(crate) use project::RekeyProjection;
#[cfg(test)]
use query::*;
use regen::*;
use render::*;
use scan::*;

/// The cache format: 9 is the rusqlite `index.db` (`PRAGMA user_version`).
/// Format 8 kept a JSON cache, which `legacy.rs` imports once; older JSON
/// formats take that migration's fallback.
const STATE_VERSION: u8 = 9;
/// The bound on the one read of a legacy JSON cache (`legacy.rs`).
const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SCAN_FILES: usize = 100_000;
const MAX_WORKLOG_BYTES: usize = 96 * 1024;
/// The longest agent or page field rendered on one line of an artifact file.
const MAX_RENDERED_FIELD_CHARS: usize = 2048;
const LOG_HEADER: &str = concat!("<!-- ", crate::namespace!(), ":daily-log v1 -->");
const MARKER_PREFIX: &str = concat!("<!-- ", crate::namespace!(), ":visit id=");

#[cfg(test)]
thread_local! {
    static LOG_BYTES_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Derives of a whole (nonempty) history: a rebuild's finalize or an
    /// adapter change. A delivery runs none.
    static FULL_DERIVES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Committed `db::write_txn` transactions.
    static TRANSACTIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Rows inserted, updated or deleted by committed `db::write_txn`s.
    static ROWS_WRITTEN: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static SCAN_LIMIT: std::cell::Cell<u64> = const { std::cell::Cell::new(MAX_SCAN_BYTES) };
    /// Runs once, right after the next catch-up on this thread finds the
    /// store needs a rebuild: a test's stand-in for another host process
    /// finishing that rebuild in between.
    static AFTER_NEEDS_REBUILD: std::cell::Cell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::Cell::new(None) };
}

/// The changed-file read bound for one catch-up pass. Tests lower it so a
/// history larger than one pass needs only a few small logs.
fn scan_byte_limit() -> u64 {
    #[cfg(test)]
    return SCAN_LIMIT.with(std::cell::Cell::get);
    #[cfg(not(test))]
    MAX_SCAN_BYTES
}

#[derive(Clone)]
pub struct ArtifactIndex {
    cache_dir: PathBuf,
    storage: StorageConfig,
    root_identity: String,
    /// The confirmed adapter document, used only to resolve title refs
    /// into mentions edges (adapter-templates.md §10.1).
    adapters: Option<Arc<Document>>,
    /// One read connection for this handle's reads. native.rs opens a
    /// handle per request, so a request's several reads share one open of
    /// `index.db` and its `-wal`/`-shm` (opening files is slow where
    /// endpoint security scans each open), and the handle is closed with
    /// the request, so no long-lived handle blocks replacing the file on
    /// Windows. Dropped whenever a catch-up runs, since it may replace it.
    reader: ReaderCache,
}

/// [`ArtifactIndex::reader`]: a clone starts with no connection of its own.
/// The connection is kept with the identity of the `index.db` it opened and
/// reused only while the path still names that file, so a replaced or
/// deleted database is never read through a stale handle. On Windows
/// nothing is cached: an open handle there would block another host from
/// replacing the file.
#[derive(Default)]
struct ReaderCache(Mutex<Option<(rusqlite::Connection, (u64, u64))>>);

impl Clone for ReaderCache {
    fn clone(&self) -> Self {
        Self::default()
    }
}

impl ReaderCache {
    /// The cached connection, if `db` still names the file it opened.
    fn take(&self, db: &Path) -> Option<rusqlite::Connection> {
        let (conn, identity) = self
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()?;
        (file_identity(db) == Some(identity)).then_some(conn)
    }

    /// Keep `conn`, opened on `db`, for this handle's next read.
    fn put(&self, conn: rusqlite::Connection, db: &Path) {
        if cfg!(unix)
            && let Some(identity) = file_identity(db)
        {
            *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some((conn, identity));
        }
    }
}

/// `path`'s device and inode, or `None` where that is not available.
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).ok()?;
        Some((metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RegenerationReport {
    pub written: usize,
    pub unchanged: usize,
    pub conflicts: usize,
    pub conflict_paths: Vec<String>,
    /// Artifacts whose file could not be written this pass. They stay dirty
    /// and never stop the pass from updating the others.
    pub failed: usize,
    pub failed_paths: Vec<String>,
    pub first_failure: Option<String>,
}

#[derive(PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedGap {
    marker: OverflowMarker,
    position: (PathBuf, u64),
}

/// A rejected body for an indexed event ID and its earliest log position.
type Conflict = (CachedEvent, Option<(PathBuf, u64)>);

/// What a read's stat-only check found (`ArtifactIndex::probe`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Probe {
    /// The snapshot is this folder's under these adapters, and every log is
    /// checkpointed at its current size and modification time.
    Current,
    /// The snapshot is this folder's, but the logs are ahead of it.
    Behind,
    /// The snapshot cannot be served without a catch-up: no database, a
    /// legacy file, another folder's rows, other adapters, or a store that
    /// does not yet hold every event its watermark covers.
    Unusable,
}

/// Whether `error` is the write lock staying busy past its timeout.
pub(crate) fn busy(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(cause.downcast_ref::<db::DbError>(), Some(db::DbError::Busy))
            || cause
                .downcast_ref::<rusqlite::Error>()
                .is_some_and(db::is_busy)
    })
}

const INCOMPLETE_CATCH_UP: &str = "artifact history is unavailable: catch-up reached its 64 MiB per-pass limit; the next pass resumes after the logs already read";

/// Metadata is only an ingestion checkpoint, never capture authorization.
/// Reparse a changed file in full so partial trailing markers and legacy
/// markers (which use the preceding prose line) are handled identically.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LogCheckpoint {
    bytes: u64,
    modified: (u64, u32),
}

impl LogCheckpoint {
    fn read(metadata: &cap_std::fs::Metadata) -> Result<Self> {
        let modified = metadata
            .modified()?
            .into_std()
            .duration_since(std::time::UNIX_EPOCH)?;
        Ok(Self {
            bytes: metadata.len(),
            modified: (modified.as_secs(), modified.subsec_nanos()),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum EventData {
    Activity(ActivityEvent),
    Why(WhyEvent),
    Legacy(LegacyVisit),
    Summary(SummaryEvent),
}

/// Cache format 7 adds names-only provenance beside each event. Flattening
/// retains the previous tagged event shape; old records deserialize with
/// unknown evidence and migrate without dropping their log checkpoints.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
struct CachedEvent {
    #[serde(flatten)]
    data: EventData,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url_provenance: Option<UrlProvenance>,
}

impl From<EventData> for CachedEvent {
    fn from(data: EventData) -> Self {
        Self {
            data,
            url_provenance: None,
        }
    }
}

impl CachedEvent {
    fn artifact_id(&self) -> &str {
        match &self.data {
            EventData::Activity(event) => &event.artifact_id,
            EventData::Why(event) => &event.artifact_id,
            EventData::Legacy(event) => &event.artifact_id,
            EventData::Summary(event) => &event.artifact_id,
        }
    }

    /// URLs attributed to the event, with separate positional evidence for
    /// modes and trail sources. Trail URLs never name this artifact's notes.
    fn url_records(&self) -> Vec<(String, StoredUrl)> {
        let (url, modes, trails): (&str, &[_], &[_]) = match &self.data {
            EventData::Activity(event) => {
                (&event.url, event.modes.as_slice(), event.trails.as_slice())
            }
            EventData::Why(event) => (&event.url, &[], &[]),
            EventData::Legacy(event) => (&event.url, &[], &[]),
            EventData::Summary(event) => (&event.url, &[], &[]),
        };
        let provenance = self.url_provenance.as_ref();
        let mut result = vec![(
            "url".into(),
            StoredUrl {
                url: url.into(),
                stripped_params: provenance.map(|p| p.url.clone()),
            },
        )];
        result.extend(modes.iter().enumerate().map(|(index, mode)| {
            (
                format!("mode:{index}"),
                StoredUrl {
                    url: mode.url.clone(),
                    stripped_params: provenance.map(|p| p.modes[index].clone()),
                },
            )
        }));
        result.extend(trails.iter().enumerate().filter_map(|(index, trail)| {
            trail.from_url.as_ref().map(|url| {
                (
                    format!("trail:{index}"),
                    StoredUrl {
                        url: url.clone(),
                        stripped_params: provenance.and_then(|p| p.trails[index].clone()),
                    },
                )
            })
        }));
        result
    }

    fn urls(&self) -> impl Iterator<Item = StoredUrl> {
        self.url_records()
            .into_iter()
            .filter(|(role, _)| !role.starts_with("trail:"))
            .map(|(_, url)| url)
    }

    fn valid_provenance(&self) -> bool {
        self.url_provenance
            .as_ref()
            .is_none_or(|provenance| match &self.data {
                EventData::Activity(event) => provenance.valid(
                    event.modes.len(),
                    &event
                        .trails
                        .iter()
                        .map(|trail| trail.from_url.is_some())
                        .collect::<Vec<_>>(),
                ),
                _ => provenance.valid(0, &[]),
            })
    }

    fn activity(event: &ActivityEvent, strip: &[String]) -> Result<Self> {
        Ok(Self {
            data: EventData::Activity(normalized_activity(event, strip)?),
            url_provenance: Some(UrlProvenance::activity(event, strip)?),
        })
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyVisit {
    marker_id: String,
    artifact_id: String,
    occurred_at: String,
    url: String,
    title: String,
}

#[derive(Clone)]
struct ArtifactRow {
    artifact_id: String,
    artifact_type: String,
    site: String,
    title: String,
    url: String,
    first_seen: String,
    last_seen: String,
    focused_secs: u64,
    background_secs: u64,
    opens: u64,
    days: BTreeSet<String>,
    last_mode: Option<String>,
    why: Option<String>,
    activities: Vec<ActivityEvent>,
    summaries: Vec<SummaryEvent>,
    /// Instants of M1 visit entries, each one cold open with no measures.
    legacy_visits: Vec<OffsetDateTime>,
    latest_at: OffsetDateTime,
    /// The record holds activity rolled up from artifacts within it. Its
    /// file says so (`brauser.rollup: true`), so a rebuild without the
    /// cache's `container_files` can still find a container file whose
    /// record no longer derives and remove it.
    rolled_up: bool,
}

#[derive(Clone)]
struct VisitRow {
    event_id: String,
    artifact_id: String,
    occurred_at: String,
    local_date: String,
    focused_secs: u64,
    background_secs: u64,
    opens: u64,
    mode: Option<String>,
    title: String,
    url: String,
    /// Set only for a visit derived from an `EventData::Summary`, so
    /// `worklog` can report it distinctly instead of as a zero-measure,
    /// merely-"seen" artifact.
    summary: Option<SummaryEvent>,
    /// A container's copy of a visit to an artifact within it. The visit
    /// itself is the child's row; totals across artifacts skip this copy so
    /// each second counts once.
    contained: bool,
}

#[derive(Clone, Serialize)]
struct EdgeRow {
    source: String,
    target: String,
    kind: String,
    weight: u64,
    last_seen: String,
}

struct Derived {
    artifacts: BTreeMap<String, ArtifactRow>,
    visits: Vec<VisitRow>,
    edges: Vec<EdgeRow>,
    /// The instant of each row's `why`, for rows that have one, so a later
    /// fold can be seeded with it (`Fold::seeded`).
    why_at: BTreeMap<String, OffsetDateTime>,
}

/// The local day the last regeneration pass completed, the UTC offset that
/// day was taken in, and the storage it covered (`ArtifactIndex::daily_pass_due`).
/// The cache is shared, so a pass for another folder never counts.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PassMarker {
    date: String,
    offset_seconds: i32,
    root: String,
    root_identity: String,
    artifacts_dir: String,
}

impl ArtifactIndex {
    pub fn open(storage: &StorageConfig, expected_identity: Option<&str>) -> Result<Self> {
        let dirs = ProjectDirs::from("", "", APP_NAME)
            .context("cannot locate this user's application cache directory")?;
        Self::open_in(storage, expected_identity, dirs.cache_dir())
    }

    pub fn open_in(
        storage: &StorageConfig,
        expected_identity: Option<&str>,
        cache_dir: &Path,
    ) -> Result<Self> {
        checked_relative_dir(&storage.log_dir)?;
        checked_relative_dir(&storage.artifacts_dir)?;
        let (_, identity) = open_selected_root(Path::new(&storage.root), expected_identity)?;
        Ok(Self {
            cache_dir: cache_dir.to_path_buf(),
            storage: storage.clone(),
            root_identity: identity,
            adapters: None,
            reader: ReaderCache::default(),
        })
    }

    /// Resolve title refs through this adapter document. Without one, every
    /// ref stays an unresolved key that never counts as a link.
    pub fn with_adapters(mut self, adapters: Option<&Document>) -> Self {
        self.adapters = adapters.cloned().map(Arc::new);
        self
    }

    fn adapters_fingerprint(&self) -> String {
        self.adapters
            .as_deref()
            .map_or_else(String::new, |document| {
                sha256_hex(format!("{document:?}").as_bytes())
            })
    }

    pub fn record_activity(&self, event: &ActivityEvent, strip_params: &[String]) -> Result<()> {
        self.record_events([CachedEvent::activity(event, strip_params)?])
    }

    pub fn record_why(&self, event: &WhyEvent, strip_params: &[String]) -> Result<()> {
        let provenance = UrlProvenance::page(&event.url, strip_params)?;
        let mut event = event.clone();
        event.event_id = parse_event_id(&event.event_id).map_err(anyhow::Error::msg)?;
        event.url = canonical_url(&event.url, strip_params)?;
        self.record_events([CachedEvent {
            data: EventData::Why(event),
            url_provenance: Some(provenance),
        }])
    }

    /// Index a "Summarize & log" result already durable in the daily log
    /// (`crate::capture::CaptureStore::record_summary` wrote it). `event.url`
    /// and `event.artifact_id` must already be normalized/derived, exactly as
    /// that writer leaves them.
    pub fn record_summary(&self, event: &SummaryEvent) -> Result<()> {
        self.record_events([EventData::Summary(event.clone()).into()])
    }

    pub fn record_activities<'a>(
        &self,
        events: impl IntoIterator<Item = &'a ActivityEvent>,
        strip_params: &[String],
    ) -> Result<()> {
        let events = events
            .into_iter()
            .map(|event| CachedEvent::activity(event, strip_params))
            .collect::<Result<Vec<_>>>()?;
        self.record_events(events)
    }

    /// Index events whose log append already succeeded. Catch-up has just
    /// read the authoritative log, so a body that disagrees with an indexed
    /// event is skipped (and reported) without dropping the rest.
    ///
    /// The events go into `brauser_events` in one transaction that first
    /// catches the `brauser_logs` watermark up with the logs (delivery step
    /// 2, usually just this delivery's own append), then derives only the
    /// affected groups (each event's artifact and container), by the append
    /// fast path where it is exact, and marks them dirty with fresh
    /// generations; no full derive runs.
    fn record_events(&self, events: impl IntoIterator<Item = CachedEvent>) -> Result<()> {
        let events = events.into_iter().collect::<Vec<_>>();
        if self.catch_up_store(&events)? > 0 {
            bail!("an indexed event ID cannot be reused with different content");
        }
        Ok(())
    }

    /// Reconcile a terminal replay or a newly drained legacy visit from the
    /// authoritative owned markers. In particular, never let an untrusted
    /// retry body replace the event that is already durable in the log.
    /// This is the incremental catch-up: only changed logs are read, and
    /// only events newly ingested from them make their artifacts dirty, so
    /// a duplicate retry neither forces a regeneration pass nor replaces
    /// external edits to files that had no new activity. A catch-up that
    /// reaches the per-pass read bound reports unavailable history until a
    /// later call finishes it.
    pub fn reconcile_from_logs(&self) -> Result<()> {
        self.catch_up_store(&[]).map(|_| ())
    }

    /// Bring `index.db` level with the logs and then ingest `events`, all in
    /// one write transaction. Returns how many of `events` conflicted with
    /// an indexed body.
    ///
    /// - A pending format 8 migration advances first, one bounded pass.
    /// - With no database and nothing to index (no events, no logs), nothing
    ///   is created: `index.db` is opened only to commit ingested data.
    /// - A store for another folder or layout (or a new one), or one with a
    ///   staged rebuild pending (a shrunk or removed log, a stopped
    ///   reindex), runs one bounded staged-rebuild pass; until it finishes,
    ///   history is unavailable.
    /// - A changed adapter document re-derives the store once.
    /// - A catch-up that reaches the per-pass read bound commits what it
    ///   read and reports unavailable history; the next call resumes.
    fn catch_up_store(&self, events: &[CachedEvent]) -> Result<usize> {
        let result = self.catch_up_store_once(events);
        if let Err(error) = &result
            && !events.is_empty()
            && format!("{error:#}").contains(INCOMPLETE_CATCH_UP)
        {
            // A pending rebuild kept these events out. The logs hold them,
            // but a fallback finalize would not mark their existing files.
            let noted = store::open(&self.cache_dir).and_then(|mut conn| {
                db::write_txn(&mut conn, |tx| {
                    rebuild::note_delivered(tx, events.iter().map(|event| event_identity(event).0))
                })
            });
            if let Err(noted) = noted {
                tracing::warn!(
                    error = format!("{noted:#}"),
                    "could not note a delivery made during an index rebuild"
                );
            }
        }
        result
    }

    /// [`Self::catch_up_store`] without noting a delivery a pending rebuild
    /// kept out.
    fn catch_up_store_once(&self, events: &[CachedEvent]) -> Result<usize> {
        // A fallback finalize marks only missing files, so the caller's
        // events it just read from the log still mark their groups below.
        let fallback_finished = self.migrate_legacy(false)?;
        if events.is_empty() && self.nothing_to_commit()? {
            return Ok(0);
        }
        let adapters = self.adapters.as_deref();
        let fingerprint = self.adapters_fingerprint();
        // A catch-up that finds a shrunk or removed log commits the rebuild
        // request, which the next round's staged pass then serves.
        for _ in 0..2 {
            // The stat-free check keeps a normal delivery to one write
            // transaction; the pass decides again under the write lock.
            if self.needs_rebuild()? {
                #[cfg(test)]
                if let Some(hook) = AFTER_NEEDS_REBUILD.with(std::cell::Cell::take) {
                    hook();
                }
                if !self.rebuild_if_needed(rebuild::Finish::Changed)? {
                    bail!(INCOMPLETE_CATCH_UP);
                }
            }
            let mut conn = store::open(&self.cache_dir)?;
            let outcome = store::write(&mut conn, adapters, |tx| {
                if !self.location_matches(tx)? || catchup::rebuilding(tx)? {
                    return Ok(None);
                }
                rebuild::adapter_change(tx, adapters, &fingerprint, &self.storage.artifacts_dir)?;
                let mut containers = Containers::new(adapters);
                let mut changes = store::Changes::default();
                let mut dirty = BTreeSet::new();
                let caught = {
                    let mut sink = |tx: &rusqlite::Transaction<'_>,
                                    event: CachedEvent,
                                    position: (PathBuf, u64)| {
                        let (outcome, groups) =
                            store::ingest(tx, &mut containers, &event, Some(&position))?;
                        if matches!(outcome, store::Ingest::Inserted | store::Ingest::Moved) {
                            dirty.extend(groups.iter().cloned());
                        }
                        changes.record(outcome, &event_identity(&event).0, groups);
                        Ok(())
                    };
                    catchup::catch_up(tx, &self.storage, &self.root_identity, &mut sink)?
                };
                let complete = match caught {
                    catchup::CatchUp::Rebuilding => return Ok(None),
                    catchup::CatchUp::Caught { complete, .. } => complete,
                };
                // An incomplete pass ingests only what it read: the caller's
                // events are logged, so a later pass reaches them in order.
                let mut conflicts = 0;
                if complete {
                    for event in events {
                        let (outcome, groups) = store::ingest(tx, &mut containers, event, None)?;
                        if outcome == store::Ingest::Conflict {
                            conflicts += 1;
                        }
                        // A body already indexed (by an earlier delivery, or
                        // by this catch-up from its own log append) changes
                        // no row, so it neither marks nor bumps a generation,
                        // unless a fallback migration finished in this call:
                        // its finalize indexed the body without marking an
                        // existing file, and this delivery is a real change.
                        let fresh =
                            matches!(outcome, store::Ingest::Inserted | store::Ingest::Moved)
                                || (fallback_finished && outcome == store::Ingest::Same);
                        if fresh {
                            dirty.extend(groups.iter().cloned());
                        }
                        changes.record(outcome, &event_identity(event).0, groups);
                    }
                }
                store::apply(tx, adapters, &changes)?;
                store::mark_dirty(tx, &dirty)?;
                Ok(Some((complete, conflicts)))
            })?;
            match outcome {
                Some((true, conflicts)) => return Ok(conflicts),
                Some((false, _)) => bail!(INCOMPLETE_CATCH_UP),
                None => {}
            }
        }
        bail!(INCOMPLETE_CATCH_UP)
    }

    /// Whether the store must be (re)built from the logs before it can take
    /// a catch-up: it is new, it describes another folder or layout, or a
    /// staged rebuild is pending.
    fn needs_rebuild(&self) -> Result<bool> {
        let Some(mut conn) = db::open(&self.cache_dir, db::Mode::IfExists)? else {
            return Ok(true);
        };
        db::read_txn(&mut conn, |tx| {
            Ok(!self.location_matches(tx)? || catchup::rebuilding(tx)?)
        })
    }

    /// `index.db` is opened (and so created) only to commit ingested data.
    /// With no database yet, no logs to index and no owned roll-up file a
    /// rebuild would adopt for removal (a lost cache), a read, a pass or a
    /// reconciliation leaves the cache without one. Anything else at the
    /// path (a directory, a damaged or foreign file) is left to
    /// [`db::open`], which reports history unavailable without touching it.
    fn nothing_to_commit(&self) -> Result<bool> {
        let path = db::db_path(&self.cache_dir);
        match db::detect_legacy(&path) {
            Ok(db::FileState::Missing) => {
                Ok(!self.logs_exist() && self.owned_rollup_files().is_empty())
            }
            Ok(_) => Ok(false),
            Err(error) => Err(error)
                .with_context(|| format!("artifact history is unavailable: {}", path.display())),
        }
    }

    /// Rebuild `index.db` in place from owned log markers through the staged
    /// rebuild (`rebuild.rs`), then regenerate every artifact file. This is
    /// the explicit repair path for a deleted or corrupt cache and
    /// intentionally replaces user edits to owned files. A history larger
    /// than one pass's read bound is read in several bounded passes, each
    /// committing its progress, within this one call; a rebuild already in
    /// progress (another call, or a host that stopped mid-pass) resumes.
    ///
    /// A database SQLite reports as damaged (`SQLITE_CORRUPT`,
    /// `SQLITE_NOTADB`), even one whose header still reads as this cache's,
    /// is deleted with its sidecars and rebuilt from the logs.
    pub fn reindex(&self) -> Result<RegenerationReport> {
        match self.rebuild_for_reindex() {
            Err(error) if db::is_corrupt(&error) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "index.db is damaged; replacing it with one rebuilt from the logs"
                );
                db::discard(&self.cache_dir)?;
                self.rebuild_for_reindex()?;
            }
            result => result?,
        }
        // Vanished container files were adopted from the store and from the
        // owned roll-up files on disk; a replaced file is not Brauser's.
        let report = self.regenerate_with(|_, _| {});
        self.checkpoint_wal();
        report
    }

    /// The reindex's rebuild, with every artifact marked dirty after it.
    fn rebuild_for_reindex(&self) -> Result<()> {
        // A legacy cache's import still supplies the previous winners; the
        // reindex then finishes its rebuild and its cleanup.
        let migrating = self.legacy_step()? != Legacy::None;
        while !self.rebuild_pass(rebuild::Finish::All)? {}
        if migrating {
            self.finish_legacy()?;
        }
        // Another host may have finalized this rebuild's stage first, with
        // its own (narrower) marking, so the forced rewrite is marked here.
        self.mark_every_artifact_dirty()
    }

    /// Best-effort lifecycle pass used at native-host connection boundaries.
    pub fn regenerate_dirty(&self) -> Result<RegenerationReport> {
        self.reconcile_from_logs()?;
        self.regenerate_with(|_, _| {})
    }

    /// Whether no pass has completed yet on the current local day (§6.7), so
    /// even a delivery-only connection should run one. The day is the user's,
    /// taken from the offset of their latest activity when the pass ran; a
    /// missing or unreadable marker means a pass is due.
    pub fn daily_pass_due(&self, now: OffsetDateTime) -> bool {
        let Ok(bytes) = fs::read(self.pass_marker_path()) else {
            return true;
        };
        let Ok(marker) = serde_json::from_slice::<PassMarker>(&bytes) else {
            return true;
        };
        let Ok(offset) = UtcOffset::from_whole_seconds(marker.offset_seconds) else {
            return true;
        };
        marker.root != self.storage.root
            || marker.root_identity != self.root_identity
            || marker.artifacts_dir != self.storage.artifacts_dir
            || now.to_offset(offset).date().to_string() != marker.date
    }

    /// Remove index-cache and artifact temporaries left by a host killed
    /// between creating and publishing them. Only regular files whose whole
    /// name is one this module generates, and that are older than
    /// `STALE_TEMPORARY_AGE`, are removed. Returns how many were removed.
    pub fn sweep_stale_temporaries(&self) -> Result<usize> {
        let mut removed = match Dir::open_ambient_dir(&self.cache_dir, cap_std::ambient_authority())
        {
            Ok(cache) => sweep_temporaries_in(&cache, &format!(".{NAMESPACE}-index-"))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error).context("opening artifact index cache directory"),
        };
        let (root, _) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))?;
        let artifacts = match root.open_dir(checked_relative_dir(&self.storage.artifacts_dir)?) {
            Ok(dir) => dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(removed),
            Err(error) => return Err(error).context("opening artifacts directory"),
        };
        // Artifact files live exactly at `<type>/<site>/` (`artifact_path`).
        let prefix = format!(".{NAMESPACE}-artifact-");
        for types in subdirectories(&artifacts)? {
            for sites in subdirectories(&types)? {
                removed += sweep_temporaries_in(&sites, &prefix)?;
            }
        }
        Ok(removed)
    }

    fn pass_marker_path(&self) -> PathBuf {
        self.cache_dir.join("regenerated.json")
    }

    /// Owned artifact files on disk marked `brauser.rollup: true`, by
    /// artifact ID, relative to the home folder. Best effort: an unreadable
    /// folder or file is skipped (with a warning for a folder), since this
    /// only recovers cleanup a lost cache would otherwise miss.
    fn owned_rollup_files(&self) -> BTreeMap<String, PathBuf> {
        match self.scan_rollup_files() {
            Ok(found) => found,
            Err(error) => {
                tracing::warn!(
                    error = format!("{error:#}"),
                    "could not look for container files to clean up"
                );
                BTreeMap::new()
            }
        }
    }

    fn scan_rollup_files(&self) -> Result<BTreeMap<String, PathBuf>> {
        let mut found = BTreeMap::new();
        let (root, _) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))?;
        let base = checked_relative_dir(&self.storage.artifacts_dir)?;
        let artifacts = match root.open_dir(&base) {
            Ok(dir) => dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(found),
            Err(error) => return Err(error).context("opening artifacts directory"),
        };
        // Artifact files live exactly at `<type>/<site>/` (`artifact_path`).
        for (type_name, types) in named_subdirectories(&artifacts)? {
            for (site_name, sites) in named_subdirectories(&types)? {
                for entry in sites.entries().context("listing artifacts directory")? {
                    let entry = entry.context("listing artifacts directory")?;
                    let name = entry.file_name();
                    let Some(name) = name.to_str() else { continue };
                    if !name.ends_with(".md") || !entry.file_type().is_ok_and(|kind| kind.is_file())
                    {
                        continue;
                    }
                    let Ok(file) = sites.open(name) else { continue };
                    let mut head = Vec::new();
                    if Read::by_ref(&mut { file })
                        .take(ROLLUP_SCAN_BYTES)
                        .read_to_end(&mut head)
                        .is_err()
                    {
                        continue;
                    }
                    if let Some(id) = rollup_artifact_id(&head) {
                        found.insert(id, base.join(&type_name).join(&site_name).join(name));
                    }
                }
            }
        }
        Ok(found)
    }

    /// Return the current derived record for one recognized artifact. The
    /// append-only log remains authoritative: the read catches `index.db`
    /// up with it first when it is behind.
    pub fn artifact(&self, artifact_id: &str) -> Result<Option<ArtifactView>> {
        Ok(self.artifact_with_summary(artifact_id)?.0)
    }

    /// The artifact's derived record and its latest summary from one
    /// snapshot, so a panel request checks the logs once. A summary always
    /// derives a row, so a missing row means no summary either.
    pub fn artifact_with_summary(
        &self,
        artifact_id: &str,
    ) -> Result<(Option<ArtifactView>, Option<SummaryView>)> {
        self.read(|tx| query::card(tx, artifact_id))
    }

    /// The artifact's latest summary (by instant, then append order, as the
    /// artifact file's Summary line picks it) that renders as a view, or
    /// `None` when it has none.
    pub fn summary_for(&self, artifact_id: &str) -> Result<Option<SummaryView>> {
        self.read(|tx| query::summary_for(tx, artifact_id))
    }

    /// Canonical URLs previously recorded for an artifact, including mode
    /// evidence and explicit context before any measured activity. Note
    /// lookup uses these to discover URL-keyed M1 notes.
    pub fn artifact_urls(&self, artifact_id: &str) -> Result<Vec<String>> {
        Ok(self
            .artifact_url_evidence(artifact_id)?
            .into_iter()
            .map(|stored| stored.url)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    pub fn artifact_url_evidence(&self, artifact_id: &str) -> Result<Vec<StoredUrl>> {
        self.read(|tx| query::url_evidence(tx, artifact_id))
    }

    /// Fresh authoritative evidence for impact review. No cache is trusted,
    /// published or resumed. Oversized/inconsistent history refuses the plan.
    /// Caller holds the config and capture locks, in that order.
    /// It reads only the logs, never the cache directory.
    pub(crate) fn relearn_evidence(&self) -> Result<(String, Vec<(RecordKey, StoredUrl)>)> {
        let scan = self.relearn_state()?;
        let revision = scan.revision()?;
        let evidence = scan
            .events
            .values()
            .flat_map(|event| {
                let key = match &event.data {
                    EventData::Summary(summary) => summary.record_key.clone(),
                    _ => RecordKey {
                        kind: RecordKeyKind::Artifact,
                        value: event.artifact_id().to_owned(),
                    },
                };
                event.urls().map(move |url| (key.clone(), url))
            })
            .collect();
        Ok((revision, evidence))
    }

    /// The strict log-only scan behind re-learning review and projection.
    /// No checkpoint is trusted, published, regenerated or resumed here.
    fn relearn_state(&self) -> Result<LogScan> {
        let scan = LogScan::read(&self.storage, &self.root_identity, true)?;
        anyhow::ensure!(
            scan.complete,
            "re-learning review exceeds the log scan limit"
        );
        anyhow::ensure!(
            scan.conflicts.is_empty(),
            "conflicting recorded event IDs prevent re-learning review"
        );
        Ok(scan)
    }

    /// Strict, cache-independent preview of the app's derivation under
    /// `resolution`, from a fresh log scan whose revision must still be
    /// the reviewed `expected_log_revision`. It never reads `index.db`.
    pub(crate) fn relearn_projection(
        &self,
        resolution: &Resolution,
        expected_log_revision: &str,
    ) -> Result<RekeyProjection> {
        let scan = self.relearn_state()?;
        anyhow::ensure!(
            scan.revision()? == expected_log_revision,
            "log evidence changed during projection; review a fresh plan"
        );
        let derived = derive_resolved(&scan, self.adapters.as_deref(), resolution);
        project::projection(&scan, derived)
    }

    /// Latest "why I'm here" context recorded for an artifact, independent of
    /// whether any activity has created its artifact row yet. A newly
    /// recognized artifact can have saved context before its first hourly
    /// activity is delivered.
    pub fn latest_why(&self, artifact_id: &str) -> Result<Option<String>> {
        self.read(|tx| query::latest_why(tx, artifact_id))
    }

    /// One row per artifact and local day, newest day first and then by that
    /// day's measured focus. This is deliberately bounded for one native
    /// message and does not depend on FTS5.
    pub fn recent(
        &self,
        artifact_type: Option<&str>,
        title_query: Option<&str>,
        limit: usize,
    ) -> Result<(Vec<ArtifactListItem>, Vec<String>, bool)> {
        self.read(|tx| query::recent(tx, artifact_type, title_query, limit))
    }

    /// Render a measured, agent-free summary for an inclusive range of local
    /// dates. Callers validate and bound the range before reaching here.
    pub fn worklog(&self, start: Date, end: Date) -> Result<(String, usize)> {
        let (text, count, _, _) = self.worklog_report(start, end)?;
        Ok((text, count))
    }

    pub fn worklog_report(
        &self,
        start: Date,
        end: Date,
    ) -> Result<(String, usize, Vec<OverflowMarker>, bool)> {
        self.read(|tx| query::worklog(tx, start, end))
    }

    /// Run one read in a single `read_txn` over `index.db`'s committed
    /// rows, after a stat-only freshness check: only when the logs are
    /// ahead of `brauser_logs` (or the snapshot is not this folder's under
    /// these adapters) does a catch-up write run first.
    fn read<T>(&self, query: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>) -> Result<T> {
        self.read_with(|| self.reconcile_from_logs(), query)
    }

    /// [`Self::read`] with its catch-up given. If that catch-up finds the
    /// write lock busy past its timeout, the lock holder is ingesting the
    /// same appends, so a snapshot that is otherwise this folder's is
    /// served instead of an error. While a staged rebuild is pending, the
    /// history is unavailable.
    fn read_with<T>(
        &self,
        catch_up: impl FnOnce() -> Result<()>,
        query: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<T> {
        // Usually the snapshot is current: the freshness check and the
        // query then share one read transaction on one connection.
        let mut query = Some(query);
        let probe = match self.reader_connection() {
            None => Probe::Unusable,
            Some(mut conn) => {
                let outcome = db::read_txn(&mut conn, |tx| {
                    let probe = self.probe_in(tx).unwrap_or(Probe::Unusable);
                    if probe != Probe::Current {
                        return Ok((probe, None));
                    }
                    if catchup::rebuilding(tx)? {
                        bail!(INCOMPLETE_CATCH_UP);
                    }
                    let query = query.take().expect("the query runs once");
                    query(tx).map(|value| (probe, Some(value)))
                });
                match outcome {
                    Ok((_, Some(value))) => {
                        self.reader.put(conn, &self.database_path());
                        return Ok(value);
                    }
                    Ok((probe, None)) => probe,
                    // The query itself failed: that is the read's answer.
                    Err(error) if query.is_none() => return Err(error),
                    Err(_) => Probe::Unusable,
                }
            }
        };
        // The catch-up may replace or create `index.db`, so the cached
        // connection (already taken) is not reused past it.
        if let Err(error) = catch_up()
            && !(probe == Probe::Behind && busy(&error))
        {
            return Err(error);
        }
        // The catch-up succeeded; with no database it found nothing to
        // commit, and `index.db` is never created just to answer a read.
        let (mut conn, cached) = match db::open(&self.cache_dir, db::Mode::IfExists)? {
            Some(conn) => (conn, true),
            None => (db::empty()?, false),
        };
        query::register(&conn)?;
        let query = query.take().expect("the query runs once");
        let value = db::read_txn(&mut conn, |tx| {
            if catchup::rebuilding(tx)? {
                bail!(INCOMPLETE_CATCH_UP);
            }
            query(tx)
        })?;
        if cached {
            self.reader.put(conn, &self.database_path());
        }
        Ok(value)
    }

    /// This handle's read connection, opened (and its functions
    /// registered) on first use; `None` when there is no usable database.
    fn reader_connection(&self) -> Option<rusqlite::Connection> {
        if let Some(conn) = self.reader.take(&self.database_path()) {
            return Some(conn);
        }
        let conn = db::open(&self.cache_dir, db::Mode::IfExists).ok()??;
        query::register(&conn).ok()?;
        Some(conn)
    }

    /// The stat-only freshness check: no log byte or cache file is read.
    /// Any doubt (no database, a legacy file, an unreadable log folder) is
    /// [`Probe::Unusable`], and the catch-up then reports or repairs it.
    fn probe_in(&self, tx: &rusqlite::Transaction<'_>) -> Result<Probe> {
        let adapters = store::meta(tx, "adapters")?;
        if !self.location_matches(tx)?
            || adapters.as_deref() != Some(self.adapters_fingerprint().as_str())
        {
            return Ok(Probe::Unusable);
        }
        Ok(
            match catchup::freshness(tx, &self.storage, &self.root_identity)? {
                catchup::Freshness::Current => Probe::Current,
                catchup::Freshness::Behind | catchup::Freshness::Rebuilding => Probe::Behind,
            },
        )
    }

    /// Whether the store's rows were built for this folder and layout
    /// (`brauser_meta` as a rebuild's finalize records it).
    fn location_matches(&self, tx: &rusqlite::Transaction<'_>) -> Result<bool> {
        for (key, value) in [
            ("root", self.storage.root.as_str()),
            ("root_identity", self.root_identity.as_str()),
            ("log_dir", self.storage.log_dir.as_str()),
            ("artifacts_dir", self.storage.artifacts_dir.as_str()),
        ] {
            if store::meta(tx, key)?.as_deref() != Some(value) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn database_path(&self) -> PathBuf {
        self.cache_dir.join("index.db")
    }
}

fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn parse_date(value: &str) -> Result<Date> {
    let format = time::format_description::parse_borrowed::<2>("[year]-[month]-[day]")?;
    Ok(Date::parse(value, &format)?)
}

fn plain_text(value: &str) -> String {
    crate::presentation::plain_line(value, 500)
}

fn normalized_activity(event: &ActivityEvent, strip_params: &[String]) -> Result<ActivityEvent> {
    if !activity_shape_is_valid(event) {
        bail!("invalid activity cannot enter the artifact index");
    }
    let mut event = event.clone();
    event.event_id = parse_event_id(&event.event_id).map_err(anyhow::Error::msg)?;
    event.url = canonical_url(&event.url, strip_params)?;
    for mode in &mut event.modes {
        mode.url = canonical_url(&mode.url, strip_params)?;
    }
    for trail in &mut event.trails {
        if let Some(url) = &trail.from_url {
            trail.from_url = Some(canonical_url(url, strip_params)?);
        }
    }
    Ok(event)
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn read_bounded_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("checking artifact index state"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > MAX_STATE_BYTES
    {
        bail!("artifact index state is not a bounded regular file");
    }
    let bytes = fs::read(path).context("reading artifact index state")?;
    Ok(Some(
        serde_json::from_slice(&bytes).context("parsing artifact index state")?,
    ))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("cache file has no parent")?;
    fs::create_dir_all(parent).context("creating cache directory")?;
    let temporary = parent.join(format!(".{NAMESPACE}-index-{}.tmp", Uuid::new_v4()));
    let mut options = StdOpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .context("creating cache temporary")?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    if let Err(error) = fs::rename(&temporary, path) {
        let _ = fs::remove_file(&temporary);
        return Err(error.into());
    }
    sync_host_directory(parent).context("syncing cache directory")
}

#[cfg(unix)]
fn sync_host_directory(path: &Path) -> std::io::Result<()> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_host_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}
