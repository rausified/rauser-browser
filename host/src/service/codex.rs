//! The Codex runner: one `codex app-server` per run, speaking JSON-RPC 2.0 as
//! newline-delimited JSON on stdio (spec §6.2). The run is one full-access
//! thread in the knowledge base folder, named after the page, so the user can
//! reopen it in Codex. Each turn is a `turn/start` read until
//! `turn/completed`. TESTING.md (External programs) records the installed
//! version this follows and the request and notification shapes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use brauser_protocol::HarnessId;
use serde_json::{Value, json};

use super::child::{ChildLine, LineChild};
use super::harness_api::{
    HarnessEvent, HarnessRunner, HarnessSession, ProcessSlot, RunSpec, SessionRef, TurnOutcome,
};
use crate::brand::{APP_NAME, NAMESPACE};
use crate::harness::{Adapter, resolve_for_run};

/// The `codex` arguments that start the app-server on stdio.
pub const APP_SERVER_ARGS: [&str; 3] = ["app-server", "--listen", "stdio://"];
/// How long the app-server has to answer a request.
pub const RPC_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest thread name sent, in characters.
const MAX_THREAD_NAME: usize = 100;
/// How long `close` waits for the app-server to exit on its own.
const CLOSE_GRACE: Duration = Duration::from_secs(2);
/// How often a wait checks its deadline.
const POLL: Duration = Duration::from_millis(50);
const EXITED: &str = "codex app-server exited before the turn finished";

/// Starts a `codex app-server` thread per run. See the module documentation.
#[derive(Debug, Clone)]
pub struct CodexRunner {
    output_schema: Option<Value>,
}

impl CodexRunner {
    /// `codex` is looked up on each run's own environment, as setup does.
    /// `output_schema`, when set, is sent with every `turn/start` to
    /// constrain the final message.
    pub fn new(output_schema: Option<Value>) -> Self {
        Self { output_schema }
    }
}

impl HarnessRunner for CodexRunner {
    fn harness(&self) -> HarnessId {
        HarnessId::Codex
    }

    /// The probe asks for one word, which the answer schema would forbid.
    fn for_probe(&self) -> Option<Box<dyn HarnessRunner>> {
        Some(Box::new(Self::new(None)))
    }

    fn start(&self, spec: &RunSpec, slot: &ProcessSlot) -> anyhow::Result<Box<dyn HarnessSession>> {
        let program = resolve_for_run(Adapter::Codex, &spec.env).map_err(anyhow::Error::msg)?;
        let mut command = Command::new(&program);
        command
            .args(APP_SERVER_ARGS)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(spec.env.iter().map(|(name, value)| (name, value)));
        let child = LineChild::spawn(slot, command)
            .map_err(|error| anyhow::anyhow!("starting {}: {error}", program.display()))?;
        let mut session = CodexSession {
            child: Some(child),
            slot: slot.clone(),
            next_id: 0,
            thread_id: String::new(),
            cwd: spec.cwd.clone(),
            output_schema: self.output_schema.clone(),
            pending: Vec::new(),
            turns: 0,
            exited: false,
        };
        if let Err(reason) = session.handshake(&spec.title) {
            session.stop();
            anyhow::bail!(reason);
        }
        Ok(Box::new(session))
    }
}

/// `thread/start` params: a full-access thread that never asks for approval.
/// Note the kebab-case `sandbox` here and the camelCase `sandboxPolicy` in
/// [`turn_start_params`]; both are what the app-server expects.
pub fn thread_start_params(cwd: &Path) -> Value {
    json!({
        "cwd": cwd.display().to_string(),
        "approvalPolicy": "never",
        "sandbox": "danger-full-access",
    })
}

/// `turn/start` params for `prompt` on `thread_id`, with `outputSchema` only
/// when `schema` is set.
pub fn turn_start_params(
    thread_id: &str,
    cwd: &Path,
    prompt: &str,
    schema: Option<&Value>,
) -> Value {
    let mut params = json!({
        "threadId": thread_id,
        "cwd": cwd.display().to_string(),
        "approvalPolicy": "never",
        "sandboxPolicy": {"type": "dangerFullAccess"},
        "input": [{"type": "text", "text": prompt}],
    });
    if let Some(schema) = schema {
        params["outputSchema"] = schema.clone();
    }
    params
}

