//! Rebuildable artifact index and lazily generated artifact Markdown.
//!
//! The append-only daily log is authoritative. `index-state.json` is a small
//! host cache used to avoid reparsing old logs after every delivery, while
//! `index.db` is a rusqlite database (`store`) whose `brauser_events` hold
//! every indexed event and whose derived tables serve current and future
//! readers; a delivery re-derives only the groups it touches. Either file
//! may be deleted; `brauser reindex` recreates both.
//! A rebuild (reindex, a lost cache, a removed or shrunk log) is staged in
//! `index.db` (`rebuild.rs`): each bounded pass commits its progress there,
//! the previous rows keep serving, and the next pass resumes. An incremental
//! catch-up that needs more than one pass's read bound saves its progress in
//! `index-catch-up.json`, which is never served, until plan T15.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions as StdOpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
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
mod query;
mod rebuild;
mod regen;
mod render;
mod scan;
// The hand-written snapshot writer, unused since index.db became the
// rusqlite store (plan T8); plan T15 deletes it.
#[allow(dead_code)]
mod snapshot;
mod store;
#[cfg(test)]
pub(crate) mod tests;

use fold::*;
pub(crate) use fold::{unvisited_container, unvisited_within};
use legacy::*;
#[cfg(test)]
use query::*;
use regen::*;
use render::*;
use scan::*;
#[cfg(test)]
use snapshot::*;

// v2 (main, independently) forces a rescan for the artifact-history
// recovery fix. v3 adds `EventData::Summary` on top of it; bumping again
// forces a v1 *or* v2 cache to rescan rather than silently omit summaries
// already durable in the log. v4 records log positions and the adapter
// document fingerprint. v5 counts M1 visits as opens and bounds logged
// titles. v6 gives `SummaryEvent` its record key and harness, attaches
// summaries to adapter artifacts, and dates summary Timeline lines. A v4 or
// v5 cache is migrated in place (`migrate_state`) rather than rescanned, so
// its checkpoints survive the upgrade. V7 adds per-URL stripped query names;
// v6 caches and v5/v6 pending passes keep their checkpoints with unknown
// evidence for older events, never manufactured empty lists.
// V8 indexes overflow notices. Older checkpoints skipped those markers, so
// migration retains their events but rechecks the logs once to recover gaps.
const STATE_VERSION: u8 = 8;
const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SCAN_FILES: usize = 100_000;
const MAX_WORKLOG_BYTES: usize = 96 * 1024;
const SQLITE_PAGE_SIZE: usize = 65_536;
/// The longest agent or page field rendered on one line of an artifact file.
const MAX_RENDERED_FIELD_CHARS: usize = 2048;
const LOG_HEADER: &str = concat!("<!-- ", crate::namespace!(), ":daily-log v1 -->");
const MARKER_PREFIX: &str = concat!("<!-- ", crate::namespace!(), ":visit id=");

#[cfg(test)]
thread_local! {
    static LOG_BYTES_READ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static PUBLICATIONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Derives of a whole (nonempty) history. A delivery runs none.
    static FULL_DERIVES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    static SCAN_LIMIT: std::cell::Cell<u64> = const { std::cell::Cell::new(MAX_SCAN_BYTES) };
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

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct IndexState {
    version: u8,
    root: String,
    root_identity: String,
    log_dir: String,
    artifacts_dir: String,
    events: BTreeMap<String, CachedEvent>,
    #[serde(default)]
    gaps: BTreeMap<String, CachedGap>,
    /// Kept separately so upgrading can rescan skipped overflow markers
    /// without losing the event checkpoints used to detect removed logs.
    #[serde(default)]
    gap_logs: BTreeMap<PathBuf, LogCheckpoint>,
    /// Each logged event's durable position (log path, line), which breaks
    /// ties between equal instants (context is saved with one-second
    /// resolution) the same way whichever order catch-up discovered the
    /// files in.
    positions: BTreeMap<String, (PathBuf, u64)>,
    /// Arrival order, only for events indexed before any log copy was seen.
    sequence: BTreeMap<String, u64>,
    next_sequence: u64,
    /// The adapter document index.db's resolved mentions were derived with.
    adapters: String,
    /// Conflicting bodies seen for an indexed ID during this pass, each with
    /// its earliest log position, so a rebuild can keep the winner (and its
    /// position) a valid cache already recorded.
    #[serde(skip)]
    conflicts: BTreeMap<String, Vec<Conflict>>,
    dirty: BTreeSet<String>,
    logs: BTreeMap<PathBuf, LogCheckpoint>,
    /// Containers whose generated files hold a roll-up of the visits within
    /// them, as of the last regeneration pass. An adapter change can take
    /// that roll-up away without changing what the new document derives
    /// (`within` removed, a narrowed scope, a changed child route), so
    /// these files are rewritten then too. Omitted when empty, so a cache
    /// without containers stays readable by an earlier host.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    containers: BTreeSet<String>,
    /// Where each container's generated file was written, relative to the
    /// home folder, kept until that file is removed when its container no
    /// longer derives a record (its only activity was a roll-up the adapter
    /// document no longer makes). Omitted when empty, like `containers`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    container_files: BTreeMap<String, PathBuf>,
}

#[derive(PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CachedGap {
    marker: OverflowMarker,
    position: (PathBuf, u64),
}

