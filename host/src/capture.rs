//! Host-side visit policy and durable daily Markdown logging.
//!
//! Authorization uses the original URL. Only then may URL normalization drop
//! fragments or configured tracking parameters. A stable lock outside the
//! chosen notes folder covers the event-ID check, append, and file sync.

use std::fs::{self, OpenOptions as StdOpenOptions};
use std::io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use brauser_protocol::{ActivityEvent, OverflowMarker, StorageConfig, VisitEvent, WhyEvent};
use cap_std::fs::{Dir, OpenOptions};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use url::Url;
use uuid::Uuid;

use crate::brand::{APP_NAME, NAMESPACE};
use crate::vault::{
    TempFileCleanup, checked_relative_dir, open_selected_root, sync_directory, sync_directory_chain,
};

const LOG_HEADER: &str = concat!("<!-- ", crate::namespace!(), ":daily-log v1 -->");
const MARKER_PREFIX: &str = concat!("<!-- ", crate::namespace!(), ":visit id=");
const MAX_LOG_BYTES: u64 = 32 * 1024 * 1024;
const MAX_BATCH_INTENT_BYTES: usize = 4 * 1024 * 1024;
const MAX_BATCH_ENTRIES: usize = 65;
const MAX_URL_BYTES: usize = 8192;
const MAX_TITLE_BYTES: usize = 2048;
const MAX_STRIP_RULES: usize = 64;
const MAX_FUTURE_SECS: i64 = 5 * 60;
const MAX_PAST_SECS: i64 = 30 * 24 * 60 * 60;
/// Real-world UTC offsets range from -12:00 to +14:00.
const MAX_UTC_OFFSET_MINUTES: u16 = 14 * 60;

pub struct CaptureStore {
    root: Dir,
    root_path: PathBuf,
    root_identity: String,
    log_dir: PathBuf,
    lock_path: PathBuf,
    journal_dir: PathBuf,
}

/// A durable intent is published before touching Markdown. On replay, only a
/// complete marker in the original log is terminal; an intent alone is never
/// treated as evidence that the visit was saved.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct VisitIntent {
    version: u8,
    root_path: String,
    root_identity: String,
    relative_log: String,
    entry_hash: String,
    offset: u64,
}

/// The compact journal has at most one in-flight batch because the capture
/// lock serializes every writer. Keeping the exact bounded append makes a
/// crash recoverable without retaining one file forever for every event.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct BatchIntent {
    version: u8,
    batch_id: String,
    root_path: String,
    root_identity: String,
    relative_log: String,
    event_hashes: Vec<String>,
    entry: String,
    offset: u64,
}

pub enum CaptureBatchEvent<'a> {
    Visit(&'a VisitEvent),
    Activity(&'a ActivityEvent),
}

pub struct CaptureBatchOutcome {
    pub events: Vec<(String, CaptureOutcome)>,
    pub overflow_recorded: bool,
}

struct BatchEntry {
    event_id: String,
    event_hash: String,
    timestamp: OffsetDateTime,
    entry: String,
    legacy_entry: Option<String>,
    overflow: bool,
}

#[derive(Clone, Copy)]
pub struct CapturePolicy<'a> {
    pub enabled: bool,
    /// Exact origins automatic capture may record: the adapter origins.
    pub origins: &'a [String],
    pub strip_params: &'a [String],
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptureOutcome {
    Persisted { relative_path: PathBuf },
    Suppressed { reason: String },
    Rejected { reason: String },
    Retryable { message: String },
}

struct EventRendering<'a> {
    body: Option<&'a str>,
    marker_metadata: Option<&'a str>,
}

/// Validate new deliveries before replaying adapter matching or writing them.
pub(crate) fn activity_shape_is_valid(event: &ActivityEvent) -> bool {
    activity_shape_is_bounded(event)
        && event
            .trails
            .iter()
            .all(|trail| trail_transition_is_valid(&trail.transition))
}

/// The original schema caps also apply to historical log payloads. Those may
/// contain transitions accepted before the allowlist; retain their activity
/// and exact replay identity, but never derive an edge of an unknown kind.
pub(crate) fn activity_shape_is_bounded(event: &ActivityEvent) -> bool {
    event.opens <= 100_000
        && event.focused_seconds <= 7_200
        && event.background_seconds <= 7_200
        && event.modes.len() <= 3
        && event.trails.len() <= 10
        && event.refs.len() <= 20
        && event.artifact_id.len() <= 512
        && event.artifact_type.len() <= 64
        && event.modes.iter().all(|mode| {
            matches!(mode.mode.as_str(), "view" | "edit" | "review")
                && !mode.url.is_empty()
                && mode.url.len() <= MAX_URL_BYTES
        })
        && event.trails.iter().all(|trail| {
            !trail.transition.is_empty()
                && trail.transition.len() <= 64
                && trail
                    .from_artifact_id
                    .as_ref()
                    .is_none_or(|value| value.len() <= 512)
                && trail
                    .from_url
                    .as_ref()
                    .is_none_or(|value| value.len() <= MAX_URL_BYTES)
                && trail.from_artifact_id.is_some() == trail.from_url.is_some()
        })
        && event.refs.iter().all(|reference| {
            !reference.artifact_type.is_empty()
                && reference.artifact_type.len() <= 64
                && !reference.value.is_empty()
                && reference.value.len() <= 512
        })
}

pub(crate) fn trail_transition_is_valid(transition: &str) -> bool {
    matches!(
        transition,
        "link"
            | "typed"
            | "auto_bookmark"
            | "auto_subframe"
            | "manual_subframe"
            | "generated"
            | "start_page"
            | "form_submit"
            | "reload"
            | "keyword"
            | "keyword_generated"
            // The worker records this qualifier instead of the original type
            // for back/forward navigations, with no same-tab source.
            | "forward_back"
    )
}

impl CaptureStore {
    pub fn open(storage: &StorageConfig) -> Result<Self> {
        Self::open_checked(storage, None)
    }

    pub fn open_checked(storage: &StorageConfig, expected_identity: Option<&str>) -> Result<Self> {
        let project = ProjectDirs::from("", "", APP_NAME)
            .context("cannot locate this user's application config directory")?;
        Self::open_in(storage, expected_identity, project.config_dir())
    }

    pub fn open_in(
        storage: &StorageConfig,
        expected_identity: Option<&str>,
        state_dir: &Path,
    ) -> Result<Self> {
        let chosen = Path::new(&storage.root);
        let log_dir = checked_relative_dir(&storage.log_dir)?;
        let (root, root_identity) = open_selected_root(chosen, expected_identity)?;
        // All capture processes for this user share one lock. Paths that name
        // the same vault entry can differ in case on macOS and Windows, so a
        // lock derived from configured path text would not serialize them.
        let lock_path = state_dir.join("locks").join("capture.lock");
        let journal_dir = state_dir.join("visit-ids");
        Ok(Self {
            root,
            root_path: chosen.to_path_buf(),
            root_identity,
            log_dir,
            lock_path,
            journal_dir,
        })
    }

    #[cfg(test)]
    fn open_at(
        storage: &StorageConfig,
        expected_identity: Option<&str>,
        state_dir: &Path,
    ) -> Result<Self> {
        Self::open_in(storage, expected_identity, state_dir)
    }

    pub fn record(&self, policy: CapturePolicy<'_>, event: &VisitEvent) -> CaptureOutcome {
        self.record_entry(policy, event, None, None)
    }

