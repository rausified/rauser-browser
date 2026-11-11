// Privacy scan (AGENTS.md "Local checks"): this repository is public and its
// rule is synthetic data only, yet real organization, tenant, colleague and
// board identifiers reached main through ordinary fixtures and plan examples
// (PRs #394 and #400 removed them). A reviewer cannot tell a real slug from an
// invented one without private knowledge, so the owner lists known-real
// identifiers here as salted hashes and CI rejects any text that contains one.
//
// What is scanned
//   (default)          every tracked file in the working tree, and its path
//   --staged           the staged version of every added or modified file
//                      (the opt-in pre-commit hook, scripts/hooks/pre-commit)
//   --message <f|->    one text file, or stdin: a commit message or PR body
//   --commits <range>  every commit message in a git range (CI: the PR's commits)
//   --pr-event         the PR title and body from GITHUB_EVENT_PATH (CI:
//                      pr-hygiene.yml, which reruns when either is edited)
// Hits print as `<where>:<line> <source>` and never include the matched text.
//
// Tokenization. A line is first percent-decoded, up to three times, so
// `a%2Db` and `a%252Db` read as `a-b`. It is then NFKC-normalized and
// lowercased, and split into words: maximal runs of letters and digits. Every
// other character (hyphen, underscore, dot, slash, space, quote ...) is a
// separator. A candidate is any run of 1 to 9 consecutive words joined with
// `-`, so `Acme_Corp`, `acme.corp`, `ACME-CORP`, `acme corp` and
// `acme%2Dcorp` all give the candidate `acme-corp`, and a denylisted
// `acme-corp` is found inside `github.com/acme-corp/repo`. A value written
// with no separator at all (`acmecorp`) is a different token; list it too if
// it matters. The same function (`canonical`) normalizes a value when it is
// added, so an entry matches every spelling above.
//
// The denylist (scripts/privacy-denylist.sha256) holds one `salt <hex>` line
// and lines `<sha256 hex> <word count>`, where the hash is
// sha256(salt + ":" + canonical value). It never holds a plain value. The
// salt only defeats precomputed tables: anyone who already suspects a value
// can confirm it, so the file hides identifiers from casual reading, not from
// a targeted guess. Add an entry locally with
//   node scripts/privacy-scan.mjs --add "<value>"
// and commit the changed file; the command prints neither the value nor the
// hash.
//
// --evidence <path> (local only, never in CI, never committed) also matches
// against a private evidence file such as a browser-history export: host
// labels that carry a hyphen or digit (tenant-like), ID-like path segments
// and query values, tab-separated titles of three or more words, and plain
// lines as whole values. Evidence values are compared in memory and reported
// by class only.

import { createHash, randomBytes } from "node:crypto";
import { execFileSync } from "node:child_process";
import { appendFileSync, existsSync, readFileSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
export const DEFAULT_DENYLIST = join(HERE, "privacy-denylist.sha256");
const MAX_WORDS = 9;
const MAX_FILE_BYTES = 8 * 1024 * 1024;
const WORD = /[\p{L}\p{N}]+/gu;

/** Percent-decode ASCII escapes, up to three nested levels. */
function decode(line) {
  let out = line;
  for (let i = 0; i < 3 && out.includes("%"); i += 1) {
    const next = out.replace(/%([0-7][0-9a-fA-F])/g, (_, hex) => String.fromCharCode(parseInt(hex, 16)));
    if (next === out) break;
    out = next;
  }
  return out;
}

function words(line) {
  return decode(line).normalize("NFKC").toLowerCase().match(WORD) ?? [];
}

/** The normalized form of a value: its words joined with `-`. */
export function canonical(value) {
  return words(value).join("-");
}

export function hashToken(salt, canon) {
  return createHash("sha256").update(`${salt}:${canon}`).digest("hex");
}

/** Parse the denylist file's text: `{ salt, entries: Map<hash, words>, lengths }`. */
export function parseDenylist(text) {
  let salt = null;
  const entries = new Map();
  for (const [index, raw] of text.replace(/\r\n/g, "\n").split("\n").entries()) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const saltLine = /^salt ([0-9a-f]{32,})$/.exec(line);
    if (saltLine) {
      if (salt) throw new Error(`denylist line ${index + 1}: a second salt`);
      salt = saltLine[1];
      continue;
    }
    const entry = /^([0-9a-f]{64}) ([1-9])$/.exec(line);
    if (!entry) throw new Error(`denylist line ${index + 1}: expected "<sha256> <words>"`);
    entries.set(entry[1], Number(entry[2]));
  }
  if (!salt) throw new Error("denylist has no salt line");
  return { salt, entries, lengths: new Set(entries.values()) };
}

