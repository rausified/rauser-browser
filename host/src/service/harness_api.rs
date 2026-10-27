//! The boundary between the run supervisor and an AI harness.
//!
//! A [`HarnessRunner`] starts a [`HarnessSession`] for one run; each
//! [`HarnessSession::turn`] sends a prompt and streams [`HarnessEvent`]s back.
//! Every child process a runner starts goes through [`ProcessSlot::spawn`],
//! so cancel and timeout can kill the whole process group (spec §5.2).

use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, ExitStatus};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use brauser_protocol::HarnessId;
use rustix::process::{Pid, Signal, WaitId, WaitIdOptions, kill_process_group, waitid};
use uuid::Uuid;

use super::claude::ClaudeRunner;
use super::codex::CodexRunner;
use crate::answer::answer_schema;

/// Everything a runner needs to start one run.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub run_id: Uuid,
    pub harness: HarnessId,
    pub title: String,
    /// The harness's working directory: the knowledge base folder.
    pub cwd: PathBuf,
    /// Absolute path to the run's `page.md`.
    pub page_path: PathBuf,
    /// The §6.4 prompt ([`super::prompt::summary_prompt`]).
    pub prompt: String,
    /// The complete child environment, built by the server: the service's
    /// environment overlaid with `agent.env`. Runners clear the inherited
    /// environment and set exactly these.
    pub env: Vec<(OsString, OsString)>,
}

/// The harness's own handle for the conversation, so the user can resume it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionRef {
    CodexThread(String),
    ClaudeSession(String),
}

/// Something a harness produced during a turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HarnessEvent {
    /// Answer text to append to the run's stream.
    Text(String),
    /// The harness reported its thread or session id.
    Session(SessionRef),
    /// Raw diagnostic output for `run.log`.
    Diagnostic(Vec<u8>),
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Answered { final_text: String },
    Failed { reason: String },
}

/// How often [`ProcessSlot::reap`] polls when `waitid` cannot wait.
const REAP_POLL: Duration = Duration::from_millis(10);

/// Called with each new child's process group as soon as it is spawned.
type SpawnHook = Arc<dyn Fn(i32) + Send + Sync>;

#[derive(Default)]
struct SlotState {
    group: Option<i32>,
    killed: bool,
    on_spawn: Option<SpawnHook>,
}

impl std::fmt::Debug for SlotState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlotState")
            .field("group", &self.group)
            .field("killed", &self.killed)
            .field("on_spawn", &self.on_spawn.is_some())
            .finish()
    }
}

/// The process group of a run's current child, shared between the runner
/// (which spawns) and the supervisor (which cancels or times out).
///
/// Spawning and killing take the same lock, so a kill either reaches the
/// child's group or makes every later spawn fail.
#[derive(Debug, Clone, Default)]
pub struct ProcessSlot {
    state: Arc<Mutex<SlotState>>,
}

impl ProcessSlot {
    fn lock(&self) -> MutexGuard<'_, SlotState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Spawn `command` as the leader of a new process group and record that
    /// group, then pass it to the [`ProcessSlot::set_on_spawn`] hook before
    /// returning, so it is known before the child produces anything. After
    /// [`ProcessSlot::kill`] this refuses with [`io::ErrorKind::Interrupted`]
    /// and starts nothing.
    pub fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        let mut state = self.lock();
        if state.killed {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "the run was stopped",
            ));
        }
        let child = command.process_group(0).spawn()?;
        let group = Pid::from_child(&child).as_raw_pid();
        state.group = Some(group);
        let hook = state.on_spawn.clone();
        // The hook runs outside the slot lock, so a kill is never held up
        // behind it.
        drop(state);
        if let Some(hook) = hook {
            hook(group);
        }
        Ok(child)
    }

    /// Set (or, with `None`, clear) the hook [`ProcessSlot::spawn`] calls
    /// with each new process group.
    pub fn set_on_spawn(&self, hook: Option<SpawnHook>) {
        self.lock().on_spawn = hook;
    }

    /// Mark the slot killed and send SIGKILL to the recorded process group.
    pub fn kill(&self) {
        let mut state = self.lock();
        state.killed = true;
        if let Some(pid) = state.group.and_then(Pid::from_raw) {
            let _ = kill_process_group(pid, Signal::KILL);
        }
    }

    /// Reap `child` if it has exited, and then forget its group: once the
    /// leader is reaped its id can be reused, so a later
    /// [`ProcessSlot::kill`] must not signal it. The slot lock is held across
    /// the reap, so a concurrent kill either reaches the group while the
    /// unreaped leader still holds its id or finds it already forgotten.
    pub fn try_reap(&self, child: &mut Child) -> io::Result<Option<ExitStatus>> {
        let mut state = self.lock();
        let status = child.try_wait()?;
        if status.is_some() && state.group == Some(Pid::from_child(child).as_raw_pid()) {
            state.group = None;
        }
        Ok(status)
    }

    /// Wait for `child` to exit, then reap it as [`ProcessSlot::try_reap`]
    /// does. The wait itself holds no lock, so a kill is never held up
    /// behind it.
    pub fn reap(&self, child: &mut Child) -> io::Result<ExitStatus> {
        let pid = Pid::from_child(child);
        loop {
            // Wait without reaping: the leader keeps the group's id until
            // `try_reap` takes it under the lock.
            match waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
            ) {
                Err(rustix::io::Errno::INTR) => continue,
                Ok(_) => {}
                // Already reaped, or waitid is unusable: poll instead.
                Err(_) => thread::sleep(REAP_POLL),
            }
            if let Some(status) = self.try_reap(child)? {
                return Ok(status);
            }
        }
    }

    /// The process group of the most recently spawned child, if any.
    pub fn group(&self) -> Option<i32> {
        self.lock().group
    }

    pub fn was_killed(&self) -> bool {
        self.lock().killed
    }
}

