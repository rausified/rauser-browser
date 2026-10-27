//! Rebuildable artifact index and lazily generated artifact Markdown.
//!
//! The append-only daily log is authoritative. `index-state.json` is a small
//! host cache used to avoid reparsing old logs after every delivery, while
//! `index.db` is a standards-compliant SQLite snapshot for current and future
//! readers. Either file may be deleted; `brauser reindex` recreates both.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions as StdOpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use brauser_protocol::{ActivityEvent, ArtifactListItem, ArtifactView, StorageConfig, WhyEvent};
use cap_std::fs::{Dir, OpenOptions};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::format_description::well_known::Rfc3339;
use time::{Date, OffsetDateTime};
use url::Url;
use uuid::Uuid;

use crate::brand::{APP_NAME, NAMESPACE};
use crate::capture::{
    activity_shape_is_bounded, activity_shape_is_valid, canonical_url, trail_transition_is_valid,
};
use crate::summary::SummaryEvent;
use crate::vault::{
    TempFileCleanup, checked_relative_dir, open_selected_root, sync_directory, sync_directory_chain,
};

// v2 adds `CachedEvent::Summary`; bumping forces a v1 cache to rescan the
// log rather than silently omit summaries already durable there.
const STATE_VERSION: u8 = 2;
const MAX_STATE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SCAN_FILES: usize = 100_000;
const MAX_WORKLOG_BYTES: usize = 96 * 1024;
const SQLITE_PAGE_SIZE: usize = 65_536;
const LOG_HEADER: &str = concat!("<!-- ", crate::namespace!(), ":daily-log v1 -->");
const MARKER_PREFIX: &str = concat!("<!-- ", crate::namespace!(), ":visit id=");

#[derive(Clone)]
pub struct ArtifactIndex {
    cache_dir: PathBuf,
    storage: StorageConfig,
    root_identity: String,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct RegenerationReport {
    pub written: usize,
    pub unchanged: usize,
    pub conflicts: usize,
    pub conflict_paths: Vec<String>,
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
    dirty: BTreeSet<String>,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum CachedEvent {
    Activity(ActivityEvent),
    Why(WhyEvent),
    Legacy(LegacyVisit),
    Summary(SummaryEvent),
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
        })
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

    fn record_events(&self, events: impl IntoIterator<Item = CachedEvent>) -> Result<()> {
        let mut state = self.load_or_rebuild()?;
        for event in events {
            let (event_id, artifact_id) = event_identity(&event);
            if let Some(previous) = state.events.get(&event_id) {
                if previous != &event {
                    bail!("an indexed event ID cannot be reused with different content");
                }
                state.dirty.insert(artifact_id);
                continue;
            }
            state.events.insert(event_id, event);
            state.dirty.insert(artifact_id);
        }
        self.publish_state_and_database(&state)
    }

    /// Reconcile a terminal replay or a newly drained legacy visit from the
    /// authoritative owned markers. In particular, never let an untrusted
    /// retry body replace the event that is already durable in the log.
    pub fn reconcile_from_logs(&self) -> Result<()> {
        let state = self.scan_logs()?;
        self.publish_state_and_database(&state)
    }

    /// Rebuild both cache files from owned log markers, then regenerate every
    /// artifact file. This is the explicit repair path for a deleted or
    /// corrupt cache and intentionally replaces user edits to owned files.
    pub fn reindex(&self) -> Result<RegenerationReport> {
        let mut state = self.scan_logs()?;
        let derived = derive(&state.events);
        state.dirty = derived.artifacts.keys().cloned().collect();
        self.publish_state_and_database(&state)?;
        self.regenerate_state(state)
    }

    /// Best-effort lifecycle pass used at native-host connection boundaries.
    pub fn regenerate_dirty(&self) -> Result<RegenerationReport> {
        let state = self.load_or_rebuild()?;
        self.regenerate_state(state)
    }

