//! The Claude Code runner: one `claude -p` process per turn, reading
//! newline-delimited stream-json from its stdout (spec §6.3). The run is one
//! session in the knowledge base folder with every permission check skipped,
//! the equivalent of Codex's full access. The first turn starts the session
//! under an id Brauser chooses; a later turn resumes the session the
//! `system/init` event reported, so the user can resume it too. TESTING.md
//! (External programs) records the installed version this follows and the
//! event shapes.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

use brauser_protocol::HarnessId;
use serde_json::Value;
use uuid::Uuid;

use super::child::{ChildLine, LineChild};
use super::harness_api::{
    HarnessEvent, HarnessRunner, HarnessSession, ProcessSlot, RunSpec, SessionRef, TurnOutcome,
};
use crate::harness::{Adapter, resolve_for_run};

/// The flags every turn passes, before the session flag.
const PRINT_ARGS: [&str; 6] = [
    "-p",
    "--output-format",
    "stream-json",
    "--verbose",
    "--include-partial-messages",
    "--dangerously-skip-permissions",
];
/// How often the turn checks whether the child has exited.
const POLL: Duration = Duration::from_millis(10);
/// How long to wait for the output pipes to close after the child exits.
const DRAIN_GRACE: Duration = Duration::from_secs(2);
/// How much trailing stderr is kept to find the last line for the failure.
const STDERR_TAIL: usize = 8 * 1024;

/// Runs `claude -p` once per turn. See the module documentation.
/// `claude` is looked up on each run's own environment, as setup does.
#[derive(Debug, Clone, Default)]
pub struct ClaudeRunner;

impl HarnessRunner for ClaudeRunner {
    fn harness(&self) -> HarnessId {
        HarnessId::Claude
    }

    fn start(&self, spec: &RunSpec, slot: &ProcessSlot) -> anyhow::Result<Box<dyn HarnessSession>> {
        let program =
            resolve_for_run(Adapter::ClaudeCode, &spec.env).map_err(anyhow::Error::msg)?;
        Ok(Box::new(ClaudeSession {
            program,
            cwd: spec.cwd.clone(),
            env: spec.env.clone(),
            slot: slot.clone(),
            session_id: Uuid::new_v4(),
            reported: None,
            turns: 0,
        }))
    }
}

/// The first turn's arguments: print mode, streamed JSON with partial
/// messages, no permission checks, and the session id Brauser chose.
pub fn first_turn_args(session_id: &Uuid) -> Vec<String> {
    session_args("--session-id", &session_id.to_string())
}

/// A later turn's arguments: the same flags, resuming `session_id`.
pub fn resume_turn_args(session_id: &str) -> Vec<String> {
    session_args("--resume", session_id)
}

fn session_args(flag: &str, session_id: &str) -> Vec<String> {
    PRINT_ARGS
        .iter()
        .copied()
        .chain([flag, session_id])
        .map(str::to_string)
        .collect()
}

/// The stream-json lines a turn acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudeLine {
    /// `system`/`init`: the session id.
    Session(String),
    /// A `stream_event` `content_block_delta` with a `text_delta`.
    Delta(String),
    /// An `assistant` message's text blocks, joined.
    AssistantText(String),
    /// The final `result`: its text, or the error when `is_error`.
    Result {
        is_error: bool,
        text: String,
    },
    Other,
}