/// Whether `child` has exited, without reaping it, so its process group id
/// stays reserved and the group can still be signalled safely. A child that
/// was already reaped counts as exited.
pub fn has_exited(child: &Child) -> io::Result<bool> {
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOWAIT | WaitIdOptions::NOHANG;
    loop {
        match waitid(WaitId::Pid(Pid::from_child(child)), options) {
            Ok(status) => return Ok(status.is_some()),
            Err(rustix::io::Errno::INTR) => continue,
            Err(rustix::io::Errno::CHILD) => return Ok(true),
            Err(error) => return Err(error.into()),
        }
    }
}

/// Starts sessions for one harness.
pub trait HarnessRunner: Send + Sync {
    fn harness(&self) -> HarnessId;
    fn start(&self, spec: &RunSpec, slot: &ProcessSlot) -> anyhow::Result<Box<dyn HarnessSession>>;

    /// The runner harness setup's probe uses, when it differs from this one:
    /// the same invocation without what shapes a summary answer.
    fn for_probe(&self) -> Option<Box<dyn HarnessRunner>> {
        None
    }
}

/// One run's conversation with a harness.
pub trait HarnessSession: Send {
    /// First call: `spec.prompt`. PR 3 may call once more with the
    /// JSON-correction follow-up.
    fn turn(&mut self, prompt: &str, sink: &mut dyn FnMut(HarnessEvent)) -> TurnOutcome;
    fn close(self: Box<Self>);
}

/// The runners the service can use, in preference order.
#[derive(Default)]
pub struct HarnessRegistry {
    runners: Vec<Box<dyn HarnessRunner>>,
}

impl HarnessRegistry {
    /// The runner for `id`, or the first registered runner when `id` is
    /// `None`.
    pub fn get(&self, id: Option<HarnessId>) -> Option<&dyn HarnessRunner> {
        self.runners
            .iter()
            .find(|runner| id.is_none_or(|id| runner.harness() == id))
            .map(|runner| runner.as_ref())
    }

    /// A registry of exactly `runners`, for tests with in-process harnesses.
    #[cfg(test)]
    pub(crate) fn of(runners: Vec<Box<dyn HarnessRunner>>) -> Self {
        Self { runners }
    }

