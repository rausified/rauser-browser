//! On-disk run records: one private directory per run under `<support>/runs/`.
//!
//! Each run directory (0700) holds `status.json`, `page.md`, `stream.txt`,
//! `result.json` and `run.log`, every one of them 0600.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use brauser_protocol::{
    HarnessId, RecordKey, RunAnswer, RunErrorKind, RunKind, RunState, RunView, SiteDraftAnswerView,
    SummaryVocabulary,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::answer::SummaryAnswer;

/// The largest `stream.txt`; text past it is dropped.
pub const STREAM_CAP: usize = 4 * 1024 * 1024;
/// The most raw stream bytes one page returns. Even with every byte escaped
/// six-fold as JSON, a page stays under the 900 KiB outbound frame limit.
pub const STREAM_PAGE_BYTES: usize = 128 * 1024;
/// The largest `run.log`; output past it is dropped.
pub const LOG_CAP: usize = 1024 * 1024;

/// `start_run` bounds from `protocol/schema.json`. The url is measured in
/// bytes (as capture does); content and selection in characters, as the
/// schema counts them.
pub const MAX_URL_BYTES: usize = 8192;
pub const MAX_TITLE_CHARS: usize = 2048;
pub const MAX_CONTENT_CHARS: usize = 1024 * 1024;
pub const MAX_SELECTION_CHARS: usize = 64 * 1024;
/// A stored title is also cut to this many bytes, as notes cut theirs, so
/// twenty run views always fit one outbound frame.
pub const MAX_TITLE_BYTES: usize = 2048;
/// `RunView.error` is at most this long.
pub const MAX_ERROR_BYTES: usize = 2048;
/// The longest harness thread or session id kept, as `RunView` allows.
pub const MAX_SESSION_ID_BYTES: usize = 256;

/// Whether a harness thread or session id may be stored. An unfit id is
/// dropped whole, never cut: a shortened id could name, and so resume, a
/// different conversation.
pub fn fit_session_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_SESSION_ID_BYTES && !id.chars().any(char::is_control)
}

/// Refuse a `start_run` whose fields pass the schema's bounds. A url with a
/// control character is refused too: a browser-serialized URL never has one,
/// and each would grow six-fold in every JSON view of the run.
pub fn check_start_run(
    url: &str,
    title: &str,
    content: &str,
    selection: Option<&str>,
) -> Result<(), &'static str> {
    if url.len() > MAX_URL_BYTES {
        return Err("url is longer than 8192 bytes");
    }
    if url.chars().any(char::is_control) {
        return Err("url contains a control character");
    }
    if title.chars().count() > MAX_TITLE_CHARS {
        return Err("title is longer than 2048 characters");
    }
    if content.chars().count() > MAX_CONTENT_CHARS {
        return Err("content is longer than 1048576 characters");
    }
    if selection.is_some_and(|selection| selection.chars().count() > MAX_SELECTION_CHARS) {
        return Err("selection is longer than 65536 characters");
    }
    Ok(())
}

/// The longest task-bar instruction, in characters, as the schema states.
pub const MAX_TASK_INSTRUCTION_CHARS: usize = 4096;
/// A task's instruction is cut to this many characters (plus "…") in
/// `list_runs`, so twenty views fit one frame.
pub const LIST_INSTRUCTION_CHARS: usize = 280;
/// The longest task report kept and shown, in bytes.
pub const MAX_TASK_REPORT_BYTES: usize = 16 * 1024;

/// The task bar's instruction, as the host and the service both bound it:
/// not blank once trimmed, at most [`MAX_TASK_INSTRUCTION_CHARS`]
/// characters, and no control character but newline and tab. The refusal
/// texts never quote the instruction, so they are safe to log.
pub fn check_task_instruction(text: &str) -> Result<(), &'static str> {
    if text.trim().is_empty() {
        return Err("instruction is empty");
    }
    if text.chars().count() > MAX_TASK_INSTRUCTION_CHARS {
        return Err("instruction is longer than 4096 characters");
    }
    if text
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return Err("instruction contains a control character");
    }
    Ok(())
}

/// A task's instruction as `list_runs` shows it: at most
/// [`LIST_INSTRUCTION_CHARS`] characters, plus "…" when cut.
pub fn list_instruction(text: &str) -> String {
    match text.char_indices().nth(LIST_INSTRUCTION_CHARS) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_string(),
    }
}

/// A task run's request, kept in `status.json` for the run's view and the
/// check when it leaves the queue: the user's instruction and the absolute
/// paths the prompt names as Brauser's own. A queued task is never resumed
/// after a restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskStatus {
    pub instruction: String,
    pub protected: Vec<String>,
}

