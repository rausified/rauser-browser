//! The run supervisor: queues runs, drives each one through its harness on a
//! worker thread, enforces the timeout, handles cancel, and finishes the run
//! through a [`RunFinisher`] (spec §5.2).
//!
//! Lifecycle of one run:
//! 1. `start_run` writes a `queued` record and submits it to the
//!    [`Scheduler`]; a second run for the same record key returns the first.
//! 2. When a slot frees, a worker thread writes `running`, starts the harness
//!    session, and streams its events into the record. A watchdog thread
//!    refreshes `heartbeat_at` every [`HEARTBEAT`] and kills the process group
//!    at the deadline.
//! 3. The answer is validated (spec §6.4); an invalid one gets one repair
//!    turn in the same session.
//! 4. The outcome is mapped to a terminal state; a validated answer is kept in
//!    `result.json` before `save`, so a save failure can be retried without
//!    running the harness again.
//!
//! Every status transition is a read-modify-write of `status.json` under a
//! per-run mutex, so the heartbeat never overwrites a terminal state.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use brauser_protocol::{
    ErrorCode, HarnessId, RecordKey, RunErrorKind, RunState, RunView, SummaryVocabulary,
};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use uuid::Uuid;

use super::harness_api::{
    HarnessEvent, HarnessRegistry, ProcessSlot, RunSpec, SessionRef, TurnOutcome,
};
use super::maintenance;
use super::prompt::{repair_prompt, summary_prompt};
use super::queue::{Scheduler, Submit};
use super::record::{
    MAX_TITLE_BYTES, RunStatusFile, RunStore, StreamError, StreamPage, cap_bytes, fit_session_id,
    page_markdown,
};
use crate::answer::parse_answer;
use crate::config::{default_summary_vocabulary, valid_summary_vocabulary};
use crate::open::Opener;

/// How long a run may take before its process group is killed.
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(600);
/// How often a running record's `heartbeat_at` is refreshed.
pub const HEARTBEAT: Duration = Duration::from_secs(5);
/// The most runs `list_runs` returns.
pub const LIST_LIMIT: usize = 20;
/// How long `cancel_run` waits for a running run to reach its end state.
const CANCEL_WAIT: Duration = Duration::from_secs(5);

/// Where a successful save wrote the run's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRun {
    pub written_key: RecordKey,
    pub summary_path: Option<String>,
}

/// Validates a harness's final answer and saves it. Errors are user-facing
/// reasons stored in the run's `error`.
pub trait RunFinisher: Send + Sync {
    /// The normalized answer to keep in `result.json`, or what is wrong with
    /// `final_text`; the error goes back to the agent in the repair turn.
    fn accept(&self, run: &RunStatusFile, final_text: &str) -> Result<serde_json::Value, String>;
    /// Writes the run's outputs from its kept answer; `retry_save` calls it
    /// again after a failure (`super::finisher::SummaryFinisher`).
    fn save(&self, run: &RunStatusFile, result: &serde_json::Value) -> Result<SavedRun, String>;
}

/// Validate `final_text` as a §6.4 answer against the run's knowledge base
/// folder (its working directory) and re-serialize it normalized.
pub fn accept_summary_answer(
    run: &RunStatusFile,
    final_text: &str,
) -> Result<serde_json::Value, String> {
    let Some(cwd) = run.cwd.as_deref() else {
        return Err("run has no working directory".into());
    };
    let answer = parse_answer(final_text, Path::new(cwd))?;
    serde_json::to_value(answer).map_err(|_| "the answer could not be encoded".to_string())
}

/// Builds a run's child environment (the service environment overlaid with
/// `agent.env`) when the run leaves the queue. The error is shown as the
/// run's error, so it must never carry a value from the file.
pub type EnvSource = Arc<dyn Fn() -> Result<Vec<(OsString, OsString)>, String> + Send + Sync>;

/// Checks, when a run leaves the queue (including a queued run resumed after
/// a restart), that the folder it was queued to run in is still the
/// knowledge base folder the user picked, so a folder replaced or remounted
/// at the same path while the run waited is never handed to a harness. The
/// error is shown as the run's error.
pub type CwdCheck = Arc<dyn Fn(&Path) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub struct ServiceOptions {
    /// The limit for a run that brings none of its own.
    pub run_timeout: Duration,
    /// Set only by the scripted `RUN_TIMEOUT_SECS_ENV` seam: this limit
    /// beats every run's own, so a smoke can time out a configured harness.
    pub forced_run_timeout: Option<Duration>,
    /// How long the service stays up with nothing queued or running.
    pub idle_exit: Duration,
    /// Recorded in each run's `status.json` for recovery.
    pub service_pid: u32,
    /// Read at the start of every run, including runs resumed after a
    /// restart; a run whose environment cannot be built fails before its
    /// harness starts.
    pub run_env: EnvSource,
    /// Run at the start of every run, right after `run_env`; a run whose
    /// folder fails it ends before its harness starts.
    pub run_cwd: CwdCheck,
    /// Opens a run's Codex thread and reveals `agent.env`.
    pub opener: Opener,
}

impl ServiceOptions {
    /// The limit for a run whose harness entry asks for `requested`.
    fn timeout_for(&self, requested: Option<Duration>) -> Duration {
        self.forced_run_timeout
            .unwrap_or_else(|| requested.unwrap_or(self.run_timeout))
    }
}

impl fmt::Debug for ServiceOptions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServiceOptions")
            .field("run_timeout", &self.run_timeout)
            .field("forced_run_timeout", &self.forced_run_timeout)
            .field("idle_exit", &self.idle_exit)
            .field("service_pid", &self.service_pid)
            .finish_non_exhaustive()
    }
}

/// A request to start a run. The server resolves the record key and the
/// working directory; the child environment comes from
/// [`ServiceOptions::run_env`] when the run starts, so the supervisor never
/// reads `agent.env` itself.
#[derive(Debug, Clone)]
pub struct StartRun {
    pub url: String,
    pub title: String,
    pub content: String,
    pub selection: Option<String>,
    pub harness: Option<HarnessId>,
    pub record_key: RecordKey,
    pub requested_at: String,
    pub cwd: PathBuf,
    /// The harness entry's `timeout_secs`; `None` uses
    /// [`ServiceOptions::run_timeout`].
    pub timeout: Option<Duration>,
    /// The document types and categories the prompt offers.
    pub vocabulary: SummaryVocabulary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    Cancel,
    Timeout,
}

#[derive(Debug, Default)]
struct StopState {
    reason: Option<Stop>,
    /// Set once the answer is accepted or the harness turn has returned;
    /// later stops kill nothing and do not change how the run ends.
    turn_done: bool,
}

/// What the worker needs that is not in `status.json`.
struct Launch {
    cwd: PathBuf,
    vocabulary: SummaryVocabulary,
}

/// The in-memory side of one queued or running run.
struct RunControl {
    slot: ProcessSlot,
    /// How long the run may take once it is running.
    timeout: Duration,
    stop: Mutex<StopState>,
    launch: Mutex<Option<Launch>>,
    done: Mutex<bool>,
    done_changed: Condvar,
}

impl RunControl {
    fn new(launch: Launch, timeout: Duration) -> Self {
        Self {
            slot: ProcessSlot::default(),
            timeout,
            stop: Mutex::new(StopState::default()),
            launch: Mutex::new(Some(launch)),
            done: Mutex::new(false),
            done_changed: Condvar::new(),
        }
    }

    /// Record why the run is being stopped (the first reason wins) and kill
    /// its process group, unless the harness turn is already over.
    fn request_stop(&self, reason: Stop) {
        let mut stop = lock(&self.stop);
        if stop.turn_done {
            return;
        }
        stop.reason.get_or_insert(reason);
        self.slot.kill();
    }

    /// Whether a cancel or timeout has been requested.
    fn stop_requested(&self) -> bool {
        lock(&self.stop).reason.is_some()
    }

    /// Mark the turn over and return why it was stopped, if it was. Later
    /// stops change nothing; calling this again returns the same reason.
    fn end_turn(&self) -> Option<Stop> {
        let mut stop = lock(&self.stop);
        stop.turn_done = true;
        stop.reason
    }

    fn mark_done(&self) {
        *lock(&self.done) = true;
        self.done_changed.notify_all();
    }

    /// Wait until the run is done or `timeout` passes; true when done.
    fn wait_done(&self, timeout: Duration) -> bool {
        let done = lock(&self.done);
        let (done, _) = self
            .done_changed
            .wait_timeout_while(done, timeout, |done| !*done)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *done
    }
}

/// Test seam: given a thread's name, true makes creating it fail.
#[cfg(test)]
type SpawnFault = Arc<dyn Fn(&str) -> bool + Send + Sync>;

struct Inner {
    store: RunStore,
    scheduler: Mutex<Scheduler>,
    runs: Mutex<HashMap<Uuid, Arc<RunControl>>>,
    record_locks: Mutex<HashMap<Uuid, Arc<Mutex<()>>>>,
    registry: HarnessRegistry,
    finisher: Box<dyn RunFinisher>,
    options: ServiceOptions,
    last_activity: Mutex<Instant>,
    /// Serializes [`maintenance::prune`] between worker threads.
    prune_lock: Mutex<()>,
    #[cfg(test)]
    spawn_fault: std::sync::OnceLock<SpawnFault>,
}

/// Owns every run of this service process. Cheap to clone.
#[derive(Clone)]
pub struct Supervisor {
    inner: Arc<Inner>,
}

impl Supervisor {
    /// Recover the runs a previous service process left behind (interrupted
    /// `running` records fail with "service restarted"), prune old records,
    /// then re-submit the runs that were still queued, oldest first.
    pub fn new(
        store: RunStore,
        registry: HarnessRegistry,
        finisher: Box<dyn RunFinisher>,
        options: ServiceOptions,
    ) -> Self {
        let (supervisor, queued) = Self::recovered(store, registry, finisher, options);
        supervisor.resume(queued);
        supervisor
    }

    /// As [`Supervisor::new`], with thread creation failing wherever `fault`
    /// says so, from the first resumed run on.
    #[cfg(test)]
    fn with_spawn_fault(
        store: RunStore,
        registry: HarnessRegistry,
        finisher: Box<dyn RunFinisher>,
        options: ServiceOptions,
        fault: SpawnFault,
    ) -> Self {
        let (supervisor, queued) = Self::recovered(store, registry, finisher, options);
        let _ = supervisor.inner.spawn_fault.set(fault);
        supervisor.resume(queued);
        supervisor
    }

    /// Recover and prune the records a previous process left behind; the
    /// still-queued runs are returned for [`Supervisor::resume`].
    fn recovered(
        store: RunStore,
        registry: HarnessRegistry,
        finisher: Box<dyn RunFinisher>,
        options: ServiceOptions,
    ) -> (Self, Vec<Uuid>) {
        let now = OffsetDateTime::now_utc();
        let queued =
            maintenance::recover(&store, now, options.service_pid, &group_alive, &kill_group);
        maintenance::prune(&store, now);
        let supervisor = Self {
            inner: Arc::new(Inner {
                store,
                scheduler: Mutex::new(Scheduler::default()),
                runs: Mutex::new(HashMap::new()),
                record_locks: Mutex::new(HashMap::new()),
                registry,
                finisher,
                options,
                last_activity: Mutex::new(Instant::now()),
                prune_lock: Mutex::new(()),
                #[cfg(test)]
                spawn_fault: std::sync::OnceLock::new(),
            }),
        };
        (supervisor, queued)
    }

