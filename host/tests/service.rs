//! The native host and the per-user service end to end. The built host
//! serves real frames; with the spawn seam set it starts `brauser service`
//! itself (never launchd), and the only harness the service can run is a
//! fake `#!/bin/sh` script. Each test has its own short temp `HOME`, a
//! cleared environment, and a notes folder, so nothing here touches the
//! user's real configuration, service, or support directory. Without the
//! feature, or off Unix, this file is empty.
#![cfg(all(feature = "scripted-dialogs", unix))]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use brauser_host::brand::{
    FAKE_HARNESS_ENV, RUN_TIMEOUT_SECS_ENV, SCRIPTED_DIALOGS_ENV, SERVICE_IDLE_SECS_ENV,
    SERVICE_SPAWN_ENV,
};
use brauser_host::frame::{read_frame, write_request};
use brauser_protocol::{PROTOCOL_VERSION, Request};
use rustix::process::{Pid, Signal, kill_process, test_kill_process};
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

const HOST: &str = env!("CARGO_BIN_EXE_brauser");
/// A valid §6.4 answer, already in the normalized form `result.json` keeps.
const ANSWER: &str = r#"{"document_type":"report","category":"news","entities":{"technologies":[],"products":[],"people":[],"companies":[]},"description":"A fake summary.","importance":{"level":"medium","reason":"A test page."},"assessment":"Nothing new.","related_notes":[]}"#;
/// Long enough for a slow machine; every wait polls and returns early.
const WAIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(50);
const RUN_FILES: [&str; 5] = [
    "status.json",
    "page.md",
    "stream.txt",
    "result.json",
    "run.log",
];

/// The fake harness. `mode` in the log directory picks what it does:
/// `answer` replies at once, `slow` replies after two seconds, and `hang`
/// starts a grandchild in its process group and waits on it forever.
fn harness_script(log: &Path) -> String {
    format!(
        r#"#!/bin/sh
log='{log}'
echo $$ > "$log/harness.pid"
printf '%s\n' "$1" > "$log/page-arg.txt"
echo 'fake harness diagnostics' >&2
mode=$(/bin/cat "$log/mode")
case "$mode" in
  hang)
    /bin/sleep 60 &
    echo $! > "$log/grandchild.pid"
    wait
    ;;
  slow)
    /bin/cat > "$log/prompt.txt"
    /bin/sleep 2
    ;;
  *)
    /bin/cat > "$log/prompt.txt"
    ;;
esac
printf '%s\n' '{answer}'
"#,
        log = log.display(),
        answer = ANSWER,
    )
}

/// A short temp home (the service socket path must fit `sun_path`), a fake
/// harness, a notes folder, and the config the host resolves under it.
struct World {
    root: tempfile::TempDir,
    config: PathBuf,
    /// The host binary this world runs; a copy when the test changes it.
    host: PathBuf,
    /// Whether hosts may start the service through the spawn seam.
    spawn: bool,
    /// The service's idle exit, in seconds.
    idle_secs: &'static str,
}

