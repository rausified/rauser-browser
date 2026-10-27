//! The native host's link to the per-user service: connect, start the
//! service when it is not running (through launchd on macOS), check that it
//! is this build, then forward one request.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use brauser_protocol::{ErrorCode, PROTOCOL_VERSION, Request, Response, ServiceHelloRequest};

use super::paths::{ServicePaths, build_id};
#[cfg(target_os = "macos")]
use crate::brand::SERVICE_LABEL;
use crate::config::ConfigStore;
use crate::frame::{FrameError, read_frame, write_request};

pub const NOT_INSTALLED: &str = "The Brauser service is not installed. Reinstall Brauser, or for a development build run scripts/register-dev-host.mjs.";
pub const WRONG_BUILD: &str = "The Brauser service is running a different build of Brauser. Reinstall Brauser, or for a development build run scripts/register-dev-host.mjs again.";
const NOT_STARTED: &str = "The Brauser service did not start in time. Try again in a moment.";
/// Off macOS there is no launchd job, so nothing starts the service for the
/// host; a service the user started with `brauser service` is still used.
#[cfg(not(target_os = "macos"))]
pub const NO_AUTOSTART: &str =
    "starting it automatically is only supported on macOS; run `brauser service` and try again";
const NO_ANSWER: &str = "The Brauser service stopped before answering. Try again in a moment.";
/// This host is older than the binary on disk, so the running service may
/// well be the current build; restarting it would kill a newer host's runs.
pub const HOST_OUTDATED: &str =
    "Brauser was updated while this window was open. Reload the Brauser extension, then try again.";

/// How long to wait for a started service to accept connections.
pub const DEFAULT_WAIT: Duration = Duration::from_secs(5);
const RETRY: Duration = Duration::from_millis(100);
/// A service that is exiting when it is started can make launchd report the
/// job as running; one more start after this long brings up a fresh one.
const RELAUNCH_AFTER: Duration = Duration::from_secs(1);
/// Run requests are answered from records on disk; anything slower than
/// this means the service is stuck, and the panel can retry.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(30);
/// Harness setup waits out detection and the probe; the settings page
/// gives up at two minutes.
const SETUP_EXCHANGE_TIMEOUT: Duration = Duration::from_secs(115);

/// What asking launchd (or a test seam) to start the service did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    Started,
    NotInstalled,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    /// The service is missing, would not start, or is the wrong build. The
    /// message is for the user; the code is always `service_unavailable`.
    Unavailable(String),
    /// The service answered something this host cannot use.
    Protocol(String),
}

/// Starts the service; the argument asks to restart a running one. A boxed
/// closure rather than a `fn` so the test seam can carry the support paths
/// and unit tests can record their calls.
pub type LaunchFn = Box<dyn Fn(bool) -> Launch + Send + Sync>;

pub struct ServiceLink {
    pub socket: PathBuf,
    pub launch: LaunchFn,
    pub wait: Duration,
    /// This host's build, fixed when the host started; a service reporting
    /// another one is restarted, but only while this is still the build on
    /// disk.
    pub build_id: String,
    /// The build on disk now. A host whose `build_id` differs is the stale
    /// party and never restarts the service.
    pub disk_build: fn() -> String,
    /// Held while deciding on and making a restart, so that of several hosts
    /// finding the same stale service only one restarts it.
    pub restart_lock: PathBuf,
}

impl ServiceLink {
    pub fn for_config(config: &ConfigStore) -> Result<Self> {
        let paths = ServicePaths::for_config(config)?;
        Ok(Self {
            socket: paths.socket()?,
            launch: default_launch(&paths),
            wait: DEFAULT_WAIT,
            build_id: build_id(),
            disk_build: build_id,
            restart_lock: paths.restart_lock(),
        })
    }