    pub fn record_activity(
        &self,
        policy: CapturePolicy<'_>,
        event: &ActivityEvent,
    ) -> CaptureOutcome {
        if !activity_shape_is_valid(event) {
            return CaptureOutcome::Rejected {
                reason: "invalid_activity".into(),
            };
        }
        let modes = event
            .modes
            .iter()
            .map(|mode| mode.mode.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let mut measures = Vec::new();
        if event.focused_seconds > 0 {
            measures.push(format!("focused {}s", event.focused_seconds));
        }
        if event.background_seconds > 0 {
            measures.push(format!("background {}s", event.background_seconds));
        }
        if event.opens > 0 {
            measures.push(format!("opened {} time(s)", event.opens));
        }
        if !modes.is_empty() {
            measures.push(format!("modes {modes}"));
        }
        if measures.is_empty() {
            return CaptureOutcome::Rejected {
                reason: "empty_activity".into(),
            };
        }
        let body = format!(
            "{} — {} — {}",
            event.occurred_at,
            safe_title(event.title.as_deref()),
            measures.join(" · ")
        );
        let mut payload = event.clone();
        for mode in &mut payload.modes {
            let source = match parse_page_url(&mode.url) {
                Ok(source) => source,
                Err(reason) => return CaptureOutcome::Rejected { reason },
            };
            if !is_authorized(&source, policy.origins) {
                return CaptureOutcome::Rejected {
                    reason: "mode_site_not_allowed".into(),
                };
            }
            mode.url = match normalize_url(source, policy.strip_params) {
                Ok(normalized) => normalized,
                Err(message) => return CaptureOutcome::Retryable { message },
            };
        }
        for trail in &mut payload.trails {
            let Some(raw_source) = trail.from_url.as_deref() else {
                continue;
            };
            let source = match parse_page_url(raw_source) {
                Ok(source) => source,
                Err(reason) => return CaptureOutcome::Rejected { reason },
            };
            if !is_authorized(&source, policy.origins) {
                return CaptureOutcome::Rejected {
                    reason: "trail_site_not_allowed".into(),
                };
            }
            trail.from_url = match normalize_url(source, policy.strip_params) {
                Ok(normalized) => Some(normalized),
                Err(message) => return CaptureOutcome::Retryable { message },
            };
        }
        let visit = VisitEvent {
            event_id: event.event_id.clone(),
            url: event.url.clone(),
            title: event.title.clone(),
            occurred_at: event.occurred_at.clone(),
            incognito: event.incognito,
        };
        let marker = match serde_json::to_value(payload) {
            Ok(value) => value,
            Err(_) => {
                return CaptureOutcome::Rejected {
                    reason: "invalid_activity".into(),
                };
            }
        };
        self.record_entry(policy, &visit, Some(&body), Some(("activity", marker)))
    }

    pub fn record_why(&self, policy: CapturePolicy<'_>, event: &WhyEvent) -> CaptureOutcome {
        let text = safe_markdown_line(Some(&event.text), 500);
        if text == "Untitled"
            || event.artifact_id.is_empty()
            || event.artifact_id.len() > 512
            || event.text.chars().count() > 500
        {
            return CaptureOutcome::Rejected {
                reason: "invalid_why".into(),
            };
        }
        let body = format!("{} — Why: {}", event.occurred_at, text);
        let visit = VisitEvent {
            event_id: event.event_id.clone(),
            url: event.url.clone(),
            title: Some(event.text.clone()),
            occurred_at: event.occurred_at.clone(),
            incognito: event.incognito,
        };
        let marker = match serde_json::to_value(event) {
            Ok(value) => value,
            Err(_) => {
                return CaptureOutcome::Rejected {
                    reason: "invalid_why".into(),
                };
            }
        };
        self.record_entry(policy, &visit, Some(&body), Some(("why", marker)))
    }

    /// Append the durable evidence for worker-side buffer loss. The batch ID
    /// is the marker's stable event ID, so retrying the same batch cannot add
    /// the notice twice.
    pub fn record_overflow(
        &self,
        policy: CapturePolicy<'_>,
        batch_id: &str,
        anchor_url: &str,
        overflow: &OverflowMarker,
    ) -> CaptureOutcome {
        if overflow.count == 0
            || parse_event_time(&overflow.first_occurred_at).is_err()
            || parse_event_time(&overflow.last_occurred_at).is_err()
            || overflow.first_occurred_at > overflow.last_occurred_at
        {
            return CaptureOutcome::Rejected {
                reason: "invalid_overflow_marker".into(),
            };
        }
        let body = format!(
            "{} — Capture overflow: {} completed activity hour(s) were dropped between {} and {}",
            overflow.last_occurred_at,
            overflow.count,
            overflow.first_occurred_at,
            overflow.last_occurred_at
        );
        let marker = match serde_json::to_value(overflow) {
            Ok(value) => value,
            Err(_) => {
                return CaptureOutcome::Rejected {
                    reason: "invalid_overflow_marker".into(),
                };
            }
        };
        let event = VisitEvent {
            event_id: batch_id.to_owned(),
            url: anchor_url.to_owned(),
            title: None,
            occurred_at: overflow.last_occurred_at.clone(),
            incognito: false,
        };
        self.record_entry(policy, &event, Some(&body), Some(("overflow", marker)))
    }

    /// Validate a complete native delivery first, then publish its surviving
    /// records with one append and one log sync. Events in a batch must share
    /// a local calendar day; the worker enforces that boundary before send.
    pub fn record_batch(
        &self,
        policy: CapturePolicy<'_>,
        batch_id: &str,
        events: &[CaptureBatchEvent<'_>],
        overflow: Option<&OverflowMarker>,
    ) -> CaptureBatchOutcome {
        let mut results = Vec::with_capacity(events.len());
        let mut prepared = Vec::with_capacity(events.len() + usize::from(overflow.is_some()));
        for event in events {
            let (id, candidate) = match event {
                CaptureBatchEvent::Visit(event) => (
                    event.event_id.clone(),
                    self.prepare_batch_entry(policy, event, None, None, false),
                ),
                CaptureBatchEvent::Activity(event) => {
                    let id = event.event_id.clone();
                    (id, self.prepare_batch_activity(policy, event))
                }
            };
            match candidate {
                Ok(entry) => prepared.push(entry),
                Err(outcome) => results.push((id, outcome)),
            }
        }

        if let (Some(overflow), Some(anchor)) = (overflow, events.first()) {
            let anchor_url = match anchor {
                CaptureBatchEvent::Visit(event) => &event.url,
                CaptureBatchEvent::Activity(event) => &event.url,
            };
            let valid = overflow.count > 0
                && parse_event_time(&overflow.first_occurred_at).is_ok()
                && parse_event_time(&overflow.last_occurred_at).is_ok()
                && overflow.first_occurred_at <= overflow.last_occurred_at;
            if valid {
                let body = format!(
                    "{} — Capture overflow: {} completed activity hour(s) were dropped between {} and {}",
                    overflow.last_occurred_at,
                    overflow.count,
                    overflow.first_occurred_at,
                    overflow.last_occurred_at
                );
                let marker = serde_json::to_value(overflow).ok();
                let anchor_time = match anchor {
                    CaptureBatchEvent::Visit(event) => event.occurred_at.clone(),
                    CaptureBatchEvent::Activity(event) => event.occurred_at.clone(),
                };
                let event = VisitEvent {
                    event_id: batch_id.to_owned(),
                    url: anchor_url.clone(),
                    title: None,
                    occurred_at: anchor_time,
                    incognito: false,
                };
                if let Ok(entry) = self.prepare_batch_entry(
                    policy,
                    &event,
                    Some(&body),
                    marker.map(|value| ("overflow", value)),
                    true,
                ) {
                    prepared.push(entry);
                }
            }
        }

        if prepared.is_empty() {
            return CaptureBatchOutcome {
                events: results,
                overflow_recorded: false,
            };
        }
        match self.append_batch(batch_id, &prepared) {
            Ok(outcomes) => {
                let mut overflow_recorded = false;
                for (entry, outcome) in prepared.into_iter().zip(outcomes) {
                    if entry.overflow {
                        overflow_recorded = matches!(
                            outcome,
                            CaptureOutcome::Persisted { .. } | CaptureOutcome::Suppressed { .. }
                        );
                    } else {
                        results.push((entry.event_id, outcome));
                    }
                }
                CaptureBatchOutcome {
                    events: results,
                    overflow_recorded,
                }
            }
            Err(error) => {
                for entry in prepared {
                    if !entry.overflow {
                        results.push((
                            entry.event_id,
                            CaptureOutcome::Retryable {
                                message: error.to_string(),
                            },
                        ));
                    }
                }
                CaptureBatchOutcome {
                    events: results,
                    overflow_recorded: false,
                }
            }
        }
    }

    fn prepare_batch_entry(
        &self,
        policy: CapturePolicy<'_>,
        event: &VisitEvent,
        custom_body: Option<&str>,
        marker_payload: Option<(&str, serde_json::Value)>,
        overflow: bool,
    ) -> std::result::Result<BatchEntry, CaptureOutcome> {
        if !policy.enabled {
            return Err(CaptureOutcome::Suppressed {
                reason: "capture_disabled".into(),
            });
        }
        if event.incognito {
            return Err(CaptureOutcome::Suppressed {
                reason: "incognito".into(),
            });
        }
        let event_id = parse_event_id(&event.event_id)
            .map_err(|reason| CaptureOutcome::Rejected { reason })?;
        let timestamp = parse_event_time(&event.occurred_at)
            .map_err(|reason| CaptureOutcome::Rejected { reason })?;
        check_event_window(timestamp).map_err(|reason| CaptureOutcome::Rejected { reason })?;
        if event.url.len() > MAX_URL_BYTES {
            return Err(CaptureOutcome::Rejected {
                reason: "url_too_long".into(),
            });
        }
        if event
            .title
            .as_ref()
            .is_some_and(|value| value.len() > MAX_TITLE_BYTES)
        {
            return Err(CaptureOutcome::Rejected {
                reason: "title_too_long".into(),
            });
        }
        let original =
            parse_page_url(&event.url).map_err(|reason| CaptureOutcome::Rejected { reason })?;
        if !is_authorized(&original, policy.origins) {
            return Err(CaptureOutcome::Rejected {
                reason: "site_not_allowed".into(),
            });
        }
        let normalized = normalize_url(original, policy.strip_params)
            .map_err(|message| CaptureOutcome::Retryable { message })?;
        let marker_metadata = match marker_payload {
            Some((kind, mut payload)) => {
                if let Some(object) = payload.as_object_mut() {
                    object.insert("url".into(), serde_json::Value::String(normalized.clone()));
                }
                let json = serde_json::to_vec(&payload).map_err(|_| CaptureOutcome::Rejected {
                    reason: "invalid_event_metadata".into(),
                })?;
                format!(" kind={kind} data={}", hex::encode(json))
            }
            None => String::new(),
        };
        let title = safe_title(event.title.as_deref());
        let body = custom_body.map_or_else(
            || format!("{} — {} — <{}>", event.occurred_at, title, normalized),
            |prefix| format!("{prefix} — <{normalized}>"),
        );
        let event_hash = sha256_hex(event_id.as_bytes());
        let url_hash = sha256_hex(normalized.as_bytes());
        let entry = format!(
            "\n- {body}\n{MARKER_PREFIX}{event_hash} url={url_hash} at={}{} -->\n",
            event.occurred_at, marker_metadata
        );
        let legacy_entry = pre_sanitizer_entry(&entry, event, false);
        Ok(BatchEntry {
            event_id,
            event_hash,
            timestamp,
            entry,
            legacy_entry,
            overflow,
        })
    }

    fn prepare_batch_activity(
        &self,
        policy: CapturePolicy<'_>,
        event: &ActivityEvent,
    ) -> std::result::Result<BatchEntry, CaptureOutcome> {
        if !activity_shape_is_valid(event) {
            return Err(CaptureOutcome::Rejected {
                reason: "invalid_activity".into(),
            });
        }
        let modes = event
            .modes
            .iter()
            .map(|mode| mode.mode.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let mut measures = Vec::new();
        if event.focused_seconds > 0 {
            measures.push(format!("focused {}s", event.focused_seconds));
        }
        if event.background_seconds > 0 {
            measures.push(format!("background {}s", event.background_seconds));
        }
        if event.opens > 0 {
            measures.push(format!("opened {} time(s)", event.opens));
        }
        if !modes.is_empty() {
            measures.push(format!("modes {modes}"));
        }
        if measures.is_empty() {
            return Err(CaptureOutcome::Rejected {
                reason: "empty_activity".into(),
            });
        }
        let mut payload = event.clone();
        for mode in &mut payload.modes {
            let source =
                parse_page_url(&mode.url).map_err(|reason| CaptureOutcome::Rejected { reason })?;
            if !is_authorized(&source, policy.origins) {
                return Err(CaptureOutcome::Rejected {
                    reason: "mode_site_not_allowed".into(),
                });
            }
            mode.url = normalize_url(source, policy.strip_params)
                .map_err(|message| CaptureOutcome::Retryable { message })?;
        }
        for trail in &mut payload.trails {
            let Some(raw_source) = trail.from_url.as_deref() else {
                continue;
            };
            let source =
                parse_page_url(raw_source).map_err(|reason| CaptureOutcome::Rejected { reason })?;
            if !is_authorized(&source, policy.origins) {
                return Err(CaptureOutcome::Rejected {
                    reason: "trail_site_not_allowed".into(),
                });
            }
            trail.from_url = Some(
                normalize_url(source, policy.strip_params)
                    .map_err(|message| CaptureOutcome::Retryable { message })?,
            );
        }
        let body = format!(
            "{} — {} — {}",
            event.occurred_at,
            safe_title(event.title.as_deref()),
            measures.join(" · ")
        );
        let marker = serde_json::to_value(payload).map_err(|_| CaptureOutcome::Rejected {
            reason: "invalid_activity".into(),
        })?;
        self.prepare_batch_entry(
            policy,
            &VisitEvent {
                event_id: event.event_id.clone(),
                url: event.url.clone(),
                title: event.title.clone(),
                occurred_at: event.occurred_at.clone(),
                incognito: event.incognito,
            },
            Some(&body),
            Some(("activity", marker)),
            false,
        )
    }

    fn record_entry(
        &self,
        policy: CapturePolicy<'_>,
        event: &VisitEvent,
        custom_body: Option<&str>,
        marker_payload: Option<(&str, serde_json::Value)>,
    ) -> CaptureOutcome {
        if !policy.enabled {
            return CaptureOutcome::Suppressed {
                reason: "capture_disabled".into(),
            };
        }
        if event.incognito {
            return CaptureOutcome::Suppressed {
                reason: "incognito".into(),
            };
        }
        let event_id = match parse_event_id(&event.event_id) {
            Ok(value) => value,
            Err(reason) => return CaptureOutcome::Rejected { reason },
        };
        let timestamp = match parse_event_time(&event.occurred_at) {
            Ok(value) => value,
            Err(reason) => return CaptureOutcome::Rejected { reason },
        };
        if let Err(reason) = check_event_window(timestamp) {
            return CaptureOutcome::Rejected { reason };
        }
        if event.url.len() > MAX_URL_BYTES {
            return CaptureOutcome::Rejected {
                reason: "url_too_long".into(),
            };
        }
        if event
            .title
            .as_ref()
            .is_some_and(|value| value.len() > MAX_TITLE_BYTES)
        {
            return CaptureOutcome::Rejected {
                reason: "title_too_long".into(),
            };
        }
        let original = match parse_page_url(&event.url) {
            Ok(value) => value,
            Err(reason) => return CaptureOutcome::Rejected { reason },
        };
        if !is_authorized(&original, policy.origins) {
            return CaptureOutcome::Rejected {
                reason: "site_not_allowed".into(),
            };
        }
        let normalized = match normalize_url(original, policy.strip_params) {
            Ok(value) => value,
            Err(message) => return CaptureOutcome::Retryable { message },
        };
        let marker_metadata = match marker_payload {
            Some((kind, mut payload)) => {
                if let Some(object) = payload.as_object_mut() {
                    object.insert("url".into(), serde_json::Value::String(normalized.clone()));
                }
                match serde_json::to_vec(&payload) {
                    Ok(json) => Some(format!(" kind={kind} data={}", hex::encode(json))),
                    Err(_) => {
                        return CaptureOutcome::Rejected {
                            reason: "invalid_event_metadata".into(),
                        };
                    }
                }
            }
            None => None,
        };
        match self.record_authorized(
            &event_id,
            timestamp,
            &normalized,
            event,
            EventRendering {
                body: custom_body,
                marker_metadata: marker_metadata.as_deref(),
            },
        ) {
            Ok(outcome) => outcome,
            Err(error) => CaptureOutcome::Retryable {
                message: error.to_string(),
            },
        }
    }

    fn record_authorized(
        &self,
        event_id: &str,
        timestamp: OffsetDateTime,
        normalized_url: &str,
        event: &VisitEvent,
        rendering: EventRendering<'_>,
    ) -> Result<CaptureOutcome> {
        let lock_parent = self
            .lock_path
            .parent()
            .context("capture lock path has no parent")?;
        fs::create_dir_all(lock_parent).context("creating capture lock directory")?;
        let lock_file = StdOpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)
            .context("opening capture lock")?;
        lock_file.lock().context("locking capture log")?;

        // A previous host may have stopped partway through a compact batch.
        // Finish it before a legacy single-record append can land after the
        // torn prefix and make the durable batch intent unrecoverable.
        self.recover_batch_intent()?;

        let event_hash = sha256_hex(event_id.as_bytes());
        let url_hash = sha256_hex(normalized_url.as_bytes());
        let title = safe_title(event.title.as_deref());
        let body = rendering.body.map_or_else(
            || format!("{} — {} — <{}>", event.occurred_at, title, normalized_url),
            |prefix| format!("{prefix} — <{normalized_url}>"),
        );
        let entry = format!(
            "\n- {body}\n{MARKER_PREFIX}{event_hash} url={url_hash} at={}{} -->\n",
            event.occurred_at,
            rendering.marker_metadata.unwrap_or("")
        );
        let journal_path = self.journal_path(&event_hash);
        if let Some(intent) = read_intent(&journal_path)? {
            let legacy_entry = pre_sanitizer_entry(
                &entry,
                event,
                rendering
                    .marker_metadata
                    .is_some_and(|value| value.starts_with(" kind=why ")),
            );
            return self.replay_intent(
                &intent,
                &event_hash,
                entry.as_bytes(),
                legacy_entry.as_deref(),
            );
        }

        let (relative_dir, filename) = self.log_parts(timestamp);
        // Parent directory entries need syncing only until the day's log is
        // published; later appends sync just the log and its directory.
        let new_log = !self.root.exists(relative_dir.join(&filename));
        self.root
            .create_dir_all(&relative_dir)
            .context("creating daily log directory")?;
        if new_log {
            sync_directory_chain(&self.root, &relative_dir)
                .context("syncing daily log parent directories")?;
        }
        let day_dir = self
            .root
            .open_dir(&relative_dir)
            .context("opening daily log directory")?;
        let ownership = ensure_daily_log(&day_dir, &filename)?;
        if !ownership {
            return Ok(CaptureOutcome::Rejected {
                reason: "daily_log_name_conflict".into(),
            });
        }

        let metadata = day_dir
            .symlink_metadata(&filename)
            .context("reading daily log metadata")?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Ok(CaptureOutcome::Rejected {
                reason: "daily_log_name_conflict".into(),
            });
        }
        // An oversized log is refused without scanning it. That is terminal,
        // like a log that the next entry would push over the limit.
        if metadata.len() > MAX_LOG_BYTES {
            return Ok(CaptureOutcome::Rejected {
                reason: "daily_log_full".into(),
            });
        }
        // A previous attempt may have synced the entry but failed to sync a
        // newly published directory entry. Do this before terminal replay.
        sync_directory(&day_dir).context("syncing daily log directory before acknowledgement")?;
        let mut file = day_dir
            .open_with(&filename, OpenOptions::new().read(true).append(true))
            .context("opening owned daily log")?;
        let scan = scan_log(&mut file, &event_hash)?;
        if scan.duplicate {
            // A previous attempt may have written this marker but failed its
            // file sync. Do not acknowledge replay until it is durable.
            file.sync_all()
                .context("syncing replayed daily log entry")?;
            let intent = self.intent(&relative_dir.join(&filename), &entry, metadata.len())?;
            publish_intent(&journal_path, &intent)?;
            return Ok(CaptureOutcome::Suppressed {
                reason: "duplicate_event".into(),
            });
        }

        if metadata.len().saturating_add(entry.len() as u64) > MAX_LOG_BYTES {
            return Ok(CaptureOutcome::Rejected {
                reason: "daily_log_full".into(),
            });
        }
        let intent = self.intent(&relative_dir.join(&filename), &entry, metadata.len())?;
        publish_intent(&journal_path, &intent)
            .context("recording visit intent before Markdown append")?;
        if let Err(error) = file
            .write_all(entry.as_bytes())
            .and_then(|()| file.sync_all())
        {
            // A torn entry left in place would block this event's replay once
            // another visit is appended after it.
            roll_back_append(
                &day_dir,
                Path::new(&filename),
                metadata.len(),
                entry.as_bytes(),
            );
            return Err(error).context("appending daily log entry");
        }
        sync_directory(&day_dir).context("syncing daily log directory")?;
        Ok(CaptureOutcome::Persisted {
            relative_path: relative_dir.join(filename),
        })
    }

    fn journal_path(&self, event_hash: &str) -> PathBuf {
        self.journal_dir
            .join(&event_hash[..2])
            .join(format!("{event_hash}.json"))
    }

    fn intent(&self, relative_log: &Path, entry: &str, offset: u64) -> Result<VisitIntent> {
        Ok(VisitIntent {
            version: 1,
            root_path: self
                .root_path
                .to_str()
                .context("notes path is not UTF-8")?
                .to_owned(),
            root_identity: self.root_identity.clone(),
            relative_log: relative_log
                .to_str()
                .context("log path is not UTF-8")?
                .to_owned(),
            entry_hash: sha256_hex(entry.as_bytes()),
            offset,
        })
    }

    fn replay_intent(
        &self,
        intent: &VisitIntent,
        event_hash: &str,
        entry: &[u8],
        legacy_entry: Option<&str>,
    ) -> Result<CaptureOutcome> {
        if intent.version != 1 || intent.entry_hash.len() != 64 || intent.offset > MAX_LOG_BYTES {
            bail!("visit intent is invalid; manual repair is required");
        }
        let relative_log = checked_relative_dir(&intent.relative_log)?;
        let parent = relative_log
            .parent()
            .context("visit intent has no log directory")?;
        let filename = relative_log
            .file_name()
            .context("visit intent has no log filename")?;
        let (root, _) =
            open_selected_root(Path::new(&intent.root_path), Some(&intent.root_identity)).context(
                "original notes folder changed; choose it again or discard the queued visit",
            )?;
        let day_dir = root
            .open_dir(parent)
            .context("original daily log is unavailable; manual repair is required")?;
        let metadata = day_dir
            .symlink_metadata(filename)
            .context("original daily log is unavailable; manual repair is required")?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > MAX_LOG_BYTES
        {
            bail!("original daily log changed; manual repair is required");
        }
        if !existing_log_is_owned(
            &day_dir,
            filename.to_str().context("log filename is not UTF-8")?,
        )? {
            bail!("original daily log is not owned by {APP_NAME}");
        }
        let mut file = day_dir
            .open_with(filename, OpenOptions::new().read(true).append(true))
            .context("opening original daily log")?;
        let scan = scan_log(&mut file, event_hash)?;
        if scan.duplicate {
            file.sync_all().context("syncing replayed daily log")?;
            sync_directory(&day_dir).context("syncing original daily log directory")?;
            return Ok(CaptureOutcome::Suppressed {
                reason: "duplicate_event".into(),
            });
        }
        if self.root_identity != intent.root_identity {
            bail!("visit belongs to a previously selected notes folder; do not append there");
        }
        // V1 intents hash presentation bytes. For an interrupted pre-sanitizer
        // append, accept only the exact older rendering whose complete hash
        // matches the durable intent. Complete markers above still dedupe,
        // and the tail check below must match this same historical entry.
        let entry = if intent.entry_hash == sha256_hex(entry) {
            entry
        } else if let Some(legacy) = legacy_entry
            && intent.entry_hash == sha256_hex(legacy.as_bytes())
        {
            legacy.as_bytes()
        } else {
            bail!("queued visit changed after a partial write; manual repair is required");
        };
        if metadata.len() < intent.offset {
            bail!("original daily log changed after visit intent; manual repair is required");
        }
        file.seek(SeekFrom::Start(intent.offset))?;
        let mut region = Vec::new();
        file.read_to_end(&mut region)?;
        // Other visits may have been appended after this intent was recorded,
        // either before this entry was started or after an earlier resume.
        // Anything else after them must be an exact prefix of this entry.
        let tail = &region[complete_entries_len(&region)..];
        if tail.len() > entry.len() || tail != &entry[..tail.len()] {
            bail!("original daily log was edited after visit intent; manual repair is required");
        }
        let resume_from = metadata.len() - tail.len() as u64;
        if let Err(error) = file
            .write_all(&entry[tail.len()..])
            .and_then(|()| file.sync_all())
        {
            roll_back_append(&day_dir, Path::new(filename), resume_from, entry);
            return Err(error).context("completing interrupted daily log entry");
        }
        sync_directory(&day_dir).context("syncing original daily log directory")?;
        Ok(CaptureOutcome::Persisted {
            relative_path: relative_log,
        })
    }

    fn append_batch(&self, batch_id: &str, entries: &[BatchEntry]) -> Result<Vec<CaptureOutcome>> {
        let lock_parent = self
            .lock_path
            .parent()
            .context("capture lock path has no parent")?;
        fs::create_dir_all(lock_parent).context("creating capture lock directory")?;
        let lock_file = StdOpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)
            .context("opening capture lock")?;
        lock_file.lock().context("locking capture log")?;

        self.recover_batch_intent()?;
        let (relative_dir, filename) = self.log_parts(entries[0].timestamp);
        if entries.iter().any(|entry| {
            self.log_parts(entry.timestamp) != (relative_dir.clone(), filename.clone())
        }) {
            bail!("capture batch crosses a local calendar-day boundary");
        }

        // Pre-upgrade queue items may already have M1's per-event durable
        // intent. It remains the global event-ID authority across root and
        // log-directory changes, and may name an append that needs finishing.
        let mut outcomes: Vec<Option<CaptureOutcome>> = (0..entries.len()).map(|_| None).collect();
        for (index, entry) in entries.iter().enumerate() {
            let journal_path = self.journal_path(&entry.event_hash);
            if let Some(intent) = read_intent(&journal_path)? {
                outcomes[index] = Some(self.replay_intent(
                    &intent,
                    &entry.event_hash,
                    entry.entry.as_bytes(),
                    entry.legacy_entry.as_deref(),
                )?);
            }
        }
        if outcomes.iter().all(Option::is_some) {
            return Ok(outcomes.into_iter().map(Option::unwrap).collect());
        }

        let new_log = !self.root.exists(relative_dir.join(&filename));
        self.root
            .create_dir_all(&relative_dir)
            .context("creating daily log directory")?;
        if new_log {
            sync_directory_chain(&self.root, &relative_dir)
                .context("syncing daily log parent directories")?;
        }
        let day_dir = self
            .root
            .open_dir(&relative_dir)
            .context("opening daily log directory")?;
        if !ensure_daily_log(&day_dir, &filename)? {
            bail!("daily log name conflicts with an unowned file");
        }
        let metadata = day_dir
            .symlink_metadata(&filename)
            .context("reading daily log metadata")?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > MAX_LOG_BYTES
        {
            bail!("daily log is unavailable or full");
        }
        sync_directory(&day_dir).context("syncing daily log directory before acknowledgement")?;
        let mut file = day_dir
            .open_with(&filename, OpenOptions::new().read(true).append(true))
            .context("opening owned daily log")?;

        let mut combined = String::new();
        let mut appended = Vec::new();
        for (index, entry) in entries.iter().enumerate() {
            if outcomes[index].is_some() {
                continue;
            }
            file.seek(SeekFrom::Start(0))?;
            let scan = scan_log(&mut file, &entry.event_hash)?;
            if scan.duplicate {
                outcomes[index] = Some(CaptureOutcome::Suppressed {
                    reason: "duplicate_event".into(),
                });
            } else {
                combined.push_str(&entry.entry);
                appended.push(index);
            }
        }
        if combined.is_empty() {
            file.sync_all().context("syncing replayed capture batch")?;
            return Ok(outcomes.into_iter().map(Option::unwrap).collect());
        }
        if metadata.len().saturating_add(combined.len() as u64) > MAX_LOG_BYTES {
            return Ok(outcomes
                .into_iter()
                .map(|outcome| {
                    outcome.unwrap_or_else(|| CaptureOutcome::Rejected {
                        reason: "daily_log_full".into(),
                    })
                })
                .collect());
        }
        let relative_log = relative_dir.join(&filename);
        let intent = BatchIntent {
            version: 2,
            batch_id: batch_id.to_owned(),
            root_path: self
                .root_path
                .to_str()
                .context("notes path is not UTF-8")?
                .to_owned(),
            root_identity: self.root_identity.clone(),
            relative_log: relative_log
                .to_str()
                .context("log path is not UTF-8")?
                .to_owned(),
            event_hashes: entries
                .iter()
                .map(|entry| entry.event_hash.clone())
                .collect(),
            entry: combined.clone(),
            offset: metadata.len(),
        };
        publish_batch_intent(&self.batch_intent_path(), &intent)?;
        if let Err(error) = file
            .write_all(combined.as_bytes())
            .and_then(|()| file.sync_all())
        {
            roll_back_append(
                &day_dir,
                Path::new(&filename),
                metadata.len(),
                combined.as_bytes(),
            );
            return Err(error).context("appending capture batch");
        }
        sync_directory(&day_dir).context("syncing daily log directory")?;
        clear_batch_intent(&self.batch_intent_path())?;
        for index in appended {
            outcomes[index] = Some(CaptureOutcome::Persisted {
                relative_path: relative_log.clone(),
            });
        }
        Ok(outcomes.into_iter().map(Option::unwrap).collect())
    }

