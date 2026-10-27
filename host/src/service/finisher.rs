//! The service's run finisher (spec §5.2 steps 5-6, §7): accepts a §6.4
//! answer, then writes the summary file and the `summary` daily-log event
//! under the configuration current at finish, not the one the run started
//! with. A key, adapter, or home folder changed mid-run is followed.
//!
//! `save` holds the config lock from its refresh to the end, because the
//! artifact index has no lock of its own (`brwr-7vf`). Every failure is a
//! user-facing reason that ends in what to do: Retry save when a retry can
//! work; a retry re-reads the kept answer, so the agent never runs again.
//! A retry is idempotent: the event time is the run's `answered_at` stamp,
//! not the clock, so every save builds the same entry and file; the run ID
//! is the log event's ID, so its line is written once (and an entry rolled
//! back after its intent was journaled replays), and the identical summary
//! file is left unchanged.
//!
//! The daily log is authoritative, and the artifact index catches up from
//! it: once the event is logged, a failure to open or update the index is
//! only reported to stderr, and the save still succeeds.
//!
//! One limit: an append that failed after its intent was journaled can be
//! completed only as it was first built, in the home folder it was built
//! for (`CaptureStore`'s replay). If the home folder or an adapter changed
//! before the retry, the summary file follows the change but the log event
//! cannot be completed, and the reason says so (`LOG_ORIGINAL_FOLDER`,
//! `LOG_INTERRUPTED`) instead of offering a retry that cannot work.

use std::path::{Path, PathBuf};

use brauser_protocol::{ErrorCode, HarnessId, RecordKey, RecordKeyKind};
use serde::Deserialize;
use time::format_description::well_known::Rfc3339;
use time::{OffsetDateTime, UtcOffset};

use super::record::RunStatusFile;
use super::supervisor::{RunFinisher, SavedRun, accept_summary_answer, timestamp};
use crate::answer::SummaryAnswer;
use crate::brand::{APP_NAME, NAMESPACE};
use crate::capture::{CaptureOutcome, CapturePolicy, CaptureStore, canonical_url};
use crate::config::ConfigStore;
use crate::index::ArtifactIndex;
use crate::layout;
use crate::markdown::link_style;
use crate::native::{HOME_UNAVAILABLE, open_index, resolve_record_key};
use crate::summary::{
    MAX_TITLE_BYTES, PageSummary, SessionLink, SummaryDocument, WEB_PAGE_TYPE, render_summary,
};
use crate::vault::{OwnedWrite, Vault, url_record_key};

const INVALID_ANSWER: &str = "The stored answer is not a valid summary.";
const NEEDS_REPAIR: &str = "The configuration needs repair. Fix it in settings, then Retry save.";
const NO_STORAGE: &str = "Choose a notes folder in settings, then Retry save.";
const LAYOUT_MOVING: &str = concat!(
    "Your notes folders are moving to the fixed layout. Open ",
    crate::app_name!(),
    " settings to finish the move, then Retry save."
);
const FOLDER_UNAVAILABLE: &str = "The notes folder is unavailable or was replaced. Choose it again in settings, then Retry save.";
const FILE_UNWRITABLE: &str = "The summary file could not be written. Retry save.";
const LOG_UNWRITABLE: &str = "The daily log could not be written. Retry save.";
const LOG_CLOCK: &str = "The daily log did not accept this summary's time. Check the computer's clock, then Retry save.";
const LOG_FULL: &str =
    "The day's daily log is full, so this summary cannot be logged. The summary file was saved.";
const LOG_NAME_CONFLICT: &str = concat!(
    "A file ",
    crate::app_name!(),
    " does not own is where the daily log goes, in logs/. Move it, then Retry save."
);
const LOG_INVALID: &str = "The run's details are not valid for the daily log, so this summary cannot be logged. The summary file was saved.";
const LOG_INTERRUPTED: &str = "An earlier attempt to log this summary was interrupted, and its daily log entry has changed since, so it cannot be completed. The summary file was saved.";
const LOG_ORIGINAL_FOLDER: &str = "An earlier attempt to log this summary was interrupted in a notes folder that has since changed. Choose that folder again in settings, then Retry save.";

/// Writes a run's summary under the config read at save time.
pub struct SummaryFinisher {
    config_path: PathBuf,
    now: fn() -> OffsetDateTime,
}

impl SummaryFinisher {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            config_path,
            now: OffsetDateTime::now_utc,
        }
    }

    #[cfg(test)]
    fn with_clock(config_path: PathBuf, now: fn() -> OffsetDateTime) -> Self {
        Self { config_path, now }
    }
}

impl RunFinisher for SummaryFinisher {
    fn accept(&self, run: &RunStatusFile, final_text: &str) -> Result<serde_json::Value, String> {
        accept_summary_answer(run, final_text)
    }

