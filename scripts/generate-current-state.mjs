#!/usr/bin/env node

// Generates AGENTS.md's "Current state" table from one file per row in
// docs/agents/current-state/ (npm run gen:current-state).
//
// The table is the part of AGENTS.md that most PRs change, one row each, and
// two PRs that change neighbouring rows of a hand-kept table conflict. With a
// file per row, written one sentence per line, the sources merge cleanly, and
// the brauser-generated merge driver (scripts/merge-generated.mjs) rebuilds
// the table from the merged rows instead of conflicting.
//
// A row file is `NNN-<slug>.md` (rows sort by file name; leave gaps in the
// numbers for later rows) holding exactly:
//
//   # <Component>
//
//   <State, one sentence per line>
//
//   Pointer: <where to read more>
//
// The state's lines are joined with single spaces into the table cell. The
// table sits between the GENERATED markers below; everything else in
// AGENTS.md is hand-written and left byte for byte.
//
// Usage: node scripts/generate-current-state.mjs [--root DIR] [--check]
//   --check exits 1, writing nothing, when AGENTS.md's table is not what the
//   row files generate or a row file is malformed.

import { existsSync, readFileSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

export const AGENTS_PATH = "AGENTS.md";
export const ROWS_DIR = "docs/agents/current-state";
export const BEGIN = "<!-- BEGIN GENERATED current-state: edit the row files in docs/agents/current-state/, then run npm run gen:current-state -->";
export const END = "<!-- END GENERATED current-state -->";
const HEADER = ["| Component | State | Pointer |", "|---|---|---|"];
const ROW_NAME = /^\d{3}-[a-z0-9]+(?:-[a-z0-9]+)*\.md$/;

/** Parses one row file: `{ ok: true, row }` or `{ ok: false, reason }`. */
export function parseRow(text) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  if (lines.at(-1) !== "" || lines.at(-2) === "") return { ok: false, reason: "must end with exactly one newline" };
  lines.pop();
  const heading = /^# (\S.*)$/.exec(lines[0] ?? "");
  if (!heading) return { ok: false, reason: "first line must be '# <Component>'" };
  if (lines[1] !== "") return { ok: false, reason: "a blank line must follow the heading" };
  const pointer = /^Pointer: (\S.*)$/.exec(lines.at(-1) ?? "");
  if (!pointer || lines.at(-2) !== "") return { ok: false, reason: "last line must be 'Pointer: <where>' after a blank line" };
  const state = lines.slice(2, -2);
  if (state.length === 0 || state.some((line) => line.trim() !== line || line === "")) {
    return { ok: false, reason: "the state must be one paragraph of unindented lines between the heading and the pointer" };
  }
  const cells = [heading[1], state.join(" "), pointer[1]];
  if (cells.some((cell) => cell.includes("|"))) return { ok: false, reason: "a cell may not contain '|'" };
  return { ok: true, row: `| ${cells.join(" | ")} |` };
}

/** The table the row files in `root` generate: `{ table, errors }`. */
export function generateTable(root) {
  const dir = join(root, ROWS_DIR);
  const errors = [];
  const rows = [];
  const names = existsSync(dir) ? readdirSync(dir).sort() : [];
  if (names.length === 0) errors.push(`${ROWS_DIR}/ has no row files`);
  for (const name of names) {
    if (!ROW_NAME.test(name)) {
      errors.push(`${ROWS_DIR}/${name}: not a row file; name rows NNN-<slug>.md (lowercase slug)`);
      continue;
    }
    const parsed = parseRow(readFileSync(join(dir, name), "utf8"));
    if (parsed.ok) rows.push(parsed.row);
    else errors.push(`${ROWS_DIR}/${name}: ${parsed.reason}`);
  }
  return { table: [...HEADER, ...rows].join("\n"), errors };
}

/** `agents` with its GENERATED block replaced by `table`, or null when the
 * markers are missing or out of order. */
export function withTable(agents, table) {
  const eol = agents.includes("\r\n") ? "\r\n" : "\n";
  const lines = agents.replace(/\r\n/g, "\n").split("\n");
  const begin = lines.indexOf(BEGIN);
  const end = lines.indexOf(END);
  if (begin < 0 || end < begin || lines.indexOf(BEGIN, begin + 1) >= 0 || lines.indexOf(END, end + 1) >= 0) return null;
  return [...lines.slice(0, begin + 1), table, ...lines.slice(end)].join("\n").replace(/\n/g, eol);
}

/** `agents` with whatever is between its markers replaced by `block`'s
 * (another AGENTS.md's) GENERATED block, or dropped when that has none; null
 * when `agents` itself has no well-formed block. The merge driver merges
 * the hand-written text this way, then regenerates the table. */
export function withBlockOf(agents, other) {
  const otherLines = other.replace(/\r\n/g, "\n").split("\n");
  const begin = otherLines.indexOf(BEGIN);
  const end = otherLines.indexOf(END);
  const table = begin >= 0 && end > begin ? otherLines.slice(begin + 1, end).join("\n") : "";
  return withTable(agents, table);
}

export function run({ root, check }) {
  const path = join(root, AGENTS_PATH);
  const agents = readFileSync(path, "utf8");
  const { table, errors } = generateTable(root);
  const next = withTable(agents, table);
  if (next === null) errors.push(`${AGENTS_PATH}: needs exactly one '${BEGIN}' line followed by one '${END}' line`);
  if (errors.length) return { code: 1, messages: errors };
  if (next === agents) return { code: 0, messages: [] };
  if (check) return { code: 1, messages: [`${AGENTS_PATH}: the Current state table is not what ${ROWS_DIR}/ generates; run npm run gen:current-state`] };
  writeFileSync(path, next);
  return { code: 0, messages: [`wrote ${AGENTS_PATH}`] };
}

function isEntryPoint() {
  try {
    return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href;
  } catch {
    return false;
  }
}

if (isEntryPoint()) {
  const args = process.argv.slice(2);
  const rootAt = args.indexOf("--root");
  const root = rootAt >= 0 ? args[rootAt + 1] : join(dirname(fileURLToPath(import.meta.url)), "..");
  const { code, messages } = run({ root, check: args.includes("--check") });
  for (const message of messages) (code ? console.error : console.log)(message);
  process.exit(code);
}
