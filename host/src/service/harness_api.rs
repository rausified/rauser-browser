//! The boundary between the run supervisor and an AI harness.
//!
//! A [`HarnessRunner`] starts a [`HarnessSession`] for one run; each
//! [`HarnessSession::turn`] sends a prompt and streams [`HarnessEvent`]s back.
//! Every child process a runner starts goes through [`ProcessSlot::spawn`],
//! so cancel and timeout can kill the whole process group (spec §5.2).

use std::ffi::OsString;
use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::{Arc, Mutex, MutexGuard};

use brauser_protocol::HarnessId;
use rustix::process::{Pid, Signal, kill_process_group};
use uuid::Uuid;

/// Everything a runner needs to start one run.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub run_id: Uuid,
    pub harness: HarnessId,
    pub title: String,
    /// The harness's working directory. PR 2: `storage.root`; PR 3 (Task 31):
    /// the knowledge base directory.
    pub cwd: PathBuf,
    /// Absolute path to the run's `page.md`.
    pub page_path: PathBuf,
    /// PR 2: [`draft_prompt`]; PR 3: the §6.4 prompt.
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

#[derive(Debug, Default)]
struct SlotState {
    group: Option<i32>,
    killed: bool,
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
    /// group. After [`ProcessSlot::kill`] this refuses with
    /// [`io::ErrorKind::Interrupted`] and starts nothing.
    pub fn spawn(&self, command: &mut Command) -> io::Result<Child> {
        let mut state = self.lock();
        if state.killed {
            return Err(io::Error::new(
                io::ErrorKind::Interrupted,
                "the run was stopped",
            ));
        }
        let child = command.process_group(0).spawn()?;
        state.group = Some(Pid::from_child(&child).as_raw_pid());
        Ok(child)
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

    /// PR 2 registers no production runner, so `start_run` reports
    /// `not_configured`. Test builds register a [`CommandRunner`] for both
    /// harness ids when `FAKE_HARNESS_ENV` names an absolute executable.
    ///
    /// [`CommandRunner`]: super::command_runner::CommandRunner
    pub fn from_lookup(get: impl Fn(&str) -> Option<OsString>) -> Self {
        #[allow(unused_mut)]
        let mut runners: Vec<Box<dyn HarnessRunner>> = Vec::new();
        #[cfg(any(test, feature = "scripted-dialogs"))]
        if let Some(program) = fake_harness(&get) {
            use super::command_runner::CommandRunner;
            for harness in [HarnessId::Codex, HarnessId::Claude] {
                runners.push(Box::new(CommandRunner {
                    harness,
                    program: program.clone(),
                }));
            }
        }
        #[cfg(not(any(test, feature = "scripted-dialogs")))]
        let _ = get;
        Self { runners }
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

/// A placeholder prompt naming the page file. PR 3 replaces it with the §6.4
/// prompt.
pub fn draft_prompt(page_path: &Path) -> String {
    format!(
        "Read the web page saved as Markdown at {} and summarize it. \
         Reply with a single JSON object.\n",
        page_path.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    use crate::brand::FAKE_HARNESS_ENV;

    #[test]
    fn registry_is_empty_without_fake_env() {
        let registry = HarnessRegistry::from_lookup(|_| None);
        assert!(registry.get(None).is_none());
        assert!(registry.get(Some(HarnessId::Codex)).is_none());
        assert!(registry.get(Some(HarnessId::Claude)).is_none());

        // A relative path or a missing file does not register a fake either.
        let relative = HarnessRegistry::from_lookup(|name| {
            (name == FAKE_HARNESS_ENV).then(|| OsString::from("fake-harness"))
        });
        assert!(relative.get(None).is_none());
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        let absent = HarnessRegistry::from_lookup(|name| {
            (name == FAKE_HARNESS_ENV).then(|| missing.clone().into_os_string())
        });
        assert!(absent.get(None).is_none());
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

        // A file that is not executable is ignored.
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o644)).unwrap();
        let registry = HarnessRegistry::from_lookup(|name| {
            (name == FAKE_HARNESS_ENV).then(|| fake.clone().into_os_string())
        });
        assert!(registry.get(None).is_none());
    }

    #[test]
    fn draft_prompt_names_the_page_file() {
        let prompt = draft_prompt(Path::new("/tmp/runs/abc/page.md"));
        assert!(prompt.contains("/tmp/runs/abc/page.md"), "{prompt}");
    }
}