impl World {
    fn new() -> Self {
        let root = tempfile::Builder::new()
            .prefix("bs")
            .tempdir_in("/tmp")
            .unwrap();
        for dir in ["h", "bin", "log", "dialogs", "notes"] {
            fs::create_dir(root.path().join(dir)).unwrap();
            fs::set_permissions(root.path().join(dir), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut world = Self {
            root,
            config: PathBuf::new(),
            host: PathBuf::from(HOST),
            spawn: true,
            idle_secs: "2",
        };
        let harness = world.harness();
        fs::write(&harness, harness_script(&world.path("log"))).unwrap();
        fs::set_permissions(&harness, fs::Permissions::from_mode(0o755)).unwrap();
        world.set_mode("answer");
        world.config = world.config_path();
        assert!(
            world.config.starts_with(world.path("h")),
            "the host must resolve its config under the temp HOME, not {}",
            world.config.display()
        );
        world.write_config();
        world
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn harness(&self) -> PathBuf {
        self.path("bin").join("fake-harness")
    }

    fn set_mode(&self, mode: &str) {
        fs::write(self.path("log").join("mode"), mode).unwrap();
    }

    fn support(&self) -> PathBuf {
        self.config.parent().unwrap().to_path_buf()
    }

    fn socket(&self) -> PathBuf {
        self.support().join("service.sock")
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.support().join("runs").join(run_id)
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.host);
        command
            .env_clear()
            .env("HOME", self.path("h"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env(SCRIPTED_DIALOGS_ENV, self.path("dialogs"))
            .env(FAKE_HARNESS_ENV, self.harness())
            .env(SERVICE_IDLE_SECS_ENV, self.idle_secs)
            .env(RUN_TIMEOUT_SECS_ENV, "30");
        if self.spawn {
            command.env(SERVICE_SPAWN_ENV, "1");
        }
        command
    }

    fn config_path(&self) -> PathBuf {
        let output = self.command().arg("--config-path").output().unwrap();
        assert!(output.status.success());
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    }

    /// A config with a notes folder chosen and Codex set up for summaries
    /// (the fake harness answers for it; the binary is never run).
    fn write_config(&self) {
        let root = fs::canonicalize(self.path("notes")).unwrap();
        let metadata = fs::metadata(&root).unwrap();
        let identity = format!("unix:{:016x}:{:016x}", metadata.dev(), metadata.ino());
        fs::create_dir_all(self.config.parent().unwrap()).unwrap();
        fs::write(
            &self.config,
            format!(
                "root_picker_confirmed = true\nroot_identity = {:?}\n\n[storage]\nroot = {:?}\nprofile = \"neutral\"\nlog_dir = \"log\"\npages_dir = \"pages\"\nlater_dir = \"later\"\n\n[summarizer]\nharness = \"codex\"\nbinary = \"/usr/local/bin/codex\"\nversion = \"0.144.4\"\ntimeout_secs = 600\nprobed_at = \"2026-10-27T10:00:00Z\"\n",
                identity,
                root.to_str().unwrap(),
            ),
        )
        .unwrap();
    }

    fn serve(&self) -> Host {
        // Refuse to start unless this binary is the scripted build.
        let version = self.command().arg("--version").output().unwrap();
        assert!(
            String::from_utf8_lossy(&version.stdout).contains("(scripted dialogs)"),
            "{version:?}"
        );
        let mut child = self
            .command()
            .arg("serve")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        Host {
            input: Some(child.stdin.take().unwrap()),
            output: child.stdout.take().unwrap(),
            child,
            next_id: 0,
        }
    }

    /// The pid the running service recorded, if any.
    fn service_pid(&self) -> Option<i32> {
        fs::read_to_string(self.support().join("service.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    fn pid_file(&self, name: &str) -> Option<i32> {
        fs::read_to_string(self.path("log").join(name))
            .ok()?
            .trim()
            .parse()
            .ok()
    }

    /// Run a host for one request and close it.
    fn call_once(&self, request: Value) -> Value {
        let mut host = self.serve();
        let reply = host.call(request);
        host.close();
        reply
    }
}

impl Drop for World {
    /// Stop anything this world started before its directory goes away.
    fn drop(&mut self) {
        for pid in [
            self.service_pid(),
            self.pid_file("harness.pid"),
            self.pid_file("grandchild.pid"),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(pid) = Pid::from_raw(pid).filter(|pid| pid.as_raw_nonzero().get() > 1) {
                let _ = kill_process(pid, Signal::KILL);
            }
        }
    }
}

struct Host {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    next_id: u32,
}

impl Host {
    fn call(&mut self, mut request: Value) -> Value {
        self.next_id += 1;
        let id = format!("r{}", self.next_id);
        request["protocol_version"] = json!(PROTOCOL_VERSION);
        request["request_id"] = json!(id);
        let request: Request = serde_json::from_value(request).unwrap();
        write_request(self.input.as_mut().unwrap(), &request).unwrap();
        let body = read_frame(&mut self.output).unwrap().unwrap();
        let reply: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(reply["request_id"], json!(id), "{reply}");
        reply
    }

    fn start_run(&mut self) -> Value {
        let reply = self.call(start_run_request());
        assert_eq!(reply["type"], "run_started", "{reply}");
        reply["run"].clone()
    }

    fn get_run(&mut self, run_id: &str) -> Value {
        let reply = self.call(json!({"type": "get_run", "run_id": run_id, "since_offset": 0}));
        assert_eq!(reply["type"], "run_fetched", "{reply}");
        reply
    }

    /// Poll until the run is no longer queued or running.
    fn finished(&mut self, run_id: &str) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let fetched = self.get_run(run_id);
            let state = fetched["run"]["state"].as_str().unwrap().to_owned();
            if state != "queued" && state != "running" {
                return fetched;
            }
            assert!(Instant::now() < deadline, "run never finished: {fetched}");
            thread::sleep(POLL);
        }
    }

    fn close(mut self) {
        drop(self.input.take());
        assert!(self.child.wait().unwrap().success());
    }
}

impl Drop for Host {
    /// A test that panics mid-call must not leave a host writing into a
    /// temp home that is being removed.
    fn drop(&mut self) {
        drop(self.input.take());
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn start_run_request() -> Value {
    json!({
        "type": "start_run",
        "url": "https://example.com/article",
        "title": "An article",
        "content": "# An article\n\nSome text.\n",
        "selection": null,
        // The extension sends null; naming the set-up harness is the same.
        "harness": "codex",
        "record_key": null,
        "requested_at": OffsetDateTime::now_utc()
            .replace_nanosecond(0)
            .unwrap()
            .format(&Rfc3339)
            .unwrap(),
    })
}

fn alive(pid: i32) -> bool {
    Pid::from_raw(pid).is_some_and(|pid| test_kill_process(pid).is_ok())
}

fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting: {what}");
        thread::sleep(POLL);
    }
}

#[test]
fn host_spawns_service_and_run_succeeds() {
    let world = World::new();
    assert!(!world.socket().exists());
    let mut host = world.serve();
    let run = host.start_run();
    assert_eq!(run["record_key"]["kind"], "url", "{run}");
    assert_eq!(run["harness"], "codex", "{run}");
    let run_id = run["run_id"].as_str().unwrap().to_owned();
    let fetched = host.finished(&run_id);
    assert_eq!(fetched["run"]["state"], "succeeded", "{fetched}");
    assert_eq!(fetched["stream_text"].as_str().unwrap().trim(), ANSWER);
    host.close();

    assert!(world.socket().exists());
    let service = world.service_pid().expect("the service wrote its pid");
    assert!(alive(service));
    let dir = world.run_dir(&run_id);
    for name in RUN_FILES {
        assert!(dir.join(name).is_file(), "{name} is missing");
    }
    assert!(
        fs::read_to_string(dir.join("page.md"))
            .unwrap()
            .contains("Some text.")
    );
    let result: Value =
        serde_json::from_str(&fs::read_to_string(dir.join("result.json")).unwrap()).unwrap();
    assert_eq!(result, serde_json::from_str::<Value>(ANSWER).unwrap());
    assert!(
        fs::read_to_string(dir.join("run.log"))
            .unwrap()
            .contains("fake harness diagnostics")
    );
    // The harness was handed the run's own page file (perhaps through the
    // `/tmp` symlink, so compare the files, not the spellings).
    let given = fs::read_to_string(world.path("log").join("page-arg.txt")).unwrap();
    assert_eq!(
        fs::canonicalize(given.trim()).unwrap(),
        fs::canonicalize(dir.join("page.md")).unwrap()
    );
}

#[test]
fn run_survives_host_exit() {
    let world = World::new();
    world.set_mode("slow");
    let mut host = world.serve();
    let run_id = host.start_run()["run_id"].as_str().unwrap().to_owned();
    // The browser goes away mid-run.
    host.close();

    let mut later = world.serve();
    let fetched = later.finished(&run_id);
    assert_eq!(fetched["run"]["state"], "succeeded", "{fetched}");
    later.close();
    assert!(world.run_dir(&run_id).join("result.json").is_file());
}

#[test]
fn cancel_kills_fake_group() {
    let world = World::new();
    world.set_mode("hang");
    let mut host = world.serve();
    let run_id = host.start_run()["run_id"].as_str().unwrap().to_owned();
    wait_until("the fake harness started its grandchild", || {
        world.pid_file("grandchild.pid").is_some()
    });
    let harness = world.pid_file("harness.pid").unwrap();
    let grandchild = world.pid_file("grandchild.pid").unwrap();
    assert!(alive(harness) && alive(grandchild));

    let reply = host.call(json!({"type": "cancel_run", "run_id": run_id}));
    assert_eq!(reply["type"], "run_updated", "{reply}");
    let fetched = host.finished(&run_id);
    assert_eq!(fetched["run"]["state"], "canceled", "{fetched}");
    assert_eq!(fetched["run"]["error_kind"], "canceled", "{fetched}");
    host.close();

    // The whole process group went, the grandchild included.
    wait_until("the harness group was killed", || {
        !alive(harness) && !alive(grandchild)
    });
}

#[test]
fn service_exits_when_idle() {
    let world = World::new();
    let reply = world.call_once(json!({"type": "list_runs"}));
    assert_eq!(reply["type"], "runs_listed", "{reply}");
    let service = world.service_pid().expect("the service wrote its pid");
    assert!(alive(service));
    // The idle seam is two seconds.
    wait_until("the idle service exited", || {
        !alive(service) && !world.socket().exists()
    });
    assert!(world.service_pid().is_none());
}

#[test]
fn without_spawn_seam_host_reports_service_unavailable() {
    let mut world = World::new();
    world.spawn = false;
    let reply = world.call_once(json!({"type": "list_runs"}));
    assert_eq!(reply["type"], "error", "{reply}");
    assert_eq!(reply["code"], "service_unavailable", "{reply}");
    assert!(
        reply["message"].as_str().unwrap().contains("not installed"),
        "{reply}"
    );
    let reply = world.call_once(start_run_request());
    assert_eq!(reply["code"], "service_unavailable", "{reply}");
    assert!(!world.socket().exists());
    assert!(world.service_pid().is_none());
    assert!(!world.support().join("runs").exists());
}

#[test]
fn rebuilt_binary_restarts_the_running_service() {
    let mut world = World::new();
    // Touch a private copy: other tests' services run the shared binary,
    // and a newer mtime on it would restart them mid-run.
    let copy = world.path("bin").join("brauser");
    fs::copy(HOST, &copy).unwrap();
    world.host = copy.clone();
    // The old service must still be up when the rebuilt host arrives, so
    // only a restart can replace it.
    world.idle_secs = "30";

    let reply = world.call_once(json!({"type": "list_runs"}));
    assert_eq!(reply["type"], "runs_listed", "{reply}");
    let old = world.service_pid().expect("the service wrote its pid");
    assert!(alive(old));

    // `touch` changes only the times. Opening the binary for writing would
    // invalidate its code signature, and macOS would kill the next exec.
    let before = fs::metadata(&copy).unwrap().mtime();
    let touched = Command::new("/usr/bin/touch")
        .args(["-A", "05"])
        .arg(&copy)
        .status()
        .unwrap();
    assert!(touched.success());
    assert_ne!(fs::metadata(&copy).unwrap().mtime(), before);
    assert!(alive(old));

    let mut host = world.serve();
    let run_id = host.start_run()["run_id"].as_str().unwrap().to_owned();
    let fetched = host.finished(&run_id);
    assert_eq!(fetched["run"]["state"], "succeeded", "{fetched}");
    host.close();

    let new = world
        .service_pid()
        .expect("the restarted service wrote its pid");
    assert_ne!(new, old);
    assert!(alive(new));
    wait_until("the old build exited", || !alive(old));
}

#[test]
fn a_host_older_than_the_binary_never_restarts_the_newer_service() {
    let mut world = World::new();
    let copy = world.path("bin").join("brauser");
    fs::copy(HOST, &copy).unwrap();
    world.host = copy.clone();
    world.idle_secs = "30";

    // This host captures its build before the binary changes.
    let mut old_host = world.serve();
    let reply = old_host.call(json!({"type": "list_runs"}));
    assert_eq!(reply["type"], "runs_listed", "{reply}");
    let old_service = world.service_pid().expect("the service wrote its pid");

    let touched = Command::new("/usr/bin/touch")
        .args(["-A", "05"])
        .arg(&copy)
        .status()
        .unwrap();
    assert!(touched.success());

    // A host started after the change restarts the service on the new build.
    let mut new_host = world.serve();
    let reply = new_host.call(json!({"type": "list_runs"}));
    assert_eq!(reply["type"], "runs_listed", "{reply}");
    let new_service = world
        .service_pid()
        .expect("the restarted service wrote its pid");
    assert_ne!(new_service, old_service);
    wait_until("the old build exited", || !alive(old_service));

    // The outdated host is told to reload and leaves the current service
    // and its runs alone.
    world.set_mode("slow");
    let run_id = new_host.start_run()["run_id"].as_str().unwrap().to_owned();
    for _ in 0..3 {
        let reply = old_host.call(json!({"type": "get_run", "run_id": run_id, "since_offset": 0}));
        assert_eq!(reply["type"], "error", "{reply}");
        assert_eq!(reply["code"], "service_unavailable", "{reply}");
        assert!(
            reply["message"].as_str().unwrap().contains("Reload"),
            "{reply}"
        );
    }
    assert_eq!(world.service_pid(), Some(new_service));
    assert!(alive(new_service));
    let fetched = new_host.finished(&run_id);
    assert_eq!(fetched["run"]["state"], "succeeded", "{fetched}");
    assert_eq!(world.service_pid(), Some(new_service));
    old_host.close();
    new_host.close();
}