    fn regenerate_state(&self, mut state: IndexState) -> Result<RegenerationReport> {
        let derived = derive(&state.events);
        let (root, _) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))?;
        let mut report = RegenerationReport::default();
        let dirty = state.dirty.iter().cloned().collect::<Vec<_>>();
        for id in dirty {
            let Some(artifact) = derived.artifacts.get(&id) else {
                state.dirty.remove(&id);
                continue;
            };
            let markdown = render_artifact(&self.storage.artifacts_dir, artifact);
            match replace_artifact(&root, &self.storage.artifacts_dir, artifact, &markdown)? {
                ArtifactWrite::Written => {
                    report.written += 1;
                    state.dirty.remove(&id);
                }
                ArtifactWrite::Unchanged => {
                    report.unchanged += 1;
                    state.dirty.remove(&id);
                }
                ArtifactWrite::Conflict => {
                    report.conflicts += 1;
                    if report.conflict_paths.len() < 64 {
                        report.conflict_paths.push(
                            artifact_path(&self.storage.artifacts_dir, artifact)
                                .to_string_lossy()
                                .into_owned(),
                        );
                    }
                }
            }
        }
        self.publish_state_and_database(&state)?;
        Ok(report)
    }

    fn load_or_rebuild(&self) -> Result<IndexState> {
        match read_state(&self.state_path()) {
            Ok(Some(state)) if self.state_matches(&state) => Ok(state),
            Ok(_) | Err(_) => self.scan_logs(),
        }
    }

    fn state_matches(&self, state: &IndexState) -> bool {
        state.version == STATE_VERSION
            && state.root == self.storage.root
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
            dirty: BTreeSet::new(),
        }
    }

    fn scan_logs(&self) -> Result<IndexState> {
        let (root, _) =
            open_selected_root(Path::new(&self.storage.root), Some(&self.root_identity))?;
        let log_dir = checked_relative_dir(&self.storage.log_dir)?;
        let mut state = self.empty_state();
        let Ok(logs) = root.open_dir(&log_dir) else {
            return Ok(state);
        };
        let mut files = Vec::new();
        collect_markdown(&logs, Path::new(""), 0, &mut files)?;
        if files.len() > MAX_SCAN_FILES {
            bail!("artifact log contains too many files to index");
        }
        files.sort();
        for relative in files {
            let metadata = logs
                .symlink_metadata(&relative)
                .context("checking artifact log file")?;
            if !metadata.is_file() || metadata.len() > MAX_LOG_BYTES {
                continue;
            }
            let mut file = logs.open(&relative).context("opening artifact log file")?;
            let mut bytes = Vec::new();
            Read::by_ref(&mut file)
                .take(MAX_LOG_BYTES + 1)
                .read_to_end(&mut bytes)
                .context("reading artifact log file")?;
            if bytes.len() as u64 > MAX_LOG_BYTES {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&bytes) else {
                continue;
            };
            if !text.starts_with(LOG_HEADER) {
                continue;
            }
            for event in parse_log(text) {
                let (id, artifact_id) = event_identity(&event);
                match state.events.entry(id) {
                    std::collections::btree_map::Entry::Vacant(entry) => {
                        entry.insert(event);
                        state.dirty.insert(artifact_id);
                    }
                    std::collections::btree_map::Entry::Occupied(entry)
                        if entry.get() != &event =>
                    {
                        bail!("artifact log reuses an event ID with different content");
                    }
                    std::collections::btree_map::Entry::Occupied(_) => {}
                }
            }
        }
        Ok(state)
    }

    fn publish_state_and_database(&self, state: &IndexState) -> Result<()> {
        fs::create_dir_all(&self.cache_dir).context("creating artifact index cache directory")?;
        let bytes = serde_json::to_vec(state).context("serializing artifact index state")?;
        if bytes.len() as u64 > MAX_STATE_BYTES {
            bail!("artifact index state exceeds its 64 MiB limit");
        }
        atomic_write(&self.state_path(), &bytes)?;
        let database = sqlite_snapshot(&derive(&state.events), &state.dirty)?;
        atomic_write(&self.database_path(), &database)
    }

    /// Return the current derived record for one recognized artifact. The
    /// append-only log remains authoritative; this never trusts index.db.
    pub fn artifact(&self, artifact_id: &str) -> Result<Option<ArtifactView>> {
        let derived = derive(&self.load_or_rebuild()?.events);
        Ok(derived.artifacts.get(artifact_id).map(|row| {
            artifact_view(
                row,
                linked_count(&derived.edges, &derived.artifacts, artifact_id),
            )
        }))
    }

    /// Canonical URLs previously recorded for an artifact. Note lookup uses
    /// these to discover URL-keyed M1 notes without scanning arbitrary files.
    pub fn artifact_urls(&self, artifact_id: &str) -> Result<Vec<String>> {
        let derived = derive(&self.load_or_rebuild()?.events);
        let mut urls = derived
            .visits
            .iter()
            .filter(|visit| visit.artifact_id == artifact_id)
            .map(|visit| visit.url.clone())
            .collect::<BTreeSet<_>>();
        if let Some(row) = derived.artifacts.get(artifact_id) {
            urls.insert(row.url.clone());
        }
        Ok(urls.into_iter().collect())
    }

    /// Every distinct artifact previously recorded for a canonical URL,
    /// checking both an activity's own URL and any mode-evidence URL hourly
    /// coalescing may have folded into a later event's primary URL (so an
    /// alternate route such as `/edit` is not lost once a visit's primary
    /// URL settles on `/view`). Note lookup falls back to this when no
    /// adapter currently recognizes the URL (for example, the adapter that
    /// matched it was removed), so an existing shared artifact note stays
    /// discoverable instead of silently switching to a fresh URL-keyed one.
    /// Returns the raw candidate set rather than collapsing it: the caller
    /// must combine it with any other recovery source before deciding
    /// uniqueness, since stripped query identity can make two different
    /// artifacts share one canonical form, and a wrong guess from either
    /// source alone would silently redirect a save into an unrelated
    /// artifact's note. M1's legacy `web:<host>/<url_id>` identity
    /// (`web.page`, a reserved synthetic scheme no real adapter may use) is
    /// always excluded from the candidates.
    pub fn artifact_candidates_for_url(&self, url: &str) -> Result<BTreeSet<String>> {
        let events = &self.load_or_rebuild()?.events;
        let is_legacy = |id: &str| id.starts_with("web:");
        let mut candidates = BTreeSet::new();
        for event in events.values() {
            match event {
                CachedEvent::Activity(activity) if !is_legacy(&activity.artifact_id) => {
                    if activity.url == url || activity.modes.iter().any(|mode| mode.url == url) {
                        candidates.insert(activity.artifact_id.clone());
                    }
                }
                CachedEvent::Legacy(legacy)
                    if !is_legacy(&legacy.artifact_id) && legacy.url == url =>
                {
                    candidates.insert(legacy.artifact_id.clone());
                }
                _ => {}
            }
        }
        Ok(candidates)
    }

    /// Latest "why I'm here" context recorded for an artifact, independent of
    /// whether any activity has created its artifact row yet. A newly
    /// recognized artifact can have saved context before its first hourly
    /// activity is delivered.
    pub fn latest_why(&self, artifact_id: &str) -> Result<Option<String>> {
        let events = &self.load_or_rebuild()?.events;
        let mut latest: Option<(OffsetDateTime, String)> = None;
        for event in events.values() {
            if let CachedEvent::Why(why) = event
                && why.artifact_id == artifact_id
            {
                let at = parsed_timestamp(&why.occurred_at).unwrap_or(OffsetDateTime::UNIX_EPOCH);
                if latest.as_ref().is_none_or(|current| at >= current.0) {
                    latest = Some((at, why.text.clone()));
                }
            }
        }
        Ok(latest.map(|(_, text)| text))
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
        let derived = derive(&self.load_or_rebuild()?.events);
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
                });
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
        let derived = derive(&self.load_or_rebuild()?.events);
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
        let opened = rows
            .values()
            .filter(|row| row.opens > 0 || row.summary.is_some())
            .count();
        let carried = rows
            .values()
            .filter(|row| row.opens == 0 && row.summary.is_none() && row.focused > 0)
            .count();
        let mut text = format!(
            "Opened {opened} artifact{}, and focused on {carried} left open from an earlier day.",
            if opened == 1 { "" } else { "s" }
        );
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
                format!(
                    "Summarized: {} — {}, {} importance",
                    plain_text(&summary.title),
                    plain_text(&summary.document_type),
                    summary.importance
                )
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