    /// Forward one request. The service is started when nothing listens,
    /// and restarted once when it is another build or protocol version and
    /// this host is the build on disk. A `service_hello` that finds the
    /// connection closed (a service exiting at its idle deadline) is tried
    /// once more on a fresh connection; hello is idempotent.
    ///
    /// A restart is decided under [`ServiceLink::restart_lock`], with the
    /// build checked again once the lock is held, so a host that waited for
    /// another host's restart finds the fresh service and leaves it running.
    pub fn request(&self, request: &Request) -> Result<Response, ServiceError> {
        let mut launched = false;
        let mut restarted = false;
        let mut reconnected = false;
        let mut restart_lock = None;
        loop {
            let mut stream = self.connect(&mut launched)?;
            let current = match self.hello(&mut stream, request.request_id()) {
                Err(ServiceError::Unavailable(message)) if message == NO_ANSWER && !reconnected => {
                    reconnected = true;
                    launched = false;
                    continue;
                }
                result => result?,
            };
            if current {
                drop(restart_lock);
                return exchange(&mut stream, request);
            }
            if restarted {
                return Err(ServiceError::Unavailable(WRONG_BUILD.into()));
            }
            if (self.disk_build)() != self.build_id {
                return Err(ServiceError::Unavailable(HOST_OUTDATED.into()));
            }
            drop(stream);
            if restart_lock.is_none() {
                restart_lock = Some(self.lock_restart()?);
                continue;
            }
            restarted = true;
            // The restart starts the service, so a refused connection while
            // it comes up is waited out rather than launched again.
            launched = true;
            self.start(true)?;
        }
    }

    /// Wait for the exclusive lock on [`ServiceLink::restart_lock`]; it is
    /// released when the returned file is dropped.
    fn lock_restart(&self) -> Result<File, ServiceError> {
        let unavailable = |error: std::io::Error| {
            ServiceError::Unavailable(format!(
                "The Brauser service could not be restarted: locking {}: {error}",
                self.restart_lock.display()
            ))
        };
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&self.restart_lock)
            .map_err(unavailable)?;
        file.lock().map_err(unavailable)?;
        Ok(file)
    }

    fn start(&self, restart: bool) -> Result<(), ServiceError> {
        match (self.launch)(restart) {
            Launch::Started => Ok(()),
            Launch::NotInstalled => Err(ServiceError::Unavailable(NOT_INSTALLED.into())),
            Launch::Failed(reason) => Err(ServiceError::Unavailable(format!(
                "The Brauser service could not be started: {reason}"
            ))),
        }
    }

    /// Connect, starting the service once if nothing listens, then retrying
    /// every 100 ms for `wait`. A start this call made that still has nothing
    /// listening after [`RELAUNCH_AFTER`] is repeated once.
    fn connect(&self, launched: &mut bool) -> Result<UnixStream, ServiceError> {
        let mut deadline = None;
        let mut relaunch_at = None;
        loop {
            match UnixStream::connect(&self.socket) {
                Ok(stream) => return Ok(stream),
                Err(error)
                    if matches!(
                        error.kind(),
                        ErrorKind::NotFound | ErrorKind::ConnectionRefused
                    ) =>
                {
                    if !*launched {
                        *launched = true;
                        self.start(false)?;
                        relaunch_at = Some(Instant::now() + RELAUNCH_AFTER);
                    } else if relaunch_at.is_some_and(|at| Instant::now() >= at) {
                        relaunch_at = None;
                        self.start(false)?;
                    }
                    let deadline = *deadline.get_or_insert_with(|| Instant::now() + self.wait);
                    if Instant::now() >= deadline {
                        return Err(ServiceError::Unavailable(NOT_STARTED.into()));
                    }
                    thread::sleep(RETRY);
                }
                Err(error) => {
                    return Err(ServiceError::Unavailable(format!(
                        "The Brauser service could not be reached: {error}"
                    )));
                }
            }
        }
    }

    /// Whether the service on `stream` is this build at this protocol.
    fn hello(&self, stream: &mut UnixStream, request_id: &str) -> Result<bool, ServiceError> {
        let hello = Request::ServiceHello(ServiceHelloRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: request_id.to_owned(),
        });
        match exchange(stream, &hello)? {
            Response::ServiceReady(ready) => Ok(ready.build_id == self.build_id),
            Response::Error(refused) if refused.code == ErrorCode::UnsupportedProtocolVersion => {
                Ok(false)
            }
            _ => Err(ServiceError::Protocol(
                "unexpected reply to service_hello".into(),
            )),
        }
    }
}

