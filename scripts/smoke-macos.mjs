#!/usr/bin/env node

// Guided real-Chrome M1 smoke run, plus summary harness setup and summary
// runs. Native dialogs are answered by a person, or with --auto through UI
// scripting; all host and vault state is kept in a temp home. The extension
// holds HTTP(S) access from install, so Chrome shows no permission prompt.
// --headless runs the same flow with nothing on screen: headless Chrome and a
// scripted-dialogs host build. Only --headless starts the agent service
// (never through launchd), sets up a summary harness, and summarizes pages
// through the real Codex and Claude Code runners, against fake `codex` and
// `claude` programs on a pinned PATH and a cleared environment.
import { spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { constants } from "node:fs";
import {
  access, chmod, lstat, mkdir, mkdtemp, open, readFile,
  readdir, realpath, rename, rm, unlink, writeFile,
} from "node:fs/promises";
import { createServer } from "node:http";
import os from "node:os";
import path from "node:path";
import { createInterface } from "node:readline/promises";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import { APP_NAME, BINARY_NAME, NATIVE_HOST_NAME, storageKey } from "../extension/brand.ts";

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const DIST = path.join(REPO, "extension", "dist");
const DEFAULT_HOST = path.join(REPO, "target", "debug", BINARY_NAME);
// A separate target directory keeps the scripted build away from the host a
// developer registers for everyday use.
const SCRIPTED_HOST = path.join(REPO, "target", "scripted-dialogs", "debug", BINARY_NAME);
// An absolute target directory, so the printed command builds where the
// runner looks even when run from host/.
const SCRIPTED_BUILD = `cargo build --locked -p ${BINARY_NAME} --features scripted-dialogs --target-dir ${shellQuote(path.dirname(path.dirname(SCRIPTED_HOST)))}`;
const SCRIPTED_DIALOGS_ENV = `${BINARY_NAME.toUpperCase()}_SCRIPTED_DIALOGS`;
// Keep in step with OPEN_LOG_ENV in host/src/brand.rs: a scripted-dialogs
// build appends what it would open there instead of running /usr/bin/open.
// The wrapper never sets SERVICE_SPAWN_ENV: the smoke starts the service
// itself, and SERVICE_LABEL is never passed to launchctl.
const OPEN_LOG_ENV = `${BINARY_NAME.toUpperCase()}_OPEN_LOG`;
const DEFAULT_CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
const DEV_DESCRIPTION = `${APP_NAME} development native host`;
// Keep in step with PROTOCOL_VERSION in protocol/schema.json.
const PROTOCOL_VERSION = 6;
// Keep in step with ACTIVITY_TOTALS_KEY in extension/worker.ts.
const ACTIVITY_TOTALS_KEY = storageKey("activity_totals_v1");
const EXTENSION_ID = /^[a-p]{32}$/;
// Keep in step with PROBE_PROMPT in host/src/harness.rs and repair_prompt in
// host/src/service/prompt.rs: the fakes answer the setup probe and the repair
// turn by these.
const PROBE_PROMPT = "Reply with the single word OK.";
const REPAIR_PREFIX = "Your previous answer was not valid";
const EXTENSION_ID_ALPHABET = "abcdefghijklmnop";

function usage() {
  console.log(`Usage: node scripts/smoke-macos.mjs [--host /absolute/path/to/${BINARY_NAME}] [--chrome /absolute/path/to/Google Chrome] [--default-port] [--auto | --headless]`);
  console.log("  --default-port  Serve the fixture on port 80 and enter the site as http://127.0.0.1:80.");
  console.log("  --auto          Answer the prompts through macOS UI scripting; the terminal needs Accessibility access.");
  console.log(`  --headless      Show nothing on screen: headless Chrome and scripted dialogs. Build the host with:\n                  ${SCRIPTED_BUILD}`);
}

function argumentsForRun(args) {
  let host = null;
  let chrome = DEFAULT_CHROME;
  let defaultPort = false;
  let auto = false;
  let headless = false;
  for (let index = 0; index < args.length; index += 1) {
    if (args[index] === "--help") {
      usage();
      return null;
    }
    if (args[index] === "--default-port") {
      defaultPort = true;
      continue;
    }
    if (args[index] === "--auto") {
      auto = true;
      continue;
    }
    if (args[index] === "--headless") {
      headless = true;
      continue;
    }
    if ((args[index] === "--host" || args[index] === "--chrome") && args[index + 1]) {
      if (!path.isAbsolute(args[index + 1])) throw new Error(`${args[index]} needs an absolute path`);
      if (args[index] === "--host") host = args[index + 1];
      else chrome = args[index + 1];
      index += 1;
      continue;
    }
    throw new Error(`Unknown argument: ${args[index]}`);
  }
  if (auto && headless) throw new Error("Choose either --auto or --headless");
  return { host: host ?? (headless ? SCRIPTED_HOST : DEFAULT_HOST), chrome, defaultPort, auto, headless };
}

function shellQuote(value) {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function inside(parent, child) {
  const relative = path.relative(parent, child);
  return relative === "" || (relative !== ".." && !relative.startsWith(`..${path.sep}`) && !path.isAbsolute(relative));
}

async function maybeLstat(filename) {
  try {
    return await lstat(filename);
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

async function atomicWrite(filename, contents, mode = 0o600) {
  const directory = path.dirname(filename);
  await mkdir(directory, { recursive: true });
  const temporary = path.join(directory, `.${path.basename(filename)}.${randomUUID()}.tmp`);
  const file = await open(temporary, "wx", mode);
  try {
    await file.writeFile(contents);
    await file.chmod(mode);
    await file.sync();
  } finally {
    await file.close();
  }
  try {
    await rename(temporary, filename);
  } catch (error) {
    await unlink(temporary).catch(() => undefined);
    throw error;
  }
}

async function commandOutput(executable, args) {
  return new Promise((resolve, reject) => {
    const child = spawn(executable, args, { stdio: ["ignore", "pipe", "pipe"] });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", (chunk) => { stderr += chunk; });
    child.on("error", reject);
    child.on("close", (code) => {
      if (code === 0) resolve(stdout.trim());
      else reject(new Error(`${executable} exited ${code}: ${stderr.trim()}`));
    });
  });
}

// --auto answers the prompts through macOS UI scripting, which needs the
// terminal to hold Accessibility access. The host's folder picker does not
// answer Accessibility queries, so it is driven by keyboard; its Yes/No
// alert is drawn by UserNotificationCenter, which does.
async function osascript(script, language = "AppleScript") {
  return commandOutput("/usr/bin/osascript", ["-l", language, "-e", script]);
}

async function requireAccessibility() {
  const enabled = await osascript('tell application "System Events" to get UI elements enabled');
  requireCondition(enabled === "true" && await osascript(
    'tell application "System Events" to get name of first process whose frontmost is true',
  ).then(() => true, () => false),
  "--auto needs Accessibility access for this terminal: System Settings > Privacy & Security > Accessibility");
}

async function onScreenWindows(pid) {
  const listed = await osascript(`ObjC.import("CoreGraphics");
    JSON.stringify(ObjC.deepUnwrap(ObjC.castRefToObject($.CGWindowListCopyWindowInfo($.kCGWindowListOptionOnScreenOnly, 0)))
      .filter((window) => window.kCGWindowOwnerPID === ${Number(pid)}).length)`, "JavaScript");
  return Number(listed);
}

async function pickerPid() {
  const pids = await commandOutput("/usr/bin/pgrep", ["-f", `${BINARY_NAME} __dialog-pick-folder`]).catch(() => "");
  const list = pids.split("\n").filter(Boolean);
  requireCondition(list.length === 1, list.length ? "More than one folder picker is open" : "The folder picker is not open yet");
  requireCondition(await onScreenWindows(list[0]) > 0, "The folder picker window is not on screen yet");
  return Number(list[0]);
}

// Keystrokes go to the frontmost process. System Events cannot report the
// unbundled picker process as frontmost, so activate it and wait.
async function keysToPicker(pid, keys) {
  await osascript(`tell application "System Events"
    set frontmost of (first process whose unix id is ${pid}) to true
    delay 1
    ${keys}
  end tell`);
}

async function cancelPicker() {
  const pid = await waitFor(pickerPid, { timeout: 20_000 });
  await keysToPicker(pid, "key code 53");
}

async function choosePickerFolder(folder) {
  const pid = await waitFor(pickerPid, { timeout: 20_000 });
  const closed = async () => requireCondition(
    !(await commandOutput("/bin/ps", ["-p", String(pid)]).then(() => true, () => false)), "The folder picker is still open");
  // Early keystrokes can be lost while the picker takes focus; if it is still
  // open, repeat the whole sequence, which is safe from either picker state.
  for (let attempt = 1; ; attempt += 1) {
    await keysToPicker(pid, `keystroke "g" using {command down, shift down}
      delay 1.5
      keystroke "a" using {command down}
      keystroke ${JSON.stringify(folder)}
      delay 1
      key code 36
      delay 1.5
      key code 36`);
    try {
      return await waitFor(closed, { timeout: 5_000 });
    } catch (error) {
      if (attempt === 3) throw error;
    }
  }
}

// Fake `codex` and `claude` for summary harness setup and runs. They are Node
// scripts (this Node, by absolute path, so nothing depends on PATH) speaking
// just enough of each harness's protocol for the real runners: Codex's
// app-server JSON-RPC on stdio and Claude Code's `-p` stream-json. Each logs
// JSON lines to harness-argv.log: one per start with its argv, cwd and
// environment variable names (never values), then what it read on stdin (each
// JSON-RPC request for codex, the whole prompt for claude). Each run writes a
// line of stderr, which the service keeps in run.log. $HOME/fake-agent.mode
// picks how a summary turn ends:
// - ok: a valid §6.4 answer, streamed in two deltas;
// - slow: no answer, until killed;
// - fail: "fake failure" on stderr, and a failed turn;
// - badjson-once: text that is not JSON, then a valid answer to the repair turn;
// - badjson: text that is not JSON, every time.
// The setup probe is answered "OK" in every mode except fail.
const FAKE_SESSION = "fake-session-1";
const FAKE_THREAD = "fake-thread-1";
const FAKE_ANSWER = {
  document_type: "article", category: "news",
  entities: { technologies: ["Smoke"], products: [], people: [], companies: [] },
  description: "A smoke article the fake agent read.",
  importance: { level: "high", reason: "The smoke run needs it." },
  assessment: "Nothing new beyond the smoke run.",
  related_notes: [],
};

function fakeHarnessPrelude(name, log) {
  return `#!${process.execPath}
"use strict";
const fs = require("node:fs");
const path = require("node:path");
const NAME = ${JSON.stringify(name)};
const LOG = ${JSON.stringify(log)};
const log = (entry) => fs.appendFileSync(LOG, JSON.stringify({ program: NAME, pid: process.pid, ...entry }) + "\\n");
const mode = () => {
  try { return fs.readFileSync(path.join(process.env.HOME || "/nonexistent", "fake-agent.mode"), "utf8").trim(); } catch { return "ok"; }
};
// What a turn answers: { text } or { fail: true } or { slow: true }.
function reply(prompt) {
  const current = mode();
  if (current === "fail") return { fail: true };
  if (prompt.trim() === ${JSON.stringify(PROBE_PROMPT)}) return { text: "OK" };
  if (current === "slow") return { slow: true };
  const bad = current === "badjson" || (current === "badjson-once" && !prompt.startsWith(${JSON.stringify(REPAIR_PREFIX)}));
  return { text: bad ? "Here is my summary, not as JSON." : ${JSON.stringify(JSON.stringify(FAKE_ANSWER))} };
}
const halves = (text) => [text.slice(0, Math.ceil(text.length / 2)), text.slice(Math.ceil(text.length / 2))];
const args = process.argv.slice(2);
log({ argv: args, cwd: process.cwd(), env: Object.keys(process.env).sort() });
`;
}

const FAKE_CODEX = `if (args.length === 1 && args[0] === "--version") {
  process.stdout.write("codex-cli 0.144.4\\n");
  process.exit(0);
}
if (args.join(" ") !== "app-server --listen stdio://") process.exit(2);
process.stderr.write("fake codex diagnostics\\n");
const send = (message) => process.stdout.write(JSON.stringify({ jsonrpc: "2.0", ...message }) + "\\n");
const notify = (method, params) => send({ method, params: { threadId: ${JSON.stringify(FAKE_THREAD)}, ...params } });
let turns = 0;
require("node:readline").createInterface({ input: process.stdin }).on("line", (line) => {
  let request;
  try { request = JSON.parse(line); } catch { return; }
  log({ rpc: request });
  const { id, method, params } = request;
  if (method === "initialize") return send({ id, result: { userAgent: "fake-codex" } });
  if (method === "thread/start") return send({ id, result: { thread: { id: ${JSON.stringify(FAKE_THREAD)} } } });
  if (method === "thread/name/set") return send({ id, result: {} });
  if (method !== "turn/start") return send({ id, error: { code: -32601, message: "fake codex: " + method } });
  turns += 1;
  const turn = "fake-turn-" + turns;
  send({ id, result: { turn: { id: turn } } });
  const answer = reply(params.input.map((item) => item.text).join(""));
  if (answer.slow) return;
  if (answer.fail) {
    process.stderr.write("fake failure\\n");
    return notify("turn/completed", { turn: { id: turn, status: "failed", error: { message: "fake failure" } } });
  }
  const item = "msg-" + turns;
  for (const delta of halves(answer.text)) notify("item/agentMessage/delta", { turnId: turn, itemId: item, delta });
  notify("item/completed", { turnId: turn, item: { type: "agentMessage", id: item, text: answer.text } });
  notify("turn/completed", { turn: { id: turn, status: "completed", error: null } });
});
`;

const FAKE_CLAUDE = `if (args.length === 1 && args[0] === "--version") {
  process.stdout.write("2.1.310 (Claude Code)\\n");
  process.exit(0);
}
if (args[0] !== "-p") process.exit(2);
process.stderr.write("fake claude diagnostics\\n");
const chunks = [];
process.stdin.on("data", (chunk) => chunks.push(chunk));
process.stdin.on("end", () => {
  const prompt = Buffer.concat(chunks).toString("utf8");
  log({ stdin: prompt });
  const answer = reply(prompt);
  if (answer.fail) {
    process.stderr.write("fake failure\\n");
    process.exit(3);
  }
  const line = (value) => process.stdout.write(JSON.stringify(value) + "\\n");
  line({ type: "system", subtype: "init", session_id: ${JSON.stringify(FAKE_SESSION)}, cwd: process.cwd(), tools: [] });
  if (answer.slow) {
    setTimeout(() => undefined, 60_000);
    return;
  }
  for (const text of halves(answer.text)) {
    line({ type: "stream_event", session_id: ${JSON.stringify(FAKE_SESSION)},
      event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } } });
  }
  line({ type: "assistant", session_id: ${JSON.stringify(FAKE_SESSION)},
    message: { role: "assistant", content: [{ type: "text", text: answer.text }] } });
  line({ type: "result", subtype: "success", is_error: false, result: answer.text, session_id: ${JSON.stringify(FAKE_SESSION)} });
});
`;

async function installFakeHarnesses(runDir) {
  const fakeBin = path.join(runDir, "fake-bin");
  const argvLog = path.join(runDir, "harness-argv.log");
  await mkdir(fakeBin, { mode: 0o700 });
  await chmod(fakeBin, 0o700);
  for (const [name, body] of [["codex", FAKE_CODEX], ["claude", FAKE_CLAUDE]]) {
    const file = path.join(fakeBin, name);
    await writeFile(file, `${fakeHarnessPrelude(name, argvLog)}${body}`, { mode: 0o700 });
    await chmod(file, 0o700);
  }
  return { fakeBin, argvLog };
}

// Every file a finished run keeps in its record directory.
const RUN_FILES = ["status.json", "page.md", "stream.txt", "result.json", "run.log"];

// RFC 3339 to the second in UTC, as start_run's requested_at requires.
const requestedAt = () => new Date().toISOString().replace(/\.\d{3}Z$/, "Z");

async function stopService(service) {
  if (!service || service.exitCode !== null || service.signalCode !== null) return;
  const exited = new Promise((resolve) => service.once("exit", resolve));
  service.kill("SIGKILL");
  await Promise.race([exited, delay(5_000).then(() => {
    throw new Error(`The service process ${service.pid} did not exit`);
  })]);
}

// What the fakes logged, one entry per line; each entry also carries its pid,
// so a start and what that process read can be matched.
async function harnessLog(argvLog) {
  return (await readFile(argvLog, "utf8").catch(() => "")).split("\n").filter(Boolean)
    .map((line) => JSON.parse(line));
}

// Each fake process: its start entry with what it read on stdin.
async function harnessRuns(argvLog) {
  const runs = new Map();
  for (const entry of await harnessLog(argvLog)) {
    if (entry.argv) runs.set(entry.pid, { ...entry, rpc: [], stdin: null });
    const run = runs.get(entry.pid);
    if (!run) continue;
    if (entry.rpc) run.rpc.push(entry.rpc);
    if (typeof entry.stdin === "string") run.stdin = entry.stdin;
  }
  return [...runs.values()];
}

const isVersionCheck = (run) => run.argv.length === 1 && run.argv[0] === "--version";
const isProbe = (run) => run.stdin?.trim() === PROBE_PROMPT ||
  run.rpc.some((request) => request.method === "turn/start" &&
    request.params.input.some((item) => item.text === PROBE_PROMPT));

// Spawn a short-lived host process for one framed request, independent of
// whatever host process is serving the live panel or settings connection.
// The host's config-file lock (§7) serializes these against each other.
function hostRequest(wrapper, request) {
  const body = Buffer.from(JSON.stringify(request));
  const length = Buffer.alloc(4);
  length.writeUInt32LE(body.length);
  return new Promise((resolve, reject) => {
    const child = spawn(wrapper, ["serve"], { stdio: ["pipe", "pipe", "pipe"] });
    const output = [];
    const errors = [];
    let settled = false;
    const timer = setTimeout(() => {
      child.kill("SIGKILL");
      finish(new Error(`Native host ${request.type} timed out`));
    }, 10_000);
    function finish(error, value) {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error) reject(error);
      else resolve(value);
    }
    child.stdout.on("data", (chunk) => { output.push(chunk); });
    child.stderr.on("data", (chunk) => { errors.push(chunk); });
    child.on("error", (error) => finish(error));
    child.on("close", (code) => {
      try {
        if (code !== 0) throw new Error(`Native host ${request.type} failed: ${Buffer.concat(errors).toString("utf8").trim()}`);
        const frame = Buffer.concat(output);
        if (frame.length < 4 || frame.readUInt32LE(0) !== frame.length - 4) {
          throw new Error("Native host returned an invalid response frame");
        }
        const reply = JSON.parse(frame.subarray(4).toString("utf8"));
        if (reply.protocol_version !== PROTOCOL_VERSION) {
          throw new Error(`Native host returned ${reply.type ?? "an unknown response"}`);
        }
        finish(null, reply);
      } catch (error) {
        finish(error);
      }
    });
    child.stdin.end(Buffer.concat([length, body]));
  });
}

async function hostConfig(wrapper) {
  const reply = await hostRequest(wrapper, {
    type: "get_config", protocol_version: PROTOCOL_VERSION, request_id: randomUUID(),
  });
  if (reply.type !== "config_result") {
    throw new Error(`Native host returned ${reply.type ?? "an unknown response"} instead of config_result`);
  }
  return reply;
}

// Simulates a second Brauser window saving this page's note first, so the
// panel's own next save is refused as stale (§4.4, §5.3).
async function hostSaveNote(wrapper, url, title, body, expectedRevision, expectedKey) {
  const reply = await hostRequest(wrapper, {
    type: "save_note", protocol_version: PROTOCOL_VERSION, request_id: randomUUID(),
    url, title, body, expected_revision: expectedRevision, expected_key: expectedKey,
  });
  if (reply.type !== "note_saved") {
    throw new Error(`Native host returned ${reply.type ?? "an unknown response"} instead of note_saved`);
  }
  return reply;
}

// One reader for Chrome's DevTools pipe. Replies are matched by id; events
// are ignored because every wait below polls page or host state instead.
class Cdp {
  constructor(chrome) {
    this.chrome = chrome;
    this.outgoing = chrome.stdio[3];
    this.incoming = chrome.stdio[4];
    if (!this.outgoing || !this.incoming) throw new Error("Chrome DevTools pipe is unavailable");
    this.nextId = 1;
    this.pending = new Map();
    this.buffer = Buffer.alloc(0);
    this.incoming.on("data", (chunk) => this.receive(chunk));
    const fail = (error) => {
      for (const { reject } of this.pending.values()) reject(error);
      this.pending.clear();
    };
    this.incoming.on("error", fail);
    chrome.once("exit", (code) => fail(new Error(`Chrome exited (${code})`)));
  }

  receive(chunk) {
    this.buffer = Buffer.concat([this.buffer, chunk]);
    for (;;) {
      const end = this.buffer.indexOf(0);
      if (end < 0) return;
      const text = this.buffer.subarray(0, end).toString("utf8");
      this.buffer = this.buffer.subarray(end + 1);
      let message;
      try { message = JSON.parse(text); } catch { continue; }
      const waiter = message.id === undefined ? null : this.pending.get(message.id);
      if (!waiter) continue;
      this.pending.delete(message.id);
      if (message.error) waiter.reject(new Error(`Chrome ${waiter.method} failed: ${message.error.message ?? JSON.stringify(message.error)}`));
      else waiter.resolve(message.result);
    }
  }

  send(method, params = {}, sessionId = undefined, timeout = 15_000) {
    const id = this.nextId++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`Chrome ${method} timed out`));
      }, timeout);
      this.pending.set(id, {
        method,
        resolve: (value) => { clearTimeout(timer); resolve(value); },
        reject: (error) => { clearTimeout(timer); reject(error); },
      });
      this.outgoing.write(`${JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) })}\0`,
        (error) => { if (error) this.pending.get(id)?.reject(error); });
    });
  }

  async targets() {
    return (await this.send("Target.getTargets")).targetInfos;
  }

  async targetFor(url) {
    return (await this.targets()).find((target) => target.url === url || target.url.startsWith(`${url}?`)) ?? null;
  }

  /** Attach to a target for one operation, then detach. */
  async withTarget(targetId, action) {
    const { sessionId } = await this.send("Target.attachToTarget", { targetId, flatten: true });
    try {
      return await action(new PageDriver(this, sessionId));
    } finally {
      await this.send("Target.detachFromTarget", { sessionId }).catch(() => undefined);
    }
  }
}