    pub fn from_environment() -> Self {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// The real runners, Codex first. Each looks its program up on the run
    /// environment's `PATH` plus `$HOME/.local/bin` when a run starts, the
    /// search setup's discovery uses, so agent.env's `PATH` counts. Test builds instead register
    /// a [`CommandRunner`] for both harness ids when `FAKE_HARNESS_ENV` names
    /// an absolute executable, so no test can reach a real harness that way.
    ///
    /// [`CommandRunner`]: super::command_runner::CommandRunner
    pub fn from_lookup(get: impl Fn(&str) -> Option<OsString>) -> Self {
        #[cfg(any(test, feature = "scripted-dialogs"))]
        if let Some(program) = fake_harness(&get) {
            use super::command_runner::CommandRunner;
            let runners = [HarnessId::Codex, HarnessId::Claude]
                .into_iter()
                .map(|harness| {
                    Box::new(CommandRunner {
                        harness,
                        program: program.clone(),
                    }) as Box<dyn HarnessRunner>
                })
                .collect();
            return Self { runners };
        }
        // Only the test fake is looked up; the real runners read nothing
        // from the service's own environment.
        #[cfg(not(any(test, feature = "scripted-dialogs")))]
        let _ = get;
        Self {
            runners: vec![
                // The installed app-server takes `outputSchema` on
                // `turn/start` (TESTING.md), so Codex is held to the shape.
                Box::new(CodexRunner::new(Some(answer_schema()))),
                Box::new(ClaudeRunner),
            ],
        }
    }
}

#[cfg(any(test, feature = "scripted-dialogs"))]
fn fake_harness(get: &impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let program = PathBuf::from(get(crate::brand::FAKE_HARNESS_ENV)?);
    if !program.is_absolute() {
        return None;
    }
    let metadata = std::fs::metadata(&program).ok()?;
    (metadata.is_file() && metadata.permissions().mode() & 0o111 != 0).then_some(program)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use crate::brand::FAKE_HARNESS_ENV;

    fn spec(cwd: &Path) -> RunSpec {
        RunSpec {
            run_id: Uuid::new_v4(),
            harness: HarnessId::Codex,
            title: "Page".into(),
            cwd: cwd.to_path_buf(),
            page_path: cwd.join("page.md"),
            prompt: "prompt".into(),
            env: Vec::new(),
        }
    }

    /// The error a runner's `start` gives, or `None` when it started.
    fn start_error(registry: &HarnessRegistry, id: HarnessId, cwd: &Path) -> Option<String> {
        match registry
            .get(Some(id))
            .unwrap()
            .start(&spec(cwd), &ProcessSlot::default())
        {
            Ok(session) => {
                session.close();
                None
            }
            Err(error) => Some(format!("{error:#}")),
        }
    }

    #[test]
    fn production_registry_serves_codex_and_claude() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("bin");
        std::fs::create_dir(&empty).unwrap();
        // `claude` is found only under the run's $HOME/.local/bin, so a
        // found claude proves that HOME was searched. It is never run.
        let home = dir.path().join("home");
        let local_bin = home.join(".local").join("bin");
        std::fs::create_dir_all(&local_bin).unwrap();
        let claude = local_bin.join("claude");
        std::fs::write(&claude, "#!/bin/sh\nexit 1\n").unwrap();
        std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
        let lookup = |name: &str| match name {
            "PATH" => Some(empty.clone().into_os_string()),
            "HOME" => Some(home.clone().into_os_string()),
            _ => None,
        };
        let registry = HarnessRegistry::from_lookup(lookup);
        assert_eq!(
            registry.get(Some(HarnessId::Codex)).unwrap().harness(),
            HarnessId::Codex
        );
        assert_eq!(
            registry.get(Some(HarnessId::Claude)).unwrap().harness(),
            HarnessId::Claude
        );
        // Codex is preferred when the request names no harness.
        assert_eq!(registry.get(None).unwrap().harness(), HarnessId::Codex);
        // Codex is looked up on the run's PATH only, never the test's own.
        let codex = start_error(&registry, HarnessId::Codex, dir.path()).unwrap();
        assert!(
            codex.contains("codex is not on the service's PATH"),
            "{codex}"
        );
        let mut run = spec(dir.path());
        run.env = vec![
            ("PATH".into(), empty.clone().into_os_string()),
            ("HOME".into(), home.clone().into_os_string()),
        ];
        let claude = registry
            .get(Some(HarnessId::Claude))
            .unwrap()
            .start(&run, &ProcessSlot::default());
        claude.expect("claude found under the run's HOME").close();
        // The service's own HOME is not searched.
        assert!(start_error(&registry, HarnessId::Claude, dir.path()).is_some());

        // A relative or missing fake does not replace the real runners.
        let relative = HarnessRegistry::from_lookup(|name| {
            if name == FAKE_HARNESS_ENV {
                Some(OsString::from("fake-harness"))
            } else {
                lookup(name)
            }
        });
        let codex = start_error(&relative, HarnessId::Codex, dir.path()).unwrap();
        assert!(
            codex.contains("codex is not on the service's PATH"),
            "{codex}"
        );
        let missing = dir.path().join("missing");
        let absent = HarnessRegistry::from_lookup(|name| {
            if name == FAKE_HARNESS_ENV {
                Some(missing.clone().into_os_string())
            } else {
                lookup(name)
            }
        });
        let codex = start_error(&absent, HarnessId::Codex, dir.path()).unwrap();
        assert!(
            codex.contains("codex is not on the service's PATH"),
            "{codex}"
        );
    }

