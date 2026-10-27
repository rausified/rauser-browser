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
    ActivityEvent, ArtifactListItem, ArtifactView, StorageConfig, SummaryView, WhyEvent,
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
    activity_shape_is_valid, canonical_url, logged_activity_is_bounded, trail_transition_is_valid,
};
use crate::markdown::{self, yaml_string};
use crate::summary::SummaryEvent;
use crate::vault::{
    OwnedWrite, PostPublishWarning, STALE_TEMPORARY_AGE, checked_relative_dir,
    is_generated_temporary, open_selected_root, owned_frontmatter, replace_owned_file,
};

// v2 (main, independently) forces a rescan for the artifact-history
// recovery fix. v3 adds `CachedEvent::Summary` on top of it; bumping again
// forces a v1 *or* v2 cache to rescan rather than silently omit summaries
// already durable in the log. v4 records log positions and the adapter
// document fingerprint. v5 counts M1 visits as opens and bounds logged
// titles. v6 gives `SummaryEvent` its record key and harness, attaches
// summaries to adapter artifacts, and dates summary Timeline lines. A v4 or
// v5 cache is migrated in place (`migrate_state`) rather than rescanned, so
// its checkpoints survive the upgrade.
const STATE_VERSION: u8 = 6;
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
enum CachedEvent {
    Activity(ActivityEvent),
    Why(WhyEvent),
    Legacy(LegacyVisit),
    Summary(SummaryEvent),
}

impl CachedEvent {
    fn artifact_id(&self) -> &str {
        match self {
            Self::Activity(event) => &event.artifact_id,
            Self::Why(event) => &event.artifact_id,
            Self::Legacy(event) => &event.artifact_id,
            Self::Summary(event) => &event.artifact_id,
        }
    }

    /// Only URLs durably attributed to this artifact, never trail sources
    /// (which belong to another artifact). Share this evidence in both note
    /// lookup directions so primary recovery and older-note discovery agree.
    fn urls(&self) -> impl Iterator<Item = &str> {
        let (url, modes): (&str, &[brauser_protocol::ActivityMode]) = match self {
            Self::Activity(event) => (&event.url, &event.modes),
            Self::Why(event) => (&event.url, &[]),
            Self::Legacy(event) => (&event.url, &[]),
            Self::Summary(event) => (&event.url, &[]),
        };
        std::iter::once(url).chain(modes.iter().map(|mode| mode.url.as_str()))
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
    /// Set only for a visit derived from a `CachedEvent::Summary`, so
    /// `worklog` can report it distinctly instead of as a zero-measure,
    /// merely-"seen" artifact.
    summary: Option<SummaryEvent>,
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
        let event = normalized_activity(event, strip_params)?;
        self.record_events([CachedEvent::Activity(event)])
    }

    pub fn record_why(&self, event: &WhyEvent, strip_params: &[String]) -> Result<()> {
        let mut event = event.clone();
        event.url = canonical_url(&event.url, strip_params)?;
        self.record_events([CachedEvent::Why(event)])
    }

    /// Index a "Summarize & log" result already durable in the daily log
    /// (`crate::capture::CaptureStore::record_summary` wrote it). `event.url`
    /// and `event.artifact_id` must already be normalized/derived, exactly as
    /// that writer leaves them.
    pub fn record_summary(&self, event: &SummaryEvent) -> Result<()> {
        self.record_events([CachedEvent::Summary(event.clone())])
    }