// Drives an extension or fixture page. Clicks are real input events, so
// Chrome treats them as user gestures just as it would a mouse click.
class PageDriver {
  constructor(cdp, sessionId) {
    this.cdp = cdp;
    this.sessionId = sessionId;
  }

  async evaluate(expression, { userGesture = false } = {}) {
    const reply = await this.cdp.send("Runtime.evaluate", {
      expression, awaitPromise: true, returnByValue: true, userGesture,
    }, this.sessionId);
    if (reply.exceptionDetails) {
      throw new Error(reply.exceptionDetails.exception?.description ?? reply.exceptionDetails.text);
    }
    return reply.result?.value;
  }

  async click(selector) {
    const point = await this.evaluate(`(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) throw new Error("Missing ${selector.replaceAll('"', "'")}");
      if (element.disabled) throw new Error("${selector.replaceAll('"', "'")} is disabled");
      element.scrollIntoView({ block: "center" });
      const box = element.getBoundingClientRect();
      return { x: box.left + box.width / 2, y: box.top + box.height / 2 };
    })()`);
    for (const type of ["mousePressed", "mouseReleased"]) {
      await this.cdp.send("Input.dispatchMouseEvent", {
        type, x: point.x, y: point.y, button: "left", clickCount: 1,
      }, this.sessionId);
    }
  }

  async fill(selector, value) {
    await this.evaluate(`(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      if (!element) throw new Error("Missing ${selector.replaceAll('"', "'")}");
      element.value = ${JSON.stringify(value)};
      element.dispatchEvent(new Event("input", { bubbles: true }));
    })()`);
  }

  text(selector) {
    return this.evaluate(`document.querySelector(${JSON.stringify(selector)})?.textContent ?? null`);
  }

  enabled(selector) {
    return this.evaluate(`(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      return element !== null && !element.disabled;
    })()`);
  }

  hidden(selector) {
    return this.evaluate(`document.querySelector(${JSON.stringify(selector)})?.hidden ?? null`);
  }
}

async function nativeHostPreflight(cdp, extensionId) {
  const target = await cdp.send("Target.createTarget", {
    url: `chrome-extension://${extensionId}/panel.html`, background: true,
  });
  try {
    const expression = `new Promise(resolve => {
      const port = chrome.runtime.connectNative("${NATIVE_HOST_NAME}");
      const timer = setTimeout(() => resolve({ok: false, error: "native host timed out"}), 5000);
      port.onMessage.addListener(message => {
        clearTimeout(timer);
        resolve({ok: message.type === "hello_result", type: message.type});
        port.disconnect();
      });
      port.onDisconnect.addListener(() => {
        clearTimeout(timer);
        resolve({ok: false, error: chrome.runtime.lastError?.message ?? "disconnected"});
      });
      port.postMessage({type: "hello", protocol_version: ${PROTOCOL_VERSION}, request_id: "smoke-preflight"});
    })`;
    await cdp.withTarget(target.targetId, async (page) => {
      let lastError = "extension page did not load";
      for (let attempt = 0; attempt < 20; attempt += 1) {
        let reply;
        try {
          reply = await page.evaluate(expression);
        } catch (error) {
          lastError = error.message;
          if (!/connectNative/.test(lastError)) break;
          await delay(250);
          continue;
        }
        if (reply?.ok === true) return;
        lastError = reply?.error ?? `unexpected host response ${reply?.type ?? "none"}`;
        break;
      }
      throw new Error(`Chrome could not connect to the native host: ${lastError}`);
    });
  } finally {
    await cdp.send("Target.closeTarget", { targetId: target.targetId }).catch(() => undefined);
    // This tab has the side panel's URL; later steps must not mistake it for the panel.
    await waitFor(async () => requireCondition(
      !(await cdp.targets()).some((entry) => entry.targetId === target.targetId),
      "Preflight tab is still closing")).catch(() => undefined);
  }
}

// The /article page: a link-only navigation block beside an article body.
// Extraction must keep the article and drop the navigation.
const articleText = (caseId) => `The smoke article body ${caseId}`;
const ARTICLE_NAV_TEXT = "Smoke navigation link";