    fn save(&self, run: &RunStatusFile, result: &serde_json::Value) -> Result<SavedRun, String> {
        let answer = SummaryAnswer::deserialize(result).map_err(|_| INVALID_ANSWER.to_string())?;
        let mut config = ConfigStore::load_from(self.config_path.clone());
        let _config_lock = config.lock_current().map_err(|error| {
            eprintln!("{NAMESPACE} service: locking the configuration: {error:#}");
            "The configuration could not be locked. Retry save.".to_string()
        })?;
        if config.refresh().is_err() {
            return Err(NEEDS_REPAIR.into());
        }
        // A missing or replaced folder is also a config issue, but its
        // repair is choosing the folder again, so the vault's own message
        // goes first; any other issue is a repair in settings.
        let Some(storage) = config.snapshot().storage.clone() else {
            return Err(if config.config_issue().is_some() {
                NEEDS_REPAIR
            } else {
                NO_STORAGE
            }
            .into());
        };
        let vault = Vault::open_checked(&storage, config.root_identity())
            .map_err(|_| FOLDER_UNAVAILABLE.to_string())?;
        // An older layout (one without a summaries folder included) is a
        // config issue too, but its fix is finishing the move, which
        // settings does when it opens.
        if !layout::is_fixed(&storage) {
            return Err(LAYOUT_MOVING.into());
        }
        if config.config_issue().is_some() || !config.configured() {
            return Err(NEEDS_REPAIR.into());
        }
        let key = resolve_record_key(&mut config, &run.url).map_err(key_error)?;
        let canonical = canonical_url(&run.url, &config.snapshot().strip_params)
            .map_err(|_| "The page URL is invalid; this run cannot be saved.".to_string())?;

        let style = link_style(&storage.profile);
        let relative = vault
            .summary_relative_path(&key, &canonical)
            .map_err(|error| unwritable(&error))?;
        let summary_path = slash_path(&relative).ok_or_else(|| FILE_UNWRITABLE.to_string())?;
        let note_link = vault
            .note_link(&key, &canonical, style)
            .map_err(|error| unwritable(&error))?;
        // The answer's stamp, so every save of the run builds the same log
        // entry; the clock only for a record that has none.
        let now = run
            .answered_at
            .as_deref()
            .and_then(|at| OffsetDateTime::parse(at, &Rfc3339).ok())
            .unwrap_or_else(self.now);
        let occurred_at = summarized_at(now, &run.requested_at);
        let title = event_title(&run.title, &canonical);
        let document = SummaryDocument {
            key: &key,
            canonical_url: &canonical,
            title: &title,
            summarized_at: &occurred_at,
            harness: run.harness,
            session: session_link(run),
            note_link,
            answer: &answer,
        };
        let markdown = render_summary(&document, style);
        match vault.write_summary(&key, &canonical, &markdown) {
            Ok(OwnedWrite::Written(warnings)) => {
                // Published: a failed sync or cleanup is only logged.
                for warning in warnings {
                    eprintln!("{NAMESPACE} service: after writing a summary file: {warning:?}");
                }
            }
            Ok(OwnedWrite::Unchanged) => {}
            Ok(OwnedWrite::Conflict) => {
                return Err(format!(
                    "A file {APP_NAME} does not own is at {summary_path}. Move it, then Retry save."
                ));
            }
            Err(error) => return Err(unwritable(&error)),
        }

        let index = config
            .cache_dir()
            .and_then(|cache_dir| open_index(&config, &storage, &cache_dir));
        if let Err(error) = &index {
            eprintln!("{NAMESPACE} service: opening the artifact index: {error:#}");
        }
        let index = index.ok();
        let (event_key, artifact_type) = summary_artifact(&config, &run.url, &key, index.as_ref());
        let summary = PageSummary {
            event_id: run.run_id.clone(),
            url: run.url.clone(),
            title,
            occurred_at,
            document_type: answer.document_type.clone(),
            category: answer.category.clone(),
            importance: answer.importance.level,
            summary_path: summary_path.clone(),
            record_key: event_key,
            artifact_type,
            harness: run.harness,
        };
        let store = config
            .dir()
            .and_then(|dir| CaptureStore::open_in(&storage, config.root_identity(), dir))
            .map_err(|error| log_failure(&format!("{error:#}")))?;
        let origins = config.capture_origins();
        let policy = CapturePolicy {
            enabled: config.snapshot().capture_enabled,
            origins: &origins,
            strip_params: &config.snapshot().strip_params,
        };
        let (outcome, event) = store.record_summary(policy, &summary);
        let indexed = match (&outcome, event) {
            (CaptureOutcome::Persisted { .. }, Some(event)) => {
                index.as_ref().map(|index| index.record_summary(&event))
            }
            (CaptureOutcome::Suppressed { reason }, _) if reason == "duplicate_event" => {
                index.as_ref().map(ArtifactIndex::reconcile_from_logs)
            }
            (CaptureOutcome::Persisted { .. }, None) => None,
            (CaptureOutcome::Suppressed { reason } | CaptureOutcome::Rejected { reason }, _) => {
                return Err(log_refusal(reason));
            }
            (CaptureOutcome::Retryable { message }, _) => return Err(log_failure(message)),
        };
        // The daily log is authoritative and already holds the event; the
        // index catches up from it on its next read.
        if let Some(Err(error)) = indexed {
            eprintln!("{NAMESPACE} service: updating the artifact index: {error:#}");
        }
        Ok(SavedRun {
            written_key: key,
            summary_path: Some(summary_path),
        })
    }
}