fn derive(events: &BTreeMap<String, CachedEvent>) -> Derived {
    let mut artifacts = BTreeMap::<String, ArtifactRow>::new();
    let mut visits = Vec::new();
    let mut edges = BTreeMap::<(String, String, String), EdgeRow>::new();
    let mut whys = BTreeMap::<String, (OffsetDateTime, String)>::new();
    for event in events.values() {
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
                for reference in &event.refs {
                    let target = format!("{}:{}", reference.artifact_type, reference.value);
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
                        latest_at: at,
                    });
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
                    opens: 0,
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
                        latest_at: at,
                    });
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

fn parse_log(text: &str) -> Vec<CachedEvent> {
    let mut output = Vec::new();
    let mut previous = "";
    for line in text.lines() {
        if let Some(event) = parse_marker_event(line, previous) {
            output.push(event);
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
            if !activity_shape_is_bounded(&event)
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
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_host_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

enum ArtifactWrite {
    Written,
    Unchanged,
    Conflict,
}

fn replace_artifact(
    root: &Dir,
    artifacts_dir: &str,
    artifact: &ArtifactRow,
    markdown: &str,
) -> Result<ArtifactWrite> {
    let relative = artifact_path(artifacts_dir, artifact);
    let parent = relative.parent().context("artifact path has no parent")?;
    let filename = relative
        .file_name()
        .context("artifact path has no filename")?;
    root.create_dir_all(parent)
        .context("creating artifact directory")?;
    sync_directory_chain(root, parent).context("syncing artifact directory chain")?;
    let dir = root
        .open_dir(parent)
        .context("opening artifact directory")?;
    let existed = match dir.symlink_metadata(filename) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.len() > 4 * 1024 * 1024
            {
                return Ok(ArtifactWrite::Conflict);
            }
            let mut file = dir.open(filename).context("opening existing artifact")?;
            let mut existing = Vec::new();
            Read::by_ref(&mut file)
                .take(4 * 1024 * 1024 + 1)
                .read_to_end(&mut existing)?;
            if existing.len() > 4 * 1024 * 1024 || !owned_artifact(&existing, &artifact.artifact_id)
            {
                return Ok(ArtifactWrite::Conflict);
            }
            if existing == markdown.as_bytes() {
                return Ok(ArtifactWrite::Unchanged);
            }
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error).context("checking artifact file"),
    };

    let temporary = format!(".{NAMESPACE}-artifact-{}.tmp", Uuid::new_v4());
    let mut file = dir
        .open_with(&temporary, OpenOptions::new().write(true).create_new(true))
        .context("creating artifact temporary")?;
    let mut cleanup = TempFileCleanup::new(&dir, &temporary);
    file.write_all(markdown.as_bytes())
        .and_then(|()| file.sync_all())?;
    drop(file);
    if existed {
        dir.rename(&temporary, &dir, filename)
            .context("replacing artifact file")?;
    } else {
        match dir.hard_link(&temporary, &dir, filename) {
            Ok(()) => dir
                .remove_file(&temporary)
                .context("removing published artifact temporary")?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Ok(ArtifactWrite::Conflict);
            }
            Err(error) => return Err(error).context("publishing artifact file"),
        }
    }
    cleanup.disarm();
    drop(cleanup);
    sync_directory(&dir).context("syncing artifact directory")?;
    Ok(ArtifactWrite::Written)
}

