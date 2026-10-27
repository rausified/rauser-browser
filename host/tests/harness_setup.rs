//! Harness setup end to end: the built host serves real frames and, with the
//! spawn seam set, starts `brauser service` itself (never launchd). The
//! service detects harnesses on its own `PATH`, where the only `codex` and
//! `claude` are fake `#!/bin/sh` scripts, and probes through the real Claude
//! Code runner. Setup shows no dialog; the scripted-dialog directory only
//! proves that. Each test has a short temp `HOME` and a cleared environment,
//! so nothing here touches the user's real config, harness, or service.
//! Without the feature, or off Unix, this file is empty.
#![cfg(all(feature = "scripted-dialogs", unix))]

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use brauser_host::brand::{SCRIPTED_DIALOGS_ENV, SERVICE_IDLE_SECS_ENV, SERVICE_SPAWN_ENV};
use brauser_host::frame::{read_frame, write_request};
use brauser_protocol::{PROTOCOL_VERSION, Request};
use rustix::process::{Pid, Signal, kill_process};
use serde_json::{Value, json};

const HOST: &str = env!("CARGO_BIN_EXE_brauser");
const PROBE_PROMPT: &str = "Reply with the single word OK.";
/// What a passing Claude Code `-p` stream-json probe prints.
const PASSING: &str = r#"printf '%s\n' '{"type":"system","subtype":"init","session_id":"probe-session"}' '{"type":"assistant","message":{"content":[{"type":"text","text":"OK"}]}}' '{"type":"result","subtype":"success","is_error":false,"result":"OK"}'"#;
/// The v5 harness entry and checked record a native setup wrote.
const V5_TABLES: &str = "[agent]\ndefault = \"claude-code\"\n[agent.harnesses.claude-code]\nbinary = \"/usr/local/bin/claude\"\nargs = [\"-p\", \"{prompt}\"]\nenv_allow = [\"HOME\", \"PATH\"]\ntimeout_secs = 120\n\n[harness_record]\nharness_id = \"claude-code\"\nreal_path = \"/usr/local/bin/claude\"\nsize = 1\nmtime_ns = \"1\"\nversion = \"2.1.284\"\nhelp_sha256 = \"00\"\nconfirmed_flags = [\"-p\"]\nprobe_passed_at = \"2026-09-28T00:00:00Z\"\n";

/// A short temp home (the service socket path must fit `sun_path`), fake
/// harnesses, a scripted-dialog directory, a notes folder, and the config
/// path the host resolves under that home.
struct World {
    root: tempfile::TempDir,
    config: PathBuf,
}