/// `resolve_record_key`'s refusal as a Retry save reason. Its config and
/// folder checks already ran above, so these are races with a change made
/// in between, and get the same messages.
fn key_error((code, message): crate::native::Refusal) -> String {
    match code {
        ErrorCode::InvalidConfig => NEEDS_REPAIR.into(),
        ErrorCode::NotConfigured => NO_STORAGE.into(),
        ErrorCode::Internal if message == HOME_UNAVAILABLE => FOLDER_UNAVAILABLE.into(),
        _ => format!("This page's notes record could not be found ({message}). Retry save."),
    }
}

fn unwritable(error: &anyhow::Error) -> String {
    eprintln!("{NAMESPACE} service: writing a summary file: {error:#}");
    FILE_UNWRITABLE.into()
}

/// The daily log's refusal of the event, as a reason in words. Only a
/// refusal the user can fix offers Retry save; the raw reason goes to
/// stderr. (A summary is an explicit action, so `capture_disabled` and
/// `site_not_allowed` do not apply to it.)
fn log_refusal(reason: &str) -> String {
    eprintln!("{NAMESPACE} service: the daily log refused a summary event: {reason}");
    match reason {
        // A retry re-stamps an answer outside the window (`retry_save`).
        "timestamp_too_old" | "timestamp_in_future" => LOG_CLOCK,
        "daily_log_full" => LOG_FULL,
        "daily_log_name_conflict" => LOG_NAME_CONFLICT,
        _ => LOG_INVALID,
    }
    .into()
}

/// A failed append (`CaptureOutcome::Retryable`, or the store not opening)
/// in words. A replay of an interrupted append that its home folder or
/// entry no longer matches cannot be fixed by retrying; any other failure
/// (I/O, mostly) can.
fn log_failure(message: &str) -> String {
    eprintln!("{NAMESPACE} service: appending a summary event: {message}");
    if message.contains("original home folder changed")
        || message.contains("previously selected home folder")
    {
        LOG_ORIGINAL_FOLDER
    } else if message.contains("manual repair is required") || message.contains("is not owned by") {
        LOG_INTERRUPTED
    } else {
        LOG_UNWRITABLE
    }
    .into()
}

/// A `/`-separated home-relative path, as a summary event links it.
fn slash_path(path: &Path) -> Option<String> {
    let parts = path
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()?;
    Some(parts.join("/"))
}

/// The run's title, or its canonical URL for an untitled page, since a
/// summary event needs a title; at most `MAX_TITLE_BYTES`.
fn event_title(title: &str, canonical: &str) -> String {
    let title = if title.trim().is_empty() {
        canonical
    } else {
        title
    };
    let mut end = title.len().min(MAX_TITLE_BYTES);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    title[..end].to_owned()
}

/// Where the run can be resumed, from the session its harness reported.
fn session_link(run: &RunStatusFile) -> SessionLink {
    match run.harness {
        HarnessId::Codex => run.thread_id.clone().map(SessionLink::CodexThread),
        HarnessId::Claude => run.session_id.clone().map(SessionLink::ClaudeSession),
    }
    .unwrap_or(SessionLink::None)
}

/// `now` in `requested_at`'s offset (the user's clock when they clicked),
/// to the whole second; an unparseable `requested_at` gives UTC (`Z`).
pub(crate) fn summarized_at(now: OffsetDateTime, requested_at: &str) -> String {
    let offset = OffsetDateTime::parse(requested_at, &Rfc3339)
        .map_or(UtcOffset::UTC, |requested| requested.offset());
    let local = now.to_offset(offset);
    let local = local.replace_nanosecond(0).unwrap_or(local);
    local.format(&Rfc3339).unwrap_or_else(|_| timestamp(now))
}