fn owned_artifact(bytes: &[u8], artifact_id: &str) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return false;
    };
    if !text.starts_with("---\n") {
        return false;
    }
    let Some(frontmatter) = text[4..].split_once("\n---\n").map(|value| value.0) else {
        return false;
    };
    let mut blocks = 0;
    let mut in_brauser = false;
    let mut kinds = 0;
    let mut kind_matches = false;
    let mut ids = 0;
    let mut id_matches = false;
    for line in frontmatter.lines() {
        if !line.starts_with(' ') {
            in_brauser = line == "brauser:";
            if in_brauser {
                blocks += 1;
            }
            continue;
        }
        if !in_brauser {
            continue;
        }
        if let Some(value) = line.strip_prefix("  kind: ") {
            kinds += 1;
            kind_matches = value == "artifact";
        } else if let Some(value) = line.strip_prefix("  artifact_id: ") {
            ids += 1;
            id_matches = serde_json::from_str::<String>(value).ok().as_deref() == Some(artifact_id);
        }
    }
    blocks == 1 && kinds == 1 && kind_matches && ids == 1 && id_matches
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
    let ups = artifact_path(artifacts_dir, artifact)
        .parent()
        .map_or(0, |dir| dir.components().count());
    format!("{}{target}", "../".repeat(ups))
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
        out.push_str(&format!("Why: {}\n\n", safe_markdown(why)));
    }
    if let Some(summary) = artifact.summaries.last() {
        let link = relative_artifact_link(artifacts_dir, artifact, &summary.summary_path);
        out.push_str(&format!(
            "Summary: [{}, {} importance](<{link}>)\n\n",
            safe_markdown(&summary.document_type),
            safe_markdown(&summary.importance),
        ));
    }
    out.push_str("## Timeline\n\n");
    const MAX_SUMMARY_LINES: usize = 20;
    let total_summaries = artifact.summaries.len();
    let shown_summaries = total_summaries.min(MAX_SUMMARY_LINES);
    for summary in artifact.summaries.iter().rev().take(shown_summaries) {
        out.push_str(&format!(
            "- {} · Summarized: {}\n",
            summary.occurred_at,
            safe_markdown(&summary.title)
        ));
    }
    if total_summaries > shown_summaries {
        out.push_str(&format!(
            "- {} earlier summaries\n",
            total_summaries - shown_summaries
        ));
    }
    for period in timeline_rollups(&artifact.activities) {
        out.push_str(&format!(
            "- {} · {}\n",
            period.label,
            measures(period.focused, period.background, period.opens)
        ));
    }
    out
}