    pub fn record_activities<'a>(
        &self,
        events: impl IntoIterator<Item = &'a ActivityEvent>,
        strip_params: &[String],
    ) -> Result<()> {
        let events = events
            .into_iter()
            .map(|event| normalized_activity(event, strip_params).map(CachedEvent::Activity))
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
                Some(previous) if previous != &event => conflicts += 1,
                Some(_) => {
                    state.dirty.insert(artifact_id);
                }
                None => {
                    state.ingest(event, None, false);
                }
            }
        }
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
        state.dirty = dirty;
    }

    /// Rebuild both cache files from owned log markers, then regenerate every
    /// artifact file. This is the explicit repair path for a deleted or
    /// corrupt cache and intentionally replaces user edits to owned files.
    /// A history larger than one pass's read bound is read in several
    /// bounded passes, each saving its progress, within this one call.
    pub fn reindex(&self) -> Result<RegenerationReport> {
        let (mut state, _) = loop {
            if let Some(rescanned) = self.rescan_logs(false)? {
                break rescanned;
            }
        };
        let derived = self.derive(&state);
        state.dirty = derived.artifacts.keys().cloned().collect();
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
        // A caught-up or rebuilt state needs publishing even if no file does.
        let mut unpublished = publish;
        for id in dirty {
            let Some(artifact) = derived.artifacts.get(&id) else {
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
                        eprintln!(
                            "{NAMESPACE}: warning: wrote {}, but {step} failed: {error}",
                            path()
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
            eprintln!("{NAMESPACE}: warning: could not record artifact pass: {error:#}");
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
        if reconcile && let Some(previous) = previous {
            self.keep_unchanged_files_clean(&mut state, *previous);
        }
        // Ref resolution follows the current adapters, so a policy change
        // republishes index.db's edges even when no log changed.
        let adapters = self.adapters_fingerprint();
        let resolution_changed = state.adapters != adapters;
        state.adapters = adapters;
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

    fn pending_path(&self) -> PathBuf {
        self.cache_dir.join("index-catch-up.json")
    }

    /// A saved partial pass for this root, layout, and cache format, if any.
    /// An unreadable one is ignored like an unreadable cache.
    fn read_pending(&self) -> Option<PendingCatchUp> {
        let mut pending: PendingCatchUp = read_bounded_json(&self.pending_path()).ok()??;
        // Only version 5 progress can exist from an older build (#90 added
        // the file at version 5); anything else restarts the pass.
        if !self.state_location_matches(&pending.state)
            || !matches!(pending.state.version, 5 | STATE_VERSION)
            || !migrate_state(&mut pending.state)
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
            positions: BTreeMap::new(),
            sequence: BTreeMap::new(),
            next_sequence: 0,
            adapters: self.adapters_fingerprint(),
            conflicts: BTreeMap::new(),
            dirty: BTreeSet::new(),
            logs: BTreeMap::new(),
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
        collect_markdown(&logs, Path::new(""), 0, &mut files)?;
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
        let changed = reset || state.logs != checkpoints;
        let limit = scan_byte_limit();
        let mut scanned = 0;
        let mut complete = true;
        for (relative, checkpoint) in &checkpoints {
            if state.logs.get(relative) == Some(checkpoint) {
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
            let text = String::from_utf8_lossy(&bytes);
            if !text.starts_with(LOG_HEADER) {
                continue;
            }
            for (line, event) in parse_log_lines(&text) {
                state.ingest(event, Some((relative.clone(), line)), rebuild || reset);
            }
        }
        if complete {
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
        let database = sqlite_snapshot(&self.derive(state), &state.dirty)?;
        if database.skipped_rows > 0 {
            // index.db is a derived snapshot; the log and the ingestion
            // cache keep the event. One bad row never stops publication.
            eprintln!(
                "{NAMESPACE}: warning: left {} artifact index row(s) larger than one SQLite page out of index.db",
                database.skipped_rows
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
        let view = artifact_view(
            row,
            linked_count(&derived.edges, &derived.artifacts, artifact_id),
        );
        // Rows keep summaries sorted by instant, then event ID; the newest
        // that renders is the latest, so a broken one hides no earlier one.
        Ok((
            Some(view),
            row.summaries.iter().rev().find_map(summary_view),
        ))
    }

    /// The artifact's latest summary (by instant, then event ID, as the
    /// artifact file's Summary line picks it) that renders as a view, or
    /// `None` when it has none.
    pub fn summary_for(&self, artifact_id: &str) -> Result<Option<SummaryView>> {
        let cache = self.load_or_rebuild()?;
        let mut summaries = cache
            .events
            .values()
            .filter_map(|event| match event {
                CachedEvent::Summary(summary) if summary.artifact_id == artifact_id => {
                    Some(summary)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        summaries.sort_by(|a, b| {
            timestamp_order(&a.occurred_at, &b.occurred_at).then(a.event_id.cmp(&b.event_id))
        });
        Ok(summaries.into_iter().rev().find_map(summary_view))
    }

    /// Canonical URLs previously recorded for an artifact, including mode
    /// evidence and explicit context before any measured activity. Note
    /// lookup uses these to discover URL-keyed M1 notes.
    pub fn artifact_urls(&self, artifact_id: &str) -> Result<Vec<String>> {
        Ok(self
            .load_or_rebuild()?
            .events
            .values()
            .filter(|event| event.artifact_id() == artifact_id)
            .flat_map(CachedEvent::urls)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect())
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
            .find_map(|event| match event {
                CachedEvent::Why(why) if why.artifact_id == artifact_id => Some(why.text.clone()),
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
        let derived = self.derive(&self.load_or_rebuild()?);
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
            if let Some(summary) = &visit.summary
                && row.summary.as_ref().is_none_or(|current| {
                    timestamp_order(&summary.occurred_at, &current.occurred_at).is_ge()
                })
            {
                row.summary = Some(summary.clone());
            }
        }
        // A summary is not measured activity: it is counted on its own and
        // never as an open.
        let opened = rows.values().filter(|row| row.opens > 0).count();
        let carried = rows
            .values()
            .filter(|row| row.opens == 0 && row.focused > 0)
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
        Ok((text, ordered.len()))
    }

    fn state_path(&self) -> PathBuf {
        self.cache_dir.join("index-state.json")
    }

    pub fn database_path(&self) -> PathBuf {
        self.cache_dir.join("index.db")
    }
}

/// Bring a cache (or saved catch-up progress) of an older format up to
/// `STATE_VERSION` without rereading the logs. False means it must be
/// rebuilt from the logs instead.
fn migrate_state(state: &mut IndexState) -> bool {
    match state.version {
        STATE_VERSION => return true,
        4 => migrate_v4_state(state),
        5 => {}
        _ => return false,
    }
    migrate_v5_state(state);
    true
}

/// Upgrade a version 5 cache. It holds no `CachedEvent::Summary`: #59's
/// summary events had no caller, so no log held one, and a cached event of
/// #59's shape (no record key or harness) fails to deserialize, which makes
/// the whole cache unreadable and rescanned. Version 6 changes summary
/// parse acceptance (the record key and harness, adapter artifact types)
/// and summary derivation only, so no generated file differs and nothing
/// becomes dirty. The checkpoints are kept, so a log a version 5 build
/// checkpointed after skipping a version 6 summary marker (a downgrade,
/// then an upgrade) is not reread until it changes; `brauser reindex`
/// recovers that summary.
fn migrate_v5_state(state: &mut IndexState) {
    state.version = STATE_VERSION;
}

/// Upgrade a version 4 cache to version 5: drop activity a log rescan
/// would now skip (a title over capture's limit), and mark those artifacts
/// and every M1 visit's artifact dirty so their owned files are regenerated
/// with the v5 derivation (M1 visits count as opens).
fn migrate_v4_state(state: &mut IndexState) {
    let dropped = state
        .events
        .iter()
        .filter(|(_, event)| {
            matches!(event, CachedEvent::Activity(event) if !logged_activity_is_bounded(event))
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
        if let CachedEvent::Legacy(event) = event {
            state.dirty.insert(event.artifact_id.clone());
        }
    }
    state.version = 5;
}

/// The wire view of a summary event. `summarized_at` is the event's own
/// timestamp; one longer than the wire's 32 bytes (fractional seconds in a
/// hand-edited log) is shown at whole seconds in the same offset. A
/// timestamp that still cannot fit is not shown as a summary at all, so
/// `summarized_at` is always the RFC 3339 instant the schema promises.
fn summary_view(event: &SummaryEvent) -> Option<SummaryView> {
    let summarized_at = if event.occurred_at.len() <= 32 {
        event.occurred_at.clone()
    } else {
        parsed_timestamp(&event.occurred_at)
            .and_then(|at| at.replace_nanosecond(0).ok())
            .and_then(|at| at.format(&Rfc3339).ok())
            .filter(|value| value.len() <= 32)?
    };
    Some(SummaryView {
        summarized_at,
        document_type: event.document_type.clone(),
        category: event.category.clone(),
        importance: event.importance,
        summary_path: event.summary_path.clone(),
        harness: event.harness,
    })
}

fn saturating_u32(value: u64) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn artifact_view(row: &ArtifactRow, linked: usize) -> ArtifactView {
    ArtifactView {
        artifact_id: row.artifact_id.clone(),
        artifact_type: row.artifact_type.clone(),
        title: row.title.clone(),
        url: row.url.clone(),
        first_seen: row.first_seen.clone(),
        last_seen: row.last_seen.clone(),
        focused_seconds: saturating_u32(row.focused_secs),
        background_seconds: saturating_u32(row.background_secs),
        opens: saturating_u32(row.opens),
        days_opened: saturating_u32(row.days.len() as u64),
        last_mode: row.last_mode.clone(),
        why: row.why.clone(),
        linked_count: saturating_u32(linked as u64),
    }
}

fn linked_count(
    edges: &[EdgeRow],
    artifacts: &BTreeMap<String, ArtifactRow>,
    artifact_id: &str,
) -> usize {
    let mut linked = BTreeSet::new();
    for edge in edges {
        let other = if edge.source == artifact_id {
            Some(edge.target.as_str())
        } else if edge.target == artifact_id {
            Some(edge.source.as_str())
        } else {
            None
        };
        if let Some(other) = other
            && other != artifact_id
            && artifacts.contains_key(other)
        {
            linked.insert(other);
        }
    }
    linked.len()
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

fn event_identity(event: &CachedEvent) -> (String, String) {
    match event {
        CachedEvent::Activity(event) => (event.event_id.clone(), event.artifact_id.clone()),
        CachedEvent::Why(event) => (event.event_id.clone(), event.artifact_id.clone()),
        CachedEvent::Legacy(event) => (event.marker_id.clone(), event.artifact_id.clone()),
        CachedEvent::Summary(event) => (event.event_id.clone(), event.artifact_id.clone()),
    }
}

fn event_time(event: &CachedEvent) -> OffsetDateTime {
    let at = match event {
        CachedEvent::Activity(event) => &event.occurred_at,
        CachedEvent::Why(event) => &event.occurred_at,
        CachedEvent::Legacy(event) => &event.occurred_at,
        CachedEvent::Summary(event) => &event.occurred_at,
    };
    parsed_timestamp(at).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// Events by instant, then durable log position, never by (random) event
/// ID, so the latest title, mode, and context do not depend on UUID order.
fn ordered_events(state: &IndexState) -> Vec<&CachedEvent> {
    let mut ordered = state
        .events
        .iter()
        .map(|(id, event)| {
            let position = state.positions.get(id);
            let sequence = state.sequence.get(id).copied().unwrap_or(u64::MAX);
            (
                (
                    event_time(event),
                    position.is_none(),
                    position,
                    sequence,
                    id,
                ),
                event,
            )
        })
        .collect::<Vec<_>>();
    ordered.sort_by(|left, right| left.0.cmp(&right.0));
    ordered.into_iter().map(|(_, event)| event).collect()
}

fn derive(state: &IndexState, adapters: Option<&Document>) -> Derived {
    let mut artifacts = BTreeMap::<String, ArtifactRow>::new();
    let mut visits = Vec::new();
    let mut edges = BTreeMap::<(String, String, String), EdgeRow>::new();
    let mut whys = BTreeMap::<String, (OffsetDateTime, String)>::new();
    for event in ordered_events(state) {
        match event {
            CachedEvent::Activity(event) => {
                let at = parsed_timestamp(&event.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
                let day = at.date().to_string();
                let site = Url::parse(&event.url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .unwrap_or_default();
                let title = event.title.clone().unwrap_or_else(|| "Untitled".into());
                let row = artifacts
                    .entry(event.artifact_id.clone())
                    .or_insert_with(|| ArtifactRow {
                        artifact_id: event.artifact_id.clone(),
                        artifact_type: event.artifact_type.clone(),
                        site,
                        title: title.clone(),
                        url: event.url.clone(),
                        first_seen: day.clone(),
                        last_seen: day.clone(),
                        focused_secs: 0,
                        background_secs: 0,
                        opens: 0,
                        days: BTreeSet::new(),
                        last_mode: None,
                        why: None,
                        activities: Vec::new(),
                        summaries: Vec::new(),
                        legacy_visits: Vec::new(),
                        latest_at: at,
                    });
                row.focused_secs = row
                    .focused_secs
                    .saturating_add(u64::from(event.focused_seconds));
                row.background_secs = row
                    .background_secs
                    .saturating_add(u64::from(event.background_seconds));
                row.opens = row.opens.saturating_add(u64::from(event.opens));
                if event.opens > 0 {
                    row.days.insert(day.clone());
                }
                if day < row.first_seen {
                    row.first_seen = day.clone();
                }
                if day > row.last_seen {
                    row.last_seen = day.clone();
                }
                if at >= row.latest_at {
                    row.latest_at = at;
                    if let Some(title) = &event.title {
                        row.title = title.clone();
                    }
                    row.url = event.url.clone();
                    if let Some(mode) = event.modes.last() {
                        row.last_mode = Some(mode.mode.clone());
                    }
                }
                row.activities.push(event.clone());
                visits.push(VisitRow {
                    event_id: event.event_id.clone(),
                    artifact_id: event.artifact_id.clone(),
                    occurred_at: event.occurred_at.clone(),
                    local_date: day.clone(),
                    focused_secs: u64::from(event.focused_seconds),
                    background_secs: u64::from(event.background_seconds),
                    opens: u64::from(event.opens),
                    mode: event.modes.last().map(|mode| mode.mode.clone()),
                    title,
                    url: event.url.clone(),
                    summary: None,
                });
                for trail in &event.trails {
                    if trail_transition_is_valid(&trail.transition)
                        && let Some(source) = &trail.from_artifact_id
                    {
                        add_edge(
                            &mut edges,
                            source,
                            &event.artifact_id,
                            &trail.transition,
                            &day,
                        );
                    }
                }
                let refs = event
                    .refs
                    .iter()
                    .map(|reference| Ref {
                        artifact_type: reference.artifact_type.clone(),
                        value: reference.value.clone(),
                    })
                    .collect::<Vec<_>>();
                // Without a self ID the resolver keeps one output per ref,
                // so a resolved self-reference is dropped here, not kept as
                // an unresolved key.
                let resolved = adapters.map(|document| document.resolve_refs(&refs, None));
                let mut targets = BTreeSet::new();
                for (index, reference) in refs.iter().enumerate() {
                    // An unresolved key never equals a recorded artifact ID
                    // (adapter-templates.md §10.1), so it is never shown.
                    let target = resolved
                        .as_ref()
                        .and_then(|resolved| resolved.get(index))
                        .and_then(|found| found.artifact_id.clone())
                        .unwrap_or_else(|| {
                            format!("{}:{}", reference.artifact_type, reference.value)
                        });
                    // The host checks each ref but not uniqueness; one event
                    // is one mention of each target.
                    if target != event.artifact_id {
                        targets.insert(target);
                    }
                }
                for target in targets {
                    add_edge(&mut edges, &event.artifact_id, &target, "mentions", &day);
                }
            }
            CachedEvent::Why(event) => {
                let candidate = (
                    parsed_timestamp(&event.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH),
                    event.text.clone(),
                );
                if whys
                    .get(&event.artifact_id)
                    .is_none_or(|current| candidate.0 >= current.0)
                {
                    whys.insert(event.artifact_id.clone(), candidate);
                }
            }
            CachedEvent::Legacy(event) => {
                let at = parsed_timestamp(&event.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
                let day = at.date().to_string();
                let site = Url::parse(&event.url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .unwrap_or_default();
                let row = artifacts
                    .entry(event.artifact_id.clone())
                    .or_insert_with(|| ArtifactRow {
                        artifact_id: event.artifact_id.clone(),
                        artifact_type: "web.page".into(),
                        site,
                        title: event.title.clone(),
                        url: event.url.clone(),
                        first_seen: day.clone(),
                        last_seen: day.clone(),
                        focused_secs: 0,
                        background_secs: 0,
                        opens: 0,
                        days: BTreeSet::new(),
                        last_mode: None,
                        why: None,
                        activities: Vec::new(),
                        summaries: Vec::new(),
                        legacy_visits: Vec::new(),
                        latest_at: at,
                    });
                // An M1 visit entry recorded one navigation to the page:
                // count it as one open, with no measured time.
                row.opens = row.opens.saturating_add(1);
                row.legacy_visits.push(at);
                row.days.insert(day.clone());
                if day < row.first_seen {
                    row.first_seen = day.clone();
                }
                if day > row.last_seen {
                    row.last_seen = day.clone();
                }
                if at >= row.latest_at {
                    row.latest_at = at;
                    row.title = event.title.clone();
                    row.url = event.url.clone();
                }
                visits.push(VisitRow {
                    event_id: event.marker_id.clone(),
                    artifact_id: event.artifact_id.clone(),
                    occurred_at: event.occurred_at.clone(),
                    local_date: day,
                    focused_secs: 0,
                    background_secs: 0,
                    opens: 1,
                    mode: None,
                    title: event.title.clone(),
                    url: event.url.clone(),
                    summary: None,
                });
            }
            CachedEvent::Summary(event) => {
                let at = parsed_timestamp(&event.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
                let day = at.date().to_string();
                let site = Url::parse(&event.url)
                    .ok()
                    .and_then(|url| url.host_str().map(str::to_owned))
                    .unwrap_or_default();
                let row = artifacts
                    .entry(event.artifact_id.clone())
                    .or_insert_with(|| ArtifactRow {
                        artifact_id: event.artifact_id.clone(),
                        artifact_type: event.artifact_type.clone(),
                        site,
                        title: event.title.clone(),
                        url: event.url.clone(),
                        first_seen: day.clone(),
                        last_seen: day.clone(),
                        focused_secs: 0,
                        background_secs: 0,
                        opens: 0,
                        days: BTreeSet::new(),
                        last_mode: None,
                        why: None,
                        activities: Vec::new(),
                        summaries: Vec::new(),
                        legacy_visits: Vec::new(),
                        latest_at: at,
                    });
                // A summary moves first and last seen but is not an open,
                // so it never adds to the days opened.
                if day < row.first_seen {
                    row.first_seen = day.clone();
                }
                if day > row.last_seen {
                    row.last_seen = day.clone();
                }
                if at >= row.latest_at {
                    row.latest_at = at;
                    row.title = event.title.clone();
                    row.url = event.url.clone();
                }
                row.summaries.push(event.clone());
                visits.push(VisitRow {
                    event_id: event.event_id.clone(),
                    artifact_id: event.artifact_id.clone(),
                    occurred_at: event.occurred_at.clone(),
                    local_date: day,
                    focused_secs: 0,
                    background_secs: 0,
                    opens: 0,
                    mode: None,
                    title: event.title.clone(),
                    url: event.url.clone(),
                    summary: Some(event.clone()),
                });
            }
        }
    }
    visits.sort_by(|a, b| {
        timestamp_order(&a.occurred_at, &b.occurred_at).then(a.event_id.cmp(&b.event_id))
    });
    for row in artifacts.values_mut() {
        row.activities.sort_by(|a, b| {
            timestamp_order(&a.occurred_at, &b.occurred_at).then(a.event_id.cmp(&b.event_id))
        });
        row.summaries.sort_by(|a, b| {
            timestamp_order(&a.occurred_at, &b.occurred_at).then(a.event_id.cmp(&b.event_id))
        });
        row.why = whys.remove(&row.artifact_id).map(|(_, text)| text);
    }
    Derived {
        artifacts,
        visits,
        edges: edges.into_values().collect(),
    }
}

fn add_edge(
    edges: &mut BTreeMap<(String, String, String), EdgeRow>,
    source: &str,
    target: &str,
    kind: &str,
    day: &str,
) {
    let key = (source.to_owned(), target.to_owned(), kind.to_owned());
    let row = edges.entry(key).or_insert_with(|| EdgeRow {
        source: source.into(),
        target: target.into(),
        kind: kind.into(),
        weight: 0,
        last_seen: day.into(),
    });
    row.weight = row.weight.saturating_add(1);
    if day > row.last_seen.as_str() {
        row.last_seen = day.into();
    }
}

fn parsed_timestamp(timestamp: &str) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(timestamp, &Rfc3339).ok()
}

fn timestamp_order(left: &str, right: &str) -> std::cmp::Ordering {
    match (parsed_timestamp(left), parsed_timestamp(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn collect_markdown(
    dir: &Dir,
    prefix: &Path,
    depth: usize,
    output: &mut Vec<PathBuf>,
) -> Result<()> {
    if depth > 8 {
        bail!("artifact log directory nesting exceeds its limit");
    }
    for entry in dir.entries().context("listing artifact log directory")? {
        let entry = entry.context("listing artifact log directory")?;
        let name = entry.file_name();
        let relative = prefix.join(&name);
        let metadata = dir
            .symlink_metadata(&name)
            .context("checking artifact log entry")?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            let child = dir
                .open_dir(&name)
                .context("opening artifact log subdirectory")?;
            collect_markdown(&child, &relative, depth + 1, output)?;
        } else if metadata.is_file()
            && relative.extension().and_then(|value| value.to_str()) == Some("md")
        {
            output.push(relative);
            if output.len() > MAX_SCAN_FILES {
                bail!("artifact log contains too many files to index");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
fn parse_log(text: &str) -> Vec<CachedEvent> {
    parse_log_lines(text)
        .into_iter()
        .map(|(_, event)| event)
        .collect()
}

/// Owned events with their zero-based line numbers.
fn parse_log_lines(text: &str) -> Vec<(u64, CachedEvent)> {
    let mut output = Vec::new();
    let mut previous = "";
    for (number, line) in text.lines().enumerate() {
        if let Some(event) = parse_marker_event(line, previous) {
            output.push((number as u64, event));
        }
        previous = line;
    }
    output
}

fn parse_marker_event(line: &str, previous: &str) -> Option<CachedEvent> {
    let rest = line.strip_prefix(MARKER_PREFIX)?.strip_suffix(" -->")?;
    let (id_hash, rest) = rest.split_once(" url=")?;
    let (url_hash, rest) = rest.split_once(" at=")?;
    if !valid_hash(id_hash) || !valid_hash(url_hash) {
        return None;
    }
    let mut fields = rest.split(' ');
    let at = fields.next()?;
    OffsetDateTime::parse(at, &Rfc3339).ok()?;
    let metadata = fields.collect::<Vec<_>>();
    let kind = metadata
        .iter()
        .find_map(|field| field.strip_prefix("kind="));
    let data = metadata
        .iter()
        .find_map(|field| field.strip_prefix("data="));
    match (kind, data) {
        (Some("activity"), Some(data)) => {
            let bytes = hex::decode(data).ok()?;
            if bytes.len() > 256 * 1024 {
                return None;
            }
            let event: ActivityEvent = serde_json::from_slice(&bytes).ok()?;
            if !logged_activity_is_bounded(&event)
                || Uuid::parse_str(&event.event_id).is_err()
                || event.artifact_id.is_empty()
                || event.artifact_id.len() > 512
                || event.artifact_type.is_empty()
                || event.artifact_type.len() > 64
                || !safe_page_url(&event.url)
                || event.modes.iter().any(|mode| !safe_page_url(&mode.url))
                || event
                    .trails
                    .iter()
                    .filter_map(|trail| trail.from_url.as_deref())
                    .any(|url| !safe_page_url(url))
                || sha256_hex(event.event_id.as_bytes()) != id_hash
                || sha256_hex(event.url.as_bytes()) != url_hash
                || event.occurred_at != at
            {
                return None;
            }
            Some(CachedEvent::Activity(event))
        }
        (Some("why"), Some(data)) => {
            let bytes = hex::decode(data).ok()?;
            if bytes.len() > 256 * 1024 {
                return None;
            }
            let event: WhyEvent = serde_json::from_slice(&bytes).ok()?;
            if event.artifact_id.is_empty()
                || event.artifact_id.len() > 512
                || event.text.chars().count() > 500
                || Uuid::parse_str(&event.event_id).is_err()
                || !safe_page_url(&event.url)
                || sha256_hex(event.event_id.as_bytes()) != id_hash
                || sha256_hex(event.url.as_bytes()) != url_hash
                || event.occurred_at != at
            {
                return None;
            }
            Some(CachedEvent::Why(event))
        }
        (Some("summary"), Some(data)) => {
            let bytes = hex::decode(data).ok()?;
            if bytes.len() > 256 * 1024 {
                return None;
            }
            let event: SummaryEvent = serde_json::from_slice(&bytes).ok()?;
            if event.event_id.is_empty()
                || event.event_id.len() > 512
                || !safe_page_url(&event.url)
                || sha256_hex(event.event_id.as_bytes()) != id_hash
                || sha256_hex(event.url.as_bytes()) != url_hash
                || event.occurred_at != at
                || !crate::summary::validate_summary_event(&event)
            {
                return None;
            }
            Some(CachedEvent::Summary(event))
        }
        (None, None) => parse_legacy(previous, id_hash, url_hash, at).map(CachedEvent::Legacy),
        _ => None,
    }
}

fn parse_legacy(line: &str, marker_id: &str, url_hash: &str, at: &str) -> Option<LegacyVisit> {
    let body = line.strip_prefix("- ")?;
    let (_, tail) = body.split_once(" — ")?;
    let (title, url) = tail.rsplit_once(" — <")?;
    let url = url.strip_suffix('>')?;
    let parsed = Url::parse(url).ok()?;
    if url.len() > 8192
        || title.len() > 2048
        || !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || sha256_hex(url.as_bytes()) != url_hash
    {
        return None;
    }
    let host = parsed.host_str()?;
    Some(LegacyVisit {
        marker_id: marker_id.into(),
        artifact_id: format!("web:{host}/{url_hash}"),
        occurred_at: at.into(),
        url: url.into(),
        title: unescape_markdown(title),
    })
}

fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn safe_page_url(value: &str) -> bool {
    value.len() <= 8192
        && Url::parse(value).is_ok_and(|url| {
            matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
        })
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn unescape_markdown(value: &str) -> String {
    let mut out = String::new();
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else {
            out.push(ch);
        }
    }
    if escaped {
        out.push('\\');
    }
    out
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

fn replace_artifact(
    root: &Dir,
    artifacts_dir: &str,
    artifact: &ArtifactRow,
    contents: &str,
) -> Result<OwnedWrite> {
    replace_owned_file(
        root,
        &artifact_path(artifacts_dir, artifact),
        contents,
        "artifact",
        &|existing| owned_artifact(existing, &artifact.artifact_id),
    )
}

/// Each checkpoint rewrites the whole cache, so space them by at least four
/// times the last one's cost: for a large index, checkpoints then take at
/// most a fifth of the pass instead of dominating it.
fn checkpoint_interval(minimum: Duration, last_publish: Duration) -> Duration {
    minimum.max(last_publish.saturating_mul(4))
}

/// Subdirectories of `dir`, not following symlinks.
fn subdirectories(dir: &Dir) -> Result<Vec<Dir>> {
    let mut found = Vec::new();
    for entry in dir.entries().context("listing artifacts directory")? {
        let entry = entry.context("listing artifacts directory")?;
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            found.push(entry.open_dir().context("opening artifacts directory")?);
        }
    }
    Ok(found)
}

fn sweep_temporaries_in(dir: &Dir, prefix: &str) -> Result<usize> {
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in dir.entries().context("listing temporaries")? {
        let entry = entry.context("listing temporaries")?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_generated_temporary(name, prefix) {
            continue;
        }
        // Not following symlinks: a link is not ours, whatever its name.
        let Ok(metadata) = dir.symlink_metadata(name) else {
            continue;
        };
        let stale = metadata.is_file()
            && metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified.into_std()).ok())
                .is_some_and(|elapsed| elapsed >= STALE_TEMPORARY_AGE);
        if stale && dir.remove_file(name).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

fn owned_artifact(bytes: &[u8], artifact_id: &str) -> bool {
    owned_frontmatter(bytes, "artifact", &[("artifact_id", artifact_id)], &[])
}

fn artifact_path(artifacts_dir: &str, artifact: &ArtifactRow) -> PathBuf {
    let native = artifact
        .artifact_id
        .split_once('/')
        .map_or(artifact.artifact_id.as_str(), |(_, value)| value);
    let hash = sha256_hex(artifact.artifact_id.as_bytes());
    checked_relative_dir(artifacts_dir)
        .expect("validated artifacts directory")
        .join(slug(&artifact.artifact_type, "artifact"))
        .join(slug(&artifact.site, "site"))
        .join(format!("{}-{}.md", slug(native, "item"), &hash[..12]))
}

fn slug(value: &str, fallback: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
        if out.len() >= 80 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { fallback.into() } else { out }
}

/// A link from the rendered artifact file (nested under
/// `artifacts_dir/<type>/<site>/...md`) to a vault-root-relative target,
/// e.g. a summary file.
fn relative_artifact_link(artifacts_dir: &str, artifact: &ArtifactRow, target: &str) -> String {
    let path = artifact_path(artifacts_dir, artifact);
    let from_dir = path.parent().unwrap_or(Path::new(""));
    markdown::relative_link(from_dir, Path::new(target))
}

fn render_artifact(artifacts_dir: &str, artifact: &ArtifactRow) -> String {
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!(
        "title: {}\n",
        yaml_string(&crate::presentation::plain_line(&artifact.title, 2048))
    ));
    out.push_str(&format!("url: {}\n", yaml_string(&artifact.url)));
    out.push_str("brauser:\n  kind: artifact\n");
    out.push_str(&format!(
        "  artifact_id: {}\n",
        yaml_string(&artifact.artifact_id)
    ));
    out.push_str(&format!(
        "  type: {}\n",
        yaml_string(&artifact.artifact_type)
    ));
    out.push_str(&format!("  first_seen: {}\n", artifact.first_seen));
    out.push_str(&format!("  last_seen: {}\n", artifact.last_seen));
    out.push_str(&format!(
        "  focused_min: {}\n",
        whole_minutes(artifact.focused_secs)
    ));
    out.push_str(&format!(
        "  background_min: {}\n",
        whole_minutes(artifact.background_secs)
    ));
    out.push_str(&format!("  opens: {}\n", artifact.opens));
    out.push_str(&format!("  days_opened: {}\n", artifact.days.len()));
    if let Some(mode) = &artifact.last_mode {
        out.push_str(&format!("  last_mode: {}\n", yaml_string(mode)));
    }
    out.push_str("---\n\n");
    if let Some(why) = &artifact.why {
        out.push_str(&format!(
            "Why: {}\n\n",
            markdown::inline(why, MAX_RENDERED_FIELD_CHARS)
        ));
    }
    if let Some(summary) = artifact.summaries.last() {
        let link = relative_artifact_link(artifacts_dir, artifact, &summary.summary_path);
        out.push_str(&format!(
            "Summary: [{}, {} importance](<{link}>)\n\n",
            markdown::inline(&summary.document_type, MAX_RENDERED_FIELD_CHARS),
            crate::summary::importance_str(summary.importance),
        ));
    }
    out.push_str("## Timeline\n\n");
    // Newest first: each summary line, then the day rollups whose latest
    // date is not after it, so a summary sorts before a rollup of its day.
    // Rollups and summary lines are both dated in their event's own offset,
    // so the 20 newest summaries (by instant) are merged by that local date,
    // newest instant first within a date; across offsets (travel) the lines
    // can then differ from instant order, but each stays above its own
    // day's rollup.
    const MAX_SUMMARY_LINES: usize = 20;
    let mut shown = artifact
        .summaries
        .iter()
        .rev()
        .take(MAX_SUMMARY_LINES)
        .map(|summary| (summary_instant(summary), summary))
        .collect::<Vec<_>>();
    shown.sort_by(|(left, _), (right, _)| right.date().cmp(&left.date()).then(right.cmp(left)));
    let mut summaries = shown.into_iter().map(|(_, summary)| summary).peekable();
    for period in timeline_rollups(artifact) {
        while let Some(summary) =
            summaries.next_if(|summary| summary_instant(summary).date() >= period.last_date)
        {
            out.push_str(&timeline_summary_line(summary));
        }
        out.push_str(&format!(
            "- {} · {}\n",
            period.label,
            measures(period.focused, period.background, period.opens)
        ));
    }
    for summary in summaries {
        out.push_str(&timeline_summary_line(summary));
    }
    let earlier = artifact.summaries.len().saturating_sub(MAX_SUMMARY_LINES);
    if earlier > 0 {
        out.push_str(&format!("- {earlier} earlier summaries\n"));
    }
    out
}

/// A summary's instant in its own offset. Logged events are validated
/// RFC 3339, so the epoch fallback is never reached in practice.
fn summary_instant(summary: &SummaryEvent) -> OffsetDateTime {
    parsed_timestamp(&summary.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH)
}

/// `- YYYY-MM-DD HH:MM · Summarized: <title> — <type>, <importance> importance`,
/// in the event's own offset and without seconds, so the file does not
/// churn, worded like the worklog.
fn timeline_summary_line(summary: &SummaryEvent) -> String {
    let at = summary_instant(summary);
    format!(
        "- {} {:02}:{:02} · Summarized: {} — {}, {} importance\n",
        at.date(),
        at.hour(),
        at.minute(),
        markdown::inline(&summary.title, MAX_RENDERED_FIELD_CHARS),
        markdown::inline(&summary.document_type, MAX_RENDERED_FIELD_CHARS),
        crate::summary::importance_str(summary.importance),
    )
}

fn whole_minutes(seconds: u64) -> u64 {
    seconds / 60
}

#[derive(Clone)]
struct Rollup {
    label: String,
    focused: u64,
    background: u64,
    opens: u64,
    last_date: Date,
}

/// Activity totals and M1 visit entries (one open each) by day.
fn timeline_rollups(artifact: &ArtifactRow) -> Vec<Rollup> {
    let activities = artifact.activities.iter().filter_map(|event| {
        let at = OffsetDateTime::parse(&event.occurred_at, &Rfc3339).ok()?;
        Some((
            at,
            u64::from(event.focused_seconds),
            u64::from(event.background_seconds),
            u64::from(event.opens),
        ))
    });
    let legacy = artifact.legacy_visits.iter().map(|at| (*at, 0, 0, 1));
    let mut days = BTreeMap::<Date, Rollup>::new();
    for (at, focused, background, opens) in activities.chain(legacy) {
        let date = at.date();
        let row = days.entry(date).or_insert_with(|| Rollup {
            label: date.to_string(),
            focused: 0,
            background: 0,
            opens: 0,
            last_date: date,
        });
        row.focused = row.focused.saturating_add(focused);
        row.background = row.background.saturating_add(background);
        row.opens = row.opens.saturating_add(opens);
    }
    let dates = days.keys().copied().collect::<Vec<_>>();
    let recent = dates.len().saturating_sub(14);
    let latest = dates.last().copied();
    let mut rolled = BTreeMap::<String, Rollup>::new();
    for (index, date) in dates.into_iter().enumerate() {
        let row = days.remove(&date).expect("date came from map");
        let label = if index >= recent {
            row.label.clone()
        } else if latest.is_some_and(|end| (end - date).whole_days() <= 26 * 7) {
            let (year, week, _) = date.to_iso_week_date();
            format!("{year:04}-W{week:02}")
        } else {
            format!("{:04}-{:02}", date.year(), date.month() as u8)
        };
        let target = rolled.entry(label.clone()).or_insert(Rollup {
            label,
            focused: 0,
            background: 0,
            opens: 0,
            last_date: date,
        });
        target.focused = target.focused.saturating_add(row.focused);
        target.background = target.background.saturating_add(row.background);
        target.opens = target.opens.saturating_add(row.opens);
        target.last_date = target.last_date.max(date);
    }
    let mut output = rolled.into_values().collect::<Vec<_>>();
    output.sort_by(|left, right| {
        right
            .last_date
            .cmp(&left.last_date)
            .then(right.label.cmp(&left.label))
    });
    output
}

fn measures(focused: u64, background: u64, opens: u64) -> String {
    let mut values = Vec::new();
    if focused > 0 {
        values.push(format!("focused {} min", whole_minutes(focused)));
    }
    if background > 0 {
        values.push(format!("{} min in background", whole_minutes(background)));
    }
    if opens > 0 {
        values.push(format!("opened {opens}×"));
    }
    if values.is_empty() {
        "seen".into()
    } else {
        values.join(" · ")
    }
}

#[derive(Clone)]
enum SqlValue {
    Null,
    Integer(i64),
    Text(String),
}

struct SqlTable {
    name: &'static str,
    sql: &'static str,
    rows: Vec<Vec<SqlValue>>,
}

fn sqlite_snapshot(derived: &Derived, dirty: &BTreeSet<String>) -> Result<SqliteImage> {
    let artifacts = derived
        .artifacts
        .values()
        .map(|row| {
            vec![
                SqlValue::Text(row.artifact_id.clone()),
                SqlValue::Text(row.artifact_type.clone()),
                SqlValue::Text(row.site.clone()),
                SqlValue::Text(row.title.clone()),
                SqlValue::Text(row.url.clone()),
                SqlValue::Text(row.first_seen.clone()),
                SqlValue::Text(row.last_seen.clone()),
                sql_u64(row.focused_secs),
                sql_u64(row.background_secs),
                sql_u64(row.opens),
                sql_u64(row.days.len() as u64),
                row.last_mode.clone().map_or(SqlValue::Null, SqlValue::Text),
                row.why.clone().map_or(SqlValue::Null, SqlValue::Text),
            ]
        })
        .collect();
    let visits = derived
        .visits
        .iter()
        .map(|row| {
            vec![
                SqlValue::Text(row.event_id.clone()),
                SqlValue::Text(row.artifact_id.clone()),
                SqlValue::Text(row.occurred_at.clone()),
                SqlValue::Text(row.local_date.clone()),
                sql_u64(row.focused_secs),
                sql_u64(row.background_secs),
                sql_u64(row.opens),
                row.mode.clone().map_or(SqlValue::Null, SqlValue::Text),
                SqlValue::Text(row.title.clone()),
                SqlValue::Text(row.url.clone()),
            ]
        })
        .collect();
    let edges = derived
        .edges
        .iter()
        .map(|row| {
            vec![
                SqlValue::Text(row.source.clone()),
                SqlValue::Text(row.target.clone()),
                SqlValue::Text(row.kind.clone()),
                sql_u64(row.weight),
                SqlValue::Text(row.last_seen.clone()),
            ]
        })
        .collect();
    let dirty_rows = dirty
        .iter()
        .map(|id| vec![SqlValue::Text(id.clone())])
        .collect();
    build_sqlite(&[
        SqlTable {
            name: "artifacts",
            sql: "CREATE TABLE artifacts(artifact_id TEXT,artifact_type TEXT,site TEXT,title TEXT,url TEXT,first_seen TEXT,last_seen TEXT,focused_secs INTEGER,background_secs INTEGER,opens INTEGER,days_opened INTEGER,last_mode TEXT,why TEXT)",
            rows: artifacts,
        },
        SqlTable {
            name: "visits",
            sql: "CREATE TABLE visits(event_id TEXT,artifact_id TEXT,occurred_at TEXT,local_date TEXT,focused_secs INTEGER,background_secs INTEGER,opens INTEGER,mode TEXT,title TEXT,url TEXT)",
            rows: visits,
        },
        SqlTable {
            name: "edges",
            sql: "CREATE TABLE edges(source_artifact_id TEXT,target_artifact_id TEXT,kind TEXT,weight INTEGER,last_seen TEXT)",
            rows: edges,
        },
        SqlTable {
            name: "dirty_artifacts",
            sql: "CREATE TABLE dirty_artifacts(artifact_id TEXT)",
            rows: dirty_rows,
        },
    ])
}

fn sql_u64(value: u64) -> SqlValue {
    SqlValue::Integer(i64::try_from(value).unwrap_or(i64::MAX))
}

/// SQLite stores a table-leaf payload of up to U-35 bytes (U is the usable
/// page size: the whole page, as no bytes are reserved) on the leaf itself.
/// A larger payload must spill to overflow pages, which this writer does
/// not emit, so such a row is left out instead of written unreadably.
const SQLITE_MAX_LOCAL_PAYLOAD: usize = SQLITE_PAGE_SIZE - 35;

struct SqliteImage {
    bytes: Vec<u8>,
    /// Rows left out because their payload would need overflow pages.
    skipped_rows: usize,
}

/// Emit the small subset of the SQLite file format this append-derived
/// snapshot needs: schema and rowid table B-trees, without secondary indexes
/// or overflow records. A 64 KiB page keeps every bounded event row local;
/// a row over the local payload limit is skipped and counted.
fn build_sqlite(tables: &[SqlTable]) -> Result<SqliteImage> {
    let root_count = tables.len() + 1;
    let mut pages = vec![vec![0u8; SQLITE_PAGE_SIZE]; root_count];
    let mut next_page = root_count + 1;
    let mut skipped_rows = 0;
    for (table_index, table) in tables.iter().enumerate() {
        let root_page = table_index + 2;
        let mut cells = Vec::with_capacity(table.rows.len());
        for (index, row) in table.rows.iter().enumerate() {
            match table_leaf_cell((index + 1) as u64, row)? {
                Some(cell) => cells.push(cell),
                None => skipped_rows += 1,
            }
        }
        let chunks = partition_cells(&cells, 8)?;
        if chunks.len() <= 1 {
            write_leaf_page(
                &mut pages[root_page - 1],
                0,
                chunks.first().map(Vec::as_slice).unwrap_or(&[]),
            )?;
            continue;
        }
        let mut children = Vec::new();
        for chunk in chunks {
            let page_number = next_page;
            next_page += 1;
            let mut page = vec![0u8; SQLITE_PAGE_SIZE];
            write_leaf_page(&mut page, 0, &chunk)?;
            let max_rowid = decode_cell_rowid(chunk.last().expect("nonempty leaf chunk"))?;
            children.push((page_number as u32, max_rowid));
            pages.push(page);
        }
        write_interior_page(&mut pages[root_page - 1], &children)?;
    }

    let schema_rows = tables
        .iter()
        .enumerate()
        .map(|(index, table)| {
            vec![
                SqlValue::Text("table".into()),
                SqlValue::Text(table.name.into()),
                SqlValue::Text(table.name.into()),
                SqlValue::Integer((index + 2) as i64),
                SqlValue::Text(table.sql.into()),
            ]
        })
        .collect::<Vec<_>>();
    let schema_cells = schema_rows
        .iter()
        .enumerate()
        .map(|(index, row)| {
            table_leaf_cell((index + 1) as u64, row)?
                .context("SQLite schema row exceeds the local payload limit")
        })
        .collect::<Result<Vec<_>>>()?;
    write_leaf_page(&mut pages[0], 100, &schema_cells)?;
    let page_count = pages.len() as u32;
    write_database_header(&mut pages[0], page_count);
    Ok(SqliteImage {
        bytes: pages.into_iter().flatten().collect(),
        skipped_rows,
    })
}

fn partition_cells(cells: &[Vec<u8>], header: usize) -> Result<Vec<Vec<Vec<u8>>>> {
    if cells.is_empty() {
        return Ok(vec![Vec::new()]);
    }
    let mut chunks = Vec::<Vec<Vec<u8>>>::new();
    let mut current = Vec::new();
    let mut used = header;
    for cell in cells {
        if cell.len() + header + 2 > SQLITE_PAGE_SIZE {
            bail!("artifact index row exceeds one SQLite page");
        }
        if !current.is_empty() && used + cell.len() + 2 > SQLITE_PAGE_SIZE {
            chunks.push(current);
            current = Vec::new();
            used = header;
        }
        used += cell.len() + 2;
        current.push(cell.clone());
    }
    chunks.push(current);
    Ok(chunks)
}

fn write_database_header(page: &mut [u8], pages: u32) {
    page[..16].copy_from_slice(b"SQLite format 3\0");
    page[16..18].copy_from_slice(&1u16.to_be_bytes()); // 1 means 65536 bytes.
    page[18] = 1;
    page[19] = 1;
    page[20] = 0;
    page[21] = 64;
    page[22] = 32;
    page[23] = 32;
    page[24..28].copy_from_slice(&1u32.to_be_bytes());
    page[28..32].copy_from_slice(&pages.to_be_bytes());
    page[40..44].copy_from_slice(&1u32.to_be_bytes());
    page[44..48].copy_from_slice(&4u32.to_be_bytes());
    page[56..60].copy_from_slice(&1u32.to_be_bytes());
    page[68..72].copy_from_slice(&0x4252_5752u32.to_be_bytes()); // BRWR
    page[92..96].copy_from_slice(&1u32.to_be_bytes());
    page[96..100].copy_from_slice(&3_045_000u32.to_be_bytes());
}

fn write_leaf_page(page: &mut [u8], offset: usize, cells: &[Vec<u8>]) -> Result<()> {
    page[offset] = 0x0d;
    page[offset + 1..offset + 3].copy_from_slice(&0u16.to_be_bytes());
    page[offset + 3..offset + 5].copy_from_slice(&(cells.len() as u16).to_be_bytes());
    let mut content = SQLITE_PAGE_SIZE;
    for (index, cell) in cells.iter().enumerate() {
        content = content
            .checked_sub(cell.len())
            .context("SQLite leaf page overflow")?;
        let pointer = offset + 8 + index * 2;
        if pointer + 2 > content {
            bail!("SQLite leaf page pointer overflow");
        }
        page[content..content + cell.len()].copy_from_slice(cell);
        page[pointer..pointer + 2].copy_from_slice(&(content as u16).to_be_bytes());
    }
    let encoded = if content == SQLITE_PAGE_SIZE {
        0
    } else {
        content as u16
    };
    page[offset + 5..offset + 7].copy_from_slice(&encoded.to_be_bytes());
    page[offset + 7] = 0;
    Ok(())
}

fn write_interior_page(page: &mut [u8], children: &[(u32, u64)]) -> Result<()> {
    if children.len() < 2 {
        bail!("SQLite interior page needs at least two children");
    }
    page[0] = 0x05;
    page[1..3].copy_from_slice(&0u16.to_be_bytes());
    page[3..5].copy_from_slice(&((children.len() - 1) as u16).to_be_bytes());
    page[8..12].copy_from_slice(&children.last().expect("children checked").0.to_be_bytes());
    let mut content = SQLITE_PAGE_SIZE;
    for (index, (child, max_rowid)) in children[..children.len() - 1].iter().enumerate() {
        let mut cell = child.to_be_bytes().to_vec();
        cell.extend(sqlite_varint(*max_rowid));
        content = content
            .checked_sub(cell.len())
            .context("SQLite interior page overflow")?;
        let pointer = 12 + index * 2;
        if pointer + 2 > content {
            bail!("SQLite interior page pointer overflow");
        }
        page[content..content + cell.len()].copy_from_slice(&cell);
        page[pointer..pointer + 2].copy_from_slice(&(content as u16).to_be_bytes());
    }
    page[5..7].copy_from_slice(&(content as u16).to_be_bytes());
    page[7] = 0;
    Ok(())
}

/// One table-leaf cell, or `None` when its payload is too large to keep
/// local (`SQLITE_MAX_LOCAL_PAYLOAD`).
fn table_leaf_cell(rowid: u64, values: &[SqlValue]) -> Result<Option<Vec<u8>>> {
    let payload = sqlite_record(values)?;
    if payload.len() > SQLITE_MAX_LOCAL_PAYLOAD {
        return Ok(None);
    }
    let mut cell = sqlite_varint(payload.len() as u64);
    cell.extend(sqlite_varint(rowid));
    cell.extend(payload);
    Ok(Some(cell))
}

fn decode_cell_rowid(cell: &[u8]) -> Result<u64> {
    let (_, first) = read_sqlite_varint(cell)?;
    let (rowid, _) = read_sqlite_varint(&cell[first..])?;
    Ok(rowid)
}

fn sqlite_record(values: &[SqlValue]) -> Result<Vec<u8>> {
    let mut serial = Vec::new();
    let mut body = Vec::new();
    for value in values {
        match value {
            SqlValue::Null => serial.extend(sqlite_varint(0)),
            SqlValue::Integer(value) => {
                serial.extend(sqlite_varint(6));
                body.extend(value.to_be_bytes());
            }
            SqlValue::Text(value) => {
                let bytes = value.as_bytes();
                serial.extend(sqlite_varint(13 + 2 * bytes.len() as u64));
                body.extend(bytes);
            }
        }
    }
    let mut header_len = serial.len() + 1;
    loop {
        let encoded = sqlite_varint(header_len as u64);
        let next = serial.len() + encoded.len();
        if next == header_len {
            let mut record = encoded;
            record.extend(serial);
            record.extend(body);
            return Ok(record);
        }
        header_len = next;
    }
}

fn sqlite_varint(value: u64) -> Vec<u8> {
    if value <= 0x7f {
        return vec![value as u8];
    }
    if value > 0x00ff_ffff_ffff_ffff {
        let mut out = vec![0u8; 9];
        out[8] = value as u8;
        let mut shifted = value >> 8;
        for index in (0..8).rev() {
            out[index] = ((shifted & 0x7f) as u8) | 0x80;
            shifted >>= 7;
        }
        return out;
    }
    let bits = 64 - value.leading_zeros() as usize;
    let count = bits.div_ceil(7);
    let mut out = vec![0u8; count];
    for (index, byte) in out.iter_mut().enumerate() {
        let shift = (count - index - 1) * 7;
        *byte = ((value >> shift) & 0x7f) as u8;
        if index + 1 != count {
            *byte |= 0x80;
        }
    }
    out
}

fn read_sqlite_varint(bytes: &[u8]) -> Result<(u64, usize)> {
    let mut value = 0u64;
    for (index, byte) in bytes.iter().copied().take(9).enumerate() {
        if index == 8 {
            return Ok(((value << 8) | u64::from(byte), 9));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Ok((value, index + 1));
        }
    }
    bail!("invalid SQLite varint")
}

#[cfg(test)]
mod tests {
    use super::*;
    use brauser_protocol::{ActivityMode, ActivityRef, ActivityTrail};
    use std::fs::File;
    use std::process::Command;
    use tempfile::TempDir;

    fn storage(root: &Path) -> StorageConfig {
        StorageConfig {
            root: root.to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: "log".into(),
            pages_dir: "pages".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: None,
            knowledge_base_dir: None,
        }
    }

    fn activity(id: &str, native_id: &str, title: &str, at: &str) -> ActivityEvent {
        ActivityEvent {
            event_id: id.into(),
            artifact_id: format!("work:example.com/{native_id}"),
            artifact_type: "work.item".into(),
            url: format!("https://example.com/items/{native_id}?utm_source=test"),
            title: Some(title.into()),
            occurred_at: at.into(),
            incognito: false,
            opens: 1,
            focused_seconds: 125,
            background_seconds: 61,
            modes: vec![ActivityMode {
                mode: "view".into(),
                url: format!("https://example.com/items/{native_id}"),
            }],
            trails: Vec::<ActivityTrail>::new(),
            refs: Vec::<ActivityRef>::new(),
        }
    }

    fn index(root: &TempDir, cache: &TempDir) -> ArtifactIndex {
        ArtifactIndex::open_in(&storage(root.path()), None, cache.path()).unwrap()
    }

    #[test]
    fn hostile_presentation_is_safe_in_logs_and_rebuilt_artifacts_without_changing_payloads() {
        use crate::capture::{CaptureBatchEvent, CaptureOutcome, CapturePolicy, CaptureStore};
        use brauser_protocol::VisitEvent;

        let hostile = "Report\u{202e}evil\u{202c}\u{200b}\u{200f}\u{2066}hidden\u{2069}\u{feff} %%comment%% ==marked== #tag ![embed](https://bad.invalid)";
        for batched in [false, true] {
            let root = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let config = storage(root.path());
            let capture = CaptureStore::open_in(&config, None, cache.path()).unwrap();
            let origins = ["https://example.com".to_owned()];
            let policy = || CapturePolicy {
                enabled: true,
                origins: &origins,
                strip_params: &[],
            };
            let event = activity(
                "00000000-0000-4000-8000-000000000001",
                "ABC",
                hostile,
                &time::OffsetDateTime::now_utc()
                    .replace_minute(0)
                    .unwrap()
                    .replace_second(0)
                    .unwrap()
                    .replace_nanosecond(0)
                    .unwrap()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap(),
            );
            let visit = VisitEvent {
                event_id: "00000000-0000-4000-8000-000000000002".into(),
                url: "https://example.com/old".into(),
                title: Some(hostile.into()),
                occurred_at: event.occurred_at.clone(),
                incognito: false,
            };
            if batched {
                let outcome = capture.record_batch(
                    policy(),
                    "00000000-0000-4000-8000-000000000004",
                    &[
                        CaptureBatchEvent::Activity(&event),
                        CaptureBatchEvent::Visit(&visit),
                    ],
                    None,
                );
                assert_eq!(outcome.events.len(), 2);
                assert!(
                    outcome
                        .events
                        .iter()
                        .all(|(_, result)| matches!(result, CaptureOutcome::Persisted { .. }))
                );
            } else {
                assert!(matches!(
                    capture.record_activity(policy(), &event),
                    CaptureOutcome::Persisted { .. }
                ));
                assert!(matches!(
                    capture.record(policy(), &visit),
                    CaptureOutcome::Persisted { .. }
                ));
            }
            let why = WhyEvent {
                event_id: "00000000-0000-4000-8000-000000000003".into(),
                artifact_id: event.artifact_id.clone(),
                url: event.url.clone(),
                occurred_at: event.occurred_at.clone(),
                incognito: false,
                text: hostile.into(),
            };
            assert!(matches!(
                capture.record_why(policy(), &why),
                CaptureOutcome::Persisted { .. }
            ));
            let day = &event.occurred_at[..10];
            let log = root
                .path()
                .join(format!("log/{}/{}/{day}.md", &day[..4], &day[5..7]));
            let original = fs::read_to_string(&log).unwrap();
            for (kind, payload) in [
                ("activity", serde_json::to_value(&event).unwrap()),
                ("why", serde_json::to_value(&why).unwrap()),
            ] {
                let expected = hex::encode(serde_json::to_vec(&payload).unwrap());
                assert!(original.contains(&format!(" kind={kind} data={expected} -->")));
            }
            let rows = original.lines().filter(|line| line.starts_with("- "));
            assert_eq!(rows.clone().count(), 3);
            for row in rows {
                assert!(
                    !row.contains(['\u{202e}', '\u{200b}', '\u{200f}', '\u{2066}', '\u{feff}'])
                );
                assert!(row.contains(r"\%\%comment\%\% \=\=marked\=\= \#tag \!\[embed\]"));
            }

            // Rebuild from the actual markers, not a second sanitized event.
            let decoded = parse_log(&original);
            assert_eq!(decoded.len(), 3);
            assert!(decoded.iter().any(|stored| matches!(stored,
                CachedEvent::Activity(value) if serde_json::to_value(value).unwrap() == serde_json::to_value(&event).unwrap()
            )));
            assert!(decoded.iter().any(|stored| matches!(stored,
                CachedEvent::Why(value) if serde_json::to_value(value).unwrap() == serde_json::to_value(&why).unwrap()
            )));
            let index = index(&root, &cache);
            assert_eq!(index.reindex().unwrap().written, 2);
            for path in artifact_files(root.path()) {
                let markdown = fs::read_to_string(path).unwrap();
                assert!(
                    !markdown
                        .contains(['\u{202e}', '\u{200b}', '\u{200f}', '\u{2066}', '\u{feff}'])
                );
                assert!(markdown.contains("Report"));
            }
            assert_eq!(fs::read_to_string(&log).unwrap(), original);
            assert_eq!(index.reindex().unwrap().unchanged, 2);
        }
    }

    #[test]
    fn artifact_files_and_worklogs_neutralize_unicode_formatting() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let hostile = "Visible\u{202e}spoof\u{202c}\u{200b}\u{2069}\u{feff} end";
        let event = activity(
            "00000000-0000-4000-8000-000000000001",
            "ABC",
            hostile,
            "2026-10-15T12:00:00Z",
        );
        index.record_activity(&event, &[]).unwrap();
        index
            .record_why(
                &WhyEvent {
                    event_id: "00000000-0000-4000-8000-000000000002".into(),
                    artifact_id: event.artifact_id.clone(),
                    url: event.url.clone(),
                    occurred_at: event.occurred_at.clone(),
                    incognito: false,
                    text: format!("{hostile} %%hidden%% ==mark== #tag"),
                },
                &[],
            )
            .unwrap();
        assert_eq!(index.regenerate_dirty().unwrap().written, 1);
        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert!(!markdown.contains(['\u{202e}', '\u{202c}', '\u{200b}', '\u{2069}', '\u{feff}']));
        assert!(markdown.contains(r"\%\%hidden\%\% \=\=mark\=\= \#tag"));
        let (worklog, _) = index
            .worklog(
                parse_date("2026-10-15").unwrap(),
                parse_date("2026-10-15").unwrap(),
            )
            .unwrap();
        assert!(!worklog.contains(['\u{202e}', '\u{202c}', '\u{200b}', '\u{2069}', '\u{feff}']));
        assert!(worklog.contains("Visible spoof"));
    }

    #[test]
    fn index_db_has_queryable_tables_and_capture_does_not_touch_artifact_files() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let hostile = "---\n![fetch](https://attacker.invalid/x) [break]";
        index
            .record_activity(
                &activity(
                    "00000000-0000-4000-8000-000000000001",
                    "ABC",
                    hostile,
                    "2026-10-15T12:00:00+00:00",
                ),
                &["utm_*".into()],
            )
            .unwrap();
        assert!(!root.path().join("artifacts").exists());

        let script = r#"import json, sqlite3, sys
c=sqlite3.connect(sys.argv[1])
tables=[r[0] for r in c.execute("select name from sqlite_schema where type='table' order by name")]
row=c.execute("select title, url from artifacts").fetchone()
print(json.dumps([tables,row]))"#;
        let output = Command::new("python3")
            .args(["-c", script])
            .arg(index.database_path())
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let decoded: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            decoded[0],
            serde_json::json!(["artifacts", "dirty_artifacts", "edges", "visits"])
        );
        assert_eq!(decoded[1][0], hostile);
        assert_eq!(decoded[1][1], "https://example.com/items/ABC");
    }

    #[test]
    fn artifact_urls_include_mode_evidence_urls() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let edit_url = "https://docs.example/d/abc/edit";
        let view_url = "https://docs.example/d/abc/view";
        let event = ActivityEvent {
            event_id: "00000000-0000-4000-8000-000000000003".into(),
            artifact_id: "gdoc:docs.example/abc".into(),
            artifact_type: "gdoc".into(),
            url: view_url.into(),
            title: Some("Doc".into()),
            occurred_at: "2026-10-16T12:00:00+00:00".into(),
            incognito: false,
            opens: 1,
            focused_seconds: 30,
            background_seconds: 0,
            modes: vec![ActivityMode {
                mode: "edit".into(),
                url: edit_url.into(),
            }],
            trails: Vec::new(),
            refs: Vec::new(),
        };
        index.record_activity(&event, &[]).unwrap();

        assert_eq!(
            index.artifact_urls("gdoc:docs.example/abc").unwrap(),
            vec![edit_url.to_string(), view_url.to_string()]
        );
    }

    #[test]
    fn recent_and_worklog_are_measured_bounded_views_of_the_index() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let first = activity(
            "00000000-0000-4000-8000-000000000001",
            "ABC",
            "Release blocker",
            "2026-10-15T12:00:00-04:00",
        );
        let second = activity(
            "00000000-0000-4000-8000-000000000002",
            "XYZ",
            "Design notes",
            "2026-10-16T09:00:00-04:00",
        );
        index.record_activities([&first, &second], &[]).unwrap();
        index
            .record_why(
                &WhyEvent {
                    event_id: "00000000-0000-4000-8000-000000000003".into(),
                    artifact_id: first.artifact_id.clone(),
                    url: first.url.clone(),
                    occurred_at: "2026-10-16T10:00:00-04:00".into(),
                    incognito: false,
                    text: "blocking the release".into(),
                },
                &[],
            )
            .unwrap();

        let (recent, types, truncated) = index.recent(None, Some("release"), 200).unwrap();
        assert_eq!(types, ["work.item"]);
        assert!(!truncated);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].local_date, "2026-10-15");
        assert_eq!(recent[0].focused_seconds, 125);
        assert_eq!(recent[0].why.as_deref(), Some("blocking the release"));

        let (text, count) = index
            .worklog(
                parse_date("2026-10-15").unwrap(),
                parse_date("2026-10-16").unwrap(),
            )
            .unwrap();
        assert_eq!(count, 2);
        assert!(text.starts_with("Opened 2 artifacts"));
        assert!(text.contains("Release blocker"));
        assert!(text.contains("Why: blocking the release"));
    }

    #[test]
    fn linked_artifacts_are_distinct_and_unresolved_mentions_do_not_count() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let first = activity(
            "00000000-0000-4000-8000-000000000011",
            "ABC",
            "Source",
            "2026-10-15T12:00:00Z",
        );
        let mut second = activity(
            "00000000-0000-4000-8000-000000000012",
            "XYZ",
            "Target",
            "2026-10-15T13:00:00Z",
        );
        second.trails = ["link", "form_submit"]
            .into_iter()
            .map(|transition| ActivityTrail {
                from_artifact_id: Some(first.artifact_id.clone()),
                from_url: Some(first.url.clone()),
                transition: transition.into(),
            })
            .collect();
        second.refs.push(ActivityRef {
            artifact_type: "work.item".into(),
            value: "NOT-RECORDED".into(),
        });
        index.record_activities([&first, &second], &[]).unwrap();

        assert_eq!(
            index
                .artifact(&first.artifact_id)
                .unwrap()
                .unwrap()
                .linked_count,
            1
        );
        assert_eq!(
            index
                .artifact(&second.artifact_id)
                .unwrap()
                .unwrap()
                .linked_count,
            1
        );
    }

    #[test]
    fn case_collisions_external_edits_and_unchanged_files_are_handled() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let first = activity(
            "00000000-0000-4000-8000-000000000001",
            "AbC",
            "![x](https://bad.invalid)",
            "2026-10-15T12:00:00Z",
        );
        let second = activity(
            "00000000-0000-4000-8000-000000000002",
            "abc",
            "Second",
            "2026-10-15T13:00:00Z",
        );
        index.record_activities([&first, &second], &[]).unwrap();
        let report = index.regenerate_dirty().unwrap();
        assert_eq!(report.written, 2);
        let files = artifact_files(root.path());
        assert_eq!(files.len(), 2);
        assert_ne!(files[0].file_name(), files[1].file_name());
        let first_path = files
            .iter()
            .find(|path| fs::read_to_string(path).unwrap().contains("AbC"))
            .unwrap();
        let original = fs::read_to_string(first_path).unwrap();
        // The title is YAML-escaped with no raw `![` (markdown::yaml_string).
        assert!(original.contains("title: \"\\u0021[x]"));
        assert!(!original.contains("\n![x]"));

        fs::write(first_path, format!("{original}\nexternal edit\n")).unwrap();
        let replacement = activity(
            "00000000-0000-4000-8000-000000000003",
            "AbC",
            "Safe",
            "2026-10-15T14:00:00Z",
        );
        index.record_activity(&replacement, &[]).unwrap();
        assert_eq!(index.regenerate_dirty().unwrap().written, 1);
        let replaced = fs::read_to_string(first_path).unwrap();
        assert!(!replaced.contains("external edit"));
        let modified = fs::metadata(first_path).unwrap().modified().unwrap();
        index.record_activity(&replacement, &[]).unwrap();
        assert_eq!(index.regenerate_dirty().unwrap().unchanged, 1);
        assert_eq!(
            fs::metadata(first_path).unwrap().modified().unwrap(),
            modified
        );
    }

    #[test]
    fn a_heavy_day_only_marks_the_artifact_dirty_until_regeneration() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let events = (0..96)
            .map(|hour| {
                activity(
                    &format!("00000000-0000-4000-8000-{hour:012}"),
                    "BUSY",
                    "Busy day",
                    &format!("2026-10-15T{:02}:00:00Z", hour % 24),
                )
            })
            .collect::<Vec<_>>();
        index.record_activities(events.iter(), &[]).unwrap();
        assert!(!root.path().join("artifacts").exists());
        let output = Command::new("python3")
            .args([
                "-c",
                "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('select count(*) from dirty_artifacts').fetchone()[0])",
            ])
            .arg(index.database_path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "1");
        assert_eq!(index.regenerate_dirty().unwrap().written, 1);
    }

    fn typed_activity(id: u32, native_id: &str, artifact_type: &str) -> ActivityEvent {
        let mut event = activity(
            &format!("00000000-0000-4000-8000-{id:012}"),
            native_id,
            native_id,
            "2026-10-15T12:00:00-07:00",
        );
        event.artifact_type = artifact_type.into();
        event
    }

    fn dirty_ids(index: &ArtifactIndex) -> Vec<String> {
        read_state(&index.state_path())
            .unwrap()
            .unwrap()
            .dirty
            .into_iter()
            .collect()
    }

    #[test]
    fn an_unwritable_file_stays_dirty_while_the_pass_publishes_the_rest() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let events = [
            typed_activity(1, "A", "work.item"),
            typed_activity(2, "B", "blocked.item"),
            typed_activity(3, "C", "work.item"),
        ];
        index.record_activities(events.iter(), &[]).unwrap();
        // A file where B's type directory belongs makes B's write fail.
        fs::create_dir_all(root.path().join("artifacts")).unwrap();
        fs::write(root.path().join("artifacts/blocked-item"), "in the way").unwrap();
        let report = index.regenerate_dirty().unwrap();
        assert_eq!((report.written, report.failed), (2, 1));
        assert_eq!(
            dirty_ids(&index),
            ["work:example.com/B"],
            "A's and C's completed writes are published; only B waits"
        );
        fs::remove_file(root.path().join("artifacts/blocked-item")).unwrap();
        let report = index.regenerate_dirty().unwrap();
        assert_eq!((report.written, report.unchanged, report.failed), (1, 0, 0));
    }

    #[test]
    fn a_pass_killed_midway_resumes_after_the_files_it_wrote() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let events = ["A", "B", "C", "D"]
            .iter()
            .zip(1..)
            .map(|(name, id)| typed_activity(id, name, "work.item"))
            .collect::<Vec<_>>();
        index.record_activities(events.iter(), &[]).unwrap();
        let (state, _) = index.load_current_state().unwrap();
        // Chrome kills a host whose port closed; a panic stands in for that
        // here, after the first file, with the first checkpoint due at once.
        let killed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            index.regenerate_state_with(state, false, std::time::Duration::ZERO, |written| {
                assert!(written < 1, "killed");
            })
        }));
        assert!(killed.is_err());
        assert_eq!(
            dirty_ids(&index),
            [
                "work:example.com/B",
                "work:example.com/C",
                "work:example.com/D"
            ]
        );
        let report = index.regenerate_dirty().unwrap();
        assert_eq!((report.written, report.unchanged), (3, 0));
    }

    #[test]
    fn a_pass_ending_on_a_checkpoint_does_not_publish_twice() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        index
            .record_activity(&typed_activity(1, "A", "work.item"), &[])
            .unwrap();
        let (state, _) = index.load_current_state().unwrap();
        PUBLICATIONS.with(|count| count.set(0));
        index
            .regenerate_state_with(state, false, Duration::ZERO, |_| {})
            .unwrap();
        assert_eq!(PUBLICATIONS.with(|count| count.get()), 1);
        assert!(dirty_ids(&index).is_empty());
        // Nothing dirty and nothing to repair: nothing to publish.
        let (state, _) = index.load_current_state().unwrap();
        PUBLICATIONS.with(|count| count.set(0));
        index
            .regenerate_state_with(state, false, REGENERATION_CHECKPOINT, |_| {})
            .unwrap();
        assert_eq!(PUBLICATIONS.with(|count| count.get()), 0);
    }

    #[test]
    fn checkpoints_back_off_when_publishing_the_cache_is_slow() {
        let minimum = Duration::from_millis(500);
        assert_eq!(
            checkpoint_interval(minimum, Duration::from_millis(10)),
            minimum
        );
        assert_eq!(
            checkpoint_interval(minimum, Duration::from_millis(300)),
            Duration::from_millis(1200)
        );
    }

    #[test]
    fn the_daily_pass_is_due_once_per_local_day() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let at = |text: &str| OffsetDateTime::parse(text, &Rfc3339).unwrap();
        assert!(index.daily_pass_due(at("2026-10-15T20:00:00Z")));
        index
            .record_activity(&typed_activity(1, "A", "work.item"), &[])
            .unwrap();
        index.regenerate_dirty().unwrap();
        // The pass ran today in the user's -07:00 offset (from their latest
        // activity), so 23:30 local the same day is not a new day, even
        // though UTC has already moved on.
        let marker = fs::read_to_string(cache.path().join("regenerated.json")).unwrap();
        let today = OffsetDateTime::now_utc()
            .to_offset(time::UtcOffset::from_whole_seconds(-7 * 3600).unwrap())
            .date();
        assert!(marker.contains(&today.to_string()), "{marker}");
        let local_late = today
            .with_hms(23, 30, 0)
            .unwrap()
            .assume_offset(time::UtcOffset::from_whole_seconds(-7 * 3600).unwrap());
        assert!(!index.daily_pass_due(local_late));
        assert!(index.daily_pass_due(local_late + time::Duration::hours(1)));
        // A pass for another home folder says nothing about this one.
        let other_root = TempDir::new().unwrap();
        let other =
            ArtifactIndex::open_in(&storage(other_root.path()), None, cache.path()).unwrap();
        assert!(other.daily_pass_due(local_late));
        let mut moved = storage(root.path());
        moved.artifacts_dir = "elsewhere".into();
        let moved = ArtifactIndex::open_in(&moved, None, cache.path()).unwrap();
        assert!(moved.daily_pass_due(local_late));
        fs::write(cache.path().join("regenerated.json"), "not json").unwrap();
        assert!(index.daily_pass_due(local_late));
    }

    #[test]
    fn stale_artifact_and_index_temporaries_are_swept() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        index
            .record_activity(&typed_activity(1, "A", "work.item"), &[])
            .unwrap();
        index.regenerate_dirty().unwrap();
        let site = root.path().join("artifacts/work-item/example-com");
        let old = crate::vault::STALE_TEMPORARY_AGE + std::time::Duration::from_secs(60);
        let make = |dir: &Path, name: String, stale: bool| {
            let path = dir.join(name);
            fs::write(&path, "partial").unwrap();
            if stale {
                crate::vault::tests::age_file(&path, old);
            }
            path
        };
        let stale_artifact = make(
            &site,
            format!(".{NAMESPACE}-artifact-{}.tmp", Uuid::new_v4()),
            true,
        );
        let fresh_artifact = make(
            &site,
            format!(".{NAMESPACE}-artifact-{}.tmp", Uuid::new_v4()),
            false,
        );
        let user_file = make(&site, format!(".{NAMESPACE}-artifact-mine.tmp"), true);
        let stale_index = make(
            cache.path(),
            format!(".{NAMESPACE}-index-{}.tmp", Uuid::new_v4()),
            true,
        );
        let fresh_index = make(
            cache.path(),
            format!(".{NAMESPACE}-index-{}.tmp", Uuid::new_v4()),
            false,
        );
        assert_eq!(index.sweep_stale_temporaries().unwrap(), 2);
        assert!(!stale_artifact.exists());
        assert!(!stale_index.exists());
        assert!(fresh_artifact.exists());
        assert!(fresh_index.exists());
        assert!(user_file.exists());
        assert_eq!(artifact_files(root.path()).len(), 1);
    }

    #[test]
    fn an_unowned_file_at_the_artifact_path_is_never_adopted() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let event = activity(
            "00000000-0000-4000-8000-000000000099",
            "CONFLICT",
            "Conflict",
            "2026-10-15T12:00:00Z",
        );
        index.record_activity(&event, &[]).unwrap();
        let derived = derive(&index.load_or_rebuild().unwrap(), None);
        let artifact = derived.artifacts.get(&event.artifact_id).unwrap();
        let occupied = root.path().join(artifact_path("artifacts", artifact));
        fs::create_dir_all(occupied.parent().unwrap()).unwrap();
        fs::write(&occupied, "someone else's file\n").unwrap();

        let report = index.regenerate_dirty().unwrap();
        assert_eq!(report.conflicts, 1);
        assert_eq!(
            report.conflict_paths,
            vec![
                artifact_path("artifacts", artifact)
                    .to_string_lossy()
                    .into_owned()
            ]
        );
        assert_eq!(
            fs::read_to_string(occupied).unwrap(),
            "someone else's file\n"
        );
    }

    #[test]
    fn ownership_requires_one_matching_brauser_identity_block() {
        let id = "work:example.com/7";
        let owned = format!(
            "---\ntitle: \"Seven\"\nbrauser:\n  kind: artifact\n  artifact_id: {}\n---\n",
            serde_json::to_string(id).unwrap()
        );
        assert!(owned_artifact(owned.as_bytes(), id));

        let unrelated = format!(
            "---\ntitle: \"Seven\"\nother:\n  kind: artifact\n  artifact_id: {}\n---\n",
            serde_json::to_string(id).unwrap()
        );
        assert!(!owned_artifact(unrelated.as_bytes(), id));

        let duplicate = format!(
            "---\nbrauser:\n  kind: artifact\n  artifact_id: {}\nbrauser:\n  kind: artifact\n  artifact_id: {}\n---\n",
            serde_json::to_string(id).unwrap(),
            serde_json::to_string(id).unwrap()
        );
        assert!(!owned_artifact(duplicate.as_bytes(), id));
    }

    #[test]
    fn an_event_id_cannot_replace_different_indexed_content() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let original = activity(
            "00000000-0000-4000-8000-000000000001",
            "7",
            "Original",
            "2026-10-15T12:00:00Z",
        );
        index.record_activity(&original, &[]).unwrap();

        let mut forged = original.clone();
        forged.title = Some("Forged retry".into());
        assert!(index.record_activity(&forged, &[]).is_err());

        let output = Command::new("python3")
            .args([
                "-c",
                "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('select title from artifacts').fetchone()[0])",
            ])
            .arg(index.database_path())
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "Original");
    }

    #[test]
    fn latest_metadata_uses_instant_order_across_offsets() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let early = activity(
            "00000000-0000-4000-8000-000000000999",
            "OFFSET",
            "Earlier",
            "2026-10-16T01:00:00Z",
        );
        let mut later = activity(
            "00000000-0000-4000-8000-000000000001",
            "OFFSET",
            "Later",
            "2026-10-15T23:00:00-07:00",
        );
        later.modes[0].mode = "edit".into();
        index.record_activities([&early, &later], &[]).unwrap();
        index.regenerate_dirty().unwrap();

        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert!(markdown.contains("title: \"Later\""));
        assert!(markdown.contains("  last_mode: \"edit\""));
    }

    #[test]
    fn passive_activity_does_not_invent_an_open_or_erase_the_last_mode() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let opened = activity(
            "00000000-0000-4000-8000-000000000001",
            "CARRY",
            "Opened",
            "2026-10-15T23:00:00Z",
        );
        let mut carried = activity(
            "00000000-0000-4000-8000-000000000002",
            "CARRY",
            "Still open",
            "2026-10-16T00:00:00Z",
        );
        carried.opens = 0;
        carried.modes.clear();
        index.record_activities([&opened, &carried], &[]).unwrap();

        let derived = derive(&index.load_or_rebuild().unwrap(), None);
        let row = derived.artifacts.get(&opened.artifact_id).unwrap();
        assert_eq!(row.days.len(), 1);
        assert_eq!(row.last_mode.as_deref(), Some("view"));
        assert_eq!(row.title, "Still open");
    }

    #[test]
    fn a_year_of_daily_opens_has_a_bounded_timeline() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let start = Date::from_calendar_date(2025, time::Month::October, 1).unwrap();
        let mut events = Vec::new();
        for day in 0..365 {
            let date = start + time::Duration::days(day);
            events.push(activity(
                &format!("00000000-0000-4000-8000-{day:012}"),
                "YEAR",
                "Year",
                &format!("{date}T12:00:00Z"),
            ));
        }
        index.record_activities(events.iter(), &[]).unwrap();
        index.regenerate_dirty().unwrap();
        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        let lines = markdown
            .lines()
            .filter(|line| line.starts_with("- "))
            .count();
        assert!(lines <= 48, "unbounded Timeline had {lines} lines");
        assert_eq!(
            markdown.lines().find(|line| line.starts_with("- ")),
            Some("- 2026-09-30 · focused 2 min · 1 min in background · opened 1×")
        );
        assert!(markdown.contains("- 2025-10"));
        assert!(markdown.contains("- 2026-09-30"));
        assert!(
            fs::metadata(index.database_path()).unwrap().len() > (5 * SQLITE_PAGE_SIZE) as u64,
            "the SQLite fixture did not exercise an interior B-tree page"
        );
        let output = Command::new("python3").args([
            "-c", "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('select count(*) from visits').fetchone()[0])",
        ]).arg(index.database_path()).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "365");
    }

    #[test]
    fn old_unknown_trail_transitions_preserve_activity_but_never_become_edges() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let source = activity(
            "00000000-0000-4000-8000-000000000021",
            "SOURCE",
            "Source",
            "2026-10-15T12:00:00Z",
        );
        let mut target = activity(
            "00000000-0000-4000-8000-000000000022",
            "TARGET",
            "Target",
            "2026-10-15T13:00:00Z",
        );
        target.trails = ["link", "\u{202e}unknown"]
            .into_iter()
            .map(|transition| ActivityTrail {
                from_artifact_id: Some(source.artifact_id.clone()),
                from_url: Some(source.url.clone()),
                transition: transition.into(),
            })
            .collect();
        let marker = |event: &ActivityEvent| {
            format!(
                "{MARKER_PREFIX}{} url={} at={} kind=activity data={} -->\n",
                sha256_hex(event.event_id.as_bytes()),
                sha256_hex(event.url.as_bytes()),
                event.occurred_at,
                hex::encode(serde_json::to_vec(event).unwrap()),
            )
        };
        let log = format!("{LOG_HEADER}\n{}{}", marker(&source), marker(&target));
        let logs = root.path().join("log/2026/10");
        fs::create_dir_all(&logs).unwrap();
        let log_path = logs.join("2026-10-15.md");
        fs::write(&log_path, &log).unwrap();

        // Simulate an ingestion cache from before transition validation.
        let mut state = index.empty_state();
        for event in [&source, &target] {
            state
                .events
                .insert(event.event_id.clone(), CachedEvent::Activity(event.clone()));
            state.dirty.insert(event.artifact_id.clone());
        }
        index.publish_state_and_database(&state).unwrap();
        let check = || {
            let artifact = index.artifact(&target.artifact_id).unwrap().unwrap();
            assert_eq!(artifact.opens, 1);
            assert_eq!(artifact.focused_seconds, 125);
            assert_eq!(artifact.background_seconds, 61);
            assert_eq!(artifact.linked_count, 1);
            let output = Command::new("python3")
                .args([
                    "-c",
                    "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); print(c.execute('select count(*) from visits').fetchone()[0]); print([r[0] for r in c.execute('select kind from edges')])",
                ])
                .arg(index.database_path())
                .output()
                .unwrap();
            assert!(output.status.success(), "{:?}", output.stderr);
            // Python writes \r\n on Windows.
            assert_eq!(
                String::from_utf8(output.stdout)
                    .unwrap()
                    .replace("\r\n", "\n"),
                "2\n['link']\n"
            );
            // Do not rewrite historical payloads: their exact content detects
            // conflicting reuse of an event ID during replay and rebuilding.
            assert!(
                index.load_or_rebuild().unwrap().events[&target.event_id]
                    == CachedEvent::Activity(target.clone())
            );
        };
        check();
        assert_eq!(index.reindex().unwrap().written, 2);
        check();
        assert_eq!(fs::read_to_string(log_path).unwrap(), log);

        // Legacy tolerance must keep the original size and paired-source caps.
        let mut malformed = target.clone();
        malformed.trails[0].transition = "x".repeat(65);
        assert!(parse_log(&marker(&malformed)).is_empty());
        malformed = target.clone();
        malformed.trails[0].from_url = None;
        assert!(parse_log(&marker(&malformed)).is_empty());
        malformed = target.clone();
        malformed.trails = vec![target.trails[0].clone(); 11];
        assert!(parse_log(&marker(&malformed)).is_empty());
    }

    fn activity_marker(event: &ActivityEvent) -> String {
        format!(
            "{MARKER_PREFIX}{} url={} at={} kind=activity data={} -->\n",
            sha256_hex(event.event_id.as_bytes()),
            sha256_hex(event.url.as_bytes()),
            event.occurred_at,
            hex::encode(serde_json::to_vec(event).unwrap())
        )
    }

    #[test]
    fn checkpoints_skip_unchanged_logs_and_recover_completed_trailing_markers() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let first = activity(
            "00000000-0000-4000-8000-000000000001",
            "FIRST",
            "First",
            "2026-10-15T12:00:00Z",
        );
        let second = activity(
            "00000000-0000-4000-8000-000000000002",
            "SECOND",
            "Second",
            "2026-10-16T12:00:00Z",
        );
        let dir = root.path().join("log");
        fs::create_dir(&dir).unwrap();
        fs::write(
            dir.join("first.md"),
            format!("{LOG_HEADER}\n{}", activity_marker(&first)),
        )
        .unwrap();
        assert!(index.artifact(&first.artifact_id).unwrap().is_some());
        // A partial trailing marker is not lost when its completion arrives;
        // the entire changed file (including the previous prose) is reparsed.
        let marker = activity_marker(&second);
        let cut = marker.len() / 2;
        let path = dir.join("second.md");
        fs::write(&path, format!("{LOG_HEADER}\n{}", &marker[..cut])).unwrap();
        LOG_BYTES_READ.with(|count| count.set(0));
        assert!(index.artifact(&second.artifact_id).unwrap().is_none());
        assert_eq!(
            LOG_BYTES_READ.with(|count| count.get()),
            fs::metadata(&path).unwrap().len()
        );
        StdOpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(&marker.as_bytes()[cut..])
            .unwrap();
        LOG_BYTES_READ.with(|count| count.set(0));
        assert_eq!(
            index.artifact(&second.artifact_id).unwrap().unwrap().opens,
            1
        );
        assert_eq!(
            LOG_BYTES_READ.with(|count| count.get()),
            fs::metadata(&path).unwrap().len()
        );
        assert!(
            read_state(&index.state_path())
                .unwrap()
                .unwrap()
                .dirty
                .contains(&second.artifact_id)
        );
        LOG_BYTES_READ.with(|count| count.set(0));
        assert_eq!(
            index.artifact(&second.artifact_id).unwrap().unwrap().opens,
            1
        );
        assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);
        // The old format has no ingestion checkpoints. Treat it as a cache
        // miss and rebuild every event, even if its old events map is valid.
        let mut old: serde_json::Value =
            serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
        old["version"] = 1.into();
        old["events"].as_object_mut().unwrap().clear();
        old.as_object_mut().unwrap().remove("logs");
        fs::write(index.state_path(), serde_json::to_vec(&old).unwrap()).unwrap();
        assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 2);
        assert_eq!(
            read_state(&index.state_path()).unwrap().unwrap().version,
            STATE_VERSION
        );
    }

    #[test]
    fn catch_up_bounds_aggregate_reads_without_publishing_partial_history() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        index
            .record_activity(
                &activity(
                    "00000000-0000-4000-8000-000000000001",
                    "FIRST",
                    "First",
                    "2026-10-15T12:00:00Z",
                ),
                &[],
            )
            .unwrap();
        let old_state = fs::read(index.state_path()).unwrap();
        let old_db = fs::read(index.database_path()).unwrap();
        fs::create_dir(root.path().join("log")).unwrap();
        for day in 1..=3 {
            let mut file = File::create(root.path().join(format!("log/{day}.md"))).unwrap();
            file.write_all(format!("{LOG_HEADER}\n").as_bytes())
                .unwrap();
            file.set_len(24 * 1024 * 1024).unwrap();
        }
        LOG_BYTES_READ.with(|count| count.set(0));
        let error = index.recent(None, None, 32).unwrap_err().to_string();
        assert!(
            error.contains("catch-up reached its 64 MiB per-pass limit"),
            "{error}"
        );
        assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 48 * 1024 * 1024);
        assert_eq!(fs::read(index.state_path()).unwrap(), old_state);
        assert_eq!(fs::read(index.database_path()).unwrap(), old_db);
        // The next pass resumes after the logs already read.
        LOG_BYTES_READ.with(|count| count.set(0));
        assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 1);
        assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 24 * 1024 * 1024);
        assert!(!index.pending_path().exists());
    }

    /// Run `pass` until it succeeds. Every pass must stay within the
    /// per-pass read bound and report history unavailable until it is whole.
    fn bounded_passes<T>(limit: u64, mut pass: impl FnMut() -> Result<T>) -> (T, usize) {
        for passes in 1..=16 {
            LOG_BYTES_READ.with(|count| count.set(0));
            let result = pass();
            let read = LOG_BYTES_READ.with(|count| count.get());
            assert!(read <= limit, "pass {passes} read {read} of {limit} bytes");
            match result {
                Ok(value) => return (value, passes),
                Err(error) => {
                    let error = format!("{error:#}");
                    assert!(error.contains("history is unavailable"), "{error}");
                }
            }
        }
        panic!("catch-up never finished");
    }

    #[test]
    fn full_rebuilds_finish_in_bounded_passes_once_history_exceeds_one_pass() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let events = (1..=6)
            .map(|day| {
                activity(
                    &format!("00000000-0000-4000-8000-{day:012}"),
                    &format!("N{day}"),
                    &format!("Item {day}"),
                    &format!("2026-10-1{day}T12:00:00Z"),
                )
            })
            .collect::<Vec<_>>();
        let log = |day: usize| format!("2026-10-1{day}.md");
        // Two logs fit in one pass; the whole history needs three.
        let size = fs::metadata(write_log(root.path(), &log(1), &[&events[0]]))
            .unwrap()
            .len();
        let limit = size * 5 / 2;
        SCAN_LIMIT.with(|value| value.set(limit));
        // History grows one day at a time, so each catch-up reads one log.
        for (day, event) in events.iter().enumerate().map(|(n, event)| (n + 1, event)) {
            write_log(root.path(), &log(day), &[event]);
            let (recent, passes) = bounded_passes(limit, || index.recent(None, None, 32));
            assert_eq!((recent.0.len(), passes), (day, 1));
        }

        // A deleted cache rebuilds in bounded passes and never publishes a
        // partial checkpoint on the way.
        fs::remove_file(index.state_path()).unwrap();
        let old_db = fs::read(index.database_path()).unwrap();
        let mut partial = Vec::new();
        let (recent, passes) = bounded_passes(limit, || {
            let result = index.recent(None, None, 32);
            if result.is_err() {
                partial.push((
                    index.state_path().exists(),
                    fs::read(index.database_path()).unwrap() == old_db,
                ));
            }
            result
        });
        assert_eq!((recent.0.len(), passes), (6, 3));
        assert_eq!(partial, [(false, true); 2]);
        LOG_BYTES_READ.with(|count| count.set(0));
        assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 6);
        assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);

        // The documented repair command finishes in one invocation.
        let report = index.reindex().unwrap();
        assert_eq!(report.written, 6);
        assert_eq!(artifact_files(root.path()).len(), 6);
        LOG_BYTES_READ.with(|count| count.set(0));
        assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 6);
        assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);

        // Duplicate reconciliation resumes its own rebuild.
        let (_, passes) = bounded_passes(limit, || index.reconcile_from_logs());
        assert_eq!(passes, 3);
        assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 6);

        // A deleted log resets and rebuilds the remaining five.
        fs::remove_file(root.path().join("log").join(log(1))).unwrap();
        let (recent, passes) = bounded_passes(limit, || index.recent(None, None, 32));
        assert_eq!((recent.0.len(), passes), (5, 3));
        // A truncated log does the same with the remaining four.
        fs::write(
            root.path().join("log").join(log(2)),
            format!("{LOG_HEADER}\n"),
        )
        .unwrap();
        let (report, _) = bounded_passes(limit, || index.regenerate_dirty());
        assert_eq!(report.failed, 0);
        assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 4);
        assert!(index.artifact(&events[1].artifact_id).unwrap().is_none());
    }

    #[test]
    fn markdown_without_a_log_header_does_not_stall_a_bounded_rebuild() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let event = activity(
            "00000000-0000-4000-8000-000000000001",
            "FIRST",
            "First",
            "2026-10-15T12:00:00Z",
        );
        let size = fs::metadata(write_log(root.path(), "2026-10-15.md", &[&event]))
            .unwrap()
            .len();
        // Two notes that are not logs fill the first pass and sort first.
        for name in ["2026-01-01.md", "2026-01-02.md"] {
            fs::write(
                root.path().join("log").join(name),
                "x".repeat(size as usize),
            )
            .unwrap();
        }
        let limit = size * 5 / 2;
        SCAN_LIMIT.with(|value| value.set(limit));
        let (recent, passes) = bounded_passes(limit, || index.recent(None, None, 32));
        assert_eq!((recent.0.len(), passes), (1, 2));
        LOG_BYTES_READ.with(|count| count.set(0));
        assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 1);
        assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);
    }

    #[test]
    fn a_rebuild_split_over_passes_keeps_the_earliest_logged_body() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let original = activity(
            "00000000-0000-4000-8000-000000000010",
            "REUSED",
            "Original",
            "2026-10-20T12:00:00Z",
        );
        let mut backdated = original.clone();
        backdated.title = Some("Backdated".into());
        let other = activity(
            "00000000-0000-4000-8000-000000000011",
            "OTHER",
            "Other",
            "2026-10-25T12:00:00Z",
        );
        let size = fs::metadata(write_log(root.path(), "2026-10-20.md", &[&original]))
            .unwrap()
            .len();
        write_log(root.path(), "2026-10-25.md", &[&other]);
        // One log per pass, with no cache to keep a winner from.
        let limit = size * 3 / 2;
        SCAN_LIMIT.with(|value| value.set(limit));
        assert!(index.recent(None, None, 32).is_err());
        // An earlier-dated log lands between passes.
        write_log(root.path(), "2026-10-15.md", &[&backdated]);
        let (recent, _) = bounded_passes(limit, || index.recent(None, None, 32));
        assert_eq!(recent.0.len(), 2);
        let bounded = index
            .artifact(&original.artifact_id)
            .unwrap()
            .unwrap()
            .title;
        // The same logs rebuilt in one pass without a cache.
        SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
        fs::remove_file(index.state_path()).unwrap();
        let single = index
            .artifact(&original.artifact_id)
            .unwrap()
            .unwrap()
            .title;
        assert_eq!(
            (bounded.as_str(), single.as_str()),
            ("Backdated", "Backdated")
        );
    }

    #[test]
    fn a_reset_catch_up_keeps_the_conflict_winner_the_cache_chose() {
        let original = activity(
            "00000000-0000-4000-8000-000000000010",
            "REUSED",
            "Original",
            "2026-10-20T12:00:00Z",
        );
        let mut backdated = original.clone();
        backdated.title = Some("Backdated".into());
        let unrelated = activity(
            "00000000-0000-4000-8000-000000000011",
            "OTHER",
            "Other",
            "2026-09-01T12:00:00Z",
        );
        // Same artifact and instant, so log position decides the latest
        // title: the winner's own position, not the rejected body's.
        let sibling = activity(
            "00000000-0000-4000-8000-000000000012",
            "REUSED",
            "Sibling",
            "2026-10-20T12:00:00Z",
        );
        // Whether the rebuild after the reset takes one pass or several.
        for bounded in [false, true] {
            let root = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let index = index(&root, &cache);
            let title = || {
                index
                    .artifact(&original.artifact_id)
                    .unwrap()
                    .unwrap()
                    .title
            };
            write_log(root.path(), "2026-09-01.md", &[&unrelated]);
            write_log(root.path(), "2026-10-18.md", &[&sibling]);
            write_log(root.path(), "2026-10-20.md", &[&original]);
            index.regenerate_dirty().unwrap();
            assert_eq!(title(), "Original");
            // A later delivery backdated into an earlier-dated log.
            let backdated_log = write_log(root.path(), "2026-10-15.md", &[&backdated]);
            index.regenerate_dirty().unwrap();
            assert_eq!(title(), "Original", "bounded={bounded}");
            index.reindex().unwrap();
            assert_eq!(title(), "Original", "bounded={bounded}");
            // One log per pass, so the two bodies meet in different passes.
            let limit = fs::metadata(&backdated_log).unwrap().len() * 3 / 2;
            if bounded {
                SCAN_LIMIT.with(|value| value.set(limit));
            }
            // Removing an unrelated log must not swap in the backdated body.
            fs::remove_file(root.path().join("log/2026-09-01.md")).unwrap();
            let (report, passes) = bounded_passes(MAX_SCAN_BYTES, || index.regenerate_dirty());
            assert_eq!(report.failed, 0);
            assert_eq!(passes, if bounded { 3 } else { 1 });
            assert_eq!(title(), "Original", "bounded={bounded}");
            let owned = artifact_files(root.path())
                .into_iter()
                .map(|path| fs::read_to_string(path).unwrap())
                .collect::<String>();
            assert!(owned.contains("Original"), "bounded={bounded}: {owned}");
            assert!(!owned.contains("Backdated"), "bounded={bounded}: {owned}");
            SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
        }
    }

    #[test]
    fn reindex_rebuilds_database_and_files_from_log_markers() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let event = activity(
            "00000000-0000-4000-8000-000000000010",
            "REBUILD",
            "Rebuilt",
            "2026-10-15T12:00:00Z",
        );
        let payload = hex::encode(serde_json::to_vec(&event).unwrap());
        let event_hash = sha256_hex(event.event_id.as_bytes());
        let url_hash = sha256_hex(event.url.as_bytes());
        let log = format!(
            "<!-- brauser:daily-log v1 -->\n\n- ignored presentation\n<!-- brauser:visit id={event_hash} url={url_hash} at={} kind=activity data={payload} -->\n<!-- brauser:visit id=broken -->\n",
            event.occurred_at,
        );
        let dir = root.path().join("log/2026/10");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("2026-10-15.md"), log).unwrap();

        let report = index.reindex().unwrap();
        assert_eq!(report.written, 1);
        let artifact = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert!(artifact.contains("Rebuilt"));
        let output = Command::new("python3").args([
            "-c", "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute('select count(*) from visits').fetchone()[0])",
        ]).arg(index.database_path()).output().unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), "1");
    }

    fn write_log(root: &Path, name: &str, events: &[&ActivityEvent]) -> PathBuf {
        let dir = root.join("log");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let markers = events.iter().map(|event| activity_marker(event));
        fs::write(
            &path,
            format!("{LOG_HEADER}\n{}", markers.collect::<String>()),
        )
        .unwrap();
        path
    }

    #[test]
    fn a_renamed_log_folder_is_rescanned_and_its_artifacts_regenerate_in_place() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let old = index(&root, &cache);
        let event = activity(
            "00000000-0000-4000-8000-000000000020",
            "MOVED",
            "Moved",
            "2026-10-15T12:00:00Z",
        );
        write_log(root.path(), "2026-10-15.md", &[&event]);
        assert_eq!(
            old.artifact(&event.artifact_id).unwrap().unwrap().title,
            "Moved"
        );
        old.regenerate_dirty().unwrap();
        // The layout migration renames log/ to logs/ and records the new
        // name; the cache that still names log/ must not hide the history.
        fs::rename(root.path().join("log"), root.path().join("logs")).unwrap();
        let mut renamed = storage(root.path());
        renamed.log_dir = "logs".into();
        let new = ArtifactIndex::open_in(&renamed, None, cache.path()).unwrap();
        assert_eq!(
            new.artifact(&event.artifact_id).unwrap().unwrap().title,
            "Moved"
        );
        let files = artifact_files(root.path());
        assert_eq!(files.len(), 1);
        let before = fs::read(&files[0]).unwrap();
        new.regenerate_dirty().unwrap();
        assert_eq!(artifact_files(root.path()), files);
        assert_eq!(fs::read(&files[0]).unwrap(), before);
    }

    #[test]
    fn latest_metadata_follows_time_and_log_order_not_event_id_order() {
        // A passive later hour (no title after a reload, no mode) whose
        // random event ID sorts first must not hide the earlier hour's title
        // and mode, in either ID order.
        for (opened_id, passive_id) in [
            (
                "ffffffff-0000-4000-8000-000000000001",
                "00000000-0000-4000-8000-000000000002",
            ),
            (
                "00000000-0000-4000-8000-000000000001",
                "ffffffff-0000-4000-8000-000000000002",
            ),
        ] {
            let root = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let index = index(&root, &cache);
            let mut opened = activity(opened_id, "SPEC", "Spec", "2026-10-15T10:00:00Z");
            opened.modes[0].mode = "edit".into();
            let mut passive = activity(passive_id, "SPEC", "ignored", "2026-10-15T11:00:00Z");
            passive.title = None;
            passive.opens = 0;
            passive.modes.clear();
            write_log(root.path(), "2026-10-15.md", &[&opened, &passive]);
            let view = index.artifact(&opened.artifact_id).unwrap().unwrap();
            assert_eq!(view.last_mode.as_deref(), Some("edit"), "{opened_id}");
            assert_eq!(view.title, "Spec", "{opened_id}");
        }

        // Context is saved with one-second timestamps, so two saves in the
        // same second keep the one appended last, whatever their IDs.
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let event = activity(
            "00000000-0000-4000-8000-000000000001",
            "WHY",
            "Why",
            "2026-10-15T10:00:00Z",
        );
        index.record_activity(&event, &[]).unwrap();
        for (id, text) in [
            ("ffffffff-0000-4000-8000-000000000001", "first"),
            ("00000000-0000-4000-8000-000000000003", "second"),
        ] {
            let why = WhyEvent {
                event_id: id.into(),
                artifact_id: event.artifact_id.clone(),
                url: "https://example.com/items/WHY".into(),
                occurred_at: "2026-10-15T10:30:00Z".into(),
                incognito: false,
                text: text.into(),
            };
            index.record_why(&why, &[]).unwrap();
        }
        assert_eq!(
            index.latest_why(&event.artifact_id).unwrap().as_deref(),
            Some("second")
        );
        assert_eq!(
            index
                .artifact(&event.artifact_id)
                .unwrap()
                .unwrap()
                .why
                .as_deref(),
            Some("second")
        );
    }

    #[test]
    fn equal_instants_across_logs_order_by_log_position_not_discovery() {
        let why = |id: &str, at: &str, text: &str| WhyEvent {
            event_id: id.into(),
            artifact_id: "work:example.com/TIE".into(),
            url: "https://example.com/items/TIE".into(),
            occurred_at: at.into(),
            incognito: false,
            text: text.into(),
        };
        let marker = |event: &WhyEvent| {
            format!(
                "{MARKER_PREFIX}{} url={} at={} kind=why data={} -->\n",
                sha256_hex(event.event_id.as_bytes()),
                sha256_hex(event.url.as_bytes()),
                event.occurred_at,
                hex::encode(serde_json::to_vec(event).unwrap())
            )
        };
        // One instant written with two offsets lands in two date files.
        let early = why(
            "00000000-0000-4000-8000-000000000001",
            "2026-10-15T23:30:00-02:00",
            "early path",
        );
        let late = why(
            "00000000-0000-4000-8000-000000000002",
            "2026-10-16T01:30:00Z",
            "late path",
        );
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let dir = root.path().join("log");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("2026-10-16.md"),
            format!("{LOG_HEADER}\n{}", marker(&late)),
        )
        .unwrap();
        assert_eq!(
            index.latest_why(&late.artifact_id).unwrap().as_deref(),
            Some("late path")
        );
        // The earlier-sorting file is discovered later.
        fs::write(
            dir.join("2026-10-15.md"),
            format!("{LOG_HEADER}\n{}", marker(&early)),
        )
        .unwrap();
        let incremental = index.latest_why(&late.artifact_id).unwrap();
        fs::remove_file(index.state_path()).unwrap();
        let rebuilt = index.latest_why(&late.artifact_id).unwrap();
        assert_eq!(incremental, rebuilt);
        assert_eq!(rebuilt.as_deref(), Some("late path"));
    }

    #[test]
    fn a_torn_multibyte_line_keeps_the_rest_of_that_days_log() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let before = activity(
            "00000000-0000-4000-8000-000000000001",
            "BEFORE",
            "Before",
            "2026-10-15T10:00:00Z",
        );
        let after = activity(
            "00000000-0000-4000-8000-000000000002",
            "AFTER",
            "After",
            "2026-10-15T11:00:00Z",
        );
        let path = write_log(root.path(), "2026-10-15.md", &[&before]);
        // A crash tore an append inside an em dash; capture keeps appending.
        let mut file = StdOpenOptions::new().append(true).open(&path).unwrap();
        file.write_all(b"- 10:30 \xe2\x80\n").unwrap();
        file.write_all(activity_marker(&after).as_bytes()).unwrap();
        drop(file);
        assert!(index.artifact(&before.artifact_id).unwrap().is_some());
        assert!(index.artifact(&after.artifact_id).unwrap().is_some());
        assert_eq!(index.reindex().unwrap().written, 2);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_log_names_its_path() {
        use std::os::unix::fs::PermissionsExt;
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let event = activity(
            "00000000-0000-4000-8000-000000000001",
            "LOCKED",
            "Locked",
            "2026-10-15T10:00:00Z",
        );
        let path = write_log(root.path(), "2026-10-15.md", &[&event]);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let error = format!("{:#}", index.reindex().unwrap_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(error.contains("2026-10-15.md"), "{error}");
    }

    #[test]
    fn a_reused_event_id_keeps_the_earliest_durable_event_and_the_rest_of_history() {
        let original = activity(
            "00000000-0000-4000-8000-000000000010",
            "REUSED",
            "Original",
            "2026-10-24T12:00:00Z",
        );
        let mut forged = original.clone();
        forged.title = Some("Forged".into());
        forged.occurred_at = "2026-10-25T12:00:00Z".into();
        let mut same_time = original.clone();
        same_time.title = Some("Also forged".into());
        // A later marker claiming an earlier time must not win either.
        let mut backdated = original.clone();
        backdated.title = Some("Backdated".into());
        backdated.occurred_at = "2026-10-23T12:00:00Z".into();
        let other = activity(
            "00000000-0000-4000-8000-000000000011",
            "OTHER",
            "Other",
            "2026-10-25T13:00:00Z",
        );
        // Full rebuilds and incremental catch-up agree on the winner.
        for incremental in [false, true] {
            let root = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let index = index(&root, &cache);
            write_log(root.path(), "2026-10-24.md", &[&original]);
            if incremental {
                assert!(index.artifact(&original.artifact_id).unwrap().is_some());
            }
            write_log(
                root.path(),
                "2026-10-25.md",
                &[&forged, &same_time, &backdated, &other],
            );
            if !incremental {
                index.reindex().unwrap();
            }
            let view = index.artifact(&original.artifact_id).unwrap().unwrap();
            assert_eq!(view.title, "Original", "incremental={incremental}");
            assert!(index.artifact(&other.artifact_id).unwrap().is_some());
            assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 2);
            // A later delivery backdated into an earlier-sorting log never
            // replaces the body already indexed.
            write_log(root.path(), "2026-10-23.md", &[&backdated]);
            let view = index.artifact(&original.artifact_id).unwrap().unwrap();
            assert_eq!(view.title, "Original", "incremental={incremental}");
            assert!(index.record_activity(&backdated, &[]).is_err());
            let view = index.artifact(&original.artifact_id).unwrap().unwrap();
            assert_eq!(view.title, "Original", "incremental={incremental}");
            // Rescans keep the winner a valid cache already recorded.
            index.reconcile_from_logs().unwrap();
            let view = index.artifact(&original.artifact_id).unwrap().unwrap();
            assert_eq!(view.title, "Original", "incremental={incremental}");
            index.reindex().unwrap();
            let view = index.artifact(&original.artifact_id).unwrap().unwrap();
            assert_eq!(view.title, "Original", "incremental={incremental}");
        }
    }

    #[test]
    fn reconciling_a_duplicate_keeps_the_dirty_set_and_external_edits() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let events = (1..=3)
            .map(|n| {
                activity(
                    &format!("00000000-0000-4000-8000-{n:012}"),
                    &format!("N{n}"),
                    &format!("Item {n}"),
                    "2026-10-15T10:00:00Z",
                )
            })
            .collect::<Vec<_>>();
        let path = write_log(root.path(), "2026-10-15.md", &[&events[0], &events[1]]);
        assert_eq!(index.regenerate_dirty().unwrap().written, 2);
        let edited = artifact_files(root.path())
            .into_iter()
            .find(|path| fs::read_to_string(path).unwrap().contains("Item 1"))
            .unwrap();
        let kept = format!("{}\nmy edit\n", fs::read_to_string(&edited).unwrap());
        fs::write(&edited, &kept).unwrap();
        // A drained legacy visit or duplicate retry reconciles from the log.
        StdOpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(activity_marker(&events[2]).as_bytes())
            .unwrap();
        index.reconcile_from_logs().unwrap();
        assert_eq!(
            read_state(&index.state_path()).unwrap().unwrap().dirty,
            BTreeSet::from([events[2].artifact_id.clone()])
        );
        assert_eq!(index.regenerate_dirty().unwrap().written, 1);
        assert_eq!(fs::read_to_string(&edited).unwrap(), kept);
    }

    #[cfg(unix)]
    #[test]
    fn one_failing_artifact_path_does_not_block_the_others() {
        use std::os::unix::fs::PermissionsExt;
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let blocked = activity(
            "00000000-0000-4000-8000-000000000001",
            "AAA",
            "Blocked",
            "2026-10-15T10:00:00Z",
        );
        let mut open = activity(
            "00000000-0000-4000-8000-000000000002",
            "BBB",
            "Open",
            "2026-10-15T10:00:00Z",
        );
        open.artifact_id = "work:other.example/BBB".into();
        open.url = "https://other.example/items/BBB".into();
        open.modes[0].url = open.url.clone();
        index.record_activities([&blocked, &open], &[]).unwrap();
        let derived = derive(&index.load_or_rebuild().unwrap(), None);
        let relative = artifact_path("artifacts", &derived.artifacts[&blocked.artifact_id]);
        let site = root.path().join(relative.parent().unwrap());
        fs::create_dir_all(&site).unwrap();
        fs::set_permissions(&site, fs::Permissions::from_mode(0o555)).unwrap();
        let report = index.regenerate_dirty();
        fs::set_permissions(&site, fs::Permissions::from_mode(0o755)).unwrap();
        let report = report.unwrap();
        assert_eq!(report.written, 1);
        assert_eq!(report.failed, 1);
        assert_eq!(report.failed_paths.len(), 1);
        assert!(
            report.failed_paths[0].contains("aaa"),
            "{:?}",
            report.failed_paths
        );
        assert_eq!(
            read_state(&index.state_path()).unwrap().unwrap().dirty,
            BTreeSet::from([blocked.artifact_id.clone()])
        );
        assert_eq!(artifact_files(root.path()).len(), 1);
        // Once the path is writable again, the remaining artifact catches up.
        let report = index.regenerate_dirty().unwrap();
        assert_eq!((report.written, report.failed), (1, 0));
    }

    #[test]
    fn mentions_resolve_through_the_adapter_document() {
        let document: adapter_templates::Value = serde_json::from_value(serde_json::json!({
            "format": 1,
            "artifacts": [{
                "type": "jira.issue",
                "origin": "https://acme.atlassian.net",
                "match": [{"path": "/browse/{key:jira_key}"}],
                "id": "jira:{host}/{key}",
            }],
        }))
        .unwrap();
        let document = adapter_templates::Document::from_value(&document).unwrap();
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache).with_adapters(Some(&document));
        let mut issue = activity(
            "00000000-0000-4000-8000-000000000001",
            "PROJ-1",
            "Issue",
            "2026-10-15T10:00:00Z",
        );
        issue.artifact_id = "jira:acme.atlassian.net/PROJ-1".into();
        issue.artifact_type = "jira.issue".into();
        issue.url = "https://acme.atlassian.net/browse/PROJ-1".into();
        issue.modes.clear();
        // An artifact's title can name its own key; that is never a link.
        issue.refs.push(ActivityRef {
            artifact_type: "jira.issue".into(),
            value: "PROJ-1".into(),
        });
        let mut pull = activity(
            "00000000-0000-4000-8000-000000000002",
            "412",
            "PROJ-1 Fix SBOM export pagination",
            "2026-10-15T11:00:00Z",
        );
        pull.refs = ["PROJ-1", "PROJ-1", "UTF-8"]
            .into_iter()
            .map(|value| ActivityRef {
                artifact_type: "jira.issue".into(),
                value: value.into(),
            })
            .collect();
        index.record_activities([&issue, &pull], &[]).unwrap();
        for id in [&issue.artifact_id, &pull.artifact_id] {
            assert_eq!(index.artifact(id).unwrap().unwrap().linked_count, 1, "{id}");
        }
        let query = |sql: &str| {
            let output = Command::new("python3")
                .args([
                    "-c",
                    "import sqlite3,sys; print(sqlite3.connect(sys.argv[1]).execute(sys.argv[2]).fetchone()[0])",
                ])
                .arg(index.database_path())
                .arg(sql)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        let linked = "select count(*) from edges e join artifacts a on a.artifact_id = e.target_artifact_id where e.kind = 'mentions'";
        assert_eq!(query(linked), "1");
        // A repeated ref is one mention, not one per copy.
        assert_eq!(
            query("select max(weight) from edges where kind = 'mentions'"),
            "1"
        );
        assert_eq!(
            query(&format!(
                "select count(*) from edges where source_artifact_id = '{}'",
                issue.artifact_id
            )),
            "0"
        );
        // Without a document (or with an ambiguous one) a ref stays
        // unresolved, and the published snapshot follows the change.
        let unresolved = index.clone().with_adapters(None);
        assert_eq!(
            unresolved
                .artifact(&issue.artifact_id)
                .unwrap()
                .unwrap()
                .linked_count,
            0
        );
        assert_eq!(query(linked), "0");
    }

    #[test]
    fn a_valid_stale_cache_catches_up_events_appended_by_another_path() {
        // brwr-ee5: deliver A and keep its cache; B reaches the durable log
        // without an index update; the next delivery or query includes B.
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let a = activity(
            "00000000-0000-4000-8000-000000000001",
            "A",
            "A",
            "2026-10-15T10:00:00Z",
        );
        let b = activity(
            "00000000-0000-4000-8000-000000000002",
            "B",
            "B",
            "2026-10-15T11:00:00Z",
        );
        let c = activity(
            "00000000-0000-4000-8000-000000000003",
            "C",
            "C",
            "2026-10-16T11:00:00Z",
        );
        let path = write_log(root.path(), "2026-10-15.md", &[&a]);
        index.record_activity(&a, &[]).unwrap();
        let stale = fs::read(index.state_path()).unwrap();
        StdOpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(activity_marker(&b).as_bytes())
            .unwrap();
        fs::write(index.state_path(), &stale).unwrap();
        write_log(root.path(), "2026-10-16.md", &[&c]);
        index.record_activity(&c, &[]).unwrap();
        assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 3);
        // A restored stale cache on a later query (a restart) also catches up.
        fs::write(index.state_path(), &stale).unwrap();
        let restarted = ArtifactIndex::open_in(&storage(root.path()), None, cache.path()).unwrap();
        assert!(restarted.artifact(&b.artifact_id).unwrap().is_some());
    }

    fn page_summary(url: &str, occurred_at: &str) -> crate::summary::PageSummary {
        crate::summary::PageSummary {
            event_id: uuid::Uuid::new_v4().to_string(),
            url: url.into(),
            title: "A good discussion".into(),
            occurred_at: occurred_at.into(),
            document_type: "discussion".into(),
            category: "reading".into(),
            importance: brauser_protocol::ImportanceLevel::High,
            summary_path: "pages/hn-item.summary.md".into(),
            record_key: crate::vault::url_record_key(url).unwrap(),
            artifact_type: "web.page".into(),
            harness: brauser_protocol::HarnessId::Codex,
        }
    }

    fn rfc3339_seconds_ago(seconds: i64) -> String {
        (OffsetDateTime::now_utc() - time::Duration::seconds(seconds))
            .replace_nanosecond(0)
            .unwrap()
            .format(&Rfc3339)
            .unwrap()
    }

    #[test]
    fn summary_event_appends_once_on_a_page_no_adapter_covers() {
        use crate::capture::{CapturePolicy, CaptureStore};

        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let config = storage(root.path());
        let capture = CaptureStore::open_in(&config, None, cache.path()).unwrap();
        let index = index(&root, &cache);
        let policy = CapturePolicy {
            enabled: true,
            origins: &[],
            strip_params: &[],
        };
        let url = "https://news.ycombinator.com/item?id=50251344";
        let summary = page_summary(url, &rfc3339_seconds_ago(5));

        assert!(matches!(
            capture.record_summary(policy, &summary).0,
            crate::capture::CaptureOutcome::Persisted { .. }
        ));

        let report = index.reindex().unwrap();
        assert_eq!(report.written, 1);
        let expected_id = crate::summary::web_page_artifact_id(url).unwrap();
        let artifact = index.artifact(&expected_id).unwrap().expect("indexed");
        assert_eq!(artifact.artifact_type, "web.page");
        assert_eq!(artifact.title, "A good discussion");

        let path = artifact_files(root.path()).pop().unwrap();
        let file = fs::read_to_string(&path).unwrap();
        assert!(file.contains("type: \"web.page\""));
        // The artifact file is nested 3 directories under the vault root
        // (artifacts/<type>/<site>/<file>.md); a root-relative summary_path
        // must resolve back up through all of them, or the Markdown link is
        // broken from the artifact file's own location.
        assert!(
            path.strip_prefix(root.path()).unwrap().components().count() == 4,
            "expected artifacts/<type>/<site>/<file>.md, got {path:?}"
        );
        assert!(file.contains(
            "Summary: [discussion, high importance](<../../../pages/hn-item.summary.md>)"
        ));
        assert!(file.contains("Summarized: A good discussion"));

        // Re-summarizing the same URL updates the one artifact, not a second.
        let mut again = page_summary(url, &rfc3339_seconds_ago(0));
        again.title = "An even better discussion".into();
        assert!(matches!(
            capture.record_summary(policy, &again).0,
            crate::capture::CaptureOutcome::Persisted { .. }
        ));
        index.reindex().unwrap();
        assert_eq!(artifact_files(root.path()).len(), 1);
        let updated = index.artifact(&expected_id).unwrap().expect("indexed");
        assert_eq!(updated.title, "An even better discussion");
    }

    #[test]
    fn record_summary_returns_the_event_it_wrote() {
        use crate::capture::{CaptureOutcome, CapturePolicy, CaptureStore};

        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let capture = CaptureStore::open_in(&storage(root.path()), None, cache.path()).unwrap();
        let policy = CapturePolicy {
            enabled: true,
            origins: &[],
            strip_params: &[],
        };
        let url = "https://jira.example/browse/PROJ-1";
        let web = page_summary(url, &rfc3339_seconds_ago(5));
        let adapter = crate::summary::PageSummary {
            event_id: uuid::Uuid::new_v4().to_string(),
            record_key: brauser_protocol::RecordKey {
                kind: brauser_protocol::RecordKeyKind::Artifact,
                value: "jira:PROJ-1".into(),
            },
            artifact_type: "jira.issue".into(),
            harness: brauser_protocol::HarnessId::Claude,
            ..page_summary(url, &rfc3339_seconds_ago(4))
        };
        for summary in [web, adapter] {
            let (CaptureOutcome::Persisted { relative_path }, Some(event)) =
                capture.record_summary(policy, &summary)
            else {
                panic!("expected a persisted summary");
            };
            let log = fs::read_to_string(root.path().join(relative_path)).unwrap();
            let replayed = parse_log(&log)
                .into_iter()
                .filter_map(|replayed| match replayed {
                    CachedEvent::Summary(replayed) => Some(replayed),
                    _ => None,
                })
                .find(|replayed| replayed.event_id == event.event_id)
                .expect("the written summary replays");
            assert_eq!(replayed, event);
            assert_eq!(event.record_key, summary.record_key);
            assert_eq!(event.harness, summary.harness);
        }
    }

    #[test]
    fn the_artifact_summary_link_is_percent_encoded() {
        use crate::capture::{CapturePolicy, CaptureStore};

        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        // #82 fixed the folder names, so the reserved character sits in the
        // file name inside the fixed summaries folder.
        let config = crate::layout::fixed(&storage(root.path()));
        let capture = CaptureStore::open_in(&config, None, cache.path()).unwrap();
        let index = ArtifactIndex::open_in(&config, None, cache.path()).unwrap();
        let policy = CapturePolicy {
            enabled: true,
            origins: &[],
            strip_params: &[],
        };
        let url = "https://news.ycombinator.com/item?id=50251344";
        let mut summary = page_summary(url, &rfc3339_seconds_ago(5));
        summary.summary_path = format!("{}/hn item.summary.md", crate::layout::SUMMARIES_DIR);

        assert!(matches!(
            capture.record_summary(policy, &summary).0,
            crate::capture::CaptureOutcome::Persisted { .. }
        ));
        assert_eq!(index.reindex().unwrap().written, 1);

        let file = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert!(file.contains(
            "Summary: [discussion, high importance](<../../../summaries/hn%20item.summary.md>)"
        ));
    }

    #[test]
    fn a_summary_joins_the_m1_legacy_visit_of_the_same_url() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://news.ycombinator.com/item?id=50251344";
        let artifact_id = crate::summary::web_page_artifact_id(url).unwrap();
        let url_hash = sha256_hex(url.as_bytes());

        let legacy_line = format!(
            "\n- 2026-10-20T10:00:00Z — A good discussion — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
            sha256_hex("legacy-marker".as_bytes()),
        );

        let event = SummaryEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            artifact_id: artifact_id.clone(),
            artifact_type: "web.page".into(),
            url: url.into(),
            title: "A good discussion".into(),
            occurred_at: "2026-10-26T18:00:00Z".into(),
            document_type: "discussion".into(),
            category: "reading".into(),
            importance: brauser_protocol::ImportanceLevel::High,
            summary_path: "pages/hn-item.summary.md".into(),
            record_key: crate::vault::url_record_key(url).unwrap(),
            harness: brauser_protocol::HarnessId::Codex,
        };
        let payload = hex::encode(serde_json::to_vec(&event).unwrap());
        let summary_line = format!(
            "<!-- brauser:visit id={} url={url_hash} at={} kind=summary data={payload} -->\n",
            sha256_hex(event.event_id.as_bytes()),
            event.occurred_at,
        );

        let dir = root.path().join("log/2026/10");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("2026-10-20.md"),
            format!("{LOG_HEADER}\n{legacy_line}"),
        )
        .unwrap();
        fs::write(
            dir.join("2026-10-26.md"),
            format!("{LOG_HEADER}\n{summary_line}"),
        )
        .unwrap();

        index.reindex().unwrap();
        assert_eq!(artifact_files(root.path()).len(), 1, "one shared artifact");
        let artifact = index.artifact(&artifact_id).unwrap().expect("indexed");
        assert_eq!(artifact.artifact_type, "web.page");
    }

    #[test]
    fn forged_summary_markers_are_ignored() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://news.ycombinator.com/item?id=50251344";
        let url_hash = sha256_hex(url.as_bytes());
        let event = SummaryEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            artifact_id:
                "web:evil.example/0000000000000000000000000000000000000000000000000000000000000000"
                    .into(),
            artifact_type: "web.page".into(),
            url: url.into(),
            title: "A good discussion".into(),
            occurred_at: "2026-10-26T18:00:00Z".into(),
            document_type: "discussion".into(),
            category: "reading".into(),
            importance: brauser_protocol::ImportanceLevel::High,
            summary_path: "pages/hn-item.summary.md".into(),
            record_key: crate::vault::url_record_key(url).unwrap(),
            harness: brauser_protocol::HarnessId::Codex,
        };
        let payload = hex::encode(serde_json::to_vec(&event).unwrap());
        let line = format!(
            "<!-- brauser:visit id={} url={url_hash} at={} kind=summary data={payload} -->\n",
            sha256_hex(event.event_id.as_bytes()),
            event.occurred_at,
        );
        let dir = root.path().join("log/2026/10");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("2026-10-26.md"), format!("{LOG_HEADER}\n{line}")).unwrap();

        let report = index.reindex().unwrap();
        assert_eq!(report.written, 0);
        assert!(!root.path().join("artifacts").exists());
    }

    fn web_summary(url: &str, at: &str) -> SummaryEvent {
        SummaryEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            artifact_id: crate::summary::web_page_artifact_id(url).unwrap(),
            artifact_type: "web.page".into(),
            url: url.into(),
            title: "A good discussion".into(),
            occurred_at: at.into(),
            document_type: "discussion".into(),
            category: "reading".into(),
            importance: brauser_protocol::ImportanceLevel::High,
            summary_path: "summaries/hn-item.summary.md".into(),
            record_key: crate::vault::url_record_key(url).unwrap(),
            harness: brauser_protocol::HarnessId::Codex,
        }
    }

    fn expected_view(event: &SummaryEvent) -> brauser_protocol::SummaryView {
        brauser_protocol::SummaryView {
            summarized_at: event.occurred_at.clone(),
            document_type: event.document_type.clone(),
            category: event.category.clone(),
            importance: event.importance,
            summary_path: event.summary_path.clone(),
            harness: event.harness,
        }
    }

    #[test]
    fn recent_lists_a_summary_only_page_with_its_summary() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://news.ycombinator.com/item?id=50251344";
        let event = web_summary(url, "2026-10-15T12:00:00-07:00");
        index.record_summary(&event).unwrap();

        let (recent, types, truncated) = index.recent(None, None, 32).unwrap();
        assert_eq!(types, ["web.page"]);
        assert!(!truncated);
        assert_eq!(recent.len(), 1);
        let row = &recent[0];
        assert_eq!(row.local_date, "2026-10-15");
        assert_eq!(row.artifact_id, event.artifact_id);
        assert_eq!(row.artifact_type, "web.page");
        assert_eq!(row.title, "A good discussion");
        assert_eq!(
            (row.opens, row.focused_seconds, row.background_seconds),
            (0, 0, 0)
        );
        assert_eq!(row.summary, Some(expected_view(&event)));
        assert_eq!(
            row.summary.as_ref().unwrap().importance,
            brauser_protocol::ImportanceLevel::High
        );
    }

    #[test]
    fn a_recent_row_carries_only_its_own_days_latest_summary() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let measured = activity(
            "00000000-0000-4000-8000-000000000031",
            "PROJ-1",
            "Ticket",
            "2026-10-15T12:00:00Z",
        );
        index.record_activity(&measured, &[]).unwrap();
        let summary = |at: &str, level| SummaryEvent {
            importance: level,
            ..adapter_summary(&measured.artifact_id, "work.item", &measured.url, at)
        };
        let early = summary(
            "2026-10-16T08:00:00Z",
            brauser_protocol::ImportanceLevel::Low,
        );
        let late = summary(
            "2026-10-16T09:00:00Z",
            brauser_protocol::ImportanceLevel::Medium,
        );
        // Recorded out of order: the later summary still wins.
        index.record_summary(&late).unwrap();
        index.record_summary(&early).unwrap();

        let (recent, _, _) = index.recent(None, None, 32).unwrap();
        let days = recent
            .iter()
            .map(|row| (row.local_date.as_str(), row.summary.clone()))
            .collect::<Vec<_>>();
        assert_eq!(
            days,
            [
                ("2026-10-16", Some(expected_view(&late))),
                ("2026-10-15", None)
            ]
        );
        assert_eq!(
            index.summary_for(&measured.artifact_id).unwrap(),
            Some(expected_view(&late))
        );
        // The panel's single-load lookup picks the same latest summary.
        let (artifact, latest) = index.artifact_with_summary(&measured.artifact_id).unwrap();
        assert_eq!(artifact.unwrap().artifact_id, measured.artifact_id);
        assert_eq!(latest, Some(expected_view(&late)));
        assert_eq!(index.summary_for("work:example.com/none").unwrap(), None);
        assert_eq!(
            index
                .artifact_with_summary("work:example.com/none")
                .unwrap(),
            (None, None)
        );
    }

    /// A latest summary with no view (an unparseable, over-long time) does
    /// not hide an earlier one that renders.
    #[test]
    fn a_latest_summary_without_a_view_falls_back_to_the_newest_that_renders() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://news.ycombinator.com/item?id=50251344";
        let valid = web_summary(url, "2026-10-16T08:00:00Z");
        let broken = web_summary(url, "2026-10-16T23:59:59 and later, in the evening, local");
        assert_eq!(summary_view(&broken), None);
        assert_eq!(
            timestamp_order(&broken.occurred_at, &valid.occurred_at),
            std::cmp::Ordering::Greater
        );
        index.record_summary(&valid).unwrap();
        index.record_summary(&broken).unwrap();

        assert_eq!(
            index.summary_for(&valid.artifact_id).unwrap(),
            Some(expected_view(&valid))
        );
        let (_, latest) = index.artifact_with_summary(&valid.artifact_id).unwrap();
        assert_eq!(latest, Some(expected_view(&valid)));
        let (recent, _, _) = index.recent(None, None, 32).unwrap();
        let row = recent
            .iter()
            .find(|row| row.artifact_id == valid.artifact_id)
            .expect("the summarized page is listed");
        assert_eq!(row.summary, Some(expected_view(&valid)));
    }

    #[test]
    fn a_summary_view_keeps_summarized_at_within_the_wire_bound() {
        let url = "https://news.ycombinator.com/item?id=50251344";
        let short = web_summary(url, "2026-10-26T14:05:00-07:00");
        assert_eq!(
            summary_view(&short).unwrap().summarized_at,
            "2026-10-26T14:05:00-07:00"
        );
        let long = web_summary(url, "2026-10-26T14:05:00.123456789-07:00");
        assert_eq!(
            summary_view(&long).unwrap().summarized_at,
            "2026-10-26T14:05:00-07:00"
        );
        // Too long and not a timestamp: no view rather than an empty time.
        let unparseable = web_summary(url, "2026-10-26 sometime in the afternoon, local");
        assert_eq!(summary_view(&unparseable), None);
    }

    #[test]
    fn worklog_names_summaries_without_counting_them_as_opened() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://news.ycombinator.com/item?id=50251344";
        index
            .record_summary(&web_summary(url, "2026-10-15T12:00:00Z"))
            .unwrap();

        let (text, count) = index
            .worklog(
                parse_date("2026-10-15").unwrap(),
                parse_date("2026-10-15").unwrap(),
            )
            .unwrap();
        assert_eq!(count, 1);
        // A summary is a distinct, named action: it is neither an open nor
        // the generic "seen" wording of an artifact with no measured time.
        assert!(
            text.starts_with(
                "Opened 0 artifacts, focused on 0 left open from an earlier day, and summarized 1 page.\n"
            ),
            "{text}"
        );
        assert!(text.contains("\nSummarized: A good discussion — discussion, high importance"));
        assert!(!text.contains("seen"));
    }

    #[test]
    fn an_m1_visit_and_a_later_summary_open_the_page_on_one_day() {
        // brwr-hnr: an M1 legacy visit on day A and a summary on day B are
        // one artifact opened on one day, and the summary is never an open.
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://news.ycombinator.com/item?id=50251344";
        let url_hash = sha256_hex(url.as_bytes());
        let legacy_line = format!(
            "\n- 2026-10-20T10:00:00Z — A good discussion — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
            sha256_hex("legacy-marker-hnr".as_bytes()),
        );
        let summary = web_summary(url, "2026-10-26T18:00:00Z");
        let dir = root.path().join("log/2026/10");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("2026-10-20.md"),
            format!("{LOG_HEADER}\n{legacy_line}"),
        )
        .unwrap();
        fs::write(
            dir.join("2026-10-26.md"),
            format!("{LOG_HEADER}\n{}", summary_marker(&summary)),
        )
        .unwrap();

        index.reindex().unwrap();
        let artifact = index
            .artifact(&summary.artifact_id)
            .unwrap()
            .expect("indexed");
        assert_eq!((artifact.opens, artifact.days_opened), (1, 1));
        assert_eq!(
            (artifact.first_seen.as_str(), artifact.last_seen.as_str()),
            ("2026-10-20", "2026-10-26")
        );
        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert!(
            markdown.contains("  opens: 1\n  days_opened: 1\n"),
            "{markdown}"
        );

        let worklog = |start: &str, end: &str| {
            index
                .worklog(parse_date(start).unwrap(), parse_date(end).unwrap())
                .unwrap()
        };
        let (both, count) = worklog("2026-10-20", "2026-10-26");
        assert_eq!(count, 1);
        assert_eq!(
            both,
            "Opened 1 artifact, focused on 0 left open from an earlier day, and summarized 1 page.\n\
             Summarized: A good discussion — discussion, high importance — opened 1×"
        );
        let (summary_day, _) = worklog("2026-10-26", "2026-10-26");
        assert_eq!(
            summary_day,
            "Opened 0 artifacts, focused on 0 left open from an earlier day, and summarized 1 page.\n\
             Summarized: A good discussion — discussion, high importance"
        );
        let (visit_day, _) = worklog("2026-10-20", "2026-10-20");
        assert!(
            visit_day.starts_with(
                "Opened 1 artifact, and focused on 0 left open from an earlier day.\n"
            ),
            "{visit_day}"
        );
    }

    #[test]
    fn worklog_keeps_measures_when_a_summary_lands_on_an_already_measured_artifact() {
        // Exercises the future adapter-attached-summary case (spec §7.2's
        // "the adapter does recognize the page" branch, not wired by any
        // caller yet): the summary wording must not silently drop real
        // measured activity on the same artifact.
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let event = activity(
            "00000000-0000-4000-8000-000000000020",
            "PROJ-1",
            "Ticket",
            "2026-10-15T12:00:00Z",
        );
        index.record_activity(&event, &[]).unwrap();
        index
            .record_summary(&SummaryEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                artifact_id: event.artifact_id.clone(),
                artifact_type: event.artifact_type.clone(),
                url: event.url.clone(),
                title: "Ticket".into(),
                occurred_at: "2026-10-15T13:00:00Z".into(),
                document_type: "ticket".into(),
                category: "work".into(),
                importance: brauser_protocol::ImportanceLevel::High,
                summary_path: "pages/proj-1.summary.md".into(),
                record_key: brauser_protocol::RecordKey {
                    kind: brauser_protocol::RecordKeyKind::Artifact,
                    value: event.artifact_id.clone(),
                },
                harness: brauser_protocol::HarnessId::Claude,
            })
            .unwrap();

        let (text, count) = index
            .worklog(
                parse_date("2026-10-15").unwrap(),
                parse_date("2026-10-15").unwrap(),
            )
            .unwrap();
        assert_eq!(count, 1);
        assert!(
            text.starts_with(
                "Opened 1 artifact, focused on 0 left open from an earlier day, and summarized 1 page.\n"
            ),
            "{text}"
        );
        assert!(text.contains("Summarized: Ticket — ticket, high importance"));
        assert!(text.contains("focused 2 min"), "{text}");
    }

    #[test]
    fn a_version_1_state_cache_is_rescanned() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let url = "https://news.ycombinator.com/item?id=50251344";
        let url_hash = sha256_hex(url.as_bytes());
        let event = SummaryEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            artifact_id: crate::summary::web_page_artifact_id(url).unwrap(),
            artifact_type: "web.page".into(),
            url: url.into(),
            title: "A good discussion".into(),
            occurred_at: "2026-10-26T18:00:00Z".into(),
            document_type: "discussion".into(),
            category: "reading".into(),
            importance: brauser_protocol::ImportanceLevel::High,
            summary_path: "pages/hn-item.summary.md".into(),
            record_key: crate::vault::url_record_key(url).unwrap(),
            harness: brauser_protocol::HarnessId::Codex,
        };
        let payload = hex::encode(serde_json::to_vec(&event).unwrap());
        let line = format!(
            "<!-- brauser:visit id={} url={url_hash} at={} kind=summary data={payload} -->\n",
            sha256_hex(event.event_id.as_bytes()),
            event.occurred_at,
        );
        let dir = root.path().join("log/2026/10");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("2026-10-26.md"), format!("{LOG_HEADER}\n{line}")).unwrap();

        // A cache written by a build that predates `CachedEvent::Summary`
        // (version 1) must never be trusted as-is: it cannot contain this
        // summary, so loading it unchanged would hide it until a manual
        // reindex. Write one by hand and confirm it is rescanned instead.
        let stale_cache_dir = cache.path();
        fs::create_dir_all(stale_cache_dir).unwrap();
        let stale_state = serde_json::json!({
            "version": 1,
            "root": root.path().to_string_lossy(),
            "root_identity": open_selected_root(root.path(), None).unwrap().1,
            "log_dir": "log",
            "artifacts_dir": "artifacts",
            "events": {},
            "dirty": [],
        });
        fs::write(
            stale_cache_dir.join("index-state.json"),
            serde_json::to_vec(&stale_state).unwrap(),
        )
        .unwrap();

        let index = index(&root, &cache);
        let report = index.regenerate_dirty().unwrap();
        assert_eq!(
            report.written, 1,
            "a v1 cache must be rescanned, not trusted"
        );
        assert_eq!(artifact_files(root.path()).len(), 1);
    }

    /// A summary attached to an adapter artifact, as `CaptureStore::record_summary`
    /// writes it for an `Artifact` record key.
    fn adapter_summary(
        artifact_id: &str,
        artifact_type: &str,
        url: &str,
        at: &str,
    ) -> SummaryEvent {
        SummaryEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            artifact_id: artifact_id.into(),
            artifact_type: artifact_type.into(),
            url: url.into(),
            title: "A good discussion".into(),
            occurred_at: at.into(),
            document_type: "discussion".into(),
            category: "reading".into(),
            importance: brauser_protocol::ImportanceLevel::High,
            summary_path: "summaries/a.summary.md".into(),
            record_key: brauser_protocol::RecordKey {
                kind: brauser_protocol::RecordKeyKind::Artifact,
                value: artifact_id.into(),
            },
            harness: brauser_protocol::HarnessId::Claude,
        }
    }

    fn summary_marker(event: &SummaryEvent) -> String {
        format!(
            "<!-- brauser:visit id={} url={} at={} kind=summary data={} -->\n",
            sha256_hex(event.event_id.as_bytes()),
            sha256_hex(event.url.as_bytes()),
            event.occurred_at,
            hex::encode(serde_json::to_vec(event).unwrap()),
        )
    }

    fn timeline(markdown: &str) -> &str {
        let start = markdown.find("## Timeline\n\n").expect("a Timeline") + "## Timeline\n\n".len();
        &markdown[start..]
    }

    #[test]
    fn timeline_interleaves_summaries_and_rollups_by_date() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let day1 = activity(
            "00000000-0000-4000-8000-000000000101",
            "MIX",
            "Item",
            "2026-10-01T12:00:00Z",
        );
        let day5 = ActivityEvent {
            event_id: "00000000-0000-4000-8000-000000000105".into(),
            occurred_at: "2026-10-05T12:00:00Z".into(),
            ..day1.clone()
        };
        index.record_activities([&day1, &day5], &[]).unwrap();
        let mut on_day3 = adapter_summary(
            &day1.artifact_id,
            &day1.artifact_type,
            &day1.url,
            "2026-10-03T08:30:00Z",
        );
        on_day3.title = "Day three".into();
        let mut on_day5 = adapter_summary(
            &day1.artifact_id,
            &day1.artifact_type,
            &day1.url,
            "2026-10-05T07:00:00Z",
        );
        on_day5.title = "Day five".into();
        on_day5.importance = brauser_protocol::ImportanceLevel::Low;
        index.record_summary(&on_day3).unwrap();
        index.record_summary(&on_day5).unwrap();
        assert_eq!(index.regenerate_dirty().unwrap().written, 1);

        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert_eq!(
            timeline(&markdown),
            "- 2026-10-05 07:00 · Summarized: Day five — discussion, low importance\n\
             - 2026-10-05 · focused 2 min · 1 min in background · opened 1×\n\
             - 2026-10-03 08:30 · Summarized: Day three — discussion, high importance\n\
             - 2026-10-01 · focused 2 min · 1 min in background · opened 1×\n",
            "{markdown}"
        );
        // A summary-only day is not a day opened.
        assert!(
            markdown.contains("  opens: 2\n  days_opened: 2\n"),
            "{markdown}"
        );
        assert_eq!(
            index
                .artifact(&day1.artifact_id)
                .unwrap()
                .unwrap()
                .days_opened,
            2
        );
    }

    #[test]
    fn timeline_merges_summaries_across_offsets_by_their_own_date() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let day5 = activity(
            "00000000-0000-4000-8000-000000000111",
            "TRAVEL",
            "Item",
            "2026-10-05T12:00:00Z",
        );
        let day6 = ActivityEvent {
            event_id: "00000000-0000-4000-8000-000000000112".into(),
            occurred_at: "2026-10-06T12:00:00Z".into(),
            ..day5.clone()
        };
        index.record_activities([&day5, &day6], &[]).unwrap();
        // The later instant has the earlier local date.
        let mut late_evening = adapter_summary(
            &day5.artifact_id,
            &day5.artifact_type,
            &day5.url,
            "2026-10-05T23:30:00-07:00",
        );
        late_evening.title = "Late evening".into();
        let mut early_morning = adapter_summary(
            &day5.artifact_id,
            &day5.artifact_type,
            &day5.url,
            "2026-10-06T01:00:00+09:00",
        );
        early_morning.title = "Early morning".into();
        index.record_summary(&late_evening).unwrap();
        index.record_summary(&early_morning).unwrap();
        index.regenerate_dirty().unwrap();

        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert_eq!(
            timeline(&markdown),
            "- 2026-10-06 01:00 · Summarized: Early morning — discussion, high importance\n\
             - 2026-10-06 · focused 2 min · 1 min in background · opened 1×\n\
             - 2026-10-05 23:30 · Summarized: Late evening — discussion, high importance\n\
             - 2026-10-05 · focused 2 min · 1 min in background · opened 1×\n",
            "{markdown}"
        );
    }

    #[test]
    fn a_timeline_merges_summaries_with_m1_legacy_visit_lines() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://news.ycombinator.com/item?id=50251344";
        let url_hash = sha256_hex(url.as_bytes());
        let legacy_line = format!(
            "\n- 2026-10-20T10:00:00Z — A good discussion — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
            sha256_hex("legacy-marker-merge".as_bytes()),
        );
        let summary = |at: &str| {
            let mut summary = adapter_summary(
                &crate::summary::web_page_artifact_id(url).unwrap(),
                "web.page",
                url,
                at,
            );
            summary.record_key = crate::vault::url_record_key(url).unwrap();
            summary
        };
        let before = summary("2026-10-19T09:00:00Z");
        let after = summary("2026-10-26T18:05:59Z");
        let dir = root.path().join("log/2026/10");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("2026-10-19.md"),
            format!("{LOG_HEADER}\n{}", summary_marker(&before)),
        )
        .unwrap();
        fs::write(
            dir.join("2026-10-20.md"),
            format!("{LOG_HEADER}\n{legacy_line}"),
        )
        .unwrap();
        fs::write(
            dir.join("2026-10-26.md"),
            format!("{LOG_HEADER}\n{}", summary_marker(&after)),
        )
        .unwrap();

        assert_eq!(index.reindex().unwrap().written, 1);
        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert_eq!(
            timeline(&markdown),
            "- 2026-10-26 18:05 · Summarized: A good discussion — discussion, high importance\n\
             - 2026-10-20 · opened 1×\n\
             - 2026-10-19 09:00 · Summarized: A good discussion — discussion, high importance\n",
            "{markdown}"
        );
    }

    #[test]
    fn a_timeline_summary_line_has_the_title_and_no_seconds() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://news.ycombinator.com/item?id=50251344";
        let mut event = adapter_summary(
            &crate::summary::web_page_artifact_id(url).unwrap(),
            "web.page",
            url,
            "2026-10-20T09:15:42-07:00",
        );
        event.record_key = crate::vault::url_record_key(url).unwrap();
        index.record_summary(&event).unwrap();
        index.regenerate_dirty().unwrap();

        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        // The time stays in the event's own offset, not UTC.
        assert_eq!(
            timeline(&markdown),
            "- 2026-10-20 09:15 · Summarized: A good discussion — discussion, high importance\n",
            "{markdown}"
        );
    }

    #[test]
    fn timeline_summary_fields_are_escaped_and_capped_at_twenty_lines() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://jira.example/browse/PROJ-9";
        for day in 1..=22 {
            let mut event = adapter_summary(
                "jira:jira.example/PROJ-9",
                "jira.issue",
                url,
                &format!("2026-09-{day:02}T10:00:00Z"),
            );
            event.title = "![x](https://e.example/a.png) `t`".into();
            event.document_type = "a`b".into();
            index.record_summary(&event).unwrap();
        }
        index.regenerate_dirty().unwrap();

        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        let lines = timeline(&markdown).lines().collect::<Vec<_>>();
        assert_eq!(lines.len(), 21, "{markdown}");
        assert_eq!(
            lines[0],
            format!(
                "- 2026-09-22 10:00 · Summarized: {} — {}, high importance",
                markdown::inline(
                    "![x](https://e.example/a.png) `t`",
                    MAX_RENDERED_FIELD_CHARS
                ),
                markdown::inline("a`b", MAX_RENDERED_FIELD_CHARS),
            )
        );
        assert!(!lines[0].contains('`'), "{}", lines[0]);
        assert!(lines[19].starts_with("- 2026-09-03 10:00 · Summarized:"));
        assert_eq!(lines[20], "- 2 earlier summaries");
    }

    #[test]
    fn a_summary_on_an_adapter_artifact_without_activity_creates_that_row() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let summary = adapter_summary(
            "jira:jira.example/PROJ-1",
            "jira.issue",
            "https://jira.example/browse/PROJ-1",
            "2026-10-20T09:00:00Z",
        );
        index.record_summary(&summary).unwrap();
        let row = index
            .artifact(&summary.artifact_id)
            .unwrap()
            .expect("a row");
        assert_eq!(row.artifact_type, "jira.issue");
        assert_eq!(row.title, "A good discussion");
        // A summary is not measured activity.
        assert_eq!((row.opens, row.focused_seconds, row.days_opened), (0, 0, 0));
        index.regenerate_dirty().unwrap();
        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert!(
            markdown.contains("  opens: 0\n  days_opened: 0\n"),
            "{markdown}"
        );

        let later = ActivityEvent {
            artifact_id: summary.artifact_id.clone(),
            artifact_type: "jira.issue".into(),
            url: "https://jira.example/browse/PROJ-1?focusedCommentId=2".into(),
            ..activity(
                "00000000-0000-4000-8000-000000000201",
                "PROJ-1",
                "PROJ-1 renamed",
                "2026-10-21T09:00:00Z",
            )
        };
        index.record_activity(&later, &[]).unwrap();
        index.regenerate_dirty().unwrap();
        assert_eq!(artifact_files(root.path()).len(), 1);
        let row = index.artifact(&summary.artifact_id).unwrap().unwrap();
        assert_eq!(row.artifact_type, "jira.issue");
        assert_eq!(row.title, "PROJ-1 renamed");
        assert_eq!(row.url, later.url);
        assert_eq!((row.opens, row.days_opened), (1, 1));
    }

    #[test]
    fn an_adapter_summary_is_a_note_key_candidate() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://jira.example/browse/PROJ-1";
        let summary = adapter_summary(
            "jira:jira.example/PROJ-1",
            "jira.issue",
            url,
            "2026-10-20T09:00:00Z",
        );
        index.record_summary(&summary).unwrap();
        assert_eq!(index.artifact_urls(&summary.artifact_id).unwrap(), [url]);
    }

    #[test]
    fn a_web_summary_url_stays_with_its_own_web_artifact() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let adapter_url = "https://jira.example/browse/PROJ-1";
        let web_url = "https://jira.example/browse/PROJ-1?view=print";
        let adapter = ActivityEvent {
            artifact_id: "jira:jira.example/PROJ-1".into(),
            artifact_type: "jira.issue".into(),
            url: adapter_url.into(),
            modes: Vec::new(),
            ..activity(
                "00000000-0000-4000-8000-000000000301",
                "PROJ-1",
                "PROJ-1",
                "2026-10-20T09:00:00Z",
            )
        };
        index.record_activity(&adapter, &[]).unwrap();
        let web_id = crate::summary::web_page_artifact_id(web_url).unwrap();
        let mut web = adapter_summary(&web_id, "web.page", web_url, "2026-10-20T10:00:00Z");
        web.record_key = crate::vault::url_record_key(web_url).unwrap();
        index.record_summary(&web).unwrap();

        // The web summary's URL is indexed under its own `web:` artifact,
        // not the adapter artifact on the same host. This checks the index
        // side only; `resolve_note_context`'s `names_artifact` filter is
        // what keeps a URL from becoming another artifact's note key.
        assert_eq!(
            index.artifact_urls(&adapter.artifact_id).unwrap(),
            [adapter_url]
        );
        assert_eq!(index.artifact_urls(&web_id).unwrap(), [web_url]);
    }

    #[test]
    fn reusing_a_summary_event_id_with_other_content_bails() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let first = adapter_summary(
            "jira:jira.example/PROJ-1",
            "jira.issue",
            "https://jira.example/browse/PROJ-1",
            "2026-10-20T09:00:00Z",
        );
        index.record_summary(&first).unwrap();
        // An identical retry is accepted.
        index.record_summary(&first).unwrap();
        let mut forged = first.clone();
        forged.title = "Something else".into();
        let error = index.record_summary(&forged).unwrap_err().to_string();
        assert!(error.contains("cannot be reused"), "{error}");
        assert_eq!(
            index.artifact(&first.artifact_id).unwrap().unwrap().title,
            "A good discussion"
        );
    }

    #[test]
    fn a_version_5_state_cache_is_migrated() {
        assert_eq!(STATE_VERSION, 6);
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let event = activity(
            "00000000-0000-4000-8000-000000000401",
            "V5",
            "Version five",
            "2026-10-20T12:00:00Z",
        );
        write_log(root.path(), "2026-10-20.md", &[&event]);
        assert_eq!(index.reindex().unwrap().written, 1);
        let path = artifact_files(root.path()).pop().unwrap();
        let current = fs::read_to_string(&path).unwrap();
        // The clean cache a version 5 build left. It can hold no summary
        // (#59 had no caller), so nothing it derived renders differently.
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
        state["version"] = 5.into();
        fs::write(index.state_path(), serde_json::to_vec(&state).unwrap()).unwrap();

        // Migrated in place: no log is reread and no file is rewritten.
        LOG_BYTES_READ.with(|count| count.set(0));
        assert_eq!(index.regenerate_dirty().unwrap().written, 0);
        assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);
        assert_eq!(fs::read_to_string(&path).unwrap(), current);
        assert_eq!(
            read_state(&index.state_path()).unwrap().unwrap().version,
            STATE_VERSION
        );
        assert!(index.artifact(&event.artifact_id).unwrap().is_some());
    }

    #[test]
    fn version_5_catch_up_progress_resumes_and_older_progress_restarts() {
        for (version, resumes) in [(5, true), (4, false), (3, false)] {
            let root = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let index = index(&root, &cache);
            let events = (1..=3)
                .map(|day| {
                    activity(
                        &format!("00000000-0000-4000-8000-{:012}", 500 + day),
                        &format!("P{day}"),
                        &format!("Item {day}"),
                        &format!("2026-10-1{day}T12:00:00Z"),
                    )
                })
                .collect::<Vec<_>>();
            let mut size = 0;
            for (day, event) in events.iter().enumerate() {
                size = fs::metadata(write_log(
                    root.path(),
                    &format!("2026-10-1{}.md", day + 1),
                    &[event],
                ))
                .unwrap()
                .len();
            }
            // One pass reads two of the three logs and saves its progress.
            SCAN_LIMIT.with(|value| value.set(size * 5 / 2));
            LOG_BYTES_READ.with(|count| count.set(0));
            assert!(index.recent(None, None, 32).is_err());
            assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 2 * size);
            let mut pending: serde_json::Value =
                serde_json::from_slice(&fs::read(index.pending_path()).unwrap()).unwrap();
            assert_eq!(pending["state"]["version"], STATE_VERSION);
            pending["state"]["version"] = version.into();
            fs::write(index.pending_path(), serde_json::to_vec(&pending).unwrap()).unwrap();

            // Version 5 progress migrates and resumes after the logs it read;
            // progress from any older format is discarded and restarts.
            LOG_BYTES_READ.with(|count| count.set(0));
            let result = index.recent(None, None, 32);
            let read = LOG_BYTES_READ.with(|count| count.get());
            if resumes {
                assert_eq!((result.unwrap().0.len(), read), (3, size), "v{version}");
            } else {
                assert!(result.is_err(), "v{version}");
                assert_eq!(read, 2 * size, "v{version}");
                assert_eq!(index.recent(None, None, 32).unwrap().0.len(), 3);
            }
            assert!(!index.pending_path().exists());
            assert_eq!(
                read_state(&index.state_path()).unwrap().unwrap().version,
                STATE_VERSION
            );
            SCAN_LIMIT.with(|value| value.set(MAX_SCAN_BYTES));
        }
    }

    /// Run SQLite's own integrity check on `path`, then one query whose
    /// rows are printed one per line.
    fn sqlite_check(path: &Path, query: &str) -> String {
        let output = Command::new("python3")
            .args([
                "-c",
                "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); print(c.execute('pragma integrity_check').fetchone()[0]); [print(*r) for r in c.execute(sys.argv[2])]",
            ])
            .arg(path)
            .arg(query)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // Python writes \r\n on Windows.
        String::from_utf8(output.stdout)
            .unwrap()
            .replace("\r\n", "\n")
    }

    #[test]
    fn a_logged_title_over_the_capture_limit_is_skipped_without_stopping_the_index() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        // Capture refuses titles over 2048 bytes; an edited or crafted log
        // line can still carry one inside an otherwise valid marker.
        let crafted = activity(
            "00000000-0000-4000-8000-0000000000a1",
            "CRAFTED",
            &"T".repeat(100_000),
            "2026-10-15T10:00:00Z",
        );
        let longest = activity(
            "00000000-0000-4000-8000-0000000000a2",
            "LONGEST",
            &"L".repeat(2048),
            "2026-10-15T11:00:00Z",
        );
        let just_over = activity(
            "00000000-0000-4000-8000-0000000000a4",
            "OVER",
            &"O".repeat(2049),
            "2026-10-15T11:30:00Z",
        );
        write_log(
            root.path(),
            "2026-10-15.md",
            &[&crafted, &longest, &just_over],
        );
        assert_eq!(index.reindex().unwrap().written, 1);
        assert!(index.artifact(&crafted.artifact_id).unwrap().is_none());
        assert!(index.artifact(&just_over.artifact_id).unwrap().is_none());
        assert!(index.artifact(&longest.artifact_id).unwrap().is_some());
        // Later deliveries keep updating index.db.
        let later = activity(
            "00000000-0000-4000-8000-0000000000a3",
            "LATER",
            "Later",
            "2026-10-15T12:00:00Z",
        );
        index.record_activity(&later, &[]).unwrap();
        assert_eq!(
            sqlite_check(
                &index.database_path(),
                "select title from artifacts order by title"
            ),
            format!("ok\n{}\nLater\n", "L".repeat(2048))
        );
    }

    #[test]
    fn an_oversize_cached_row_is_left_out_of_index_db_without_stopping_publication() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        // Log parsing bounds titles, so only a hand-edited or otherwise
        // unexpected cache can hold an event whose rows exceed one SQLite
        // page. Publication must still survive it.
        let oversize = activity(
            "00000000-0000-4000-8000-0000000000b1",
            "OVERSIZE",
            &"T".repeat(100_000),
            "2026-10-15T10:00:00Z",
        );
        let good = activity(
            "00000000-0000-4000-8000-0000000000b2",
            "GOOD",
            "Good",
            "2026-10-15T11:00:00Z",
        );
        let mut state = index.empty_state();
        for event in [&oversize, &good] {
            state.ingest(CachedEvent::Activity(event.clone()), None, false);
        }
        index.publish_state_and_database(&state).unwrap();
        let later = activity(
            "00000000-0000-4000-8000-0000000000b3",
            "LATER",
            "Later",
            "2026-10-15T12:00:00Z",
        );
        index.record_activity(&later, &[]).unwrap();
        assert_eq!(
            sqlite_check(
                &index.database_path(),
                "select title from visits order by title"
            ),
            "ok\nGood\nLater\n"
        );
        assert_eq!(
            sqlite_check(&index.database_path(), "select count(*) from artifacts"),
            "ok\n2\n"
        );
        // Only index.db leaves the row out; the derived history keeps it.
        assert!(index.artifact(&oversize.artifact_id).unwrap().is_some());
        // A version 4 cache (from before log titles were bounded) drops
        // the event while it migrates, as a log rescan now would.
        let mut cached: serde_json::Value =
            serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
        cached["version"] = 4.into();
        fs::write(index.state_path(), serde_json::to_vec(&cached).unwrap()).unwrap();
        assert!(index.artifact(&oversize.artifact_id).unwrap().is_none());
        assert!(index.artifact(&good.artifact_id).unwrap().is_some());
        assert_eq!(
            read_state(&index.state_path()).unwrap().unwrap().version,
            STATE_VERSION
        );
    }

    #[test]
    fn index_db_rows_up_to_the_local_payload_limit_are_readable_and_larger_ones_are_left_out() {
        // A table-leaf payload over U-35 = 65,501 bytes must spill to
        // overflow pages, which this writer does not emit. A one-column text
        // record has a 4-byte header here, so 65,497 bytes is the limit.
        let at_limit = "a".repeat(65_497);
        let over_limit = "b".repeat(65_498);
        let image = build_sqlite(&[SqlTable {
            name: "t",
            sql: "CREATE TABLE t(v TEXT)",
            rows: vec![
                vec![SqlValue::Text(at_limit.clone())],
                vec![SqlValue::Text(over_limit)],
                vec![SqlValue::Text("small".into())],
            ],
        }])
        .unwrap();
        assert_eq!(image.skipped_rows, 1);
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("t.db");
        fs::write(&path, image.bytes).unwrap();
        assert_eq!(
            sqlite_check(&path, "select length(v) from t order by rowid"),
            "ok\n65497\n5\n"
        );
    }

    #[test]
    fn an_m1_legacy_visit_counts_one_open_with_a_dated_timeline_line() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://example.org/legacy-page";
        let artifact_id = crate::summary::web_page_artifact_id(url).unwrap();
        let url_hash = sha256_hex(url.as_bytes());
        let legacy_line = format!(
            "\n- 2026-10-20T10:00:00Z — Legacy page — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
            sha256_hex("legacy-marker-open".as_bytes()),
        );
        let dir = root.path().join("log/2026/10");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("2026-10-20.md"),
            format!("{LOG_HEADER}\n{legacy_line}"),
        )
        .unwrap();
        assert_eq!(index.reindex().unwrap().written, 1);
        let markdown = fs::read_to_string(artifact_files(root.path()).pop().unwrap()).unwrap();
        assert!(
            markdown.contains("  opens: 1\n  days_opened: 1\n"),
            "{markdown}"
        );
        assert!(
            markdown.ends_with("## Timeline\n\n- 2026-10-20 · opened 1×\n"),
            "{markdown}"
        );
        assert_eq!(index.artifact(&artifact_id).unwrap().unwrap().opens, 1);
        assert_eq!(index.recent(None, None, 32).unwrap().0[0].opens, 1);
        let (worklog, _) = index
            .worklog(
                parse_date("2026-10-20").unwrap(),
                parse_date("2026-10-20").unwrap(),
            )
            .unwrap();
        assert!(worklog.starts_with("Opened 1 artifact,"), "{worklog}");
        assert_eq!(
            sqlite_check(&index.database_path(), "select opens from visits"),
            "ok\n1\n"
        );
    }

    #[test]
    fn a_version_4_cache_migrates_so_existing_m1_artifact_files_gain_their_opens() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://example.org/legacy-page";
        let url_hash = sha256_hex(url.as_bytes());
        let dir = root.path().join("log/2026/10");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("2026-10-20.md"),
            format!(
                "{LOG_HEADER}\n\n- 2026-10-20T10:00:00Z — Legacy page — <{url}>\n{MARKER_PREFIX}{} url={url_hash} at=2026-10-20T10:00:00Z -->\n",
                sha256_hex("legacy-marker-upgrade".as_bytes()),
            ),
        )
        .unwrap();
        index.reindex().unwrap();
        // Simulate the file and clean cache a version 4 build left: legacy
        // visits rendered no open and no Timeline line.
        let path = artifact_files(root.path()).pop().unwrap();
        let current = fs::read_to_string(&path).unwrap();
        let old = current
            .replace("  opens: 1\n", "  opens: 0\n")
            .replace("- 2026-10-20 · opened 1×\n", "");
        assert_ne!(old, current);
        fs::write(&path, &old).unwrap();
        let mut state: serde_json::Value =
            serde_json::from_slice(&fs::read(index.state_path()).unwrap()).unwrap();
        state["version"] = 4.into();
        state["dirty"] = serde_json::json!([]);
        fs::write(index.state_path(), serde_json::to_vec(&state).unwrap()).unwrap();

        // The migration keeps the checkpoints: no log is reread.
        LOG_BYTES_READ.with(|count| count.set(0));
        assert_eq!(index.regenerate_dirty().unwrap().written, 1);
        assert_eq!(LOG_BYTES_READ.with(|count| count.get()), 0);
        assert_eq!(fs::read_to_string(&path).unwrap(), current);
    }

    fn artifact_files(root: &Path) -> Vec<PathBuf> {
        fn walk(path: &Path, output: &mut Vec<PathBuf>) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    walk(&path, output);
                } else if path.extension().and_then(|value| value.to_str()) == Some("md") {
                    output.push(path);
                }
            }
        }
        let mut output = Vec::new();
        walk(&root.join("artifacts"), &mut output);
        output.sort();
        output
    }
}
