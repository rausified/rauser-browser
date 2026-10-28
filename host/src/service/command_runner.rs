//! A runner that executes one command per turn: `program <page_path>` with
//! the prompt on stdin. It backs the test fake harness (`FAKE_HARNESS_ENV`)
//! and the supervisor tests; PR 3 adds the real Codex and Claude runners.
//!
//! Output protocol:
//! - each stdout line is a [`HarnessEvent::Text`], except `session: <id>`,
//!   which is a [`HarnessEvent::Session`];
//! - stderr is passed through as [`HarnessEvent::Diagnostic`];
//! - exit 0 answers with stdout minus the session lines; any other exit fails
//!   with the last stderr line.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

use brauser_protocol::HarnessId;

use super::child::{ChildLine, LineChild};
use super::harness_api::{
    HarnessEvent, HarnessRunner, HarnessSession, ProcessSlot, RunSpec, SessionRef, TurnOutcome,
};

const SESSION_PREFIX: &str = "session: ";
/// How often the turn checks whether the child has exited.
const POLL: Duration = Duration::from_millis(10);
/// How long to wait for the output pipes to close after the child exits.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// How much trailing stderr is kept to find the last line for the failure.
const STDERR_TAIL: usize = 8 * 1024;

/// Runs `program` once per turn. See the module documentation.
#[derive(Debug, Clone)]
pub struct CommandRunner {
    pub harness: HarnessId,
    pub program: PathBuf,
}

impl HarnessRunner for CommandRunner {
    fn harness(&self) -> HarnessId {
        self.harness
    }

    fn start(&self, spec: &RunSpec, slot: &ProcessSlot) -> anyhow::Result<Box<dyn HarnessSession>> {
        Ok(Box::new(CommandSession {
            program: self.program.clone(),
            page_path: spec.page_path.clone(),
            cwd: spec.cwd.clone(),
            env: spec.env.clone(),
            slot: slot.clone(),
        }))
    }
}

struct CommandSession {
    program: PathBuf,
    page_path: PathBuf,
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
    slot: ProcessSlot,
}

impl HarnessSession for CommandSession {
    fn turn(&mut self, prompt: &str, sink: &mut dyn FnMut(HarnessEvent)) -> TurnOutcome {
        let mut command = Command::new(&self.program);
        command
            .arg(&self.page_path)
            .current_dir(&self.cwd)
            .env_clear()
            .envs(self.env.iter().map(|(name, value)| (name, value)));
        let mut child = match LineChild::spawn(&self.slot, command) {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                return failed("the run was stopped before the harness started".to_string());
            }
            Err(error) => {
                return failed(format!("starting {}: {error}", display_name(&self.program)));
            }
        };

        // The prompt goes as is: `write_line` adds the `\n` it may end with.
        // A harness that exits without reading it fails on its exit status.
        let _ = child.write_line(prompt.strip_suffix('\n').unwrap_or(prompt));
        child.close_stdin();

        let mut final_text = String::new();
        let mut stderr_tail = Vec::new();
        let mut exit: Option<ExitStatus> = None;
        let mut drain_deadline = None;
        let mut wait_error = None;
        let mut closed = false;
        loop {
            match child.next(POLL) {
                Some(ChildLine::Stdout(line)) => {
                    let line = line.strip_suffix('\r').unwrap_or(&line);
                    match line.strip_prefix(SESSION_PREFIX) {
                        Some(id) if !id.trim().is_empty() => sink(HarnessEvent::Session(
                            SessionRef::ClaudeSession(id.trim().to_string()),
                        )),
                        _ => {
                            let text = format!("{line}\n");
                            final_text.push_str(&text);
                            sink(HarnessEvent::Text(text));
                        }
                    }
                }
                Some(ChildLine::Stderr(bytes)) => {
                    stderr_tail.extend_from_slice(&bytes);
                    if stderr_tail.len() > STDERR_TAIL {
                        stderr_tail.drain(..stderr_tail.len() - STDERR_TAIL);
                    }
                    sink(HarnessEvent::Diagnostic(bytes));
                }
                Some(ChildLine::Closed) => closed = true,
                None => {}
            }
            if exit.is_none() && wait_error.is_none() {
                match child.try_wait() {
                    Ok(Some(status)) => exit = Some(status),
                    Ok(None) => {}
                    Err(error) => wait_error = Some(error),
                }
                // `try_wait` also ends what the child left running in its
                // group, closing the pipes such children hold.
                if exit.is_some() || wait_error.is_some() {
                    drain_deadline = Some(Instant::now() + DRAIN_GRACE);
                }
            }
            if let Some(deadline) = drain_deadline
                && (closed || Instant::now() >= deadline)
            {
                break;
            }
        }
        if let Some(error) = wait_error {
            let _ = child.wait();
            return failed(format!("waiting for the harness: {error}"));
        }
        let status = exit.expect("the loop ends only after the child exits");
        if status.success() {
            return TurnOutcome::Answered { final_text };
        }
        let mut reason = match status.code() {
            Some(code) => format!("harness exited with status {code}"),
            None => "harness was killed by a signal".to_string(),
        };
        if let Some(line) = last_line(&stderr_tail) {
            reason.push_str(": ");
            reason.push_str(&line);
        }
        failed(reason)
    }

    fn close(self: Box<Self>) {}
}