/// "Brauser: <title>" on one line, at most 100 characters, ending in an
/// ellipsis when cut.
pub fn thread_name(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let title = if title.is_empty() {
        "Untitled page".to_string()
    } else {
        title
    };
    let name = format!("{APP_NAME}: {title}");
    if name.chars().count() <= MAX_THREAD_NAME {
        return name;
    }
    let mut cut: String = name.chars().take(MAX_THREAD_NAME - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// The notifications a turn acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Notice {
    /// `item/agentMessage/delta`: more text of an agent message.
    Delta {
        item: String,
        text: String,
    },
    /// `item/completed` for an `agentMessage`: its whole text.
    Completed {
        item: String,
        text: String,
    },
    /// `turn/completed`, with the error message unless it completed.
    TurnDone {
        failed: Option<String>,
    },
    Other,
}

fn parse_notice(value: &Value) -> Notice {
    let params = &value["params"];
    let text = |value: &Value| value.as_str().unwrap_or_default().to_string();
    match value["method"].as_str() {
        Some("item/agentMessage/delta") => Notice::Delta {
            item: text(&params["itemId"]),
            text: text(&params["delta"]),
        },
        Some("item/completed") if params["item"]["type"] == "agentMessage" => Notice::Completed {
            item: text(&params["item"]["id"]),
            text: text(&params["item"]["text"]),
        },
        Some("turn/completed") => {
            let turn = &params["turn"];
            let failed = match turn["status"].as_str() {
                Some("completed") => None,
                Some("interrupted") => Some("The Codex turn was interrupted.".to_string()),
                _ => Some(
                    turn["error"]["message"]
                        .as_str()
                        .filter(|message| !message.trim().is_empty())
                        .unwrap_or("The Codex turn failed.")
                        .to_string(),
                ),
            };
            Notice::TurnDone { failed }
        }
        _ => Notice::Other,
    }
}

/// One message from the app-server.
enum Incoming {
    Response {
        id: u64,
        result: Result<Value, String>,
    },
    Notice(Value),
    Exited,
}

struct CodexSession {
    /// `None` once closed.
    child: Option<LineChild>,
    slot: ProcessSlot,
    next_id: u64,
    thread_id: String,
    cwd: PathBuf,
    output_schema: Option<Value>,
    /// Events from the handshake, given to the first turn's sink.
    pending: Vec<HarnessEvent>,
    turns: usize,
    exited: bool,
}

impl CodexSession {
    /// `initialize`, `thread/start` and `thread/name/set`. Only a failed
    /// name is not fatal.
    fn handshake(&mut self, title: &str) -> Result<(), String> {
        let mut events = Vec::new();
        let result = self.handshake_into(title, &mut |event| events.push(event));
        self.pending.extend(events);
        result
    }

    fn handshake_into(
        &mut self,
        title: &str,
        sink: &mut dyn FnMut(HarnessEvent),
    ) -> Result<(), String> {
        let client = json!({
            "clientInfo": {"name": NAMESPACE, "version": env!("CARGO_PKG_VERSION")},
            "capabilities": {"experimentalApi": true},
        });
        self.call("initialize", client, sink)?;
        let thread = self.call("thread/start", thread_start_params(&self.cwd), sink)?;
        let id = thread["thread"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("codex app-server started a thread without an id")?;
        self.thread_id = id.to_string();
        sink(HarnessEvent::Session(SessionRef::CodexThread(
            self.thread_id.clone(),
        )));
        let name = json!({"threadId": self.thread_id, "name": thread_name(title)});
        if let Err(reason) = self.call("thread/name/set", name, sink) {
            sink(HarnessEvent::Diagnostic(
                format!("brauser: naming the Codex thread failed: {reason}\n").into_bytes(),
            ));
        }
        Ok(())
    }

    /// Send a request and return its id. A broken pipe means the app-server
    /// is gone, whether or not its stdout has closed yet.
    fn send(&mut self, method: &str, params: Value) -> Result<u64, String> {
        self.next_id += 1;
        let id = self.next_id;
        let request = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let child = self.child.as_mut().ok_or(EXITED)?;
        match child.write_line(&request.to_string()) {
            Ok(()) => Ok(id),
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => {
                self.exited = true;
                Err(EXITED.to_string())
            }
            Err(error) => Err(format!("sending {method} to codex app-server: {error}")),
        }
    }

    /// Send a request and wait up to [`RPC_TIMEOUT`] for its result.
    fn call(
        &mut self,
        method: &str,
        params: Value,
        sink: &mut dyn FnMut(HarnessEvent),
    ) -> Result<Value, String> {
        let id = self.send(method, params)?;
        let deadline = Instant::now() + RPC_TIMEOUT;
        loop {
            match self.receive(Some(deadline), sink) {
                Some(Incoming::Response {
                    id: answered,
                    result,
                }) if answered == id => {
                    return result.map_err(|error| format!("{method}: {error}"));
                }
                Some(Incoming::Response { .. } | Incoming::Notice(_)) => {}
                Some(Incoming::Exited) => {
                    return Err(format!("codex app-server exited before answering {method}"));
                }
                None => return Err(no_answer(method)),
            }
        }
    }

    /// The next response or notification, or `None` at `deadline`. Stderr and
    /// stdout that is not JSON-RPC go to `sink` as diagnostics; a request
    /// from the app-server is refused, since full access needs no approvals.
    fn receive(
        &mut self,
        deadline: Option<Instant>,
        sink: &mut dyn FnMut(HarnessEvent),
    ) -> Option<Incoming> {
        if self.exited {
            return Some(Incoming::Exited);
        }
        loop {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return None;
            }
            let line = match self.child.as_ref()?.next(POLL) {
                None => continue,
                Some(ChildLine::Closed) => {
                    self.exited = true;
                    return Some(Incoming::Exited);
                }
                Some(ChildLine::Stderr(bytes)) => {
                    sink(HarnessEvent::Diagnostic(bytes));
                    continue;
                }
                Some(ChildLine::Stdout(line)) => line,
            };
            let message = serde_json::from_str::<Value>(&line)
                .ok()
                .filter(Value::is_object);
            let Some(message) = message else {
                if !line.trim().is_empty() {
                    sink(HarnessEvent::Diagnostic(format!("{line}\n").into_bytes()));
                }
                continue;
            };
            match (message.get("id"), message["method"].as_str()) {
                (Some(id), Some(method)) => self.refuse(id.clone(), method),
                (None, Some(_)) => return Some(Incoming::Notice(message)),
                (Some(id), None) => {
                    let Some(id) = id.as_u64() else { continue };
                    let result = match message.get("error") {
                        Some(error) => Err(error["message"]
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| error.to_string())),
                        None => Ok(message.get("result").cloned().unwrap_or(Value::Null)),
                    };
                    return Some(Incoming::Response { id, result });
                }
                (None, None) => {}
            }
        }
    }

    /// Answer a request from the app-server with "method not found".
    fn refuse(&mut self, id: Value, method: &str) {
        let reply = json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("{APP_NAME} does not handle {method}")},
        });
        if let Some(child) = self.child.as_mut() {
            let _ = child.write_line(&reply.to_string());
        }
    }

    /// Kill the app-server's process group and reap it.
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            child.close_stdin();
            let deadline = Instant::now() + CLOSE_GRACE;
            while Instant::now() < deadline && !child.has_exited() {
                std::thread::sleep(POLL);
            }
            self.slot.kill();
            let _ = child.wait();
        }
    }
}