    /// Re-submit recovered queued runs. A run without a recorded working
    /// directory, or a second queued run for the same record key, cannot be
    /// resumed and fails with "service restarted".
    fn resume(&self, queued: Vec<Uuid>) {
        let inner = &self.inner;
        for id in queued {
            let Ok(status) = inner.store.status(id) else {
                continue;
            };
            let resumable = status.cwd.as_ref().map(|cwd| {
                let mut scheduler = lock(&inner.scheduler);
                (scheduler.submit(&status.start_key, id), PathBuf::from(cwd))
            });
            match resumable {
                Some((Submit::New, cwd)) => {
                    // The run's own limit and vocabulary, as recorded when
                    // it was queued; a record without them (or with a
                    // vocabulary no configuration could hold) gets the
                    // service's limit and the default lists.
                    let requested = status
                        .run_timeout_secs
                        .filter(|secs| *secs > 0)
                        .map(Duration::from_secs);
                    let timeout = inner.options.timeout_for(requested);
                    let vocabulary = status
                        .vocabulary
                        .filter(valid_summary_vocabulary)
                        .unwrap_or_else(default_summary_vocabulary);
                    let launch = Launch { cwd, vocabulary };
                    let control = RunControl::new(launch, timeout);
                    lock(&inner.runs).insert(id, Arc::new(control));
                }
                Some((Submit::Existing(_), _)) | None => inner.finish(id, |status| {
                    status.state = RunState::Failed;
                    status.error_kind = Some(RunErrorKind::ServiceRestarted);
                    status.error = Some(maintenance::SERVICE_RESTARTED.to_string());
                }),
            }
        }
        Inner::pump(inner);
    }

    /// The runners this service starts runs with; harness setup probes
    /// through them too.
    pub fn registry(&self) -> &HarnessRegistry {
        &self.inner.registry
    }

    /// The run records, for reading a run's thread without touching it.
    pub fn store(&self) -> &RunStore {
        &self.inner.store
    }

    /// Queue a run, or return the queued-or-running run for the same record
    /// key (`true` = existing).
    pub fn start_run(&self, mut req: StartRun) -> Result<(RunView, bool), (ErrorCode, String)> {
        let inner = &self.inner;
        cap_bytes(&mut req.title, MAX_TITLE_BYTES);
        inner.touch();
        let Some(runner) = inner.registry.get(req.harness) else {
            return Err((
                ErrorCode::NotConfigured,
                "No AI harness is set up for summaries.".to_string(),
            ));
        };
        let id = Uuid::new_v4();
        {
            let mut scheduler = lock(&inner.scheduler);
            if let Submit::Existing(owner) = scheduler.submit(&req.record_key, id) {
                drop(scheduler);
                return inner.view(owner).map(|view| (view, true));
            }
            let now = now_timestamp();
            let status = RunStatusFile {
                version: 1,
                run_id: id.to_string(),
                state: RunState::Queued,
                harness: runner.harness(),
                service_pid: Some(inner.options.service_pid),
                process_group: None,
                heartbeat_at: None,
                thread_id: None,
                session_id: None,
                cwd: Some(req.cwd.display().to_string()),
                start_key: req.record_key.clone(),
                written_key: None,
                url: req.url.clone(),
                title: req.title.clone(),
                requested_at: req.requested_at.clone(),
                created_at: now,
                started_at: None,
                finished_at: None,
                error: None,
                error_kind: None,
                summary_path: None,
                stream_truncated: false,
                run_timeout_secs: req.timeout.map(|timeout| timeout.as_secs()),
                vocabulary: Some(req.vocabulary.clone()),
                answered_at: None,
            };
            let page = page_markdown(&req.title, &req.url, &req.content, req.selection.as_deref());
            if let Err(error) = inner.store.create(&status, &page) {
                scheduler.finished(id);
                eprintln!("brauser service: could not create run {id}: {error:#}");
                return Err((
                    ErrorCode::Internal,
                    "The run could not be recorded.".to_string(),
                ));
            }
            let timeout = inner.options.timeout_for(req.timeout);
            let launch = Launch {
                cwd: req.cwd,
                vocabulary: req.vocabulary,
            };
            let control = RunControl::new(launch, timeout);
            lock(&inner.runs).insert(id, Arc::new(control));
        }
        Inner::pump(inner);
        inner.view(id).map(|view| (view, false))
    }

    /// The run's view and the stream page starting at `since_offset`.
    pub fn get_run(
        &self,
        id: Uuid,
        since_offset: u32,
    ) -> Result<(RunView, StreamPage), (ErrorCode, String)> {
        self.inner.touch();
        let view = self.inner.view(id)?;
        let page = self
            .inner
            .store
            .read_stream(id, since_offset)
            .map_err(|error| match error {
                StreamError::NotCharBoundary | StreamError::PastEnd => {
                    (ErrorCode::InvalidRequest, error.to_string())
                }
                StreamError::Io(_) => {
                    eprintln!("brauser service: reading run {id} stream: {error}");
                    (
                        ErrorCode::Internal,
                        "The run's output could not be read.".to_string(),
                    )
                }
            })?;
        Ok((view, page))
    }

    /// Every queued or running run plus the newest finished ones, at most
    /// [`LIST_LIMIT`], newest first.
    pub fn list_runs(&self) -> Vec<RunView> {
        self.inner.touch();
        let mut runs = self.inner.store.list();
        runs.reverse();
        let (active, finished): (Vec<_>, Vec<_>) = runs
            .into_iter()
            .partition(|run| matches!(run.state, RunState::Queued | RunState::Running));
        let room = LIST_LIMIT.saturating_sub(active.len());
        let mut chosen: Vec<RunStatusFile> = active
            .into_iter()
            .take(LIST_LIMIT)
            .chain(finished.into_iter().take(room))
            .collect();
        chosen.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.run_id.cmp(&a.run_id))
        });
        chosen
            .iter()
            .filter_map(|run| {
                let id = Uuid::parse_str(&run.run_id).ok()?;
                Some(self.inner.view_of(id, run))
            })
            .collect()
    }

    /// Cancel a run. A queued run is dequeued and never starts; a running
    /// run's process group is killed and this waits briefly for it to end.
    /// A finished run is returned unchanged.
    pub fn cancel_run(&self, id: Uuid) -> Result<RunView, (ErrorCode, String)> {
        let inner = &self.inner;
        inner.touch();
        inner.view(id)?;
        let control = lock(&inner.runs).get(&id).cloned();
        let mut scheduler = lock(&inner.scheduler);
        if scheduler.is_waiting(id) {
            // Dequeue only once `canceled` is durable: a run still `queued`
            // on disk would be resumed after a restart. The scheduler lock
            // keeps `pump` from starting it in between.
            let finished = now_timestamp();
            if let Err(error) = inner.update(id, |status| {
                status.state = RunState::Canceled;
                status.error_kind = Some(RunErrorKind::Canceled);
                status.finished_at = Some(finished);
            }) {
                drop(scheduler);
                eprintln!("brauser service: recording cancel of run {id}: {error:#}");
                return Err((
                    ErrorCode::Internal,
                    "The run could not be canceled.".to_string(),
                ));
            }
            scheduler.dequeue(id);
            drop(scheduler);
            lock(&inner.runs).remove(&id);
            if let Some(control) = control {
                control.mark_done();
            }
            inner.prune();
            Inner::pump(inner);
        } else if let Some(control) = control {
            drop(scheduler);
            control.request_stop(Stop::Cancel);
            control.wait_done(CANCEL_WAIT);
        }
        inner.view(id)
    }

    /// Save a run whose save failed, from its kept `result.json`, without
    /// running the harness again. Anything else is a conflict.
    pub fn retry_save(&self, id: Uuid) -> Result<RunView, (ErrorCode, String)> {
        let inner = &self.inner;
        inner.touch();
        let Some(record_lock) = inner.record_lock(id) else {
            return Err(no_such_run());
        };
        let _guard = lock(&record_lock);
        let mut status = inner.status(id)?;
        let result = inner.store.read_result(id).ok().flatten();
        let retryable = status.view(0, result.as_ref()).can_retry_save;
        let Some(result) = result.filter(|_| retryable) else {
            return Err((
                ErrorCode::Conflict,
                "This run has no failed save to retry.".to_string(),
            ));
        };
        // A record kept before answers were stamped gets its stamp now,
        // before the save, so later retries reuse it.
        if status.answered_at.is_none() {
            status.answered_at = Some(now_timestamp());
            inner.store.write_status(&status).map_err(|error| {
                eprintln!("brauser service: stamping run {id} answer: {error:#}");
                (
                    ErrorCode::Internal,
                    "The run could not be updated.".to_string(),
                )
            })?;
        }
        match inner.finisher.save(&status, &result) {
            Ok(saved) => {
                status.state = RunState::Succeeded;
                status.error = None;
                status.error_kind = None;
                status.written_key = Some(saved.written_key);
                status.summary_path = saved.summary_path;
            }
            Err(reason) => status.error = Some(reason),
        }
        status.finished_at = Some(now_timestamp());
        inner.store.write_status(&status).map_err(|error| {
            eprintln!("brauser service: recording retry save of run {id}: {error:#}");
            (
                ErrorCode::Internal,
                "The run could not be updated.".to_string(),
            )
        })?;
        Ok(inner.view_of(id, &status))
    }

    /// True when nothing is queued or running.
    pub fn is_idle(&self) -> bool {
        lock(&self.inner.scheduler).is_idle()
    }

    /// True once the service has been idle, with no requests, for
    /// `idle_exit`.
    pub fn idle_expired(&self) -> bool {
        self.is_idle() && lock(&self.inner.last_activity).elapsed() >= self.inner.options.idle_exit
    }
}

impl Inner {
    fn touch(&self) {
        *lock(&self.last_activity) = Instant::now();
    }

    /// The run's mutex, or `None` when it has no record. Ids come from
    /// extension messages, so a lock is only made for a run that exists;
    /// [`Inner::prune`] drops it once the record is gone. Both check the
    /// record under the map lock, so a live run only ever has one mutex.
    fn record_lock(&self, id: Uuid) -> Option<Arc<Mutex<()>>> {
        let mut locks = lock(&self.record_locks);
        if let Some(record_lock) = locks.get(&id) {
            return Some(record_lock.clone());
        }
        self.store
            .run_dir(id)
            .exists()
            .then(|| locks.entry(id).or_default().clone())
    }

    fn status(&self, id: Uuid) -> Result<RunStatusFile, (ErrorCode, String)> {
        if !self.store.run_dir(id).exists() {
            return Err(no_such_run());
        }
        self.store.status(id).map_err(|error| {
            eprintln!("brauser service: reading run {id}: {error:#}");
            (
                ErrorCode::Internal,
                "The run's record could not be read.".to_string(),
            )
        })
    }

    fn view(&self, id: Uuid) -> Result<RunView, (ErrorCode, String)> {
        let status = self.status(id)?;
        Ok(self.view_of(id, &status))
    }

    fn view_of(&self, id: Uuid, status: &RunStatusFile) -> RunView {
        let stream_bytes = self.store.stream_bytes(id).unwrap_or(0);
        // Only a run past validation has a result to read.
        let result = matches!(status.state, RunState::Succeeded | RunState::Failed)
            .then(|| self.store.read_result(id).ok().flatten())
            .flatten();
        status.view(stream_bytes, result.as_ref())
    }

    /// Read-modify-write `status.json` under the run's mutex.
    fn update(
        &self,
        id: Uuid,
        change: impl FnOnce(&mut RunStatusFile),
    ) -> anyhow::Result<RunStatusFile> {
        let Some(record_lock) = self.record_lock(id) else {
            anyhow::bail!("run {id} has no record");
        };
        let _guard = lock(&record_lock);
        let mut status = self.store.status(id)?;
        change(&mut status);
        self.store.write_status(&status)?;
        Ok(status)
    }