    fn batch_intent_path(&self) -> PathBuf {
        self.journal_dir.join("index-v2.json")
    }

    fn recover_batch_intent(&self) -> Result<()> {
        let path = self.batch_intent_path();
        let Some(intent) = read_batch_intent(&path)? else {
            return Ok(());
        };
        if intent.version != 2
            || intent.event_hashes.is_empty()
            || intent.event_hashes.len() > MAX_BATCH_ENTRIES
            || intent
                .event_hashes
                .iter()
                .any(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
            || Uuid::parse_str(&intent.batch_id).is_err()
            || intent.offset > MAX_LOG_BYTES
            || intent.entry.is_empty()
            || intent.entry.len() > MAX_BATCH_INTENT_BYTES
        {
            bail!("capture batch intent is invalid; manual repair is required");
        }
        let relative_log = checked_relative_dir(&intent.relative_log)?;
        let parent = relative_log
            .parent()
            .context("capture batch intent has no log directory")?;
        let filename = relative_log
            .file_name()
            .context("capture batch intent has no log filename")?;
        let (root, _) =
            open_selected_root(Path::new(&intent.root_path), Some(&intent.root_identity)).context(
                "original notes folder changed; choose it again or discard the queued batch",
            )?;
        let day_dir = root
            .open_dir(parent)
            .context("original daily log is unavailable; manual repair is required")?;
        if !existing_log_is_owned(
            &day_dir,
            filename.to_str().context("log filename is not UTF-8")?,
        )? {
            bail!("original daily log is not owned by {APP_NAME}");
        }
        let metadata = day_dir
            .symlink_metadata(filename)
            .context("reading original daily log")?;
        if metadata.file_type().is_symlink()
            || !metadata.is_file()
            || metadata.len() > MAX_LOG_BYTES
        {
            bail!("original daily log changed; manual repair is required");
        }
        let mut file = day_dir
            .open_with(filename, OpenOptions::new().read(true).append(true))
            .context("opening original daily log")?;
        let mut complete = true;
        for hash in &intent.event_hashes {
            file.seek(SeekFrom::Start(0))?;
            complete &= scan_log(&mut file, hash)?.duplicate;
        }
        if !complete {
            if self.root_identity != intent.root_identity || metadata.len() < intent.offset {
                bail!("capture batch belongs to a changed notes folder or log");
            }
            file.seek(SeekFrom::Start(intent.offset))?;
            let mut region = Vec::new();
            file.read_to_end(&mut region)?;
            if region.len() > intent.entry.len()
                || region != intent.entry.as_bytes()[..region.len()]
            {
                bail!(
                    "daily log was edited after a capture batch intent; manual repair is required"
                );
            }
            if let Err(error) = file
                .write_all(&intent.entry.as_bytes()[region.len()..])
                .and_then(|()| file.sync_all())
            {
                roll_back_append(
                    &day_dir,
                    Path::new(filename),
                    intent.offset,
                    intent.entry.as_bytes(),
                );
                return Err(error).context("completing interrupted capture batch");
            }
        } else {
            file.sync_all().context("syncing replayed capture batch")?;
        }
        sync_directory(&day_dir).context("syncing original daily log directory")?;
        clear_batch_intent(&path)
    }

    fn log_parts(&self, timestamp: OffsetDateTime) -> (PathBuf, String) {
        let year = timestamp.year();
        let month = timestamp.month() as u8;
        let day = timestamp.day();
        let relative_dir = self
            .log_dir
            .join(format!("{year:04}"))
            .join(format!("{month:02}"));
        (relative_dir, format!("{year:04}-{month:02}-{day:02}.md"))
    }
}

fn parse_event_id(value: &str) -> std::result::Result<String, String> {
    let parsed = Uuid::parse_str(value).map_err(|_| "invalid_event_id".to_owned())?;
    Ok(parsed.hyphenated().to_string())
}

/// Accept `YYYY-MM-DDTHH:MM:SSZ` or the same with a `+HH:MM`/`-HH:MM`
/// offset. The offset is the user's local time at the visit and chooses the
/// daily log date, so evening visits stay in that day's log.
fn parse_event_time(value: &str) -> std::result::Result<OffsetDateTime, String> {
    let bytes = value.as_bytes();
    let offset_ok = match bytes.len() {
        20 => bytes[19] == b'Z',
        25 => matches!(bytes[19], b'+' | b'-') && bytes[22] == b':',
        _ => false,
    };
    if !offset_ok
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return Err("invalid_timestamp".into());
    }
    let parsed = OffsetDateTime::parse(value, &Rfc3339).map_err(|_| "invalid_timestamp")?;
    if parsed.offset().whole_minutes().unsigned_abs() > MAX_UTC_OFFSET_MINUTES {
        return Err("invalid_timestamp".into());
    }
    Ok(parsed)
}

