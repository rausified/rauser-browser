#!/usr/bin/env node

// Brauser's local log reader (DESIGN.md §15): merges the host, service, and
// CLI JSON-lines files (and their .1-.3 rotations), plus an optional
// extension diagnostics export, by time and prints one compact line per
// event. Read-only: it never writes, moves, or truncates a log file.
//
//   npm run logs -- [--request <id>] [--run <id>] [--level <min>] [--errors]
//                   [--since 10m|2h|<ISO>] [--proc host|service|cli|ext]
//                   [--extension <file>] [--dir <log dir>] [--json] [--utc]
//                   [--follow]
import { open, readFile, stat } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

// Keep in step with host/src/diag.rs: Process::name, KEPT_ROTATIONS,
// LOG_DIR_ENV, and default_log_dir.
export const PROCESSES = ["host", "service", "cli"];
export const KEPT_ROTATIONS = 3;
export const LOG_DIR_ENV = "BRAUSER_LOG_DIR";
const APP_NAME = "Brauser";

export const LEVELS = ["trace", "debug", "info", "warn", "error"];

// Fields shown on every line, in this order, when an event or its spans
// carry them. Identifiers are shortened; `--json` keeps them whole.
const KEY_FIELDS = ["kind", "request_id", "run_id", "harness", "outcome", "origin", "elapsed_ms", "error"];
const SHORT_FIELDS = new Set(["request_id", "run_id"]);
const SHORT_LENGTH = 8;

/**
 * The directory diag.rs writes to: BRAUSER_LOG_DIR, else
 * ~/Library/Logs/Brauser on macOS, %LOCALAPPDATA%\Brauser\data\logs on
 * Windows (directories' ProjectDirs data_local_dir), and
 * $XDG_DATA_HOME/brauser/logs (default ~/.local/share/brauser/logs) elsewhere.
 */
export function defaultLogDir({ env = process.env, platform = process.platform, home = os.homedir() } = {}) {
  const override = env[LOG_DIR_ENV];
  if (override) return override;
  if (platform === "darwin") return path.posix.join(home, "Library", "Logs", APP_NAME);
  if (platform === "win32") {
    const local = env.LOCALAPPDATA || path.win32.join(home, "AppData", "Local");
    return path.win32.join(local, APP_NAME, "data", "logs");
  }
  const xdg = env.XDG_DATA_HOME && path.posix.isAbsolute(env.XDG_DATA_HOME) ? env.XDG_DATA_HOME : path.posix.join(home, ".local", "share");
  return path.posix.join(xdg, APP_NAME.toLowerCase(), "logs");
}

/** File names for one process, oldest first: `.3`, `.2`, `.1`, then the live file. */
export function logFileNames(proc) {
  const live = `${proc}.jsonl`;
  const names = [];
  for (let index = KEPT_ROTATIONS; index >= 1; index -= 1) names.push(`${live}.${index}`);
  names.push(live);
  return names;
}

function levelIndex(level) {
  return LEVELS.indexOf(String(level ?? "").toLowerCase());
}

/** Parses a `--level` value; throws on an unknown level. */
export function parseLevel(text) {
  const level = String(text).toLowerCase();
  if (level === "warning") return "warn";
  if (!LEVELS.includes(level)) throw new Error(`unknown level ${JSON.stringify(text)}; use one of ${LEVELS.join(", ")}`);
  return level;
}

/**
 * Parses `--since`: a duration before `now` (`30s`, `10m`, `2h`, `1d`) or an
 * ISO date. Returns epoch milliseconds; throws on anything else.
 */
export function parseSince(text, now = Date.now()) {
  const duration = /^(\d+(?:\.\d+)?)([smhd])$/.exec(String(text).trim());
  if (duration) {
    const unit = { s: 1_000, m: 60_000, h: 3_600_000, d: 86_400_000 }[duration[2]];
    return now - Number(duration[1]) * unit;
  }
  const when = Date.parse(String(text));
  if (Number.isNaN(when)) throw new Error(`--since needs a duration like 10m or 2h, or an ISO date; got ${JSON.stringify(text)}`);
  return when;
}

function timeOf(value) {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string") {
    const parsed = Date.parse(value);
    if (!Number.isNaN(parsed)) return parsed;
  }
  return null;
}

function isObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Normalizes one host/service/cli JSON line (tracing-subscriber's JSON
 * format) into an event, or returns null when the line is not one.
 * `proc` is the file's process, used when the line has no `process` span.
 */