    /// Start a worker for every run that can take a free slot.
    ///
    /// A run whose worker cannot be created fails, freeing its slot for the
    /// next queued run, so the queue is taken again until a pass starts
    /// every ready run. Each failure ends a run, so this terminates.
    fn pump(this: &Arc<Self>) {
        loop {
            let ready: Vec<Uuid> = {
                let mut scheduler = lock(&this.scheduler);
                std::iter::from_fn(|| scheduler.next_ready()).collect()
            };
            let mut freed = false;
            for id in ready {
                let worker = this.clone();
                let spawned = this.spawn(format!("run-{id}"), move || Inner::run(&worker, id));
                if let Err(error) = spawned {
                    eprintln!("brauser service: could not start a worker for run {id}: {error}");
                    this.finish(id, |status| {
                        status.state = RunState::Failed;
                        status.error_kind = Some(RunErrorKind::HarnessFailed);
                        status.error = Some("The run could not be started.".to_string());
                    });
                    freed = true;
                }
            }
            if !freed {
                return;
            }
        }
    }

    /// One run, start to end state, on its worker thread.
    fn run(this: &Arc<Self>, id: Uuid) {
        let Some(control) = lock(&this.runs).get(&id).cloned() else {
            this.finish(id, |_| {});
            return;
        };
        let launch = lock(&control.launch).take();
        // agent.env as it is now, not as it was when the run was queued.
        let env = match (this.options.run_env)() {
            Ok(env) => env,
            Err(reason) => {
                control.end_turn();
                this.finish_with(&control, id, |status| {
                    status.state = RunState::Failed;
                    status.error_kind = Some(RunErrorKind::HarnessFailed);
                    status.error = Some(reason);
                });
                return;
            }
        };
        // The folder as it is now, not as it was when the run was queued.
        if let Some(Err(reason)) = launch
            .as_ref()
            .map(|launch| (this.options.run_cwd)(&launch.cwd))
        {
            control.end_turn();
            this.finish_with(&control, id, |status| {
                status.state = RunState::Failed;
                status.error_kind = Some(RunErrorKind::HarnessFailed);
                status.error = Some(reason);
            });
            return;
        }
        let started = now_timestamp();
        let status = this.update(id, |status| {
            status.state = RunState::Running;
            status.started_at = Some(started.clone());
            status.heartbeat_at = Some(started);
            status.service_pid = Some(this.options.service_pid);
        });
        let (status, launch) = match (status, launch) {
            (Ok(status), Some(launch)) => (status, launch),
            (status, _) => {
                if let Err(error) = status {
                    eprintln!("brauser service: recording start of run {id}: {error:#}");
                }
                control.end_turn();
                this.finish_with(&control, id, |status| {
                    status.state = RunState::Failed;
                    status.error_kind = Some(RunErrorKind::HarnessFailed);
                    status.error = Some("The run could not be started.".to_string());
                });
                return;
            }
        };

        let watchdog = {
            let worker = this.clone();
            let control = control.clone();
            this.spawn(format!("run-{id}-watchdog"), move || {
                worker.watch(id, &control);
            })
        };
        // Without a watchdog nothing enforces the timeout, so the harness
        // never starts.
        let watchdog = match watchdog {
            Ok(watchdog) => watchdog,
            Err(error) => {
                eprintln!("brauser service: no watchdog for run {id}: {error}");
                control.end_turn();
                this.finish_with(&control, id, |status| {
                    status.state = RunState::Failed;
                    status.error_kind = Some(RunErrorKind::HarnessFailed);
                    status.error = Some("The run could not be started.".to_string());
                });
                return;
            }
        };

        let outcome = this.drive(id, &control, &status, launch, env);
        let stopped = control.end_turn();
        let finished = match (stopped, outcome) {
            (Some(Stop::Cancel), _) => Finish::fail(RunErrorKind::Canceled, None),
            (Some(Stop::Timeout), _) => Finish::fail(
                RunErrorKind::TimedOut,
                Some(timeout_message(control.timeout)),
            ),
            (None, Driven::Failed(reason)) => {
                Finish::fail(RunErrorKind::HarnessFailed, Some(reason))
            }
            (None, Driven::Invalid(reason)) => {
                Finish::fail(RunErrorKind::InvalidOutput, Some(reason))
            }
            (None, Driven::Accepted(result)) => this.conclude(id, &result),
        };
        this.finish_with(&control, id, |status| finished.apply(status));
        let _ = watchdog.join();
    }

    /// Start the harness session and run its turn, streaming events into the
    /// record, then validate the answer. An invalid answer gets one repair
    /// turn in the same session; both turns stream into `stream.txt`.
    fn drive(
        self: &Arc<Self>,
        id: Uuid,
        control: &RunControl,
        status: &RunStatusFile,
        launch: Launch,
        env: Vec<(OsString, OsString)>,
    ) -> Driven {
        let Some(runner) = self.registry.get(Some(status.harness)) else {
            return Driven::Failed("The AI harness is no longer set up.".to_string());
        };
        let page_path = self.store.page_path(id);
        let spec = RunSpec {
            run_id: id,
            harness: status.harness,
            title: status.title.clone(),
            prompt: summary_prompt(&page_path, &launch.cwd, &launch.vocabulary),
            cwd: launch.cwd,
            page_path,
            env,
        };
        // Record each child's process group the moment it is spawned, so a
        // restart can kill a harness that hangs without writing anything.
        let worker = Arc::downgrade(self);
        control.slot.set_on_spawn(Some(Arc::new(move |group| {
            if let Some(worker) = worker.upgrade() {
                worker.note(id, |status| status.process_group = Some(group));
            }
        })));
        let mut session = match runner.start(&spec, &control.slot) {
            Ok(session) => session,
            Err(error) => return Driven::Failed(format!("starting the harness: {error:#}")),
        };
        let mut truncated = false;
        let mut sink = |event| match event {
            HarnessEvent::Text(text) => match self.store.append_stream(id, &text) {
                Ok(true) => {}
                Ok(false) if !truncated => {
                    truncated = true;
                    self.note(id, |status| status.stream_truncated = true);
                }
                Ok(false) => {}
                Err(error) => eprintln!("brauser service: run {id} stream: {error:#}"),
            },
            HarnessEvent::Diagnostic(bytes) => {
                if let Err(error) = self.store.append_log(id, &bytes) {
                    eprintln!("brauser service: run {id} log: {error:#}");
                }
            }
            HarnessEvent::Session(reference) => {
                let (SessionRef::CodexThread(handle) | SessionRef::ClaudeSession(handle)) =
                    &reference;
                if !fit_session_id(handle) {
                    eprintln!(
                        "brauser service: run {id} ignored an over-long or unprintable session id"
                    );
                    return;
                }
                match reference {
                    SessionRef::CodexThread(thread) => {
                        self.note(id, |status| status.thread_id = Some(thread));
                    }
                    SessionRef::ClaudeSession(session) => {
                        self.note(id, |status| status.session_id = Some(session));
                    }
                }
            }
        };
        let driven = match session.turn(&spec.prompt, &mut sink) {
            TurnOutcome::Failed { reason } => Driven::Failed(reason),
            TurnOutcome::Answered { final_text } => match self.finisher.accept(status, &final_text)
            {
                Ok(result) => Driven::Accepted(result),
                // A cancel or timeout between the turns starts no repair
                // turn; the stop decides how the run ends.
                Err(_) if control.stop_requested() => {
                    Driven::Failed("The run was stopped.".to_string())
                }
                Err(problem) => match session.turn(&repair_prompt(&problem), &mut sink) {
                    TurnOutcome::Failed { reason } => Driven::Failed(reason),
                    TurnOutcome::Answered { final_text } => {
                        match self.finisher.accept(status, &final_text) {
                            Ok(result) => Driven::Accepted(result),
                            Err(problem) => Driven::Invalid(format!(
                                "The agent's answer was not valid after one correction: {problem}"
                            )),
                        }
                    }
                },
            },
        };
        // An accepted answer stands: a cancel or timeout from here on (while
        // the session closes, which can take a while for Codex, or while the
        // answer is saved) cannot overturn it. A stop already requested
        // still wins.
        if matches!(driven, Driven::Accepted(_)) {
            control.end_turn();
        }
        session.close();
        driven
    }

    /// Keep and save a validated answer. The record is stamped with the
    /// answer's time before the save, and the save gets the record as it
    /// is now (with the session the harness reported), so a retry writes
    /// the same event as this save would have.
    fn conclude(&self, id: Uuid, result: &serde_json::Value) -> Finish {
        const NOT_KEPT: &str = "The answer could not be kept for saving.";
        if let Err(error) = self.store.write_result(id, result) {
            eprintln!("brauser service: keeping run {id} result: {error:#}");
            return Finish::fail(RunErrorKind::SaveFailed, Some(NOT_KEPT.to_string()));
        }
        let answered = now_timestamp();
        let status = match self.update(id, |status| {
            status.answered_at.get_or_insert(answered);
        }) {
            Ok(status) => status,
            Err(error) => {
                eprintln!("brauser service: stamping run {id} answer: {error:#}");
                return Finish::fail(RunErrorKind::SaveFailed, Some(NOT_KEPT.to_string()));
            }
        };
        match self.finisher.save(&status, result) {
            Ok(saved) => Finish::Succeeded(saved),
            Err(reason) => Finish::fail(RunErrorKind::SaveFailed, Some(reason)),
        }
    }

    /// Refresh the heartbeat until the run is done; kill it at the deadline.
    fn watch(&self, id: Uuid, control: &RunControl) {
        let deadline = Instant::now() + control.timeout;
        let mut next_beat = Instant::now() + HEARTBEAT;
        let mut timed_out = false;
        loop {
            let now = Instant::now();
            if !timed_out && now >= deadline {
                timed_out = true;
                control.request_stop(Stop::Timeout);
            }
            if now >= next_beat {
                next_beat = now + HEARTBEAT;
                let beat = now_timestamp();
                self.note(id, |status| {
                    if status.state == RunState::Running {
                        status.heartbeat_at = Some(beat);
                    }
                });
            }
            let mut wake = next_beat;
            if !timed_out {
                wake = wake.min(deadline);
            }
            if control.wait_done(wake.saturating_duration_since(Instant::now())) {
                return;
            }
        }
    }

    /// Start a named thread. Tests can make this fail through
    /// [`Supervisor::with_spawn_fault`].
    fn spawn(
        &self,
        name: String,
        body: impl FnOnce() + Send + 'static,
    ) -> io::Result<JoinHandle<()>> {
        #[cfg(test)]
        if self.spawn_fault.get().is_some_and(|fault| fault(&name)) {
            return Err(io::Error::other("thread creation failed (test)"));
        }
        thread::Builder::new().name(name).spawn(body)
    }

    /// A best-effort status change during a run.
    fn note(&self, id: Uuid, change: impl FnOnce(&mut RunStatusFile)) {
        if let Err(error) = self.update(id, change) {
            eprintln!("brauser service: updating run {id}: {error:#}");
        }
    }

    fn finish_with(
        self: &Arc<Self>,
        control: &RunControl,
        id: Uuid,
        change: impl FnOnce(&mut RunStatusFile),
    ) {
        self.finish(id, change);
        control.mark_done();
        Inner::pump(self);
    }