fn check_event_window(value: OffsetDateTime) -> std::result::Result<(), String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "host_clock_unavailable".to_owned())?
        .as_secs() as i64;
    let at = value.unix_timestamp();
    if at > now + MAX_FUTURE_SECS {
        return Err("timestamp_in_future".into());
    }
    if at < now - MAX_PAST_SECS {
        return Err("timestamp_too_old".into());
    }
    Ok(())
}

fn parse_page_url(value: &str) -> std::result::Result<Url, String> {
    let url = Url::parse(value).map_err(|_| "invalid_url".to_owned())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("invalid_url_scheme".into());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("url_contains_credentials".into());
    }
    Ok(url)
}

/// Return a canonical page URL. Callers must authorize `raw` first.
pub fn canonical_url(raw: &str, strip_params: &[String]) -> Result<String> {
    let url = parse_page_url(raw).map_err(anyhow::Error::msg)?;
    normalize_url(url, strip_params).map_err(anyhow::Error::msg)
}

/// Check the original URL's exact origin against the capture origins. Call
/// this before normalization; dropping query parameters must never widen
/// authorization.
fn is_authorized(url: &Url, origins: &[String]) -> bool {
    let origin = url.origin().ascii_serialization();
    origins.iter().any(|allowed| allowed == &origin)
}