impl HarnessSession for CodexSession {
    fn turn(&mut self, prompt: &str, sink: &mut dyn FnMut(HarnessEvent)) -> TurnOutcome {
        for event in self.pending.drain(..) {
            sink(event);
        }
        if self.exited {
            return failed(EXITED);
        }
        self.turns += 1;
        // A later turn's text is set apart from the earlier answer, once it
        // has text to show.
        let mut separate = self.turns > 1;
        let mut emit = |text: String, sink: &mut dyn FnMut(HarnessEvent)| {
            if std::mem::take(&mut separate) {
                sink(HarnessEvent::Text("\n\n".to_string()));
            }
            sink(HarnessEvent::Text(text));
        };
        let params = turn_start_params(
            &self.thread_id,
            &self.cwd,
            prompt,
            self.output_schema.as_ref(),
        );
        let id = match self.send("turn/start", params) {
            Ok(id) => id,
            Err(reason) => return failed(&reason),
        };
        // The turn itself has no deadline of its own: the supervisor's
        // timeout and cancel kill the process group, which ends it.
        let mut start_deadline = Some(Instant::now() + RPC_TIMEOUT);
        let mut streamed: HashMap<String, String> = HashMap::new();
        let mut final_text = String::new();
        loop {
            let message = match self.receive(start_deadline, sink) {
                None => return failed(&no_answer("turn/start")),
                Some(Incoming::Exited) => return failed(EXITED),
                Some(Incoming::Response {
                    id: answered,
                    result,
                }) => {
                    if answered == id {
                        if let Err(error) = result {
                            return failed(&format!("codex refused the turn: {error}"));
                        }
                        start_deadline = None;
                    }
                    continue;
                }
                Some(Incoming::Notice(message)) => message,
            };
            match parse_notice(&message) {
                Notice::Delta { item, text } => {
                    if !text.is_empty() {
                        streamed.entry(item).or_default().push_str(&text);
                        emit(text, sink);
                    }
                }
                Notice::Completed { item, text } => {
                    // Stream whatever of the message the deltas did not.
                    let sent = streamed.remove(&item).unwrap_or_default();
                    if let Some(rest) = text.strip_prefix(sent.as_str())
                        && !rest.is_empty()
                    {
                        emit(rest.to_string(), sink);
                    }
                    final_text = text;
                }
                Notice::TurnDone {
                    failed: Some(reason),
                } => return failed(&reason),
                Notice::TurnDone { failed: None } => return TurnOutcome::Answered { final_text },
                Notice::Other => {}
            }
        }
    }