    /// Write the end state, then free the run's slot and key. The scheduler
    /// lock is held across both, so no caller sees a finished record whose
    /// key is still taken. (Lock order: scheduler, then a record lock.)
    fn finish(&self, id: Uuid, change: impl FnOnce(&mut RunStatusFile)) {
        let finished = now_timestamp();
        let mut scheduler = lock(&self.scheduler);
        if let Err(error) = self.update(id, |status| {
            change(status);
            status.finished_at = Some(finished);
        }) {
            eprintln!("brauser service: recording the end of run {id}: {error:#}");
        }
        scheduler.finished(id);
        drop(scheduler);
        lock(&self.runs).remove(&id);
        self.touch();
        self.prune();
    }

    /// Apply retention after a run finishes, then drop the locks of runs
    /// whose records are gone. A record whose lock is held (a retry save in
    /// progress) is skipped this time. The lock map stays locked across each
    /// removal, so no new holder can appear while the directory goes.
    fn prune(&self) {
        let _guard = lock(&self.prune_lock);
        maintenance::prune_with(&self.store, OffsetDateTime::now_utc(), |id, remove| {
            let locks = lock(&self.record_locks);
            match locks.get(&id) {
                None => remove(),
                Some(record_lock) => record_lock.try_lock().is_ok_and(|_held| remove()),
            }
        });
        lock(&self.record_locks).retain(|id, _| self.store.run_dir(*id).exists());
    }
}

fn no_such_run() -> (ErrorCode, String) {
    (
        ErrorCode::InvalidRequest,
        "There is no such run.".to_string(),
    )
}

/// Whether process group `group` still exists.
fn group_alive(group: i32) -> bool {
    rustix::process::Pid::from_raw(group)
        .is_some_and(|pid| rustix::process::test_kill_process_group(pid).is_ok())
}