/// Parse one stdout line. Anything that is not a JSON object, or not one of
/// the events above, is [`ClaudeLine::Other`].
pub fn parse_line(line: &str) -> ClaudeLine {
    let Ok(value) = serde_json::from_str::<Value>(line) else {
        return ClaudeLine::Other;
    };
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    match value["type"].as_str() {
        Some("system") if value["subtype"] == "init" => match value["session_id"].as_str() {
            Some(id) if !id.trim().is_empty() => ClaudeLine::Session(id.trim().to_string()),
            _ => ClaudeLine::Other,
        },
        Some("stream_event")
            if value["event"]["type"] == "content_block_delta"
                && value["event"]["delta"]["type"] == "text_delta" =>
        {
            ClaudeLine::Delta(text(&value["event"]["delta"]["text"]))
        }
        Some("assistant") => {
            let blocks = value["message"]["content"].as_array();
            let joined: String = blocks
                .into_iter()
                .flatten()
                .filter(|block| block["type"] == "text")
                .filter_map(|block| block["text"].as_str())
                .collect();
            ClaudeLine::AssistantText(joined)
        }
        Some("result") => {
            // An error result may carry its messages in `errors` instead.
            let is_error = value["is_error"]
                .as_bool()
                .unwrap_or(value["subtype"] != "success");
            let mut text = text(&value["result"]);
            if text.trim().is_empty()
                && let Some(errors) = value["errors"].as_array()
            {
                text = errors
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("; ");
            }
            ClaudeLine::Result { is_error, text }
        }
        _ => ClaudeLine::Other,
    }
}

struct ClaudeSession {
    program: PathBuf,
    cwd: PathBuf,
    env: Vec<(OsString, OsString)>,
    slot: ProcessSlot,
    /// The id the first turn starts the session under.
    session_id: Uuid,
    /// The id `system/init` last reported, which a later turn resumes.
    reported: Option<String>,
    turns: usize,
}

impl ClaudeSession {
    fn args(&self) -> Vec<String> {
        match (self.turns, &self.reported) {
            (0, _) => first_turn_args(&self.session_id),
            (_, Some(id)) => resume_turn_args(id),
            (_, None) => resume_turn_args(&self.session_id.to_string()),
        }
    }
}

impl HarnessSession for ClaudeSession {
    fn turn(&mut self, prompt: &str, sink: &mut dyn FnMut(HarnessEvent)) -> TurnOutcome {
        let mut command = Command::new(&self.program);
        command
            .args(self.args())
            .current_dir(&self.cwd)
            .env_clear()
            .envs(self.env.iter().map(|(name, value)| (name, value)));
        self.turns += 1;
        let mut child = match LineChild::spawn(&self.slot, command) {
            Ok(child) => child,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                return failed("the run was stopped before claude started".to_string());
            }
            Err(error) => return failed(format!("starting {}: {error}", self.program.display())),
        };

        // The prompt goes as is: `write_line` adds the `\n` it may end with.
        // A claude that exits without reading it fails on its exit status.
        let _ = child.write_line(prompt.strip_suffix('\n').unwrap_or(prompt));
        child.close_stdin();

