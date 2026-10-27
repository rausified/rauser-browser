//! The service's private Unix socket: one listener in the 0700 support
//! directory, one thread per connection, and the same length-prefixed JSON
//! framing as native messaging. The service answers `service_hello`, the
//! five run messages, harness setup (`discover_harnesses`,
//! `set_up_harness`), `open_path` and `get_agent_env`; it exits after
//! `idle_exit` with nothing queued or running and no request.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{ErrorKind, Write};
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use brauser_protocol::{
    AgentEnvListed, ConfigUpdated, ErrorCode, HarnessId, HarnessesDiscovered, PROTOCOL_VERSION,
    PathOpened, Request, Response, RunFetched, RunStarted, RunUpdated, RunsListed, ServiceReady,
    SetUpHarnessRequest, SummaryVocabulary,
};
use uuid::Uuid;

use super::agent_env::{AgentEnv, AgentEnvError};
use super::open::{list_agent_env, open_target};
use super::paths::{self, ServicePaths};
use super::record::check_start_run;
use super::setup;
use super::supervisor::{EnvSource, ServiceOptions, StartRun, Supervisor};
use crate::brand::NAMESPACE;
use crate::config::ConfigStore;
use crate::frame::{FrameError, read_frame, write_frame};
use crate::harness::HarnessEnv;
use crate::native::{error, parse_envelope};
use crate::open::Opener;
use crate::vault::selected_root_identity;

/// How often the accept loop checks for a connection and for idle exit.
const ACCEPT_POLL: Duration = Duration::from_millis(250);
pub const NOT_CONFIGURED: &str = "Choose a notes folder in Brauser settings first.";
pub const SET_UP_HARNESS: &str = "Set up a summary harness in Brauser settings first.";
pub const KNOWLEDGE_BASE_CHANGED: &str =
    "The knowledge base folder changed or is missing. Choose it again in settings.";

/// Why [`serve`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeExit {
    /// Nothing ran and nothing arrived for `idle_exit`.
    Idle,
    /// Another service process holds `service.lock`.
    AlreadyRunning,
}

/// A run's child environment: this process's environment overlaid with
/// `agent.env`. The error never carries a value from the file.
pub fn run_env(paths: &ServicePaths) -> Result<Vec<(OsString, OsString)>, AgentEnvError> {
    Ok(AgentEnv::read(&paths.agent_env())?.overlay(std::env::vars_os()))
}

/// [`run_env`] for the supervisor to call when each run starts, so a run that
/// waited in the queue, or was resumed after a restart, uses `agent.env` as
/// it is then and fails when it cannot be read.
pub fn env_source(paths: &ServicePaths) -> EnvSource {
    let paths = paths.clone();
    Arc::new(move || run_env(&paths).map_err(|error| error.to_string()))
}

/// Serve the socket until the service is idle.
///
/// Startup: make the support directory private, take `service.lock` (a held
/// lock returns [`ServeExit::AlreadyRunning`] at once), write `service.pid`,
/// replace a stale socket, build the supervisor, then bind the socket with
/// mode 0600. The supervisor is built only under the lock, because its
/// recovery fails and kills interrupted runs and must never run beside a
/// live service. The socket and pid file are removed on the way out.
pub fn serve(
    paths: &ServicePaths,
    config: ConfigStore,
    options: &ServiceOptions,
    supervisor: impl FnOnce() -> Result<Supervisor>,
) -> Result<ServeExit> {
    paths.ensure_private()?;
    let lock_path = paths.lock();
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(&lock_path)
        .with_context(|| format!("opening {}", lock_path.display()))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(ServeExit::AlreadyRunning),
        Err(TryLockError::Error(error)) => {
            return Err(error).with_context(|| format!("locking {}", lock_path.display()));
        }
    }
    let mut cleanup = Cleanup {
        socket: None,
        pid: paths.pid(),
        _lock: lock,
    };
    write_pid(&cleanup.pid)?;
    let socket = paths.socket()?;
    remove_stale_socket(&socket)?;
    let supervisor = supervisor()?;
    let listener = UnixListener::bind(&socket)
        .with_context(|| format!("listening on {}", socket.display()))?;
    cleanup.socket = Some(socket.clone());
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))
        .with_context(|| format!("securing {}", socket.display()))?;
    listener
        .set_nonblocking(true)
        .context("configuring the service socket")?;

    let server = Arc::new(Server {
        paths: paths.clone(),
        config: Mutex::new(config),
        supervisor,
        // The id of the image this process runs, fixed at startup: a rebuilt
        // binary on disk must not make an old service look current.
        build_id: paths::build_id(),
        last_arrival: Mutex::new(Instant::now()),
        opener: options.opener,
    });
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                server.arrived();
                let server = server.clone();
                thread::spawn(move || server.connection(stream));
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if server.idle(options.idle_exit) {
                    return Ok(ServeExit::Idle);
                }
                thread::sleep(ACCEPT_POLL);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            // Running out of descriptors is transient; keep serving.
            Err(error) => {
                eprintln!("{NAMESPACE} service: accepting a connection: {error}");
                thread::sleep(ACCEPT_POLL);
            }
        }
    }
}