/// Send one request and read its correlated response.
fn exchange(stream: &mut UnixStream, request: &Request) -> Result<Response, ServiceError> {
    let unavailable = |_| ServiceError::Unavailable(NO_ANSWER.into());
    let timeout = match request {
        Request::SetUpHarness(_) => SETUP_EXCHANGE_TIMEOUT,
        _ => EXCHANGE_TIMEOUT,
    };
    stream
        .set_read_timeout(Some(timeout))
        .map_err(unavailable)?;
    stream
        .set_write_timeout(Some(timeout))
        .map_err(unavailable)?;
    write_request(stream, request)
        .map_err(|error| ServiceError::Protocol(format!("sending to the service: {error:#}")))?;
    let body = match read_frame(stream) {
        Ok(Some(body)) => body,
        Ok(None) | Err(FrameError::Io(_)) => {
            return Err(ServiceError::Unavailable(NO_ANSWER.into()));
        }
        Err(FrameError::TooLarge | FrameError::InvalidLength) => {
            return Err(ServiceError::Protocol(
                "the service sent an invalid frame".into(),
            ));
        }
    };
    let response: Response = serde_json::from_slice(&body)
        .map_err(|_| ServiceError::Protocol("the service sent an unreadable response".into()))?;
    if response.request_id() != request.request_id() {
        return Err(ServiceError::Protocol(
            "the service answered a different request".into(),
        ));
    }
    Ok(response)
}

/// Production starts the LaunchAgent on macOS and starts nothing elsewhere
/// ([`NO_AUTOSTART`]). Tests never start anything: unit tests get
/// `NotInstalled`, and a `scripted-dialogs` build spawns `brauser service`
/// itself only when `SERVICE_SPAWN_ENV=1`, on any Unix.
#[cfg(all(target_os = "macos", not(any(test, feature = "scripted-dialogs"))))]
fn default_launch(_paths: &ServicePaths) -> LaunchFn {
    Box::new(launchd_kickstart)
}

#[cfg(all(not(target_os = "macos"), not(any(test, feature = "scripted-dialogs"))))]
fn default_launch(_paths: &ServicePaths) -> LaunchFn {
    Box::new(|_| Launch::Failed(NO_AUTOSTART.into()))
}

#[cfg(any(test, feature = "scripted-dialogs"))]
fn default_launch(paths: &ServicePaths) -> LaunchFn {
    #[cfg(all(feature = "scripted-dialogs", not(test)))]
    if std::env::var_os(crate::brand::SERVICE_SPAWN_ENV).is_some_and(|value| value == "1") {
        let paths = paths.clone();
        return Box::new(move |restart| spawn::launch(&paths, restart));
    }
    let _ = paths;
    Box::new(|_| Launch::NotInstalled)
}

/// `/bin/launchctl kickstart [-k] gui/<uid>/<SERVICE_LABEL>`.
#[cfg(target_os = "macos")]
pub fn launchd_kickstart(restart: bool) -> Launch {
    let target = format!("gui/{}/{SERVICE_LABEL}", rustix::process::getuid().as_raw());
    let mut command = Command::new("/bin/launchctl");
    command.arg("kickstart");
    if restart {
        command.arg("-k");
    }
    command
        .arg(&target)
        .stdin(Stdio::null())
        .stdout(Stdio::null());
    match command.output() {
        Ok(output) => kickstart_outcome(
            output.status.code(),
            &String::from_utf8_lossy(&output.stderr),
        ),
        Err(error) => Launch::Failed(format!("could not run launchctl: {error}")),
    }
}