export function parseHostLine(line, proc) {
  let raw;
  try {
    raw = JSON.parse(line);
  } catch {
    return null;
  }
  if (!isObject(raw)) return null;
  const ts = timeOf(raw.timestamp);
  if (ts === null || typeof raw.level !== "string") return null;
  const fields = isObject(raw.fields) ? raw.fields : {};
  const spans = Array.isArray(raw.spans) ? raw.spans.filter(isObject) : [];
  // `spans` lists the spans the event happened in, outermost first.
  const root = spans.find((span) => span.name === "process");
  const message = typeof fields.message === "string" ? fields.message : "";
  return {
    ts,
    proc: typeof root?.proc === "string" ? root.proc : proc,
    pid: root?.pid ?? null,
    level: raw.level.toLowerCase(),
    message,
    target: typeof raw.target === "string" ? raw.target : null,
    fields,
    spans,
    raw: line,
  };
}

/**
 * The export without its first line when that line is the settings page's
 * header (`{type: "brauser_diagnostics", exported_at, ...}`), which is not
 * an event and so is not malformed either.
 */
export function withoutExportHeader(text) {
  const end = text.indexOf("\n");
  const first = end === -1 ? text : text.slice(0, end);
  try {
    const header = JSON.parse(first);
    if (isObject(header) && header.type === "brauser_diagnostics") return end === -1 ? "" : text.slice(end + 1);
  } catch {
    // Not a header; every line is parsed as an entry.
  }
  return text;
}

/**
 * Normalizes one line of the options page's "Copy diagnostics" export
 * (`{ts, level, ctx, event, fields}`) into an event of proc `ext`, or null.
 */
export function parseExtensionLine(line) {
  let raw;
  try {
    raw = JSON.parse(line);
  } catch {
    return null;
  }
  if (!isObject(raw)) return null;
  const ts = timeOf(raw.ts);
  if (ts === null || typeof raw.event !== "string") return null;
  return {
    ts,
    proc: "ext",
    pid: typeof raw.ctx === "string" ? raw.ctx : null,
    level: typeof raw.level === "string" ? raw.level.toLowerCase() : "info",
    message: raw.event,
    target: null,
    fields: isObject(raw.fields) ? raw.fields : {},
    spans: [],
    raw: line,
  };
}

/**
 * Parses a whole file's text. Blank lines are skipped; every other line that
 * is not an event counts as malformed.
 */
export function parseText(text, parse) {
  const events = [];
  let malformed = 0;
  for (const line of text.split(/\r?\n/)) {
    if (line.trim() === "") continue;
    const event = parse(line);
    if (event) events.push(event);
    else malformed += 1;
  }
  return { events, malformed };
}

/** The value of `name` on the event, else on its spans, innermost first. */
export function fieldOf(event, name) {
  if (event.fields[name] !== undefined) return event.fields[name];
  for (let index = event.spans.length - 1; index >= 0; index -= 1) {
    const value = event.spans[index][name];
    if (value !== undefined) return value;
  }
  return undefined;
}

function idMatches(event, name, wanted) {
  const values = [event.fields[name], ...event.spans.map((span) => span[name])];
  // A prefix is enough, so the short ids the reader prints can be pasted back.
  return values.some((value) => value !== undefined && value !== null && String(value).startsWith(wanted));
}

/** Whether an event passes every filter given. */
export function matches(event, filters = {}) {
  if (filters.request && !idMatches(event, "request_id", filters.request)) return false;
  if (filters.run && !idMatches(event, "run_id", filters.run)) return false;
  if (filters.level && levelIndex(event.level) < levelIndex(filters.level)) return false;
  if (filters.since !== undefined && event.ts < filters.since) return false;
  if (filters.procs && !filters.procs.includes(event.proc)) return false;
  return true;
}

/** Sorts by time; events with the same time keep their read order. */
export function mergeEvents(groups) {
  return groups.flat().map((event, order) => ({ event, order }))
    .sort((left, right) => left.event.ts - right.event.ts || left.order - right.order)
    .map(({ event }) => event);
}

function pad(number, width = 2) {
  return String(number).padStart(width, "0");
}

/** `2026-10-28 14:03:07.123`, in local time unless `utc`. */
export function formatTime(ts, { utc = false } = {}) {
  const date = new Date(ts);
  const parts = utc
    ? [date.getUTCFullYear(), date.getUTCMonth() + 1, date.getUTCDate(), date.getUTCHours(), date.getUTCMinutes(), date.getUTCSeconds(), date.getUTCMilliseconds()]
    : [date.getFullYear(), date.getMonth() + 1, date.getDate(), date.getHours(), date.getMinutes(), date.getSeconds(), date.getMilliseconds()];
  const [year, month, day, hour, minute, second, ms] = parts;
  return `${year}-${pad(month)}-${pad(day)} ${pad(hour)}:${pad(minute)}:${pad(second)}.${pad(ms, 3)}${utc ? "Z" : ""}`;
}