/// Removes the socket and pid file when `serve` returns, then releases the
/// lock (fields drop in order).
struct Cleanup {
    socket: Option<PathBuf>,
    pid: PathBuf,
    _lock: File,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Some(socket) = &self.socket {
            let _ = fs::remove_file(socket);
        }
        let _ = fs::remove_file(&self.pid);
    }
}

fn write_pid(path: &Path) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    writeln!(file, "{}", std::process::id()).with_context(|| format!("writing {}", path.display()))
}

/// A socket left by a killed service is removed; anything else at that path
/// is left alone and refuses startup.
fn remove_stale_socket(socket: &Path) -> Result<()> {
    match fs::symlink_metadata(socket) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            fs::remove_file(socket).with_context(|| format!("removing stale {}", socket.display()))
        }
        Ok(_) => bail!("{} exists and is not a socket", socket.display()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("reading {}", socket.display())),
    }
}

struct Server {
    paths: ServicePaths,
    config: Mutex<ConfigStore>,
    supervisor: Supervisor,
    build_id: String,
    last_arrival: Mutex<Instant>,
    opener: Opener,
}

impl Server {
    fn arrived(&self) {
        *lock(&self.last_arrival) = Instant::now();
    }

    /// Nothing queued or running, and neither a request nor a run event for
    /// `idle_exit`.
    fn idle(&self, idle_exit: Duration) -> bool {
        self.supervisor.idle_expired() && lock(&self.last_arrival).elapsed() >= idle_exit
    }

    /// Answer frames in order until the peer closes the connection.
    fn connection(&self, mut stream: UnixStream) {
        // Accepted sockets inherit the listener's non-blocking mode.
        if stream.set_nonblocking(false).is_err() {
            return;
        }
        loop {
            let body = match read_frame(&mut stream) {
                Ok(Some(body)) => body,
                Ok(None) | Err(FrameError::Io(_)) => return,
                Err(FrameError::TooLarge) => {
                    // The body was not read, so the stream cannot be resynced.
                    let _ = write_frame(
                        &mut stream,
                        &error(
                            "",
                            ErrorCode::MessageTooLarge,
                            "service message exceeds the 4 MiB inbound limit",
                        ),
                    );
                    return;
                }
                Err(FrameError::InvalidLength) => {
                    let response = error(
                        "",
                        ErrorCode::InvalidRequest,
                        "service message has an empty body",
                    );
                    if write_frame(&mut stream, &response).is_err() {
                        return;
                    }
                    continue;
                }
            };
            self.arrived();
            let response = match parse_envelope(&body) {
                Ok((_, request)) => self.dispatch_service(request),
                Err(response) => response,
            };
            if write_frame(&mut stream, &response).is_err() {
                return;
            }
        }
    }