fn normalize_url(mut url: Url, strip_params: &[String]) -> std::result::Result<String, String> {
    if strip_params.len() > MAX_STRIP_RULES {
        return Err("too many URL parameter rules".into());
    }
    for pattern in strip_params {
        let core = pattern.strip_suffix('*').unwrap_or(pattern);
        if core.is_empty()
            || pattern.len() > 64
            || !core
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            || core.contains('*')
        {
            return Err("invalid URL parameter rule".into());
        }
    }
    url.set_fragment(None);
    if let Some(query) = url.query().map(str::to_owned) {
        // Inspect decoded keys, but retain every untouched raw query segment.
        // Rebuilding all pairs would change the encoding of meaningful values.
        let segments: Vec<_> = query.split('&').collect();
        let kept: Vec<_> = segments
            .iter()
            .copied()
            .filter(|segment| {
                let key = url::form_urlencoded::parse(segment.as_bytes())
                    .next()
                    .map(|(key, _)| key.into_owned())
                    .unwrap_or_default();
                !strip_params
                    .iter()
                    .any(|pattern| matches_param(&key, pattern))
            })
            .collect();
        if kept.len() != segments.len() {
            if kept.is_empty() {
                url.set_query(None);
            } else {
                url.set_query(Some(&kept.join("&")));
            }
        }
    }
    Ok(url.to_string())
}

fn matches_param(key: &str, pattern: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let pattern = pattern.to_ascii_lowercase();
    match pattern.strip_suffix('*') {
        Some(prefix) => key.starts_with(prefix),
        None => key == pattern,
    }
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn read_intent(path: &Path) -> Result<Option<VisitIntent>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("checking visit intent"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() > 8192 {
        bail!("visit intent is invalid; manual repair is required");
    }
    let bytes = fs::read(path).context("reading visit intent")?;
    if bytes.len() > 8192 {
        bail!("visit intent exceeds its size limit");
    }
    let intent: VisitIntent = serde_json::from_slice(&bytes)
        .context("visit intent is malformed; manual repair is required")?;
    Ok(Some(intent))
}

fn publish_intent(path: &Path, intent: &VisitIntent) -> Result<()> {
    let bytes = serde_json::to_vec(intent).context("serializing visit intent")?;
    if bytes.len() > 8192 {
        bail!("visit intent exceeds its size limit");
    }
    let parent = path.parent().context("visit intent has no parent")?;
    fs::create_dir_all(parent).context("creating visit intent directory")?;
    // Flush each newly created directory entry before the Markdown append.
    let journal_dir = parent
        .parent()
        .context("visit intent has no shard parent")?;
    sync_state_directory(
        journal_dir
            .parent()
            .context("visit intent has no state parent")?,
    )?;
    sync_state_directory(journal_dir)?;
    sync_state_directory(parent)?;
    let temporary = parent.join(format!(".{NAMESPACE}-intent-{}.tmp", Uuid::new_v4()));
    let mut options = StdOpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .context("creating visit intent temporary file")?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes).context("writing visit intent")?;
        file.sync_all().context("syncing visit intent")?;
        drop(file);
        fs::hard_link(&temporary, path).context("publishing visit intent")?;
        sync_state_directory(parent).context("syncing visit intent directory")?;
        Ok(())
    })();
    if let Err(error) = fs::remove_file(&temporary) {
        eprintln!("{NAMESPACE}: warning: could not remove visit intent temporary file: {error}");
    }
    result
}

fn read_batch_intent(path: &Path) -> Result<Option<BatchIntent>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("checking capture batch intent"),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_BATCH_INTENT_BYTES as u64
    {
        bail!("capture batch intent is invalid; manual repair is required");
    }
    let bytes = fs::read(path).context("reading capture batch intent")?;
    serde_json::from_slice(&bytes)
        .context("capture batch intent is malformed; manual repair is required")
        .map(Some)
}

fn publish_batch_intent(path: &Path, intent: &BatchIntent) -> Result<()> {
    let bytes = serde_json::to_vec(intent).context("serializing capture batch intent")?;
    if bytes.len() > MAX_BATCH_INTENT_BYTES {
        bail!("capture batch intent exceeds its size limit");
    }
    let parent = path
        .parent()
        .context("capture batch intent has no parent")?;
    fs::create_dir_all(parent).context("creating capture batch intent directory")?;
    if let Some(state_parent) = parent.parent() {
        sync_state_directory(state_parent)?;
    }
    sync_state_directory(parent)?;
    let temporary = parent.join(format!(".{NAMESPACE}-batch-intent-{}.tmp", Uuid::new_v4()));
    let mut options = StdOpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .context("creating capture batch intent temporary file")?;
    let result = (|| -> Result<()> {
        file.write_all(&bytes)
            .context("writing capture batch intent")?;
        file.sync_all().context("syncing capture batch intent")?;
        drop(file);
        fs::rename(&temporary, path).context("publishing capture batch intent")?;
        sync_state_directory(parent).context("syncing capture batch intent directory")
    })();
    if let Err(error) = fs::remove_file(&temporary)
        && error.kind() != io::ErrorKind::NotFound
    {
        eprintln!("{NAMESPACE}: warning: could not remove batch intent temporary file: {error}");
    }
    result
}

fn clear_batch_intent(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => sync_state_directory(
            path.parent()
                .context("capture batch intent has no parent")?,
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("clearing capture batch intent"),
    }
}

#[cfg(not(windows))]
fn sync_state_directory(path: &Path) -> Result<()> {
    fs::File::open(path)
        .with_context(|| format!("opening state directory {}", path.display()))?
        .sync_all()
        .with_context(|| format!("syncing state directory {}", path.display()))
}

#[cfg(windows)]
fn sync_state_directory(_path: &Path) -> Result<()> {
    // Windows directory-entry power-loss durability needs separate validation.
    Ok(())
}

fn ensure_daily_log(dir: &Dir, name: &str) -> Result<bool> {
    match dir.symlink_metadata(name) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Ok(false);
            }
            return existing_log_is_owned(dir, name);
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("checking daily log"),
    }
    let temporary = format!(".{NAMESPACE}-log-{}.tmp", Uuid::new_v4());
    let mut file = dir
        .open_with(&temporary, OpenOptions::new().write(true).create_new(true))
        .context("creating daily log temporary file")?;
    let mut cleanup = TempFileCleanup::new(dir, &temporary);
    file.write_all(format!("{LOG_HEADER}\n").as_bytes())?;
    file.sync_all()?;
    drop(file);
    match dir.hard_link(&temporary, dir, name) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let metadata = dir.symlink_metadata(name)?;
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Ok(false);
            }
            return existing_log_is_owned(dir, name);
        }
        Err(error) => return Err(error).context("publishing daily log"),
    }
    cleanup.disarm();
    if let Err(error) = dir.remove_file(&temporary) {
        eprintln!("{NAMESPACE}: warning: could not remove daily log temporary file: {error}");
    }
    sync_directory(dir).context("syncing new daily log directory")?;
    Ok(true)
}

/// Only the header line is read, so this is bounded for a log of any size;
/// the caller decides what an oversized log means.
fn existing_log_is_owned(dir: &Dir, name: &str) -> Result<bool> {
    let file = dir.open(name).context("opening existing daily log")?;
    let mut first = Vec::new();
    BufReader::new(file.take(LOG_HEADER.len() as u64 + 2)).read_until(b'\n', &mut first)?;
    while matches!(first.last(), Some(b'\r' | b'\n')) {
        first.pop();
    }
    Ok(first == LOG_HEADER.as_bytes())
}

