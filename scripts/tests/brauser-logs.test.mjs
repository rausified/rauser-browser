import assert from "node:assert/strict";
import { appendFile, mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import {
  defaultLogDir, fieldOf, follow, formatEvent, logFileNames, main, malformedSummary,
  matches, mergeEvents, parseArgs, parseExtensionLine, parseHostLine, parseSince, parseText, readAll,
  withoutExportHeader,
} from "../brauser-logs.mjs";

const REQUEST = "0f8e7d6c-1111-4222-8333-444455556666";
const RUN = "a1b2c3d4-5555-4666-8777-888899990000";

// One line as tracing-subscriber's JSON layer writes it from diag.rs.
function hostLine({ ts, level = "INFO", message, fields = {}, spans = [], proc = "host", pid = 41 }) {
  return JSON.stringify({
    timestamp: ts,
    level,
    fields: { message, ...fields },
    target: "brauser::native",
    spans: [{ name: "process", proc, pid, build: "0.0.0-dev" }, ...spans],
  });
}

async function logDir(t) {
  const dir = await mkdtemp(path.join(os.tmpdir(), "brauser-logs-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  return dir;
}

test("default log directory matches diag.rs on each platform", () => {
  assert.equal(defaultLogDir({ env: {}, platform: "darwin", home: "/Users/u" }), "/Users/u/Library/Logs/Brauser");
  assert.equal(defaultLogDir({ env: { LOCALAPPDATA: "C:\\Users\\u\\AppData\\Local" }, platform: "win32", home: "C:\\Users\\u" }),
    "C:\\Users\\u\\AppData\\Local\\Brauser\\data\\logs");
  assert.equal(defaultLogDir({ env: {}, platform: "linux", home: "/home/u" }), "/home/u/.local/share/brauser/logs");
  assert.equal(defaultLogDir({ env: { XDG_DATA_HOME: "/x" }, platform: "linux", home: "/home/u" }), "/x/brauser/logs");
  assert.equal(defaultLogDir({ env: { BRAUSER_LOG_DIR: "/tmp/l" }, platform: "darwin", home: "/Users/u" }), "/tmp/l");
});

test("rotations are read oldest first", () => {
  assert.deepEqual(logFileNames("host"), ["host.jsonl.3", "host.jsonl.2", "host.jsonl.1", "host.jsonl"]);
});

test("a host line takes proc and pid from the process span, and fields from inner spans", () => {
  const event = parseHostLine(hostLine({
    ts: "2026-10-28T12:00:00.123456Z",
    message: "request done",
    fields: { outcome: "ok", elapsed_ms: 12 },
    spans: [{ name: "request", request_id: REQUEST, kind: "start_run" }],
    proc: "service",
    pid: 77,
  }), "host");
  assert.equal(event.proc, "service");
  assert.equal(event.pid, 77);
  assert.equal(event.level, "info");
  assert.equal(event.ts, Date.parse("2026-10-28T12:00:00.123Z"));
  assert.equal(fieldOf(event, "request_id"), REQUEST);
  assert.equal(fieldOf(event, "outcome"), "ok");
  assert.equal(fieldOf(event, "missing"), undefined);
});

test("an event's own fields beyond the key ones are printed after them", () => {
  const line = parseHostLine(hostLine({
    ts: "2026-10-28T12:00:00Z",
    message: "harness exited",
    fields: { exit_code: 1, stderr_bytes: 812, reason: "save failed" },
    spans: [{ name: "request", request_id: REQUEST, outcome: "ok" }],
  }), "host");
  assert.match(formatEvent(line, { utc: true }),
    /harness exited request_id=0f8e7d6c outcome=ok exit_code=1 stderr_bytes=812 reason="save failed"$/);
});

test("malformed lines are counted, blank lines are not", () => {
  const good = hostLine({ ts: "2026-10-28T12:00:00Z", message: "ok" });
  const text = [good, "not json", "", "{\"timestamp\":\"nope\",\"level\":\"INFO\"}", "[1]", good, ""].join("\n");
  const { events, malformed } = parseText(text, (line) => parseHostLine(line, "host"));
  assert.equal(events.length, 2);
  assert.equal(malformed, 3);
  assert.equal(malformedSummary({ "host.jsonl": 3, "x.jsonl": 1 }), "brauser-logs: skipped 4 malformed lines (host.jsonl: 3, x.jsonl: 1)");
  assert.equal(malformedSummary({}), null);
});

test("extension export lines become proc ext with their context", () => {
  const event = parseExtensionLine(JSON.stringify({ ts: "2026-10-28T12:00:01Z", level: "warn", ctx: "worker", event: "native send failed", fields: { request_id: REQUEST, kind: "visit" } }));
  assert.equal(event.proc, "ext");
  assert.equal(event.pid, "worker");
  assert.equal(event.level, "warn");
  assert.equal(formatEvent(event, { utc: true }), "2026-10-28 12:00:01.000Z ext:worker WARN  native send failed kind=visit request_id=0f8e7d6c");
  const numeric = parseExtensionLine(JSON.stringify({ ts: Date.parse("2026-10-28T12:00:02Z"), level: "info", ctx: "panel", event: "x" }));
  assert.equal(numeric.ts, Date.parse("2026-10-28T12:00:02Z"));
  assert.equal(parseExtensionLine("{\"ts\":1}"), null);
});

test("filters match ids on fields or any span, by prefix", () => {
  const inSpan = parseHostLine(hostLine({ ts: "2026-10-28T12:00:00Z", message: "a", spans: [{ name: "run", run_id: RUN }] }), "service");
  const inField = parseHostLine(hostLine({ ts: "2026-10-28T12:00:00Z", message: "b", fields: { request_id: REQUEST } }), "host");
  assert.ok(matches(inSpan, { run: RUN }));
  assert.ok(matches(inSpan, { run: RUN.slice(0, 8) }));
  assert.ok(!matches(inField, { run: RUN }));
  assert.ok(matches(inField, { request: REQUEST.slice(0, 8) }));
  assert.ok(!matches(inSpan, { request: REQUEST }));
  assert.ok(!matches(inField, { level: "warn" }));
  assert.ok(matches(inField, { level: "info", procs: ["host"] }));
  assert.ok(!matches(inField, { procs: ["service"] }));
  assert.ok(!matches(inField, { since: Date.parse("2026-10-28T12:00:01Z") }));
});

test("since takes durations and ISO dates", () => {
  const now = Date.parse("2026-10-28T12:00:00Z");
  assert.equal(parseSince("10m", now), now - 600_000);
  assert.equal(parseSince("2h", now), now - 7_200_000);
  assert.equal(parseSince("30s", now), now - 30_000);
  assert.equal(parseSince("1d", now), now - 86_400_000);
  assert.equal(parseSince("2026-10-28T11:00:00Z", now), Date.parse("2026-10-28T11:00:00Z"));
  assert.throws(() => parseSince("soon", now), /--since/);
});

test("arguments parse, and bad ones are refused", () => {
  const options = parseArgs(["--request", "abc", "--errors", "--proc", "host,ext", "--json"]);
  assert.deepEqual(options.filters, { request: "abc", level: "warn", procs: ["host", "ext"] });
  assert.equal(options.json, true);
  assert.throws(() => parseArgs(["--level", "loud"]), /unknown level/);
  assert.throws(() => parseArgs(["--proc", "chrome"]), /--proc/);
  assert.throws(() => parseArgs(["--run"]), /needs a value/);
  assert.throws(() => parseArgs(["--bogus"]), /unknown option/);
});

test("merging sorts by time and keeps read order for ties", () => {
  const at = (ts, message) => ({ ts, message });
  const merged = mergeEvents([[at(3, "c"), at(1, "a")], [at(2, "b"), at(1, "a2")]]);
  assert.deepEqual(merged.map((event) => event.message), ["a", "a2", "b", "c"]);
});

test("readAll merges processes, rotations, and the extension export by time", async (t) => {
  const dir = await logDir(t);
  await writeFile(path.join(dir, "host.jsonl.1"), `${hostLine({ ts: "2026-10-28T12:00:00Z", message: "first" })}\n`);
  await writeFile(path.join(dir, "host.jsonl"), `${hostLine({ ts: "2026-10-28T12:00:03Z", message: "fourth" })}\ngarbage\n`);
  await writeFile(path.join(dir, "service.jsonl"), `${hostLine({ ts: "2026-10-28T12:00:02Z", message: "third", proc: "service", pid: 9 })}\n`);
  const extension = path.join(dir, "ext.jsonl");
  await writeFile(extension, `${JSON.stringify({ ts: "2026-10-28T12:00:01Z", level: "info", ctx: "panel", event: "second" })}\n`);
  const { events, malformed, files } = await readAll({ dir, extension });
  assert.deepEqual(events.map((event) => event.message), ["first", "second", "third", "fourth"]);
  assert.deepEqual(malformed, { "host.jsonl": 1 });
  assert.equal(files.length, 4);
});

test("main prints compact lines for one request and reports malformed lines", async (t) => {
  const dir = await logDir(t);
  const lines = [
    hostLine({ ts: "2026-10-28T12:00:00Z", message: "request", spans: [{ name: "request", request_id: REQUEST, kind: "start_run" }] }),
    hostLine({ ts: "2026-10-28T12:00:01Z", level: "WARN", message: "request failed", fields: { outcome: "service_unavailable", error: "no socket", elapsed_ms: 40 }, spans: [{ name: "request", request_id: REQUEST, kind: "start_run" }] }),
    hostLine({ ts: "2026-10-28T12:00:02Z", message: "other" }),
    "{",
  ];
  await writeFile(path.join(dir, "host.jsonl"), `${lines.join("\n")}\n`);
  const out = [];
  const err = [];
  const code = await main(["--dir", dir, "--request", REQUEST.slice(0, 8), "--utc"], { out: (line) => out.push(line), err: (line) => err.push(line) });
  assert.equal(code, 0);
  assert.deepEqual(out, [
    `2026-10-28 12:00:00.000Z host:41 INFO  request kind=start_run request_id=0f8e7d6c`,
    `2026-10-28 12:00:01.000Z host:41 WARN  request failed kind=start_run request_id=0f8e7d6c outcome=service_unavailable elapsed_ms=40 error="no socket"`,
  ]);
  assert.deepEqual(err, ["brauser-logs: skipped 1 malformed line (host.jsonl: 1)"]);

  const json = [];
  await main(["--dir", dir, "--errors", "--json"], { out: (line) => json.push(line), err: () => {} });
  assert.deepEqual(json, [lines[1]]);
});

test("main reads BRAUSER_LOG_DIR and says when there are no files", async (t) => {
  const dir = await logDir(t);
  const err = [];
  const code = await main([], { env: { BRAUSER_LOG_DIR: dir }, out: () => {}, err: (line) => err.push(line) });
  assert.equal(code, 0);
  assert.deepEqual(err, [`brauser-logs: no log files in ${dir}`]);
  assert.equal(await main(["--level"], { out: () => {}, err: () => {} }), 2);
});

test("follow yields appended lines, waits for a partial line, and survives rotation", async (t) => {
  const dir = await logDir(t);
  const live = path.join(dir, "host.jsonl");
  await writeFile(live, `${hostLine({ ts: "2026-10-28T12:00:00Z", message: "old" })}\n`);
  const controller = new AbortController();
  t.after(() => controller.abort());
  const batches = follow({ dir, procs: ["host"], intervalMs: 5, signal: controller.signal });
  const next = async () => (await batches.next()).value;
  const messages = (batch) => batch.events.map((event) => event.message);

  assert.deepEqual(messages(await next()), [], "existing lines are not repeated");
  const line = hostLine({ ts: "2026-10-28T12:00:01Z", message: "new" });
  await appendFile(live, line.slice(0, 20));
  assert.deepEqual(messages(await next()), []);
  await appendFile(live, `${line.slice(20)}\nnot json\n`);
  const batch = await next();
  assert.deepEqual(messages(batch), ["new"]);
  assert.equal(batch.malformed, 1);

  // The writer rotated: the live file starts over, smaller than before.
  await writeFile(live, `${hostLine({ ts: "2026-10-28T12:00:02Z", message: "after" })}\n`);
  assert.deepEqual(messages(await next()), ["after"]);
  controller.abort();
  assert.equal((await batches.next()).done, true);
});

test("the settings page's export header is skipped, not counted as malformed", async (t) => {
  const dir = await logDir(t);
  const extension = path.join(dir, "export.jsonl");
  const header = JSON.stringify({ type: "brauser_diagnostics", exported_at: "2026-10-28T12:00:00Z", entries: 1 });
  const entry = JSON.stringify({ ts: "2026-10-28T12:00:01Z", level: "warn", ctx: "worker", event: "host sync failed" });
  await writeFile(extension, `${header}\n${entry}\nnot json\n`);
  const { events, malformed } = await readAll({ dir, procs: [], extension });
  assert.deepEqual(events.map((event) => event.message), ["host sync failed"]);
  assert.deepEqual(malformed, { "export.jsonl": 1 });
  assert.equal(withoutExportHeader(entry), entry, "an export without a header keeps its first entry");
});

test("follow starts where the snapshot stopped, so a line appended in between is not lost", async (t) => {
  const dir = await logDir(t);
  const live = path.join(dir, "host.jsonl");
  const partial = hostLine({ ts: "2026-10-28T12:00:01Z", message: "between" });
  await writeFile(live, `${hostLine({ ts: "2026-10-28T12:00:00Z", message: "old" })}\n${partial.slice(0, 20)}`);
  const { events, offsets } = await readAll({ dir, procs: ["host"] });
  assert.deepEqual(events.map((event) => event.message), ["old"], "a half-written last line is left for follow");
  // Written after the snapshot, before follow's first poll.
  await appendFile(live, `${partial.slice(20)}\n`);
  const controller = new AbortController();
  t.after(() => controller.abort());
  const batches = follow({ dir, procs: ["host"], intervalMs: 5, signal: controller.signal, offsets });
  assert.deepEqual((await batches.next()).value.events.map((event) => event.message), ["between"]);
  controller.abort();
});