    fn dispatch_service(&self, request: Request) -> Response {
        let id = request.request_id().to_owned();
        match request {
            Request::ServiceHello(value) => Response::ServiceReady(ServiceReady {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                build_id: self.build_id.clone(),
            }),
            Request::StartRun(value) => {
                let Some(record_key) = value.record_key else {
                    return error(&id, ErrorCode::InvalidRequest, "record_key is required");
                };
                // The host checks these too; a direct socket peer is bounded
                // the same way.
                if let Err(message) = check_start_run(
                    &value.url,
                    &value.title,
                    &value.content,
                    value.selection.as_deref(),
                ) {
                    return error(&id, ErrorCode::InvalidRequest, message);
                }
                let setup = match self.run_setup(value.harness) {
                    Ok(setup) => setup,
                    Err((code, message)) => return error(&id, code, message),
                };
                // Checked now so a bad agent.env is reported at the click;
                // the run reads it again when it leaves the queue.
                if let Err(env_error) = run_env(&self.paths) {
                    return error(&id, ErrorCode::InvalidConfig, &env_error.to_string());
                }
                match self.supervisor.start_run(StartRun {
                    url: value.url,
                    title: value.title,
                    content: value.content,
                    selection: value.selection,
                    harness: Some(setup.harness),
                    record_key,
                    requested_at: value.requested_at,
                    cwd: setup.cwd,
                    timeout: setup.timeout,
                    vocabulary: setup.vocabulary,
                }) {
                    Ok((run, existing)) => Response::RunStarted(RunStarted {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id,
                        run,
                        existing,
                    }),
                    Err((code, message)) => error(&id, code, &message),
                }
            }
            Request::GetRun(value) => {
                let Some(run_id) = parse_run_id(&value.run_id) else {
                    return bad_run_id(&id);
                };
                match self.supervisor.get_run(run_id, value.since_offset) {
                    Ok((run, page)) => Response::RunFetched(RunFetched {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id,
                        run,
                        stream_text: page.text,
                        next_offset: page.next_offset,
                        more: page.more,
                    }),
                    Err((code, message)) => error(&id, code, &message),
                }
            }
            Request::ListRuns(value) => Response::RunsListed(RunsListed {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                runs: self.supervisor.list_runs(),
            }),
            Request::CancelRun(value) => {
                let Some(run_id) = parse_run_id(&value.run_id) else {
                    return bad_run_id(&id);
                };
                run_updated(&id, self.supervisor.cancel_run(run_id))
            }
            Request::RetrySave(value) => {
                let Some(run_id) = parse_run_id(&value.run_id) else {
                    return bad_run_id(&id);
                };
                run_updated(&id, self.supervisor.retry_save(run_id))
            }
            // Detected with the environment a run gets, so what settings
            // shows is what runs.
            Request::DiscoverHarnesses(value) => match run_env(&self.paths) {
                Ok(env) => Response::HarnessesDiscovered(HarnessesDiscovered {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: value.request_id,
                    harnesses: setup::discover(&HarnessEnv::from_run_env(&env), &env),
                }),
                Err(env_error) => error(&id, ErrorCode::InvalidConfig, &env_error.to_string()),
            },
            Request::SetUpHarness(value) => self.set_up_harness(value),
            Request::OpenPath(value) => {
                let run_id = match value.run_id.as_deref().map(parse_run_id) {
                    None => None,
                    Some(Some(run_id)) => Some(run_id),
                    Some(None) => return bad_run_id(&id),
                };
                let store = self.supervisor.store();
                match open_target(&self.paths, store, value.target, run_id, self.opener) {
                    Ok(()) => Response::PathOpened(PathOpened {
                        protocol_version: PROTOCOL_VERSION,
                        request_id: value.request_id,
                    }),
                    Err((code, message)) => error(&id, code, &message),
                }
            }
            Request::GetAgentEnv(value) => {
                let (names, problem) = list_agent_env(&self.paths);
                Response::AgentEnvListed(AgentEnvListed {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: value.request_id,
                    names,
                    problem,
                })
            }
            _ => error(&id, ErrorCode::InvalidRequest, "not a service request"),
        }
    }

    /// Detect and probe the harness, then commit it. Setup gets its own
    /// config store, so the probe never holds up another connection.
    fn set_up_harness(&self, value: SetUpHarnessRequest) -> Response {
        let env = match run_env(&self.paths) {
            Ok(env) => env,
            Err(env_error) => {
                return error(
                    &value.request_id,
                    ErrorCode::InvalidConfig,
                    &env_error.to_string(),
                );
            }
        };
        let mut config = lock(&self.config).reopen();
        match setup::set_up(&mut config, self.supervisor.registry(), &value, env) {
            Ok((config, revision)) => Response::ConfigUpdated(ConfigUpdated {
                protocol_version: PROTOCOL_VERSION,
                request_id: value.request_id,
                config,
                revision,
            }),
            Err((code, message)) => error(&value.request_id, code, &message),
        }
    }

    /// The harness, working directory, and time limit for a new run, read
    /// from the service's own configuration under the config lock. Runs use
    /// the harness setup probed and committed, with its entry's time limit;
    /// a request may name that harness but no other. The harness runs in the
    /// knowledge base folder, which must still be the folder the user
    /// picked.
    fn run_setup(
        &self,
        requested: Option<HarnessId>,
    ) -> Result<RunSetup, (ErrorCode, &'static str)> {
        let mut config = lock(&self.config);
        let _config_lock = config.lock_current().map_err(|_| {
            (
                ErrorCode::Internal,
                "The configuration could not be locked.",
            )
        })?;
        if config.refresh().is_err() || config.config_issue().is_some() {
            return Err((
                ErrorCode::InvalidConfig,
                "The configuration is unavailable or needs repair in Brauser settings.",
            ));
        }
        let knowledge_base = checked_knowledge_base(&config)?;
        let agent = config
            .snapshot()
            .agent
            .as_ref()
            .ok_or((ErrorCode::NotConfigured, SET_UP_HARNESS))?;
        if let Some(requested) = requested.filter(|requested| *requested != agent.harness) {
            return Err((ErrorCode::NotConfigured, not_set_up(requested)));
        }
        Ok(RunSetup {
            harness: agent.harness,
            cwd: knowledge_base,
            timeout: Some(Duration::from_secs(agent.timeout_secs.into())),
            vocabulary: config.snapshot().summary_vocabulary.clone(),
        })
    }
}

/// The refusal for a run naming `harness` when another one is set up.
fn not_set_up(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::Codex => "Codex is not the summary harness set up in Brauser settings.",
        HarnessId::Claude => "Claude Code is not the summary harness set up in Brauser settings.",
    }
}