/** Append `value`'s hash to the denylist file; returns false when it is already listed. */
export function addEntry(path, value) {
  const canon = canonical(value);
  const count = canon ? canon.split("-").length : 0;
  if (count === 0) throw new Error("--add: the value has no letters or digits");
  if (count > MAX_WORDS) throw new Error(`--add: at most ${MAX_WORDS} words`);
  if (!existsSync(path)) appendFileSync(path, `# Privacy denylist: see scripts/privacy-scan.mjs.\nsalt ${randomBytes(16).toString("hex")}\n`);
  const text = readFileSync(path, "utf8");
  const list = parseDenylist(text);
  const hash = hashToken(list.salt, canon);
  if (list.entries.has(hash)) return false;
  appendFileSync(path, `${text.endsWith("\n") || text === "" ? "" : "\n"}${hash} ${count}\n`);
  return true;
}

/** Evidence tokens from a private export: `[{ value, kind }]`, values canonical. */
export function evidenceTokens(text) {
  const out = new Map();
  const put = (value, kind) => {
    const canon = canonical(value);
    if (canon.replace(/-/g, "").length >= 4 && canon.split("-").length <= MAX_WORDS && !out.has(canon)) out.set(canon, kind);
  };
  const idLike = (s) =>
    (s.length >= 8 && /\d/.test(s) && /[a-z]/i.test(s) && /^[\w-]+$/.test(s)) || /^\d{9,}$/.test(s) ||
    /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(s);
  for (const line of text.replace(/\r\n/g, "\n").split("\n")) {
    if (!line.trim()) continue;
    const urls = line.match(/https?:\/\/[^\s"'<>]+/gi) ?? [];
    for (const raw of urls) {
      let url;
      try {
        url = new URL(raw);
      } catch {
        continue;
      }
      for (const label of url.hostname.split(".")) {
        if (/[-\d]/.test(label) && label.length >= 4) put(label, "host");
      }
      const parts = url.pathname.split("/").map((s) => decode(s));
      for (const [, v] of url.searchParams) parts.push(v);
      for (const part of parts) if (idLike(part)) put(part, "id");
    }
    const rest = line.replace(/https?:\/\/[^\s"'<>]+/gi, "\t");
    const fields = rest.split("\t").map((f) => f.trim()).filter(Boolean);
    if (urls.length === 0 && !line.includes("\t")) {
      put(line, "value");
      continue;
    }
    for (const field of fields) {
      const ws = words(field);
      if (ws.length >= 3 && ws.filter((w) => /\p{L}/u.test(w)).length >= 2) put(field, "title");
    }
  }
  return [...out].map(([value, kind]) => ({ value, kind }));
}

/** A matcher over the denylist and optional evidence tokens. */
export function matcher(list, evidence = []) {
  const lengths = new Set(list.lengths);
  const byValue = new Map();
  for (const { value, kind } of evidence) {
    byValue.set(value, kind);
    lengths.add(value.split("-").length);
  }
  const base = createHash("sha256").update(`${list.salt}:`);
  return {
    lengths: [...lengths].sort((a, b) => a - b),
    match(canon) {
      const kind = byValue.get(canon);
      if (kind) return `evidence:${kind}`;
      if (list.entries.size && list.entries.has(base.copy().update(canon).digest("hex"))) return "denylist";
      return null;
    },
  };
}

function asMatcher(m) {
  return m.match ? m : matcher(m);
}

/** Line numbers (1-based) of `text` holding a match, with the source of the first match on each. */
export function scanLines(text, m) {
  const hits = [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  for (const [index, line] of lines.entries()) {
    const source = scanLine(line, m);
    if (source) hits.push({ line: index + 1, source });
  }
  return hits;
}

function scanLine(line, m) {
  if (!/[\p{L}\p{N}]/u.test(line)) return null;
  const ws = words(line);
  for (let i = 0; i < ws.length; i += 1) {
    for (const n of m.lengths) {
      if (i + n > ws.length) break;
      const source = m.match(n === 1 ? ws[i] : ws.slice(i, i + n).join("-"));
      if (source) return source;
    }
  }
  return null;
}

/** Line numbers of `text` that hold a denylisted (or evidence) token. */
export function scanText(text, m) {
  return scanLines(text, asMatcher(m)).map((h) => h.line);
}

function git(args, cwd) {
  return execFileSync("git", args, {
    cwd,
    encoding: "buffer",
    maxBuffer: 256 * 1024 * 1024,
    env: { ...process.env, GIT_OPTIONAL_LOCKS: "0" },
  });
}

function isBinary(buf) {
  return buf.subarray(0, 8000).includes(0);
}

function scanBuffer(where, buf, m, report) {
  if (isBinary(buf)) return;
  for (const hit of scanLines(buf.toString("utf8"), m)) report(where, hit.line, hit.source);
}

function scanPath(path, m, report) {
  const source = scanLine(path, m);
  if (source) report(path, 0, source);
}

function scanTree(cwd, m, report) {
  const root = git(["rev-parse", "--show-toplevel"], cwd).toString("utf8").trim();
  const files = git(["ls-files", "-z"], root).toString("utf8").split("\0").filter(Boolean);
  let scanned = 0;
  for (const file of files) {
    scanPath(file, m, report);
    const full = join(root, file);
    let stat;
    try {
      stat = statSync(full);
    } catch {
      continue; // deleted in the working tree
    }
    if (!stat.isFile() || stat.size > MAX_FILE_BYTES) continue;
    scanBuffer(file, readFileSync(full), m, report);
    scanned += 1;
  }
  return scanned;
}

function scanStaged(cwd, m, report) {
  const root = git(["rev-parse", "--show-toplevel"], cwd).toString("utf8").trim();
  const files = git(["diff", "--cached", "--name-only", "-z", "--diff-filter=ACMRT"], root)
    .toString("utf8")
    .split("\0")
    .filter(Boolean);
  for (const file of files) {
    scanPath(file, m, report);
    scanBuffer(file, git(["show", `:${file}`], root), m, report);
  }
  return files.length;
}

function scanCommits(range, cwd, m, report) {
  const out = git(["log", "--format=%H%x00%B%x00", range], cwd).toString("utf8").split("\0");
  let count = 0;
  for (let i = 0; i + 1 < out.length; i += 2) {
    const sha = out[i].trim().slice(0, 12);
    if (!sha) continue;
    scanBuffer(`commit ${sha}`, Buffer.from(out[i + 1]), m, report);
    count += 1;
  }
  return count;
}

function scanPrEvent(m, report) {
  const path = process.env.GITHUB_EVENT_PATH;
  if (!path) throw new Error("--pr-event needs GITHUB_EVENT_PATH");
  const pr = JSON.parse(readFileSync(path, "utf8")).pull_request;
  if (!pr) return 0;
  scanBuffer("PR title", Buffer.from(pr.title ?? ""), m, report);
  scanBuffer("PR body", Buffer.from(pr.body ?? ""), m, report);
  return 2;
}

function parseArgs(argv) {
  const opts = { denylist: DEFAULT_DENYLIST, modes: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    const value = () => {
      const v = argv[++i];
      if (v === undefined) throw new Error(`${arg} needs a value`);
      return v;
    };
    if (arg === "--denylist") opts.denylist = value();
    else if (arg === "--add") opts.add = value();
    else if (arg === "--evidence") opts.evidence = value();
    else if (arg === "--staged") opts.modes.push(["staged"]);
    else if (arg === "--message") opts.modes.push(["message", value()]);
    else if (arg === "--commits") opts.modes.push(["commits", value()]);
    else if (arg === "--pr-event") opts.modes.push(["pr-event"]);
    else if (arg === "--tree") opts.modes.push(["tree"]);
    else throw new Error(`unknown argument ${arg}`);
  }
  if (opts.modes.length === 0) opts.modes.push(["tree"]);
  return opts;
}

function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (opts.add !== undefined) {
    const added = addEntry(opts.denylist, opts.add);
    console.log(added ? "privacy scan: added one denylist entry; commit the denylist file." : "privacy scan: that value is already listed.");
    return;
  }
  if (opts.evidence && process.env.CI) throw new Error("--evidence is a local mode; never run it in CI");
  const list = parseDenylist(readFileSync(opts.denylist, "utf8"));
  const evidence = opts.evidence ? evidenceTokens(readFileSync(opts.evidence, "utf8")) : [];
  const m = matcher(list, evidence);
  const annotate = Boolean(process.env.GITHUB_ACTIONS);
  const hits = [];
  const report = (where, line, source) => {
    hits.push(where);
    console.log(line ? `${where}:${line} ${source}` : `${where} (path) ${source}`);
    if (annotate && !where.includes(" ")) {
      console.log(`::error file=${where}${line ? `,line=${line}` : ""},title=Privacy scan::A known real identifier (${source}); replace it with a synthetic value.`);
    }
  };
  const counts = [];
  for (const [mode, arg] of opts.modes) {
    if (mode === "tree") counts.push(`${scanTree(process.cwd(), m, report)} files`);
    else if (mode === "staged") counts.push(`${scanStaged(process.cwd(), m, report)} staged files`);
    else if (mode === "message") {
      const text = arg === "-" ? readFileSync(0) : readFileSync(arg);
      scanBuffer(arg === "-" ? "stdin" : arg, text, m, report);
      counts.push("1 message");
    } else if (mode === "commits") counts.push(`${scanCommits(arg, process.cwd(), m, report)} commit messages`);
    else if (mode === "pr-event") counts.push(`${scanPrEvent(m, report)} PR fields`);
  }
  const places = new Set(hits).size;
  const scope = `${counts.join(", ")}; ${list.entries.size} denylist entries${evidence.length ? `, ${evidence.length} evidence tokens` : ""}`;
  if (hits.length) {
    console.log(`privacy scan: ${hits.length} hit${hits.length === 1 ? "" : "s"} in ${places} place${places === 1 ? "" : "s"} (${scope}).`);
    console.log("Replace each with a synthetic value (acme-corp, contoso, example.com); the matched text is not printed.");
    process.exitCode = 1;
  } else {
    console.log(`privacy scan: 0 hits (${scope}).`);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  try {
    main();
  } catch (error) {
    console.error(`privacy scan: ${error.message}`);
    process.exitCode = 2;
  }
}