    fn close(mut self: Box<Self>) {
        self.stop();
    }
}

fn failed(reason: &str) -> TurnOutcome {
    TurnOutcome::Failed {
        reason: reason.to_string(),
    }
}

fn no_answer(method: &str) -> String {
    format!(
        "codex app-server did not answer {method} within {} s",
        RPC_TIMEOUT.as_secs()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::thread;

    use brauser_protocol::HarnessId;
    use serde_json::json;
    use tempfile::TempDir;
    use uuid::Uuid;

    use crate::service::harness_api::{HarnessEvent, SessionRef, TurnOutcome};

    const EXITED: &str = "codex app-server exited before the turn finished";

    /// A fake `codex` in its own `bin` directory, plus a knowledge base
    /// folder for the run's cwd. The fake logs its argv, cwd and environment
    /// variable names, and every stdin line to `rpc.log`. It answers ids 1-3
    /// (`initialize`, `thread/start`, `thread/name/set`) in order and runs
    /// `turn` for each later line, with the request id in `$n`.
    struct Fake {
        dir: TempDir,
    }

    impl Fake {
        fn new(name_reply: &str, turn: &str) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("bin");
            std::fs::create_dir(&bin).unwrap();
            std::fs::create_dir(dir.path().join("kb")).unwrap();
            let log = dir.path().display();
            let script = format!(
                r#"#!/bin/sh
LOG='{log}'
printf '%s\n' "$*" > "$LOG/argv.log"
pwd -P > "$LOG/cwd.log"
/usr/bin/env | /usr/bin/cut -d= -f1 > "$LOG/env.log"
n=0
while IFS= read -r line; do
  printf '%s\n' "$line" >> "$LOG/rpc.log"
  n=$((n+1))
  case $n in
    1) printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"userAgent":"fake"}}}}' ;;
    2) printf '%s\n' '{{"jsonrpc":"2.0","method":"thread/started","params":{{"thread":{{"id":"thr-1"}}}}}}'
       printf '%s\n' '{{"jsonrpc":"2.0","id":2,"result":{{"thread":{{"id":"thr-1"}}}}}}' ;;
    3) printf '%s\n' '{name_reply}' ;;
    *) {turn} ;;
  esac