/// launchctl exits 113 (or says so) when the label is not loaded.
#[cfg(target_os = "macos")]
fn kickstart_outcome(code: Option<i32>, stderr: &str) -> Launch {
    match code {
        Some(0) => Launch::Started,
        Some(113) => Launch::NotInstalled,
        _ if stderr.contains("Could not find service") => Launch::NotInstalled,
        Some(code) => {
            let reason = stderr.trim();
            Launch::Failed(if reason.is_empty() {
                format!("launchctl exited with status {code}")
            } else {
                reason.to_owned()
            })
        }
        None => Launch::Failed("launchctl was stopped by a signal".into()),
    }
}

/// The `scripted-dialogs` stand-in for launchd: run this binary's service
/// subcommand detached, with the host's environment.
#[cfg(all(feature = "scripted-dialogs", not(test)))]
mod spawn {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use rustix::process::{Pid, Signal, kill_process, test_kill_process};

    use super::Launch;
    use crate::service::COMMAND;
    use crate::service::paths::ServicePaths;

    const STOP_WAIT: Duration = Duration::from_secs(2);

    pub fn launch(paths: &ServicePaths, restart: bool) -> Launch {
        if restart {
            stop(paths);
        }
        let exe = match std::env::current_exe() {
            Ok(exe) => exe,
            Err(error) => return Launch::Failed(format!("cannot locate this binary: {error}")),
        };
        match Command::new(exe)
            .arg(COMMAND)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
        {
            Ok(mut child) => {
                // Reap it if it exits while this host is still running.
                thread::spawn(move || {
                    let _ = child.wait();
                });
                Launch::Started
            }
            Err(error) => Launch::Failed(format!("cannot start the service: {error}")),
        }
    }

    /// SIGTERM the recorded service, then wait up to 2 s for it to go.
    fn stop(paths: &ServicePaths) {
        let Ok(text) = std::fs::read_to_string(paths.pid()) else {
            return;
        };
        let Ok(raw) = text.trim().parse::<i32>() else {
            return;
        };
        if raw <= 1 || raw == std::process::id() as i32 {
            return;
        }
        let Some(pid) = Pid::from_raw(raw) else {
            return;
        };
        if kill_process(pid, Signal::TERM).is_err() {
            return;
        }
        let socket = paths.socket().ok();
        let deadline = Instant::now() + STOP_WAIT;
        while Instant::now() < deadline
            && test_kill_process(pid).is_ok()
            && socket.as_ref().is_some_and(|socket| socket.exists())
        {
            thread::sleep(Duration::from_millis(50));
        }
    }
}