function formatValue(name, value) {
  let text = typeof value === "string" ? value : JSON.stringify(value);
  if (SHORT_FIELDS.has(name) && text.length > SHORT_LENGTH) text = text.slice(0, SHORT_LENGTH);
  return /[\s"=]/.test(text) ? JSON.stringify(text) : text;
}

/** One compact human line: time, proc:pid, level, message, key fields. */
export function formatEvent(event, options = {}) {
  const who = event.pid === null || event.pid === undefined ? event.proc : `${event.proc}:${event.pid}`;
  const parts = [formatTime(event.ts, options), who, event.level.toUpperCase().padEnd(5), event.message];
  for (const name of KEY_FIELDS) {
    const value = fieldOf(event, name);
    if (value !== undefined && value !== null) parts.push(`${name}=${formatValue(name, value)}`);
  }
  // Then the event's own other fields (exit_code, reason, launch, ...), so
  // a failure line says why without --json.
  for (const [name, value] of Object.entries(event.fields)) {
    if (name === "message" || KEY_FIELDS.includes(name) || value === undefined || value === null) continue;
    parts.push(`${name}=${formatValue(name, value)}`);
  }
  return parts.join(" ");
}

/** Parses command-line arguments; throws with a usage hint on a bad one. */
export function parseArgs(argv, now = Date.now()) {
  const options = { filters: {}, json: false, follow: false, utc: false, dir: null, extension: null, help: false };
  const value = (index, flag) => {
    const next = argv[index + 1];
    if (next === undefined || next.startsWith("--")) throw new Error(`${flag} needs a value`);
    return next;
  };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    switch (flag) {
      case "--request": options.filters.request = value(index, flag); index += 1; break;
      case "--run": options.filters.run = value(index, flag); index += 1; break;
      case "--level": options.filters.level = parseLevel(value(index, flag)); index += 1; break;
      case "--errors": options.filters.level = "warn"; break;
      case "--since": options.filters.since = parseSince(value(index, flag), now); index += 1; break;
      case "--proc": {
        const procs = value(index, flag).split(",");
        for (const proc of procs) {
          if (![...PROCESSES, "ext"].includes(proc)) throw new Error(`--proc takes ${[...PROCESSES, "ext"].join(", ")}; got ${JSON.stringify(proc)}`);
        }
        options.filters.procs = procs;
        index += 1;
        break;
      }
      case "--dir": options.dir = value(index, flag); index += 1; break;
      case "--extension": options.extension = value(index, flag); index += 1; break;
      case "--json": options.json = true; break;
      case "--follow": options.follow = true; break;
      case "--utc": options.utc = true; break;
      case "--help": case "-h": options.help = true; break;
      default: throw new Error(`unknown option ${JSON.stringify(flag)}; see --help`);
    }
  }
  return options;
}

const USAGE = `Usage: npm run logs -- [options]
  --request <id>     only events for this request_id (a prefix is enough)
  --run <id>         only events for this run_id (a prefix is enough)
  --level <min>      trace, debug, info, warn, or error
  --errors           same as --level warn
  --since <when>     a duration (30s, 10m, 2h, 1d) or an ISO date
  --proc <list>      host, service, cli, ext (comma separated)
  --extension <file> merge an extension "Copy diagnostics" export as proc ext
  --dir <dir>        the log directory (default: $${LOG_DIR_ENV}, else the OS default)
  --json             print the matching raw lines instead
  --utc              print times in UTC
  --follow           keep printing new lines as they are written`;

