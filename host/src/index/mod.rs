//! Rebuildable artifact index and lazily generated artifact Markdown.
//!
//! The append-only daily log is authoritative. `index-state.json` is a small
//! host cache used to avoid reparsing old logs after every delivery, while
//! `index.db` is a standards-compliant SQLite snapshot for current and future
//! readers. Either file may be deleted; `brauser reindex` recreates both.
//! A catch-up that needs more than one pass's read bound (a rebuild of a
//! long history) saves its progress in `index-catch-up.json`, which is never
//! served, and the next pass resumes from it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions as StdOpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

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

mod fold;
mod legacy;
mod ord;
mod query;
mod regen;
mod render;
mod scan;
mod snapshot;
#[cfg(test)]
mod tests;

use fold::*;
pub(crate) use fold::{unvisited_container, unvisited_within};
use legacy::*;
use query::*;
use regen::*;
use render::*;
use scan::*;
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
// How often a long regeneration pass publishes its progress. Chrome ends a
// host soon after its port closes; without checkpoints a pass killed that
// way would restart at the same sorted prefix every time.
const REGENERATION_CHECKPOINT: Duration = Duration::from_millis(500);
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

#[derive(Serialize, Deserialize)]
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
    /// Started by `reconcile_from_logs`: on completion, only artifacts whose
    /// generated file changes become dirty.
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
}

#[derive(Default)]
struct WorklogRow {
    focused: u64,
    background: u64,
    opens: u64,
    /// Focus and opens from the artifact's own visits, without those of
    /// artifacts within it, for the range's totals.
    own_focused: u64,
    own_opens: u64,
    /// The latest summary (by `occurred_at`) among this artifact's visits
    /// within the worklog's date range, if any.
    summary: Option<SummaryEvent>,
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
    fn record_events(&self, events: impl IntoIterator<Item = CachedEvent>) -> Result<()> {
        let (mut state, _) = self.load_current_state()?;
        let mut conflicts = 0usize;
        for event in events {
            let (event_id, artifact_id) = event_identity(&event);
            match state.events.get(&event_id) {
                Some(previous) if previous.data != event.data => conflicts += 1,
                Some(_) => {
                    state.dirty.insert(artifact_id);
                }
                None => {
                    state.ingest(event, None, false);
                }
            }
        }
        self.dirty_containers(&mut state);
        self.publish_state_and_database(&state)?;
        if conflicts > 0 {
            bail!("an indexed event ID cannot be reused with different content");
        }
        Ok(())
    }

    /// Reconcile a terminal replay or a newly drained legacy visit from the
    /// authoritative owned markers. In particular, never let an untrusted
    /// retry body replace the event that is already durable in the log.
    /// Only artifacts whose generated file would change become dirty, so a
    /// duplicate retry neither forces a full regeneration pass nor replaces
    /// external edits to files that had no new activity.
    ///
    /// A history larger than one pass's read bound is rescanned over several
    /// calls (or queries); until then this reports unavailable history.
    pub fn reconcile_from_logs(&self) -> Result<()> {
        let Some((mut state, previous)) = self.rescan_logs(true)? else {
            bail!(INCOMPLETE_CATCH_UP);
        };
        if let Some(previous) = previous {
            self.keep_unchanged_files_clean(&mut state, previous);
        }
        self.publish_state_and_database(&state)
    }