    /// A fake `program` in `dir` that answers `--version` and otherwise
    /// records that it was started, then exits.
    fn marking_fake(dir: &Path, program: &str, version: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let path = dir.join(program);
        let mark = dir.join("started");
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{version}'; exit 0; fi\n\
                 : > '{}'\nexit 1\n",
                mark.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// The service's own PATH and the run environment's PATH (agent.env can
    /// set it) name different fakes. Setup's discovery and the run must
    /// agree on the run environment's one.
    #[test]
    fn runs_start_the_program_setup_detects_on_the_run_path() {
        let dir = tempfile::tempdir().unwrap();
        let process_bin = dir.path().join("process-bin");
        let run_bin = dir.path().join("run-bin");
        for program in ["codex", "claude"] {
            marking_fake(&process_bin, program, "codex-cli 1.0.0");
        }
        let run_codex = marking_fake(&run_bin, "codex", "codex-cli 2.0.0");
        let registry = HarnessRegistry::from_lookup(|name| {
            (name == "PATH").then(|| process_bin.clone().into_os_string())
        });
        let run_env = vec![(OsString::from("PATH"), run_bin.clone().into_os_string())];

        let detected = crate::harness::detect(
            &crate::harness::HarnessEnv::from_run_env(&run_env),
            &run_env,
        );
        assert_eq!(detected[0].binary.as_deref(), Some(run_codex.as_path()));
        assert_eq!(detected[0].version.as_deref(), Some("2.0.0"));

        let mut run = spec(dir.path());
        run.env = run_env.clone();
        let codex = registry.get(Some(HarnessId::Codex)).unwrap();
        if let Ok(session) = codex.start(&run, &ProcessSlot::default()) {
            session.close();
        }
        assert!(
            run_bin.join("started").exists(),
            "the run's codex did not run"
        );
        assert!(
            !process_bin.join("started").exists(),
            "the service's codex ran"
        );

        // `claude` is only on the service's PATH, so setup would not find
        // it, and neither does a run.
        let mut run = spec(dir.path());
        run.env = run_env;
        let claude_run = registry.get(Some(HarnessId::Claude)).unwrap();
        let error = match claude_run.start(&run, &ProcessSlot::default()) {
            Ok(mut session) => {
                session.turn("prompt", &mut |_| {});
                session.close();
                String::new()
            }
            Err(error) => format!("{error:#}"),
        };
        assert!(
            error.contains("claude is not on the service's PATH"),
            "{error}"
        );
        assert!(
            !process_bin.join("started").exists(),
            "the service's claude ran"
        );
    }

    /// A run refuses a program that setup's install check would refuse.
    #[test]
    fn runs_refuse_a_program_another_user_could_replace() {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let codex = marking_fake(&bin, "codex", "codex-cli 1.0.0");
        std::fs::set_permissions(&codex, std::fs::Permissions::from_mode(0o777)).unwrap();
        let registry = HarnessRegistry::from_lookup(|_| None);
        let mut run = spec(dir.path());
        run.env = vec![(OsString::from("PATH"), bin.clone().into_os_string())];
        let error = match registry
            .get(Some(HarnessId::Codex))
            .unwrap()
            .start(&run, &ProcessSlot::default())
        {
            Ok(session) => {
                session.close();
                String::new()
            }
            Err(error) => format!("{error:#}"),
        };
        assert!(error.contains("is writable by other users"), "{error}");
        assert!(!bin.join("started").exists(), "the replaceable codex ran");
    }

    #[test]
    fn registry_serves_fake_for_both_ids() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("fake");
        std::fs::write(&fake, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let registry = HarnessRegistry::from_lookup(|name| {
            (name == FAKE_HARNESS_ENV).then(|| fake.clone().into_os_string())
        });
        assert_eq!(
            registry.get(Some(HarnessId::Codex)).unwrap().harness(),
            HarnessId::Codex
        );
        assert_eq!(
            registry.get(Some(HarnessId::Claude)).unwrap().harness(),
            HarnessId::Claude
        );
        assert!(registry.get(None).is_some());

        // A file that is not executable is ignored, leaving the real
        // runners, which find nothing on an empty PATH.
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o644)).unwrap();
        let registry = HarnessRegistry::from_lookup(|name| {
            (name == FAKE_HARNESS_ENV).then(|| fake.clone().into_os_string())
        });
        let codex = start_error(&registry, HarnessId::Codex, dir.path()).unwrap();
        assert!(
            codex.contains("codex is not on the service's PATH"),
            "{codex}"
        );
    }
}
