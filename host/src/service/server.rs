//! The service's private Unix socket: one listener in the 0700 support
//! directory, one thread per connection, and the same length-prefixed JSON
//! framing as native messaging. The service answers `service_hello` and the
//! five run messages; it exits after `idle_exit` with nothing queued or
//! running and no request.

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
    ErrorCode, PROTOCOL_VERSION, Request, Response, RunFetched, RunStarted, RunUpdated, RunsListed,
    ServiceReady,
};
use uuid::Uuid;

use super::agent_env::{AgentEnv, AgentEnvError};
use super::paths::{self, ServicePaths};
use super::record::check_start_run;
use super::supervisor::{EnvSource, ServiceOptions, StartRun, Supervisor};
use crate::brand::NAMESPACE;
use crate::config::ConfigStore;
use crate::frame::{FrameError, read_frame, write_frame};
use crate::native::{error, parse_envelope};

/// How often the accept loop checks for a connection and for idle exit.
const ACCEPT_POLL: Duration = Duration::from_millis(250);
pub const NOT_CONFIGURED: &str = "Choose a notes folder in Brauser settings first.";

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
                let cwd = match self.run_cwd() {
                    Ok(cwd) => cwd,
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
                    harness: value.harness,
                    record_key,
                    requested_at: value.requested_at,
                    cwd,
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
            _ => error(&id, ErrorCode::InvalidRequest, "not a service request"),
        }
    }

    /// PR 2 runs the harness in the notes folder, read from the service's own
    /// configuration under the config lock.
    fn run_cwd(&self) -> Result<PathBuf, (ErrorCode, &'static str)> {
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
        match config.snapshot().storage.as_ref() {
            Some(storage) if config.configured() => Ok(PathBuf::from(&storage.root)),
            _ => Err((ErrorCode::NotConfigured, NOT_CONFIGURED)),
        }
    }
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
        ErrorCode, GetRunRequest, HelloRequest, ListRunsRequest, PROTOCOL_VERSION, RecordKey,
        RecordKeyKind, Request, Response, RunState, ServiceHelloRequest, StartRunRequest,
    };

    use crate::brand::FAKE_HARNESS_ENV;
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
        fn new(body: &str) -> Self {
            let world = Self::unconfigured(body);
            let mut store = world.config();
            let snapshot = brauser_protocol::ConfigSnapshot {
                storage: Some(brauser_protocol::StorageConfig {
                    root: world.notes.to_string_lossy().into_owned(),
                    profile: "neutral".into(),
                    log_dir: crate::layout::LOGS_DIR.into(),
                    pages_dir: crate::layout::NOTES_DIR.into(),
                    later_dir: crate::layout::LATER_DIR.into(),
                    artifacts_dir: crate::layout::ARTIFACTS_DIR.into(),
                    summaries_dir: Some(crate::layout::SUMMARIES_DIR.into()),
                    knowledge_base_dir: None,
                }),
                capture_enabled: false,
                strip_params: Vec::new(),
                ..crate::config::empty_config()
            };
            let identity = crate::vault::selected_root_identity(&world.notes).unwrap();
            store
                .update_with_root_identity(snapshot, "missing", Some(&identity))
                .unwrap()
                .unwrap();
            world
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

        fn options(paths: &ServicePaths, idle_exit: Duration) -> ServiceOptions {
            ServiceOptions {
                run_timeout: DEFAULT_RUN_TIMEOUT,
                idle_exit,
                service_pid: std::process::id(),
                run_env: env_source(paths),
            }
        }

        /// Run `serve` on a thread and wait until its socket accepts.
        fn start(&self, idle_exit: Duration) -> JoinHandle<Result<ServeExit>> {
            let paths = self.paths.clone();
            let config = self.config();
            let finisher_config = self.config();
            let program = self.program.clone();
            let handle = thread::spawn(move || {
                let options = Self::options(&paths, idle_exit);
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

    const ANSWER: &str = "echo '{\"ok\": true}'\n";

    #[test]
    fn framed_start_and_get_run_over_the_socket() {
        let world = World::new(ANSWER);
        world.start(Duration::from_secs(600));
        let run_id = world.started_run_id();
        let fetched = world.wait_finished(&run_id);
        assert_eq!(fetched.run.state, RunState::Succeeded, "{fetched:?}");
        assert!(fetched.stream_text.contains("\"ok\""), "{fetched:?}");
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
        let world = World::new(ANSWER);
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
        let world = World::new(ANSWER);
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
        let world = World::new(ANSWER);
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
        let world = World::new(ANSWER);
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
        let world = World::new(ANSWER);
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
        let world = World::unconfigured(ANSWER);
        world.start(Duration::from_secs(600));
        let (code, message) = error_of(world.start_run(Some(url_key())));
        assert_eq!(code, ErrorCode::NotConfigured);
        assert_eq!(message, "Choose a notes folder in Brauser settings first.");
    }

    #[test]
    fn agent_env_file_reaches_the_harness() {
        let world = World::new("echo \"{\\\"foo\\\": \\\"$FOO\\\"}\"\n");
        world.write_agent_env("FOO=bar\n", 0o600);
        world.start(Duration::from_secs(600));
        let run_id = world.started_run_id();
        let fetched = world.wait_finished(&run_id);
        assert_eq!(fetched.run.state, RunState::Succeeded, "{fetched:?}");
        assert!(fetched.stream_text.contains("bar"), "{fetched:?}");
    }

    #[test]
    fn group_readable_agent_env_fails_start_without_values() {
        let world = World::new(ANSWER);
        world.write_agent_env("FOO=secret\n", 0o644);
        world.start(Duration::from_secs(600));
        let (code, message) = error_of(world.start_run(Some(url_key())));
        assert_eq!(code, ErrorCode::InvalidConfig);
        assert!(message.contains("agent.env"), "{message}");
        assert!(!message.contains("secret"), "{message}");
    }

    #[test]
    fn second_instance_reports_already_running() {
        let world = World::new(ANSWER);
        world.start(Duration::from_secs(600));
        let built = Arc::new(AtomicBool::new(false));
        let flag = built.clone();
        let exit = serve(
            &world.paths,
            world.config(),
            &World::options(&world.paths, Duration::from_secs(600)),
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
        let world = World::new(ANSWER);
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
        let world = World::new(ANSWER);
        world.start(Duration::from_secs(600));
        let dir = std::fs::metadata(&world.support).unwrap();
        assert_eq!(dir.permissions().mode() & 0o777, 0o700);
        let socket = std::fs::symlink_metadata(world.socket()).unwrap();
        assert!(socket.file_type().is_socket());
        assert_eq!(socket.permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_regular_file_at_the_socket_path_is_not_removed() {
        let world = World::new(ANSWER);
        std::fs::write(world.socket(), "keep").unwrap();
        let error = serve(
            &world.paths,
            world.config(),
            &World::options(&world.paths, Duration::from_secs(600)),
            || anyhow::bail!("not reached"),
        );
        assert!(error.is_err());
        assert_eq!(std::fs::read_to_string(world.socket()).unwrap(), "keep");
        // The failed start left no pid file behind.
        assert!(!world.paths.pid().exists());
    }
}