fn failed(reason: String) -> TurnOutcome {
    TurnOutcome::Failed { reason }
}

fn display_name(program: &Path) -> String {
    program
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| program.display().to_string())
}

/// The last non-blank line of `bytes`, trimmed.
fn last_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    use brauser_protocol::HarnessId;
    use uuid::Uuid;

    use crate::service::harness_api::{
        HarnessEvent, HarnessRunner, ProcessSlot, RunSpec, SessionRef, TurnOutcome,
    };

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn spec(dir: &Path) -> RunSpec {
        let page_path = dir.join("page.md");
        std::fs::write(&page_path, "# Page\n").unwrap();
        RunSpec {
            run_id: Uuid::new_v4(),
            harness: HarnessId::Codex,
            title: "Title".to_string(),
            cwd: dir.to_path_buf(),
            page_path,
            prompt: "the prompt".to_string(),
            output_schema: crate::service::harness_api::OutputSchema::RunnerDefault,
            env: vec![(OsString::from("PATH"), OsString::from("/bin:/usr/bin"))],
        }
    }

    fn run_turn(program: PathBuf, spec: &RunSpec) -> (Vec<HarnessEvent>, TurnOutcome) {
        let runner = CommandRunner {
            harness: HarnessId::Codex,
            program,
        };
        let slot = ProcessSlot::default();
        let mut session = runner.start(spec, &slot).unwrap();
        let mut events = Vec::new();
        let outcome = session.turn(&spec.prompt, &mut |event| events.push(event));
        session.close();
        (events, outcome)
    }

    #[test]
    fn streams_lines_and_answers_with_stdout() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "fake",
            "echo working\necho 'session: abc-123'\necho '{\"ok\":true}'\necho oops >&2\n",
        );
        let (events, outcome) = run_turn(program, &spec(dir.path()));
        let texts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(texts, ["working\n", "{\"ok\":true}\n"]);
        assert!(
            events.contains(&HarnessEvent::Session(SessionRef::ClaudeSession(
                "abc-123".to_string()
            )))
        );
        let diagnostics: Vec<u8> = events
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::Diagnostic(bytes) => Some(bytes.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(diagnostics, b"oops\n");
        assert_eq!(
            outcome,
            TurnOutcome::Answered {
                final_text: "working\n{\"ok\":true}\n".to_string()
            }
        );
    }

    #[test]
    fn prompt_arrives_on_stdin_and_page_path_as_argv() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "fake",
            "printf 'arg=%s\\n' \"$1\"\nprintf 'cwd=%s\\n' \"$(pwd -P)\"\nprintf 'stdin=%s\\n' \"$(cat)\"\nprintf 'home=%s\\n' \"${HOME-unset}\"\n",
        );
        let spec = spec(dir.path());
        let (_, outcome) = run_turn(program, &spec);
        let cwd = dir.path().canonicalize().unwrap();
        let expected = format!(
            "arg={}\ncwd={}\nstdin=the prompt\nhome=unset\n",
            spec.page_path.display(),
            cwd.display()
        );
        assert_eq!(
            outcome,
            TurnOutcome::Answered {
                final_text: expected
            }
        );
    }

    /// A harness that echoes its prompt as it reads it gets a prompt far
    /// larger than the output queue and the pipes hold, and still answers
    /// with all of it instead of stalling until the run is killed.
    #[test]
    fn a_large_prompt_to_an_echoing_harness_is_answered() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "fake", "exec cat\n");
        let mut spec = spec(dir.path());
        spec.prompt = (0..30_000).map(|i| format!("line {i:05}\n")).collect();
        let runner = CommandRunner {
            harness: HarnessId::Codex,
            program,
        };
        let slot = ProcessSlot::default();
        let watchdog = slot.clone();
        let (stop, stopped) = mpsc::channel::<()>();
        thread::spawn(move || {
            if stopped.recv_timeout(Duration::from_secs(5)).is_err() {
                watchdog.kill();
            }
        });
        let mut session = runner.start(&spec, &slot).unwrap();
        let started = Instant::now();
        let outcome = session.turn(&spec.prompt, &mut |_| {});
        let elapsed = started.elapsed();
        let _ = stop.send(());
        session.close();
        assert!(
            elapsed < Duration::from_secs(4),
            "the turn took {elapsed:?}"
        );
        assert_eq!(
            outcome,
            TurnOutcome::Answered {
                final_text: spec.prompt.clone()
            }
        );
    }

    #[test]
    fn nonzero_exit_fails_with_last_stderr_line() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(
            dir.path(),
            "fake",
            "echo partial\necho first >&2\necho 'second problem' >&2\nexit 3\n",
        );
        let (_, outcome) = run_turn(program, &spec(dir.path()));
        assert_eq!(
            outcome,
            TurnOutcome::Failed {
                reason: "harness exited with status 3: second problem".to_string()
            }
        );
    }

    #[test]
    fn kill_ends_the_whole_process_group() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "fake", "sleep 30 &\necho started\nwait\n");
        let spec = spec(dir.path());
        let slot = ProcessSlot::default();
        let (started_tx, started_rx) = mpsc::channel();
        let worker = {
            let slot = slot.clone();
            thread::spawn(move || {
                let runner = CommandRunner {
                    harness: HarnessId::Codex,
                    program,
                };
                let mut session = runner.start(&spec, &slot).unwrap();
                let outcome = session.turn(&spec.prompt, &mut |event| {
                    if event == HarnessEvent::Text("started\n".to_string()) {
                        let _ = started_tx.send(());
                    }
                });
                session.close();
                outcome
            })
        };
        started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let group = slot.group().expect("the slot records the process group");
        assert!(!slot.was_killed());
        slot.kill();
        assert!(slot.was_killed());

        let pid = rustix::process::Pid::from_raw(group).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if rustix::process::test_kill_process_group(pid).is_err() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "process group {group} still alive after kill"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let outcome = worker.join().unwrap();
        assert!(matches!(outcome, TurnOutcome::Failed { .. }), "{outcome:?}");
    }

    #[test]
    fn spawn_after_kill_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let program = script(dir.path(), "fake", "echo should-not-run\n");
        let slot = ProcessSlot::default();
        slot.kill();
        let error = slot.spawn(&mut Command::new(&program)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
        assert_eq!(slot.group(), None);

        let runner = CommandRunner {
            harness: HarnessId::Claude,
            program,
        };
        let spec = spec(dir.path());
        let mut session = runner.start(&spec, &slot).unwrap();
        let mut events = Vec::new();
        let outcome = session.turn(&spec.prompt, &mut |event| events.push(event));
        assert!(events.is_empty(), "{events:?}");
        assert!(matches!(outcome, TurnOutcome::Failed { .. }), "{outcome:?}");
    }

    /// After a turn its child is reaped, so the slot no longer names the
    /// group a later cancel or timeout would kill.
    #[test]
    fn a_finished_turn_leaves_no_group_to_kill() {
        let dir = tempfile::tempdir().unwrap();
        let spec = spec(dir.path());
        for body in ["echo answer\n", "echo bad >&2\nexit 2\n"] {
            let program = script(dir.path(), "fake", body);
            let runner = CommandRunner {
                harness: HarnessId::Claude,
                program,
            };
            let slot = ProcessSlot::default();
            let mut session = runner.start(&spec, &slot).unwrap();
            session.turn(&spec.prompt, &mut |_| {});
            assert_eq!(slot.group(), None, "{body}");
            session.close();
        }
    }
}
