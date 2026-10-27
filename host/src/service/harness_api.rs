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
use std::process::{Child, Command};
use std::sync::{Arc, Mutex, MutexGuard};

use brauser_protocol::HarnessId;
use rustix::process::{Pid, Signal, kill_process_group};
use uuid::Uuid;

use super::claude::ClaudeRunner;
use super::codex::CodexRunner;
use crate::answer::answer_schema;
use crate::harness::search_path;

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

    /// The process group of the most recently spawned child, if any.
    pub fn group(&self) -> Option<i32> {
        self.lock().group
    }

    pub fn was_killed(&self) -> bool {
        self.lock().killed
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

    pub fn from_environment() -> Self {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    /// The real runners, Codex first, each looking its program up on the
    /// service's `PATH` plus `$HOME/.local/bin`. Test builds instead register
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
        let home = get("HOME").map(PathBuf::from);
        let search_path = search_path(get("PATH").as_deref(), home.as_deref());
        Self {
            runners: vec![
                // The installed app-server takes `outputSchema` on
                // `turn/start` (TESTING.md), so Codex is held to the shape.
                Box::new(CodexRunner::new(search_path.clone(), Some(answer_schema()))),
                Box::new(ClaudeRunner::new(search_path)),
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
        // `claude` is found only under $HOME/.local/bin, so a found claude
        // proves the looked-up HOME was searched. It is never run.
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
        // Codex is looked up on the given PATH only, never the test's own.
        let codex = start_error(&registry, HarnessId::Codex, dir.path()).unwrap();
        assert!(
            codex.contains("codex is not on the service's PATH"),
            "{codex}"
        );
        assert_eq!(start_error(&registry, HarnessId::Claude, dir.path()), None);

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