/// The record key and artifact type the summary's log event uses:
/// - a `Url` key is its own `web.page`;
/// - an `Artifact` key takes the type of the adapter that matches `url` to
///   it, or, for a key recovered from an older contract, of its indexed row;
/// - an artifact whose type neither knows falls back to the URL's own key
///   and `web.page`. The summary file keeps the resolved key.
pub(crate) fn summary_artifact(
    config: &ConfigStore,
    url: &str,
    key: &RecordKey,
    index: Option<&ArtifactIndex>,
) -> (RecordKey, String) {
    if key.kind == RecordKeyKind::Url {
        return (key.clone(), WEB_PAGE_TYPE.into());
    }
    let matched = config
        .adapter_match(url, None)
        .ok()
        .filter(|found| found.artifact_id == key.value)
        .map(|found| found.artifact_type);
    let known = matched.or_else(|| {
        index
            .and_then(|index| index.artifact(&key.value).ok().flatten())
            .map(|artifact| artifact.artifact_type)
            .filter(|artifact_type| artifact_type != WEB_PAGE_TYPE)
    });
    if let Some(artifact_type) = known {
        return (key.clone(), artifact_type);
    }
    let url_key = canonical_url(url, &config.snapshot().strip_params)
        .ok()
        .and_then(|canonical| url_record_key(&canonical).ok())
        .unwrap_or_else(|| key.clone());
    (url_key, WEB_PAGE_TYPE.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    use brauser_protocol::{HarnessId, ImportanceLevel, RecordKeyKind, RunState};
    use time::format_description::well_known::Rfc3339;
    use uuid::Uuid;

    use crate::answer::TEST_ANSWER;
    use crate::config::fixtures;
    use crate::summary::{SummaryEvent, web_page_artifact_id};
    use crate::vault::{Vault, url_record_key};

    const PAGE: &str = "https://example.com/a";
    const WORK_PAGE: &str = "https://work.example/items/7";
    const WORK_ID: &str = "work:work.example/7";
    const WORK_ADAPTER: &str = "[[artifacts]]\ntype = \"work.item\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/items/{id:int}\" }]\nid = \"work:{host}/{id}\"\n";
    const WORK_HISTORY: &str = "[[adapter_id_history]]\ntype = \"work.item\"\norigin = \"https://work.example\"\nmatch = [{ path = \"/items/{id:int}\" }]\nid = \"work:{host}/{id}\"\n";

    /// The click time of every test run; only its offset is used.
    const REQUESTED_AT: &str = "2026-10-26T14:00:00-07:00";

    /// The clock of a first save: a whole minute shortly before the test
    /// started, the same on every call. Relative to the real clock, since
    /// the daily log refuses an event outside its window
    /// (`capture::MAX_PAST_SECS`).
    fn fixed_now() -> OffsetDateTime {
        static BASE: std::sync::OnceLock<OffsetDateTime> = std::sync::OnceLock::new();
        *BASE.get_or_init(|| {
            let now = OffsetDateTime::now_utc() - time::Duration::minutes(10);
            now.replace_second(0)
                .and_then(|now| now.replace_nanosecond(0))
                .unwrap()
        })
    }

    /// A minute after [`fixed_now`], as the clock reads on a later retry.
    fn later_now() -> OffsetDateTime {
        fixed_now() + time::Duration::minutes(1)
    }

    /// The record stamp the supervisor writes when it keeps the answer:
    /// 29.75 seconds before [`fixed_now`].
    fn answered_at() -> OffsetDateTime {
        fixed_now() - time::Duration::milliseconds(29_750)
    }

    fn answered_stamp() -> String {
        timestamp(answered_at())
    }

    /// `at` as the summary file and log event show it for a test run.
    fn shown(at: OffsetDateTime) -> String {
        summarized_at(at, REQUESTED_AT)
    }

    /// A home folder and a config.toml naming it, in a temp directory.
    struct World {
        dir: tempfile::TempDir,
        root: PathBuf,
    }

    impl World {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("home");
            fs::create_dir(&root).unwrap();
            let world = Self { dir, root };
            world.configure(&world.root.clone(), "");
            world
        }

        fn config_path(&self) -> PathBuf {
            self.dir.path().join("config.toml")
        }

        /// Rewrite config.toml for `root`, with `adapters` (an adapter
        /// document body) when not empty.
        fn configure(&self, root: &Path, adapters: &str) {
            let tables = if adapters.is_empty() {
                String::new()
            } else {
                format!("[adapters]\nformat = 1\n\n{adapters}")
            };
            fixtures::write(&self.config_path(), root, "", "", &tables);
        }

        /// Rewrite config.toml with only a retired contract for `WORK_PAGE`.
        fn configure_history_only(&self) {
            fixtures::write(&self.config_path(), &self.root, "", "", WORK_HISTORY);
        }

        fn finisher(&self) -> SummaryFinisher {
            SummaryFinisher::with_clock(self.config_path(), fixed_now)
        }

        fn later_finisher(&self) -> SummaryFinisher {
            SummaryFinisher::with_clock(self.config_path(), later_now)
        }

        /// The one daily log file under `logs/`.
        fn log_file(&self) -> PathBuf {
            fn find(dir: &Path) -> Option<PathBuf> {
                for entry in fs::read_dir(dir).ok()? {
                    let path = entry.unwrap().path();
                    if path.is_dir() {
                        if let Some(found) = find(&path) {
                            return Some(found);
                        }
                    } else if path.extension().is_some_and(|ext| ext == "md") {
                        return Some(path);
                    }
                }
                None
            }
            find(&self.root.join("logs")).expect("a daily log")
        }

        fn config(&self) -> ConfigStore {
            ConfigStore::load_from(self.config_path())
        }

        fn vault(&self) -> Vault {
            let config = self.config();
            Vault::open_checked(
                config.snapshot().storage.as_ref().unwrap(),
                config.root_identity(),
            )
            .unwrap()
        }

        fn index(&self) -> ArtifactIndex {
            let config = self.config();
            ArtifactIndex::open_in(
                config.snapshot().storage.as_ref().unwrap(),
                config.root_identity(),
                &config.cache_dir().unwrap(),
            )
            .unwrap()
            .with_adapters(config.adapter_document().ok())
        }

        /// The summary file's home-relative path for a URL-keyed page.
        fn summary_path(&self, url: &str) -> PathBuf {
            let key = url_record_key(url).unwrap();
            self.vault().summary_relative_path(&key, url).unwrap()
        }

        /// Every `summary` event in the daily logs, in file order.
        fn summary_events(&self) -> Vec<SummaryEvent> {
            let mut events = Vec::new();
            collect_summary_events(&self.root.join("logs"), &mut events);
            events
        }
    }

    fn collect_summary_events(dir: &Path, events: &mut Vec<SummaryEvent>) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.map(|entry| entry.unwrap().path()).collect();
        paths.sort();
        for path in paths {
            if path.is_dir() {
                collect_summary_events(&path, events);
                continue;
            }
            let text = fs::read_to_string(&path).unwrap();
            for line in text.lines() {
                let Some((_, data)) = line.split_once(" kind=summary data=") else {
                    continue;
                };
                let hex_data = data.split([' ', '-']).next().unwrap();
                let bytes = hex::decode(hex_data).unwrap();
                events.push(serde_json::from_slice(&bytes).unwrap());
            }
        }
    }

    fn run(world: &World, url: &str, harness: HarnessId) -> RunStatusFile {
        RunStatusFile {
            version: 1,
            run_id: Uuid::new_v4().to_string(),
            state: RunState::Running,
            harness,
            service_pid: None,
            process_group: None,
            heartbeat_at: None,
            thread_id: Some("t-1".into()),
            session_id: Some("s-1".into()),
            cwd: Some(world.root.display().to_string()),
            start_key: url_record_key(url).unwrap(),
            written_key: None,
            url: url.into(),
            title: "A page".into(),
            requested_at: REQUESTED_AT.into(),
            created_at: "2026-10-26T21:00:00Z".into(),
            started_at: None,
            finished_at: None,
            error: None,
            error_kind: None,
            summary_path: None,
            stream_truncated: false,
            run_timeout_secs: None,
            vocabulary: None,
            answered_at: None,
        }
    }

    fn answer() -> serde_json::Value {
        serde_json::from_str(TEST_ANSWER).unwrap()
    }

    fn slash(path: &Path) -> String {
        path.to_string_lossy().replace('\\', "/")
    }

    #[test]
    fn save_writes_the_summary_file_the_log_event_and_an_index_row() {
        let world = World::new();
        let run = run(&world, PAGE, HarnessId::Codex);
        let saved = world.finisher().save(&run, &answer()).unwrap();

        let relative = world.summary_path(PAGE);
        assert_eq!(saved.written_key, url_record_key(PAGE).unwrap());
        assert_eq!(
            saved.summary_path.as_deref(),
            Some(slash(&relative).as_str())
        );
        let file = fs::read_to_string(world.root.join(&relative)).unwrap();
        assert!(
            file.starts_with("---\nbrauser:\n  kind: summary\n"),
            "{file}"
        );
        assert!(
            file.contains(&format!("summarized_at: \"{}\"", shown(fixed_now()))),
            "{file}"
        );
        assert!(file.contains("harness: codex"), "{file}");
        assert!(file.contains("codex://threads/t-1"), "{file}");

        let events = world.summary_events();
        assert_eq!(events.len(), 1, "{events:?}");
        let event = &events[0];
        assert_eq!(event.event_id, run.run_id);
        assert_eq!(event.occurred_at, shown(fixed_now()));
        assert_eq!(event.artifact_type, "web.page");
        assert_eq!(event.record_key, url_record_key(PAGE).unwrap());
        assert_eq!(event.harness, HarnessId::Codex);
        assert_eq!(event.importance, ImportanceLevel::High);
        assert_eq!(event.summary_path, slash(&relative));

        let id = web_page_artifact_id(PAGE).unwrap();
        let (artifact, summary) = world.index().artifact_with_summary(&id).unwrap();
        assert_eq!(artifact.unwrap().artifact_type, "web.page");
        let summary = summary.unwrap();
        assert_eq!(summary.importance, ImportanceLevel::High);
        assert_eq!(summary.summary_path, slash(&relative));
    }

    #[test]
    fn save_never_touches_the_page_note() {
        let world = World::new();
        let vault = world.vault();
        crate::note::save_note(&vault, PAGE, "A page", "My own words", "missing").unwrap();
        let key = url_record_key(PAGE).unwrap();
        let note = world
            .root
            .join(vault.note_relative_path(&key, PAGE).unwrap());
        let before = fs::read(&note).unwrap();
        let modified = fs::metadata(&note).unwrap().modified().unwrap();

        world
            .finisher()
            .save(&run(&world, PAGE, HarnessId::Claude), &answer())
            .unwrap();

        assert_eq!(fs::read(&note).unwrap(), before);
        assert_eq!(fs::metadata(&note).unwrap().modified().unwrap(), modified);
        let file = fs::read_to_string(world.root.join(world.summary_path(PAGE))).unwrap();
        assert!(file.contains("[Page note]("), "{file}");
    }

    #[test]
    fn claude_runs_link_the_session() {
        let world = World::new();
        world
            .finisher()
            .save(&run(&world, PAGE, HarnessId::Claude), &answer())
            .unwrap();
        let file = fs::read_to_string(world.root.join(world.summary_path(PAGE))).unwrap();
        assert!(file.contains("harness: claude"), "{file}");
        assert!(file.contains("s-1"), "{file}");
        assert!(!file.contains("codex://"), "{file}");
        assert_eq!(world.summary_events()[0].harness, HarnessId::Claude);
    }

    #[test]
    fn an_adapter_added_mid_run_is_followed() {
        let world = World::new();
        let run = run(&world, WORK_PAGE, HarnessId::Codex);
        assert_eq!(run.start_key.kind, RecordKeyKind::Url);
        world.configure(&world.root, WORK_ADAPTER);

        let saved = world.finisher().save(&run, &answer()).unwrap();

        let key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: WORK_ID.into(),
        };
        assert_eq!(saved.written_key, key);
        let relative = world
            .vault()
            .summary_relative_path(&key, WORK_PAGE)
            .unwrap();
        assert_eq!(
            saved.summary_path.as_deref(),
            Some(slash(&relative).as_str())
        );
        assert!(world.root.join(&relative).is_file());
        let events = world.summary_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].artifact_id, WORK_ID);
        assert_eq!(events[0].artifact_type, "work.item");
        assert_eq!(events[0].record_key, key);
        let index = world.index();
        let (artifact, summary) = index.artifact_with_summary(WORK_ID).unwrap();
        assert_eq!(artifact.unwrap().artifact_type, "work.item");
        assert!(summary.is_some());
        let web = web_page_artifact_id(WORK_PAGE).unwrap();
        assert!(index.artifact_with_summary(&web).unwrap().0.is_none());
    }

    #[test]
    fn a_recovered_artifact_key_without_a_known_type_logs_under_the_web_page() {
        let world = World::new();
        world.configure_history_only();
        let config = world.config();
        assert!(config.adapter_match(WORK_PAGE, None).is_err());

        let saved = world
            .finisher()
            .save(&run(&world, WORK_PAGE, HarnessId::Codex), &answer())
            .unwrap();

        // The file goes where the page's note is: the recovered artifact key.
        let key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: WORK_ID.into(),
        };
        assert_eq!(saved.written_key, key);
        let relative = world
            .vault()
            .summary_relative_path(&key, WORK_PAGE)
            .unwrap();
        assert!(world.root.join(&relative).is_file());
        // The event, with no known type, falls back to the URL's web page.
        let events = world.summary_events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].artifact_type, "web.page");
        assert_eq!(events[0].record_key, url_record_key(WORK_PAGE).unwrap());
        assert_eq!(
            events[0].artifact_id,
            web_page_artifact_id(WORK_PAGE).unwrap()
        );
        assert_eq!(events[0].summary_path, slash(&relative));
    }

    #[test]
    fn a_recovered_artifact_key_takes_the_indexed_type() {
        let world = World::new();
        world.configure(&world.root, WORK_ADAPTER);
        world
            .finisher()
            .save(&run(&world, WORK_PAGE, HarnessId::Codex), &answer())
            .unwrap();
        // The adapter is removed; its contract is kept as history only.
        world.configure_history_only();

        world
            .finisher()
            .save(&run(&world, WORK_PAGE, HarnessId::Codex), &answer())
            .unwrap();

        let events = world.summary_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[1].artifact_id, WORK_ID);
        assert_eq!(events[1].artifact_type, "work.item");
    }

    #[test]
    fn a_home_folder_changed_mid_run_receives_the_file() {
        let world = World::new();
        let run = run(&world, PAGE, HarnessId::Codex);
        let moved = world.dir.path().join("elsewhere");
        fs::create_dir(&moved).unwrap();
        world.configure(&moved, "");

        let saved = world.finisher().save(&run, &answer()).unwrap();

        let relative = PathBuf::from(saved.summary_path.unwrap());
        assert!(moved.join(&relative).is_file());
        assert!(!world.root.join(&relative).exists());
        assert!(!world.root.join("logs").exists());
        assert!(moved.join("logs").is_dir());
    }

    /// The plan's "summaries_dir changed mid-run" case: the layout is
    /// fixed now, so a different `summaries_dir` is a folder move still in
    /// progress. Nothing is written until it finishes.
    #[test]
    fn a_summaries_dir_changed_mid_run_waits_for_the_layout_move() {
        let world = World::new();
        let run = run(&world, PAGE, HarnessId::Codex);
        fixtures::write(
            &world.config_path(),
            &world.root,
            "summaries_dir = \"other\"\n",
            "",
            "",
        );

        let error = world.finisher().save(&run, &answer()).unwrap_err();

        assert_eq!(
            error,
            "Your notes folders are moving to the fixed layout. Open Brauser settings to finish the move, then Retry save."
        );
        assert!(!world.root.join("other").exists());
        assert!(!world.root.join("summaries").exists());
        assert!(world.summary_events().is_empty());
    }

    #[test]
    fn a_stamped_run_uses_its_answer_time_not_the_clock() {
        let world = World::new();
        let mut run = run(&world, PAGE, HarnessId::Codex);
        run.answered_at = Some(answered_stamp());

        world.later_finisher().save(&run, &answer()).unwrap();

        let file = fs::read_to_string(world.root.join(world.summary_path(PAGE))).unwrap();
        assert!(
            file.contains(&format!("summarized_at: \"{}\"", shown(answered_at()))),
            "{file}"
        );
        assert_eq!(world.summary_events()[0].occurred_at, shown(answered_at()));
    }

    /// Review Focus 4: an append that failed and was rolled back leaves its
    /// intent behind, so the retry must rebuild the identical entry even
    /// though the clock has moved on.
    #[test]
    fn a_retry_after_a_rolled_back_log_append_succeeds_with_a_later_clock() {
        let world = World::new();
        let mut run = run(&world, PAGE, HarnessId::Codex);
        run.answered_at = Some(answered_stamp());
        world.finisher().save(&run, &answer()).unwrap();
        let log = world.log_file();
        let text = fs::read_to_string(&log).unwrap();
        let cut = text.rfind("\n- ").unwrap();
        fs::write(&log, &text[..cut]).unwrap();
        assert!(world.summary_events().is_empty());

        world.later_finisher().save(&run, &answer()).unwrap();

        let events = world.summary_events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].occurred_at, shown(answered_at()));
    }

    /// Leave `run`'s log entry as an append that failed after its intent
    /// was journaled and was rolled back: save it, then cut the entry.
    fn interrupt_the_log_append(world: &World, run: &RunStatusFile) {
        world.finisher().save(run, &answer()).unwrap();
        let log = world.log_file();
        let text = fs::read_to_string(&log).unwrap();
        let cut = text.rfind("\n- ").unwrap();
        fs::write(&log, &text[..cut]).unwrap();
        assert!(world.summary_events().is_empty());
    }

    /// The module doc's limit: an interrupted append is completed only as
    /// it was first built, so an adapter added since cannot be followed for
    /// the log event (the summary file still is), and the reason does not
    /// offer a retry that cannot work.
    #[test]
    fn an_interrupted_log_append_then_an_added_adapter_cannot_be_completed() {
        let world = World::new();
        let mut run = run(&world, WORK_PAGE, HarnessId::Codex);
        run.answered_at = Some(answered_stamp());
        interrupt_the_log_append(&world, &run);
        world.configure(&world.root, WORK_ADAPTER);

        let error = world.later_finisher().save(&run, &answer()).unwrap_err();

        assert_eq!(
            error,
            "An earlier attempt to log this summary was interrupted, and its daily log entry has changed since, so it cannot be completed. The summary file was saved."
        );
        let key = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: WORK_ID.into(),
        };
        let relative = world
            .vault()
            .summary_relative_path(&key, WORK_PAGE)
            .unwrap();
        assert!(world.root.join(&relative).is_file());
        assert!(world.summary_events().is_empty());
    }

    #[test]
    fn an_interrupted_log_append_then_a_new_home_folder_names_the_original() {
        let world = World::new();
        let mut run = run(&world, PAGE, HarnessId::Codex);
        run.answered_at = Some(answered_stamp());
        interrupt_the_log_append(&world, &run);
        let moved = world.dir.path().join("elsewhere");
        fs::create_dir(&moved).unwrap();
        world.configure(&moved, "");

        let error = world.later_finisher().save(&run, &answer()).unwrap_err();

        assert_eq!(
            error,
            "An earlier attempt to log this summary was interrupted in a notes folder that has since changed. Choose that folder again in settings, then Retry save."
        );
        assert!(!moved.join("logs").exists());
    }

    /// The supervisor re-stamps such an answer before a retry; a save given
    /// one anyway names the clock rather than a raw reason.
    #[test]
    fn an_answer_older_than_the_daily_log_window_names_the_clock() {
        let world = World::new();
        let mut run = run(&world, PAGE, HarnessId::Codex);
        run.answered_at = Some(timestamp(fixed_now() - time::Duration::days(31)));

        let error = world.finisher().save(&run, &answer()).unwrap_err();

        assert_eq!(
            error,
            "The daily log did not accept this summary's time. Check the computer's clock, then Retry save."
        );
        assert!(world.summary_events().is_empty());
    }

    #[test]
    fn log_refusals_are_plain_reasons() {
        assert_eq!(
            log_refusal("daily_log_full"),
            "The day's daily log is full, so this summary cannot be logged. The summary file was saved."
        );
        assert_eq!(
            log_refusal("daily_log_name_conflict"),
            "A file Brauser does not own is where the daily log goes, in logs/. Move it, then Retry save."
        );
        assert_eq!(log_refusal("timestamp_in_future"), LOG_CLOCK);
        for reason in ["invalid_summary_key", "title_too_long", "something_new"] {
            assert_eq!(
                log_refusal(reason),
                "The run's details are not valid for the daily log, so this summary cannot be logged. The summary file was saved."
            );
        }
        assert_eq!(
            log_failure("appending daily log entry"),
            "The daily log could not be written. Retry save."
        );
        for reason in [&log_refusal("daily_log_full"), LOG_INVALID, LOG_INTERRUPTED] {
            assert!(!reason.contains("Retry save"), "{reason}");
        }
    }

    #[test]
    fn a_retry_after_success_with_a_later_clock_leaves_the_file_unchanged() {
        let world = World::new();
        let mut run = run(&world, PAGE, HarnessId::Codex);
        run.answered_at = Some(answered_stamp());
        world.finisher().save(&run, &answer()).unwrap();
        let path = world.root.join(world.summary_path(PAGE));
        let written = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();

        world.later_finisher().save(&run, &answer()).unwrap();

        assert_eq!(fs::read(&path).unwrap(), written);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert_eq!(world.summary_events().len(), 1);
    }

    #[test]
    fn an_unavailable_home_folder_refusal_names_the_fix() {
        assert_eq!(
            key_error((ErrorCode::Internal, crate::native::HOME_UNAVAILABLE.into())),
            FOLDER_UNAVAILABLE
        );
    }

    #[test]
    fn a_replaced_notes_folder_fails_with_the_retry_save_message() {
        let world = World::new();
        let run = run(&world, PAGE, HarnessId::Codex);
        fs::rename(&world.root, world.dir.path().join("old")).unwrap();
        fs::create_dir(&world.root).unwrap();

        let error = world.finisher().save(&run, &answer()).unwrap_err();

        assert_eq!(
            error,
            "The notes folder is unavailable or was replaced. Choose it again in settings, then Retry save."
        );
        assert_eq!(fs::read_dir(&world.root).unwrap().count(), 0);
    }

    #[test]
    fn an_unmounted_then_restored_notes_folder_fails_then_retry_save_succeeds() {
        let world = World::new();
        let run = run(&world, PAGE, HarnessId::Codex);
        let away = world.dir.path().join("unmounted");
        fs::rename(&world.root, &away).unwrap();

        let error = world.finisher().save(&run, &answer()).unwrap_err();
        assert!(error.contains("Choose it again"), "{error}");
        assert!(!world.root.exists(), "nothing recreated the folder");

        fs::rename(&away, &world.root).unwrap();
        world.finisher().save(&run, &answer()).unwrap();
        assert_eq!(world.summary_events().len(), 1);
        assert!(world.root.join(world.summary_path(PAGE)).is_file());
    }

    #[test]
    fn an_unowned_file_at_the_summary_name_fails_and_is_untouched() {
        let world = World::new();
        let relative = world.summary_path(PAGE);
        let path = world.root.join(&relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "my own file\n").unwrap();

        let error = world
            .finisher()
            .save(&run(&world, PAGE, HarnessId::Codex), &answer())
            .unwrap_err();

        assert_eq!(
            error,
            format!(
                "A file Brauser does not own is at {}. Move it, then Retry save.",
                slash(&relative)
            )
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "my own file\n");
        assert!(world.summary_events().is_empty());
    }

    #[test]
    fn retry_after_the_fix_writes_once_and_is_idempotent() {
        let world = World::new();
        let run = run(&world, PAGE, HarnessId::Codex);
        let path = world.root.join(world.summary_path(PAGE));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "my own file\n").unwrap();
        assert!(world.finisher().save(&run, &answer()).is_err());

        fs::remove_file(&path).unwrap();
        let first = world.finisher().save(&run, &answer()).unwrap();
        let written = fs::read(&path).unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        let second = world.finisher().save(&run, &answer()).unwrap();

        assert_eq!(first, second);
        assert_eq!(fs::read(&path).unwrap(), written);
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        assert_eq!(world.summary_events().len(), 1);
        let id = web_page_artifact_id(PAGE).unwrap();
        assert!(
            world
                .index()
                .artifact_with_summary(&id)
                .unwrap()
                .1
                .is_some()
        );
    }

    #[test]
    fn a_config_that_needs_repair_or_has_no_folder_names_the_fix() {
        let world = World::new();
        let run = run(&world, PAGE, HarnessId::Codex);
        fs::write(world.config_path(), "not = [valid").unwrap();
        assert_eq!(
            world.finisher().save(&run, &answer()).unwrap_err(),
            "The configuration needs repair. Fix it in settings, then Retry save."
        );
        fs::remove_file(world.config_path()).unwrap();
        assert_eq!(
            world.finisher().save(&run, &answer()).unwrap_err(),
            "Choose a notes folder in settings, then Retry save."
        );
        assert_eq!(
            world
                .finisher()
                .save(&run, &serde_json::json!({}))
                .unwrap_err(),
            "The stored answer is not a valid summary."
        );
    }

    #[test]
    fn summarized_at_uses_the_click_offset() {
        let now = OffsetDateTime::parse("2026-10-26T21:05:00.75Z", &Rfc3339).unwrap();
        assert_eq!(
            summarized_at(now, "2026-10-26T14:00:00-07:00"),
            "2026-10-26T14:05:00-07:00"
        );
        assert_eq!(summarized_at(now, "yesterday"), "2026-10-26T21:05:00Z");
    }

    #[test]
    fn summary_artifact_picks_the_event_identity() {
        let world = World::new();
        let config = world.config();
        let url_key = url_record_key(PAGE).unwrap();
        assert_eq!(
            summary_artifact(&config, PAGE, &url_key, None),
            (url_key.clone(), "web.page".to_string())
        );
        let unknown = RecordKey {
            kind: RecordKeyKind::Artifact,
            value: WORK_ID.into(),
        };
        assert_eq!(
            summary_artifact(&config, WORK_PAGE, &unknown, None),
            (url_record_key(WORK_PAGE).unwrap(), "web.page".to_string())
        );
        world.configure(&world.root, WORK_ADAPTER);
        let config = world.config();
        assert_eq!(
            summary_artifact(&config, WORK_PAGE, &unknown, None),
            (unknown.clone(), "work.item".to_string())
        );
    }
}