/// Cut `text` to at most `max` bytes on a character boundary.
pub fn cap_bytes(text: &mut String, max: usize) {
    if text.len() <= max {
        return;
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

/// Control characters become spaces, then the text is cut to `max` bytes.
fn fit_for_view(text: &mut String, max: usize) {
    if text.chars().any(char::is_control) {
        *text = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
    }
    cap_bytes(text, max);
}

const STATUS_VERSION: u32 = 1;
const STATUS_FILE: &str = "status.json";
const PAGE_FILE: &str = "page.md";
const STREAM_FILE: &str = "stream.txt";
const RESULT_FILE: &str = "result.json";
const LOG_FILE: &str = "run.log";

/// The content of a run's `status.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunStatusFile {
    pub version: u32,
    pub run_id: String,
    pub state: RunState,
    pub harness: HarnessId,
    pub service_pid: Option<u32>,
    pub process_group: Option<i32>,
    pub heartbeat_at: Option<String>,
    pub thread_id: Option<String>,
    pub session_id: Option<String>,
    pub cwd: Option<String>,
    pub start_key: RecordKey,
    pub written_key: Option<RecordKey>,
    pub url: String,
    pub title: String,
    pub requested_at: String,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub error: Option<String>,
    pub error_kind: Option<RunErrorKind>,
    pub summary_path: Option<String>,
    pub stream_truncated: bool,
    /// The run's own time limit (its harness entry's `timeout_secs`) and
    /// the vocabulary its prompt offers, as they were when it was queued,
    /// so a run resumed after a restart keeps them. Older records have
    /// neither and resume with the service's limit and the default lists.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_timeout_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocabulary: Option<SummaryVocabulary>,
    /// When the validated answer was kept, stamped once before its first
    /// save, so every save of the run (a retry included) writes the same
    /// event time. An older record gets it on its first retry, and a retry
    /// re-stamps one the daily log's window no longer accepts.
    ///
    /// Like the two fields above, this is optional but not forward
    /// compatible: `deny_unknown_fields` means a service built before it
    /// cannot read a record that has it. `STATUS_VERSION` stays 1 because
    /// none of these has been released.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_at: Option<String>,
    /// The first `answered_at`, kept when a retry re-stamps it, so a save
    /// is still ordered (and its summary file dated) by when the answer was
    /// kept, and a re-stamped old answer never replaces a newer summary;
    /// the re-stamp is only the log event's time. Optional, and not forward
    /// compatible, as the fields above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_answered_at: Option<String>,
    /// The last save failed in a way no retry can fix (the day's daily log
    /// is full, say), so the run offers no Retry save. Optional, and not
    /// forward compatible, as the fields above.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub save_final: bool,
    /// What the run is for: a page summary, or a Capture Site agent draft.
    /// A record without one (every record written before site drafts) is a
    /// summary. Optional, and not forward compatible, as `answered_at`.
    #[serde(default = "summary_kind", skip_serializing_if = "is_summary")]
    pub kind: RunKind,
    /// A task run's request; absent for every other kind. Optional, and
    /// not forward compatible, as `kind`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task: Option<TaskStatus>,
}

fn summary_kind() -> RunKind {
    RunKind::Summary
}

/// Whether `kind` is the summary kind, which `status.json` leaves unwritten.
pub fn is_summary(kind: &RunKind) -> bool {
    *kind == RunKind::Summary
}

impl RunStatusFile {
    /// The protocol view of this run, with `result` its kept `result.json`.
    /// Retry save is offered only for a save failure a retry can fix whose
    /// validated answer is still on disk. A result that is not a [`SummaryAnswer`] (a PR 2
    /// record) shows no answer. A site-draft run never has an answer; its
    /// view carries the validated tables as `site_draft` instead. A task
    /// run carries its full instruction and, once it has succeeded, its
    /// report; it never offers Retry save, because a task saves nothing.
    pub fn view(&self, stream_bytes: u32, result: Option<&serde_json::Value>) -> RunView {
        let mut answer = None;
        let mut site_draft = None;
        let mut instruction = None;
        let mut task_report = None;
        match self.kind {
            RunKind::Summary => {
                answer = result
                    .and_then(|value| SummaryAnswer::deserialize(value).ok())
                    .map(|answer| RunAnswer {
                        document_type: answer.document_type,
                        category: answer.category,
                        importance: answer.importance.level,
                        importance_reason: answer.importance.reason,
                    });
            }
            RunKind::SiteDraft => {
                site_draft = result.and_then(|value| SiteDraftAnswerView::deserialize(value).ok());
            }
            RunKind::Task => {
                instruction = self.task.as_ref().map(|task| task.instruction.clone());
                task_report = result
                    .filter(|_| self.state == RunState::Succeeded)
                    .and_then(|value| value.get("report"))
                    .and_then(serde_json::Value::as_str)
                    .map(|report| {
                        let mut report = report.to_string();
                        cap_bytes(&mut report, MAX_TASK_REPORT_BYTES);
                        report
                    });
            }
        }
        let summary = is_summary(&self.kind);
        RunView {
            run_id: self.run_id.clone(),
            state: self.state,
            harness: self.harness,
            record_key: self.start_key.clone(),
            written_record_key: self.written_key.clone(),
            url: self.url.clone(),
            title: self.title.clone(),
            cwd: self.cwd.clone(),
            created_at: self.created_at.clone(),
            started_at: self.started_at.clone(),
            finished_at: self.finished_at.clone(),
            error: self.error.clone(),
            error_kind: self.error_kind,
            thread_id: self.thread_id.clone(),
            session_id: self.session_id.clone(),
            summary_path: self.summary_path.clone(),
            stream_bytes,
            can_retry_save: self.kind != RunKind::Task
                && self.state == RunState::Failed
                && self.error_kind == Some(RunErrorKind::SaveFailed)
                && !self.save_final
                && result.is_some(),
            answer,
            // Left out for a summary run, as `status.json` leaves it out: an
            // extension built before run kinds rejects unknown keys.
            kind: (!summary).then_some(self.kind),
            site_draft,
            instruction,
            task_report,
        }
    }

    fn id(&self) -> Result<Uuid> {
        Uuid::parse_str(&self.run_id).context("run id is not a UUID")
    }
}

/// One page of `stream.txt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamPage {
    pub text: String,
    pub next_offset: u32,
    pub more: bool,
}

/// Why a stream page could not be read.
#[derive(Debug)]
pub enum StreamError {
    /// The offset falls inside a UTF-8 character.
    NotCharBoundary,
    /// The offset is past the end of the stream.
    PastEnd,
    Io(io::Error),
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotCharBoundary => f.write_str("stream offset is not on a character boundary"),
            Self::PastEnd => f.write_str("stream offset is past the end of the stream"),
            Self::Io(error) => write!(f, "reading the run stream: {error}"),
        }
    }
}

impl std::error::Error for StreamError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for StreamError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// The `runs/` directory and the run directories inside it.
#[derive(Debug, Clone)]
pub struct RunStore {
    runs: PathBuf,
}