    /// Mark dirty only the artifacts whose generated file differs from the
    /// one `previous` derives, plus those `previous` already had dirty.
    fn keep_unchanged_files_clean(&self, state: &mut IndexState, previous: IndexState) {
        let rendered = |state: &IndexState| {
            self.derive(state)
                .artifacts
                .into_iter()
                .map(|(id, row)| (id, render_artifact(&self.storage.artifacts_dir, &row)))
                .collect::<BTreeMap<_, _>>()
        };
        let (before, after) = (rendered(&previous), rendered(state));
        let mut dirty = previous.dirty;
        for id in before.keys().chain(after.keys()) {
            if before.get(id) != after.get(id) {
                dirty.insert(id.clone());
            }
        }
        // `previous` is rendered with the current adapters too, so a
        // document change since it was published is compared separately.
        // A rescan stamps the new fingerprint before this runs, so the load
        // path's own check (`load_current_state`) would never see it.
        if previous.adapters != self.adapters_fingerprint() {
            dirty.extend(previous.containers.iter().cloned());
            dirty.extend(self.adapter_dependent_files(state));
        }
        state.dirty = dirty;
        // The files on disk are still the ones `previous` regenerated.
        state.containers = previous.containers;
        state.container_files = previous.container_files;
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

    /// Rebuild both cache files from owned log markers, then regenerate every
    /// artifact file. This is the explicit repair path for a deleted or
    /// corrupt cache and intentionally replaces user edits to owned files.
    /// A history larger than one pass's read bound is read in several
    /// bounded passes, each saving its progress, within this one call.
    pub fn reindex(&self) -> Result<RegenerationReport> {
        let (mut state, previous) = loop {
            if let Some(rescanned) = self.rescan_logs(false)? {
                break rescanned;
            }
        };
        let derived = self.derive(&state);
        state.dirty = derived.artifacts.keys().cloned().collect();
        // A rebuilt state knows no container files, so a container that no
        // longer derives a record would keep its file. Take them from the
        // published cache and from the owned files on disk.
        let mut known = previous
            .map(|previous| previous.container_files)
            .unwrap_or_default();
        for (id, path) in self.owned_rollup_files() {
            known.entry(id).or_insert(path);
        }
        adopt_vanished_container_files(&mut state, &derived, known);
        self.publish_state_and_database(&state)?;
        self.regenerate_state(state, false)
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

    fn regenerate_state(&self, state: IndexState, publish: bool) -> Result<RegenerationReport> {
        self.regenerate_state_with(state, publish, REGENERATION_CHECKPOINT, |_| {})
    }

    /// Publish the cleared dirty set at least every `checkpoint`, and before
    /// returning an error, so a pass that is killed or fails keeps its work.
    /// `publish` says the loaded state differs from the published cache.
    /// `after_file` (a test seam) sees the running count of written files.
    fn regenerate_state_with(
        &self,
        mut state: IndexState,
        publish: bool,
        checkpoint: Duration,
        mut after_file: impl FnMut(usize),
    ) -> Result<RegenerationReport> {
        let derived = self.derive(&state);
        let (root, _) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))?;
        let mut report = RegenerationReport::default();
        let dirty = state.dirty.iter().cloned().collect::<Vec<_>>();
        let mut published_at = Instant::now();
        let mut interval = checkpoint;
        // Every container this pass derives, and any earlier one whose file
        // is still dirty (not yet rewritten without its roll-up). A container
        // type's artifact counts even with only direct visits: its file is
        // titled from its ID while some adapter declares that type.
        let container_types = Containers::new(self.adapters.as_deref()).types;
        let containers = derived
            .edges
            .iter()
            .filter(|edge| edge.kind == "within")
            .map(|edge| edge.target.clone())
            .chain(
                derived
                    .artifacts
                    .values()
                    .filter(|row| container_types.contains(row.artifact_type.as_str()))
                    .map(|row| row.artifact_id.clone()),
            )
            .chain(
                state
                    .containers
                    .iter()
                    .filter(|id| state.dirty.contains(*id))
                    .cloned(),
            )
            .collect::<BTreeSet<_>>();
        // Each container's file, and, until it is removed, the file of an
        // earlier container that no longer derives a record.
        let container_files = containers
            .iter()
            .filter_map(|id| {
                derived
                    .artifacts
                    .get(id)
                    .map(|row| (id.clone(), artifact_path(&self.storage.artifacts_dir, row)))
            })
            .chain(
                state
                    .container_files
                    .iter()
                    .filter(|(id, _)| {
                        state.dirty.contains(*id) && !derived.artifacts.contains_key(*id)
                    })
                    .map(|(id, path)| (id.clone(), path.clone())),
            )
            .collect::<BTreeMap<_, _>>();
        // A caught-up or rebuilt state needs publishing even if no file does.
        let mut unpublished =
            publish || containers != state.containers || container_files != state.container_files;
        state.containers = containers;
        state.container_files = container_files;
        for id in dirty {
            let Some(artifact) = derived.artifacts.get(&id) else {
                // A container whose only activity was a roll-up the adapters
                // no longer make: its file would keep totals the index no
                // longer has, so it is removed (only if still Brauser's).
                if let Some(path) = state.container_files.get(&id).cloned() {
                    if let Err(error) = remove_artifact(&root, &path, &id) {
                        report.failed += 1;
                        let path = path.to_string_lossy().into_owned();
                        report
                            .first_failure
                            .get_or_insert_with(|| format!("{path}: {error:#}"));
                        if report.failed_paths.len() < 64 {
                            report.failed_paths.push(path);
                        }
                        continue;
                    }
                    state.container_files.remove(&id);
                }
                state.dirty.remove(&id);
                unpublished = true;
                continue;
            };
            let rendered = render_artifact(&self.storage.artifacts_dir, artifact);
            let path = || {
                artifact_path(&self.storage.artifacts_dir, artifact)
                    .to_string_lossy()
                    .into_owned()
            };
            match replace_artifact(&root, &self.storage.artifacts_dir, artifact, &rendered) {
                // A warning is post-publication work (temporary cleanup,
                // directory sync); the file is in place, so it is written and
                // not retried, as for page notes.
                Ok(OwnedWrite::Written(warnings)) => {
                    for warning in warnings {
                        let (step, error) = match warning {
                            PostPublishWarning::DirectorySync(error) => {
                                ("syncing its folder", error)
                            }
                            PostPublishWarning::TemporaryCleanup(error) => {
                                ("removing its temporary", error)
                            }
                        };
                        tracing::warn!(
                            path = %path(),
                            step,
                            %error,
                            "artifact file written, but a later step failed"
                        );
                    }
                    report.written += 1;
                    state.dirty.remove(&id);
                    unpublished = true;
                }
                Ok(OwnedWrite::Unchanged) => {
                    report.unchanged += 1;
                    state.dirty.remove(&id);
                    unpublished = true;
                }
                Ok(OwnedWrite::Conflict) => {
                    report.conflicts += 1;
                    if report.conflict_paths.len() < 64 {
                        report.conflict_paths.push(path());
                    }
                }
                // One unwritable path (a locked file, a read-only folder)
                // stays dirty for the next pass without blocking the rest.
                Err(error) => {
                    report.failed += 1;
                    if report.failed_paths.len() < 64 {
                        report.failed_paths.push(path());
                    }
                    report
                        .first_failure
                        .get_or_insert_with(|| format!("{}: {error:#}", path()));
                }
            }
            if unpublished && published_at.elapsed() >= interval {
                let started = Instant::now();
                self.publish_state_and_database(&state)?;
                published_at = Instant::now();
                interval = checkpoint_interval(checkpoint, published_at - started);
                unpublished = false;
            }
            after_file(report.written);
        }
        if unpublished {
            self.publish_state_and_database(&state)?;
        }
        self.record_pass(&derived);
        Ok(report)
    }