/// A fake service for tests: a `UnixListener` thread in a short `/tmp`
/// directory that records every request and answers from a closure.
#[cfg(test)]
pub(crate) mod fake {
    use std::os::unix::net::UnixListener;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};
    use std::thread;

    use brauser_protocol::{
        HarnessId, PROTOCOL_VERSION, RecordKey, Request, Response, RunState, RunView, ServiceReady,
    };

    use crate::frame::{read_frame, write_frame};

    pub type Answer = Arc<dyn Fn(&Request, usize) -> Response + Send + Sync>;

    /// A directory short enough for `sun_path`.
    pub fn short_dir() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("bc")
            .tempdir_in("/tmp")
            .unwrap()
    }

    pub struct FakeService {
        pub requests: Arc<Mutex<Vec<Request>>>,
    }

    impl FakeService {
        /// Listen at `socket` now. `answer` gets each request and the number
        /// of `service_hello` requests seen before it.
        pub fn listen(socket: &Path, answer: Answer) -> Self {
            let listener = UnixListener::bind(socket).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let hellos = Arc::new(Mutex::new(0usize));
            let recorded = Arc::clone(&requests);
            thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { return };
                    let recorded = Arc::clone(&recorded);
                    let hellos = Arc::clone(&hellos);
                    let answer = Arc::clone(&answer);
                    thread::spawn(move || {
                        while let Ok(Some(body)) = read_frame(&mut stream) {
                            let request: Request = serde_json::from_slice(&body).unwrap();
                            let seen = {
                                let mut hellos = hellos.lock().unwrap();
                                let seen = *hellos;
                                if matches!(request, Request::ServiceHello(_)) {
                                    *hellos += 1;
                                }
                                seen
                            };
                            let response = answer(&request, seen);
                            recorded.lock().unwrap().push(request);
                            if write_frame(&mut stream, &response).is_err() {
                                return;
                            }
                        }
                    });
                }
            });
            Self { requests }
        }

        pub fn requests(&self) -> Vec<Request> {
            self.requests.lock().unwrap().clone()
        }
    }

    pub fn ready(request: &Request, build_id: &str) -> Response {
        Response::ServiceReady(ServiceReady {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id().to_owned(),
            build_id: build_id.to_owned(),
        })
    }

    /// Hello reports `build_id`; anything else gets an empty run list.
    pub fn answering(build_id: &'static str) -> Answer {
        Arc::new(move |request, _| match request {
            Request::ServiceHello(_) => ready(request, build_id),
            _ => runs_listed(request),
        })
    }

    pub fn runs_listed(request: &Request) -> Response {
        Response::RunsListed(brauser_protocol::RunsListed {
            protocol_version: PROTOCOL_VERSION,
            request_id: request.request_id().to_owned(),
            runs: Vec::new(),
        })
    }

    pub fn run_view(record_key: RecordKey) -> RunView {
        RunView {
            run_id: "00000000-0000-4000-8000-000000000001".into(),
            state: RunState::Queued,
            harness: HarnessId::Codex,
            record_key,
            written_record_key: None,
            url: "https://example.com/".into(),
            title: "Example".into(),
            cwd: None,
            created_at: "2026-10-26T00:00:00Z".into(),
            started_at: None,
            finished_at: None,
            error: None,
            error_kind: None,
            thread_id: None,
            session_id: None,
            summary_path: None,
            stream_bytes: 0,
            can_retry_save: false,
            answer: None,
        }
    }

    pub fn socket_in(dir: &tempfile::TempDir) -> PathBuf {
        dir.path().join("service.sock")
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use brauser_protocol::{ErrorCode, ListRunsRequest, PROTOCOL_VERSION, Request, Response};

    use super::fake::{self, FakeService};
    use super::*;
    use crate::native::error;

    const BUILD: &str = "this-build";

    fn list_runs() -> Request {
        Request::ListRuns(ListRunsRequest {
            protocol_version: PROTOCOL_VERSION,
            request_id: "list-1".into(),
        })
    }

    /// A link whose launch calls are recorded, then answered by `then`.
    fn link(
        socket: PathBuf,
        then: impl Fn(bool) -> Launch + Send + Sync + 'static,
    ) -> (ServiceLink, Arc<Mutex<Vec<bool>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&calls);
        let link = ServiceLink {
            restart_lock: socket.with_file_name("restart.lock"),
            socket,
            launch: Box::new(move |restart| {
                recorded.lock().unwrap().push(restart);
                then(restart)
            }),
            wait: Duration::from_secs(3),
            build_id: BUILD.into(),
            disk_build: || BUILD.to_owned(),
        };
        (link, calls)
    }

    #[test]
    fn a_running_matching_service_is_used_without_launching() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        let service = FakeService::listen(&socket, fake::answering(BUILD));
        let (link, calls) = link(socket, |_| panic!("must not launch"));
        let response = link.request(&list_runs()).unwrap();
        assert!(matches!(response, Response::RunsListed(_)));
        assert!(calls.lock().unwrap().is_empty());
        let requests = service.requests();
        assert!(matches!(requests[0], Request::ServiceHello(_)));
        assert_eq!(requests[1], list_runs());
    }

    #[test]
    fn not_installed_service_gives_service_unavailable() {
        let dir = fake::short_dir();
        let (link, calls) = link(fake::socket_in(&dir), |_| Launch::NotInstalled);
        assert_eq!(
            link.request(&list_runs()),
            Err(ServiceError::Unavailable(NOT_INSTALLED.into()))
        );
        assert_eq!(*calls.lock().unwrap(), vec![false]);
    }

    #[test]
    fn a_failed_launch_is_service_unavailable_with_its_reason() {
        let dir = fake::short_dir();
        let (link, _) = link(fake::socket_in(&dir), |_| Launch::Failed("boom".into()));
        match link.request(&list_runs()) {
            Err(ServiceError::Unavailable(message)) => assert!(message.contains("boom")),
            other => panic!("expected unavailable, got {other:?}"),
        }
    }

    #[test]
    fn kickstart_then_retry_reaches_a_late_listener() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        let late = socket.clone();
        let service = Arc::new(Mutex::new(None));
        let started = Arc::clone(&service);
        let (link, calls) = link(socket, move |_| {
            let late = late.clone();
            let started = Arc::clone(&started);
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(300));
                *started.lock().unwrap() = Some(FakeService::listen(&late, fake::answering(BUILD)));
            });
            Launch::Started
        });
        let response = link.request(&list_runs()).unwrap();
        assert!(matches!(response, Response::RunsListed(_)));
        assert_eq!(*calls.lock().unwrap(), vec![false]);
    }

    #[test]
    fn a_service_that_never_listens_is_unavailable_after_the_wait() {
        let dir = fake::short_dir();
        let (mut link, calls) = link(fake::socket_in(&dir), |_| Launch::Started);
        link.wait = Duration::from_millis(300);
        assert!(matches!(
            link.request(&list_runs()),
            Err(ServiceError::Unavailable(_))
        ));
        assert_eq!(*calls.lock().unwrap(), vec![false]);
    }

    #[test]
    fn stale_service_build_is_restarted() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        // The service is the old build until it is restarted.
        let fresh = Arc::new(AtomicBool::new(false));
        let serving = Arc::clone(&fresh);
        let service = FakeService::listen(
            &socket,
            Arc::new(move |request, _| match request {
                Request::ServiceHello(_) if !serving.load(Ordering::SeqCst) => {
                    fake::ready(request, "old")
                }
                Request::ServiceHello(_) => fake::ready(request, BUILD),
                _ => fake::runs_listed(request),
            }),
        );
        let (link, calls) = link(socket, move |_| {
            fresh.store(true, Ordering::SeqCst);
            Launch::Started
        });
        assert!(matches!(
            link.request(&list_runs()).unwrap(),
            Response::RunsListed(_)
        ));
        assert_eq!(*calls.lock().unwrap(), vec![true]);
        // Hello, hello again under the restart lock, hello the restarted
        // service, then the request.
        let requests = service.requests();
        assert_eq!(requests.len(), 4);
        assert_eq!(requests[3], list_runs());
    }

    #[test]
    fn hosts_racing_on_a_stale_service_restart_it_once() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        use std::sync::Barrier;

        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        let fresh = Arc::new(AtomicBool::new(false));
        // Both hosts' first hellos are answered "old" before either goes on.
        let barrier = Arc::new(Barrier::new(2));
        let serving = Arc::clone(&fresh);
        FakeService::listen(
            &socket,
            Arc::new(move |request, hellos| match request {
                Request::ServiceHello(_) if serving.load(Ordering::SeqCst) => {
                    fake::ready(request, BUILD)
                }
                Request::ServiceHello(_) => {
                    if hellos < 2 {
                        barrier.wait();
                    }
                    fake::ready(request, "old")
                }
                _ => fake::runs_listed(request),
            }),
        );
        // Each restart records whether it found the fresh service running.
        let restarts = Arc::new(Mutex::new(Vec::new()));
        let hosts: Vec<_> = (0..2)
            .map(|_| {
                let restarts = Arc::clone(&restarts);
                let fresh = Arc::clone(&fresh);
                let (link, _) = link(socket.clone(), move |restart| {
                    assert!(restart);
                    restarts
                        .lock()
                        .unwrap()
                        .push(fresh.swap(true, Ordering::SeqCst));
                    Launch::Started
                });
                thread::spawn(move || link.request(&list_runs()))
            })
            .collect();
        for host in hosts {
            assert!(matches!(host.join().unwrap(), Ok(Response::RunsListed(_))));
        }
        assert_eq!(*restarts.lock().unwrap(), vec![false]);
    }

    #[test]
    fn persistent_build_mismatch_is_service_unavailable() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        let service = FakeService::listen(&socket, fake::answering("old"));
        let (link, calls) = link(socket, |_| Launch::Started);
        assert_eq!(
            link.request(&list_runs()),
            Err(ServiceError::Unavailable(WRONG_BUILD.into()))
        );
        assert_eq!(*calls.lock().unwrap(), vec![true]);
        // The run request itself never reached the wrong build.
        assert!(
            service
                .requests()
                .iter()
                .all(|request| matches!(request, Request::ServiceHello(_)))
        );
    }

    #[test]
    fn a_host_older_than_the_binary_on_disk_never_restarts_the_service() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        // The service runs the newer build on disk; this host predates it.
        let service = FakeService::listen(&socket, fake::answering("newer"));
        let (mut link, calls) = link(socket, |_| panic!("must not restart"));
        link.disk_build = || "newer".to_owned();
        assert_eq!(
            link.request(&list_runs()),
            Err(ServiceError::Unavailable(HOST_OUTDATED.into()))
        );
        assert!(calls.lock().unwrap().is_empty());
        assert!(
            service
                .requests()
                .iter()
                .all(|request| matches!(request, Request::ServiceHello(_)))
        );
    }

    #[test]
    fn a_hello_cut_off_by_an_exiting_service_is_retried_once() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        use crate::frame::{read_frame, write_frame};
        use std::os::unix::net::UnixListener;

        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        let listener = UnixListener::bind(&socket).unwrap();
        thread::spawn(move || {
            // The first connection is accepted and closed unanswered, as by
            // a service that decided to exit just after accepting it.
            let (mut first, _) = listener.accept().unwrap();
            let _ = read_frame(&mut first);
            drop(first);
            let (mut stream, _) = listener.accept().unwrap();
            while let Ok(Some(body)) = read_frame(&mut stream) {
                let request: Request = serde_json::from_slice(&body).unwrap();
                let response = match request {
                    Request::ServiceHello(_) => fake::ready(&request, BUILD),
                    _ => fake::runs_listed(&request),
                };
                write_frame(&mut stream, &response).unwrap();
            }
        });
        let (link, calls) = link(socket, |_| panic!("must not launch"));
        assert!(matches!(
            link.request(&list_runs()).unwrap(),
            Response::RunsListed(_)
        ));
        assert!(calls.lock().unwrap().is_empty());
    }

    #[test]
    fn a_hello_that_never_answers_is_not_retried_forever() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        use crate::frame::read_frame;
        use std::os::unix::net::UnixListener;

        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        let listener = UnixListener::bind(&socket).unwrap();
        let accepted = Arc::new(Mutex::new(0usize));
        let counter = Arc::clone(&accepted);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                *counter.lock().unwrap() += 1;
                let _ = read_frame(&mut stream);
            }
        });
        let (link, _) = link(socket, |_| panic!("must not launch"));
        assert_eq!(
            link.request(&list_runs()),
            Err(ServiceError::Unavailable(NO_ANSWER.into()))
        );
        assert_eq!(*accepted.lock().unwrap(), 2);
    }

    #[test]
    fn a_start_that_finds_the_old_service_still_exiting_is_repeated_once() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        let late = socket.clone();
        let service = Arc::new(Mutex::new(None));
        let started = Arc::clone(&service);
        let (link, calls) = link(socket, move |_| {
            // Only the second start brings a service up.
            if started.lock().unwrap().is_none() {
                *started.lock().unwrap() = Some(None);
                return Launch::Started;
            }
            let late = late.clone();
            *started.lock().unwrap() =
                Some(Some(FakeService::listen(&late, fake::answering(BUILD))));
            Launch::Started
        });
        assert!(matches!(
            link.request(&list_runs()).unwrap(),
            Response::RunsListed(_)
        ));
        assert_eq!(*calls.lock().unwrap(), vec![false, false]);
    }

    #[test]
    fn version_skew_restarts_once() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        let fresh = Arc::new(AtomicBool::new(false));
        let serving = Arc::clone(&fresh);
        FakeService::listen(
            &socket,
            Arc::new(move |request, _| match request {
                Request::ServiceHello(_) if !serving.load(Ordering::SeqCst) => error(
                    request.request_id(),
                    ErrorCode::UnsupportedProtocolVersion,
                    "extension and host protocol versions differ",
                ),
                Request::ServiceHello(_) => fake::ready(request, BUILD),
                _ => fake::runs_listed(request),
            }),
        );
        let (link, calls) = link(socket, move |_| {
            fresh.store(true, Ordering::SeqCst);
            Launch::Started
        });
        assert!(matches!(
            link.request(&list_runs()).unwrap(),
            Response::RunsListed(_)
        ));
        assert_eq!(*calls.lock().unwrap(), vec![true]);
    }

    #[test]
    fn a_restart_that_is_not_installed_is_service_unavailable() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        FakeService::listen(&socket, fake::answering("old"));
        let (link, _) = link(socket, |_| Launch::NotInstalled);
        assert_eq!(
            link.request(&list_runs()),
            Err(ServiceError::Unavailable(NOT_INSTALLED.into()))
        );
    }

    #[test]
    fn an_uncorrelated_reply_is_a_protocol_error() {
        if crate::ipc_test_guard::ipc_denied() {
            return;
        }
        let dir = fake::short_dir();
        let socket = fake::socket_in(&dir);
        FakeService::listen(
            &socket,
            Arc::new(|request, _| match request {
                Request::ServiceHello(_) => fake::ready(request, BUILD),
                _ => fake::runs_listed(&Request::ListRuns(ListRunsRequest {
                    protocol_version: PROTOCOL_VERSION,
                    request_id: "someone-else".into(),
                })),
            }),
        );
        let (link, _) = link(socket, |_| panic!("must not launch"));
        assert!(matches!(
            link.request(&list_runs()),
            Err(ServiceError::Protocol(_))
        ));
    }

    #[test]
    fn tests_never_select_launchd() {
        let dir = fake::short_dir();
        let config = ConfigStore::for_test(dir.path().join("config.toml"));
        let link = ServiceLink::for_config(&config).unwrap();
        assert_eq!(link.socket, dir.path().join("service.sock"));
        assert_eq!(link.wait, Duration::from_secs(5));
        assert_eq!(link.build_id, crate::service::paths::build_id());
        // The test process never sets the spawn seam, so nothing starts.
        assert_eq!((link.launch)(false), Launch::NotInstalled);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn kickstart_output_maps_missing_service_to_not_installed() {
        assert_eq!(kickstart_outcome(Some(113), ""), Launch::NotInstalled);
        assert_eq!(
            kickstart_outcome(Some(1), "Could not find service \"x\" in domain"),
            Launch::NotInstalled
        );
        assert_eq!(kickstart_outcome(Some(0), ""), Launch::Started);
        assert!(
            matches!(kickstart_outcome(Some(5), "denied\n"), Launch::Failed(m) if m == "denied")
        );
        assert!(matches!(kickstart_outcome(None, ""), Launch::Failed(_)));
    }
}