impl RunStore {
    /// Open `runs`, creating it if needed. It must be a real directory, never
    /// a symlink; its mode is narrowed to 0700.
    pub fn open(runs: PathBuf) -> Result<Self> {
        match fs::DirBuilder::new().mode(0o700).create(&runs) {
            Ok(()) => {}
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error).with_context(|| format!("creating {}", runs.display()));
            }
        }
        let metadata =
            fs::symlink_metadata(&runs).with_context(|| format!("reading {}", runs.display()))?;
        if metadata.file_type().is_symlink() {
            bail!("{} is a symlink, not a directory", runs.display());
        }
        if !metadata.is_dir() {
            bail!("{} is not a directory", runs.display());
        }
        fs::set_permissions(&runs, fs::Permissions::from_mode(0o700))
            .with_context(|| format!("securing {}", runs.display()))?;
        Ok(Self { runs })
    }

    /// The service's support folder, which holds `runs/` and `agent.env`.
    pub fn support_dir(&self) -> &Path {
        self.runs.parent().unwrap_or(&self.runs)
    }

    pub fn run_dir(&self, id: Uuid) -> PathBuf {
        self.runs.join(id.hyphenated().to_string())
    }

    pub fn page_path(&self, id: Uuid) -> PathBuf {
        self.run_dir(id).join(PAGE_FILE)
    }

    /// Create a new run directory holding `page.md` and `status.json`. An
    /// existing directory for the same run is refused and left untouched.
    pub fn create(&self, status: &RunStatusFile, page_md: &str) -> Result<()> {
        let id = status.id()?;
        let dir = self.run_dir(id);
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .with_context(|| format!("creating run directory {}", dir.display()))?;
        let result = write_new(&dir.join(PAGE_FILE), page_md.as_bytes())
            .context("writing page.md")
            .and_then(|()| self.write_status(status));
        if result.is_err() {
            let _ = fs::remove_dir_all(&dir);
        }
        result
    }

    /// Replace `status.json` atomically: temp file, fsync, rename, dir fsync.
    /// The title and error are made fit for every view first: control
    /// characters (an ANSI colour code, a newline) become spaces, and each is
    /// cut to its byte limit on a character boundary.
    pub fn write_status(&self, status: &RunStatusFile) -> Result<()> {
        let id = status.id()?;
        let unfit = |text: &str, max: usize| text.len() > max || text.chars().any(char::is_control);
        let capped;
        let status = if unfit(&status.title, MAX_TITLE_BYTES)
            || status
                .error
                .as_deref()
                .is_some_and(|error| unfit(error, MAX_ERROR_BYTES))
        {
            let mut copy = status.clone();
            fit_for_view(&mut copy.title, MAX_TITLE_BYTES);
            if let Some(error) = copy.error.as_mut() {
                fit_for_view(error, MAX_ERROR_BYTES);
            }
            capped = copy;
            &capped
        } else {
            status
        };
        let bytes = serde_json::to_vec_pretty(status).context("encoding status.json")?;
        atomic_write(&self.run_dir(id), STATUS_FILE, &bytes).context("writing status.json")
    }

    pub fn status(&self, id: Uuid) -> Result<RunStatusFile> {
        let path = self.run_dir(id).join(STATUS_FILE);
        let bytes = fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let status: RunStatusFile = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing {}", path.display()))?;
        if status.version != STATUS_VERSION {
            bail!(
                "{} has unsupported version {}",
                path.display(),
                status.version
            );
        }
        if status.id()? != id {
            bail!("{} names a different run", path.display());
        }
        Ok(status)
    }

    /// Every readable run, oldest first by `created_at`. Directories whose
    /// names are not canonical UUIDs, and unreadable statuses, are skipped.
    pub fn list(&self) -> Vec<RunStatusFile> {
        let Ok(entries) = fs::read_dir(&self.runs) else {
            return Vec::new();
        };
        let mut runs: Vec<RunStatusFile> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                let id = Uuid::parse_str(&name).ok()?;
                (id.hyphenated().to_string() == name).then_some(id)
            })
            .filter_map(|id| self.status(id).ok())
            .collect();
        runs.sort_by(|a, b| {
            a.created_at
                .cmp(&b.created_at)
                .then_with(|| a.run_id.cmp(&b.run_id))
        });
        runs
    }

    /// Append to `stream.txt`. When `text` would pass [`STREAM_CAP`], only
    /// the part that fits (cut on a character boundary) is written and the
    /// result is `false`; nothing is ever written past the cap.
    pub fn append_stream(&self, id: Uuid, text: &str) -> Result<bool> {
        let mut file = open_append(&self.run_dir(id).join(STREAM_FILE))?;
        let len = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
        let room = STREAM_CAP.saturating_sub(len);
        if text.len() <= room {
            file.write_all(text.as_bytes())?;
            return Ok(true);
        }
        let mut cut = room;
        while !text.is_char_boundary(cut) {
            cut -= 1;
        }
        file.write_all(&text.as_bytes()[..cut])?;
        Ok(false)
    }

    /// The current length of `stream.txt` in bytes; 0 when absent.
    pub fn stream_bytes(&self, id: Uuid) -> Result<u32> {
        match fs::metadata(self.run_dir(id).join(STREAM_FILE)) {
            Ok(metadata) => Ok(u32::try_from(metadata.len()).unwrap_or(u32::MAX)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(0),
            Err(error) => Err(error.into()),
        }
    }

    /// Read at most [`STREAM_PAGE_BYTES`] of `stream.txt` starting at
    /// `since_offset`. A page never ends inside a character: when the page
    /// limit or a torn end of the file falls mid-character, the page stops
    /// before it and `next_offset` points at that character.
    pub fn read_stream(&self, id: Uuid, since_offset: u32) -> Result<StreamPage, StreamError> {
        let mut file = match File::open(self.run_dir(id).join(STREAM_FILE)) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        let len = match &file {
            Some(file) => file.metadata()?.len(),
            None => 0,
        };
        let offset = u64::from(since_offset);
        if offset > len {
            return Err(StreamError::PastEnd);
        }
        let Some(file) = file.as_mut().filter(|_| offset < len) else {
            return Ok(StreamPage {
                text: String::new(),
                next_offset: since_offset,
                more: false,
            });
        };
        let want = (len - offset).min(STREAM_PAGE_BYTES as u64);
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = Vec::with_capacity(want as usize);
        file.take(want).read_to_end(&mut bytes)?;
        if bytes.first().is_some_and(|byte| byte & 0xC0 == 0x80) {
            return Err(StreamError::NotCharBoundary);
        }
        let read_end = offset + bytes.len() as u64;
        let valid = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            // An incomplete character at the end: stop before it.
            Err(error) if error.error_len().is_none() => error.valid_up_to(),
            Err(_) => {
                return Err(StreamError::Io(io::Error::new(
                    ErrorKind::InvalidData,
                    "run stream is not UTF-8",
                )));
            }
        };
        bytes.truncate(valid);
        let text = String::from_utf8(bytes).expect("validated above");
        Ok(StreamPage {
            next_offset: since_offset + valid as u32,
            more: read_end < len,
            text,
        })
    }

    /// Append harness output to `run.log`, dropping whatever passes
    /// [`LOG_CAP`].
    pub fn append_log(&self, id: Uuid, bytes: &[u8]) -> Result<()> {
        let mut file = open_append(&self.run_dir(id).join(LOG_FILE))?;
        let len = usize::try_from(file.metadata()?.len()).unwrap_or(usize::MAX);
        let room = LOG_CAP.saturating_sub(len);
        file.write_all(&bytes[..bytes.len().min(room)])?;
        Ok(())
    }

    /// Replace `result.json` atomically.
    pub fn write_result(&self, id: Uuid, value: &serde_json::Value) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(value).context("encoding result.json")?;
        atomic_write(&self.run_dir(id), RESULT_FILE, &bytes).context("writing result.json")
    }

    /// The validated answer, or `None` before one is written.
    pub fn read_result(&self, id: Uuid) -> Result<Option<serde_json::Value>> {
        let path = self.run_dir(id).join(RESULT_FILE);
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("parsing {}", path.display()))?,
            )),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    /// Delete a run directory. A run that is already gone is not an error.
    pub fn remove(&self, id: Uuid) -> Result<()> {
        let dir = self.run_dir(id);
        match fs::remove_dir_all(&dir) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("removing {}", dir.display())),
        }
    }
}