function articlePage(title, caseId) {
  const links = ["home", "news", "archive", "about"].map((name) =>
    `<li><a href="/?nav=${name}">${ARTICLE_NAV_TEXT} ${name}</a></li>`).join("");
  const paragraphs = ["first", "second", "third", "fourth"].map((which) =>
    `<p>${articleText(caseId)}, ${which} paragraph, says what a reader came for, with enough words, commas, and ` +
    "sentences that a readability pass treats it as the main content of the page, not as a menu or a footer.</p>").join("");
  return `<!doctype html><html><head><title>${title}</title></head><body>` +
    `<nav class="menu" role="navigation"><ul>${links}</ul></nav>` +
    `<article><h1>${title}</h1>${paragraphs}</article></body></html>`;
}

// The unknown-site wiki: /wiki/pages/<n>/<Title> pages that link to their
// siblings, a sibling's subpage, a search and a sign-in page. Capture Site
// drafts a rule for the pages from these shapes and skips the rest.
const WIKI_PAGES = new Map([["101", "Smoke-Home"], ["102", "Smoke-Notes"], ["103", "Smoke-Plans"]]);
const WIKI_PATH = /^\/wiki\/pages\/(\d+)\/([A-Za-z-]+)$/;
const wikiTitle = (name) => `${APP_NAME} smoke wiki ${name}`;

function wikiPage(number, caseId) {
  const name = WIKI_PAGES.get(number);
  const siblings = [...WIKI_PAGES].filter(([other]) => other !== number);
  const links = [
    ...siblings.map(([other, title]) => [`/wiki/pages/${other}/${title}`, title]),
    [`/wiki/pages/${siblings[0][0]}/${siblings[0][1]}/History`, `${siblings[0][1]} history`],
    ["/search?q=smoke", "Search"],
    ["/login", "Sign in"],
  ].map(([href, text]) => `<li><a href="${href}">${text}</a></li>`).join("");
  return `<!doctype html><html><head><title>${wikiTitle(name)}</title></head><body>` +
    `<h1>${wikiTitle(name)}</h1><p>Run ${caseId}</p><ul>${links}</ul></body></html>`;
}

function fixtureServer(caseId) {
  const server = createServer((request, response) => {
    const pathname = new URL(request.url, "http://127.0.0.1").pathname;
    const known = new Set(["/", "/allowed/page", "/blocked/page", "/allowed/while-paused", "/allowed/after-resume",
      "/allowed/after-removal", "/fresh/page", "/article"]);
    response.setHeader("Content-Type", "text/html; charset=utf-8");
    response.setHeader("Cache-Control", "no-store");
    const wiki = WIKI_PATH.exec(pathname);
    if (wiki && WIKI_PAGES.get(wiki[1]) === wiki[2]) {
      response.end(wikiPage(wiki[1], caseId));
      return;
    }
    if (!known.has(pathname)) {
      response.writeHead(404);
      response.end("Not found");
      return;
    }
    const title = pathname === "/" ? `${APP_NAME} smoke fixture` : `${APP_NAME} smoke ${pathname}`;
    if (pathname === "/article") {
      response.end(articlePage(title, caseId));
      return;
    }
    response.end(`<!doctype html><html><head><title>${title}</title></head><body><h1>${title}</h1><p>Run ${caseId}</p><ul><li><a href="/allowed/page?case=${caseId}">Allowed page</a></li><li><a href="/blocked/page?case=${caseId}">Blocked page</a></li><li><a href="/allowed/after-removal?case=${caseId}">After removal</a></li></ul></body></html>`);
  });
  return server;
}

async function startServer(server, defaultPort) {
  if (defaultPort) {
    // macOS lets an unprivileged process bind port 80 only on the wildcard
    // address, so refuse every connection that is not from this machine.
    server.on("connection", (socket) => {
      if (!["127.0.0.1", "::ffff:127.0.0.1"].includes(socket.remoteAddress ?? "")) socket.destroy();
    });
  }
  await new Promise((resolve, reject) => {
    server.once("error", (error) => reject(defaultPort && error.code === "EADDRINUSE"
      ? new Error("Port 80 is already in use; stop that server or run without --default-port")
      : error));
    server.listen(defaultPort ? 80 : 0, defaultPort ? "0.0.0.0" : "127.0.0.1", resolve);
  });
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("Cannot determine fixture port");
  return defaultPort ? "http://127.0.0.1" : `http://127.0.0.1:${address.port}`;
}

async function stopServer(server) {
  if (!server?.listening) return;
  server.closeAllConnections();
  await new Promise((resolve) => server.close(resolve));
}

async function stopChrome(chrome) {
  if (!chrome?.pid) return;
  // Chrome was started in its own process group. Never signal other profiles.
  try { process.kill(-chrome.pid, "SIGTERM"); } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
  await delay(1500);
  try { process.kill(-chrome.pid, "SIGKILL"); } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
}

function unpackedExtensionId(directory) {
  const digest = createHash("sha256").update(directory).digest();
  return [...digest.subarray(0, 16)].map((byte) =>
    `${EXTENSION_ID_ALPHABET[byte >> 4]}${EXTENSION_ID_ALPHABET[byte & 0x0f]}`).join("");
}

async function extensionIdsInProfile(profile, dist) {
  const ids = new Set();
  for (const entry of await readdir(profile, { withFileTypes: true })) {
    if (!entry.isDirectory() || (entry.name !== "Default" && !entry.name.startsWith("Profile "))) continue;
    for (const filename of ["Preferences", "Secure Preferences"]) {
      let preferences;
      try {
        preferences = JSON.parse(await readFile(path.join(profile, entry.name, filename), "utf8"));
      } catch {
        continue;
      }
      for (const [id, settings] of Object.entries(preferences.extensions?.settings ?? {})) {
        if (!EXTENSION_ID.test(id) || typeof settings?.path !== "string" || !path.isAbsolute(settings.path)) continue;
        if (await realpath(settings.path).catch(() => null) === dist) ids.add(id);
      }
    }
  }
  return [...ids];
}

async function markdownFiles(directory) {
  const found = [];
  async function visit(current) {
    let entries;
    try { entries = await readdir(current, { withFileTypes: true }); } catch (error) {
      if (error.code === "ENOENT") return;
      throw error;
    }
    for (const entry of entries) {
      const filename = path.join(current, entry.name);
      if (entry.isDirectory()) await visit(filename);
      else if (entry.isFile() && entry.name.endsWith(".md")) {
        found.push({ path: filename, text: await readFile(filename, "utf8") });
      }
    }
  }
  await visit(directory);
  return found;
}

function requireCondition(condition, message) {
  if (!condition) throw new Error(message);
}

async function waitFor(check, { timeout = 12_000, interval = 400, signal } = {}) {
  const deadline = Date.now() + timeout;
  let lastError;
  for (;;) {
    if (signal?.aborted) throw signal.reason;
    try { return await check(); } catch (error) {
      if (error.fatal) throw error;
      lastError = error;
    }
    if (Date.now() >= deadline) throw lastError;
    await delay(interval);
  }
}