async function readOptional(file) {
  try {
    return await readFile(file, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
}

/**
 * Reads every log file for the selected processes, and the extension export
 * when given. Returns the merged events, per-file malformed counts, and the
 * files read.
 */
export async function readAll({ dir, procs = PROCESSES, extension = null }) {
  const groups = [];
  const malformed = {};
  const files = [];
  const offsets = new Map();
  for (const proc of PROCESSES.filter((name) => procs.includes(name))) {
    for (const name of logFileNames(proc)) {
      const file = path.join(dir, name);
      const read = await readOptional(file);
      if (read === null) continue;
      files.push(file);
      // A live file's last line may still be being written; leave it, and
      // everything after it, to `follow`, which starts at this offset.
      const text = name === `${proc}.jsonl` ? read.slice(0, read.lastIndexOf("\n") + 1) : read;
      if (name === `${proc}.jsonl`) offsets.set(file, Buffer.byteLength(text));
      const parsed = parseText(text, (line) => parseHostLine(line, proc));
      groups.push(parsed.events);
      if (parsed.malformed > 0) malformed[name] = parsed.malformed;
    }
  }
  if (extension) {
    const text = await readFile(extension, "utf8");
    files.push(extension);
    const parsed = parseText(withoutExportHeader(text), parseExtensionLine);
    groups.push(parsed.events);
    if (parsed.malformed > 0) malformed[path.basename(extension)] = parsed.malformed;
  }
  return { events: mergeEvents(groups), malformed, files, offsets };
}

/** A summary line for malformed counts, or null when there were none. */
export function malformedSummary(malformed) {
  const entries = Object.entries(malformed);
  if (entries.length === 0) return null;
  const total = entries.reduce((sum, [, count]) => sum + count, 0);
  return `brauser-logs: skipped ${total} malformed line${total === 1 ? "" : "s"} (${entries.map(([name, count]) => `${name}: ${count}`).join(", ")})`;
}

/** Reads bytes `start` to `end` of a file. */
export async function readRange(file, start, end) {
  const handle = await open(file, "r");
  try {
    const buffer = Buffer.alloc(end - start);
    const { bytesRead } = await handle.read(buffer, 0, buffer.length, start);
    return buffer.subarray(0, bytesRead);
  } finally {
    await handle.close();
  }
}

/**
 * Tails the live files: reads what was appended since the last poll, keeps a
 * partial last line for the next poll, and starts over when a file shrinks
 * (rotated by the writer). Yields each new batch of events, merged.
 */
export async function* follow({ dir, procs = PROCESSES, intervalMs = 1000, signal, statFile = stat, read = readRange, offsets = new Map() }) {
  const state = new Map();
  for (const proc of PROCESSES.filter((name) => procs.includes(name))) {
    const file = path.join(dir, `${proc}.jsonl`);
    // Start where the snapshot stopped, so nothing appended between the
    // snapshot and the first poll is skipped.
    const offset = offsets.get(file) ?? await statFile(file).then((info) => info.size, () => 0);
    state.set(file, { proc, offset, partial: Buffer.alloc(0) });
  }
  while (!signal?.aborted) {
    const groups = [];
    let malformed = 0;
    for (const [file, entry] of state) {
      const size = await statFile(file).then((info) => info.size, () => 0);
      if (size < entry.offset) {
        entry.offset = 0;
        entry.partial = Buffer.alloc(0);
      }
      if (size === entry.offset) continue;
      const bytes = await read(file, entry.offset, size).catch(() => null);
      if (bytes === null) continue;
      entry.offset += bytes.length;
      // Split on bytes, so a character cut by a write in progress waits whole.
      const chunk = Buffer.concat([entry.partial, bytes]);
      const cut = chunk.lastIndexOf(0x0a);
      entry.partial = chunk.subarray(cut + 1);
      if (cut === -1) continue;
      const parsed = parseText(chunk.subarray(0, cut).toString("utf8"), (line) => parseHostLine(line, entry.proc));
      groups.push(parsed.events);
      malformed += parsed.malformed;
    }
    yield { events: mergeEvents(groups), malformed };
    try {
      await delay(intervalMs, undefined, signal ? { signal } : undefined);
    } catch {
      return;
    }
  }
}

/** The command; `deps` lets tests capture output and fix the clock. */
export async function main(argv, deps = {}) {
  const out = deps.out ?? ((line) => process.stdout.write(`${line}\n`));
  const err = deps.err ?? ((line) => process.stderr.write(`${line}\n`));
  const env = deps.env ?? process.env;
  let options;
  try {
    options = parseArgs(argv, deps.now ?? Date.now());
  } catch (error) {
    err(`brauser-logs: ${error.message}`);
    return 2;
  }
  if (options.help) {
    out(USAGE);
    return 0;
  }
  const dir = options.dir ?? defaultLogDir({ env, platform: deps.platform ?? process.platform, home: deps.home ?? os.homedir() });
  const procs = options.filters.procs ?? [...PROCESSES, "ext"];
  const print = (event) => {
    if (matches(event, options.filters)) out(options.json ? event.raw : formatEvent(event, { utc: options.utc }));
  };
  const { events, malformed, files, offsets } = await readAll({ dir, procs, extension: options.extension });
  for (const event of events) print(event);
  const summary = malformedSummary(malformed);
  if (summary) err(summary);
  if (files.length === 0 && !options.follow) err(`brauser-logs: no log files in ${dir}`);
  if (options.follow) {
    let skipped = 0;
    for await (const batch of follow({ dir, procs, intervalMs: deps.intervalMs ?? 1000, signal: deps.signal, offsets })) {
      for (const event of batch.events) print(event);
      if (batch.malformed > 0) {
        skipped += batch.malformed;
        err(`brauser-logs: skipped ${batch.malformed} malformed line${batch.malformed === 1 ? "" : "s"} (${skipped} while following)`);
      }
    }
  }
  return 0;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const controller = new AbortController();
  process.once("SIGINT", () => controller.abort());
  main(process.argv.slice(2), { signal: controller.signal }).then((code) => {
    process.exitCode = code;
  }, (error) => {
    process.stderr.write(`brauser-logs: ${error?.message ?? error}\n`);
    process.exitCode = 1;
  });
}