/// `page.md`: the page the harness summarizes, with any user selection in its
/// own section.
pub fn page_markdown(title: &str, url: &str, content: &str, selection: Option<&str>) -> String {
    let mut page = format!("# {title}\n\nSource: {url}\n\n{content}");
    if let Some(selection) = selection {
        page.push_str("\n\n## Highlighted by the user\n\n");
        page.push_str(selection);
    }
    page
}

fn private_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.mode(0o600);
    options
}

fn open_append(path: &Path) -> io::Result<File> {
    private_options().append(true).create(true).open(path)
}

fn write_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = private_options().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn atomic_write(dir: &Path, name: &str, bytes: &[u8]) -> io::Result<()> {
    let temporary = dir.join(format!(".{name}-{}.tmp", Uuid::new_v4()));
    if let Err(error) = write_new(&temporary, bytes) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    if let Err(error) = fs::rename(&temporary, dir.join(name)) {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }
    File::open(dir)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use brauser_protocol::{HarnessId, RecordKey, RecordKeyKind, RunErrorKind, RunState};
    use std::os::unix::fs::PermissionsExt;

    fn store() -> (tempfile::TempDir, RunStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = RunStore::open(dir.path().join("runs")).unwrap();
        (dir, store)
    }

    fn sample(id: Uuid) -> RunStatusFile {
        RunStatusFile {
            version: 1,
            run_id: id.to_string(),
            state: RunState::Queued,
            harness: HarnessId::Codex,
            service_pid: None,
            process_group: None,
            heartbeat_at: None,
            thread_id: None,
            session_id: None,
            cwd: None,
            start_key: RecordKey {
                kind: RecordKeyKind::Url,
                value: "https://example.com/a".into(),
            },
            written_key: None,
            url: "https://example.com/a".into(),
            title: "A".into(),
            requested_at: "2026-10-26T10:00:00Z".into(),
            created_at: "2026-10-26T10:00:00Z".into(),
            started_at: None,
            finished_at: None,
            error: None,
            error_kind: None,
            summary_path: None,
            stream_truncated: false,
            run_timeout_secs: None,
            vocabulary: None,
            answered_at: None,
            first_answered_at: None,
            save_final: false,
            kind: RunKind::Summary,
            task: None,
        }
    }

    #[test]
    fn start_run_bounds_follow_the_schema() {
        let ok = |url: &str, title: &str, content: &str, selection: Option<&str>| {
            check_start_run(url, title, content, selection)
        };
        let url = format!("https://e.com/{}", "a".repeat(MAX_URL_BYTES - 14));
        assert_eq!(url.len(), MAX_URL_BYTES);
        assert!(ok(&url, "", "", None).is_ok());
        assert!(ok(&format!("{url}a"), "", "", None).is_err());
        assert!(ok("https://e.com/\u{1}", "", "", None).is_err());
        let title = "界".repeat(MAX_TITLE_CHARS);
        assert!(ok("https://e.com/", &title, "", None).is_ok());
        assert!(ok("https://e.com/", &format!("{title}x"), "", None).is_err());
        let content = "é".repeat(MAX_CONTENT_CHARS);
        assert!(ok("https://e.com/", "", &content, None).is_ok());
        assert!(ok("https://e.com/", "", &format!("{content}x"), None).is_err());
        let selection = "x".repeat(MAX_SELECTION_CHARS);
        assert!(ok("https://e.com/", "", "", Some(&selection)).is_ok());
        assert!(ok("https://e.com/", "", "", Some(&format!("{selection}x"))).is_err());
    }

    #[test]
    fn written_title_and_error_fit_their_view_limits() {
        let (_dir, store) = store();
        let id = Uuid::new_v4();
        let mut status = sample(id);
        status.title = "界".repeat(MAX_TITLE_CHARS);
        status.error = Some(format!("\u{1b}[31mfailed\n{}", "é".repeat(MAX_ERROR_BYTES)));
        store.create(&status, "# A\n").unwrap();
        let stored = store.status(id).unwrap();
        assert!(stored.title.len() <= MAX_TITLE_BYTES);
        assert!(stored.title.len() > MAX_TITLE_BYTES - 3);
        let error = stored.error.unwrap();
        assert!(error.len() <= MAX_ERROR_BYTES);
        assert!(error.starts_with(" [31mfailed "), "{error}");
        assert!(!error.chars().any(char::is_control));
    }

    #[test]
    fn twenty_largest_views_fit_one_outbound_frame() {
        // The largest fields a start_run can leave behind, each escaped as
        // much as JSON allows once stored.
        let mut status = sample(Uuid::new_v4());
        status.url = format!("https://e.com/{}", "\\".repeat(MAX_URL_BYTES - 14));
        status.url.truncate(MAX_URL_BYTES);
        let key = RecordKey {
            kind: RecordKeyKind::Url,
            value: "k".repeat(MAX_URL_BYTES),
        };
        status.start_key = key.clone();
        status.written_key = Some(key);
        status.title = "\"".repeat(MAX_TITLE_BYTES);
        status.error = Some("\\".repeat(MAX_ERROR_BYTES));
        status.error_kind = Some(RunErrorKind::SaveFailed);
        status.state = RunState::Failed;
        status.cwd = Some("/".repeat(1024));
        status.summary_path = Some("s".repeat(1024));
        // The widest answer the validation lets through: four-byte chars.
        let wide = |chars: usize| "\u{1F600}".repeat(chars);
        let answer = serde_json::to_value(crate::answer::SummaryAnswer {
            document_type: wide(crate::answer::MAX_LABEL_CHARS),
            category: wide(crate::answer::MAX_LABEL_CHARS),
            entities: crate::answer::Entities {
                technologies: Vec::new(),
                products: Vec::new(),
                people: Vec::new(),
                companies: Vec::new(),
            },
            description: String::new(),
            importance: crate::answer::Importance {
                level: brauser_protocol::ImportanceLevel::Medium,
                reason: wide(crate::answer::MAX_REASON_CHARS),
            },
            assessment: String::new(),
            related_notes: Vec::new(),
        })
        .unwrap();
        let view = status.view(u32::MAX, Some(&answer));
        assert!(view.answer.is_some());
        // A listed task view: its instruction cut for the list, in four-byte
        // characters (the larger encoding), and no report.
        let mut task = largest(task_sample(&"\u{1F600}".repeat(MAX_TASK_INSTRUCTION_CHARS)));
        task.state = RunState::Failed;
        task.error_kind = Some(RunErrorKind::HarnessFailed);
        let mut task_view = task.view(u32::MAX, None);
        task_view.instruction = task_view.instruction.as_deref().map(list_instruction);
        task_view.task_report = None;
        assert_eq!(
            task_view.instruction.as_ref().unwrap().chars().count(),
            LIST_INSTRUCTION_CHARS + 1
        );
        let mut runs = Vec::new();
        for index in 0..crate::service::supervisor::LIST_LIMIT {
            runs.push(if index % 2 == 0 {
                view.clone()
            } else {
                task_view.clone()
            });
        }
        let listed = brauser_protocol::Response::RunsListed(brauser_protocol::RunsListed {
            protocol_version: brauser_protocol::PROTOCOL_VERSION,
            request_id: "r".repeat(128),
            runs,
            more: None,
        });
        let bytes = serde_json::to_vec(&listed).unwrap().len();
        assert!(bytes <= crate::frame::MAX_OUTBOUND_BYTES, "{bytes}");
        // Twenty listed task views fit as well.
        let listed = brauser_protocol::Response::RunsListed(brauser_protocol::RunsListed {
            protocol_version: brauser_protocol::PROTOCOL_VERSION,
            request_id: "r".repeat(128),
            runs: vec![task_view; crate::service::supervisor::LIST_LIMIT],
            more: None,
        });
        let bytes = serde_json::to_vec(&listed).unwrap().len();
        assert!(bytes <= crate::frame::MAX_OUTBOUND_BYTES, "{bytes}");
    }

    #[test]
    fn session_ids_must_be_short_and_printable() {
        assert!(fit_session_id("sess-1"));
        assert!(fit_session_id(&"a".repeat(MAX_SESSION_ID_BYTES)));
        assert!(!fit_session_id(&"a".repeat(MAX_SESSION_ID_BYTES + 1)));
        // Measured in bytes: 128 two-byte characters fit, 129 do not.
        assert!(fit_session_id(&"é".repeat(MAX_SESSION_ID_BYTES / 2)));
        assert!(!fit_session_id(&"é".repeat(MAX_SESSION_ID_BYTES / 2 + 1)));
        assert!(!fit_session_id(""));
        assert!(!fit_session_id("sess\u{1}1"));
        assert!(!fit_session_id("sess\n1"));
        assert!(!fit_session_id("\u{1b}[31msess"));
    }

    fn created(store: &RunStore) -> Uuid {
        let id = Uuid::new_v4();
        store.create(&sample(id), "# A\n").unwrap();
        id
    }

    fn mode(path: &std::path::Path) -> u32 {
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn stream_pages_by_byte_offset_on_char_boundaries() {
        let (_dir, store) = store();
        let id = created(&store);

        let empty = store.read_stream(id, 0).unwrap();
        assert_eq!(
            empty,
            StreamPage {
                text: String::new(),
                next_offset: 0,
                more: false
            }
        );

        assert!(store.append_stream(id, "héllo ").unwrap());
        assert!(store.append_stream(id, "wörld").unwrap());
        let all = store.read_stream(id, 0).unwrap();
        assert_eq!(all.text, "héllo wörld");
        assert_eq!(all.next_offset as usize, "héllo wörld".len());
        assert!(!all.more);

        // Offset 2 is inside the two-byte "é".
        assert!(matches!(
            store.read_stream(id, 2),
            Err(StreamError::NotCharBoundary)
        ));
        let tail = store.read_stream(id, 3).unwrap();
        assert_eq!(tail.text, "llo wörld");
        assert!(matches!(
            store.read_stream(id, 1000),
            Err(StreamError::PastEnd)
        ));

        // A page that would split a character ends before it.
        let (_dir, store) = self::store();
        let id = created(&store);
        let filler = "a".repeat(STREAM_PAGE_BYTES - 1);
        assert!(store.append_stream(id, &filler).unwrap());
        assert!(store.append_stream(id, "éz").unwrap());
        let first = store.read_stream(id, 0).unwrap();
        assert_eq!(first.text, filler);
        assert_eq!(first.next_offset as usize, STREAM_PAGE_BYTES - 1);
        assert!(first.more);
        let second = store.read_stream(id, first.next_offset).unwrap();
        assert_eq!(second.text, "éz");
        assert!(!second.more);
    }

    #[test]
    fn torn_stream_end_snaps_back_to_a_char_boundary() {
        let (_dir, store) = store();
        let id = created(&store);
        assert!(store.append_stream(id, "ab").unwrap());
        // Simulate a reader racing a writer halfway through "é".
        let path = store.run_dir(id).join(STREAM_FILE);
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.push(0xC3);
        std::fs::write(&path, bytes).unwrap();
        let page = store.read_stream(id, 0).unwrap();
        assert_eq!(page.text, "ab");
        assert_eq!(page.next_offset, 2);
        assert!(!page.more);
    }

    #[test]
    fn stream_stops_at_cap() {
        let (_dir, store) = store();
        let id = created(&store);
        let chunk = "x".repeat(1024 * 1024);
        for _ in 0..3 {
            assert!(store.append_stream(id, &chunk).unwrap());
        }
        let almost = "y".repeat(1024 * 1024 - 1);
        assert!(store.append_stream(id, &almost).unwrap());
        // One byte is left; a two-byte character does not fit.
        assert!(!store.append_stream(id, "é").unwrap());
        assert_eq!(store.stream_bytes(id).unwrap() as usize, STREAM_CAP - 1);
        assert!(!store.append_stream(id, "zz").unwrap());
        assert_eq!(store.stream_bytes(id).unwrap() as usize, STREAM_CAP);
        assert!(!store.append_stream(id, "z").unwrap());
        assert_eq!(store.stream_bytes(id).unwrap() as usize, STREAM_CAP);
    }

    #[test]
    fn status_write_is_atomic_and_round_trips() {
        let (_dir, store) = store();
        let id = created(&store);
        let mut status = store.status(id).unwrap();
        assert_eq!(status, sample(id));

        status.state = RunState::Running;
        status.service_pid = Some(42);
        status.process_group = Some(4242);
        status.heartbeat_at = Some("2026-10-26T10:00:05Z".into());
        status.thread_id = Some("thread".into());
        status.written_key = Some(RecordKey {
            kind: RecordKeyKind::Artifact,
            value: "repo:a".into(),
        });
        status.started_at = Some("2026-10-26T10:00:01Z".into());
        store.write_status(&status).unwrap();
        assert_eq!(store.status(id).unwrap(), status);

        // No temporary file is left behind.
        let names: Vec<String> = std::fs::read_dir(store.run_dir(id))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert!(
            names.iter().all(|name| !name.ends_with(".tmp")),
            "{names:?}"
        );

        // Unknown fields are refused.
        let path = store.run_dir(id).join(STATUS_FILE);
        let mut value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        value["surprise"] = serde_json::json!(true);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(store.status(id).is_err());
    }

    #[test]
    fn run_view_offers_retry_save_only_for_save_failed_with_result() {
        let id = Uuid::new_v4();
        let mut status = sample(id);
        status.state = RunState::Failed;
        status.error_kind = Some(RunErrorKind::SaveFailed);
        status.error = Some("The notes folder is gone.".into());
        let kept = serde_json::json!({});
        let view = status.view(12, Some(&kept));
        assert!(view.can_retry_save);
        assert_eq!(view.stream_bytes, 12);
        assert_eq!(view.run_id, id.to_string());
        assert_eq!(view.record_key, status.start_key);
        assert_eq!(view.error.as_deref(), Some("The notes folder is gone."));

        assert!(!status.view(12, None).can_retry_save);
        status.error_kind = Some(RunErrorKind::HarnessFailed);
        assert!(!status.view(12, Some(&kept)).can_retry_save);
        status.error_kind = Some(RunErrorKind::SaveFailed);
        status.state = RunState::Succeeded;
        assert!(!status.view(12, Some(&kept)).can_retry_save);
        // A save failure no retry can fix offers none.
        status.state = RunState::Failed;
        assert!(status.view(12, Some(&kept)).can_retry_save);
        status.save_final = true;
        assert!(!status.view(12, Some(&kept)).can_retry_save);
    }

    #[test]
    fn run_view_answer_comes_from_a_summary_answer_result() {
        let mut status = sample(Uuid::new_v4());
        status.state = RunState::Succeeded;
        assert_eq!(status.view(0, None).answer, None);
        // A PR 2 result is not a summary answer, so it shows none.
        let old = serde_json::json!({"summary": "ok"});
        assert_eq!(status.view(0, Some(&old)).answer, None);
        let answer = serde_json::json!({
            "document_type": "report",
            "category": "news",
            "entities": {"technologies": [], "products": [], "people": [], "companies": []},
            "description": "d",
            "importance": {"level": "low", "reason": "r"},
            "assessment": "a",
            "related_notes": []
        });
        assert_eq!(
            status.view(0, Some(&answer)).answer,
            Some(brauser_protocol::RunAnswer {
                document_type: "report".into(),
                category: "news".into(),
                importance: brauser_protocol::ImportanceLevel::Low,
                importance_reason: "r".into(),
            })
        );
    }

    #[test]
    fn older_status_files_without_kind_read_as_summary() {
        let (_dir, store) = store();
        let id = created(&store);
        let path = store.run_dir(id).join(STATUS_FILE);
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        // A summary record is written as before, with no kind.
        assert!(written.get("kind").is_none(), "{written}");
        let status = store.status(id).unwrap();
        assert_eq!(status.kind, RunKind::Summary);
        // A summary run's view leaves `kind` out too, so a pre-`brwr-f45`
        // version 7 extension, whose run-view check allows only its own
        // keys, still accepts summary runs.
        let view = serde_json::to_value(status.view(0, None)).unwrap();
        assert!(view.get("kind").is_none(), "{view}");
        assert!(view.get("site_draft").is_none(), "{view}");

        let mut site = status.clone();
        site.kind = RunKind::SiteDraft;
        store.write_status(&site).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["kind"], "site_draft");
        assert_eq!(store.status(id).unwrap().kind, RunKind::SiteDraft);
        assert_eq!(site.view(0, None).kind, Some(RunKind::SiteDraft));
    }

    #[test]
    fn site_draft_view_carries_the_validated_tables_and_no_answer() {
        let mut status = sample(Uuid::new_v4());
        status.kind = RunKind::SiteDraft;
        status.state = RunState::Succeeded;
        assert_eq!(status.view(0, None).site_draft, None);
        let tables = serde_json::json!([
            {"type": "tools.docs", "origin": "https://tools.example.com", "title": "{title}"}
        ]);
        let kept = serde_json::json!({ "adapters": tables });
        let view = status.view(0, Some(&kept));
        assert_eq!(view.kind, Some(RunKind::SiteDraft));
        assert_eq!(view.answer, None);
        assert_eq!(
            view.site_draft,
            Some(SiteDraftAnswerView {
                adapters: tables.as_array().unwrap().clone(),
            })
        );
        // A summary answer kept for a site-draft run is not one.
        let summary = serde_json::from_str(crate::answer::TEST_ANSWER).unwrap();
        let view = status.view(0, Some(&summary));
        assert_eq!((view.answer, view.site_draft), (None, None));
        // And a summary run never shows tables.
        status.kind = RunKind::Summary;
        let view = status.view(0, Some(&kept));
        assert_eq!((view.answer, view.site_draft), (None, None));
    }

    fn task_sample(instruction: &str) -> RunStatusFile {
        let mut status = sample(Uuid::new_v4());
        status.kind = RunKind::Task;
        status.task = Some(TaskStatus {
            instruction: instruction.into(),
            protected: vec!["/home/notes".into()],
        });
        status
    }

    #[test]
    fn task_instruction_bounds() {
        assert_eq!(check_task_instruction(""), Err("instruction is empty"));
        assert_eq!(
            check_task_instruction(" \n\t "),
            Err("instruction is empty")
        );
        assert!(check_task_instruction("Summarize this").is_ok());
        assert!(check_task_instruction("one\ntwo\tthree").is_ok());
        let longest = "界".repeat(MAX_TASK_INSTRUCTION_CHARS);
        assert!(check_task_instruction(&longest).is_ok());
        assert_eq!(
            check_task_instruction(&format!("{longest}x")),
            Err("instruction is longer than 4096 characters")
        );
        for bad in ["a\rb", "a\u{1b}[31mb", "a\u{0}b", "a\u{85}b", "a\u{7f}b"] {
            assert_eq!(
                check_task_instruction(bad),
                Err("instruction contains a control character"),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn task_view_carries_instruction_and_report_once_succeeded() {
        let mut status = task_sample("Make a list of the steps");
        let kept = serde_json::json!({ "report": "I made the list." });
        let view = status.view(0, Some(&kept));
        assert_eq!(view.kind, Some(RunKind::Task));
        assert_eq!(
            view.instruction.as_deref(),
            Some("Make a list of the steps")
        );
        // Queued: no report yet, even with a result on disk.
        assert_eq!(view.task_report, None);
        status.state = RunState::Running;
        assert_eq!(status.view(0, Some(&kept)).task_report, None);
        status.state = RunState::Succeeded;
        let view = status.view(0, Some(&kept));
        assert_eq!(view.task_report.as_deref(), Some("I made the list."));
        assert_eq!(view.answer, None);
        let json = serde_json::to_value(&view).unwrap();
        assert_eq!(json["kind"], "task");
        assert_eq!(json["instruction"], "Make a list of the steps");
        assert_eq!(json["task_report"], "I made the list.");
        // A result that is not a report shows none.
        assert_eq!(status.view(0, None).task_report, None);
        let odd = serde_json::json!({ "report": 3 });
        assert_eq!(status.view(0, Some(&odd)).task_report, None);
        // A report kept longer than the cap is cut to fit the view.
        let long = serde_json::json!({ "report": "é".repeat(MAX_TASK_REPORT_BYTES) });
        let report = status.view(0, Some(&long)).task_report.unwrap();
        assert!(report.len() <= MAX_TASK_REPORT_BYTES);
        // A summary answer kept for a task is not an answer.
        let summary = serde_json::from_str(crate::answer::TEST_ANSWER).unwrap();
        let view = status.view(0, Some(&summary));
        assert_eq!((view.answer, view.task_report), (None, None));
    }

    #[test]
    fn task_view_never_parses_its_result_as_a_site_draft() {
        let mut status = task_sample("Draft capture rules");
        status.state = RunState::Succeeded;
        let tables = serde_json::json!([
            {"type": "tools.docs", "origin": "https://tools.example.com", "title": "{title}"}
        ]);
        let kept = serde_json::json!({ "adapters": tables, "report": "done" });
        let view = status.view(0, Some(&kept));
        assert_eq!(view.site_draft, None);
        assert_eq!(view.answer, None);
        assert_eq!(view.task_report.as_deref(), Some("done"));
        let json = serde_json::to_value(&view).unwrap();
        assert!(json.get("site_draft").is_none(), "{json}");
        // And a site-draft run never shows a task's fields.
        let mut site = status.clone();
        site.kind = RunKind::SiteDraft;
        let tables_only = serde_json::json!({ "adapters": tables });
        let view = site.view(0, Some(&tables_only));
        assert_eq!((view.instruction, view.task_report), (None, None));
        assert!(view.site_draft.is_some());
        // And a task never shows a valid site draft either.
        let view = status.view(0, Some(&tables_only));
        assert_eq!((view.site_draft, view.task_report), (None, None));
    }

    #[test]
    fn summary_view_still_has_no_kind_instruction_or_report() {
        let mut status = sample(Uuid::new_v4());
        status.state = RunState::Succeeded;
        // Even a summary record that somehow carries a task shows none of it.
        status.task = task_sample("x").task;
        let kept = serde_json::json!({ "report": "r" });
        let view = serde_json::to_value(status.view(0, Some(&kept))).unwrap();
        for key in ["kind", "instruction", "task_report", "site_draft"] {
            assert!(view.get(key).is_none(), "{key}: {view}");
        }
    }

    #[test]
    fn older_status_files_without_task_still_read() {
        let (_dir, store) = store();
        let id = created(&store);
        let path = store.run_dir(id).join(STATUS_FILE);
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert!(written.get("task").is_none(), "{written}");
        assert_eq!(store.status(id).unwrap().task, None);

        // A task record round-trips with its request.
        let mut task = store.status(id).unwrap();
        task.kind = RunKind::Task;
        task.task = task_sample("Do it").task;
        store.write_status(&task).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["kind"], "task");
        assert_eq!(written["task"]["instruction"], "Do it");
        assert_eq!(written["task"]["protected"][0], "/home/notes");
        assert_eq!(store.status(id).unwrap(), task);

        // Unknown fields inside the task are refused too.
        let mut value = written;
        value["task"]["surprise"] = serde_json::json!(true);
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(store.status(id).is_err());
    }

    #[test]
    fn a_task_view_never_offers_retry_save() {
        let mut status = task_sample("Do it");
        status.state = RunState::Failed;
        status.error_kind = Some(RunErrorKind::SaveFailed);
        status.save_final = false;
        let kept = serde_json::json!({ "report": "r" });
        assert!(!status.view(0, Some(&kept)).can_retry_save);
        // The same record as a summary would offer it.
        status.kind = RunKind::Summary;
        assert!(status.view(0, Some(&kept)).can_retry_save);
    }

    /// The largest fields a run record can hold, each escaped as much as
    /// JSON allows.
    fn largest(mut status: RunStatusFile) -> RunStatusFile {
        status.url = format!("https://e.com/{}", "\\".repeat(MAX_URL_BYTES - 14));
        status.url.truncate(MAX_URL_BYTES);
        let key = RecordKey {
            kind: RecordKeyKind::Url,
            value: "k".repeat(MAX_URL_BYTES),
        };
        status.start_key = key.clone();
        status.written_key = Some(key);
        status.title = "\"".repeat(MAX_TITLE_BYTES);
        status.error = Some("\\".repeat(MAX_ERROR_BYTES));
        status.cwd = Some("/".repeat(1024));
        status.summary_path = Some("s".repeat(1024));
        status.thread_id = Some("\\".repeat(MAX_SESSION_ID_BYTES));
        status.session_id = Some("\\".repeat(MAX_SESSION_ID_BYTES));
        status
    }

    #[test]
    fn largest_task_get_run_fits_one_outbound_frame() {
        // Four-byte characters are written raw (16 KiB); 4096 `"` would
        // escape to only 8 KiB.
        let wide = "\u{1F600}".repeat(MAX_TASK_INSTRUCTION_CHARS);
        assert!(wide.len() > 2 * MAX_TASK_INSTRUCTION_CHARS);
        assert!(check_task_instruction(&wide).is_ok());
        let mut status = largest(task_sample(&wide));
        status.state = RunState::Succeeded;
        let report = "\"".repeat(MAX_TASK_REPORT_BYTES);
        let kept = serde_json::json!({ "report": report });
        let view = status.view(u32::MAX, Some(&kept));
        assert_eq!(view.instruction.as_deref(), Some(wide.as_str()));
        assert_eq!(view.task_report.as_deref(), Some(report.as_str()));
        // A stream page whose every byte escapes six-fold (`\u0001`).
        let fetched = brauser_protocol::Response::RunFetched(brauser_protocol::RunFetched {
            protocol_version: brauser_protocol::PROTOCOL_VERSION,
            request_id: "r".repeat(128),
            run: view,
            stream_text: "\u{1}".repeat(STREAM_PAGE_BYTES),
            next_offset: u32::MAX,
            more: true,
        });
        let bytes = serde_json::to_vec(&fetched).unwrap().len();
        assert!(bytes <= crate::frame::MAX_OUTBOUND_BYTES, "{bytes}");
    }

    #[test]
    fn list_instruction_cuts_to_its_limit_plus_an_ellipsis() {
        assert_eq!(list_instruction("short"), "short");
        let exact = "界".repeat(LIST_INSTRUCTION_CHARS);
        assert_eq!(list_instruction(&exact), exact);
        let cut = list_instruction(&format!("{exact}more"));
        assert_eq!(cut, format!("{exact}…"));
    }

    #[test]
    fn result_round_trips_and_is_absent_until_written() {
        let (_dir, store) = store();
        let id = created(&store);
        assert_eq!(store.read_result(id).unwrap(), None);
        let value = serde_json::json!({"summary": "s", "tags": ["a"]});
        store.write_result(id, &value).unwrap();
        assert_eq!(store.read_result(id).unwrap(), Some(value));
    }

    #[test]
    fn log_is_capped() {
        let (_dir, store) = store();
        let id = created(&store);
        store.append_log(id, &vec![b'a'; LOG_CAP - 3]).unwrap();
        store.append_log(id, b"bcdef").unwrap();
        store.append_log(id, b"more").unwrap();
        let log = std::fs::read(store.run_dir(id).join(LOG_FILE)).unwrap();
        assert_eq!(log.len(), LOG_CAP);
        assert!(log.ends_with(b"abcd"));
    }

    #[test]
    fn page_markdown_puts_selection_in_its_own_section() {
        assert_eq!(
            page_markdown("T", "https://e.com/", "Body", None),
            "# T\n\nSource: https://e.com/\n\nBody"
        );
        assert_eq!(
            page_markdown("T", "https://e.com/", "Body", Some("Picked")),
            "# T\n\nSource: https://e.com/\n\nBody\n\n## Highlighted by the user\n\nPicked"
        );
    }

    #[test]
    fn run_files_are_private() {
        let (dir, store) = store();
        assert_eq!(mode(&dir.path().join("runs")), 0o700);
        let id = created(&store);
        assert_eq!(mode(&store.run_dir(id)), 0o700);
        assert_eq!(mode(&store.page_path(id)), 0o600);
        assert_eq!(
            std::fs::read_to_string(store.page_path(id)).unwrap(),
            "# A\n"
        );
        store.append_stream(id, "s").unwrap();
        store.append_log(id, b"l").unwrap();
        store.write_result(id, &serde_json::json!({})).unwrap();
        for name in [STATUS_FILE, STREAM_FILE, LOG_FILE, RESULT_FILE] {
            assert_eq!(mode(&store.run_dir(id).join(name)), 0o600, "{name}");
        }

        // An existing run directory is refused.
        assert!(store.create(&sample(id), "again").is_err());
        assert_eq!(
            std::fs::read_to_string(store.page_path(id)).unwrap(),
            "# A\n"
        );

        store.remove(id).unwrap();
        assert!(!store.run_dir(id).exists());
        store.remove(id).unwrap();
    }

    #[test]
    fn status_with_a_non_uuid_run_id_is_skipped_by_list() {
        let (dir, store) = store();
        let good = created(&store);

        // A UUID-named directory whose status names a non-UUID run.
        let bad = Uuid::new_v4();
        let bad_dir = dir.path().join("runs").join(bad.to_string());
        std::fs::create_dir(&bad_dir).unwrap();
        let mut status = sample(bad);
        status.run_id = "not-a-uuid".into();
        std::fs::write(
            bad_dir.join(STATUS_FILE),
            serde_json::to_vec(&status).unwrap(),
        )
        .unwrap();
        assert!(store.status(bad).is_err());

        // A status whose run id names a different directory.
        let other = Uuid::new_v4();
        let other_dir = dir.path().join("runs").join(other.to_string());
        std::fs::create_dir(&other_dir).unwrap();
        let status = sample(Uuid::new_v4());
        std::fs::write(
            other_dir.join(STATUS_FILE),
            serde_json::to_vec(&status).unwrap(),
        )
        .unwrap();

        // Unparsable status, and a directory that is not a UUID at all.
        let junk = Uuid::new_v4();
        let junk_dir = dir.path().join("runs").join(junk.to_string());
        std::fs::create_dir(&junk_dir).unwrap();
        std::fs::write(junk_dir.join(STATUS_FILE), b"{").unwrap();
        let named = dir.path().join("runs").join("notes");
        std::fs::create_dir(&named).unwrap();
        std::fs::write(
            named.join(STATUS_FILE),
            serde_json::to_vec(&sample(good)).unwrap(),
        )
        .unwrap();

        let listed: Vec<String> = store.list().into_iter().map(|s| s.run_id).collect();
        assert_eq!(listed, vec![good.to_string()]);
    }
}