/// Truncate a failed append back to `offset`, but only if everything after
/// `offset` is a prefix of `entry`. The capture lock excludes other host
/// writers; an unexpected tail is left for manual repair instead.
///
/// This opens its own read-write handle: Windows refuses to truncate through
/// an append-only handle.
fn roll_back_append(dir: &Dir, name: &Path, offset: u64, entry: &[u8]) {
    let result = (|| -> io::Result<bool> {
        let mut file = dir.open_with(name, OpenOptions::new().read(true).write(true))?;
        let length = file.metadata()?.len();
        if length <= offset {
            return Ok(true);
        }
        let written = length - offset;
        if written > entry.len() as u64 {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut tail = vec![0; written as usize];
        file.read_exact(&mut tail)?;
        if tail != entry[..tail.len()] {
            return Ok(false);
        }
        file.set_len(offset)?;
        file.sync_all()?;
        Ok(true)
    })();
    match result {
        Ok(true) => {}
        Ok(false) => eprintln!("{NAMESPACE}: warning: daily log changed during a failed append"),
        Err(error) => {
            eprintln!("{NAMESPACE}: warning: could not roll back a failed append: {error}")
        }
    }
}

/// Length of the leading run of complete owned visit entries in `region`.
fn complete_entries_len(region: &[u8]) -> usize {
    let mut consumed = 0;
    while let Some(length) = complete_entry_len(&region[consumed..]) {
        consumed += length;
    }
    consumed
}

fn complete_entry_len(bytes: &[u8]) -> Option<usize> {
    let body = bytes.strip_prefix(b"\n- ")?;
    let marker_start = 3 + body.iter().position(|&byte| byte == b'\n')? + 1;
    let marker_line = &bytes[marker_start..];
    let marker_length = marker_line.iter().position(|&byte| byte == b'\n')?;
    parse_marker(std::str::from_utf8(&marker_line[..marker_length]).ok()?)?;
    Some(marker_start + marker_length + 1)
}

struct ScanResult {
    duplicate: bool,
}

fn scan_log(file: &mut cap_std::fs::File, event_hash: &str) -> Result<ScanResult> {
    // Split on bytes: a crash can leave the log cut inside a multibyte
    // character, and that must not stop the scan or the resume after it.
    let mut lines = BufReader::new(file).split(b'\n').map(|line| {
        line.map(|mut line| {
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            line
        })
    });
    if lines.next().transpose()?.as_deref() != Some(LOG_HEADER.as_bytes()) {
        bail!("daily log exists without {APP_NAME} ownership marker");
    }
    let mut result = ScanResult { duplicate: false };
    for line in lines {
        let line = line?;
        let Ok(line) = std::str::from_utf8(&line) else {
            continue;
        };
        if let Some((id, _, _)) = parse_marker(line)
            && id == event_hash
        {
            result.duplicate = true;
        }
    }
    Ok(result)
}

fn parse_marker(line: &str) -> Option<(&str, &str, i64)> {
    let rest = line.strip_prefix(MARKER_PREFIX)?;
    let (id, rest) = rest.split_once(" url=")?;
    let (url, rest) = rest.split_once(" at=")?;
    let timestamp = rest.strip_suffix(" -->")?.split(' ').next()?;
    if id.len() != 64 || url.len() != 64 {
        return None;
    }
    let at = parse_event_time(timestamp).ok()?.unix_timestamp();
    Some((id, url, at))
}

/// Reconstruct only the text prefix changed by the sanitizer upgrade, keeping
/// the URL, measurements, and encoded marker payload byte-exact. The caller
/// must verify the hash of this entire candidate against a durable v1 intent;
/// it is never used for a new append. This also permits completion of old text
/// containing formatting characters, just as compact batch recovery retains
/// its original prepared bytes. Do not use this frozen renderer for display.
fn pre_sanitizer_entry(entry: &str, event: &VisitEvent, why: bool) -> Option<String> {
    let limit = if why { 500 } else { 300 };
    let label = if why { "Why: " } else { "" };
    let prefix = format!("\n- {} — {label}", event.occurred_at);
    let current = format!(
        "{prefix}{}",
        safe_markdown_line(event.title.as_deref(), limit)
    );
    let suffix = entry.strip_prefix(&current)?;
    let mut text = String::new();
    for ch in event
        .title
        .as_deref()
        .unwrap_or("Untitled")
        .trim()
        .chars()
        .take(limit)
    {
        if ch.is_control() {
            text.push(' ');
        } else {
            if matches!(ch, '\\' | '[' | ']' | '<' | '>' | '*' | '_' | '`' | '|') {
                text.push('\\');
            }
            text.push(ch);
        }
    }
    if text.trim().is_empty() {
        text = "Untitled".into();
    }
    Some(format!("{prefix}{text}{suffix}"))
}

fn safe_title(value: Option<&str>) -> String {
    safe_markdown_line(value, 300)
}

fn safe_markdown_line(value: Option<&str>, limit: usize) -> String {
    crate::presentation::markdown_line(value.unwrap_or("Untitled"), limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use brauser_protocol::{ActivityMode, ActivityRef, ActivityTrail};

    fn storage(root: &Path, log_dir: &str) -> StorageConfig {
        StorageConfig {
            root: root.to_string_lossy().into_owned(),
            profile: "neutral".into(),
            log_dir: log_dir.into(),
            pages_dir: "pages".into(),
            later_dir: "later".into(),
            artifacts_dir: "artifacts".into(),
            summaries_dir: None,
            knowledge_base_dir: None,
        }
    }

    fn event() -> VisitEvent {
        VisitEvent {
            event_id: Uuid::new_v4().to_string(),
            url: "https://example.com/article?keep=1&secret=2".into(),
            title: Some("Article".into()),
            occurred_at: OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap()
                .format(&Rfc3339)
                .unwrap(),
            incognito: false,
        }
    }

    #[test]
    fn capture_origins_authorize_exact_origins_only() {
        let origins = ["https://example.com".to_owned()];
        for (raw, expected) in [
            ("https://example.com/any/path?q=1", true),
            ("https://example.com:443/", true),
            ("http://example.com/", false),
            ("https://example.com:8443/", false),
            ("https://sub.example.com/", false),
            ("https://example.com.evil.test/", false),
        ] {
            let url = parse_page_url(raw).unwrap();
            assert_eq!(is_authorized(&url, &origins), expected, "{raw}");
        }
        let url = parse_page_url("https://example.com/").unwrap();
        assert!(!is_authorized(&url, &[]));
    }

    fn record(store: &CaptureStore, event: &VisitEvent) -> CaptureOutcome {
        let origins = ["https://example.com".to_owned()];
        store.record(
            CapturePolicy {
                enabled: true,
                origins: &origins,
                strip_params: &[],
            },
            event,
        )
    }

    fn activity() -> ActivityEvent {
        ActivityEvent {
            event_id: Uuid::new_v4().to_string(),
            artifact_id: "web:example/article".into(),
            artifact_type: "web.page".into(),
            url: "https://example.com/article?keep=1".into(),
            title: Some("Article".into()),
            occurred_at: OffsetDateTime::now_utc()
                .replace_minute(0)
                .unwrap()
                .replace_second(0)
                .unwrap()
                .replace_nanosecond(0)
                .unwrap()
                .format(&Rfc3339)
                .unwrap(),
            incognito: false,
            opens: 2,
            focused_seconds: 42,
            background_seconds: 15,
            modes: vec![
                ActivityMode {
                    mode: "view".into(),
                    url: "https://example.com/article?keep=1&secret=2".into(),
                },
                ActivityMode {
                    mode: "review".into(),
                    url: "https://example.com/article/files?secret=2".into(),
                },
            ],
            trails: vec![ActivityTrail {
                from_artifact_id: Some("web:example/source".into()),
                from_url: Some("https://example.com/source?secret=2".into()),
                transition: "link".into(),
            }],
            refs: vec![ActivityRef {
                artifact_type: "jira.issue".into(),
                value: "PROJ-1".into(),
            }],
        }
    }

    fn record_activity(store: &CaptureStore, event: &ActivityEvent) -> CaptureOutcome {
        let origins = ["https://example.com".to_owned()];
        store.record_activity(
            CapturePolicy {
                enabled: true,
                origins: &origins,
                strip_params: &["secret".into()],
            },
            event,
        )
    }

    #[test]
    fn activity_accepts_browser_transition_types_and_the_worker_back_forward_encoding() {
        let mut event = activity();
        for transition in [
            "link",
            "typed",
            "auto_bookmark",
            "auto_subframe",
            "manual_subframe",
            "generated",
            "start_page",
            "form_submit",
            "reload",
            "keyword",
            "keyword_generated",
            "forward_back",
        ] {
            event.trails[0].transition = transition.into();
            assert!(activity_shape_is_valid(&event), "{transition}");
        }
    }

    #[test]
    fn activity_is_measured_markdown_and_replay_is_idempotent() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let event = activity();
        let CaptureOutcome::Persisted { relative_path } = record_activity(&store, &event) else {
            panic!("expected persisted activity");
        };
        let markdown = fs::read_to_string(root.path().join(relative_path)).unwrap();
        assert!(markdown.contains("focused 42s · background 15s · opened 2 time(s)"));
        assert!(markdown.contains("modes view, review"));
        assert!(markdown.contains(" kind=activity data="));
        assert!(!markdown.contains("secret=2"));
        let encoded = markdown
            .split(" data=")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap();
        let stored: ActivityEvent = serde_json::from_slice(&hex::decode(encoded).unwrap()).unwrap();
        assert_eq!(stored.url, "https://example.com/article?keep=1");
        assert_eq!(stored.modes[0].url, "https://example.com/article?keep=1");
        assert_eq!(stored.modes[1].url, "https://example.com/article/files");
        assert_eq!(
            stored.trails[0].from_url.as_deref(),
            Some("https://example.com/source")
        );
        assert_eq!(
            record_activity(&store, &event),
            CaptureOutcome::Suppressed {
                reason: "duplicate_event".into()
            }
        );
    }

    #[test]
    fn batch_appends_visits_and_overflow_once_with_a_compact_intent() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let first = event();
        let mut second = first.clone();
        second.event_id = Uuid::new_v4().to_string();
        let first_time = parse_event_time(&first.occurred_at).unwrap();
        second.occurred_at = (first_time + time::Duration::minutes(1))
            .format(&Rfc3339)
            .unwrap();
        let overflow = OverflowMarker {
            count: 3,
            first_occurred_at: first.occurred_at.clone(),
            last_occurred_at: second.occurred_at.clone(),
        };
        let origins = ["https://example.com".to_owned()];
        let policy = || CapturePolicy {
            enabled: true,
            origins: &origins,
            strip_params: &[],
        };
        let batch_id = Uuid::new_v4().to_string();
        let events = [
            CaptureBatchEvent::Visit(&first),
            CaptureBatchEvent::Visit(&second),
        ];
        let outcome = store.record_batch(policy(), &batch_id, &events, Some(&overflow));
        assert!(outcome.overflow_recorded, "events: {:?}", outcome.events);
        assert_eq!(outcome.events.len(), 2);
        assert!(
            outcome
                .events
                .iter()
                .all(|(_, outcome)| matches!(outcome, CaptureOutcome::Persisted { .. }))
        );
        assert!(
            !store.batch_intent_path().exists(),
            "successful batch clears the compact intent"
        );

        let (_, relative) = store.log_parts(first_time);
        let log = root
            .path()
            .join("log")
            .join(format!("{:04}", first_time.year()))
            .join(format!("{:02}", first_time.month() as u8))
            .join(relative);
        let markdown = fs::read_to_string(&log).unwrap();
        assert_eq!(markdown.matches("\n- ").count(), 3);
        assert!(markdown.contains("Capture overflow: 3 completed activity hour(s)"));

        // Simulate a crash after the compact intent was durable but only a
        // prefix of the combined append reached the daily log.
        let original = fs::read(&log).unwrap();
        let offset = format!("{LOG_HEADER}\n").len();
        let entry = String::from_utf8(original[offset..].to_vec()).unwrap();
        let intent = BatchIntent {
            version: 2,
            batch_id: batch_id.clone(),
            root_path: store.root_path.to_string_lossy().into_owned(),
            root_identity: store.root_identity.clone(),
            relative_log: log
                .strip_prefix(root.path())
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            event_hashes: vec![
                sha256_hex(first.event_id.as_bytes()),
                sha256_hex(second.event_id.as_bytes()),
                sha256_hex(batch_id.as_bytes()),
            ],
            entry: entry.clone(),
            offset: offset as u64,
        };
        publish_batch_intent(&store.batch_intent_path(), &intent).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&log)
            .unwrap()
            .set_len((offset + entry.len() / 2) as u64)
            .unwrap();

        let replay = store.record_batch(policy(), &batch_id, &events, Some(&overflow));
        assert!(replay.overflow_recorded);
        assert!(replay.events.iter().all(|(_, outcome)| matches!(outcome,
            CaptureOutcome::Suppressed { reason } if reason == "duplicate_event")));
        assert_eq!(fs::read(&log).unwrap(), original);
        assert!(!store.batch_intent_path().exists());
    }

    #[test]
    fn compact_intent_refuses_more_entries_than_one_protocol_batch() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let path = store.batch_intent_path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let intent = BatchIntent {
            version: 2,
            batch_id: Uuid::new_v4().to_string(),
            root_path: store.root_path.to_string_lossy().into_owned(),
            root_identity: store.root_identity.clone(),
            relative_log: "log/2026/10/2026-10-16.md".into(),
            event_hashes: vec!["a".repeat(64); MAX_BATCH_ENTRIES + 1],
            entry: "\n- entry\n".into(),
            offset: 0,
        };
        fs::write(path, serde_json::to_vec(&intent).unwrap()).unwrap();

        let error = store.recover_batch_intent().unwrap_err().to_string();
        assert!(error.contains("capture batch intent is invalid"));
    }

    #[test]
    fn pre_sanitizer_activity_and_why_recovery_keeps_exact_marker_payloads() {
        for why in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            let store =
                CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
            let mut activity = activity();
            activity.title = Some("Draft #tag %%comment%%".into());
            let context = WhyEvent {
                event_id: activity.event_id.clone(),
                artifact_id: activity.artifact_id.clone(),
                url: activity.url.clone(),
                occurred_at: activity.occurred_at.clone(),
                incognito: false,
                text: activity.title.clone().unwrap(),
            };
            let deliver = || {
                if why {
                    let origins = ["https://example.com".to_owned()];
                    store.record_why(
                        CapturePolicy {
                            enabled: true,
                            origins: &origins,
                            strip_params: &[],
                        },
                        &context,
                    )
                } else {
                    record_activity(&store, &activity)
                }
            };
            let CaptureOutcome::Persisted { relative_path } = deliver() else {
                panic!("expected fixture event");
            };
            let hash = sha256_hex(activity.event_id.as_bytes());
            let journal = store.journal_path(&hash);
            let mut intent = read_intent(&journal).unwrap().unwrap();
            let log = root.path().join(relative_path);
            let current = fs::read_to_string(&log).unwrap();
            // Only the rendered prose changes; the hex payload is retained.
            let previous =
                current.replace(r"Draft \#tag \%\%comment\%\%", "Draft #tag %%comment%%");
            assert_ne!(previous, current);
            let entry = &previous[intent.offset as usize..];
            intent.entry_hash = sha256_hex(entry.as_bytes());
            fs::write(journal, serde_json::to_vec(&intent).unwrap()).unwrap();
            fs::write(&log, &previous.as_bytes()[..previous.len() - 8]).unwrap();
            let outcome = deliver();
            assert!(
                matches!(outcome, CaptureOutcome::Persisted { .. }),
                "{outcome:?}"
            );
            assert_eq!(fs::read_to_string(&log).unwrap(), previous);
            assert!(matches!(deliver(), CaptureOutcome::Suppressed { .. }));
        }
    }

    #[test]
    fn pre_sanitizer_intents_recover_through_single_and_batch_delivery() {
        for batched in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let state = tempfile::tempdir().unwrap();
            let store =
                CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
            let earlier = event();
            let CaptureOutcome::Persisted { relative_path } = record(&store, &earlier) else {
                panic!("expected fixture visit");
            };
            let log = root.path().join(relative_path);
            let unrelated = fs::read(&log).unwrap();
            let mut visit = event();
            visit.title = Some("Report\u{202e}draft %%comment%% #tag ![embed]".into());
            visit.occurred_at = earlier.occurred_at.clone();
            let hash = sha256_hex(visit.event_id.as_bytes());
            // Exact pre-upgrade renderer output, independent of the current
            // sanitizer. Its durable v1 intent contains only a hash.
            let old_entry = format!(
                "\n- {} — Report\u{202e}draft %%comment%% #tag !\\[embed\\] — <{}>\n{MARKER_PREFIX}{hash} url={} at={} -->\n",
                visit.occurred_at,
                visit.url,
                sha256_hex(visit.url.as_bytes()),
                visit.occurred_at
            );
            let intent = store
                .intent(
                    log.strip_prefix(root.path()).unwrap(),
                    &old_entry,
                    unrelated.len() as u64,
                )
                .unwrap();
            publish_intent(&store.journal_path(&hash), &intent).unwrap();
            let deliver = |value: &VisitEvent| {
                if batched {
                    let origins = ["https://example.com".to_owned()];
                    store
                        .record_batch(
                            CapturePolicy {
                                enabled: true,
                                origins: &origins,
                                strip_params: &[],
                            },
                            &Uuid::new_v4().to_string(),
                            &[CaptureBatchEvent::Visit(value)],
                            None,
                        )
                        .events
                        .remove(0)
                        .1
                } else {
                    record(&store, value)
                }
            };
            // Cut inside the old renderer's multibyte format character.
            let cut = old_entry.find('\u{202e}').unwrap() + 1;
            let mut torn = unrelated.clone();
            torn.extend_from_slice(&old_entry.as_bytes()[..cut]);
            fs::write(&log, &torn).unwrap();
            let mut changed = visit.clone();
            changed.url.push_str("&different=1");
            assert!(matches!(
                deliver(&changed),
                CaptureOutcome::Retryable { .. }
            ));
            assert_eq!(fs::read(&log).unwrap(), torn);
            // Even with a valid intent/hash, an edited tail must be preserved.
            let mut edited = torn.clone();
            *edited.last_mut().unwrap() ^= 1;
            fs::write(&log, &edited).unwrap();
            assert!(matches!(deliver(&visit), CaptureOutcome::Retryable { .. }));
            assert_eq!(fs::read(&log).unwrap(), edited);
            fs::write(&log, &torn).unwrap();
            let outcome = deliver(&visit);
            assert!(
                matches!(outcome, CaptureOutcome::Persisted { .. }),
                "{outcome:?}"
            );
            let mut expected = unrelated;
            expected.extend_from_slice(old_entry.as_bytes());
            assert_eq!(fs::read(&log).unwrap(), expected);
            assert!(matches!(deliver(&visit), CaptureOutcome::Suppressed { .. }));
            assert_eq!(fs::read(&log).unwrap(), expected);
        }
    }

    #[test]
    fn batched_upgrade_delivery_honors_a_legacy_event_intent() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let legacy =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let visit = event();
        assert!(matches!(
            record(&legacy, &visit),
            CaptureOutcome::Persisted { .. }
        ));

        let current =
            CaptureStore::open_at(&storage(root.path(), "other-log"), None, state.path()).unwrap();
        let batch_id = Uuid::new_v4().to_string();
        let origins = ["https://example.com".to_owned()];
        let outcome = current.record_batch(
            CapturePolicy {
                enabled: true,
                origins: &origins,
                strip_params: &[],
            },
            &batch_id,
            &[CaptureBatchEvent::Visit(&visit)],
            None,
        );
        assert!(matches!(
            outcome.events.as_slice(),
            [(id, CaptureOutcome::Suppressed { reason })]
                if id == &visit.event_id && reason == "duplicate_event"
        ));
        assert!(!root.path().join("other-log").exists());
    }

    #[test]
    fn legacy_single_delivery_recovers_a_torn_compact_batch_first() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let first = event();
        let batch_id = Uuid::new_v4().to_string();
        let origins = ["https://example.com".to_owned()];
        let outcome = store.record_batch(
            CapturePolicy {
                enabled: true,
                origins: &origins,
                strip_params: &[],
            },
            &batch_id,
            &[CaptureBatchEvent::Visit(&first)],
            None,
        );
        let CaptureOutcome::Persisted { relative_path } = &outcome.events[0].1 else {
            panic!("expected the fixture batch to persist");
        };
        let log = root.path().join(relative_path);
        let original = fs::read(&log).unwrap();
        let offset = format!("{LOG_HEADER}\n").len();
        let entry = String::from_utf8(original[offset..].to_vec()).unwrap();
        publish_batch_intent(
            &store.batch_intent_path(),
            &BatchIntent {
                version: 2,
                batch_id,
                root_path: store.root_path.to_string_lossy().into_owned(),
                root_identity: store.root_identity.clone(),
                relative_log: relative_path.to_string_lossy().into_owned(),
                event_hashes: vec![sha256_hex(first.event_id.as_bytes())],
                entry: entry.clone(),
                offset: offset as u64,
            },
        )
        .unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&log)
            .unwrap()
            .set_len((offset + entry.len() / 2) as u64)
            .unwrap();

        let mut second = event();
        second.occurred_at = first.occurred_at.clone();
        assert!(matches!(
            record(&store, &second),
            CaptureOutcome::Persisted { .. }
        ));
        let repaired = fs::read_to_string(&log).unwrap();
        assert!(repaired.as_bytes().starts_with(&original));
        assert_eq!(repaired.matches("\n- ").count(), 2);
        assert!(!store.batch_intent_path().exists());
    }

    #[test]
    fn two_manual_logs_for_one_url_a_minute_apart_are_both_recorded() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let first = event();
        let mut second = first.clone();
        second.event_id = Uuid::new_v4().to_string();
        second.occurred_at = (parse_event_time(&first.occurred_at).unwrap()
            + time::Duration::minutes(1))
        .format(&Rfc3339)
        .unwrap();
        assert!(matches!(
            record(&store, &first),
            CaptureOutcome::Persisted { .. }
        ));
        assert!(matches!(
            record(&store, &second),
            CaptureOutcome::Persisted { .. }
        ));
    }

    #[test]
    fn empty_or_oversized_activity_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let mut event = activity();
        event.opens = 0;
        event.focused_seconds = 0;
        event.background_seconds = 0;
        event.modes.clear();
        assert_eq!(
            record_activity(&store, &event),
            CaptureOutcome::Rejected {
                reason: "empty_activity".into()
            }
        );
        event.focused_seconds = 7_201;
        assert_eq!(
            record_activity(&store, &event),
            CaptureOutcome::Rejected {
                reason: "invalid_activity".into()
            }
        );
    }

    #[test]
    fn why_events_append_once_and_escape_markdown() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let why = WhyEvent {
            event_id: Uuid::new_v4().to_string(),
            artifact_id: "web:example/article".into(),
            url: "https://example.com/article".into(),
            occurred_at: OffsetDateTime::now_utc()
                .replace_nanosecond(0)
                .unwrap()
                .format(&Rfc3339)
                .unwrap(),
            incognito: false,
            text: "Fix [release] blocker".into(),
        };
        let origins = ["https://example.com".to_owned()];
        let policy = || CapturePolicy {
            enabled: true,
            origins: &origins,
            strip_params: &[],
        };
        let CaptureOutcome::Persisted { relative_path } = store.record_why(policy(), &why) else {
            panic!("expected persisted why event");
        };
        let markdown = fs::read_to_string(root.path().join(relative_path)).unwrap();
        assert!(markdown.contains("Why: Fix \\[release\\] blocker"));
        assert!(markdown.contains(" kind=why data="));
        assert_eq!(
            store.record_why(policy(), &why),
            CaptureOutcome::Suppressed {
                reason: "duplicate_event".into()
            }
        );
    }

    #[test]
    fn event_id_is_deduplicated_across_day_and_log_directory() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let first =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let visit = event();
        assert!(matches!(
            record(&first, &visit),
            CaptureOutcome::Persisted { .. }
        ));
        let mut shifted = visit.clone();
        shifted.occurred_at = (OffsetDateTime::now_utc() - time::Duration::days(1))
            .replace_nanosecond(0)
            .unwrap()
            .format(&Rfc3339)
            .unwrap();
        let other =
            CaptureStore::open_at(&storage(root.path(), "other-log"), None, state.path()).unwrap();
        assert_eq!(
            record(&other, &shifted),
            CaptureOutcome::Suppressed {
                reason: "duplicate_event".into()
            }
        );
        assert!(!root.path().join("other-log").exists());
    }

    #[test]
    fn event_id_does_not_move_to_a_new_notes_root() {
        let old_root = tempfile::tempdir().unwrap();
        let new_root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let first =
            CaptureStore::open_at(&storage(old_root.path(), "log"), None, state.path()).unwrap();
        let visit = event();
        assert!(matches!(
            record(&first, &visit),
            CaptureOutcome::Persisted { .. }
        ));
        let second =
            CaptureStore::open_at(&storage(new_root.path(), "log"), None, state.path()).unwrap();
        assert_eq!(
            record(&second, &visit),
            CaptureOutcome::Suppressed {
                reason: "duplicate_event".into()
            }
        );
        assert!(!new_root.path().join("log").exists());
    }

    #[test]
    fn partial_visit_is_not_completed_in_a_deselected_root() {
        let old_root = tempfile::tempdir().unwrap();
        let new_root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let first =
            CaptureStore::open_at(&storage(old_root.path(), "log"), None, state.path()).unwrap();
        let visit = event();
        let CaptureOutcome::Persisted { relative_path } = record(&first, &visit) else {
            panic!("expected persisted visit");
        };
        let hash = sha256_hex(visit.event_id.as_bytes());
        let intent = read_intent(&first.journal_path(&hash)).unwrap().unwrap();
        let log = old_root.path().join(relative_path);
        fs::OpenOptions::new()
            .write(true)
            .open(&log)
            .unwrap()
            .set_len(intent.offset + 8)
            .unwrap();
        let before = fs::read(&log).unwrap();
        let second =
            CaptureStore::open_at(&storage(new_root.path(), "log"), None, state.path()).unwrap();
        assert!(matches!(
            record(&second, &visit),
            CaptureOutcome::Retryable { .. }
        ));
        assert_eq!(fs::read(&log).unwrap(), before);
        assert!(!new_root.path().join("log").exists());
    }

    #[test]
    fn exact_partial_entry_is_completed_but_an_edited_tail_is_preserved() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let visit = event();
        let CaptureOutcome::Persisted { relative_path } = record(&store, &visit) else {
            panic!("expected persisted visit");
        };
        let hash = sha256_hex(visit.event_id.as_bytes());
        let intent = read_intent(&store.journal_path(&hash)).unwrap().unwrap();
        let log = root.path().join(relative_path);
        let original = fs::read(&log).unwrap();
        fs::OpenOptions::new()
            .write(true)
            .open(&log)
            .unwrap()
            .set_len(intent.offset + 8)
            .unwrap();
        assert!(matches!(
            record(&store, &visit),
            CaptureOutcome::Persisted { .. }
        ));
        assert_eq!(fs::read(&log).unwrap(), original);

        fs::OpenOptions::new()
            .write(true)
            .open(&log)
            .unwrap()
            .set_len(intent.offset + 8)
            .unwrap();
        let mut file = fs::OpenOptions::new().write(true).open(&log).unwrap();
        file.seek(SeekFrom::Start(intent.offset)).unwrap();
        file.write_all(b"X").unwrap();
        drop(file);
        let before = fs::read(&log).unwrap();
        assert!(matches!(
            record(&store, &visit),
            CaptureOutcome::Retryable { .. }
        ));
        assert_eq!(fs::read(&log).unwrap(), before);
    }

    #[test]
    fn interrupted_entry_resumes_after_visits_appended_since_its_intent() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let first = event();
        let CaptureOutcome::Persisted { relative_path } = record(&store, &first) else {
            panic!("expected persisted visit");
        };
        let hash = sha256_hex(first.event_id.as_bytes());
        let intent = read_intent(&store.journal_path(&hash)).unwrap().unwrap();
        let log = root.path().join(relative_path);
        let first_entry = fs::read(&log).unwrap()[intent.offset as usize..].to_vec();
        // Simulate a crash after the intent was published but before any
        // bytes were appended, followed by another visit.
        fs::OpenOptions::new()
            .write(true)
            .open(&log)
            .unwrap()
            .set_len(intent.offset)
            .unwrap();
        let second = event();
        assert!(matches!(
            record(&store, &second),
            CaptureOutcome::Persisted { .. }
        ));
        let with_second = fs::read(&log).unwrap();
        // Then a resume of the first visit that was itself cut short.
        let mut file = fs::OpenOptions::new().append(true).open(&log).unwrap();
        file.write_all(&first_entry[..10]).unwrap();
        drop(file);

        assert!(matches!(
            record(&store, &first),
            CaptureOutcome::Persisted { .. }
        ));
        let mut expected = with_second;
        expected.extend_from_slice(&first_entry);
        assert_eq!(fs::read(&log).unwrap(), expected);
        assert!(matches!(
            record(&store, &first),
            CaptureOutcome::Suppressed { .. }
        ));
    }

    #[test]
    fn entry_torn_inside_a_multibyte_character_resumes() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let visit = event();
        let CaptureOutcome::Persisted { relative_path } = record(&store, &visit) else {
            panic!("expected persisted visit");
        };
        let hash = sha256_hex(visit.event_id.as_bytes());
        let intent = read_intent(&store.journal_path(&hash)).unwrap().unwrap();
        let log = root.path().join(relative_path);
        let original = fs::read(&log).unwrap();
        let entry = &original[intent.offset as usize..];
        let dash = entry
            .windows("—".len())
            .position(|window| window == "—".as_bytes())
            .unwrap();
        // Cut after the em dash's first byte, leaving invalid UTF-8 at the end.
        let torn = intent.offset + dash as u64 + 1;
        let truncate = || {
            fs::OpenOptions::new()
                .write(true)
                .open(&log)
                .unwrap()
                .set_len(torn)
                .unwrap();
            assert!(std::str::from_utf8(&fs::read(&log).unwrap()).is_err());
        };

        truncate();
        assert!(matches!(
            record(&store, &visit),
            CaptureOutcome::Persisted { .. }
        ));
        assert_eq!(fs::read(&log).unwrap(), original);

        // A different visit that day must not be blocked by the torn tail.
        truncate();
        assert!(matches!(
            record(&store, &event()),
            CaptureOutcome::Persisted { .. }
        ));
    }

    #[test]
    fn oversized_daily_log_is_rejected_not_retried() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let CaptureOutcome::Persisted { relative_path } = record(&store, &event()) else {
            panic!("expected persisted visit");
        };
        let log = root.path().join(relative_path);
        fs::OpenOptions::new()
            .write(true)
            .open(&log)
            .unwrap()
            .set_len(MAX_LOG_BYTES + 1)
            .unwrap();
        let before = fs::metadata(&log).unwrap().len();
        assert_eq!(
            record(&store, &event()),
            CaptureOutcome::Rejected {
                reason: "daily_log_full".into()
            }
        );
        assert_eq!(fs::metadata(&log).unwrap().len(), before);
    }

    #[test]
    fn failed_append_rolls_back_only_its_own_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = Dir::open_ambient_dir(dir.path(), cap_std::ambient_authority()).unwrap();
        let entry = b"\n- entry\n";
        root.write("log.md", b"header\n- entr").unwrap();
        roll_back_append(&root, Path::new("log.md"), 6, entry);
        assert_eq!(root.read("log.md").unwrap(), b"header");

        root.write("log.md", b"header\nuser text").unwrap();
        roll_back_append(&root, Path::new("log.md"), 6, entry);
        assert_eq!(root.read("log.md").unwrap(), b"header\nuser text");
    }

    #[test]
    fn visit_offset_chooses_the_local_calendar_date() {
        let root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let store =
            CaptureStore::open_at(&storage(root.path(), "log"), None, state.path()).unwrap();
        let now = OffsetDateTime::now_utc().replace_nanosecond(0).unwrap();
        let mut dates = Vec::new();
        // These offsets are 26 hours apart, so their local dates always differ.
        for hours in [14, -12] {
            let local = now.to_offset(time::UtcOffset::from_hms(hours, 0, 0).unwrap());
            let mut visit = event();
            visit.occurred_at = local.format(&Rfc3339).unwrap();
            assert_eq!(visit.occurred_at.len(), 25);
            let CaptureOutcome::Persisted { relative_path } = record(&store, &visit) else {
                panic!("expected persisted visit");
            };
            let expected = format!(
                "{:04}-{:02}-{:02}.md",
                local.year(),
                local.month() as u8,
                local.day()
            );
            assert_eq!(relative_path.file_name().unwrap(), expected.as_str());
            dates.push(expected);
        }
        assert_ne!(dates[0], dates[1]);
    }

    #[test]
    fn event_time_accepts_only_seconds_precision_with_real_offsets() {
        assert!(parse_event_time("2026-09-27T18:00:00Z").is_ok());
        assert!(parse_event_time("2026-09-27T18:00:00-07:00").is_ok());
        assert!(parse_event_time("2026-09-27T18:00:00+14:00").is_ok());
        assert!(parse_event_time("2026-09-27T18:00:00+15:00").is_err());
        assert!(parse_event_time("2026-09-27T18:00:00+0700").is_err());
        assert!(parse_event_time("2026-09-27T18:00:00.5Z").is_err());
        assert!(parse_event_time("2026-09-27 18:00:00-07:00").is_err());
    }

    #[test]
    fn missing_original_log_is_retryable_and_not_recreated_elsewhere() {
        let old_root = tempfile::tempdir().unwrap();
        let new_root = tempfile::tempdir().unwrap();
        let state = tempfile::tempdir().unwrap();
        let first =
            CaptureStore::open_at(&storage(old_root.path(), "log"), None, state.path()).unwrap();
        let visit = event();
        let CaptureOutcome::Persisted { relative_path } = record(&first, &visit) else {
            panic!("expected persisted visit");
        };
        fs::remove_file(old_root.path().join(relative_path)).unwrap();
        let second =
            CaptureStore::open_at(&storage(new_root.path(), "log"), None, state.path()).unwrap();
        assert!(matches!(
            record(&second, &visit),
            CaptureOutcome::Retryable { .. }
        ));
        assert!(!new_root.path().join("log").exists());
    }
}