done
"#
            );
            let path = bin.join("codex");
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            Self { dir }
        }

        /// A fake whose `thread/name/set` succeeds and whose turns run `turn`.
        fn turns(turn: &str) -> Self {
            Self::new(r#"{"jsonrpc":"2.0","id":3,"result":{}}"#, turn)
        }

        fn runner(&self, schema: Option<Value>) -> CodexRunner {
            CodexRunner::new(schema)
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
                harness: HarnessId::Codex,
                title: "Title".to_string(),
                cwd: self.kb(),
                page_path: self.dir.path().join("page.md"),
                prompt: "the prompt".to_string(),
                env: vec![
                    (OsString::from("PATH"), self.search_path()),
                    (OsString::from("HOME"), OsString::from("/users/someone")),
                    (
                        OsString::from("CODEX_HOME"),
                        OsString::from("/users/codex-home"),
                    ),
                    (OsString::from("AGENT_EXTRA"), OsString::from("1")),
                ],
            }
        }

        fn log(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
        }

        /// Every request the runner sent, parsed.
        fn requests(&self) -> Vec<Value> {
            self.log("rpc.log")
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect()
        }
    }

    /// Shell lines that answer turn/start `$n`, then send `notices`.
    fn answer_turn(notices: &[Value]) -> String {
        let mut body = String::from(
            r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"turn":{"id":"turn-%s"}}}\n' "$n" "$n""#,
        );
        for notice in notices {
            body.push_str(&format!("; printf '%s\\n' '{notice}'"));
        }
        body
    }

    fn notice(method: &str, params: Value) -> Value {
        json!({"jsonrpc": "2.0", "method": method, "params": params})
    }

    fn delta(item: &str, text: &str) -> Value {
        notice(
            "item/agentMessage/delta",
            json!({"threadId": "thr-1", "turnId": "t", "itemId": item, "delta": text}),
        )
    }

    fn completed_message(item: &str, text: &str) -> Value {
        notice(
            "item/completed",
            json!({"threadId": "thr-1", "turnId": "t",
                   "item": {"type": "agentMessage", "id": item, "text": text}}),
        )
    }

    fn turn_completed(status: &str) -> Value {
        notice(
            "turn/completed",
            json!({"threadId": "thr-1", "turn": {"id": "t", "status": status, "error": null}}),
        )
    }

    /// Start a session and run `prompts` as turns, collecting each turn's
    /// events and outcome, then close it. Fails the test if it hangs.
    fn run(
        runner: CodexRunner,
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

    fn answered(text: &str) -> TurnOutcome {
        TurnOutcome::Answered {
            final_text: text.to_string(),
        }
    }

    fn failed(reason: &str) -> TurnOutcome {
        TurnOutcome::Failed {
            reason: reason.to_string(),
        }
    }

    fn simple_turn() -> String {
        answer_turn(&[
            delta("msg-1", "{\"ok\":"),
            delta("msg-1", "true}"),
            completed_message("msg-1", "{\"ok\":true}"),
            turn_completed("completed"),
        ])
    }

    #[test]
    fn handshake_starts_a_full_access_thread_in_cwd() {
        let fake = Fake::turns(&simple_turn());
        let spec = fake.spec();
        let turns = run(fake.runner(None), spec.clone(), &["the prompt"]);

        assert_eq!(fake.log("argv.log").trim(), "app-server --listen stdio://");
        let kb = fake.kb().canonicalize().unwrap();
        assert_eq!(fake.log("cwd.log").trim(), kb.display().to_string());

        let requests = fake.requests();
        let methods: Vec<_> = requests.iter().map(|r| r["method"].clone()).collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "thread/start",
                "thread/name/set",
                "turn/start"
            ]
        );
        for (index, request) in requests.iter().enumerate() {
            assert_eq!(request["jsonrpc"], "2.0");
            assert_eq!(request["id"], index as u64 + 1);
        }
        assert_eq!(
            requests[0]["params"],
            json!({
                "clientInfo": {"name": "brauser", "version": env!("CARGO_PKG_VERSION")},
                "capabilities": {"experimentalApi": true},
            })
        );
        assert_eq!(
            requests[1]["params"],
            json!({
                "cwd": spec.cwd.display().to_string(),
                "approvalPolicy": "never",
                "sandbox": "danger-full-access",
            })
        );
        assert_eq!(requests[1]["params"], thread_start_params(&spec.cwd));
        assert_eq!(
            requests[2]["params"],
            json!({"threadId": "thr-1", "name": "Brauser: Title"})
        );
        assert_eq!(
            requests[3]["params"],
            json!({
                "threadId": "thr-1",
                "cwd": spec.cwd.display().to_string(),
                "approvalPolicy": "never",
                "sandboxPolicy": {"type": "dangerFullAccess"},
                "input": [{"type": "text", "text": "the prompt"}],
            })
        );

        let (events, outcome) = &turns[0];
        assert_eq!(
            events.first(),
            Some(&HarnessEvent::Session(SessionRef::CodexThread(
                "thr-1".to_string()
            )))
        );
        assert_eq!(outcome, &answered("{\"ok\":true}"));
        assert_eq!(fake.runner(None).harness(), HarnessId::Codex);

        // Without `codex` on the search path the run does not start.
        let empty = tempfile::tempdir().unwrap();
        let mut spec = spec;
        spec.env[0].1 = empty.path().as_os_str().to_owned();
        let error = CodexRunner::new(None)
            .start(&spec, &ProcessSlot::default())
            .err()
            .expect("no codex to start");
        assert!(error.to_string().contains("codex"), "{error:#}");
    }

    #[test]
    fn deltas_stream_and_completed_item_is_the_answer() {
        let fake = Fake::turns(&answer_turn(&[
            delta("msg-0", "Reading the page. "),
            completed_message("msg-0", "Reading the page."),
            notice(
                "item/completed",
                json!({"item": {"type": "commandExecution", "id": "cmd-1", "command": "ls"}}),
            ),
            delta("msg-1", "{\"title\":"),
            delta("msg-1", "\"T\"}"),
            completed_message("msg-1", "{\"title\":\"T\"}"),
            turn_completed("completed"),
        ]));
        let turns = run(fake.runner(None), fake.spec(), &["the prompt"]);
        let (events, outcome) = &turns[0];
        assert_eq!(
            texts(events),
            ["Reading the page. ", "{\"title\":", "\"T\"}"]
        );
        assert_eq!(outcome, &answered("{\"title\":\"T\"}"));

        // A completed message that was never streamed still reaches the
        // stream; one that was streamed in part adds only the rest.
        let fake = Fake::turns(&answer_turn(&[
            completed_message("msg-1", "whole"),
            delta("msg-2", "par"),
            completed_message("msg-2", "partial"),
            turn_completed("completed"),
        ]));
        let turns = run(fake.runner(None), fake.spec(), &["the prompt"]);
        assert_eq!(texts(&turns[0].0), ["whole", "par", "tial"]);
        assert_eq!(turns[0].1, answered("partial"));

        assert_eq!(
            parse_notice(&delta("m", "x")),
            Notice::Delta {
                item: "m".to_string(),
                text: "x".to_string()
            }
        );
        assert_eq!(
            parse_notice(&notice("thread/started", json!({}))),
            Notice::Other
        );
    }

    #[test]
    fn failed_turn_reports_codex_error_message() {
        let fake = Fake::turns(&answer_turn(&[
            delta("msg-1", "partial"),
            notice(
                "turn/completed",
                json!({"threadId": "thr-1", "turn": {"id": "t", "status": "failed",
                       "error": {"message": "Usage limit reached."}}}),
            ),
        ]));
        let turns = run(fake.runner(None), fake.spec(), &["the prompt"]);
        assert_eq!(turns[0].1, failed("Usage limit reached."));

        // A turn/start the app-server refuses fails with its message.
        let fake = Fake::turns(
            r#"printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32600,"message":"bad turn"}}\n' "$n""#,
        );
        let turns = run(fake.runner(None), fake.spec(), &["the prompt"]);
        let TurnOutcome::Failed { reason } = &turns[0].1 else {
            panic!("{:?}", turns[0].1);
        };
        assert!(reason.contains("bad turn"), "{reason}");
    }

    #[test]
    fn exit_before_turn_completed_is_a_failure() {
        let fake = Fake::turns(&format!(
            "{}; echo 'codex: lost connection' >&2; exit 1",
            answer_turn(&[delta("msg-1", "half")])
        ));
        let turns = run(fake.runner(None), fake.spec(), &["the prompt"]);
        let (events, outcome) = &turns[0];
        assert_eq!(texts(events), ["half"]);
        assert!(events.iter().any(|event| matches!(
            event, HarnessEvent::Diagnostic(bytes) if bytes.starts_with(b"codex: lost")
        )));
        assert_eq!(outcome, &failed(EXITED));
    }

    #[test]
    fn second_turn_reuses_the_thread() {
        let fake = Fake::turns(&format!(
            r#"if [ "$n" = 4 ]; then {}; else {}; fi"#,
            answer_turn(&[
                delta("msg-1", "not json"),
                completed_message("msg-1", "not json"),
                turn_completed("completed"),
            ]),
            answer_turn(&[
                delta("msg-2", "{}"),
                completed_message("msg-2", "{}"),
                turn_completed("completed"),
            ]),
        ));
        let turns = run(fake.runner(None), fake.spec(), &["the prompt", "fix it"]);
        assert_eq!(turns[0].1, answered("not json"));
        assert_eq!(texts(&turns[1].0), ["\n\n", "{}"]);
        assert_eq!(turns[1].1, answered("{}"));
        // The thread id is reported once, on the first turn.
        assert!(
            !turns[1]
                .0
                .iter()
                .any(|event| matches!(event, HarnessEvent::Session(_)))
        );

        let requests = fake.requests();
        let methods: Vec<_> = requests.iter().map(|r| r["method"].clone()).collect();
        assert_eq!(
            methods,
            [
                "initialize",
                "thread/start",
                "thread/name/set",
                "turn/start",
                "turn/start"
            ]
        );
        assert_eq!(requests[4]["id"], 5);
        assert_eq!(requests[4]["params"]["threadId"], "thr-1");
        assert_eq!(
            requests[4]["params"]["input"],
            json!([{"type": "text", "text": "fix it"}])
        );
    }

    #[test]
    fn second_turn_after_app_server_exit_fails_cleanly() {
        let fake = Fake::turns(&format!(
            "{}; exit 0",
            answer_turn(&[
                completed_message("msg-1", "not json"),
                turn_completed("completed"),
            ])
        ));
        let turns = run(fake.runner(None), fake.spec(), &["the prompt", "fix it"]);
        assert_eq!(turns[0].1, answered("not json"));
        assert_eq!(turns[1], (Vec::new(), failed(EXITED)));
    }

    /// The app-server stops reading before its stdout closes, so the next
    /// `turn/start` hits a broken pipe rather than a seen exit.
    #[test]
    fn second_turn_into_a_closed_stdin_fails_as_exited() {
        let fake = Fake::turns(&format!(
            "{}; exec 0<&-; printf '%s\\n' '{}'; sleep 2; exit 0",
            answer_turn(&[completed_message("msg-1", "not json")]),
            turn_completed("completed"),
        ));
        let turns = run(fake.runner(None), fake.spec(), &["the prompt", "fix it"]);
        assert_eq!(turns[0].1, answered("not json"));
        assert_eq!(turns[1], (Vec::new(), failed(EXITED)));
    }

    /// Closing reaps the app-server, so the slot no longer names its group
    /// and a late cancel or timeout signals nothing.
    #[test]
    fn close_leaves_no_group_to_kill() {
        let fake = Fake::turns(&simple_turn());
        let slot = ProcessSlot::default();
        let mut session = fake.runner(None).start(&fake.spec(), &slot).unwrap();
        assert_eq!(
            session.turn("the prompt", &mut |_| {}),
            answered("{\"ok\":true}")
        );
        assert!(slot.group().is_some());
        session.close();
        assert_eq!(slot.group(), None);
    }

    #[test]
    fn thread_name_is_prefixed_and_bounded() {
        assert_eq!(thread_name("Title"), "Brauser: Title");
        assert_eq!(
            thread_name("  Two\nlines\t here "),
            "Brauser: Two lines here"
        );
        assert_eq!(thread_name(""), "Brauser: Untitled page");
        let long = thread_name(&"é".repeat(300));
        assert_eq!(long.chars().count(), 100);
        assert!(long.starts_with("Brauser: éé"));
        assert!(long.ends_with('…'));
        assert_eq!(thread_name(&"x".repeat(91)).chars().count(), 100);
        assert!(!thread_name(&"x".repeat(91)).ends_with('…'));

        // A failed thread/name/set is a diagnostic; the run goes on.
        let fake = Fake::new(
            r#"{"jsonrpc":"2.0","id":3,"error":{"code":-32601,"message":"no names here"}}"#,
            &simple_turn(),
        );
        let turns = run(fake.runner(None), fake.spec(), &["the prompt"]);
        let (events, outcome) = &turns[0];
        assert_eq!(outcome, &answered("{\"ok\":true}"));
        let diagnostics: Vec<String> = events
            .iter()
            .filter_map(|event| match event {
                HarnessEvent::Diagnostic(bytes) => Some(String::from_utf8_lossy(bytes).into()),
                _ => None,
            })
            .collect();
        assert!(
            diagnostics
                .iter()
                .any(|line| line.contains("thread/name/set") && line.contains("no names here")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn output_schema_is_sent_only_when_configured() {
        let schema = json!({"type": "object", "required": ["title"]});
        let cwd = Path::new("/kb");
        assert!(
            turn_start_params("thr", cwd, "p", None)
                .get("outputSchema")
                .is_none()
        );
        assert_eq!(
            turn_start_params("thr", cwd, "p", Some(&schema))["outputSchema"],
            schema
        );

        let fake = Fake::turns(&simple_turn());
        run(
            fake.runner(Some(schema.clone())),
            fake.spec(),
            &["the prompt"],
        );
        assert_eq!(fake.requests()[3]["params"]["outputSchema"], schema);

        let fake = Fake::turns(&simple_turn());
        run(fake.runner(None), fake.spec(), &["the prompt"]);
        assert!(fake.requests()[3]["params"].get("outputSchema").is_none());
    }

    /// Setup's probe asks for one word, so it must not be held to the
    /// summary answer's shape.
    #[test]
    fn probe_runner_sends_no_output_schema() {
        let schema = json!({"type": "object", "required": ["title"]});
        let fake = Fake::turns(&simple_turn());
        let probe = fake.runner(Some(schema)).for_probe().unwrap();
        assert_eq!(probe.harness(), HarnessId::Codex);
        let mut session = probe.start(&fake.spec(), &ProcessSlot::default()).unwrap();
        session.turn("the prompt", &mut |_| {});
        session.close();
        assert!(fake.requests()[3]["params"].get("outputSchema").is_none());
    }

    #[test]
    fn service_env_and_codex_home_reach_codex() {
        let fake = Fake::turns(&simple_turn());
        run(fake.runner(None), fake.spec(), &["the prompt"]);
        let mut names: Vec<_> = fake
            .log("env.log")
            .lines()
            // What the shell itself sets.
            .filter(|name| !matches!(*name, "PWD" | "OLDPWD" | "SHLVL" | "_"))
            .map(str::to_string)
            .collect();
        names.sort();
        assert_eq!(names, ["AGENT_EXTRA", "CODEX_HOME", "HOME", "PATH"]);
    }
}