impl IndexState {
    /// Add one event. A reused event ID with different content keeps the
    /// body indexed first and never one chosen by an untrusted later
    /// delivery (which capture can append to an earlier-dated log), so one
    /// bad duplicate neither replaces durable history nor makes the rest of
    /// it unavailable. A full rebuild, having only the logs, keeps the copy
    /// at the earliest log position (`earliest_wins`), whichever pass read it
    /// first. Returns false on such a conflict.
    fn ingest(
        &mut self,
        event: CachedEvent,
        position: Option<(PathBuf, u64)>,
        earliest_wins: bool,
    ) -> bool {
        let (id, artifact_id) = event_identity(&event);
        let Some(previous) = self.events.get(&id) else {
            self.sequence.insert(id.clone(), self.next_sequence);
            self.next_sequence += 1;
            if let Some(position) = position {
                self.positions.insert(id.clone(), position);
            }
            self.events.insert(id, event);
            self.dirty.insert(artifact_id);
            return true;
        };
        if previous != &event {
            let earlier = earliest_wins
                && position.as_ref().is_some_and(|position| {
                    self.positions
                        .get(&id)
                        .is_some_and(|indexed| position < indexed)
                });
            let (event, position) = if earlier {
                // A rebuild resumed over several passes can read an
                // earlier-sorting log after a later one.
                let rejected_at = self.positions.insert(id.clone(), position.clone().unwrap());
                let rejected = self.events.insert(id.clone(), event).unwrap();
                self.dirty.insert(artifact_id);
                self.dirty.insert(rejected.artifact_id().to_owned());
                (rejected, rejected_at)
            } else {
                (event, position)
            };
            let chosen = &self.events[&id];
            let seen = self.conflicts.entry(id).or_default();
            seen.retain(|(body, _)| body != chosen);
            if let Some((_, earliest)) = seen.iter_mut().find(|(body, _)| body == &event) {
                if let Some(position) = position
                    && earliest.as_ref().is_none_or(|indexed| &position < indexed)
                {
                    *earliest = Some(position);
                }
            } else if seen.len() < 8 {
                seen.push((event, position));
            }
            return false;
        }
        // The same body seen at an earlier position (a file discovered
        // later) takes that position, as a fresh rebuild would.
        if let Some(position) = position
            && self
                .positions
                .get(&id)
                .is_none_or(|indexed| &position < indexed)
        {
            self.positions.insert(id, position);
        }
        true
    }
}

/// A catch-up pass that reached the per-pass read bound before reading every
/// changed log. It is saved apart from the published cache and never served,
/// so no partial checkpoint is published; the next pass resumes from it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingCatchUp {
    /// The pass started from no state (a missing cache, a removed or shrunk
    /// log, or a rescan), so completion keeps the conflict winners the
    /// published cache chose.
    rebuild: bool,
    /// Set by earlier builds, whose `reconcile_from_logs` rescanned from
    /// scratch. Reconciling is now the incremental catch-up and never saves
    /// a pass, so such a pass completes as an ordinary rebuild.
    reconcile: bool,
    state: IndexState,
    /// `IndexState::conflicts`, which the published cache does not keep.
    conflicts: BTreeMap<String, Vec<Conflict>>,
}

#[derive(Serialize)]
struct PendingCatchUpRef<'a> {
    rebuild: bool,
    reconcile: bool,
    state: &'a IndexState,
    conflicts: &'a BTreeMap<String, Vec<Conflict>>,
}

/// A rejected body for an indexed event ID and its earliest log position.
type Conflict = (CachedEvent, Option<(PathBuf, u64)>);