/// The knowledge base folder a harness runs in, which must still be the
/// folder the user picked. Call right after `refresh`.
pub(crate) fn checked_knowledge_base(
    config: &ConfigStore,
) -> Result<PathBuf, (ErrorCode, &'static str)> {
    let knowledge_base = match config.knowledge_base_dir() {
        Some(folder) if config.configured() => folder,
        _ => return Err((ErrorCode::NotConfigured, NOT_CONFIGURED)),
    };
    // `selected_root_identity` fails on anything but a folder.
    let unchanged = selected_root_identity(&knowledge_base)
        .is_ok_and(|actual| Some(actual.as_str()) == config.knowledge_base_identity());
    if !unchanged {
        return Err((ErrorCode::NotConfigured, KNOWLEDGE_BASE_CHANGED));
    }
    Ok(knowledge_base)
}

/// What [`Server::run_setup`] resolved for a new run.
struct RunSetup {
    harness: HarnessId,
    cwd: PathBuf,
    timeout: Option<Duration>,
    vocabulary: SummaryVocabulary,
}

fn parse_run_id(run_id: &str) -> Option<Uuid> {
    Uuid::parse_str(run_id).ok()
}

fn bad_run_id(request_id: &str) -> Response {
    error(
        request_id,
        ErrorCode::InvalidRequest,
        "run_id is not a run id",
    )
}