/// SIGKILL process group `group`, ignoring failure.
fn kill_group(group: i32) {
    if let Some(pid) = rustix::process::Pid::from_raw(group) {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
}

/// How a run's conversation with its harness ended.
enum Driven {
    /// A validated answer, normalized for `result.json`.
    Accepted(serde_json::Value),
    /// The answer was still invalid after the repair turn.
    Invalid(String),
    /// The harness could not start, or a turn failed.
    Failed(String),
}

/// A run's end state.
enum Finish {
    Succeeded(SavedRun),
    Failed {
        kind: RunErrorKind,
        error: Option<String>,
    },
}

impl Finish {
    fn fail(kind: RunErrorKind, error: Option<String>) -> Self {
        Self::Failed { kind, error }
    }

    fn apply(self, status: &mut RunStatusFile) {
        match self {
            Self::Succeeded(saved) => {
                status.state = RunState::Succeeded;
                status.error = None;
                status.error_kind = None;
                status.written_key = Some(saved.written_key);
                status.summary_path = saved.summary_path;
            }
            Self::Failed { kind, error } => {
                status.state = if kind == RunErrorKind::Canceled {
                    RunState::Canceled
                } else {
                    RunState::Failed
                };
                status.error_kind = Some(kind);
                status.error = error;
            }
        }
    }
}

/// "Timed out after 10 minutes"; timeouts under a minute (tests) are shown in
/// whole seconds, rounded up.
pub fn timeout_message(timeout: Duration) -> String {
    let seconds = timeout.as_secs() + u64::from(timeout.subsec_nanos() > 0);
    let (count, unit) = if seconds >= 60 {
        (seconds.div_ceil(60), "minute")
    } else {
        (seconds.max(1), "second")
    };
    let plural = if count == 1 { "" } else { "s" };
    format!("Timed out after {count} {unit}{plural}")
}

/// RFC 3339 UTC with exactly nine fraction digits, so timestamps sort as
/// strings (`RunStore::list` orders by `created_at`).
fn now_timestamp() -> String {
    timestamp(OffsetDateTime::now_utc())
}

/// `now` in the record timestamp format of [`now_timestamp`].
pub(crate) fn timestamp(now: OffsetDateTime) -> String {
    let now = now.to_offset(time::UtcOffset::UTC);
    let whole = now
        .replace_nanosecond(0)
        .ok()
        .and_then(|whole| whole.format(&Rfc3339).ok())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string());
    let base = whole.strip_suffix('Z').unwrap_or(&whole);
    format!("{base}.{:09}Z", now.nanosecond())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use brauser_protocol::{
        ErrorCode, HarnessId, RecordKey, RecordKeyKind, RunErrorKind, RunState,
    };
    use uuid::Uuid;

    use crate::service::record::MAX_SESSION_ID_BYTES;

    use crate::answer::TEST_ANSWER;
    use crate::brand::FAKE_HARNESS_ENV;
    use crate::service::finisher::SummaryFinisher;
    use crate::service::harness_api::{HarnessRegistry, HarnessRunner, HarnessSession};
    use crate::service::record::{RunStatusFile, RunStore};

    /// Accepts a §6.4 answer and saves nothing, counting its saves and
    /// keeping the record the last one was given.
    struct TestFinisher {
        saves: Arc<AtomicUsize>,
        seen: SeenRun,
    }

    type SeenRun = Arc<Mutex<Option<RunStatusFile>>>;

    impl RunFinisher for TestFinisher {
        fn accept(
            &self,
            run: &RunStatusFile,
            final_text: &str,
        ) -> Result<serde_json::Value, String> {
            accept_summary_answer(run, final_text)
        }

        fn save(
            &self,
            run: &RunStatusFile,
            _result: &serde_json::Value,
        ) -> Result<SavedRun, String> {
            self.saves.fetch_add(1, Ordering::SeqCst);
            *self.seen.lock().unwrap() = Some(run.clone());
            Ok(SavedRun {
                written_key: run.start_key.clone(),
                summary_path: Some("summaries/a.summary.md".to_string()),
            })
        }
    }

    type SharedEnv = Arc<Mutex<Result<Vec<(OsString, OsString)>, String>>>;
    /// Builds a fixture's finisher from its temp directory, in place of the
    /// default [`TestFinisher`].
    type FinisherFactory = Box<dyn FnOnce(&Path) -> Box<dyn RunFinisher>>;

    /// A file in a fixture run's folder that makes the fixture's folder
    /// check fail, as if the knowledge base folder had been replaced.
    const FOLDER_CHANGED: &str = "folder-changed";
    const FOLDER_CHANGED_ERROR: &str = "The knowledge base folder changed.";

    struct Fixture {
        dir: tempfile::TempDir,
        supervisor: Supervisor,
        saves: Arc<AtomicUsize>,
        /// The record the [`TestFinisher`]'s last save was given.
        seen: SeenRun,
        /// What the run environment source returns when a run starts.
        env: SharedEnv,
    }

    /// The environment every fixture run gets unless a test changes it.
    fn default_env(dir: &Path) -> Vec<(OsString, OsString)> {
        vec![
            ("PATH".into(), "/bin:/usr/bin".into()),
            ("RELEASE".into(), dir.join("release").into_os_string()),
            ("COUNT".into(), dir.join("count").into_os_string()),
            ("VALID_ANSWER".into(), TEST_ANSWER.into()),
        ]
    }

    impl Fixture {
        fn new(body: &str) -> Self {
            Self::with_timeout(body, DEFAULT_RUN_TIMEOUT)
        }

        fn with_timeout(body: &str, run_timeout: Duration) -> Self {
            Self::seeded(body, run_timeout, |_, _| {})
        }

        /// Seed `runs/` before the supervisor starts, as a previous service
        /// process would have left it.
        fn seeded(body: &str, run_timeout: Duration, seed: impl FnOnce(&RunStore, &Path)) -> Self {
            Self::seeded_with_env(body, run_timeout, None, seed)
        }

        /// As [`Fixture::seeded`], with the environment source failing with
        /// `env_error` from the start, when given.
        fn seeded_with_env(
            body: &str,
            run_timeout: Duration,
            env_error: Option<&str>,
            seed: impl FnOnce(&RunStore, &Path),
        ) -> Self {
            Self::seeded_with_fault(body, run_timeout, env_error, None, None, seed)
        }

        /// As [`Fixture::seeded`], with every thread whose name `fault`
        /// matches failing to start.
        fn faulty(
            body: &str,
            fault: impl Fn(&str) -> bool + Send + Sync + 'static,
            seed: impl FnOnce(&RunStore, &Path),
        ) -> Self {
            Self::seeded_with_fault(
                body,
                DEFAULT_RUN_TIMEOUT,
                None,
                Some(Arc::new(fault)),
                None,
                seed,
            )
        }

        /// As [`Fixture::new`], saving through `finisher` instead.
        fn with_finisher(
            body: &str,
            finisher: impl FnOnce(&Path) -> Box<dyn RunFinisher> + 'static,
        ) -> Self {
            Self::seeded_with_fault(
                body,
                DEFAULT_RUN_TIMEOUT,
                None,
                None,
                Some(Box::new(finisher)),
                |_, _| {},
            )
        }

        fn seeded_with_fault(
            body: &str,
            run_timeout: Duration,
            env_error: Option<&str>,
            spawn_fault: Option<SpawnFault>,
            finisher: Option<FinisherFactory>,
            seed: impl FnOnce(&RunStore, &Path),
        ) -> Self {
            let dir = tempfile::tempdir().unwrap();
            seed(
                &RunStore::open(dir.path().join("runs")).unwrap(),
                dir.path(),
            );
            let program = dir.path().join("fake-harness");
            std::fs::write(&program, format!("#!/bin/sh\n{body}")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let registry = HarnessRegistry::from_lookup(|name| {
                (name == FAKE_HARNESS_ENV).then(|| program.clone().into_os_string())
            });
            Self::build(dir, registry, run_timeout, env_error, spawn_fault, finisher)
        }

        fn without_runner() -> Self {
            Self::build(
                tempfile::tempdir().unwrap(),
                HarnessRegistry::default(),
                DEFAULT_RUN_TIMEOUT,
                None,
                None,
                None,
            )
        }

        fn build(
            dir: tempfile::TempDir,
            registry: HarnessRegistry,
            run_timeout: Duration,
            env_error: Option<&str>,
            spawn_fault: Option<SpawnFault>,
            finisher: Option<FinisherFactory>,
        ) -> Self {
            let store = RunStore::open(dir.path().join("runs")).unwrap();
            let saves = Arc::new(AtomicUsize::new(0));
            let seen = SeenRun::default();
            let finisher: Box<dyn RunFinisher> = match finisher {
                Some(build) => build(dir.path()),
                None => Box::new(TestFinisher {
                    saves: saves.clone(),
                    seen: seen.clone(),
                }),
            };
            let env: SharedEnv = Arc::new(Mutex::new(match env_error {
                Some(error) => Err(error.to_string()),
                None => Ok(default_env(dir.path())),
            }));
            let source = env.clone();
            let options = ServiceOptions {
                run_timeout,
                forced_run_timeout: None,
                idle_exit: Duration::from_secs(600),
                service_pid: std::process::id(),
                run_env: Arc::new(move || source.lock().unwrap().clone()),
                run_cwd: Arc::new(|cwd: &Path| {
                    if cwd.join(FOLDER_CHANGED).exists() {
                        Err(FOLDER_CHANGED_ERROR.to_string())
                    } else {
                        Ok(())
                    }
                }),
                opener: crate::open::default_opener(),
            };
            let supervisor = match spawn_fault {
                Some(fault) => {
                    Supervisor::with_spawn_fault(store, registry, finisher, options, fault)
                }
                None => Supervisor::new(store, registry, finisher, options),
            };
            Self {
                dir,
                supervisor,
                saves,
                seen,
                env,
            }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.dir.path().join(name)
        }

        fn set_env(&self, env: Result<Vec<(OsString, OsString)>, String>) {
            *self.env.lock().unwrap() = env;
        }

        fn request(&self, key: &str) -> StartRun {
            StartRun {
                url: format!("https://example.com/{key}"),
                title: format!("Page {key}"),
                content: "Body text".to_string(),
                selection: Some("Picked words".to_string()),
                harness: None,
                record_key: RecordKey {
                    kind: RecordKeyKind::Url,
                    value: format!("https://example.com/{key}"),
                },
                requested_at: "2026-10-26T10:00:00Z".to_string(),
                cwd: self.dir.path().to_path_buf(),
                timeout: None,
                vocabulary: default_summary_vocabulary(),
            }
        }

        fn start(&self, key: &str) -> Uuid {
            let (view, existing) = self.supervisor.start_run(self.request(key)).unwrap();
            assert!(!existing);
            Uuid::parse_str(&view.run_id).unwrap()
        }

        fn release(&self) {
            std::fs::write(self.path("release"), "").unwrap();
        }

        fn view(&self, id: Uuid) -> brauser_protocol::RunView {
            self.supervisor.get_run(id, 0).unwrap().0
        }

        fn stream(&self, id: Uuid) -> String {
            self.supervisor.get_run(id, 0).unwrap().1.text
        }

        fn wait_for(
            &self,
            id: Uuid,
            done: impl Fn(&brauser_protocol::RunView) -> bool,
        ) -> brauser_protocol::RunView {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let view = self.view(id);
                if done(&view) {
                    return view;
                }
                assert!(
                    Instant::now() < deadline,
                    "run never reached the state: {view:?}"
                );
                thread::sleep(Duration::from_millis(20));
            }
        }

        fn wait_finished(&self, id: Uuid) -> brauser_protocol::RunView {
            self.wait_for(id, |view| {
                matches!(
                    view.state,
                    RunState::Succeeded | RunState::Failed | RunState::Canceled
                )
            })
        }
    }

    /// Where a [`ScriptedRunner`] session calls its hook: before answering
    /// turn `n`, or in `close`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum At {
        Turn(usize),
        Close,
    }

    type ScriptHook = Arc<dyn Fn(At) + Send + Sync>;

    /// An in-process harness that starts no process: turn `n` answers
    /// `answers[n - 1]`, after calling `hook`.
    struct ScriptedRunner {
        answers: Vec<String>,
        turns: Arc<AtomicUsize>,
        hook: ScriptHook,
    }

    struct ScriptedSession {
        answers: Vec<String>,
        turns: Arc<AtomicUsize>,
        hook: ScriptHook,
    }

    impl HarnessRunner for ScriptedRunner {
        fn harness(&self) -> HarnessId {
            HarnessId::Codex
        }

        fn start(
            &self,
            _spec: &RunSpec,
            _slot: &ProcessSlot,
        ) -> anyhow::Result<Box<dyn HarnessSession>> {
            Ok(Box::new(ScriptedSession {
                answers: self.answers.clone(),
                turns: self.turns.clone(),
                hook: self.hook.clone(),
            }))
        }
    }

    impl HarnessSession for ScriptedSession {
        fn turn(&mut self, _prompt: &str, _sink: &mut dyn FnMut(HarnessEvent)) -> TurnOutcome {
            let n = self.turns.fetch_add(1, Ordering::SeqCst) + 1;
            (self.hook)(At::Turn(n));
            match self.answers.get(n - 1) {
                Some(answer) => TurnOutcome::Answered {
                    final_text: answer.clone(),
                },
                None => TurnOutcome::Failed {
                    reason: "no more turns".to_string(),
                },
            }
        }

        fn close(self: Box<Self>) {
            (self.hook)(At::Close);
        }
    }

    /// A supervisor whose only harness is a [`ScriptedRunner`]. `hook` gets
    /// the supervisor, so it can stop the run from inside the session.
    fn scripted(
        answers: &[&str],
        hook: impl Fn(&Supervisor, At) + Send + Sync + 'static,
    ) -> (Fixture, Arc<AtomicUsize>) {
        let turns = Arc::new(AtomicUsize::new(0));
        let supervisor = Arc::new(std::sync::OnceLock::<Supervisor>::new());
        let runner = ScriptedRunner {
            answers: answers.iter().map(|answer| answer.to_string()).collect(),
            turns: turns.clone(),
            hook: {
                let supervisor = supervisor.clone();
                Arc::new(move |at| hook(supervisor.get().expect("the fixture is built"), at))
            },
        };
        let fixture = Fixture::build(
            tempfile::tempdir().unwrap(),
            HarnessRegistry::of(vec![Box::new(runner)]),
            DEFAULT_RUN_TIMEOUT,
            None,
            None,
            None,
        );
        let _ = supervisor.set(fixture.supervisor.clone());
        (fixture, turns)
    }

    /// Stop every run as `reason`, as cancel or the watchdog would, without
    /// waiting for it to end.
    fn stop_all(supervisor: &Supervisor, reason: Stop) {
        let controls: Vec<_> = lock(&supervisor.inner.runs).values().cloned().collect();
        for control in controls {
            control.request_stop(reason);
        }
    }

    /// A cancel that lands after the first answer is refused but before the
    /// repair turn starts ends the run canceled, with no repair turn.
    #[test]
    fn a_cancel_before_the_repair_turn_starts_no_repair_turn() {
        let (fixture, turns) = scripted(&["not json", TEST_ANSWER], |supervisor, at| {
            if at == At::Turn(1) {
                stop_all(supervisor, Stop::Cancel);
            }
        });
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Canceled, "{view:?}");
        assert_eq!(turns.load(Ordering::SeqCst), 1, "a repair turn ran");
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 0);
    }

    /// Once the answer is accepted, a cancel or timeout that lands while the
    /// session closes does not overturn it.
    #[test]
    fn a_stop_after_the_answer_is_accepted_does_not_overturn_it() {
        for reason in [Stop::Cancel, Stop::Timeout] {
            let (fixture, turns) = scripted(&[TEST_ANSWER], move |supervisor, at| {
                if at == At::Close {
                    stop_all(supervisor, reason);
                }
            });
            let id = fixture.start("a");
            let view = fixture.wait_finished(id);
            assert_eq!(view.state, RunState::Succeeded, "{reason:?}: {view:?}");
            assert_eq!(turns.load(Ordering::SeqCst), 1);
            assert_eq!(fixture.saves.load(Ordering::SeqCst), 1);
        }
    }

    // The fake harness prints `$VALID_ANSWER`, a valid §6.4 answer.
    const ANSWER: &str = "echo 'session: sess-1'\necho working >&2\necho \"$VALID_ANSWER\"\n";
    const BLOCK_THEN_ANSWER: &str =
        "while [ ! -e \"$RELEASE\" ]; do sleep 0.05; done\necho \"$VALID_ANSWER\"\n";
    /// A fake harness that saves each turn's prompt as `prompt-<n>` and
    /// answers turn 1 with `first` and turn 2 with `second` (shell code).
    fn two_turns(first: &str, second: &str) -> String {
        format!(
            "n=$(( $(/bin/cat \"$COUNT\" 2>/dev/null || echo 0) + 1 ))\n\
             echo $n > \"$COUNT\"\n\
             /bin/cat > \"$COUNT.prompt-$n\"\n\
             if [ $n = 1 ]; then\n{first}\nelse\n{second}\nfi\n"
        )
    }

    fn run_dir(fixture: &Fixture, id: Uuid) -> PathBuf {
        fixture.path("runs").join(id.to_string())
    }

    fn read(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap()
    }

    #[test]
    fn run_succeeds_and_writes_every_record_file() {
        let fixture = Fixture::new(ANSWER);
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Succeeded, "{view:?}");
        assert_eq!(view.error_kind, None);
        assert_eq!(view.written_record_key.as_ref(), Some(&view.record_key));
        assert_eq!(view.summary_path.as_deref(), Some("summaries/a.summary.md"));
        assert_eq!(view.session_id.as_deref(), Some("sess-1"));
        assert_eq!(view.harness, HarnessId::Codex);
        assert!(view.started_at.is_some() && view.finished_at.is_some());
        assert!(!view.can_retry_save);
        assert_eq!(fixture.stream(id), format!("{TEST_ANSWER}\n"));
        assert_eq!(view.stream_bytes as usize, TEST_ANSWER.len() + 1);
        let answer = view
            .answer
            .as_ref()
            .expect("a succeeded run shows its answer");
        assert_eq!(answer.document_type, "report");
        assert_eq!(answer.importance, brauser_protocol::ImportanceLevel::High);

        let dir = run_dir(&fixture, id);
        let page = read(&dir.join("page.md"));
        assert!(
            page.starts_with("# Page a\n\nSource: https://example.com/a\n\nBody text"),
            "{page}"
        );
        assert!(
            page.contains("## Highlighted by the user\n\nPicked words"),
            "{page}"
        );
        let result: serde_json::Value =
            serde_json::from_str(&read(&dir.join("result.json"))).unwrap();
        assert_eq!(
            result,
            serde_json::from_str::<serde_json::Value>(TEST_ANSWER).unwrap()
        );
        assert_eq!(read(&dir.join("run.log")), "working\n");
        let status: serde_json::Value =
            serde_json::from_str(&read(&dir.join("status.json"))).unwrap();
        assert_eq!(status["state"], "succeeded");
        assert_eq!(status["service_pid"], std::process::id());
        assert!(status["process_group"].is_i64());
        assert!(status["heartbeat_at"].is_string());
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 1);
        assert!(fixture.supervisor.is_idle());
    }

    #[test]
    fn third_run_waits_until_a_slot_frees() {
        let fixture = Fixture::new(BLOCK_THEN_ANSWER);
        let a = fixture.start("a");
        let b = fixture.start("b");
        let c = fixture.start("c");
        fixture.wait_for(a, |view| view.state == RunState::Running);
        fixture.wait_for(b, |view| view.state == RunState::Running);
        thread::sleep(Duration::from_millis(200));
        let waiting = fixture.view(c);
        assert_eq!(waiting.state, RunState::Queued);
        assert_eq!(waiting.started_at, None);
        assert!(!fixture.supervisor.is_idle());

        let listed: Vec<String> = fixture
            .supervisor
            .list_runs()
            .into_iter()
            .map(|view| view.run_id)
            .collect();
        assert_eq!(listed.len(), 3);
        assert!(listed.contains(&c.to_string()));

        fixture.release();
        for id in [a, b, c] {
            assert_eq!(fixture.wait_finished(id).state, RunState::Succeeded);
        }
        assert!(fixture.supervisor.is_idle());
    }

    #[test]
    fn same_record_key_returns_existing_run() {
        let fixture = Fixture::new(BLOCK_THEN_ANSWER);
        let first = fixture.start("a");
        let (view, existing) = fixture.supervisor.start_run(fixture.request("a")).unwrap();
        assert!(existing);
        assert_eq!(view.run_id, first.to_string());

        fixture.release();
        assert_eq!(fixture.wait_finished(first).state, RunState::Succeeded);
        // Once the first run is over, the same key starts a new run.
        let (again, existing) = fixture.supervisor.start_run(fixture.request("a")).unwrap();
        assert!(!existing);
        assert_ne!(again.run_id, first.to_string());
    }

    #[test]
    fn cancel_kills_process_group_and_marks_canceled() {
        let fixture = Fixture::new("sleep 30 &\necho started\nwait\n");
        let running = fixture.start("a");
        let other = fixture.start("b");
        let queued = fixture.start("c");
        fixture.wait_for(running, |view| view.stream_bytes > 0);
        fixture.wait_for(other, |view| view.stream_bytes > 0);
        let group = fixture
            .supervisor
            .inner
            .store
            .status(running)
            .unwrap()
            .process_group
            .expect("running record has a process group");

        // A queued run is dequeued without ever starting.
        let view = fixture.supervisor.cancel_run(queued).unwrap();
        assert_eq!(view.state, RunState::Canceled);
        assert_eq!(view.error_kind, Some(RunErrorKind::Canceled));
        assert_eq!(view.started_at, None);
        assert!(!run_dir(&fixture, queued).join("stream.txt").exists());

        let view = fixture.supervisor.cancel_run(running).unwrap();
        assert_eq!(view.state, RunState::Canceled, "{view:?}");
        assert_eq!(view.error_kind, Some(RunErrorKind::Canceled));
        let pid = rustix::process::Pid::from_raw(group).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while rustix::process::test_kill_process_group(pid).is_ok() {
            assert!(
                Instant::now() < deadline,
                "process group {group} survived cancel"
            );
            thread::sleep(Duration::from_millis(20));
        }
        // The stream so far is kept.
        assert_eq!(fixture.stream(running), "started\n");

        // Canceling a finished run changes nothing.
        let again = fixture.supervisor.cancel_run(running).unwrap();
        assert_eq!(again, view);

        fixture.supervisor.cancel_run(other).unwrap();
        assert!(fixture.supervisor.is_idle());
        assert!(matches!(
            fixture.supervisor.cancel_run(Uuid::new_v4()),
            Err((ErrorCode::InvalidRequest, _))
        ));
    }

    #[test]
    fn a_queued_cancel_that_cannot_be_recorded_keeps_the_run_queued() {
        let fixture = Fixture::new(BLOCK_THEN_ANSWER);
        let a = fixture.start("a");
        let b = fixture.start("b");
        let queued = fixture.start("c");
        fixture.wait_for(a, |view| view.state == RunState::Running);
        fixture.wait_for(b, |view| view.state == RunState::Running);

        // A read-only run directory makes the canceled status unwritable.
        let dir = run_dir(&fixture, queued);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o500)).unwrap();
        let canceled = fixture.supervisor.cancel_run(queued);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            matches!(canceled, Err((ErrorCode::Internal, _))),
            "{canceled:?}"
        );
        assert_eq!(fixture.view(queued).state, RunState::Queued);

        // The run is still scheduled, so it runs once a slot frees.
        fixture.release();
        for id in [a, b, queued] {
            assert_eq!(fixture.wait_finished(id).state, RunState::Succeeded);
        }
        assert!(fixture.supervisor.is_idle());
    }

    #[test]
    fn a_run_without_a_watchdog_fails_before_its_harness() {
        let fixture = Fixture::faulty(
            "echo ran >> \"$COUNT\"\necho '{}'\n",
            |name| name.ends_with("-watchdog"),
            |_, _| {},
        );
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Failed, "{view:?}");
        assert_eq!(view.error_kind, Some(RunErrorKind::HarnessFailed));
        assert_eq!(view.error.as_deref(), Some("The run could not be started."));
        assert!(!fixture.path("count").exists(), "the harness ran");
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 0);
        assert!(fixture.supervisor.is_idle());
    }

    #[test]
    fn timeout_fails_with_timed_out() {
        let fixture = Fixture::with_timeout("exec sleep 30\n", Duration::from_millis(300));
        let started = Instant::now();
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert!(started.elapsed() < Duration::from_secs(8));
        assert_eq!(view.state, RunState::Failed);
        assert_eq!(view.error_kind, Some(RunErrorKind::TimedOut));
        assert!(
            view.error
                .as_deref()
                .unwrap_or("")
                .starts_with("Timed out after"),
            "{view:?}"
        );
    }

    #[test]
    fn silent_run_records_its_group_so_restart_recovery_kills_it() {
        // The harness never writes anything, so only the spawn can record
        // its process group.
        let fixture = Fixture::new("sleep 30 &\nwait\n");
        let id = fixture.start("a");
        let store = &fixture.supervisor.inner.store;
        let deadline = Instant::now() + Duration::from_secs(5);
        let group = loop {
            if let Some(group) = store.status(id).unwrap().process_group {
                break group;
            }
            assert!(
                Instant::now() < deadline,
                "a silent run never recorded its process group"
            );
            thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(fixture.stream(id), "");

        // A new service process recovers the record this one left behind.
        let restarted = RunStore::open(fixture.path("runs")).unwrap();
        maintenance::recover(
            &restarted,
            OffsetDateTime::now_utc(),
            std::process::id().wrapping_add(1),
            &group_alive,
            &kill_group,
        );
        let deadline = Instant::now() + Duration::from_secs(5);
        while group_alive(group) {
            assert!(
                Instant::now() < deadline,
                "recovery left process group {group} running"
            );
            thread::sleep(Duration::from_millis(20));
        }
        fixture.wait_finished(id);
        let status = restarted.status(id).unwrap();
        assert_eq!(status.state, RunState::Failed);
    }

    #[test]
    fn a_runs_own_timeout_wins_unless_the_seam_forces_one() {
        let mut options = ServiceOptions {
            run_timeout: DEFAULT_RUN_TIMEOUT,
            forced_run_timeout: None,
            idle_exit: Duration::from_secs(600),
            service_pid: std::process::id(),
            run_env: Arc::new(|| Ok(Vec::new())),
            run_cwd: Arc::new(|_: &Path| Ok(())),
            opener: crate::open::default_opener(),
        };
        let own = Duration::from_secs(5);
        assert_eq!(options.timeout_for(None), DEFAULT_RUN_TIMEOUT);
        assert_eq!(options.timeout_for(Some(own)), own);
        options.forced_run_timeout = Some(Duration::from_secs(2));
        assert_eq!(options.timeout_for(Some(own)), Duration::from_secs(2));
        assert_eq!(options.timeout_for(None), Duration::from_secs(2));
    }

    #[test]
    fn timeout_message_names_minutes() {
        assert_eq!(
            timeout_message(Duration::from_secs(600)),
            "Timed out after 10 minutes"
        );
        assert_eq!(
            timeout_message(Duration::from_secs(60)),
            "Timed out after 1 minute"
        );
        assert_eq!(
            timeout_message(Duration::from_millis(300)),
            "Timed out after 1 second"
        );
        assert_eq!(
            timeout_message(Duration::from_secs(45)),
            "Timed out after 45 seconds"
        );
    }

    #[test]
    fn invalid_answer_gets_one_repair_turn_and_succeeds() {
        let fixture = Fixture::new(&two_turns("echo 'not json'", "echo \"$VALID_ANSWER\""));
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Succeeded, "{view:?}");
        assert_eq!(read(&fixture.path("count")), "2\n");
        let repair = read(&fixture.path("count.prompt-2"));
        assert!(
            repair.starts_with("Your previous answer was not valid: the answer is not valid JSON"),
            "{repair}"
        );
        assert!(repair.contains("Reply with only the corrected JSON object"));
        assert!(read(&fixture.path("count.prompt-1")).contains("page.md"));
        assert_eq!(fixture.stream(id), format!("not json\n{TEST_ANSWER}\n"));
        assert!(run_dir(&fixture, id).join("result.json").is_file());
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn second_invalid_answer_fails_invalid_output_and_keeps_stream() {
        let fixture = Fixture::new(&two_turns(
            "echo 'not json at all'",
            "echo '{\"document_type\": \"report\"}'",
        ));
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Failed);
        assert_eq!(view.error_kind, Some(RunErrorKind::InvalidOutput));
        let error = view.error.as_deref().unwrap();
        assert!(
            error.starts_with("The agent's answer was not valid after one correction: "),
            "{error}"
        );
        assert!(error.contains("category"), "{error}");
        assert!(!view.can_retry_save);
        assert_eq!(view.answer, None);
        assert_eq!(
            read(&fixture.path("count")),
            "2\n",
            "exactly one repair turn"
        );
        assert_eq!(
            fixture.stream(id),
            "not json at all\n{\"document_type\": \"report\"}\n"
        );
        assert!(!run_dir(&fixture, id).join("result.json").exists());
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn empty_final_text_gets_a_repair_turn() {
        let fixture = Fixture::new(&two_turns("true", "echo \"$VALID_ANSWER\""));
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Succeeded, "{view:?}");
        let repair = read(&fixture.path("count.prompt-2"));
        assert!(repair.contains("the answer is empty"), "{repair}");
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn repair_turn_after_harness_exit_fails_as_harness_failed() {
        let fixture = Fixture::new(&two_turns(
            "echo 'not json'",
            "echo 'harness went away' >&2\nexit 4",
        ));
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Failed);
        assert_eq!(view.error_kind, Some(RunErrorKind::HarnessFailed));
        assert_eq!(
            view.error.as_deref(),
            Some("harness exited with status 4: harness went away")
        );
        assert_eq!(fixture.stream(id), "not json\n");
        assert!(!run_dir(&fixture, id).join("result.json").exists());
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn accept_without_cwd_fails_invalid_output() {
        let dir = tempfile::tempdir().unwrap();
        let finisher = SummaryFinisher::new(dir.path().join("config.toml"));
        let mut run = sample_status(Uuid::new_v4());
        assert_eq!(
            finisher.accept(&run, TEST_ANSWER).unwrap_err(),
            "run has no working directory"
        );
        run.cwd = Some(dir.path().display().to_string());
        let accepted = finisher.accept(&run, TEST_ANSWER).unwrap();
        assert_eq!(accepted["importance"]["level"], "high");
        assert!(finisher.accept(&run, "{}").is_err());
    }

    #[test]
    fn harness_failure_is_harness_failed() {
        let fixture = Fixture::new("echo 'bad login' >&2\nexit 2\n");
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Failed);
        assert_eq!(view.error_kind, Some(RunErrorKind::HarnessFailed));
        assert_eq!(
            view.error.as_deref(),
            Some("harness exited with status 2: bad login")
        );
    }

    #[test]
    fn save_failure_offers_retry_save_and_retry_succeeds() {
        // The service's own finisher, on a home folder in the fixture's
        // directory, with an unowned file where the summary goes.
        let fixture = Fixture::with_finisher(
            "echo ran >> \"$COUNT\"\necho \"$VALID_ANSWER\"\n",
            |dir: &Path| {
                let root = dir.join("home");
                std::fs::create_dir(&root).unwrap();
                let config = dir.join("config.toml");
                crate::config::fixtures::write(&config, &root, "", "", "");
                Box::new(SummaryFinisher::new(config))
            },
        );
        let page = fixture.request("a").url;
        let key = crate::vault::url_record_key(&page).unwrap();
        let config = crate::config::ConfigStore::load_from(fixture.path("config.toml"));
        let relative = crate::vault::Vault::open(config.snapshot().storage.as_ref().unwrap())
            .unwrap()
            .summary_relative_path(&key, &page)
            .unwrap();
        let summary = fixture.path("home").join(&relative);
        std::fs::create_dir_all(summary.parent().unwrap()).unwrap();
        std::fs::write(&summary, "my own file\n").unwrap();
        let shown = relative.to_string_lossy().replace('\\', "/");

        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Failed);
        assert_eq!(view.error_kind, Some(RunErrorKind::SaveFailed));
        assert_eq!(
            view.error,
            Some(format!(
                "A file Brauser does not own is at {shown}. Move it, then Retry save."
            ))
        );
        assert!(view.can_retry_save);
        assert_eq!(read(&summary), "my own file\n");
        let failed = fixture.supervisor.store().status(id).unwrap();
        let answered_at = failed.answered_at.clone().expect("stamped before the save");

        std::fs::remove_file(&summary).unwrap();
        let retried = fixture.supervisor.retry_save(id).unwrap();
        assert_eq!(retried.state, RunState::Succeeded, "{retried:?}");
        assert_eq!(retried.error, None);
        assert_eq!(retried.error_kind, None);
        assert_eq!(retried.summary_path.as_deref(), Some(shown.as_str()));
        assert!(!retried.can_retry_save);
        let written = read(&summary);
        assert!(written.starts_with("---\nbrauser:\n  kind: summary\n"));
        // The retry writes the answer's time, not its own.
        let stamped = crate::service::finisher::summarized_at(
            OffsetDateTime::parse(&answered_at, &Rfc3339).unwrap(),
            &failed.requested_at,
        );
        assert!(
            written.contains(&format!("summarized_at: \"{stamped}\"")),
            "{written}"
        );
        let stored = fixture.supervisor.store().status(id).unwrap();
        assert_eq!(stored.answered_at.as_deref(), Some(answered_at.as_str()));
        assert_eq!(
            read(&fixture.path("count")),
            "ran\n",
            "the harness ran once"
        );
        // The failed first save logged nothing; the retry logged once.
        assert_eq!(summary_lines(&fixture.path("home").join("logs")), 1);
    }

    /// The `kind=summary` lines in every daily log under `dir`.
    fn summary_lines(dir: &Path) -> usize {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return 0;
        };
        entries
            .map(|entry| entry.unwrap().path())
            .map(|path| {
                if path.is_dir() {
                    summary_lines(&path)
                } else {
                    read(&path).matches(" kind=summary data=").count()
                }
            })
            .sum()
    }

    #[test]
    fn the_first_save_gets_the_record_stamped_with_its_answer_time() {
        let fixture = Fixture::new(ANSWER);
        let id = fixture.start("a");
        assert_eq!(fixture.wait_finished(id).state, RunState::Succeeded);
        let seen = fixture.seen.lock().unwrap().clone().unwrap();
        let stored = fixture.supervisor.store().status(id).unwrap();
        assert!(seen.answered_at.is_some(), "{seen:?}");
        assert_eq!(seen.answered_at, stored.answered_at);
        // The record as it is now, with the session the harness reported.
        assert_eq!(seen.session_id.as_deref(), Some("sess-1"));
    }

    #[test]
    fn a_retry_of_an_unstamped_record_stamps_it_before_saving() {
        let mut seeded = None;
        let fixture = Fixture::seeded(ANSWER, DEFAULT_RUN_TIMEOUT, |store, cwd| {
            let id = seed_record(
                store,
                cwd,
                RunState::Failed,
                "2026-10-26T09:00:01.000000000Z",
            );
            let mut status = store.status(id).unwrap();
            status.error_kind = Some(RunErrorKind::SaveFailed);
            status.error = Some("The summary file could not be written. Retry save.".into());
            store.write_status(&status).unwrap();
            store
                .write_result(id, &serde_json::from_str(TEST_ANSWER).unwrap())
                .unwrap();
            seeded = Some(id);
        });
        let id = seeded.unwrap();
        assert_eq!(
            fixture.supervisor.store().status(id).unwrap().answered_at,
            None
        );

        let retried = fixture.supervisor.retry_save(id).unwrap();

        assert_eq!(retried.state, RunState::Succeeded, "{retried:?}");
        let seen = fixture.seen.lock().unwrap().clone().unwrap();
        assert!(seen.answered_at.is_some());
        let stored = fixture.supervisor.store().status(id).unwrap();
        assert_eq!(stored.answered_at, seen.answered_at);
    }

    #[test]
    fn retry_save_on_succeeded_run_is_conflict() {
        let fixture = Fixture::new(ANSWER);
        let id = fixture.start("a");
        assert_eq!(fixture.wait_finished(id).state, RunState::Succeeded);
        assert!(matches!(
            fixture.supervisor.retry_save(id),
            Err((ErrorCode::Conflict, _))
        ));
        assert!(matches!(
            fixture.supervisor.retry_save(Uuid::new_v4()),
            Err((ErrorCode::InvalidRequest, _))
        ));
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn retry_save_on_unknown_runs_allocates_no_lock() {
        let fixture = Fixture::new(ANSWER);
        for _ in 0..100 {
            assert!(matches!(
                fixture.supervisor.retry_save(Uuid::new_v4()),
                Err((ErrorCode::InvalidRequest, _))
            ));
        }
        assert!(lock(&fixture.supervisor.inner.record_locks).is_empty());
    }

    #[test]
    fn pruning_a_run_drops_its_lock() {
        let mut old = Vec::new();
        let fixture = Fixture::seeded(ANSWER, DEFAULT_RUN_TIMEOUT, |store, cwd| {
            for _ in 0..maintenance::KEEP_NEWEST {
                old.push(seed_record(
                    store,
                    cwd,
                    RunState::Succeeded,
                    "2020-01-01T00:00:00.000000000Z",
                ));
            }
        });
        // Equal `created_at`, so the smallest id is the first pruned.
        let oldest = *old.iter().min_by_key(|id| id.to_string()).unwrap();
        assert!(matches!(
            fixture.supervisor.retry_save(oldest),
            Err((ErrorCode::Conflict, _))
        ));
        let locks = || lock(&fixture.supervisor.inner.record_locks).contains_key(&oldest);
        assert!(locks());

        let id = fixture.start("a");
        fixture.wait_finished(id);
        let deadline = Instant::now() + Duration::from_secs(5);
        while run_dir(&fixture, oldest).exists() || locks() {
            assert!(
                Instant::now() < deadline,
                "the pruned run kept its lock or record"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn pruning_skips_a_run_whose_record_is_in_use() {
        let mut old = Vec::new();
        let fixture = Fixture::seeded(ANSWER, DEFAULT_RUN_TIMEOUT, |store, cwd| {
            for _ in 0..maintenance::KEEP_NEWEST {
                old.push(seed_record(
                    store,
                    cwd,
                    RunState::Succeeded,
                    "2020-01-01T00:00:00.000000000Z",
                ));
            }
        });
        // Equal `created_at`, so the smallest id is the first pruned.
        let oldest = *old.iter().min_by_key(|id| id.to_string()).unwrap();
        let inner = &fixture.supervisor.inner;
        let record_lock = inner.record_lock(oldest).expect("the run exists");
        let held = lock(&record_lock);
        // One more finished run puts the oldest past retention.
        seed_record(
            &inner.store,
            fixture.dir.path(),
            RunState::Succeeded,
            "2021-01-01T00:00:00.000000000Z",
        );
        // A run in retry_save holds its record lock; retention leaves it.
        inner.prune();
        assert!(
            run_dir(&fixture, oldest).exists(),
            "a held record was pruned"
        );
        drop(held);
        inner.prune();
        assert!(!run_dir(&fixture, oldest).exists());
        assert!(!lock(&inner.record_locks).contains_key(&oldest));
    }

    #[test]
    fn run_env_is_read_when_the_run_starts() {
        let fixture = Fixture::new(
            "while [ ! -e \"$RELEASE\" ]; do sleep 0.05; done\n\
             echo \"$VALID_ANSWER\" | /usr/bin/sed \"s/A page./$FOO/\"\n",
        );
        let with_foo = |value: &str| {
            let mut env = default_env(fixture.dir.path());
            env.push(("FOO".into(), value.into()));
            Ok(env)
        };
        fixture.set_env(with_foo("at-queue-time"));
        // Two blocked runs hold both slots, so the third waits in the queue.
        let first = fixture.start("a");
        let second = fixture.start("b");
        let queued = fixture.start("c");
        fixture.wait_for(first, |view| view.state == RunState::Running);
        fixture.wait_for(second, |view| view.state == RunState::Running);
        assert_eq!(fixture.view(queued).state, RunState::Queued);
        fixture.set_env(with_foo("at-start-time"));
        fixture.release();
        for id in [first, second] {
            assert_eq!(fixture.wait_finished(id).state, RunState::Succeeded);
            assert!(fixture.stream(id).contains("at-queue-time"));
        }
        assert_eq!(fixture.wait_finished(queued).state, RunState::Succeeded);
        let stream = fixture.stream(queued);
        assert!(stream.contains("at-start-time"), "{stream}");
    }

    #[test]
    fn missing_runner_is_not_configured() {
        let fixture = Fixture::without_runner();
        let request = StartRun {
            harness: Some(HarnessId::Claude),
            ..fixture.request("a")
        };
        assert!(matches!(
            fixture.supervisor.start_run(request),
            Err((ErrorCode::NotConfigured, _))
        ));
        assert!(fixture.supervisor.list_runs().is_empty());
        assert!(fixture.supervisor.is_idle());
    }

    #[test]
    fn over_long_or_control_character_session_ids_are_dropped_whole() {
        // A later unfit id neither replaces nor truncates the good one.
        let fixture = Fixture::new(&format!(
            "echo 'session: sess-1'\necho 'session: {}'\nprintf 'session: sess\\0012\\n'\necho \"$VALID_ANSWER\"\n",
            "s".repeat(MAX_SESSION_ID_BYTES + 1)
        ));
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Succeeded, "{view:?}");
        assert_eq!(view.session_id.as_deref(), Some("sess-1"));

        // With only an unfit id, none is recorded.
        let fixture = Fixture::new(&format!(
            "echo 'session: {}'\necho \"$VALID_ANSWER\"\n",
            "s".repeat(MAX_SESSION_ID_BYTES + 1)
        ));
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Succeeded, "{view:?}");
        assert_eq!(view.session_id, None);
    }

    #[test]
    fn get_run_pages_and_refuses_bad_offsets() {
        let fixture = Fixture::new("printf 'h\\303\\251llo\\n'\necho '{}'\n");
        let id = fixture.start("a");
        fixture.wait_finished(id);
        // The answer is invalid, so the repair turn repeats the output.
        let (_, page) = fixture.supervisor.get_run(id, 3).unwrap();
        assert_eq!(page.text, "llo\n{}\nh\u{e9}llo\n{}\n");
        assert!(matches!(
            fixture.supervisor.get_run(id, 2),
            Err((ErrorCode::InvalidRequest, _))
        ));
        assert!(matches!(
            fixture.supervisor.get_run(id, 10_000),
            Err((ErrorCode::InvalidRequest, _))
        ));
        assert!(matches!(
            fixture.supervisor.get_run(Uuid::new_v4(), 0),
            Err((ErrorCode::InvalidRequest, _))
        ));
    }

    /// A running record with no working directory.
    fn sample_status(id: Uuid) -> RunStatusFile {
        RunStatusFile {
            version: 1,
            run_id: id.to_string(),
            state: RunState::Running,
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
        }
    }

    #[test]
    fn summary_finisher_refuses_without_a_configured_vault() {
        let dir = tempfile::tempdir().unwrap();
        let finisher = SummaryFinisher::new(dir.path().join("config.toml"));
        let run = sample_status(Uuid::new_v4());
        let answer: serde_json::Value = serde_json::from_str(TEST_ANSWER).unwrap();
        let error = finisher.save(&run, &answer).unwrap_err();
        assert_eq!(error, "Choose a notes folder in settings, then Retry save.");
        assert!(!dir.path().join("summaries").exists());
    }

    #[test]
    fn list_runs_keeps_active_and_newest_finished_runs_newest_first() {
        let fixture = Fixture::new(ANSWER);
        let mut ids = Vec::new();
        for index in 0..(LIST_LIMIT + 3) {
            let id = fixture.start(&format!("k{index}"));
            fixture.wait_finished(id);
            ids.push(id);
        }
        let listed: Vec<String> = fixture
            .supervisor
            .list_runs()
            .into_iter()
            .map(|view| view.run_id)
            .collect();
        let expected: Vec<String> = ids
            .iter()
            .rev()
            .take(LIST_LIMIT)
            .map(|id| id.to_string())
            .collect();
        assert_eq!(listed, expected);
    }

    fn seed_record(store: &RunStore, cwd: &Path, state: RunState, created_at: &str) -> Uuid {
        let id = Uuid::new_v4();
        let finished = !matches!(state, RunState::Queued | RunState::Running);
        let status = RunStatusFile {
            version: 1,
            run_id: id.to_string(),
            state,
            harness: HarnessId::Codex,
            service_pid: Some(1_000_000),
            process_group: None,
            heartbeat_at: None,
            thread_id: None,
            session_id: None,
            cwd: Some(cwd.display().to_string()),
            start_key: RecordKey {
                kind: RecordKeyKind::Url,
                value: format!("https://example.com/{id}"),
            },
            written_key: None,
            url: format!("https://example.com/{id}"),
            title: "Seeded".into(),
            requested_at: created_at.into(),
            created_at: created_at.into(),
            started_at: None,
            finished_at: finished.then(|| created_at.to_string()),
            error: None,
            error_kind: None,
            summary_path: None,
            stream_truncated: false,
            run_timeout_secs: None,
            vocabulary: None,
            answered_at: None,
        };
        store.create(&status, "# Seeded\n").unwrap();
        id
    }

    #[test]
    fn new_fails_interrupted_runs_and_resumes_queued_ones() {
        let mut interrupted = None;
        let mut queued = None;
        let fixture = Fixture::seeded(ANSWER, DEFAULT_RUN_TIMEOUT, |store, cwd| {
            interrupted = Some(seed_record(
                store,
                cwd,
                RunState::Running,
                "2026-10-26T09:00:00.000000000Z",
            ));
            queued = Some(seed_record(
                store,
                cwd,
                RunState::Queued,
                "2026-10-26T09:00:01.000000000Z",
            ));
        });
        let view = fixture.view(interrupted.unwrap());
        assert_eq!(view.state, RunState::Failed);
        assert_eq!(view.error_kind, Some(RunErrorKind::ServiceRestarted));
        assert_eq!(view.error.as_deref(), Some("service restarted"));

        let view = fixture.wait_finished(queued.unwrap());
        assert_eq!(view.state, RunState::Succeeded, "{view:?}");
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 1);
        assert!(fixture.supervisor.is_idle());
    }

    fn custom_vocabulary() -> SummaryVocabulary {
        SummaryVocabulary {
            document_types: vec!["memo".to_string()],
            categories: vec!["legal review".to_string()],
        }
    }

    /// A queued run records its own limit and vocabulary, so a restart
    /// does not trade them for the service's limit and the default lists.
    #[test]
    fn a_queued_run_records_its_limit_and_vocabulary() {
        let fixture = Fixture::new(ANSWER);
        let mut request = fixture.request("a");
        request.timeout = Some(Duration::from_secs(900));
        request.vocabulary = custom_vocabulary();
        let (view, _) = fixture.supervisor.start_run(request).unwrap();
        let id = Uuid::parse_str(&view.run_id).unwrap();
        let status = fixture.supervisor.store().status(id).unwrap();
        assert_eq!(status.run_timeout_secs, Some(900));
        assert_eq!(status.vocabulary, Some(custom_vocabulary()));
        fixture.wait_finished(id);
    }

    #[test]
    fn a_resumed_queued_run_keeps_its_recorded_limit_and_vocabulary() {
        let mut queued = None;
        let fixture = Fixture::seeded(
            "/bin/cat > \"$COUNT.prompt\"\nexec /bin/sleep 30\n",
            DEFAULT_RUN_TIMEOUT,
            |store, cwd| {
                let id = seed_record(
                    store,
                    cwd,
                    RunState::Queued,
                    "2026-10-26T09:00:01.000000000Z",
                );
                let mut status = store.status(id).unwrap();
                status.run_timeout_secs = Some(1);
                status.vocabulary = Some(custom_vocabulary());
                store.write_status(&status).unwrap();
                queued = Some(id);
            },
        );
        let view = fixture.wait_finished(queued.unwrap());
        assert_eq!(view.error_kind, Some(RunErrorKind::TimedOut), "{view:?}");
        assert_eq!(view.error.as_deref(), Some("Timed out after 1 second"));
        let prompt = read(&fixture.path("count.prompt"));
        assert!(prompt.contains("legal review"), "{prompt}");
    }

    #[test]
    fn finished_run_prunes_old_records() {
        let mut old = Vec::new();
        let fixture = Fixture::seeded(ANSWER, DEFAULT_RUN_TIMEOUT, |store, cwd| {
            for _ in 0..maintenance::KEEP_NEWEST {
                old.push(seed_record(
                    store,
                    cwd,
                    RunState::Succeeded,
                    "2020-01-01T00:00:00.000000000Z",
                ));
            }
        });
        // Exactly 200 old runs: start-up pruning keeps all of them.
        assert!(old.iter().all(|id| run_dir(&fixture, *id).exists()));
        let id = fixture.start("a");
        fixture.wait_finished(id);
        // The new run pushes one old run outside the newest 200.
        let deadline = Instant::now() + Duration::from_secs(5);
        while old.iter().all(|id| run_dir(&fixture, *id).exists()) {
            assert!(Instant::now() < deadline, "no old run was pruned");
            thread::sleep(Duration::from_millis(20));
        }
        let kept = old
            .iter()
            .filter(|id| run_dir(&fixture, **id).exists())
            .count();
        assert_eq!(kept, maintenance::KEEP_NEWEST - 1);
        assert!(run_dir(&fixture, id).exists());
    }

    #[test]
    fn a_queued_run_whose_env_cannot_be_read_fails_before_its_harness() {
        let fixture = Fixture::new(ANSWER);
        fixture.set_env(Err("agent.env must have mode 0600".to_string()));
        let id = fixture.start("a");
        let view = fixture.wait_finished(id);
        assert_eq!(view.state, RunState::Failed);
        assert_eq!(view.error_kind, Some(RunErrorKind::HarnessFailed));
        assert_eq!(view.error.as_deref(), Some("agent.env must have mode 0600"));
        assert_eq!(view.started_at, None);
        assert_eq!(view.stream_bytes, 0);
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn worker_spawn_failures_do_not_strand_the_rest_of_the_queue() {
        // More queued runs than slots, and no worker thread can be created:
        // each failure frees a slot that the next queued run must take.
        let mut queued = Vec::new();
        let fixture = Fixture::faulty(
            ANSWER,
            |name| !name.ends_with("-watchdog"),
            |store, cwd| {
                for second in 0..4 {
                    queued.push(seed_record(
                        store,
                        cwd,
                        RunState::Queued,
                        &format!("2026-10-26T09:00:0{second}.000000000Z"),
                    ));
                }
            },
        );
        for id in queued {
            let view = fixture.wait_finished(id);
            assert_eq!(view.state, RunState::Failed, "{view:?}");
            assert_eq!(view.error.as_deref(), Some("The run could not be started."));
        }
        assert!(fixture.supervisor.is_idle());
    }

    /// The folder check runs when a queued run leaves the queue, not when it
    /// was queued: a folder replaced while the run waited fails it before
    /// its harness starts.
    #[test]
    fn a_queued_run_whose_folder_changed_fails_before_its_harness() {
        let fixture = Fixture::new(&format!(
            "/bin/echo started >> \"$COUNT\"\n{BLOCK_THEN_ANSWER}"
        ));
        let first = fixture.start("a");
        let second = fixture.start("b");
        let queued = fixture.start("c");
        fixture.wait_for(first, |view| view.state == RunState::Running);
        fixture.wait_for(second, |view| view.state == RunState::Running);
        assert_eq!(fixture.view(queued).state, RunState::Queued);
        // Wait until both running harnesses have logged their start.
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::fs::read_to_string(fixture.path("count"))
            .unwrap_or_default()
            .lines()
            .count()
            < 2
        {
            assert!(
                Instant::now() < deadline,
                "the running harnesses never started"
            );
            thread::sleep(Duration::from_millis(20));
        }
        std::fs::write(fixture.path(FOLDER_CHANGED), "").unwrap();
        fixture.release();
        for id in [first, second] {
            assert_eq!(fixture.wait_finished(id).state, RunState::Succeeded);
        }
        let view = fixture.wait_finished(queued);
        assert_eq!(view.state, RunState::Failed, "{view:?}");
        assert_eq!(view.error_kind, Some(RunErrorKind::HarnessFailed));
        assert_eq!(view.error.as_deref(), Some(FOLDER_CHANGED_ERROR));
        assert_eq!(view.started_at, None);
        assert_eq!(view.stream_bytes, 0);
        assert_eq!(read(&fixture.path("count")).lines().count(), 2);
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 2);
        assert!(fixture.supervisor.is_idle());
    }

    #[test]
    fn a_resumed_queued_run_whose_folder_changed_fails_before_its_harness() {
        let mut queued = None;
        let fixture = Fixture::seeded(
            "/bin/echo started >> \"$COUNT\"\necho \"$VALID_ANSWER\"\n",
            DEFAULT_RUN_TIMEOUT,
            |store, cwd| {
                std::fs::write(cwd.join(FOLDER_CHANGED), "").unwrap();
                queued = Some(seed_record(
                    store,
                    cwd,
                    RunState::Queued,
                    "2026-10-26T09:00:01.000000000Z",
                ));
            },
        );
        let view = fixture.wait_finished(queued.unwrap());
        assert_eq!(view.state, RunState::Failed, "{view:?}");
        assert_eq!(view.error_kind, Some(RunErrorKind::HarnessFailed));
        assert_eq!(view.error.as_deref(), Some(FOLDER_CHANGED_ERROR));
        assert_eq!(view.started_at, None);
        assert!(!fixture.path("count").exists());
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 0);
        assert!(fixture.supervisor.is_idle());
    }

    #[test]
    fn a_resumed_queued_run_fails_when_agent_env_is_unreadable() {
        let mut queued = None;
        let fixture = Fixture::seeded_with_env(
            ANSWER,
            DEFAULT_RUN_TIMEOUT,
            Some("agent.env is a symlink"),
            |store, cwd| {
                queued = Some(seed_record(
                    store,
                    cwd,
                    RunState::Queued,
                    "2026-10-26T09:00:01.000000000Z",
                ));
            },
        );
        let view = fixture.wait_finished(queued.unwrap());
        assert_eq!(view.state, RunState::Failed, "{view:?}");
        assert_eq!(view.error_kind, Some(RunErrorKind::HarnessFailed));
        assert_eq!(view.error.as_deref(), Some("agent.env is a symlink"));
        assert_eq!(fixture.saves.load(Ordering::SeqCst), 0);
        assert!(fixture.supervisor.is_idle());
    }

    #[test]
    fn service_options_debug_names_no_environment() {
        let options = ServiceOptions {
            run_timeout: DEFAULT_RUN_TIMEOUT,
            forced_run_timeout: None,
            idle_exit: Duration::from_secs(600),
            service_pid: 1,
            run_env: Arc::new(|| Ok(vec![("API_KEY".into(), "secret-value".into())])),
            run_cwd: Arc::new(|_: &Path| Ok(())),
            opener: crate::open::default_opener(),
        };
        let debug = format!("{options:?}");
        assert!(!debug.contains("secret-value"), "{debug}");
    }
}