/// What one `catch_up_logs` pass did.
struct LogPass {
    /// Logs changed since `state`'s checkpoints.
    changed: bool,
    /// A removed or shrunk log reset `state` to empty.
    reset: bool,
    /// Every changed log was read; otherwise the read bound stopped the pass.
    complete: bool,
}

/// A bounded catch-up pass, after saving or completing it.
enum CatchUp {
    /// The pass reached its read bound; its progress is saved.
    Incomplete,
    /// Every log is read. `previous` is the published cache a rebuild kept
    /// its conflict winners from.
    Complete {
        changed: bool,
        previous: Option<Box<IndexState>>,
    },
}

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

/// The `brauser_meta` key that holds `ArtifactIndex::mirror_location` while
/// the store holds every event of the logs its watermark covers, and is
/// empty otherwise. It goes with the JSON cache (plan T15).
const MIRROR_KEY: &str = "json_mirror";

/// Whether `error` is the write lock staying busy past its timeout.
fn busy(error: &anyhow::Error) -> bool {
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

#[derive(Clone)]
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

    fn derive(&self, state: &IndexState) -> Derived {
        #[cfg(test)]
        if !state.events.is_empty() {
            FULL_DERIVES.with(|count| count.set(count.get() + 1));
        }
        derive(state, self.adapters.as_deref())
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
    /// The events go into `brauser_events` in one transaction that re-derives
    /// only the affected groups (each event's artifact and container) and
    /// marks them dirty with fresh generations; no full derive runs. Until
    /// plan T15 the JSON cache takes the same events, and the store is first
    /// brought level with it when catch-up changed it. Catch-up (delivery
    /// step 2) is the `brauser_logs` watermark's tail parse
    /// ([`Self::catch_up_logs`]), and this transaction first levels a
    /// watermark that lags the cache ([`Self::level_watermark`]).
    fn record_events(&self, events: impl IntoIterator<Item = CachedEvent>) -> Result<()> {
        let (mut state, publish) = self.load_current_state()?;
        let events = events.into_iter().collect::<Vec<_>>();
        let adapters = self.adapters.as_deref();
        let fingerprint = self.adapters_fingerprint();
        let mut conn = store::open(&self.cache_dir)?;
        let conflicts = store::write(&mut conn, adapters, |tx| {
            self.level_watermark(tx, &state)?;
            let mut remarked = if publish || !store::mirrors(tx, &state, &fingerprint)? {
                store::sync_to_state(tx, adapters, &fingerprint, &state)?
            } else {
                BTreeSet::new()
            };
            let mut containers = Containers::new(adapters);
            let mut rederive = BTreeSet::new();
            let mut conflicts = 0usize;
            for event in &events {
                let (outcome, groups) = store::ingest(tx, &mut containers, event, None)?;
                if !store::FAST_PATH && matches!(outcome, store::Ingest::Inserted) {
                    rederive.extend(groups.iter().cloned());
                }
                remarked.extend(groups);
                let (event_id, artifact_id) = event_identity(event);
                match state.events.get(&event_id) {
                    Some(previous) if previous.data != event.data => conflicts += 1,
                    Some(_) => {
                        state.dirty.insert(artifact_id);
                    }
                    None => {
                        state.ingest(event.clone(), None, false);
                    }
                }
                debug_assert_eq!(
                    matches!(outcome, store::Ingest::Conflict),
                    state
                        .events
                        .get(&event_id)
                        .is_some_and(|indexed| indexed.data != event.data)
                );
            }
            store::rederive_groups(tx, adapters, &rederive)?;
            self.dirty_containers(&mut state);
            store::sync_dirty(tx, &state.dirty, &remarked)?;
            #[cfg(test)]
            assert!(
                store::sync_to_state(tx, adapters, &fingerprint, &state)?.is_empty(),
                "the store and the JSON cache took the delivery differently"
            );
            self.mark_mirror(tx, &state)?;
            Ok(conflicts)
        })?;
        drop(conn);
        self.write_state(&state)?;
        if conflicts > 0 {
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
        let (state, publish) = self.load_current_state()?;
        if publish {
            self.publish_state_and_database(&state)
        } else {
            // Nothing to republish, but a store whose watermark lags the
            // JSON cache (one written before the watermark existed) is
            // levelled here rather than on the next change.
            self.sync_store(&state, false)
        }
    }

    /// Bring the `brauser_logs` watermark and `brauser_gaps` level with
    /// `state`'s log checkpoints when they are not: a store written before
    /// the watermark, or a pass the JSON cache made itself (its first build,
    /// a resumed or reset pass). The events come from `state` through
    /// `store::sync_to_state`, so this pass only advances the watermark.
    fn level_watermark(&self, tx: &rusqlite::Transaction<'_>, state: &IndexState) -> Result<()> {
        if catchup::checkpoints(tx)? != state.logs {
            let mut ignore =
                |_: &rusqlite::Transaction<'_>, _: CachedEvent, _: (PathBuf, u64)| Ok(());
            // A shrunk or removed log the cache has not seen yet leaves the
            // staged rebuild requested; the next catch-up runs it.
            catchup::catch_up(tx, &self.storage, &self.root_identity, &mut ignore)?;
        }
        // Once level, the watermark's gaps are exactly the cache's.
        #[cfg(test)]
        if catchup::checkpoints(tx)? == state.logs {
            assert!(
                catchup::gaps(tx)? == state.gaps,
                "brauser_gaps differs from the JSON cache's gaps"
            );
        }
        Ok(())
    }

    /// Level the watermark, then make the store mirror `state` (always with
    /// `force`, else only when its events visibly differ) and its dirty set.
    fn sync_store(&self, state: &IndexState, force: bool) -> Result<()> {
        if self.nothing_to_commit(state)? {
            return Ok(());
        }
        let adapters = self.adapters.as_deref();
        let fingerprint = self.adapters_fingerprint();
        let mut conn = store::open(&self.cache_dir)?;
        store::write(&mut conn, adapters, |tx| {
            self.level_watermark(tx, state)?;
            let changed = if force || !store::mirrors(tx, state, &fingerprint)? {
                store::sync_to_state(tx, adapters, &fingerprint, state)?
            } else {
                BTreeSet::new()
            };
            store::sync_dirty(tx, &state.dirty, &changed)?;
            self.mark_mirror(tx, state)
        })
    }

    /// `index.db` is opened (and so created) only to commit ingested data.
    /// With no database yet and nothing in `state` to store (no events,
    /// gaps, dirty marks or container files), a read, a pass or a
    /// reconciliation leaves the cache without one. Anything else at the
    /// path (a directory, a damaged or foreign file) is left to
    /// [`db::open`], which reports history unavailable without touching it.
    fn nothing_to_commit(&self, state: &IndexState) -> Result<bool> {
        if !(state.events.is_empty()
            && state.gaps.is_empty()
            && state.dirty.is_empty()
            && state.container_files.is_empty())
        {
            return Ok(false);
        }
        let path = db::db_path(&self.cache_dir);
        match db::detect_legacy(&path) {
            Ok(db::FileState::Missing) => Ok(true),
            Ok(_) => Ok(false),
            Err(error) => Err(error)
                .with_context(|| format!("artifact history is unavailable: {}", path.display())),
        }
    }

    /// Artifacts whose generated file depends on the adapter document: a
    /// container's roll-up of the visits within it, which no other
    /// derivation has. An adapter change marks only these dirty, so other
    /// owned files keep any external edits.
    fn adapter_dependent_files(&self, state: &IndexState) -> BTreeSet<String> {
        if Containers::new(self.adapters.as_deref()).types.is_empty() {
            return BTreeSet::new();
        }
        let rendered = |adapters: Option<&Document>| {
            derive(state, adapters)
                .artifacts
                .into_iter()
                .map(|(id, row)| (id, render_artifact(&self.storage.artifacts_dir, &row)))
                .collect::<BTreeMap<_, _>>()
        };
        let (plain, contained) = (rendered(None), rendered(self.adapters.as_deref()));
        contained
            .into_iter()
            .filter(|(id, markdown)| plain.get(id) != Some(markdown))
            .map(|(id, _)| id)
            .collect()
    }

    /// Mark dirty the container of every dirty artifact within one, since
    /// a visit to the child changes the container's roll-up too.
    fn dirty_containers(&self, state: &mut IndexState) {
        let mut containers = Containers::new(self.adapters.as_deref());
        if containers.types.is_empty() || state.dirty.is_empty() {
            return;
        }
        let mut found = BTreeSet::new();
        for event in state.events.values() {
            let (url, artifact_id) = match &event.data {
                EventData::Activity(event) => (&event.url, &event.artifact_id),
                EventData::Legacy(event) => (&event.url, &event.artifact_id),
                EventData::Why(_) | EventData::Summary(_) => continue,
            };
            if state.dirty.contains(artifact_id)
                && let Some((_, container, _)) = containers.of(url, artifact_id)
            {
                found.insert(container);
            }
        }
        state.dirty.extend(found);
    }

    /// Rebuild `index.db` in place from owned log markers through the staged
    /// rebuild (`rebuild.rs`), then regenerate every artifact file. This is
    /// the explicit repair path for a deleted or corrupt cache and
    /// intentionally replaces user edits to owned files. A history larger
    /// than one pass's read bound is read in several bounded passes, each
    /// committing its progress, within this one call; a rebuild already in
    /// progress (another call, or a host that stopped mid-pass) resumes.
    pub fn reindex(&self) -> Result<RegenerationReport> {
        while !self.rebuild_pass(rebuild::Finish::All)? {}
        // Vanished container files were adopted from the store and from the
        // owned roll-up files on disk; a replaced file is not Brauser's.
        let state = self.store_state()?;
        self.write_state(&state)?;
        let report = self.regenerate_state(state, false);
        self.checkpoint_wal();
        report
    }

    /// Best-effort lifecycle pass used at native-host connection boundaries.
    pub fn regenerate_dirty(&self) -> Result<RegenerationReport> {
        let (state, publish) = self.load_current_state()?;
        self.regenerate_state(state, publish)
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

    /// The JSON cache, caught up and published. Reads no longer use it
    /// (they answer from `index.db`); tests still inspect it until plan T15.
    #[cfg(test)]
    fn load_or_rebuild(&self) -> Result<IndexState> {
        let (state, publish) = self.load_current_state()?;
        if publish {
            self.publish_state_and_database(&state)?;
        }
        Ok(state)
    }

    /// The published cache caught up with the logs, resuming a saved
    /// partial pass first. A pass that reaches the read bound saves its
    /// progress and reports unavailable history.
    fn load_current_state(&self) -> Result<(IndexState, bool)> {
        let (mut state, mut rebuild, reconcile, republish) = match self.read_pending() {
            Some(pending) => (pending.state, pending.rebuild, pending.reconcile, true),
            None => match self.read_current_state() {
                Some((state, migrated)) => (state, false, false, migrated),
                None => (self.empty_state(), true, false, true),
            },
        };
        let CatchUp::Complete { changed, previous } =
            self.catch_up_pass(&mut state, &mut rebuild, reconcile)?
        else {
            bail!(INCOMPLETE_CATCH_UP);
        };
        match previous {
            // A rebuild starts with no container files; carry the cache's,
            // or, without a usable cache, find them on disk, so a container
            // that no longer derives a record still has its file removed.
            Some(previous) if rebuild => {
                let derived = self.derive(&state);
                adopt_vanished_container_files(&mut state, &derived, previous.container_files);
            }
            None if rebuild => {
                let derived = self.derive(&state);
                adopt_vanished_container_files(&mut state, &derived, self.owned_rollup_files());
            }
            _ => {}
        }
        // Ref resolution follows the current adapters, so a policy change
        // republishes index.db's edges even when no log changed.
        let adapters = self.adapters_fingerprint();
        let resolution_changed = state.adapters != adapters;
        state.adapters = adapters;
        // Containers follow the adapters too: their files regenerate when
        // the document changes, not only after the next visit (§8.3).
        // The store re-derives for the new document once and marks only
        // the files whose render changes; without a store the cache marks
        // every file that depends on the adapters at all.
        if resolution_changed {
            match self.store_adapter_change()? {
                Some(dirty) => state.dirty.extend(dirty),
                None => {
                    let dependent = self.adapter_dependent_files(&state);
                    state.dirty.extend(dependent);
                    let previous = std::mem::take(&mut state.containers);
                    state.dirty.extend(previous);
                }
            }
        }
        if republish || changed || resolution_changed {
            self.dirty_containers(&mut state);
        }
        let publish = republish || changed || resolution_changed || !self.database_path().is_file();
        Ok((state, publish))
    }

    /// Run one bounded `catch_up_logs` pass over `state`. An incomplete pass
    /// saves its progress (never served) for the next pass to resume. A
    /// complete rebuild (`rebuild`, also set when the pass reset `state`)
    /// keeps, for a reused event ID, the body the published cache chose when
    /// that body is still in the logs. Log position is not arrival order
    /// (capture can append a backdated duplicate to an earlier-dated log), so
    /// only a rebuild with no usable cache falls back to the earliest
    /// position.
    fn catch_up_pass(
        &self,
        state: &mut IndexState,
        rebuild: &mut bool,
        reconcile: bool,
    ) -> Result<CatchUp> {
        let pass = self.catch_up_logs(state, *rebuild)?;
        *rebuild |= pass.reset;
        if !pass.complete {
            self.save_pending(state, *rebuild, reconcile)?;
            return Ok(CatchUp::Incomplete);
        }
        let previous = if *rebuild {
            self.read_current_state()
                .map(|(previous, _)| Box::new(previous))
        } else {
            None
        };
        if let Some(previous) = &previous {
            for (id, seen) in std::mem::take(&mut state.conflicts) {
                let Some(winner) = previous.events.get(&id) else {
                    continue;
                };
                let Some((_, position)) = seen.into_iter().find(|(body, _)| body == winner) else {
                    continue;
                };
                // The winner's own position, so same-instant ties order as
                // in the cache rather than by the rejected body's position.
                match position {
                    Some(position) => state.positions.insert(id.clone(), position),
                    None => state.positions.remove(&id),
                };
                if let Some(chosen) = state.events.insert(id, winner.clone()) {
                    state.dirty.insert(chosen.artifact_id().to_owned());
                    state.dirty.insert(winner.artifact_id().to_owned());
                }
            }
        }
        Ok(CatchUp::Complete {
            changed: pass.changed,
            previous,
        })
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

    fn pending_path(&self) -> PathBuf {
        self.cache_dir.join("index-catch-up.json")
    }

    /// A saved partial pass for this root, layout, and cache format, if any.
    /// An unreadable one is ignored like an unreadable cache.
    fn read_pending(&self) -> Option<PendingCatchUp> {
        let mut pending: PendingCatchUp = read_bounded_json(&self.pending_path()).ok()??;
        // The progress file exists since v5. V5–v7 passes retain event
        // checkpoints but recheck their prefix for gaps; older/future ones restart.
        if !self.state_location_matches(&pending.state)
            || !matches!(pending.state.version, 5 | 6 | 7 | STATE_VERSION)
            || !migrate_state(&mut pending.state)
            || pending
                .conflicts
                .values()
                .flatten()
                .any(|(event, _)| !event.valid_provenance())
        {
            return None;
        }
        pending.state.conflicts = std::mem::take(&mut pending.conflicts);
        Some(pending)
    }

    fn save_pending(&self, state: &IndexState, rebuild: bool, reconcile: bool) -> Result<()> {
        let bytes = serde_json::to_vec(&PendingCatchUpRef {
            rebuild,
            reconcile,
            state,
            conflicts: &state.conflicts,
        })
        .context("serializing artifact index catch-up progress")?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            bail!("artifact index state exceeds its 64 MiB limit");
        }
        fs::create_dir_all(&self.cache_dir).context("creating artifact index cache directory")?;
        atomic_write(&self.pending_path(), &bytes)
    }

    /// The cache for this root and layout, if any. A version 4 or 5 cache
    /// is migrated in place; the flag reports that it must be republished.
    fn read_current_state(&self) -> Option<(IndexState, bool)> {
        let mut state = read_state(&self.state_path()).ok()??;
        if !self.state_location_matches(&state) {
            return None;
        }
        let migrated = state.version != STATE_VERSION;
        migrate_state(&mut state).then_some((state, migrated))
    }

    fn state_location_matches(&self, state: &IndexState) -> bool {
        state.root == self.storage.root
            && state.root_identity == self.root_identity
            && state.log_dir == self.storage.log_dir
            && state.artifacts_dir == self.storage.artifacts_dir
    }

    fn empty_state(&self) -> IndexState {
        IndexState {
            version: STATE_VERSION,
            root: self.storage.root.clone(),
            root_identity: self.root_identity.clone(),
            log_dir: self.storage.log_dir.clone(),
            artifacts_dir: self.storage.artifacts_dir.clone(),
            events: BTreeMap::new(),
            gaps: BTreeMap::new(),
            gap_logs: BTreeMap::new(),
            positions: BTreeMap::new(),
            sequence: BTreeMap::new(),
            next_sequence: 0,
            adapters: self.adapters_fingerprint(),
            conflicts: BTreeMap::new(),
            dirty: BTreeSet::new(),
            logs: BTreeMap::new(),
            containers: BTreeSet::new(),
            container_files: BTreeMap::new(),
        }
    }

    /// Called under the native host's configuration/write lock. Limit the
    /// total bytes read in one pass as well as individual files and file count.
    /// A pass that reaches the byte bound stops before the next changed log,
    /// in sorted order, so a rebuild split over several passes ingests the
    /// logs in the same order as one pass; `state` then checkpoints only the
    /// logs it read. Callers never publish an incomplete pass.
    ///
    /// The logs are read through the `brauser_logs` watermark
    /// (`catchup::catch_up`), whose tail parse reads only what was appended
    /// to a log. Its rows and gaps commit with each pass, and `state` takes
    /// the events it found, its checkpoints and its gaps. A watermark that
    /// does not describe `state` (a store written before the watermark, a
    /// crash between the commit and the cache's publication) is reset
    /// first, so that pass reads every log in full.
    ///
    /// A rebuild (`rebuild`, a removed or shrunk log, or a staged rebuild
    /// the store already has in progress) is instead one pass of the
    /// store's staged rebuild, which keeps the previous winner of a reused
    /// event ID by joining with `brauser_events`; once it finishes, `state`
    /// becomes the store's contents.
    fn catch_up_logs(&self, state: &mut IndexState, rebuild: bool) -> Result<LogPass> {
        let (root, _) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))?;
        let log_dir = checked_relative_dir(&self.storage.log_dir)?;
        let logs = match root.open_dir(&log_dir) {
            Ok(logs) => logs,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && state.logs.is_empty() => {
                return Ok(LogPass {
                    changed: false,
                    reset: false,
                    complete: true,
                });
            }
            Err(error) => {
                return Err(error).context("artifact history is unavailable: opening logs");
            }
        };
        let mut files = Vec::new();
        collect_markdown(&logs, Path::new(""), 0, &mut files, false)?;
        let shown = |relative: &Path| log_dir.join(relative).display().to_string();
        let mut checkpoints = BTreeMap::new();
        for relative in files {
            // The watermark never indexes such a log (it warns once per pass).
            if catchup::portable_path(&relative).is_none() {
                continue;
            }
            let metadata = logs
                .symlink_metadata(&relative)
                .with_context(|| format!("checking artifact log file {}", shown(&relative)))?;
            if !metadata.is_file() {
                bail!(
                    "artifact history is unavailable: {} is not a regular file",
                    shown(&relative)
                );
            }
            checkpoints.insert(relative, LogCheckpoint::read(&metadata)?);
        }
        // A removed/truncated file invalidates the old ingestion snapshot.
        // Rebuild from remaining authoritative logs instead of retaining ghosts.
        let reset = state.logs.iter().any(|(path, old)| {
            checkpoints
                .get(path)
                .is_none_or(|new| new.bytes < old.bytes)
        });
        if reset {
            *state = self.empty_state();
        }
        let changed = reset || state.logs != checkpoints || state.gap_logs != checkpoints;
        if !changed {
            return Ok(LogPass {
                changed,
                reset,
                complete: true,
            });
        }
        // A rebuild (no usable cache, a removed or shrunk log, or one the
        // store already has in progress) is the store's staged rebuild: each
        // call advances one bounded pass, and the finished store becomes
        // the cache. Only artifacts whose render changed are marked dirty.
        if rebuild || reset || self.store_rebuilding() {
            if !self.rebuild_pass(rebuild::Finish::Changed)? {
                bail!(INCOMPLETE_CATCH_UP);
            }
            *state = self.store_state()?;
            return Ok(LogPass {
                changed: true,
                reset: true,
                complete: true,
            });
        }
        let mut found = Vec::new();
        let mut conn = store::open(&self.cache_dir)?;
        let (complete, watermark, gaps) = db::write_txn(&mut conn, |tx| {
            if state.gap_logs != state.logs || catchup::checkpoints(tx)? != state.logs {
                catchup::reset(tx)?;
            }
            // This pass advances the watermark, but its events reach the
            // store only when the JSON cache is published.
            store::set_meta(tx, MIRROR_KEY, "")?;
            let mut sink = |_: &rusqlite::Transaction<'_>, event, position| {
                found.push((event, position));
                Ok(())
            };
            match catchup::catch_up(tx, &self.storage, &self.root_identity, &mut sink)? {
                catchup::CatchUp::Caught { complete, .. } => {
                    Ok((complete, catchup::checkpoints(tx)?, catchup::gaps(tx)?))
                }
                // A log shrank since it was stat'ed above; rolled back.
                catchup::CatchUp::Rebuilding => {
                    bail!("artifact history is unavailable: a log changed during catch-up")
                }
            }
        })?;
        drop(conn);
        for (event, position) in found {
            state.ingest(event, Some(position), false);
        }
        // Retries reuse the marker ID. Conflicting copies always keep the
        // earliest log position, including after reindex.
        for (id, found) in gaps {
            match state.gaps.get(&id) {
                Some(gap) if gap.position <= found.position => {}
                _ => {
                    state.gaps.insert(id, found);
                }
            }
        }
        state.gap_logs = watermark.clone();
        state.logs = watermark;
        Ok(LogPass {
            changed,
            reset,
            complete,
        })
    }

    /// Bring `index.db` level with `state` (re-deriving only the groups whose
    /// events changed, and mirroring its dirty set), then publish the JSON
    /// cache. Plan T15 removes the JSON half.
    fn publish_state_and_database(&self, state: &IndexState) -> Result<()> {
        #[cfg(test)]
        PUBLICATIONS.with(|count| count.set(count.get() + 1));
        self.sync_store(state, true)?;
        self.write_state(state)
    }

    /// Publish the JSON cache, last: if a write fails, a subsequent read
    /// still sees the older checkpoint and retries the catch-up.
    fn write_state(&self, state: &IndexState) -> Result<()> {
        fs::create_dir_all(&self.cache_dir).context("creating artifact index cache directory")?;
        let bytes = serde_json::to_vec(state).context("serializing artifact index state")?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            bail!("artifact index state exceeds its 64 MiB limit");
        }
        atomic_write(&self.state_path(), &bytes)?;
        // The published cache now covers any saved partial pass.
        match fs::remove_file(self.pending_path()) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).context("removing artifact index catch-up progress")
            }
            _ => Ok(()),
        }
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
        let scan = LogScan::read(&self.storage, &self.root_identity, true)?;
        anyhow::ensure!(
            scan.complete,
            "re-learning review exceeds the log scan limit"
        );
        anyhow::ensure!(
            scan.conflicts.is_empty(),
            "conflicting recorded event IDs prevent re-learning review"
        );
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
        let probe = self.probe();
        if probe != Probe::Current
            && let Err(error) = catch_up()
            && !(probe == Probe::Behind && busy(&error))
        {
            return Err(error);
        }
        let mut conn = db::open(&self.cache_dir, db::Mode::IfExists)?
            .context("artifact history is unavailable: index.db is missing")?;
        query::register(&conn)?;
        db::read_txn(&mut conn, |tx| {
            if catchup::rebuilding(tx)? {
                bail!(INCOMPLETE_CATCH_UP);
            }
            query(tx)
        })
    }

    /// The stat-only freshness check: no log byte or cache file is read.
    /// Any doubt (no database, a legacy file, an unreadable log folder) is
    /// [`Probe::Unusable`], and the catch-up then reports or repairs it.
    fn probe(&self) -> Probe {
        let probe = || -> Result<Probe> {
            let Some(mut conn) = db::open(&self.cache_dir, db::Mode::IfExists)? else {
                return Ok(Probe::Unusable);
            };
            db::read_txn(&mut conn, |tx| {
                let adapters = store::meta(tx, "adapters")?;
                if store::meta(tx, MIRROR_KEY)? != Some(self.mirror_location())
                    || adapters.as_deref() != Some(self.adapters_fingerprint().as_str())
                {
                    return Ok(Probe::Unusable);
                }
                Ok(
                    match catchup::freshness(tx, &self.storage, &self.root_identity)? {
                        catchup::Freshness::Current => Probe::Current,
                        catchup::Freshness::Behind | catchup::Freshness::Rebuilding => {
                            Probe::Behind
                        }
                    },
                )
            })
        };
        probe().unwrap_or(Probe::Unusable)
    }

    /// This store's folder and layout, as `MIRROR_KEY` records it.
    fn mirror_location(&self) -> String {
        serde_json::json!([
            self.storage.root,
            self.root_identity,
            self.storage.log_dir,
            self.storage.artifacts_dir,
        ])
        .to_string()
    }

    /// Record whether the store now holds every event of the logs its
    /// watermark covers, as `state` (the JSON cache it mirrors) does. Until
    /// plan T15, only `catch_up_logs` advances the watermark without the
    /// events, and it clears this mark.
    fn mark_mirror(&self, tx: &rusqlite::Transaction<'_>, state: &IndexState) -> Result<()> {
        let level = self.state_location_matches(state) && catchup::checkpoints(tx)? == state.logs;
        let value = if level {
            self.mirror_location()
        } else {
            String::new()
        };
        store::set_meta(tx, MIRROR_KEY, &value)
    }

    fn state_path(&self) -> PathBuf {
        self.cache_dir.join("index-state.json")
    }

    pub fn database_path(&self) -> PathBuf {
        self.cache_dir.join("index.db")
    }
}

fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

#[cfg(test)]
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

fn read_state(path: &Path) -> Result<Option<IndexState>> {
    read_bounded_json(path)
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