async function run() {
  const options = argumentsForRun(process.argv.slice(2));
  if (!options) return;
  if (process.platform !== "darwin") throw new Error("This smoke runner supports macOS only");
  const build = options.headless ? SCRIPTED_BUILD : `cargo build --locked -p ${BINARY_NAME}`;
  await access(options.host, constants.X_OK).catch(() => {
    throw new Error(`Build the native host first: ${build} (${options.host} is unavailable)`);
  });
  // A host without the seam would open real dialogs, so check before any runs.
  const scriptedHost = (await commandOutput(options.host, ["--version"])).endsWith("(scripted dialogs)");
  if (options.headless) {
    requireCondition(scriptedHost, `--headless needs a scripted-dialogs host: ${SCRIPTED_BUILD}`);
  } else {
    requireCondition(!scriptedHost, "This host answers dialogs from files; use --headless or a normal build");
  }
  await access(options.chrome, constants.X_OK);
  if (options.auto) await requireAccessibility();
  await access(path.join(DIST, "manifest.json"), constants.R_OK).catch(() => {
    throw new Error(`Build the extension first: npm run build:extension (${DIST} is unavailable)`);
  });
  // The built module, since agent.ts imports other modules by their .js names.
  const { SETUP_WARNING } = await import(pathToFileURL(path.join(DIST, "agent.js")).href);

  const runDir = await mkdtemp(path.join(os.tmpdir(), `${BINARY_NAME}-smoke-macos-`));
  // The service socket lives under HOME, and a Unix socket path must fit in
  // sun_path (104 bytes), so the home is a short /tmp directory of its own.
  // It is removed at the end; everything else stays in runDir.
  const testHome = await mkdtemp("/tmp/bsh-");
  let service = null;
  let lingeringGroup = null;
  const profile = path.join(runDir, "chrome-profile");
  const notes = path.join(runDir, "notes");
  const knowledgeBase = path.join(runDir, "knowledge-base");
  const wrapper = path.join(runDir, `${BINARY_NAME}-host-wrapper`);
  const windowCheck = path.join(runDir, "smoke-macos-windows");
  const dialogs = path.join(runDir, "dialogs");
  const manifest = path.join(profile, "NativeMessagingHosts", `${NATIVE_HOST_NAME}.json`);
  const caseId = randomUUID().slice(0, 8);
  const server = fixtureServer(caseId);
  const abort = new AbortController();
  const prompt = createInterface({ input: process.stdin, output: process.stdout });
  let chrome = null;
  let installedManifest = null;
  let failed = false;
  const interrupt = () => abort.abort(new Error("Smoke run interrupted"));
  process.on("SIGINT", interrupt);
  process.on("SIGTERM", interrupt);

  async function ask(message) {
    const answer = (await prompt.question(message, { signal: abort.signal })).trim();
    if (answer.toLowerCase() === "q") throw new Error("Smoke run stopped");
    return answer;
  }

  let cdp = null;
  let extensionId = null;
  const panelUrl = () => `chrome-extension://${extensionId}/panel.html`;
  const settingsUrl = () => `chrome-extension://${extensionId}/options.html`;

  async function step(title, action) {
    console.log(`\n${title}`);
    await action();
    console.log(`PASS: ${title}`);
  }

  // Only the host's native dialogs need a person, or with --auto, the given
  // automation.
  async function userAction(instruction, check, automate) {
    if ((options.auto || options.headless) && automate) {
      console.log(`  ${options.headless ? "SCRIPTED" : "AUTO"}: ${instruction}`);
      await automate();
      return waitFor(check, { timeout: 30_000, interval: 500, signal: abort.signal });
    }
    if (options.headless) {
      // Nobody can see a headless browser, so never wait for a person.
      throw Object.assign(new Error(`--headless cannot do this step: ${instruction}`), { fatal: true });
    }
    console.log(`  ACTION: ${instruction}`);
    // Say what is still missing, so a stuck wait is diagnosable.
    let reported = null;
    let since = Date.now();
    return waitFor(async () => {
      try {
        return await check();
      } catch (error) {
        if (error.message !== reported && Date.now() - since > 5_000) {
          console.log(`  Waiting: ${error.message}`);
          reported = error.message;
          since = Date.now();
        }
        throw error;
      }
    }, { timeout: 60 * 60_000, interval: 500, signal: abort.signal });
  }

  async function onPage(url, action) {
    const target = await cdp.targetFor(url);
    requireCondition(target, `${url.endsWith("panel.html") ? "The side panel" : "The settings page"} is not open`);
    return cdp.withTarget(target.targetId, action);
  }

  const onPanel = (action) => onPage(panelUrl(), action);
  const onSettings = (action) => onPage(settingsUrl(), action);

  async function inSettings(action) {
    const target = await cdp.targetFor(settingsUrl());
    requireCondition(target, "The settings page is not open");
    // Input events reach only a visible tab.
    await cdp.send("Target.activateTarget", { targetId: target.targetId });
    return cdp.withTarget(target.targetId, action);
  }

  async function openPanel() {
    if (await cdp.targetFor(panelUrl())) return;
    // Opening the side panel needs a user gesture in an extension page.
    // Evaluate with a gesture in the settings page, or a temporary tab.
    let source = await cdp.targetFor(settingsUrl());
    let temporary = null;
    if (!source) {
      temporary = await cdp.send("Target.createTarget", { url: panelUrl(), background: true });
      source = { targetId: temporary.targetId };
    }
    try {
      await cdp.withTarget(source.targetId, async (page) => {
        await waitFor(() => page.evaluate("typeof chrome.sidePanel?.open === 'function' || Promise.reject(new Error('loading'))"));
        const windowId = await page.evaluate("chrome.tabs.getCurrent().then((tab) => tab.windowId)");
        await page.evaluate(`chrome.sidePanel.open({ windowId: ${Number(windowId)} })`, { userGesture: true });
      });
    } catch (error) {
      console.log(`  Could not open the side panel automatically (${error.message}).`);
    } finally {
      if (temporary) {
        await cdp.send("Target.closeTarget", { targetId: temporary.targetId }).catch(() => undefined);
        await waitFor(async () => requireCondition(
          !(await cdp.targets()).some((entry) => entry.targetId === temporary.targetId),
          "Temporary tab is still closing")).catch(() => undefined);
      }
    }
    try {
      await waitFor(async () => requireCondition(await cdp.targetFor(panelUrl()), "Side panel did not open"),
        { timeout: 4_000 });
    } catch {
      await userAction(`Click the ${APP_NAME} toolbar button to open the side panel.`,
        async () => requireCondition(await cdp.targetFor(panelUrl()), "Side panel is not open yet"));
    }
  }

  async function closePanel() {
    const target = await cdp.targetFor(panelUrl());
    if (!target) return;
    await cdp.withTarget(target.targetId, (page) => page.evaluate("window.close()")).catch(() => undefined);
    const closed = async () => requireCondition(!(await cdp.targetFor(panelUrl())), "Side panel is still open");
    try {
      await waitFor(closed, { timeout: 4_000 });
    } catch {
      await userAction(`Close the ${APP_NAME} side panel.`, closed);
    }
  }

  async function openSettingsTab() {
    if (await cdp.targetFor(settingsUrl())) return;
    await cdp.send("Target.createTarget", { url: settingsUrl() });
  }

  async function settingsConnected() {
    await waitFor(() => onSettings(async (page) =>
      requireCondition(await page.enabled("#choose-folder"), "Settings page is not connected to the host")));
  }

  async function fixtureTarget(origin) {
    const target = (await cdp.targets()).find((entry) => entry.type === "page" && entry.url.startsWith(`${origin}/`));
    requireCondition(target, "The fixture tab is not open");
    return target;
  }

  async function navigate(origin, url) {
    const target = await fixtureTarget(origin);
    await cdp.withTarget(target.targetId, async (page) => {
      await cdp.send("Page.navigate", { url }, page.sessionId);
      await waitFor(async () => requireCondition(
        await page.evaluate(`location.href === ${JSON.stringify(url)} && document.readyState === "complete"`),
        `Fixture did not finish loading ${url}`));
    });
    // The worker fills a visit's title from the tab's title update.
    await delay(1_500);
  }

  async function pendingVisitsCleared() {
    await waitFor(() => onPanel(async (page) => {
      const summary = await page.text("#queue-summary");
      // Live hour totals are not yet due for delivery; only the queue counts.
      requireCondition(/(^|· )0 pending capture records\.$/.test(summary), `Panel shows: ${summary}`);
    }), { timeout: 20_000 });
  }

  async function settingsStatus() {
    return onSettings((page) => page.text("#status"));
  }

  // --headless: every Chrome and host process must stay invisible, with no
  // on-screen window (menu-bar items included) and no Dock presence.
  async function nothingOnScreen() {
    const listed = await Promise.all([
      commandOutput("/usr/bin/pgrep", ["-g", String(chrome.pid)]).catch(() => ""),
      commandOutput("/usr/bin/pgrep", ["-f", options.host]).catch(() => ""),
    ]);
    const pids = [...new Set(listed.join("\n").split("\n").filter(Boolean))];
    const seen = JSON.parse(await commandOutput(windowCheck, pids));
    requireCondition(seen.windows === 0 && seen.apps === 0,
      `A test process is visible: ${seen.windows} on-screen windows, ${seen.apps} Dock apps ` +
      `(${JSON.stringify(seen.visible)})`);
  }

  // --headless: the host's dialog child records each dialog in shown.jsonl
  // and waits for an answer file. Answer only the dialog the step expects.
  let dialogsShown = 0;
  const dialogLogLines = async () => (await readFile(path.join(dialogs, "shown.jsonl"), "utf8").catch(() => ""))
    .split("\n").filter(Boolean);
  async function scriptedDialog(dialog, reply) {
    const shown = await waitFor(async () => {
      const lines = await dialogLogLines();
      requireCondition(lines.length > dialogsShown, `The ${dialog} dialog has not opened`);
      return JSON.parse(lines[dialogsShown]);
    }, { timeout: 20_000, signal: abort.signal });
    dialogsShown += 1;
    requireCondition(shown.dialog === dialog, `Expected the ${dialog} dialog; the host opened ${shown.dialog}`);
    await nothingOnScreen();
    await atomicWrite(path.join(dialogs, "answer"), `${JSON.stringify({ dialog, reply })}\n`);
    await waitFor(async () => requireCondition(!(await maybeLstat(path.join(dialogs, "answer"))),
      `The host did not take the ${dialog} answer`), { signal: abort.signal });
    return shown.text;
  }

  const dismissPicker = () => options.headless ? scriptedDialog("pick-folder", "canceled") : cancelPicker();
  const pickFolder = (folder) => options.headless
    ? scriptedDialog("pick-folder", `picked:${folder}`) : choosePickerFolder(folder);

  async function typeNote(text) {
    await waitFor(() => onPanel(async (page) =>
      requireCondition(await page.enabled("#note-body"), "The note editor is disabled")));
    await onPanel((page) => page.fill("#note-body", text));
  }

  async function waitForNoteStatus(expected) {
    return waitFor(() => onPanel(async (page) => {
      const shown = await page.text("#note-status");
      requireCondition(shown === expected, `Panel note status: ${shown || "(none yet)"}`);
    }), { timeout: 20_000 });
  }

  function fieldValue(selector) {
    return onPanel((page) => page.evaluate(`document.getElementById(${JSON.stringify(selector)}).value`));
  }

  try {
    await Promise.all([mkdir(notes), mkdir(knowledgeBase), mkdir(profile)]);
    if (options.headless) {
      await mkdir(dialogs, { mode: 0o700 });
      await commandOutput("/usr/bin/swiftc", ["-O", path.join(REPO, "scripts", "smoke-macos-windows.swift"), "-o", windowCheck]);
    }
    // PATH is pinned in every mode, so harness detection finds only the fakes,
    // never a real harness. Headless also clears the environment, so the
    // service and every run get only these names (plus agent.env).
    const { fakeBin, argvLog } = await installFakeHarnesses(runDir);
    const openLog = path.join(runDir, "open.log");
    const hostPath = `${fakeBin}:/usr/bin:/bin`;
    const wrapperBody = options.headless
      ? `exec /usr/bin/env -i HOME=${shellQuote(testHome)} PATH=${shellQuote(hostPath)} ` +
        `${SCRIPTED_DIALOGS_ENV}=${shellQuote(dialogs)} ${OPEN_LOG_ENV}=${shellQuote(openLog)} ` +
        `${shellQuote(options.host)} "$@"\n`
      : `export HOME=${shellQuote(testHome)}\nexport PATH=${shellQuote(hostPath)}\nexec ${shellQuote(options.host)} "$@"\n`;
    await writeFile(wrapper, `#!/bin/sh\n${wrapperBody}`, { mode: 0o700 });
    await chmod(wrapper, 0o700);
    const configPath = await commandOutput(wrapper, ["--config-path"]);
    const actualHome = await realpath(testHome);
    requireCondition(path.isAbsolute(configPath) &&
      (inside(testHome, configPath) || inside(actualHome, configPath)),
    `Host config would escape the temporary home: ${configPath}`);
    const dist = await realpath(DIST);
    extensionId = unpackedExtensionId(dist);
    const hostManifest = Buffer.from(`${JSON.stringify({
      name: NATIVE_HOST_NAME,
      description: DEV_DESCRIPTION,
      path: wrapper,
      type: "stdio",
      allowed_origins: [`chrome-extension://${extensionId}/`],
    }, null, 2)}\n`);
    await atomicWrite(manifest, hostManifest);
    installedManifest = hostManifest;
    console.log(`Predicted unpacked extension ID: ${extensionId}`);
    console.log(`Isolated-profile native host registered for ${extensionId} before Chrome launch.`);

    const origin = await startServer(server, options.defaultPort);
    // The same fixture under a second origin that has no adapter and no
    // starter: Capture Site drafts its rules from the page's shapes.
    const unknownOrigin = origin.replace("://127.0.0.1", "://localhost");
    // The URL typed into settings; the default-port run spells out :80.
    const siteInput = options.defaultPort ? `${origin}:80` : origin;
    const allowed = `${origin}/allowed/page?case=${caseId}`;
    const blocked = `${origin}/blocked/page?case=${caseId}`;
    const whilePaused = `${origin}/allowed/while-paused?case=${caseId}`;
    const afterResume = `${origin}/allowed/after-resume?case=${caseId}`;
    const afterRemoval = `${origin}/allowed/after-removal?case=${caseId}`;
    const freshPage = `${origin}/fresh/page?case=${caseId}`;
    const freshTitle = `${APP_NAME} smoke /fresh/page`;
    // The panel titles a page note from the tab, so this is the fixture page title.
    const title = `${APP_NAME} smoke /allowed/page`;
    const firstBody = `First note ${caseId}`;
    await writeFile(path.join(runDir, "run-info.json"), `${JSON.stringify({
      origin, allowed, blocked, afterRemoval, notes, profile, testHome, configPath,
      manifest, host: options.host, chrome: options.chrome, siteInput, fakeBin, argvLog, openLog,
    }, null, 2)}\n`);

    console.log(`\nSmoke artifacts: ${runDir}`);
    console.log(`Fixture: ${origin}${options.defaultPort ? " (port 80, entered as " + siteInput + ")" : ""}`);
    console.log(`Home folder: ${notes}`);
    console.log(`Extension folder: ${dist}`);
    console.log(`Loading the unpacked extension into the isolated ${options.headless ? "headless " : ""}Chrome profile.`);
    chrome = spawn(options.chrome, [
      ...(options.headless ? ["--headless=new"] : []),
      `--user-data-dir=${profile}`, "--no-first-run", "--no-default-browser-check",
      "--remote-debugging-pipe", "--enable-unsafe-extension-debugging",
      // Headless Chrome accepts one start page; it opens chrome://extensions later.
      ...(options.headless ? [] : ["chrome://extensions"]), `${origin}/`,
    ], { detached: true, stdio: ["ignore", "ignore", "ignore", "pipe", "pipe"] });
    await new Promise((resolve, reject) => {
      chrome.once("spawn", resolve);
      chrome.once("error", reject);
    });

    cdp = new Cdp(chrome);
    const loaded = await cdp.send("Extensions.loadUnpacked", { path: dist });
    const loadedId = loaded?.id;
    requireCondition(loadedId === extensionId,
      `Chrome loaded extension ID ${loadedId ?? "none"}; expected ${extensionId}`);
    await waitFor(async () => {
      const discovered = await extensionIdsInProfile(profile, dist);
      requireCondition(discovered.length === 1 && discovered[0] === extensionId,
        `Expected ${extensionId} for ${dist} in this isolated Chrome profile; found ${discovered.join(", ") || "none"}`);
    });
    console.log(`Loaded and confirmed isolated Chrome profile extension ID ${extensionId}.`);
    await nativeHostPreflight(cdp, extensionId);
    console.log("Chrome-to-native-host hello passed.");
    console.log(`The runner drives ${APP_NAME}'s pages itself. Answer only the prompts it names; press Ctrl+C to stop.`);

    if (options.headless) await nothingOnScreen();
    // Any empty folder will do; the suggested one is only a convenience.
    // Later checks count files, so the chosen folder must start empty.
    let notesRoot = null;
    const sameFolder = async (value) => value && notesRoot !== null &&
      await realpath(value).catch(() => null) === notesRoot;

    await step("First-run setup warning", async () => {
      await openPanel();
      await waitFor(() => onPanel(async (page) => {
        requireCondition(await page.hidden("#setup-warning") === false, "Setup warning is hidden");
        const reason = await page.text("#setup-reason");
        requireCondition(reason === "No home folder is chosen.", `Setup warning says: ${reason}`);
      }));
      const reply = await hostConfig(wrapper);
      requireCondition(reply.config.storage === null && !reply.config.capture_enabled,
        "The isolated host configuration is not in its first-run state");
      await onPanel((page) => page.click("#open-settings"));
      await waitFor(async () => requireCondition(await cdp.targetFor(settingsUrl()), "The gear did not open settings"));
      await settingsConnected();
    });

    await step("First-run folder cancellation", async () => {
      await inSettings((page) => page.click("#choose-folder"));
      await userAction("In the macOS folder picker, click Cancel.", async () => {
        const status = await settingsStatus();
        if (await onSettings((page) => page.text("#folder-path")) !== "None selected") {
          throw Object.assign(new Error("A folder was chosen, but this step needs Cancel. Rerun the smoke test."), { fatal: true });
        }
        requireCondition(status === "Folder selection was canceled. No settings were changed.", `Settings page says: ${status}`);
        await onSettings(async (page) => {
          requireCondition(await page.text("#folder-path") === "None selected", "A folder is shown as selected");
          requireCondition(await page.enabled("#choose-folder"), "Picker is still open");
        });
      }, dismissPicker);
      const reply = await hostConfig(wrapper);
      requireCondition(reply.config.storage === null && !reply.config.capture_enabled &&
        reply.config.adapters.artifacts.length === 0 && !("sites" in reply.config),
        "The isolated host configuration changed after picker cancellation");
      requireCondition(!(await maybeLstat(configPath)), "A config file appeared after picker cancellation");
    });

    await step("Choosing a folder saves it before any adapter", async () => {
      // The clipboard is the user's; only a person choosing by hand needs it.
      if (!options.headless) await commandOutput("/bin/sh", ["-c", `printf %s ${shellQuote(notes)} | pbcopy`]);
      await inSettings((page) => page.click("#choose-folder"));
      await userAction(`In the folder picker, choose any empty folder. The suggested one's path is on the clipboard: press Cmd+Shift+G, clear the box, paste, press Return, then click Open.\n  Path: ${notes}`,
        async () => {
          const reply = await hostConfig(wrapper);
          const saved = reply.config.storage?.root;
          requireCondition(saved, "The host has no home folder yet");
          const chosen = await realpath(saved);
          const entries = (await readdir(chosen)).filter((name) => name !== ".DS_Store");
          if (notesRoot === null && entries.length) {
            throw Object.assign(new Error(`${chosen} is not empty; rerun and choose an empty folder`), { fatal: true });
          }
          notesRoot = chosen;
          requireCondition(!reply.config.capture_enabled && reply.config.adapters.artifacts.length === 0,
            "Saving the folder changed adapters or the capture switch");
          const { root: _root, knowledge_base_dir: _kb, ...layout } = reply.config.storage;
          requireCondition(JSON.stringify(layout) === JSON.stringify({
            profile: "neutral", log_dir: "logs", pages_dir: "notes", later_dir: "later",
            artifacts_dir: "artifacts", summaries_dir: "summaries",
          }), `The host saved the layout ${JSON.stringify(layout)}`);
          const status = await settingsStatus();
          requireCondition(status?.startsWith("No site has an adapter yet."), `Settings page says: ${status}`);
        }, () => pickFolder(notes));
      console.log(`  Using home folder ${notesRoot}`);
      // Settings shows the fixed layout and offers no folder names to edit.
      await onSettings(async (page) => {
        const shown = await page.text("#home-layout");
        for (const folder of ["notes/", "logs/", "artifacts/", "summaries/", "later/"]) {
          requireCondition(shown?.includes(folder), `Settings shows the layout as: ${shown}`);
        }
        requireCondition(await page.evaluate('document.getElementById("summaries-dir") === null'),
          "Settings still offers a summaries folder name");
      });
      // Having no adapters is a choice, not a setup problem.
      await waitFor(() => onPanel(async (page) =>
        requireCondition(await page.hidden("#setup-warning") === true, "Panel still shows the setup warning")));
    });

    await step("Knowledge base folder is chosen with the native picker", async () => {
      if (!options.headless) await commandOutput("/bin/sh", ["-c", `printf %s ${shellQuote(knowledgeBase)} | pbcopy`]);
      const expected = await realpath(knowledgeBase);
      let title = null;
      await inSettings(async (page) => {
        await page.evaluate('document.getElementById("status").textContent = ""');
        await page.click("#choose-kb");
      });
      await userAction(`In the folder picker titled "Choose ${APP_NAME} knowledge base folder", choose the suggested folder. Its path is on the clipboard: press Cmd+Shift+G, clear the box, paste, press Return, then click Open.\n  Path: ${knowledgeBase}`,
        async () => {
          const reply = await hostConfig(wrapper);
          const saved = reply.config.storage?.knowledge_base_dir;
          requireCondition(saved, "The host has no knowledge base folder yet");
          requireCondition(await realpath(saved) === expected, `The host saved ${saved} as the knowledge base`);
          requireCondition(await sameFolder(reply.config.storage.root), "Choosing the knowledge base changed the home folder");
          const status = await settingsStatus();
          requireCondition(status?.startsWith("Knowledge base folder saved."), `Settings page says: ${status}`);
          const shown = await onSettings((page) => page.text("#kb-path"));
          requireCondition(shown === saved, `The settings page shows the knowledge base as ${shown}`);
        }, async () => { title = await pickFolder(knowledgeBase); });
      if (options.headless) {
        requireCondition(title === `Choose ${APP_NAME} knowledge base folder`, `The picker was titled ${title}`);
      }
      const written = (await readdir(knowledgeBase)).filter((name) => name !== ".DS_Store");
      requireCondition(written.length === 0, `The host wrote into the knowledge base folder: ${written.join(", ")}`);
    });

    // Standing HTTP(S) access: with no site enabled and nothing granted during
    // the run, the panel still sees a page it has never shown before.
    await step("Panel shows URL and title on a never-visited page", async () => {
      await navigate(origin, freshPage);
      await cdp.send("Target.activateTarget", { targetId: (await fixtureTarget(origin)).targetId });
      await openPanel();
      await waitFor(() => onPanel(async (page) => {
        const [shownUrl, shownTitle] = await Promise.all([page.text("#page-url"), page.text("#page-title")]);
        requireCondition(shownUrl === freshPage, `Panel shows the URL: ${shownUrl || "(none)"}`);
        requireCondition(shownTitle === freshTitle, `Panel shows the title: ${shownTitle || "(none)"}`);
        requireCondition(await page.hidden("#site-access-warning") === true, "Panel shows the site-access banner");
        requireCondition(await page.hidden("#page-unusable") === true, "Panel says it cannot use this tab");
        requireCondition(await page.hidden("#note-editor") === false, "The note editor is hidden");
        requireCondition(await page.enabled("#note-body"), "The note editor is disabled");
      }), { timeout: 20_000 });
      const pages = await markdownFiles(path.join(notesRoot, "notes"));
      requireCondition(pages.length === 0, "Showing a page's note editor wrote a note");
    });

    await step("Confirmed setup", async () => {
      // A site is on once it has an adapter. The fixture origin has no starter
      // draft, so this saves Add adapter's template for the typed address.
      await onSettings(async (page) => {
        await page.fill("#site-origin", siteInput);
      });
      await waitFor(() => onSettings(async (page) =>
        requireCondition(await page.enabled("#new-adapter"), "Add adapter is disabled")));
      await inSettings((page) => page.click("#new-adapter"));
      await waitFor(() => onSettings(async (page) =>
        requireCondition(await page.enabled("#save-adapter"), "The adapter draft cannot be saved")));
      await inSettings(async (page) => {
        // Clear the previous step's result so only a new failure stops the run.
        await page.evaluate('document.getElementById("status").textContent = ""');
        await page.click("#save-adapter");
      });
      // Saving an adapter asks Chrome for nothing: the extension already holds
      // HTTP(S) access, so the setting is saved straight to the host.
      await waitFor(async () => {
        const status = await settingsStatus().catch(() => null);
        if (status?.startsWith("Could not save artifact adapters")) {
          throw Object.assign(new Error(`${status} Rerun the smoke test.`), { fatal: true });
        }
        const reply = await hostConfig(wrapper);
        const selectedRoot = reply.config.storage?.root;
        requireCondition(await sameFolder(selectedRoot),
          `Expected ${notesRoot}; host has ${selectedRoot ?? "none"}`);
        requireCondition(reply.config.capture_enabled === true, "Capture is not enabled in the host");
        requireCondition(!("sites" in reply.config), "The host still sends a v5 site list");
        requireCondition(reply.config.adapters.artifacts.length === 1 &&
          reply.config.adapters.artifacts[0].origin === origin, "The host has no adapter for the fixture origin");
      }, { timeout: 30_000, interval: 500, signal: abort.signal });
      await waitFor(async () => {
        const status = await settingsStatus();
        requireCondition(status?.endsWith("adapter added."), `Settings page says: ${status}`);
        const sites = await onSettings((page) => page.text("#sites-list"));
        requireCondition(sites?.includes(origin), `The site list says: ${sites}`);
      }, { timeout: 20_000 });
      await openPanel();
      await waitFor(() => onPanel(async (page) =>
        requireCondition(await page.hidden("#setup-warning") === true, "Panel still shows the setup warning")),
      { timeout: 20_000 });
    });

    await step("Confirmed adapter becomes the worker and host capture boundary", async () => {
      const current = await hostConfig(wrapper);
      const next = structuredClone(current.config);
      next.adapters = {
        format: 1,
        artifacts: [{
          type: "smoke.page",
          origin,
          match: [{ path: "/allowed/{page:token}" }],
          id: "smoke:{host}/{page}",
        }],
      };
      // The smoke runner sends the revision-checked update_config directly
      // from the extension page. The same page then reloads and publishes
      // the new revision into the worker's lease.
      const saving = inSettings((page) => page.evaluate(`(() => new Promise((resolve, reject) => {
        const port = chrome.runtime.connectNative(${JSON.stringify(NATIVE_HOST_NAME)});
        const expectedRevision = ${JSON.stringify(current.revision)};
        const config = ${JSON.stringify(next)};
        const updateId = crypto.randomUUID();
        port.onDisconnect.addListener(() => reject(new Error(chrome.runtime.lastError?.message ?? "Native host disconnected")));
        port.onMessage.addListener((message) => {
          if (message.type === "error") {
            reject(new Error(message.message));
            port.disconnect();
          } else if (message.type === "config_updated" && message.request_id === updateId) {
            resolve(message.revision);
            port.disconnect();
          }
        });
        port.postMessage({
          type: "update_config", protocol_version: ${PROTOCOL_VERSION}, request_id: updateId,
          expected_revision: expectedRevision, config, picker_token: null, knowledge_base_token: null,
        });
      }))()`));
      await saving;
      const reply = await hostConfig(wrapper);
      requireCondition(reply.config.adapters.artifacts.length === 1 &&
        reply.config.adapters.artifacts[0].type === "smoke.page",
      "The fixture adapter was not saved");
      await inSettings((page) => cdp.send("Page.reload", {}, page.sessionId));
      await settingsConnected();
    });

    const logRows = async () => {
      const files = await markdownFiles(path.join(notesRoot, "logs"));
      return { files, rows: files.flatMap((file) => file.text.split("\n").filter((line) => line.startsWith("- "))) };
    };

    // An activity entry is a "- " row naming the URL, followed by its
    // `<!-- brauser:visit ... kind=activity data=... -->` marker line.
    const activityRows = (rows, url) => rows.filter((line) => line.includes(`<${url}>`));
    const logs = async () => (await markdownFiles(path.join(notesRoot, "logs"))).map((file) => file.text);
    const activityMarkers = async () => (await logs()).join("\n").split("\n")
      .filter((line) => line.startsWith("<!-- brauser:visit ") && line.includes(" kind=activity ")).length;

    // The worker's live hourly totals (DESIGN.md §5.1), read from the trusted
    // settings page, which shares chrome.storage.local with the worker.
    const activityTotals = () => onSettings((page) => page.evaluate(
      `chrome.storage.local.get(${JSON.stringify(ACTIVITY_TOTALS_KEY)})
        .then((stored) => stored[${JSON.stringify(ACTIVITY_TOTALS_KEY)}]?.totals ?? [])`));

    // Totals reach the log only after their local hour ends. Rather than wait
    // an hour, relabel the live totals in this disposable profile as the
    // previous hour (same UTC offset), so the worker's own freeze-and-deliver
    // path treats them as completed.
    const rewindActivityHour = () => onSettings((page) => page.evaluate(`(async () => {
      const key = ${JSON.stringify(ACTIVITY_TOTALS_KEY)};
      const stored = (await chrome.storage.local.get(key))[key];
      if (!stored?.totals?.length) return 0;
      const earlier = (hour) => {
        const offset = hour.slice(19);
        const sign = offset[0] === "-" ? -1 : 1;
        const minutes = sign * (Number(offset.slice(1, 3)) * 60 + Number(offset.slice(4, 6)));
        const shifted = new Date(Date.parse(hour) - 3_600_000 + minutes * 60_000);
        return shifted.toISOString().slice(0, 19) + offset;
      };
      for (const total of stored.totals) total.hour_start = earlier(total.hour_start);
      await chrome.storage.local.set({ [key]: stored });
      return stored.totals.length;
    })()`));

    await step("The worker measures allowed activity with the panel closed", async () => {
      await closePanel();
      await navigate(origin, allowed);
      await navigate(origin, blocked);
      await openSettingsTab();
      await waitFor(async () => {
        const totals = await activityTotals();
        const forAllowed = totals.filter((total) => total.url === allowed);
        requireCondition(forAllowed.length === 1 && forAllowed[0].opens === 1,
          `Expected one open of the allowed page in the hourly totals; found ${JSON.stringify(forAllowed)}`);
        requireCondition(!totals.some((total) => total.url.includes("/blocked/")),
          "The blocked path was measured");
      }, { timeout: 20_000, interval: 1_000 });
    });

    await step("A completed hour of activity reaches the daily log", async () => {
      await openPanel();
      // Retried because the worker may rewrite its totals between our read and
      // write; each attempt relabels whatever is live, then asks for a sync.
      await waitFor(async () => {
        const { files, rows } = await logRows();
        if (activityRows(rows, allowed).length === 0) {
          await rewindActivityHour();
          await onPanel((page) => page.evaluate(`chrome.runtime.sendMessage({ kind: "sync_now" })`));
        }
        const latest = await logRows();
        requireCondition(activityRows(latest.rows, allowed).length === 1,
          "Expected exactly one activity entry for the allowed page in the daily log");
        requireCondition(await activityMarkers() === 1,
          "Expected the allowed page's entry to carry exactly one activity marker");
        requireCondition(!files.some((file) => file.text.includes(blocked)),
          "Blocked-path activity appeared in the daily log");
      }, { timeout: 60_000, interval: 2_000 });
      await pendingVisitsCleared();
    });

    await step("Panel reopen does not duplicate the activity entry", async () => {
      await closePanel();
      await openPanel();
      await pendingVisitsCleared();
      await delay(1_000);
      const { rows } = await logRows();
      requireCondition(activityRows(rows, allowed).length === 1,
        "Allowed activity entry was duplicated or removed");
    });

    await step("Note autosave writes the whole note about a second after typing stops", async () => {
      await navigate(origin, allowed);
      await cdp.send("Target.activateTarget", { targetId: (await fixtureTarget(origin)).targetId });
      await openPanel();
      await typeNote(firstBody);
      // Autosave debounces ~1s after typing stops (§5.3); 20s covers that plus
      // the native round trip with room to spare.
      await waitForNoteStatus("Saved");
      const files = await markdownFiles(path.join(notesRoot, "notes"));
      requireCondition(files.length === 1, "Expected exactly one page note");
      requireCondition(files[0].text.includes(title) && files[0].text.includes(firstBody) &&
        files[0].text.includes(allowed), "Page note content does not match the fixture page");
    });

    await step("The note persists and reloads across navigating the page away and back", async () => {
      const appended = `${firstBody} plus an edit before leaving the page ${caseId}`;
      await typeNote(appended);
      await waitForNoteStatus("Saved");
      // The panel follows the active tab (§5.7): leaving and returning to
      // the page must flush any edit and load the saved note again.
      await navigate(origin, blocked);
      await navigate(origin, allowed);
      await waitForNoteStatus("Saved");
      const shown = await fieldValue("note-body");
      requireCondition(shown === appended, `Note editor shows: ${shown}`);
      const files = await markdownFiles(path.join(notesRoot, "notes"));
      requireCondition(files.length === 1 && files[0].text.includes(appended),
        "Navigating away and back did not preserve the saved note");
    });

    let externalBody;
    await step("A refused stale save shows the current note and keeps the unsaved text", async () => {
      const loaded = await hostRequest(wrapper, {
        type: "load_note", protocol_version: PROTOCOL_VERSION, request_id: randomUUID(), url: allowed,
      });
      requireCondition(loaded.type === "note_loaded" && loaded.exists,
        `Expected the note saved above to load; got ${loaded.type}`);

      // Simulate a second Brauser window saving this page's note first, so
      // the open panel's own revision becomes stale (§4.4).
      externalBody = `External edit ${caseId}`;
      await hostSaveNote(wrapper, allowed, title, externalBody, loaded.revision, loaded.key);

      const unsavedLocalEdit = `${firstBody} plus an edit that cannot be saved ${caseId}`;
      await typeNote(unsavedLocalEdit);
      await waitFor(() => onPanel(async (page) => {
        const hidden = await page.hidden("#note-conflict");
        requireCondition(hidden === false, "The conflict copy-out box did not appear");
      }), { timeout: 20_000 });
      const [unsaved, shown] = await Promise.all([fieldValue("note-unsaved"), fieldValue("note-body")]);
      requireCondition(unsaved === unsavedLocalEdit, `Unsaved copy-out shows: ${unsaved}`);
      requireCondition(shown === externalBody, `Note editor shows: ${shown}`);
      const files = await markdownFiles(path.join(notesRoot, "notes"));
      requireCondition(files.length === 1 && files[0].text.includes(externalBody) &&
        !files[0].text.includes(unsavedLocalEdit), "The refused save must not have changed the note file");
    });

    await step("Editing again after a conflict saves against the now-current revision", async () => {
      const recovered = `${externalBody} plus the recovered edit ${caseId}`;
      await typeNote(recovered);
      await waitForNoteStatus("Saved");
      const files = await markdownFiles(path.join(notesRoot, "notes"));
      requireCondition(files.length === 1 && files[0].text.includes(recovered),
        "The recovered edit was not saved after the conflict");
    });

    await step("Pausing from the panel turns capture off and offers Resume in settings", async () => {
      await openPanel();
      await waitFor(() => onPanel(async (page) =>
        requireCondition(await page.enabled("#pause-capture"), "The panel's pause button is disabled")));
      // The pause button sits in the collapsed capture strip, as a user finds it.
      await onPanel(async (page) => {
        if (!(await page.evaluate('document.getElementById("capture-strip").open'))) {
          await page.click("#capture-strip-summary");
        }
        await page.click("#pause-capture");
      });
      await waitFor(async () => requireCondition((await hostConfig(wrapper)).config.capture_enabled === false,
        "Host still has capture enabled"), { timeout: 20_000, interval: 1_000 });
      // A paused setup is complete, so no setup warning (DESIGN.md §5.7).
      await waitFor(() => onPanel(async (page) => {
        requireCondition(await page.hidden("#setup-warning") === true, "Panel shows a setup warning while paused");
        const strip = await page.text("#capture-strip-summary");
        requireCondition(strip?.includes("Capture off"), `Capture strip says: ${strip}`);
        const status = await page.text("#status");
        requireCondition(/Resume (capture|it) in settings/.test(status ?? ""), `Panel status says: ${status}`);
      }), { timeout: 20_000 });
      await openSettingsTab();
      await settingsConnected();
      await waitFor(() => onSettings(async (page) => {
        requireCondition(await page.hidden("#resume-capture") === false, "Settings does not show Resume capture");
        requireCondition(await page.enabled("#resume-capture"), "Resume capture is disabled");
      }), { timeout: 20_000 });
      // Pausing keeps the site's adapter; only the capture switch is off.
      requireCondition((await hostConfig(wrapper)).config.adapters.artifacts.length > 0,
        "Pausing removed the site's adapter");
      await closePanel();
      await navigate(origin, whilePaused);
      await openSettingsTab();
      await delay(1_000);
      requireCondition(!(await activityTotals()).some((total) => total.url === whilePaused),
        "Activity was measured while capture was paused");
    });

    await step("Resume capture turns capture on with no dialog, and captures again", async () => {
      const dialogsBefore = (await dialogLogLines()).length;
      await inSettings(async (page) => {
        await page.evaluate('document.getElementById("status").textContent = ""');
        await page.click("#resume-capture");
      });
      await waitFor(async () => {
        requireCondition((await hostConfig(wrapper)).config.capture_enabled === true,
          "Capture is not enabled in the host");
        const status = await settingsStatus();
        requireCondition(status?.startsWith("Capturing on sites with adapters."), `Settings page says: ${status}`);
      }, { timeout: 20_000, interval: 1_000 });
      // From protocol v6, resuming saves capture back on with no native confirmation.
      requireCondition((await dialogLogLines()).length === dialogsBefore, "Resume capture opened a native dialog");
      await waitFor(() => onSettings(async (page) =>
        requireCondition(await page.hidden("#resume-capture"), "Settings still shows Resume capture")));
      await openPanel();
      await waitFor(() => onPanel(async (page) => {
        const strip = await page.text("#capture-strip-summary");
        requireCondition(strip?.includes("Capture on"), `Capture strip says: ${strip}`);
      }), { timeout: 20_000 });
      await closePanel();
      await navigate(origin, afterResume);
      await openSettingsTab();
      await waitFor(async () => {
        const forResumed = (await activityTotals()).filter((total) => total.url === afterResume);
        requireCondition(forResumed.length === 1 && forResumed[0].opens === 1,
          `Expected one open of the resumed page in the hourly totals; found ${JSON.stringify(forResumed)}`);
      }, { timeout: 20_000, interval: 1_000 });
      await openPanel();
      await waitFor(async () => {
        if (activityRows((await logRows()).rows, afterResume).length === 0) {
          await rewindActivityHour();
          await onPanel((page) => page.evaluate(`chrome.runtime.sendMessage({ kind: "sync_now" })`));
        }
        const { files, rows } = await logRows();
        requireCondition(activityRows(rows, afterResume).length === 1,
          "Expected exactly one activity entry for the resumed page in the daily log");
        requireCondition(!files.some((file) => file.text.includes(whilePaused)),
          "Activity from while capture was paused reached the daily log");
      }, { timeout: 60_000, interval: 2_000 });
      await pendingVisitsCleared();
    });

    // The agent service and summary runs, headless only: a normal build would
    // ask launchd for the service, and only the scripted opener writes
    // open.log. The smoke starts `service` itself through the wrapper's
    // cleared environment. Harness setup runs in the service, so it starts
    // first; setup shows no dialog, and runs only the fakes in fake-bin.
    const support = path.dirname(configPath);
    const runDirOf = (runId) => path.join(support, "runs", runId);
    const setAgentMode = (mode) => atomicWrite(path.join(testHome, "fake-agent.mode"), `${mode}\n`);
    const agentRequest = (fields) => hostRequest(wrapper, {
      protocol_version: PROTOCOL_VERSION, request_id: randomUUID(), ...fields,
    });
    const startRun = (url) => agentRequest({
      type: "start_run", url, title: `${APP_NAME} smoke run`, content: `# Smoke run\n\n${caseId}\n`,
      selection: null, harness: null, record_key: null, requested_at: requestedAt(),
    });
    const getRun = (runId) => agentRequest({ type: "get_run", run_id: runId, since_offset: 0 });
    const runIn = (runId, states) => waitFor(async () => {
      const reply = await getRun(runId);
      requireCondition(reply.type === "run_fetched", `get_run answered ${JSON.stringify(reply)}`);
      requireCondition(states.includes(reply.run.state), `Run ${runId} is ${reply.run.state}`);
      return reply;
    }, { timeout: 30_000, interval: 300, signal: abort.signal });
    const failPage = `${origin}/run/fail?case=${caseId}`;
    const article = `${origin}/article?case=${caseId}`;
    const runIds = {};
    const noNewDialog = async (before) => requireCondition((await dialogLogLines()).length === before,
      "Harness setup opened a native dialog");
    const settledStatus = (prefix) => waitFor(async () => {
      const status = await settingsStatus();
      requireCondition(status?.startsWith(prefix), `Settings page says: ${status}`);
      return status;
    }, { timeout: 60_000 });

    // The settings page's harness list, as the service detected it.
    const harnessRows = () => onSettings((page) => page.evaluate(`[...document.querySelectorAll("#harness-list li[data-harness]")]
      .map((row) => ({ harness: row.dataset.harness, ready: row.dataset.ready === "true",
        text: row.querySelector("label span")?.textContent ?? "", binary: row.querySelector("small")?.textContent.trim() || null }))`));
    // Never click Test and use unless every harness found is one of the fakes.
    async function onlyFakeHarnesses() {
      const rows = await harnessRows();
      const fakeDirs = [fakeBin, await realpath(fakeBin)];
      const stray = rows.find((row) => row.binary !== null && !fakeDirs.some((dir) => inside(dir, row.binary)));
      if (rows.length !== 2 || stray) {
        throw Object.assign(new Error(`A harness outside the fakes was found: ${JSON.stringify(stray ?? rows)}`),
          { fatal: true });
      }
      return rows;
    }
    async function detect() {
      await inSettings(async (page) => {
        await page.evaluate('document.getElementById("status").textContent = ""');
        await page.click("#detect-harnesses");
      });
      await waitFor(async () => {
        const status = await settingsStatus();
        requireCondition(status?.startsWith("Found "), `Settings page says: ${status}`);
      }, { timeout: 60_000 });
      return onlyFakeHarnesses();
    }
    // Choose `harness` and run Test and use; resolves with the settled status.
    async function testAndUse(harness, prefix) {
      await onlyFakeHarnesses();
      await inSettings(async (page) => {
        await page.click(`#harness-list li[data-harness="${harness}"] input[type="radio"]`);
        await page.evaluate('document.getElementById("status").textContent = ""');
        await page.click("#test-harness");
      });
      return settledStatus(prefix);
    }
    const isFakeBinary = async (binary, name) =>
      [path.join(fakeBin, name), path.join(await realpath(fakeBin), name)].includes(binary);

    // The panel's run cards. Each re-renders its buttons on every poll, so a
    // button is found and clicked in one evaluation; none of them needs a
    // user gesture.
    const runCards = () => onPanel((page) => page.evaluate(`[...document.querySelectorAll("#run-area .run-card")]
      .map((card) => ({ id: card.dataset.runId, state: card.querySelector(".run-card-state")?.textContent ?? "",
        answer: card.querySelector(".run-card-answer")?.textContent ?? "",
        buttons: [...card.querySelectorAll(".run-card-actions button")].map((button) => button.textContent) }))`));
    const clickCardButton = (runId, label) => waitFor(() => onPanel((page) => page.evaluate(`(() => {
      const card = document.querySelector(${JSON.stringify(`#run-area .run-card[data-run-id="${runId}"]`)});
      const button = [...(card?.querySelectorAll(".run-card-actions button") ?? [])]
        .find((candidate) => candidate.textContent === ${JSON.stringify(label)} && !candidate.disabled);
      if (!button) throw new Error(${JSON.stringify(`The run card has no ${label} button`)});
      button.click();
    })()`)));
    const cardIn = (runId, states, timeout = 60_000) => waitFor(async () => {
      const card = (await runCards()).find((candidate) => candidate.id === runId);
      requireCondition(card, `The panel shows no card for run ${runId}`);
      requireCondition(states.some((state) => card.state.startsWith(state)), `The run card says: ${card.state}`);
      return card;
    }, { timeout, interval: 500, signal: abort.signal });
    const finished = ["Summarized", "Failed", "Canceled"];
    // Follow the article tab in a freshly opened panel, so it reads the
    // harness state from the host again.
    async function panelOnArticle() {
      await closePanel();
      await cdp.send("Target.activateTarget", { targetId: (await fixtureTarget(origin)).targetId });
      await openPanel();
      await waitFor(() => onPanel(async (page) => {
        const shown = await page.text("#page-url");
        requireCondition(shown === article, `Panel shows the URL: ${shown || "(none)"}`);
        requireCondition(await page.enabled("#summarize"), "Summarize & log is disabled");
      }), { timeout: 20_000 });
    }
    // Click Summarize & log; resolves with the new card's run id.
    async function summarizeFromPanel() {
      const before = new Set((await runCards()).map((card) => card.id));
      await onPanel((page) => page.click("#summarize"));
      return waitFor(async () => {
        const fresh = (await runCards()).find((card) => !before.has(card.id));
        requireCondition(fresh, "Summarize & log showed no new run card");
        return fresh.id;
      }, { timeout: 30_000, signal: abort.signal });
    }
    const newRuns = async (since) => (await harnessRuns(argvLog)).slice(since).filter((run) => !isVersionCheck(run));
    const anyArgvHasArticle = (runs) => runs.some((run) => run.argv.some((arg) => arg.includes(articleText(caseId))));

    if (!options.headless) {
      console.log("\nAgent service and summary steps skipped: they run with --headless only.");
    } else {
      await step("Without the service, a run is service_unavailable", async () => {
        await setAgentMode("ok");
        const reply = await startRun(failPage);
        requireCondition(reply.type === "error" && reply.code === "service_unavailable",
          `start_run answered ${JSON.stringify(reply)}`);
        requireCondition(!(await maybeLstat(path.join(support, "service.sock"))), "A service socket appeared");
      });

      await step("The service starts and listens", async () => {
        const log = await open(path.join(runDir, "service.log"), "a", 0o600);
        try {
          service = spawn(wrapper, ["service"], { stdio: ["ignore", log.fd, log.fd] });
        } finally {
          await log.close();
        }
        service.on("error", () => undefined);
        await waitFor(async () => {
          requireCondition(service.exitCode === null && service.signalCode === null,
            `The service exited early (${service.exitCode ?? service.signalCode}); see service.log`);
          const socket = await maybeLstat(path.join(support, "service.sock"));
          requireCondition(socket?.isSocket(), "service.sock is not there yet");
        }, { timeout: 15_000, interval: 200, signal: abort.signal });
      });

      await step("Harness setup in settings", async () => {
        await openSettingsTab();
        await settingsConnected();
        const warning = await onSettings((page) => page.text("#harness-warning"));
        requireCondition(warning === SETUP_WARNING, `The setup warning says: ${warning}`);
        const rows = await detect();
        const codex = rows.find((row) => row.harness === "codex");
        const claude = rows.find((row) => row.harness === "claude");
        requireCondition(codex?.ready && codex.text === "Codex 0.144.4", `Codex row: ${JSON.stringify(codex)}`);
        requireCondition(claude?.ready && claude.text === "Claude Code 2.1.310", `Claude Code row: ${JSON.stringify(claude)}`);
        const detection = await harnessRuns(argvLog);
        requireCondition(detection.length > 0 && detection.every(isVersionCheck),
          `Detection ran more than --version: ${JSON.stringify(detection.map((run) => [run.program, run.argv]))}`);

        // A failing test run changes nothing.
        const dialogsBefore = (await dialogLogLines()).length;
        const before = await hostConfig(wrapper);
        await setAgentMode("fail");
        const refused = await testAndUse("codex", "The harness test did not pass");
        requireCondition(refused.includes("Codex failed its test run") && refused.includes("fake failure"),
          `Settings page says: ${refused}`);
        const unchanged = await hostConfig(wrapper);
        requireCondition(unchanged.revision === before.revision && unchanged.config.agent === null,
          "A failed test run changed the config");

        await setAgentMode("ok");
        await testAndUse("codex", "Codex is set up.");
        await noNewDialog(dialogsBefore);
        const reply = await hostConfig(wrapper);
        const agent = reply.config.agent;
        requireCondition(agent?.harness === "codex" && await isFakeBinary(agent.binary, "codex") &&
          agent.version === "0.144.4" && agent.timeout_secs === 600,
        `The host config after setup: ${JSON.stringify(agent)}`);
        requireCondition(reply.agent_status.state === "ready" && reply.agent_status.harness_version === "0.144.4",
          `Agent status: ${JSON.stringify(reply.agent_status)}`);
        // The probe ran as a real run would: an app-server thread in the knowledge base.
        const probes = (await harnessRuns(argvLog)).filter(isProbe);
        const knowledgeBaseReal = await realpath(knowledgeBase);
        requireCondition(probes.length === 2 && probes.every((run) => run.program === "codex" &&
          run.argv.join(" ") === "app-server --listen stdio://" && run.cwd === knowledgeBaseReal),
        `Expected two Codex test runs in the knowledge base; got ${JSON.stringify(probes.map((run) => [run.argv, run.cwd]))}`);
      });

      await step("Summarize a page with Codex", async () => {
        await setAgentMode("ok");
        await navigate(origin, article);
        await panelOnArticle();
        const since = (await harnessRuns(argvLog)).length;
        runIds.codex = await summarizeFromPanel();
        const card = await cardIn(runIds.codex, finished);
        requireCondition(card.state.startsWith("Summarized"), `The run card says: ${card.state}`);
        requireCondition(/Importance: high/.test(card.answer), `The run card's answer: ${card.answer}`);
        for (const file of RUN_FILES) {
          requireCondition((await maybeLstat(path.join(runDirOf(runIds.codex), file)))?.isFile(),
            `The run record has no ${file}`);
        }
        const page = await readFile(path.join(runDirOf(runIds.codex), "page.md"), "utf8");
        requireCondition(page.includes(articleText(caseId)), "page.md lacks the article text");
        requireCondition(!page.includes(ARTICLE_NAV_TEXT), "page.md kept the navigation block");
        const runs = await newRuns(since);
        const knowledgeBaseReal = await realpath(knowledgeBase);
        requireCondition(runs.length === 1 && runs[0].program === "codex" &&
          runs[0].argv.join(" ") === "app-server --listen stdio://" && runs[0].cwd === knowledgeBaseReal,
        `Expected one app-server in the knowledge base; got ${JSON.stringify(runs.map((run) => [run.program, run.argv, run.cwd]))}`);
        const thread = runs[0].rpc.find((request) => request.method === "thread/start")?.params;
        requireCondition(thread?.approvalPolicy === "never" && thread.sandbox === "danger-full-access" &&
          await realpath(thread.cwd).catch(() => null) === knowledgeBaseReal,
        `thread/start asked for ${JSON.stringify(thread)}`);
        requireCondition(!anyArgvHasArticle(runs), "Article text reached a command line");
      });

      await step("Open in Codex", async () => {
        await clickCardButton(runIds.codex, "Open in Codex");
        await waitFor(async () => {
          const opened = (await readFile(openLog, "utf8").catch(() => "")).split("\n").filter(Boolean);
          requireCondition(opened.includes(`codex://threads/${FAKE_THREAD}`), `open.log has ${JSON.stringify(opened)}`);
        }, { timeout: 15_000, signal: abort.signal });
      });

      await step("Claude Code and resume", async () => {
        await openSettingsTab();
        await settingsConnected();
        await detect();
        const dialogsBefore = (await dialogLogLines()).length;
        await testAndUse("claude", "Claude Code is set up.");
        await noNewDialog(dialogsBefore);
        const agent = (await hostConfig(wrapper)).config.agent;
        requireCondition(agent?.harness === "claude" && await isFakeBinary(agent.binary, "claude") &&
          agent.version === "2.1.310", `The host config after Claude Code setup: ${JSON.stringify(agent)}`);

        await setAgentMode("badjson-once");
        await panelOnArticle();
        const since = (await harnessRuns(argvLog)).length;
        runIds.claude = await summarizeFromPanel();
        const card = await cardIn(runIds.claude, finished);
        requireCondition(card.state.startsWith("Summarized"), `The run card says: ${card.state}`);
        const runs = await newRuns(since);
        requireCondition(runs.length === 2 && runs.every((run) => run.program === "claude"),
          `Expected exactly two claude runs; got ${JSON.stringify(runs.map((run) => [run.program, run.argv]))}`);
        const [first, second] = runs;
        requireCondition(first.argv.includes("--dangerously-skip-permissions") && first.argv.includes("--session-id") &&
          second.argv.includes("--dangerously-skip-permissions"), `claude ran with ${JSON.stringify([first.argv, second.argv])}`);
        const resume = second.argv.indexOf("--resume");
        requireCondition(resume >= 0 && second.argv[resume + 1] === FAKE_SESSION,
          `The repair turn ran with ${JSON.stringify(second.argv)}`);
        requireCondition(first.stdin?.includes(path.join(runDirOf(runIds.claude), "page.md")) ||
          first.stdin?.includes(path.join(await realpath(runDirOf(runIds.claude)), "page.md")),
        "The prompt on stdin does not name the page file");
        requireCondition(second.stdin?.startsWith(REPAIR_PREFIX), `The repair prompt was ${JSON.stringify(second.stdin)}`);
        requireCondition(!anyArgvHasArticle(runs), "Article text reached a command line");
        // The panel copies through the clipboard; capture what it would copy.
        await onPanel((page) => page.evaluate(`Object.defineProperty(navigator.clipboard, "writeText", {
          configurable: true, value: (text) => { window.smokeCopied = text; return Promise.resolve(); } })`));
        await clickCardButton(runIds.claude, "Copy resume command");
        const copied = await waitFor(() => onPanel(async (page) => {
          const text = await page.evaluate("window.smokeCopied ?? null");
          requireCondition(text !== null, "Nothing was copied");
          return text;
        }));
        const { run } = await getRun(runIds.claude);
        requireCondition(run.session_id === FAKE_SESSION && await realpath(run.cwd) === await realpath(knowledgeBase),
          `The Claude Code run: ${JSON.stringify(run)}`);
        requireCondition(copied === `cd ${shellQuote(run.cwd)} && claude --resume ${FAKE_SESSION}`,
          `Copy resume command copied ${copied}`);
      });

      await step("Second invalid answer", async () => {
        await setAgentMode("badjson");
        runIds.badjson = await summarizeFromPanel();
        const card = await cardIn(runIds.badjson, finished);
        requireCondition(card.state.startsWith("Failed") && card.state.includes("not valid"),
          `The run card says: ${card.state}`);
        requireCondition(card.buttons.includes("Copy") && !card.buttons.includes("Retry save"),
          `The failed card offers ${JSON.stringify(card.buttons)}`);
        const { run } = await getRun(runIds.badjson);
        requireCondition(run.error_kind === "invalid_output", `The run failed as ${run.error_kind}`);
      });

      await step("Cancel", async () => {
        await setAgentMode("slow");
        runIds.slow = await summarizeFromPanel();
        await cardIn(runIds.slow, ["Running"]);
        const group = await waitFor(async () => {
          const status = JSON.parse(await readFile(path.join(runDirOf(runIds.slow), "status.json"), "utf8"));
          requireCondition(Number.isInteger(status.process_group) && status.process_group > 1,
            "The running run has no process group yet");
          return status.process_group;
        }, { timeout: 15_000, interval: 200, signal: abort.signal });
        lingeringGroup = group;
        process.kill(-group, 0);
        // A second start on the same page reattaches to the running run.
        const again = await startRun(article);
        requireCondition(again.type === "run_started" && again.existing === true && again.run.run_id === runIds.slow,
          `A second start on the same page answered ${JSON.stringify(again)}`);
        await clickCardButton(runIds.slow, "Cancel");
        const card = await cardIn(runIds.slow, finished, 20_000);
        requireCondition(card.state.startsWith("Canceled"), `The canceled run card says: ${card.state}`);
        await waitFor(() => {
          try {
            process.kill(-group, 0);
          } catch (error) {
            if (error.code === "ESRCH") return;
            throw error;
          }
          throw new Error(`Process group ${group} is still alive`);
        }, { timeout: 5_000, interval: 100, signal: abort.signal });
        lingeringGroup = null;
      });

      await step("A failing harness fails the run with its stderr", async () => {
        await setAgentMode("fail");
        const started = await startRun(failPage);
        requireCondition(started.type === "run_started", `start_run answered ${JSON.stringify(started)}`);
        runIds.fail = started.run.run_id;
        const done = await runIn(runIds.fail, ["succeeded", "failed", "canceled"]);
        requireCondition(done.run.state === "failed" && done.run.error_kind === "harness_failed" &&
          done.run.error?.includes("fake failure"), `The failing run ended ${JSON.stringify(done.run)}`);
      });

      await step("Restore", async () => {
        await panelOnArticle();
        const card = await waitFor(async () => {
          const shown = await runCards();
          const ids = new Set(shown.map((candidate) => candidate.id));
          requireCondition(Object.values(runIds).every((id) => ids.has(id)),
            `The reopened panel shows ${JSON.stringify([...ids])}; expected ${JSON.stringify(runIds)}`);
          return shown.find((candidate) => candidate.id === runIds.codex);
        }, { timeout: 20_000, signal: abort.signal });
        requireCondition(card.state.startsWith("Summarized") && /Importance: high/.test(card.answer),
          `The restored Codex card: ${JSON.stringify(card)}`);
      });

      await step("list_runs shows every run", async () => {
        const listed = await agentRequest({ type: "list_runs" });
        requireCondition(listed.type === "runs_listed", `list_runs answered ${JSON.stringify(listed)}`);
        const ids = new Set(listed.runs.map((run) => run.run_id));
        requireCondition(Object.values(runIds).length === 5 && Object.values(runIds).every((id) => ids.has(id)),
          `list_runs shows ${JSON.stringify([...ids])}; expected ${JSON.stringify(runIds)}`);
      });

      await step("agent.env", async () => {
        const value = `smoke-agent-value-${caseId}`;
        await atomicWrite(path.join(support, "agent.env"), `SMOKE_AGENT_NAME=${value}\n`, 0o600);
        await openSettingsTab();
        await inSettings((page) => cdp.send("Page.reload", {}, page.sessionId));
        await settingsConnected();
        await waitFor(() => onSettings(async (page) => {
          const names = await page.text("#agent-env-names");
          requireCondition(names?.includes("SMOKE_AGENT_NAME"), `Settings shows agent.env as: ${names}`);
          requireCondition(!(await page.evaluate("document.documentElement.outerHTML")).includes(value),
            "The settings page shows an agent.env value");
        }), { timeout: 20_000 });
      });
    }

    // An unknown site: the host drafts rules from this page's private URL
    // shapes, the panel reviews them in plain words, and one revision-checked
    // update_config saves them. No JSON, no permission prompt, no dialog.
    await step("Capture Site drafts and saves an unknown site", async () => {
      const dialogsBefore = (await dialogLogLines()).length;
      const wikiUrl = `${unknownOrigin}/wiki/pages/101/Smoke-Home`;
      const before = await hostConfig(wrapper);
      requireCondition(!before.config.adapters.artifacts.some((adapter) => adapter.origin === unknownOrigin),
        `The host already has an adapter for ${unknownOrigin}`);
      const { targetId } = await cdp.send("Target.createTarget", { url: wikiUrl });
      await cdp.withTarget(targetId, (page) => waitFor(async () => requireCondition(
        await page.evaluate(`location.href === ${JSON.stringify(wikiUrl)} && document.readyState === "complete"`),
        `The wiki page did not finish loading ${wikiUrl}`)));
      await cdp.send("Target.activateTarget", { targetId });
      await openPanel();
      await waitFor(() => onPanel(async (page) => {
        requireCondition(await page.text("#page-url") === wikiUrl, "The panel does not follow the wiki tab");
        requireCondition(await page.hidden("#capture-site-area") === false, "Capture Site is not offered");
        requireCondition(await page.hidden("#capture-site") === false, "The Capture Site button is hidden");
        requireCondition(await page.enabled("#capture-site"), "The Capture Site button is disabled");
      }), { timeout: 20_000 });
      await onPanel((page) => page.click("#capture-site"));
      const review = await waitFor(() => onPanel(async (page) => {
        const shown = await page.evaluate(`(() => {
          const text = (selector) => document.querySelector(selector)?.textContent ?? "";
          return {
            hidden: document.getElementById("capture-site-review").hidden,
            types: [...document.querySelectorAll("#capture-site-draft input[data-type]")].map((input) => input.value),
            rules: [...document.querySelectorAll("#capture-site-draft input[data-rule]")].map((box) => box.checked),
            kinds: text("#capture-site-draft"),
            preview: text("#capture-site-preview"),
            dropped: [...document.querySelectorAll("#capture-site-dropped > ul li")].map((row) => row.textContent),
            safety: text("#capture-site-dropped details summary"),
            probes: [...document.querySelectorAll("#capture-site-dropped details li")].map((row) => row.textContent),
            message: text("#capture-site-message"),
            save: !document.getElementById("capture-site-save").disabled,
          };
        })()`);
        requireCondition(!shown.hidden && shown.types.length > 0, `The review is not drafted yet: ${shown.message}`);
        return shown;
      }), { timeout: 30_000 });
      // One "pages" kind, its one rule checked, and this page captured.
      requireCondition(review.types.length === 1 && /^localhost\d*\.pages$/.test(review.types[0]),
        `The review drafted the kinds ${JSON.stringify(review.types)}`);
      requireCondition(review.rules.length === 1 && review.rules[0] === true,
        `The drafted rules start as ${JSON.stringify(review.rules)}`);
      requireCondition(review.kinds.includes("Brauser will remember"), `The review says: ${review.kinds}`);
      requireCondition(review.preview.includes(`This page: will be captured — ${wikiUrl}`),
        `The review says of this page: ${review.preview}`);
      // The sibling links match; the search and sign-in links do not.
      requireCondition(review.preview.includes("Matches 1 of 1 visited patterns and 2 of 4 link patterns."),
        `The review counts: ${review.preview}`);
      requireCondition(review.dropped.length === 2 &&
        review.dropped.some((row) => row.endsWith(": search or paging pages")) &&
        review.dropped.some((row) => row.endsWith(": no page ID in the address")),
      `The review's "won't capture" list: ${JSON.stringify(review.dropped)}`);
      requireCondition(review.safety.startsWith("Safety checks:") &&
        [`${unknownOrigin}/login`, `${unknownOrigin}/search?q=x`].every((url) => review.probes.includes(`Skipped: ${url}`)),
      `The review's safety list: ${review.safety} ${JSON.stringify(review.probes)}`);
      requireCondition(review.save, `Save is disabled: ${review.message}`);

      await onPanel(async (page) => {
        await page.evaluate('document.getElementById("status").textContent = ""');
        await page.click("#capture-site-save");
      });
      const saved = await waitFor(async () => {
        const reply = await hostConfig(wrapper);
        const adapter = reply.config.adapters.artifacts.find((entry) => entry.origin === unknownOrigin);
        requireCondition(adapter, `The host has no adapter for ${unknownOrigin}`);
        return { reply, adapter };
      }, { timeout: 30_000, interval: 500, signal: abort.signal });
      const { reply, adapter } = saved;
      requireCondition(reply.config.capture_enabled === true, "Saving the drafted site turned capture off");
      requireCondition(reply.config.adapters.artifacts.some((entry) => entry.origin === origin && entry.type === "smoke.page"),
        "Saving the drafted site changed the configured site's adapter");
      // A drafted adapter holds only the Tier 0 keys (DESIGN.md §5.1).
      const keys = Object.keys(adapter);
      requireCondition(keys.every((key) => ["type", "origin", "match", "id", "modes"].includes(key)) &&
        adapter.type === review.types[0] && Array.isArray(adapter.match) && adapter.match.length === 1 &&
        adapter.match.every((rule) => Object.keys(rule).every((key) => ["path", "query"].includes(key))),
      `The host saved the drafted adapter ${JSON.stringify(adapter)}`);
      await waitFor(() => onPanel(async (page) => {
        const status = await page.text("#status");
        requireCondition(status?.startsWith(`Saved capture rules for ${new URL(unknownOrigin).host}.`),
          `Panel status says: ${status}`);
        requireCondition(await page.hidden("#capture-site-review") === true, "The review is still open");
      }), { timeout: 20_000 });
      // A reloaded panel recognizes the page under the saved rules.
      await onPanel((page) => cdp.send("Page.reload", {}, page.sessionId));
      await waitFor(() => onPanel(async (page) => {
        requireCondition(await page.text("#page-url") === wikiUrl, "The reloaded panel does not follow the wiki tab");
        requireCondition(await page.hidden("#artifact-card") === false, "The reloaded panel shows no artifact card");
      }), { timeout: 20_000 });
      requireCondition((await dialogLogLines()).length === dialogsBefore, "Capture Site opened a native dialog");
      await cdp.send("Target.closeTarget", { targetId });
      await waitFor(async () => requireCondition(
        !(await cdp.targets()).some((entry) => entry.targetId === targetId), "The wiki tab is still closing"));
    });

    await step("Site removal", async () => {
      await openSettingsTab();
      await inSettings((page) => cdp.send("Page.reload", {}, page.sessionId));
      await settingsConnected();
      // The drafted site goes first, so the configured site's removal below
      // still turns capture off.
      await waitFor(() => onSettings(async (page) => requireCondition(
        await page.enabled(`button[aria-label="Remove site ${unknownOrigin}"]`), `Settings does not list ${unknownOrigin}`)),
      { timeout: 20_000 });
      await inSettings((page) => page.click(`button[aria-label="Remove site ${unknownOrigin}"]`));
      await waitFor(async () => {
        const reply = await hostConfig(wrapper);
        requireCondition(!reply.config.adapters.artifacts.some((adapter) => adapter.origin === unknownOrigin),
          "Host still has the drafted site's adapter");
        requireCondition(reply.config.adapters.artifacts.some((adapter) => adapter.origin === origin),
          "Removing the drafted site removed the configured site");
      }, { timeout: 20_000, interval: 1_000 });
      await waitFor(() => onSettings(async (page) => requireCondition(
        await page.enabled(`button[aria-label="Remove site ${origin}"]`), `Settings does not offer removing ${origin}`)),
      { timeout: 20_000 });
      await inSettings((page) => page.click(`button[aria-label="Remove site ${origin}"]`));
      await waitFor(async () => {
        const reply = await hostConfig(wrapper);
        requireCondition(!reply.config.capture_enabled && reply.config.adapters.artifacts.length === 0,
          "Host still has capture enabled or the site's adapter");
        // A capture change echoes the harness unchanged.
        if (options.headless) {
          requireCondition(reply.config.agent !== null, "Site removal dropped the AI harness");
        }
      }, { timeout: 20_000, interval: 1_000 });
      await waitFor(() => onPanel(async (page) => {
        requireCondition(await page.hidden("#setup-warning") === true, "Panel shows a setup warning");
        const strip = await page.text("#capture-strip-summary");
        requireCondition(strip?.includes("Capture off"), `Capture strip says: ${strip}`);
      }), { timeout: 20_000 });
    });

    await step("No capture after removal", async () => {
      await closePanel();
      await navigate(origin, afterRemoval);
      await openPanel();
      await pendingVisitsCleared();
      const { files } = await logRows();
      requireCondition(!files.some((file) => file.text.includes(afterRemoval)),
        "A visit was logged after site removal");
      // The log alone proves little within the hour; the live totals must not
      // have measured the removed site either.
      await openSettingsTab();
      requireCondition(!(await activityTotals()).some((total) => total.url === afterRemoval),
        "Activity was measured after site removal");
    });

    await step("The home folder holds only the fixed folders", async () => {
      const fixed = new Set(["notes", "logs", "artifacts", "summaries", "later"]);
      const entries = (await readdir(notesRoot)).filter((name) => name !== ".DS_Store");
      requireCondition(entries.every((name) => fixed.has(name)),
        `The home folder holds ${entries.join(", ")}`);
      requireCondition(entries.includes("notes") && entries.includes("logs"),
        `The run's notes and logs are not in the home folder: ${entries.join(", ")}`);
    });

    if (options.headless) {
      await step("Nothing reached the screen", async () => {
        const pids = (await commandOutput("/usr/bin/pgrep", ["-f", `${options.host} __dialog-`]).catch(() => ""))
          .split("\n").filter(Boolean);
        requireCondition(pids.length === 0, "A dialog process is still running");
        const logged = (await dialogLogLines()).length;
        requireCondition(dialogsShown === logged,
          `The host opened ${logged} dialogs; the smoke answered ${dialogsShown}`);
        requireCondition(!(await maybeLstat(path.join(dialogs, "answer"))), "An unused scripted answer remains");
        await nothingOnScreen();
      });
    } else if (options.auto) {
      // The busy cursor came from a host process left holding a dialog
      // window, so check that no dialog process or host window remains.
      await step("No host dialog left behind", async () => {
        const pids = (await commandOutput("/usr/bin/pgrep", ["-f", options.host]).catch(() => ""))
          .split("\n").filter(Boolean);
        const commands = await Promise.all(pids.map((pid) =>
          commandOutput("/bin/ps", ["-o", "command=", "-p", pid]).catch(() => "")));
        requireCondition(!commands.some((command) => command.includes("__dialog-")), "A native dialog process is still running");
        const windows = await Promise.all(pids.map(onScreenWindows));
        requireCondition(windows.every((count) => count === 0), "A host process still has a window on screen");
      });
    } else {
      const cursor = await ask(`\nDid a spinning busy cursor stay on screen after any ${APP_NAME} dialog closed? Type no or yes: `);
      requireCondition(cursor.toLowerCase() === "no", "A busy cursor persisted after a native dialog");
    }

    console.log(`\nPASS: ${options.headless ? "headless" : "guided"} macOS Chrome M2 step 1 smoke run completed.`);
  } catch (error) {
    failed = true;
    console.error(`\nSmoke run stopped: ${error.message}`);
  } finally {
    prompt.close();
    process.off("SIGINT", interrupt);
    process.off("SIGTERM", interrupt);
    try { await stopChrome(chrome); } catch (error) {
      failed = true;
      console.error(`Could not stop the isolated Chrome process group: ${error.message}`);
    }
    // The service, then any harness group a failed cancel step left running.
    try { await stopService(service); } catch (error) {
      failed = true;
      console.error(`Could not stop the service: ${error.message}`);
    }
    if (lingeringGroup) {
      try { process.kill(-lingeringGroup, "SIGKILL"); } catch (error) {
        if (error.code !== "ESRCH") {
          failed = true;
          console.error(`Could not stop harness process group ${lingeringGroup}: ${error.message}`);
        }
      }
    }
    try { await rm(testHome, { recursive: true, force: true }); } catch (error) {
      failed = true;
      console.error(`Could not remove the temporary home ${testHome}: ${error.message}`);
    }
    if (installedManifest) {
      try {
        const current = await readFile(manifest).catch((error) => {
          if (error.code === "ENOENT") return null;
          throw error;
        });
        if (!current || !current.equals(installedManifest)) {
          if (current) await writeFile(path.join(runDir, "manifest-found-during-cleanup.json"), current);
          failed = true;
          console.error("Isolated-profile host manifest changed during the run; leaving its current state untouched.");
          console.error(`Inspect ${manifest} manually.`);
        } else {
          await unlink(manifest);
          console.log("Removed the isolated-profile native-host manifest.");
        }
      } catch (error) {
        failed = true;
        console.error(`Could not remove the isolated-profile native-host manifest: ${error.message}`);
      }
    }
    try { await stopServer(server); } catch (error) {
      failed = true;
      console.error(`Could not stop the fixture server: ${error.message}`);
    }
    console.log(`Smoke artifacts preserved: ${runDir}`);
    if (failed) process.exitCode = 1;
  }
}

await run().catch((error) => {
  console.error(error.message);
  process.exitCode = 1;
});