fn run_updated(
    request_id: &str,
    result: Result<brauser_protocol::RunView, (ErrorCode, String)>,
) -> Response {
    match result {
        Ok(run) => Response::RunUpdated(RunUpdated {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
            run,
        }),
        Err((code, message)) => error(request_id, code, &message),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::{FileTypeExt, PermissionsExt};
    use std::os::unix::net::UnixStream;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::thread::{self, JoinHandle};
    use std::time::{Duration, Instant};

    use brauser_protocol::{
        ErrorCode, GetRunRequest, HarnessId, HelloRequest, ListRunsRequest, PROTOCOL_VERSION,
        RecordKey, RecordKeyKind, Request, Response, RunState, ServiceHelloRequest,
        StartRunRequest,
    };

    use crate::answer::TEST_ANSWER;
    use crate::brand::FAKE_HARNESS_ENV;
    use crate::config::fixtures;
    use crate::frame::{read_frame, write_request};
    use crate::service::harness_api::HarnessRegistry;
    use crate::service::paths::build_id;
    use crate::service::record::RunStore;
    use crate::service::supervisor::{DEFAULT_RUN_TIMEOUT, VaultCheckFinisher};

    /// A short `/tmp` support directory (for `sun_path`), a notes folder, and
    /// a fake harness script.
    struct World {
        _temp: tempfile::TempDir,
        support: PathBuf,
        notes: PathBuf,
        program: PathBuf,
        paths: ServicePaths,
    }

    impl World {
        /// A notes folder and a Claude Code harness entry.
        fn new(body: &str) -> Self {
            let world = Self::unconfigured(body);
            world.configure(Some(""), None);
            world
        }

        /// Write config.toml for the notes folder. `agent` adds a Claude Code
        /// entry followed by these extra tables; `knowledge_base` is a separately
        /// picked knowledge base folder, recorded with its current identity.
        fn configure(&self, agent: Option<&str>, knowledge_base: Option<&Path>) {
            let quoted = |value: &str| toml::Value::String(value.to_owned()).to_string();
            let (storage_extra, keys) = knowledge_base.map_or_else(Default::default, |folder| {
                let identity = crate::vault::selected_root_identity(folder).unwrap();
                (
                    format!("knowledge_base_dir = {}", quoted(&folder.to_string_lossy())),
                    format!("knowledge_base_identity = {}", quoted(&identity)),
                )
            });
            let tables = agent.map_or_else(String::new, |extra| {
                let binary = fixtures::absent_binary("claude");
                format!("{}{extra}", fixtures::agent_table(Path::new(&binary)))
            });
            fixtures::write(
                &self.support.join("config.toml"),
                &self.notes,
                &storage_extra,
                &keys,
                &tables,
            );
        }

        fn unconfigured(body: &str) -> Self {
            let temp = tempfile::Builder::new()
                .prefix("bs")
                .tempdir_in("/tmp")
                .unwrap();
            let support = temp.path().join("s");
            std::fs::create_dir(&support).unwrap();
            std::fs::set_permissions(&support, std::fs::Permissions::from_mode(0o755)).unwrap();
            let notes = temp.path().join("n");
            std::fs::create_dir(&notes).unwrap();
            let program = temp.path().join("h");
            std::fs::write(&program, format!("#!/bin/sh\n{body}")).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            let paths = ServicePaths::at(support.clone());
            Self {
                _temp: temp,
                support,
                notes,
                program,
                paths,
            }
        }

        fn config(&self) -> ConfigStore {
            ConfigStore::for_test(self.support.join("config.toml"))
        }

        fn socket(&self) -> PathBuf {
            self.paths.socket().unwrap()
        }

        fn options(
            paths: &ServicePaths,
            idle_exit: Duration,
            run_timeout: Duration,
        ) -> ServiceOptions {
            ServiceOptions {
                run_timeout,
                forced_run_timeout: None,
                idle_exit,
                service_pid: std::process::id(),
                run_env: env_source(paths),
                opener: crate::open::default_opener(),
            }
        }

        /// Run `serve` on a thread and wait until its socket accepts.
        fn start(&self, idle_exit: Duration) -> JoinHandle<Result<ServeExit>> {
            self.start_with(idle_exit, DEFAULT_RUN_TIMEOUT)
        }

        /// [`World::start`] with the service's own run time limit.
        fn start_with(
            &self,
            idle_exit: Duration,
            run_timeout: Duration,
        ) -> JoinHandle<Result<ServeExit>> {
            let paths = self.paths.clone();
            let config = self.config();
            let finisher_config = self.config();
            let program = self.program.clone();
            let handle = thread::spawn(move || {
                let options = Self::options(&paths, idle_exit, run_timeout);
                serve(&paths, config, &options, || {
                    let registry = HarnessRegistry::from_lookup(|name| {
                        (name == FAKE_HARNESS_ENV).then(|| program.clone().into_os_string())
                    });
                    Ok(Supervisor::new(
                        RunStore::open(paths.runs())?,
                        registry,
                        Box::new(VaultCheckFinisher::new(finisher_config)),
                        options.clone(),
                    ))
                })
            });
            let deadline = Instant::now() + Duration::from_secs(5);
            while UnixStream::connect(self.socket()).is_err() {
                assert!(Instant::now() < deadline, "service never listened");
                thread::sleep(Duration::from_millis(20));
            }
            handle
        }

        fn call(&self, request: &Request) -> Response {
            let mut stream = UnixStream::connect(self.socket()).unwrap();
            write_request(&mut stream, request).unwrap();
            read_response(&mut stream)
        }

        fn start_run(&self, record_key: Option<RecordKey>) -> Response {
            self.call(&Request::StartRun(StartRunRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "start".into(),
                url: "https://example.com/a".into(),
                title: "Page A".into(),
                content: "Body text".into(),
                selection: None,
                harness: None,
                record_key,
                requested_at: "2026-10-26T10:00:00Z".into(),
            }))
        }

        fn started_run_id(&self) -> String {
            match self.start_run(Some(url_key())) {
                Response::RunStarted(started) => {
                    assert!(!started.existing);
                    started.run.run_id
                }
                other => panic!("unexpected response: {other:?}"),
            }
        }

        fn get_run(&self, run_id: &str) -> brauser_protocol::RunFetched {
            match self.call(&Request::GetRun(GetRunRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "get".into(),
                run_id: run_id.into(),
                since_offset: 0,
            })) {
                Response::RunFetched(fetched) => fetched,
                other => panic!("unexpected response: {other:?}"),
            }
        }

        fn wait_finished(&self, run_id: &str) -> brauser_protocol::RunFetched {
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let fetched = self.get_run(run_id);
                if !matches!(fetched.run.state, RunState::Queued | RunState::Running) {
                    return fetched;
                }
                assert!(Instant::now() < deadline, "run never finished: {fetched:?}");
                thread::sleep(Duration::from_millis(20));
            }
        }

        fn write_agent_env(&self, text: &str, mode: u32) {
            let path = self.paths.agent_env();
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
    }

    fn url_key() -> RecordKey {
        RecordKey {
            kind: RecordKeyKind::Url,
            value: "https://example.com/a".into(),
        }
    }

    fn read_response(stream: &mut UnixStream) -> Response {
        let body = read_frame(stream).unwrap().expect("a response frame");
        serde_json::from_slice(&body).unwrap()
    }

    fn error_of(response: Response) -> (ErrorCode, String) {
        match response {
            Response::Error(error) => (error.code, error.message),
            other => panic!("expected an error, got {other:?}"),
        }
    }

    /// A fake harness body that prints a valid answer.
    fn answer() -> String {
        format!("echo '{TEST_ANSWER}'\n")
    }

    /// As [`answer`], with the description replaced by `description`, which
    /// the shell expands.
    fn answering(description: &str) -> String {
        format!("echo '{TEST_ANSWER}' | /usr/bin/sed \"s|A page.|{description}|\"\n")
    }

    #[test]
    fn framed_start_and_get_run_over_the_socket() {
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        let run_id = world.started_run_id();
        let fetched = world.wait_finished(&run_id);
        assert_eq!(fetched.run.state, RunState::Succeeded, "{fetched:?}");
        assert!(
            fetched.stream_text.contains("\"document_type\""),
            "{fetched:?}"
        );
        assert!(fetched.run.answer.is_some(), "{fetched:?}");
        assert!(fetched.next_offset > 0);
        assert!(!fetched.more);
        // The same record key while queued or running returns that run; once
        // finished, list_runs reports it.
        match world.call(&Request::ListRuns(ListRunsRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "list".into(),
        })) {
            Response::RunsListed(listed) => {
                assert_eq!(listed.runs.len(), 1);
                assert_eq!(listed.runs[0].run_id, run_id);
            }
            other => panic!("unexpected response: {other:?}"),
        }
        // Several frames on one connection are answered in order.
        let mut stream = UnixStream::connect(world.socket()).unwrap();
        for id in ["one", "two"] {
            write_request(
                &mut stream,
                &Request::ServiceHello(ServiceHelloRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: id.into(),
                }),
            )
            .unwrap();
            assert_eq!(read_response(&mut stream).request_id(), id);
        }
    }

    #[test]
    fn service_hello_reports_this_build() {
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        match world.call(&Request::ServiceHello(ServiceHelloRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "hi".into(),
        })) {
            Response::ServiceReady(ready) => {
                assert_eq!(ready.request_id, "hi");
                assert_eq!(ready.build_id, build_id());
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn version_5_request_gets_unsupported_protocol_version() {
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        let mut stream = UnixStream::connect(world.socket()).unwrap();
        let body = br#"{"type":"service_hello","protocol_version":5,"request_id":"old"}"#;
        stream
            .write_all(&u32::try_from(body.len()).unwrap().to_ne_bytes())
            .unwrap();
        stream.write_all(body).unwrap();
        let response = read_response(&mut stream);
        assert_eq!(response.request_id(), "old");
        assert_eq!(error_of(response).0, ErrorCode::UnsupportedProtocolVersion);
    }

    #[test]
    fn non_service_request_is_invalid_request() {
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        let (code, message) = error_of(world.call(&Request::Hello(HelloRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "hello".into(),
        })));
        assert_eq!(code, ErrorCode::InvalidRequest);
        assert_eq!(message, "not a service request");
    }

    #[test]
    fn start_run_without_record_key_is_invalid_request() {
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        let (code, _) = error_of(world.start_run(None));
        assert_eq!(code, ErrorCode::InvalidRequest);
        assert!(
            !world.paths.runs().exists()
                || std::fs::read_dir(world.paths.runs())
                    .unwrap()
                    .next()
                    .is_none()
        );
    }

    #[test]
    fn start_run_over_the_schema_bounds_from_a_socket_peer_is_invalid_request() {
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        let (code, message) = error_of(world.call(&Request::StartRun(StartRunRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "big".into(),
            url: "https://example.com/a".into(),
            title: "t".repeat(64 * 1024),
            content: "Body text".into(),
            selection: None,
            harness: None,
            record_key: Some(url_key()),
            requested_at: "2026-10-26T10:00:00Z".into(),
        })));
        assert_eq!(code, ErrorCode::InvalidRequest);
        assert!(message.contains("title"), "{message}");
        assert!(
            !world.paths.runs().exists()
                || std::fs::read_dir(world.paths.runs())
                    .unwrap()
                    .next()
                    .is_none()
        );
    }

    #[test]
    fn start_run_without_a_notes_folder_is_not_configured() {
        let world = World::unconfigured(&answer());
        world.start(Duration::from_secs(600));
        let (code, message) = error_of(world.start_run(Some(url_key())));
        assert_eq!(code, ErrorCode::NotConfigured);
        assert_eq!(message, "Choose a notes folder in Brauser settings first.");
    }

    #[test]
    fn start_run_uses_the_knowledge_base_folder_as_cwd() {
        let world = World::unconfigured(&answering("cwd=$(pwd -P)"));
        let knowledge_base = world._temp.path().join("kb");
        std::fs::create_dir(&knowledge_base).unwrap();
        world.configure(Some(""), Some(&knowledge_base));
        world.start(Duration::from_secs(600));
        let run_id = world.started_run_id();
        let fetched = world.wait_finished(&run_id);
        assert_eq!(fetched.run.state, RunState::Succeeded, "{fetched:?}");
        // The configured Claude Code entry picks the harness.
        assert_eq!(fetched.run.harness, HarnessId::Claude);
        let expected = std::fs::canonicalize(&knowledge_base).unwrap();
        assert!(
            fetched
                .stream_text
                .contains(&format!("cwd={}", expected.display())),
            "{fetched:?}"
        );
    }

    #[test]
    fn start_run_prompts_with_the_configured_vocabulary() {
        let world = World::unconfigured(&format!("cat > \"$0.prompt\"\n{}", answer()));
        world.configure(
            Some("\n[summary_vocabulary]\ndocument_types = [\"memo\"]\ncategories = [\"legal review\"]\n"),
            None,
        );
        world.start(Duration::from_secs(600));
        let run_id = world.started_run_id();
        let fetched = world.wait_finished(&run_id);
        assert_eq!(fetched.run.state, RunState::Succeeded, "{fetched:?}");
        let prompt = std::fs::read_to_string(world.program.with_extension("prompt")).unwrap();
        let page = world.paths.runs().join(&run_id).join("page.md");
        assert!(prompt.contains(&page.display().to_string()), "{prompt}");
        assert!(
            prompt.contains("\n- memo\n") && prompt.contains("\n- legal review\n"),
            "{prompt}"
        );
        assert!(!prompt.contains("- report\n"), "{prompt}");
        assert!(!prompt.contains("Body text"), "{prompt}");
    }

    #[test]
    fn start_run_without_agent_or_harness_is_not_configured() {
        let world = World::unconfigured(&answer());
        world.configure(None, None);
        world.start(Duration::from_secs(600));
        let (code, message) = error_of(world.start_run(Some(url_key())));
        assert_eq!(code, ErrorCode::NotConfigured);
        assert_eq!(
            message,
            "Set up a summary harness in Brauser settings first."
        );
        // Naming a harness does not stand in for setting one up.
        let request = named_start_run(HarnessId::Codex);
        assert_eq!(
            error_of(world.call(&Request::StartRun(request))),
            (
                ErrorCode::NotConfigured,
                "Set up a summary harness in Brauser settings first.".to_string()
            )
        );
        assert!(!world.paths.runs().exists() || dir_is_empty(&world.paths.runs()));
    }

    fn named_start_run(harness: HarnessId) -> StartRunRequest {
        StartRunRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "named".into(),
            url: "https://example.com/a".into(),
            title: "Page A".into(),
            content: "Body text".into(),
            selection: None,
            harness: Some(harness),
            record_key: Some(url_key()),
            requested_at: "2026-10-26T10:00:00Z".into(),
        }
    }

    fn dir_is_empty(path: &Path) -> bool {
        std::fs::read_dir(path).unwrap().next().is_none()
    }

    /// Only the harness that passed setup's probe runs: a request naming
    /// another is refused, and one naming the set-up harness gets its entry.
    #[test]
    fn start_run_refuses_a_harness_that_is_not_set_up() {
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        assert_eq!(
            error_of(world.call(&Request::StartRun(named_start_run(HarnessId::Codex)))),
            (
                ErrorCode::NotConfigured,
                "Codex is not the summary harness set up in Brauser settings.".to_string()
            )
        );
        assert!(!world.paths.runs().exists() || dir_is_empty(&world.paths.runs()));
        match world.call(&Request::StartRun(named_start_run(HarnessId::Claude))) {
            Response::RunStarted(started) => {
                assert_eq!(started.run.harness, HarnessId::Claude);
                let fetched = world.wait_finished(&started.run.run_id);
                assert_eq!(fetched.run.state, RunState::Succeeded, "{fetched:?}");
            }
            other => panic!("unexpected response: {other:?}"),
        }
    }

    #[test]
    fn replaced_knowledge_base_folder_is_refused() {
        let world = World::unconfigured(&answer());
        let knowledge_base = world._temp.path().join("kb");
        std::fs::create_dir(&knowledge_base).unwrap();
        world.configure(Some(""), Some(&knowledge_base));
        world.start(Duration::from_secs(600));
        let refused =
            "The knowledge base folder changed or is missing. Choose it again in settings.";
        // Missing, then a different folder at the same path.
        std::fs::remove_dir(&knowledge_base).unwrap();
        assert_eq!(
            error_of(world.start_run(Some(url_key()))),
            (ErrorCode::NotConfigured, refused.to_string())
        );
        std::fs::create_dir(&knowledge_base).unwrap();
        assert_eq!(
            error_of(world.start_run(Some(url_key()))),
            (ErrorCode::NotConfigured, refused.to_string())
        );
        // A file in its place is not a folder either.
        std::fs::remove_dir(&knowledge_base).unwrap();
        std::fs::write(&knowledge_base, "not a folder").unwrap();
        assert_eq!(
            error_of(world.start_run(Some(url_key()))),
            (ErrorCode::NotConfigured, refused.to_string())
        );
        assert!(
            !world.paths.runs().exists()
                || std::fs::read_dir(world.paths.runs())
                    .unwrap()
                    .next()
                    .is_none()
        );
    }

    #[test]
    fn agent_timeout_secs_bounds_the_run() {
        // The service's own limit is 1 second here; the entry's 10 minutes
        // win, so a 2 second answer finishes.
        let world = World::new(&format!("/bin/sleep 2\n{}", answer()));
        world.start_with(Duration::from_secs(600), Duration::from_secs(1));
        let started = Instant::now();
        let run_id = world.started_run_id();
        let fetched = world.wait_finished(&run_id);
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert_eq!(fetched.run.state, RunState::Succeeded, "{fetched:?}");

        // A request naming the set-up harness gets the same limit.
        let world = World::new(&format!("/bin/sleep 2\n{}", answer()));
        world.start_with(Duration::from_secs(600), Duration::from_secs(1));
        let run_id = match world.call(&Request::StartRun(named_start_run(HarnessId::Claude))) {
            Response::RunStarted(started) => started.run.run_id,
            other => panic!("unexpected response: {other:?}"),
        };
        let fetched = world.wait_finished(&run_id);
        assert_eq!(fetched.run.state, RunState::Succeeded, "{fetched:?}");
    }

    #[test]
    fn agent_env_file_reaches_the_harness() {
        let world = World::new(&answering("$FOO"));
        world.write_agent_env("FOO=bar\n", 0o600);
        world.start(Duration::from_secs(600));
        let run_id = world.started_run_id();
        let fetched = world.wait_finished(&run_id);
        assert_eq!(fetched.run.state, RunState::Succeeded, "{fetched:?}");
        assert!(fetched.stream_text.contains("bar"), "{fetched:?}");
    }

    #[test]
    fn group_readable_agent_env_fails_start_without_values() {
        let world = World::new(&answer());
        world.write_agent_env("FOO=secret\n", 0o644);
        world.start(Duration::from_secs(600));
        let (code, message) = error_of(world.start_run(Some(url_key())));
        assert_eq!(code, ErrorCode::InvalidConfig);
        assert!(message.contains("agent.env"), "{message}");
        assert!(!message.contains("secret"), "{message}");
    }

    #[test]
    fn second_instance_reports_already_running() {
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        let built = Arc::new(AtomicBool::new(false));
        let flag = built.clone();
        let exit = serve(
            &world.paths,
            world.config(),
            &World::options(&world.paths, Duration::from_secs(600), DEFAULT_RUN_TIMEOUT),
            || {
                flag.store(true, Ordering::SeqCst);
                anyhow::bail!("a second instance must not build a supervisor")
            },
        )
        .unwrap();
        assert_eq!(exit, ServeExit::AlreadyRunning);
        // Recovery never ran beside the live service, which still answers.
        assert!(!built.load(Ordering::SeqCst));
        assert_eq!(
            std::fs::read_to_string(world.paths.pid()).unwrap().trim(),
            std::process::id().to_string()
        );
        assert!(matches!(
            world.call(&Request::ServiceHello(ServiceHelloRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "still".into(),
            })),
            Response::ServiceReady(_)
        ));
    }

    #[test]
    fn idle_service_exits_and_removes_socket() {
        let world = World::new(&answer());
        let started = Instant::now();
        let handle = world.start(Duration::from_millis(300));
        assert!(world.paths.pid().exists());
        let exit = handle.join().unwrap().unwrap();
        assert_eq!(exit, ServeExit::Idle);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(!world.socket().exists());
        assert!(!world.paths.pid().exists());
        // A stale socket is replaced on the next start.
        std::os::unix::net::UnixListener::bind(world.socket()).unwrap();
        let handle = world.start(Duration::from_millis(300));
        assert_eq!(handle.join().unwrap().unwrap(), ServeExit::Idle);
    }

    #[test]
    fn socket_is_0600_in_a_0700_dir() {
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        let dir = std::fs::metadata(&world.support).unwrap();
        assert_eq!(dir.permissions().mode() & 0o777, 0o700);
        let socket = std::fs::symlink_metadata(world.socket()).unwrap();
        assert!(socket.file_type().is_socket());
        assert_eq!(socket.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn open_path_and_get_agent_env_over_the_socket() {
        use brauser_protocol::{GetAgentEnvRequest, OpenPathRequest, OpenTarget};
        let world = World::new(&answer());
        world.start(Duration::from_secs(600));
        world.write_agent_env("FOO=secret-value\n", 0o600);
        match world.call(&Request::GetAgentEnv(GetAgentEnvRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "env".into(),
        })) {
            Response::AgentEnvListed(listed) => {
                assert_eq!(listed.request_id, "env");
                assert_eq!(listed.names, vec!["FOO"]);
                assert_eq!(listed.problem, None);
            }
            other => panic!("unexpected response: {other:?}"),
        }
        let open = |target, run_id: Option<&str>| {
            world.call(&Request::OpenPath(OpenPathRequest {
                protocol_version: PROTOCOL_VERSION,
                request_id: "open".into(),
                target,
                run_id: run_id.map(str::to_owned),
            }))
        };
        let (code, _) = error_of(open(OpenTarget::CodexThread, Some("not-a-run-id")));
        assert_eq!(code, ErrorCode::InvalidRequest);
        // The fake Claude Code run has no Codex thread.
        let run_id = world.started_run_id();
        world.wait_finished(&run_id);
        let (code, _) = error_of(open(OpenTarget::CodexThread, Some(&run_id)));
        assert_eq!(code, ErrorCode::NotConfigured);
        // The test build never runs /usr/bin/open; agent.env is left as it is.
        open(OpenTarget::AgentEnv, None);
        assert_eq!(
            std::fs::read_to_string(world.paths.agent_env()).unwrap(),
            "FOO=secret-value\n"
        );
    }

    #[test]
    fn a_regular_file_at_the_socket_path_is_not_removed() {
        let world = World::new(&answer());
        std::fs::write(world.socket(), "keep").unwrap();
        let error = serve(
            &world.paths,
            world.config(),
            &World::options(&world.paths, Duration::from_secs(600), DEFAULT_RUN_TIMEOUT),
            || anyhow::bail!("not reached"),
        );
        assert!(error.is_err());
        assert_eq!(std::fs::read_to_string(world.socket()).unwrap(), "keep");
        // The failed start left no pid file behind.
        assert!(!world.paths.pid().exists());
    }
}