        // A later turn's text is set apart from the earlier answer, once it
        // has text to show.
        let mut separate = self.turns > 1;
        let mut emit = |text: String, sink: &mut dyn FnMut(HarnessEvent)| {
            if std::mem::take(&mut separate) {
                sink(HarnessEvent::Text("\n\n".to_string()));
            }
            sink(HarnessEvent::Text(text));
        };
        let mut streamed = false;
        let mut result: Option<(bool, String)> = None;
        let mut stderr_tail = Vec::new();
        let mut exit: Option<ExitStatus> = None;
        let mut drain_deadline = None;
        let mut wait_error = None;
        let mut closed = false;
        loop {
            match child.next(POLL) {
                Some(ChildLine::Stdout(line)) => match parse_line(&line) {
                    ClaudeLine::Session(id) => {
                        if self.reported.as_deref() != Some(id.as_str()) {
                            sink(HarnessEvent::Session(SessionRef::ClaudeSession(id.clone())));
                            self.reported = Some(id);
                        }
                    }
                    ClaudeLine::Delta(text) => {
                        if !text.is_empty() {
                            streamed = true;
                            emit(text, sink);
                        }
                    }
                    // Partial messages stream the same text as deltas; a
                    // whole message reaches the stream only without them.
                    ClaudeLine::AssistantText(text) => {
                        if !streamed && !text.is_empty() {
                            emit(text, sink);
                        }
                    }
                    ClaudeLine::Result { is_error, text } => result = Some((is_error, text)),
                    // Stdout that is not stream-json is kept for run.log.
                    ClaudeLine::Other => {
                        let line = line.trim();
                        if !line.is_empty() && !line.starts_with('{') {
                            sink(HarnessEvent::Diagnostic(format!("{line}\n").into_bytes()));
                        }
                    }
                },
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
                if exit.is_some() || wait_error.is_some() {
                    // Nothing claude left in the background outlives the
                    // turn; this also closes pipes such children hold.
                    child.kill_group();
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
            return failed(format!("waiting for claude: {error}"));
        }
        let status = exit.expect("the loop ends only after the child exits");
        match result {
            Some((false, final_text)) if status.success() => TurnOutcome::Answered { final_text },
            Some((true, text)) if !text.trim().is_empty() => failed(text.trim().to_string()),
            _ => failed(exit_reason(status, result.is_none(), &stderr_tail)),
        }
    }

    fn close(self: Box<Self>) {}
}

/// Why a turn without a usable error result failed: its exit status, or the
/// missing result, then the last stderr line.
fn exit_reason(status: ExitStatus, no_result: bool, stderr: &[u8]) -> String {
    let mut reason = match status.code() {
        Some(0) if no_result => "claude exited without a result".to_string(),
        Some(0) => "claude reported an error".to_string(),
        Some(code) => format!("claude exited with status {code}"),
        None => "claude was killed by a signal".to_string(),
    };
    if let Some(line) = last_line(stderr) {
        reason.push_str(": ");
        reason.push_str(&line);
    }
    reason
}

fn failed(reason: String) -> TurnOutcome {
    TurnOutcome::Failed { reason }
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
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;
    use std::thread;

    use serde_json::json;
    use tempfile::TempDir;

    /// A fake `claude` in its own `bin` directory, plus a knowledge base
    /// folder for the run's cwd. Each run appends its argv to `argv.log`,
    /// writes its stdin to `stdin.<n>`, and logs its cwd and environment
    /// variable names, then runs `turn` with the run's number in `$n`.
    struct Fake {
        dir: TempDir,
    }

    impl Fake {
        fn new(turn: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("bin");
            std::fs::create_dir(&bin).unwrap();
            std::fs::create_dir(dir.path().join("kb")).unwrap();
            let log = dir.path().display();
            let script = format!(
                r#"#!/bin/sh
LOG='{log}'
n=$(( $(/bin/cat "$LOG/count" 2>/dev/null || echo 0) + 1 ))
echo "$n" > "$LOG/count"
printf '%s\n' "$*" >> "$LOG/argv.log"
/bin/cat > "$LOG/stdin.$n"
pwd -P > "$LOG/cwd.log"
/usr/bin/env | /usr/bin/cut -d= -f1 > "$LOG/env.log"
{turn}
"#
            );
            let path = bin.join("claude");
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        fn runner(&self) -> ClaudeRunner {
            ClaudeRunner
        }

        /// The run's `PATH`: the fake first, then the system tools.
        fn search_path(&self) -> OsString {
            let mut path = self.dir.path().join("bin").into_os_string();
            path.push(":/usr/bin:/bin");
            path
        }

        fn kb(&self) -> PathBuf {
            self.dir.path().join("kb")
        }

        fn spec(&self) -> RunSpec {
            RunSpec {
                run_id: Uuid::new_v4(),
                harness: HarnessId::Claude,
                title: "Title".to_string(),
                cwd: self.kb(),
                page_path: self.dir.path().join("page.md"),
                prompt: "the prompt".to_string(),
                env: vec![
                    (OsString::from("PATH"), self.search_path()),
                    (OsString::from("HOME"), OsString::from("/users/someone")),
                    (
                        OsString::from("CLAUDE_CODE_USE_BEDROCK"),
                        OsString::from("1"),
                    ),
                ],
            }
        }

        fn log(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
        }

        fn argv(&self) -> Vec<Vec<String>> {
            self.log("argv.log")
                .lines()
                .map(|line| line.split(' ').map(str::to_string).collect())
                .collect()
        }
    }

    /// Shell lines that print each of `lines`.
    fn emit(lines: &[Value]) -> String {
        lines
            .iter()
            .map(|line| format!("printf '%s\\n' '{line}'"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    fn init(id: &str) -> Value {
        json!({"type": "system", "subtype": "init", "cwd": "/kb", "session_id": id,
               "tools": ["Bash"], "model": "m"})
    }

    fn delta(text: &str) -> Value {
        json!({"type": "stream_event", "session_id": "s",
               "event": {"type": "content_block_delta", "index": 0,
                         "delta": {"type": "text_delta", "text": text}}})
    }

    fn assistant(text: &str) -> Value {
        json!({"type": "assistant", "session_id": "s",
        "message": {"role": "assistant", "content": [
            {"type": "text", "text": text},
            {"type": "tool_use", "id": "t", "name": "Bash", "input": {}},
        ]}})
    }

    fn result(is_error: bool, text: &str) -> Value {
        json!({"type": "result", "subtype": if is_error { "error_during_execution" } else { "success" },
               "is_error": is_error, "result": text, "session_id": "s"})
    }

    /// Start a session and run `prompts` as turns, collecting each turn's
    /// events and outcome, then close it. Fails the test if it hangs.
    fn run(
        runner: ClaudeRunner,
        spec: RunSpec,
        prompts: &[&str],
    ) -> Vec<(Vec<HarnessEvent>, TurnOutcome)> {
        let prompts: Vec<String> = prompts.iter().map(|p| p.to_string()).collect();
        let (done, result) = mpsc::channel();
        thread::spawn(move || {
            let slot = ProcessSlot::default();
            let mut session = runner.start(&spec, &slot).unwrap();
            let mut turns = Vec::new();
            for prompt in &prompts {
                let mut events = Vec::new();
                let outcome = session.turn(prompt, &mut |event| events.push(event));
                turns.push((events, outcome));
            }
            session.close();
            let _ = done.send(turns);
        });
        result
            .recv_timeout(Duration::from_secs(20))
            .expect("the session finished")
    }

    fn texts(events: &[HarnessEvent]) -> Vec<&str> {
        events
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn sessions(events: &[HarnessEvent]) -> Vec<&SessionRef> {
        events
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::Session(session) => Some(session),
                _ => None,
            })
            .collect()
    }

    fn answered(text: &str) -> TurnOutcome {
        TurnOutcome::Answered {
            final_text: text.to_string(),
        }
    }

    fn failure(reason: &str) -> TurnOutcome {
        TurnOutcome::Failed {
            reason: reason.to_string(),
        }
    }

    #[test]
    fn parses_init_delta_assistant_and_result_lines() {
        assert_eq!(
            parse_line(&init("sess-1").to_string()),
            ClaudeLine::Session("sess-1".to_string())
        );
        assert_eq!(
            parse_line(&delta("Hel").to_string()),
            ClaudeLine::Delta("Hel".to_string())
        );
        assert_eq!(
            parse_line(&assistant("Hello").to_string()),
            ClaudeLine::AssistantText("Hello".to_string())
        );
        assert_eq!(
            parse_line(&result(false, "{\"ok\":true}").to_string()),
            ClaudeLine::Result {
                is_error: false,
                text: "{\"ok\":true}".to_string()
            }
        );
        assert_eq!(
            parse_line(&result(true, "Credit balance is too low").to_string()),
            ClaudeLine::Result {
                is_error: true,
                text: "Credit balance is too low".to_string()
            }
        );
        // An error result with no `result` text reports its `errors`.
        let errors = json!({"type": "result", "subtype": "error_during_execution",
                            "is_error": true, "errors": ["first", "second"]});
        assert_eq!(
            parse_line(&errors.to_string()),
            ClaudeLine::Result {
                is_error: true,
                text: "first; second".to_string()
            }
        );

        // What a turn does not act on.
        let others = [
            json!({"type": "system", "subtype": "api_retry", "session_id": "x"}),
            json!({"type": "system", "subtype": "init", "session_id": ""}),
            json!({"type": "stream_event", "event": {"type": "message_start"}}),
            json!({"type": "stream_event", "event": {"type": "content_block_delta",
                   "delta": {"type": "input_json_delta", "partial_json": "{"}}}),
            json!({"type": "user", "message": {"content": []}}),
        ];
        for other in others {
            assert_eq!(parse_line(&other.to_string()), ClaudeLine::Other, "{other}");
        }
        assert_eq!(parse_line("not json"), ClaudeLine::Other);
        assert_eq!(parse_line(""), ClaudeLine::Other);
    }

    #[test]
    fn argv_is_full_access_print_mode_and_prompt_is_on_stdin() {
        let fake = Fake::new(&emit(&[
            init("sess-1"),
            delta("{\"ok\":"),
            delta("true}"),
            assistant("{\"ok\":true}"),
            result(false, "{\"ok\":true}"),
        ]));
        let turns = run(fake.runner(), fake.spec(), &["the prompt\nsecond line\n"]);

        let argv = fake.argv();
        assert_eq!(argv.len(), 1);
        let (flags, session) = argv[0].split_at(6);
        assert_eq!(
            flags,
            [
                "-p",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--dangerously-skip-permissions",
            ]
        );
        assert_eq!(session[0], "--session-id");
        let assigned = Uuid::parse_str(&session[1]).expect("a pre-assigned UUID");
        assert_eq!(argv[0], first_turn_args(&assigned));
        assert!(!argv[0].iter().any(|arg| arg == "--no-session-persistence"));

        assert_eq!(fake.log("stdin.1"), "the prompt\nsecond line\n");
        let kb = fake.kb().canonicalize().unwrap();
        assert_eq!(fake.log("cwd.log").trim(), kb.display().to_string());
        let mut names: Vec<_> = fake
            .log("env.log")
            .lines()
            // What the shell itself sets.
            .filter(|name| !matches!(*name, "PWD" | "OLDPWD" | "SHLVL" | "_"))
            .map(str::to_string)
            .collect();
        names.sort();
        assert_eq!(names, ["CLAUDE_CODE_USE_BEDROCK", "HOME", "PATH"]);

        let (events, outcome) = &turns[0];
        assert_eq!(
            sessions(events),
            [&SessionRef::ClaudeSession("sess-1".to_string())]
        );
        assert_eq!(outcome, &answered("{\"ok\":true}"));
        assert_eq!(fake.runner().harness(), HarnessId::Claude);

        // Without `claude` on the search path the run does not start.
        let empty = tempfile::tempdir().unwrap();
        let mut spec = fake.spec();
        spec.env[0].1 = empty.path().as_os_str().to_owned();
        let error = ClaudeRunner
            .start(&spec, &ProcessSlot::default())
            .err()
            .expect("no claude to start");
        assert!(error.to_string().contains("claude"), "{error:#}");
    }

    #[test]
    fn is_error_result_fails_even_with_exit_zero() {
        let fake = Fake::new(&format!(
            "{}; exit 0",
            emit(&[init("sess-1"), result(true, "Credit balance is too low")])
        ));
        let turns = run(fake.runner(), fake.spec(), &["the prompt"]);
        assert_eq!(turns[0].1, failure("Credit balance is too low"));

        // Exit 0 with no result line at all is a failure too.
        let fake = Fake::new(&format!(
            "{}; echo 'claude: stream ended' >&2; exit 0",
            emit(&[init("sess-1"), delta("half")])
        ));
        let turns = run(fake.runner(), fake.spec(), &["the prompt"]);
        assert_eq!(texts(&turns[0].0), ["half"]);
        assert_eq!(
            turns[0].1,
            failure("claude exited without a result: claude: stream ended")
        );
    }

    #[test]
    fn nonzero_exit_fails_with_last_stderr_line() {
        let fake = Fake::new(&format!(
            "{}; echo first >&2; echo 'Invalid API key' >&2; exit 3",
            emit(&[init("sess-1"), result(false, "{\"ok\":true}")])
        ));
        let turns = run(fake.runner(), fake.spec(), &["the prompt"]);
        let (events, outcome) = &turns[0];
        assert_eq!(
            outcome,
            &failure("claude exited with status 3: Invalid API key")
        );
        assert!(events.iter().any(|event| matches!(
            event, HarnessEvent::Diagnostic(bytes) if bytes.starts_with(b"first")
        )));

        // A nonzero exit with an error result reports the result text.
        let fake = Fake::new(&format!(
            "{}; echo 'ignored' >&2; exit 1",
            emit(&[result(true, "Prompt is too long")])
        ));
        let turns = run(fake.runner(), fake.spec(), &["the prompt"]);
        assert_eq!(turns[0].1, failure("Prompt is too long"));

        // Stdout that is not stream-json is kept as a diagnostic.
        let fake = Fake::new("echo 'Error: unknown option'; exit 1");
        let turns = run(fake.runner(), fake.spec(), &["the prompt"]);
        assert_eq!(turns[0].1, failure("claude exited with status 1"));
        assert_eq!(
            turns[0].0,
            [HarnessEvent::Diagnostic(
                b"Error: unknown option\n".to_vec()
            )]
        );
    }

    #[test]
    fn deltas_suppress_duplicate_assistant_text() {
        let fake = Fake::new(&emit(&[
            init("sess-1"),
            delta("Reading the page. "),
            assistant("Reading the page. "),
            delta("{\"title\":"),
            delta("\"T\"}"),
            assistant("{\"title\":\"T\"}"),
            result(false, "{\"title\":\"T\"}"),
        ]));
        let turns = run(fake.runner(), fake.spec(), &["the prompt"]);
        assert_eq!(
            texts(&turns[0].0),
            ["Reading the page. ", "{\"title\":", "\"T\"}"]
        );
        assert_eq!(turns[0].1, answered("{\"title\":\"T\"}"));

        // Without partial messages each whole assistant message streams.
        let fake = Fake::new(&emit(&[
            init("sess-1"),
            assistant("Reading. "),
            assistant("{}"),
            result(false, "{}"),
        ]));
        let turns = run(fake.runner(), fake.spec(), &["the prompt"]);
        assert_eq!(texts(&turns[0].0), ["Reading. ", "{}"]);
        assert_eq!(turns[0].1, answered("{}"));
    }

    #[test]
    fn second_turn_resumes_the_init_session() {
        let fake = Fake::new(&format!(
            r#"if [ "$n" = 1 ]; then {}; else {}; fi"#,
            emit(&[
                init("sess-from-init"),
                delta("not json"),
                result(false, "not json"),
            ]),
            emit(&[init("sess-from-init"), delta("{}"), result(false, "{}")]),
        ));
        let turns = run(fake.runner(), fake.spec(), &["the prompt", "fix it"]);
        assert_eq!(turns[0].1, answered("not json"));
        assert_eq!(texts(&turns[1].0), ["\n\n", "{}"]);
        assert_eq!(turns[1].1, answered("{}"));
        // The session id is reported once, on the first turn.
        assert_eq!(
            sessions(&turns[0].0),
            [&SessionRef::ClaudeSession("sess-from-init".to_string())]
        );
        assert!(sessions(&turns[1].0).is_empty());

        let argv = fake.argv();
        assert_eq!(argv.len(), 2);
        assert_eq!(argv[0][6], "--session-id");
        assert_eq!(argv[1], resume_turn_args("sess-from-init"));
        assert_eq!(&argv[1][..6], &argv[0][..6]);
        assert_eq!(fake.log("stdin.2"), "fix it\n");
    }
}