impl World {
    /// `claude_body` runs for every call but `--version`.
    fn new(claude_body: &str) -> Self {
        let root = tempfile::Builder::new()
            .prefix("bh")
            .tempdir_in("/tmp")
            .unwrap();
        for dir in ["h", "bin", "log", "dialogs", "notes"] {
            fs::create_dir(root.path().join(dir)).unwrap();
            fs::set_permissions(root.path().join(dir), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let mut world = Self {
            config: PathBuf::new(),
            root,
        };
        world.install("claude", "2.1.284 (Claude Code)", claude_body);
        world.install("codex", "codex-cli 0.144.4", "exit 1");
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

    fn support(&self) -> PathBuf {
        self.config.parent().unwrap().to_path_buf()
    }

    /// A fake `bin/<name>` that answers `--version` and otherwise logs its
    /// argv, stdin, working directory, and environment, then runs `body`.
    fn install(&self, name: &str, version: &str, body: &str) {
        let script = self.path("bin").join(name);
        let text = format!(
            r#"#!/bin/sh
log='{log}/{name}'
if [ "$1" = --version ]; then echo '{version}'; exit 0; fi
printf '%s\n' "$*" > "$log.argv"
/bin/cat > "$log.stdin"
/bin/pwd -P > "$log.cwd"
/usr/bin/env > "$log.env"
{body}
"#,
            log = self.path("log").display(),
        );
        fs::write(&script, text).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn command(&self) -> Command {
        let mut command = Command::new(HOST);
        command
            .env_clear()
            .env("HOME", self.path("h"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env(SCRIPTED_DIALOGS_ENV, self.path("dialogs"))
            .env(SERVICE_IDLE_SECS_ENV, "10")
            .env(SERVICE_SPAWN_ENV, "1");
        command
    }

    fn config_path(&self) -> PathBuf {
        let output = self.command().arg("--config-path").output().unwrap();
        assert!(output.status.success());
        PathBuf::from(String::from_utf8(output.stdout).unwrap().trim())
    }

    /// A config with a home folder chosen in the fixed layout (so loading it
    /// moves nothing) and the v5 harness tables, which load as not set up
    /// and are dropped on the next save.
    fn write_config(&self) {
        let root = fs::canonicalize(self.path("notes")).unwrap();
        let metadata = fs::metadata(&root).unwrap();
        let identity = format!("unix:{:016x}:{:016x}", metadata.dev(), metadata.ino());
        fs::create_dir_all(self.support()).unwrap();
        fs::write(
            &self.config,
            format!(
                "root_picker_confirmed = true\nroot_identity = {:?}\n\n[storage]\nroot = {:?}\nprofile = \"neutral\"\nlog_dir = \"logs\"\npages_dir = \"notes\"\nlater_dir = \"later\"\nartifacts_dir = \"artifacts\"\nsummaries_dir = \"summaries\"\n\n{V5_TABLES}",
                identity,
                root.to_str().unwrap(),
            ),
        )
        .unwrap();
    }

    fn log(&self, file: &str) -> Option<String> {
        fs::read_to_string(self.path("log").join(file)).ok()
    }

    fn shown(&self) -> String {
        fs::read_to_string(self.path("dialogs").join("shown.jsonl")).unwrap_or_default()
    }

    fn serve(&self) -> Host {
        // Refuse to start unless this binary is the scripted build.
        let version = self.command().arg("--version").output().unwrap();
        assert!(String::from_utf8_lossy(&version.stdout).contains("(scripted dialogs)"));
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
            fake_bin: self.path("bin"),
        }
    }
}

impl Drop for World {
    /// Stop the service this world started before its directory goes away.
    fn drop(&mut self) {
        let pid = fs::read_to_string(self.support().join("service.pid"))
            .ok()
            .and_then(|text| text.trim().parse().ok())
            .and_then(Pid::from_raw)
            .filter(|pid| pid.as_raw_nonzero().get() > 1);
        if let Some(pid) = pid {
            let _ = kill_process(pid, Signal::KILL);
        }
    }
}

struct Host {
    child: Child,
    input: Option<ChildStdin>,
    output: ChildStdout,
    next_id: u32,
    fake_bin: PathBuf,
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

    fn revision(&mut self) -> String {
        let reply = self.call(json!({"type": "get_config"}));
        assert_eq!(reply["type"], "config_result", "{reply}");
        reply["revision"].as_str().unwrap().to_owned()
    }

    /// Both harnesses, from the fake directory only; anything else means a
    /// real harness was found.
    fn discover(&mut self) -> Vec<Value> {
        let reply = self.call(json!({"type": "discover_harnesses"}));
        assert_eq!(reply["type"], "harnesses_discovered", "{reply}");
        let found = reply["harnesses"].as_array().unwrap().clone();
        for harness in &found {
            let binary = Path::new(harness["binary"].as_str().unwrap());
            assert!(
                binary.starts_with(&self.fake_bin),
                "a harness outside the fake directory was found: {}",
                binary.display()
            );
        }
        found
    }

    fn set_up(&mut self, revision: &str) -> Value {
        self.call(json!({
            "type": "set_up_harness",
            "expected_revision": revision,
            "harness": "claude",
            "timeout_secs": null,
            "summaries_dir": null,
        }))
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

#[test]
fn service_detects_and_probes_fake_claude() {
    let world = World::new(PASSING);
    let mut host = world.serve();
    let revision = host.revision();
    let status = host.call(json!({"type": "get_config"}));
    assert_eq!(status["agent_status"]["state"], "not_set_up", "{status}");

    let found = host.discover();
    assert_eq!(
        found,
        [
            json!({"harness": "codex", "binary": world.path("bin").join("codex"),
                "version": "0.144.4", "problem": null}),
            json!({"harness": "claude", "binary": world.path("bin").join("claude"),
                "version": "2.1.284", "problem": null}),
        ]
    );
    // Detection runs `--version` only.
    assert_eq!(world.log("claude.argv"), None);
    assert_eq!(world.log("codex.argv"), None);

    let updated = host.set_up(&revision);
    assert_eq!(updated["type"], "config_updated", "{updated}");
    assert_ne!(updated["revision"], json!(revision));
    let agent = &updated["config"]["agent"];
    assert_eq!(agent["harness"], "claude");
    assert_eq!(
        agent["binary"].as_str(),
        world.path("bin").join("claude").to_str()
    );
    assert_eq!(agent["version"], "2.1.284");
    assert_eq!(agent["timeout_secs"], 600);
    assert_eq!(
        updated["config"]["storage"]["summaries_dir"].as_str(),
        Some("summaries")
    );

    // The probe went through the real Claude Code invocation: print mode
    // with full access, the fixed prompt on stdin, in the knowledge base
    // folder (the notes folder here).
    let argv = world.log("claude.argv").unwrap();
    assert!(
        argv.starts_with("-p --output-format stream-json --verbose"),
        "{argv}"
    );
    assert!(argv.contains("--dangerously-skip-permissions"), "{argv}");
    assert_eq!(world.log("claude.stdin").unwrap().trim_end(), PROBE_PROMPT);
    let notes = fs::canonicalize(world.path("notes")).unwrap();
    assert_eq!(
        world.log("claude.cwd").unwrap().trim_end(),
        notes.to_str().unwrap()
    );
    assert_eq!(world.log("codex.argv"), None);

    let status = host.call(json!({"type": "get_config"}));
    assert_eq!(status["agent_status"]["state"], "ready", "{status}");
    assert_eq!(status["agent_status"]["harness_version"], "2.1.284");
    host.close();

    let saved = fs::read_to_string(&world.config).unwrap();
    assert!(saved.contains("[summarizer]"), "{saved}");
    assert!(!saved.contains("[agent"), "{saved}");
    assert!(!saved.contains("harness_record"), "{saved}");
    assert!(world.shown().is_empty(), "{}", world.shown());
}

#[test]
fn probe_runs_with_agent_env_overlay() {
    let world = World::new(PASSING);
    let agent_env = world.support().join("agent.env");
    fs::write(&agent_env, "BRAUSER_PROBE_MARK=from-agent-env\n").unwrap();
    fs::set_permissions(&agent_env, fs::Permissions::from_mode(0o600)).unwrap();
    let mut host = world.serve();
    let revision = host.revision();
    let updated = host.set_up(&revision);
    assert_eq!(updated["type"], "config_updated", "{updated}");
    host.close();

    let env = world.log("claude.env").unwrap();
    let lines: Vec<&str> = env.lines().collect();
    assert!(
        lines.contains(&"BRAUSER_PROBE_MARK=from-agent-env"),
        "{env}"
    );
    // The service's own environment comes through as well.
    let home = format!("HOME={}", world.path("h").display());
    assert!(lines.contains(&home.as_str()), "{env}");
    assert!(world.shown().is_empty(), "{}", world.shown());
}

#[test]
fn failing_probe_is_refused() {
    let world = World::new("echo 'Invalid API key' >&2; exit 3");
    let mut host = world.serve();
    let revision = host.revision();
    let before = fs::read(&world.config).unwrap();
    let reply = host.set_up(&revision);
    assert_eq!(reply["type"], "error", "{reply}");
    assert_eq!(reply["code"], "invalid_config");
    let message = reply["message"].as_str().unwrap();
    assert!(
        message.starts_with("Claude Code failed its test run (")
            && message.contains("Invalid API key")
            && message.ends_with("); summaries stay off"),
        "{message}"
    );
    let status = host.call(json!({"type": "get_config"}));
    assert_eq!(status["agent_status"]["state"], "not_set_up");
    assert_eq!(host.revision(), revision);
    host.close();

    assert_eq!(fs::read(&world.config).unwrap(), before);
    assert!(world.shown().is_empty(), "{}", world.shown());
}