    /// Remember the local day this pass completed (`daily_pass_due`).
    /// Best effort: a lost marker only means one extra pass.
    fn record_pass(&self, derived: &Derived) {
        let offset = derived
            .artifacts
            .values()
            .map(|artifact| artifact.latest_at)
            .max()
            .map_or(UtcOffset::UTC, OffsetDateTime::offset);
        let marker = PassMarker {
            date: OffsetDateTime::now_utc()
                .to_offset(offset)
                .date()
                .to_string(),
            offset_seconds: offset.whole_seconds(),
            root: self.storage.root.clone(),
            root_identity: self.root_identity.clone(),
            artifacts_dir: self.storage.artifacts_dir.clone(),
        };
        if let Ok(bytes) = serde_json::to_vec(&marker)
            && fs::read(self.pass_marker_path()).ok().as_deref() != Some(bytes.as_slice())
            && let Err(error) = atomic_write(&self.pass_marker_path(), &bytes)
        {
            tracing::warn!(
                error = format!("{error:#}"),
                "could not record artifact pass"
            );
        }
    }

    fn load_or_rebuild(&self) -> Result<IndexState> {
        let (state, publish) = self.load_current_state()?;
        if publish {
            // Queries publish repaired checkpoints too. Publication failure
            // makes derived history unavailable, never silently complete.
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
            Some(previous) if reconcile => self.keep_unchanged_files_clean(&mut state, *previous),
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
        // (A reconcile above compares against the published cache instead,
        // because its rescan already carries the new fingerprint.)
        if resolution_changed {
            let dependent = self.adapter_dependent_files(&state);
            state.dirty.extend(dependent);
            let previous = std::mem::take(&mut state.containers);
            state.dirty.extend(previous);
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
        let pass = self.catch_up_logs(state, *rebuild, false)?;
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

    /// One bounded pass of a full rebuild from the logs, resuming a saved
    /// partial rebuild. Keeps the published cache's conflict winners
    /// (`catch_up_pass`) and also returns that cache. `None` means the pass
    /// reached its read bound and saved its progress.
    fn rescan_logs(&self, reconcile: bool) -> Result<Option<(IndexState, Option<IndexState>)>> {
        let mut state = match self.read_pending() {
            Some(pending) if pending.rebuild => pending.state,
            _ => self.empty_state(),
        };
        state.adapters = self.adapters_fingerprint();
        match self.catch_up_pass(&mut state, &mut true, reconcile)? {
            CatchUp::Incomplete => Ok(None),
            CatchUp::Complete { previous, .. } => Ok(Some((state, previous.map(|state| *state)))),
        }
    }

    /// Called under the native host's configuration/write lock. Limit the
    /// total bytes read in one pass as well as individual files and file count.
    /// A pass that reaches the byte bound stops before the next changed log,
    /// in sorted order, so a rebuild split over several passes ingests the
    /// logs in the same order as one pass; `state` then checkpoints only the
    /// logs it read. Callers never publish an incomplete pass.
    fn catch_up_logs(
        &self,
        state: &mut IndexState,
        rebuild: bool,
        strict: bool,
    ) -> Result<LogPass> {
        let (root, _) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))?;
        let log_dir = checked_relative_dir(&self.storage.log_dir)?;
        // Ordinary indexing skips linked history. An impact review cannot
        // present that omission as a complete inventory, including when the
        // entire logs directory is linked within the selected home folder.
        if strict
            && root
                .symlink_metadata(&log_dir)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("re-learning review cannot inspect symlinked log history");
        }
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
        collect_markdown(&logs, Path::new(""), 0, &mut files, strict)?;
        files.sort();
        let shown = |relative: &Path| log_dir.join(relative).display().to_string();
        let mut checkpoints = BTreeMap::new();
        for relative in files {
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
        let limit = scan_byte_limit();
        let mut scanned = 0;
        let mut complete = true;
        for (relative, checkpoint) in &checkpoints {
            if state.logs.get(relative) == Some(checkpoint)
                && state.gap_logs.get(relative) == Some(checkpoint)
            {
                continue;
            }
            if checkpoint.bytes > MAX_LOG_BYTES {
                bail!(
                    "artifact history is unavailable: log exceeds its 32 MiB limit: {}",
                    shown(relative)
                );
            }
            // One log always fits (the per-file bound is below the pass
            // bound), so every pass makes progress.
            if scanned > 0 && scanned + checkpoint.bytes > limit {
                complete = false;
                break;
            }
            scanned += checkpoint.bytes;
            let mut file = logs
                .open(relative)
                .with_context(|| format!("opening artifact log file {}", shown(relative)))?;
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(checkpoint.bytes + 1)
                .read_to_end(&mut bytes)
                .with_context(|| format!("reading artifact log file {}", shown(relative)))?;
            if bytes.len() as u64 != checkpoint.bytes
                || LogCheckpoint::read(&file.metadata()?)? != *checkpoint
            {
                bail!(
                    "artifact history is unavailable: {} changed during catch-up",
                    shown(relative)
                );
            }
            #[cfg(test)]
            LOG_BYTES_READ.with(|count| count.set(count.get() + bytes.len() as u64));
            // Markers are ASCII. A torn multibyte prose line (capture
            // tolerates one and keeps appending) must not drop the whole day.
            // Checkpoint every file read, a skipped non-log one included, so
            // the next pass resumes after it.
            state.logs.insert(relative.clone(), checkpoint.clone());
            state.gap_logs.insert(relative.clone(), checkpoint.clone());
            let text = String::from_utf8_lossy(&bytes);
            if !text.starts_with(LOG_HEADER) {
                continue;
            }
            for (line, event) in parse_log_lines(&text) {
                state.ingest(event, Some((relative.clone(), line)), rebuild || reset);
            }
            for (number, line) in text.lines().enumerate() {
                if let Some((id, marker)) = parse_overflow_marker(line) {
                    let position = (relative.clone(), number as u64);
                    // Retries reuse the marker ID. Conflicting copies always
                    // keep the earliest log position, including after reindex.
                    let gap = state.gaps.entry(id).or_insert_with(|| CachedGap {
                        marker: marker.clone(),
                        position: position.clone(),
                    });
                    if position < gap.position {
                        *gap = CachedGap { marker, position };
                    }
                }
            }
        }
        if complete {
            state.gap_logs = checkpoints.clone();
            state.logs = checkpoints;
        }
        Ok(LogPass {
            changed,
            reset,
            complete,
        })
    }

    fn publish_state_and_database(&self, state: &IndexState) -> Result<()> {
        #[cfg(test)]
        PUBLICATIONS.with(|count| count.set(count.get() + 1));
        fs::create_dir_all(&self.cache_dir).context("creating artifact index cache directory")?;
        let bytes = serde_json::to_vec(state).context("serializing artifact index state")?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            bail!("artifact index state exceeds its 64 MiB limit");
        }
        let database = sqlite_snapshot(&self.derive(state), &state.dirty, &state.events)?;
        if database.skipped_rows > 0 {
            // index.db is a derived snapshot; the log and the ingestion
            // cache keep the event. One bad row never stops publication.
            tracing::warn!(
                skipped_rows = database.skipped_rows,
                "left artifact index rows larger than one SQLite page out of index.db"
            );
        }
        atomic_write(&self.database_path(), &database.bytes)?;
        // Publish the checkpoint last. If either write fails, a subsequent
        // read still sees the older checkpoint and retries the catch-up.
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
    /// append-only log remains authoritative; this never trusts index.db.
    pub fn artifact(&self, artifact_id: &str) -> Result<Option<ArtifactView>> {
        Ok(self.artifact_with_summary(artifact_id)?.0)
    }

    /// The artifact's derived record and its latest summary from one
    /// catch-up, so a panel request reads the logs once. A summary always
    /// derives a row, so a missing row means no summary either.
    pub fn artifact_with_summary(
        &self,
        artifact_id: &str,
    ) -> Result<(Option<ArtifactView>, Option<SummaryView>)> {
        let derived = self.derive(&self.load_or_rebuild()?);
        let Some(row) = derived.artifacts.get(artifact_id) else {
            return Ok((None, None));
        };
        let view = artifact_view(row, &derived);
        // Rows keep summaries sorted by instant, then append order; the newest
        // that renders is the latest, so a broken one hides no earlier one.
        Ok((
            Some(view),
            row.summaries.iter().rev().find_map(summary_view),
        ))
    }

    /// The artifact's latest summary (by instant, then append order, as the
    /// artifact file's Summary line picks it) that renders as a view, or
    /// `None` when it has none.
    pub fn summary_for(&self, artifact_id: &str) -> Result<Option<SummaryView>> {
        let cache = self.load_or_rebuild()?;
        let mut summaries = ordered_events(&cache)
            .into_iter()
            .filter_map(|event| match &event.data {
                EventData::Summary(summary) if summary.artifact_id == artifact_id => Some(summary),
                _ => None,
            })
            .collect::<Vec<_>>();
        // Stable, so equal times keep `ordered_events`' log order.
        summaries.sort_by(|a, b| timestamp_order(&a.occurred_at, &b.occurred_at));
        Ok(summaries.into_iter().rev().find_map(summary_view))
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
        Ok(self
            .load_or_rebuild()?
            .events
            .values()
            .filter(|event| event.artifact_id() == artifact_id)
            .flat_map(CachedEvent::urls)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
    }

    /// Fresh authoritative evidence for impact review. No cache is trusted,
    /// published or resumed. Oversized/inconsistent history refuses the plan.
    /// Caller holds the config and capture locks, in that order.
    pub(crate) fn relearn_evidence(&self) -> Result<(String, Vec<(RecordKey, StoredUrl)>)> {
        let mut state = self.empty_state();
        let pass = self.catch_up_logs(&mut state, true, true)?;
        anyhow::ensure!(
            pass.complete,
            "re-learning review exceeds the log scan limit"
        );
        anyhow::ensure!(
            state.conflicts.is_empty(),
            "conflicting recorded event IDs prevent re-learning review"
        );
        let revision = sha256_hex(&serde_json::to_vec(&state.events)?);
        let evidence = state
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
        let state = self.load_or_rebuild()?;
        Ok(ordered_events(&state)
            .into_iter()
            .rev()
            .find_map(|event| match &event.data {
                EventData::Why(why) if why.artifact_id == artifact_id => Some(why.text.clone()),
                _ => None,
            }))
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
        let derived = self.derive(&self.load_or_rebuild()?);
        let types = derived
            .artifacts
            .values()
            .map(|row| row.artifact_type.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .take(64)
            .collect::<Vec<_>>();
        let query = title_query.map(str::trim).filter(|value| !value.is_empty());
        let query = query.map(|value| value.to_lowercase());
        let mut grouped = BTreeMap::<(String, String), ArtifactListItem>::new();
        for visit in &derived.visits {
            let Some(artifact) = derived.artifacts.get(&visit.artifact_id) else {
                continue;
            };
            if artifact_type.is_some_and(|wanted| wanted != artifact.artifact_type) {
                continue;
            }
            if query
                .as_ref()
                .is_some_and(|wanted| !artifact.title.to_lowercase().contains(wanted))
            {
                continue;
            }
            let row = grouped
                .entry((visit.local_date.clone(), visit.artifact_id.clone()))
                .or_insert_with(|| ArtifactListItem {
                    local_date: visit.local_date.clone(),
                    artifact_id: visit.artifact_id.clone(),
                    artifact_type: artifact.artifact_type.clone(),
                    title: artifact.title.clone(),
                    url: artifact.url.clone(),
                    focused_seconds: 0,
                    background_seconds: 0,
                    opens: 0,
                    why: artifact.why.clone(),
                    summary: None,
                });
            // Visits are in time order, so the day's latest summary that
            // renders wins.
            if let Some(view) = visit.summary.as_ref().and_then(summary_view) {
                row.summary = Some(view);
            }
            row.focused_seconds =
                saturating_u32(u64::from(row.focused_seconds).saturating_add(visit.focused_secs));
            row.background_seconds = saturating_u32(
                u64::from(row.background_seconds).saturating_add(visit.background_secs),
            );
            row.opens = saturating_u32(u64::from(row.opens).saturating_add(visit.opens));
        }
        let mut items = grouped.into_values().collect::<Vec<_>>();
        items.sort_by(|left, right| {
            right
                .local_date
                .cmp(&left.local_date)
                .then(right.focused_seconds.cmp(&left.focused_seconds))
                .then(left.title.cmp(&right.title))
                .then(left.artifact_id.cmp(&right.artifact_id))
        });
        let truncated = items.len() > limit;
        items.truncate(limit);
        Ok((items, types, truncated))
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
        let state = self.load_or_rebuild()?;
        let (gaps, gaps_truncated) = worklog_gaps(&state, start, end);
        let derived = self.derive(&state);
        let mut rows = BTreeMap::<String, WorklogRow>::new();
        for visit in &derived.visits {
            let Ok(date) = parse_date(&visit.local_date) else {
                continue;
            };
            if date < start || date > end {
                continue;
            }
            let row = rows.entry(visit.artifact_id.clone()).or_default();
            row.focused = row.focused.saturating_add(visit.focused_secs);
            row.background = row.background.saturating_add(visit.background_secs);
            row.opens = row.opens.saturating_add(visit.opens);
            if !visit.contained {
                row.own_focused = row.own_focused.saturating_add(visit.focused_secs);
                row.own_opens = row.own_opens.saturating_add(visit.opens);
            }
            if let Some(summary) = &visit.summary
                && row.summary.as_ref().is_none_or(|current| {
                    timestamp_order(&summary.occurred_at, &current.occurred_at).is_ge()
                })
            {
                row.summary = Some(summary.clone());
            }
        }
        // A container is still listed with its rolled-up measures, but the
        // range's totals count each visit once, by its own artifact. A
        // summary is not measured activity: it is counted on its own and
        // never as an open.
        let opened = rows.values().filter(|row| row.own_opens > 0).count();
        let carried = rows
            .values()
            .filter(|row| row.own_opens == 0 && row.own_focused > 0)
            .count();
        let summarized = rows.values().filter(|row| row.summary.is_some()).count();
        let plural = |count: usize| if count == 1 { "" } else { "s" };
        let mut text = if summarized > 0 {
            format!(
                "Opened {opened} artifact{}, focused on {carried} left open from an earlier day, and summarized {summarized} page{}.",
                plural(opened),
                plural(summarized)
            )
        } else {
            format!(
                "Opened {opened} artifact{}, and focused on {carried} left open from an earlier day.",
                plural(opened)
            )
        };
        let mut ordered = rows.into_iter().collect::<Vec<_>>();
        ordered.sort_by(|left, right| {
            right
                .1
                .focused
                .cmp(&left.1.focused)
                .then(right.1.opens.cmp(&left.1.opens))
                .then(left.0.cmp(&right.0))
        });
        let mut shown = 0usize;
        for (id, row) in &ordered {
            let Some(artifact) = derived.artifacts.get(id) else {
                continue;
            };
            let mut line = if let Some(summary) = &row.summary {
                let mut line = format!(
                    "Summarized: {} — {}, {} importance",
                    plain_text(&summary.title),
                    plain_text(&summary.document_type),
                    crate::summary::importance_str(summary.importance)
                );
                // A future adapter-attached summary (spec §7.2's "the
                // adapter does recognize the page" branch) can land on an
                // artifact that also has real measured activity; keep both
                // instead of silently dropping the measures.
                if row.focused > 0 || row.background > 0 || row.opens > 0 {
                    line.push_str(&format!(
                        " — {}",
                        measures(row.focused, row.background, row.opens)
                    ));
                }
                line
            } else {
                let mut line = format!(
                    "{} {}: {}",
                    artifact.artifact_type,
                    id,
                    plain_text(&artifact.title)
                );
                line.push_str(&format!(
                    " — {}",
                    measures(row.focused, row.background, row.opens)
                ));
                line
            };
            if let Some(why) = &artifact.why {
                line.push_str(&format!(". Why: {}", plain_text(why)));
            }
            if text.len().saturating_add(line.len()).saturating_add(1) > MAX_WORKLOG_BYTES {
                break;
            }
            text.push('\n');
            text.push_str(&line);
            shown += 1;
        }
        if shown < ordered.len() {
            text.push_str(&format!(
                "\n… {} more artifacts omitted.",
                ordered.len() - shown
            ));
        }
        Ok((text, ordered.len(), gaps, gaps_truncated))
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