fn yaml_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}
fn whole_minutes(seconds: u64) -> u64 {
    seconds / 60
}

fn safe_markdown(value: &str) -> String {
    crate::presentation::markdown_line(value, 2048)
}

#[derive(Clone)]
struct Rollup {
    label: String,
    focused: u64,
    background: u64,
    opens: u64,
    last_date: Date,
}

fn timeline_rollups(events: &[ActivityEvent]) -> Vec<Rollup> {
    let mut days = BTreeMap::<Date, Rollup>::new();
    for event in events {
        let Ok(at) = OffsetDateTime::parse(&event.occurred_at, &Rfc3339) else {
            continue;
        };
        let date = at.date();
        let row = days.entry(date).or_insert_with(|| Rollup {
            label: date.to_string(),
            focused: 0,
            background: 0,
            opens: 0,
            last_date: date,
        });
        row.focused = row.focused.saturating_add(u64::from(event.focused_seconds));
        row.background = row
            .background
            .saturating_add(u64::from(event.background_seconds));
        row.opens = row.opens.saturating_add(u64::from(event.opens));
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

fn sqlite_snapshot(derived: &Derived, dirty: &BTreeSet<String>) -> Result<Vec<u8>> {
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

/// Emit the small subset of the SQLite file format this append-derived
/// snapshot needs: schema and rowid table B-trees, without secondary indexes
/// or overflow records. A 64 KiB page keeps every bounded event row local.
fn build_sqlite(tables: &[SqlTable]) -> Result<Vec<u8>> {
    let root_count = tables.len() + 1;
    let mut pages = vec![vec![0u8; SQLITE_PAGE_SIZE]; root_count];
    let mut next_page = root_count + 1;
    for (table_index, table) in tables.iter().enumerate() {
        let root_page = table_index + 2;
        let cells = table
            .rows
            .iter()
            .enumerate()
            .map(|(index, row)| table_leaf_cell((index + 1) as u64, row))
            .collect::<Result<Vec<_>>>()?;
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
        .map(|(index, row)| table_leaf_cell((index + 1) as u64, row))
        .collect::<Result<Vec<_>>>()?;
    write_leaf_page(&mut pages[0], 100, &schema_cells)?;
    let page_count = pages.len() as u32;
    write_database_header(&mut pages[0], page_count);
    Ok(pages.into_iter().flatten().collect())
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

fn table_leaf_cell(rowid: u64, values: &[SqlValue]) -> Result<Vec<u8>> {
    let payload = sqlite_record(values)?;
    let mut cell = sqlite_varint(payload.len() as u64);
    cell.extend(sqlite_varint(rowid));
    cell.extend(payload);
    Ok(cell)
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
        use brauser_protocol::{SiteConfig, VisitEvent};

        let hostile = "Report\u{202e}evil\u{202c}\u{200b}\u{200f}\u{2066}hidden\u{2069}\u{feff} %%comment%% ==marked== #tag ![embed](https://bad.invalid)";
        for batched in [false, true] {
            let root = TempDir::new().unwrap();
            let cache = TempDir::new().unwrap();
            let config = storage(root.path());
            let capture = CaptureStore::open_in(&config, None, cache.path()).unwrap();
            let sites = [SiteConfig {
                origin: "https://example.com".into(),
                path_prefix: "/".into(),
            }];
            let policy = || CapturePolicy {
                enabled: true,
                sites: &sites,
                strip_params: &[],
            };
            let event = activity(
                "00000000-0000-4000-8000-000000000001",
                "ABC",
                hostile,
                "2026-10-15T12:00:00Z",
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
            let log = root.path().join("log/2026/10/2026-10-15.md");
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
    fn artifact_candidates_for_url_includes_mode_evidence_urls() {
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
            index.artifact_candidates_for_url(edit_url).unwrap(),
            BTreeSet::from(["gdoc:docs.example/abc".to_string()])
        );
        assert_eq!(
            index.artifact_candidates_for_url(view_url).unwrap(),
            BTreeSet::from(["gdoc:docs.example/abc".to_string()])
        );
    }

    #[test]
    fn artifact_candidates_for_url_includes_every_artifact_sharing_an_ambiguous_url() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://work.example/board?selectedIssue=stripped";
        let event_a = ActivityEvent {
            event_id: "00000000-0000-4000-8000-000000000004".into(),
            artifact_id: "work:a/1".into(),
            artifact_type: "work.item".into(),
            url: url.into(),
            title: Some("A".into()),
            occurred_at: "2026-10-16T12:00:00+00:00".into(),
            incognito: false,
            opens: 1,
            focused_seconds: 5,
            background_seconds: 0,
            modes: Vec::new(),
            trails: Vec::new(),
            refs: Vec::new(),
        };
        let event_b = ActivityEvent {
            event_id: "00000000-0000-4000-8000-000000000005".into(),
            artifact_id: "work:b/2".into(),
            ..event_a.clone()
        };
        index.record_activity(&event_a, &[]).unwrap();
        index.record_activity(&event_b, &[]).unwrap();

        assert_eq!(
            index.artifact_candidates_for_url(url).unwrap(),
            BTreeSet::from(["work:a/1".to_string(), "work:b/2".to_string()])
        );
    }

    #[test]
    fn artifact_candidates_for_url_prefers_a_real_artifact_over_an_earlier_legacy_identity() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://example.com/items/7";
        let legacy_event = ActivityEvent {
            event_id: "00000000-0000-4000-8000-000000000001".into(),
            artifact_id: "web:example.com/abc123".into(),
            artifact_type: "web.page".into(),
            url: url.into(),
            title: Some("Before an adapter recognized it".into()),
            occurred_at: "2026-10-15T12:00:00+00:00".into(),
            incognito: false,
            opens: 1,
            focused_seconds: 10,
            background_seconds: 0,
            modes: Vec::new(),
            trails: Vec::new(),
            refs: Vec::new(),
        };
        let real_event = ActivityEvent {
            event_id: "00000000-0000-4000-8000-000000000002".into(),
            artifact_id: "work:example.com/7".into(),
            artifact_type: "work.item".into(),
            url: url.into(),
            title: Some("Issue 7".into()),
            occurred_at: "2026-10-16T12:00:00+00:00".into(),
            incognito: false,
            opens: 1,
            focused_seconds: 20,
            background_seconds: 0,
            modes: Vec::new(),
            trails: Vec::new(),
            refs: Vec::new(),
        };
        // Oldest first: the legacy M1 visit predates the real adapter match.
        index.record_activity(&legacy_event, &[]).unwrap();
        index.record_activity(&real_event, &[]).unwrap();

        assert_eq!(
            index.artifact_candidates_for_url(url).unwrap(),
            BTreeSet::from(["work:example.com/7".to_string()])
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
        assert!(original.contains("title: \"![x]"));
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
        let derived = derive(&index.load_or_rebuild().unwrap().events);
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

        let derived = derive(&index.load_or_rebuild().unwrap().events);
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
            assert_eq!(String::from_utf8(output.stdout).unwrap(), "2\n['link']\n");
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

    #[test]
    fn reindex_refuses_conflicting_content_for_one_event_id() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let original = activity(
            "00000000-0000-4000-8000-000000000010",
            "REUSED",
            "Original",
            "2026-10-15T12:00:00Z",
        );
        let mut forged = original.clone();
        forged.title = Some("Forged".into());
        let marker = |event: &ActivityEvent| {
            let payload = hex::encode(serde_json::to_vec(event).unwrap());
            format!(
                "{MARKER_PREFIX}{} url={} at={} kind=activity data={payload} -->\n",
                sha256_hex(event.event_id.as_bytes()),
                sha256_hex(event.url.as_bytes()),
                event.occurred_at,
            )
        };
        for (day, event) in [("15", &original), ("16", &forged)] {
            let dir = root.path().join("log/2026/10");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(format!("2026-10-{day}.md")),
                format!("{LOG_HEADER}\n{}", marker(event)),
            )
            .unwrap();
        }

        let error = index.reindex().unwrap_err().to_string();
        assert!(error.contains("reuses an event ID"), "{error}");
    }

    fn page_summary(url: &str, occurred_at: &str) -> crate::summary::PageSummary {
        crate::summary::PageSummary {
            event_id: uuid::Uuid::new_v4().to_string(),
            url: url.into(),
            title: "A good discussion".into(),
            occurred_at: occurred_at.into(),
            document_type: "discussion".into(),
            category: "reading".into(),
            importance: "high".into(),
            summary_path: "pages/hn-item.summary.md".into(),
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
            sites: &[],
            strip_params: &[],
        };
        let url = "https://news.ycombinator.com/item?id=50251344";
        let summary = page_summary(url, &rfc3339_seconds_ago(5));

        assert!(matches!(
            capture.record_summary(policy, &summary),
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
            capture.record_summary(policy, &again),
            crate::capture::CaptureOutcome::Persisted { .. }
        ));
        index.reindex().unwrap();
        assert_eq!(artifact_files(root.path()).len(), 1);
        let updated = index.artifact(&expected_id).unwrap().expect("indexed");
        assert_eq!(updated.title, "An even better discussion");
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
            importance: "high".into(),
            summary_path: "pages/hn-item.summary.md".into(),
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
            importance: "high".into(),
            summary_path: "pages/hn-item.summary.md".into(),
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

    #[test]
    fn worklog_names_a_summary_only_artifact_instead_of_just_seen() {
        let root = TempDir::new().unwrap();
        let cache = TempDir::new().unwrap();
        let index = index(&root, &cache);
        let url = "https://news.ycombinator.com/item?id=50251344";
        let artifact_id = crate::summary::web_page_artifact_id(url).unwrap();
        index
            .record_summary(&SummaryEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                artifact_id: artifact_id.clone(),
                artifact_type: "web.page".into(),
                url: url.into(),
                title: "A good discussion".into(),
                occurred_at: "2026-10-15T12:00:00Z".into(),
                document_type: "discussion".into(),
                category: "reading".into(),
                importance: "high".into(),
                summary_path: "pages/hn-item.summary.md".into(),
            })
            .unwrap();

        let (text, count) = index
            .worklog(
                parse_date("2026-10-15").unwrap(),
                parse_date("2026-10-15").unwrap(),
            )
            .unwrap();
        assert_eq!(count, 1);
        // Zero focused/background/opens must not collapse this into the
        // generic "seen" wording a bare-activity artifact with no measured
        // time would get: a summary is a distinct, named action.
        assert!(text.starts_with("Opened 1 artifact,"));
        assert!(text.contains("Summarized: A good discussion — discussion, high importance"));
        assert!(!text.contains("seen"));
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
            importance: "high".into(),
            summary_path: "pages/hn-item.summary.md".into(),
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
