#!/usr/bin/env node

// Merge driver for generated files (AGENTS.md "PR doctor", .gitattributes
// `merge=brauser-generated`). Generated files conflict whenever two branches
// both regenerate them, even when their sources merge cleanly, and a conflict
// stalls the merge queue until a person regenerates by hand. This driver
// resolves such a conflict by regenerating instead:
//
//   1. During `git merge` (the only git command that names the other commit
//      to a driver, as GITHEAD_<sha>), it rebuilds the whole merge with
//      `git merge-tree --write-tree HEAD <other>`. That inner merge runs this
//      driver again in inner mode (BRAUSER_MERGE_INNER=1), which only makes
//      the scratch tree usable: a pure output keeps ours (regeneration
//      overwrites it), the generated parts of a mixed file (catalog.json's
//      generated lines, AGENTS.md's Current state table) take the merge
//      base's text before an ordinary text merge of the hand-written rest,
//      and ids.lock.json takes the union.
//   2. A conflict in any of the generator's other inputs (family files,
//      schema.json, shapes, docs, the generator itself) stops here.
//   3. Otherwise it extracts the generator's inputs from the merged tree into
//      a scratch directory, runs the generator, then the generator's own
//      --check, and writes the regenerated file as the merge result.
//
// Whenever it cannot regenerate (another git command, an input conflict, a
// failing generator or --check), it leaves an ordinary conflict with markers
// and says why on stderr; it never picks a side silently. The one exception is
// protocol/starters/ids.lock.json, which is append-only: its merge is always
// the ordered union of both sides' entries (base's in place, then ours' new
// ones, then theirs'), and only a key both sides set to different ids is a
// conflict.
//
// A generator runs only when the merged tree's copy is byte-identical to a
// trusted one: with BRAUSER_MERGE_TOOLS=<dir> (the unattended PR doctor) only
// <dir>'s copy, main's; otherwise the merge base's, your checked-out HEAD's or
// origin/main's. Merging into a branch you checked out therefore runs that
// branch's generator, because you are already running its code. A merge that
// changes a generator any other way is a conflict to resolve by hand.
// Generators run with no credentials in their environment (`generatorEnv`).
// BRAUSER_MERGE_LOG=<file> gets one
// `<path>\t<how>` line per file the driver resolved.
//
// Usage (git runs it; scripts/dev/setup-merge-drivers configures it):
//   node scripts/merge-generated.mjs %O %A %B %P %L

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  appendFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, realpathSync, rmSync, statSync, writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join } from "node:path";
import { pathToFileURL } from "node:url";

export const DRIVER = "brauser-generated";

export const LOCK_PATH = "protocol/starters/ids.lock.json";
export const CATALOG_PATH = "protocol/capture-site/catalog.json";
export const AGENTS_PATH = "AGENTS.md";

/** Each generator: the outputs it owns, the input paths a conflict in which
 * stops regeneration, what to extract from the merged tree, and how to run
 * it and its --check inside the scratch directory. */
export const GENERATORS = Object.freeze([
  Object.freeze({
    name: "protocol",
    tool: "protocol/generate.py",
    outputs: Object.freeze(["protocol/src/generated.rs", "protocol/ts/generated.ts"]),
    inputs: Object.freeze(["protocol/schema.json", "protocol/generate.py"]),
    extract: Object.freeze(["protocol/schema.json", "protocol/generate.py"]),
    run: (dir, check) => [python(), [join(dir, "protocol/generate.py"), ...(check ? ["--check"] : [])]],
  }),
  Object.freeze({
    name: "starters",
    tool: "scripts/generate-starters.mjs",
    outputs: Object.freeze([
      "extension/starter-families.generated.ts",
      CATALOG_PATH,
      "protocol/capture-site/starter-shapes.json",
      "protocol/adapter-templates/starter-vectors.json",
      "docs/adapters/README.md",
      LOCK_PATH,
    ]),
    inputs: Object.freeze([
      "protocol/starters/", "protocol/capture-site/", "protocol/adapter-templates/", "docs/adapters/",
      "scripts/generate-starters.mjs",
    ]),
    extract: Object.freeze(["protocol", "docs/adapters", "scripts/generate-starters.mjs"]),
    run: (dir, check) => [process.execPath, [join(dir, "scripts/generate-starters.mjs"), "--root", dir, ...(check ? ["--check"] : [])]],
  }),
  Object.freeze({
    name: "current-state",
    tool: "scripts/generate-current-state.mjs",
    outputs: Object.freeze([AGENTS_PATH]),
    inputs: Object.freeze(["docs/agents/current-state/", "scripts/generate-current-state.mjs"]),
    extract: Object.freeze([AGENTS_PATH, "docs/agents/current-state", "scripts/generate-current-state.mjs"]),
    run: (dir, check) => [process.execPath, [join(dir, "scripts/generate-current-state.mjs"), "--root", dir, ...(check ? ["--check"] : [])]],
  }),
]);

function python() {
  return process.env.BRAUSER_MERGE_PYTHON || (process.platform === "win32" ? "python" : "python3");
}

export function generatorFor(path) {
  return GENERATORS.find((generator) => generator.outputs.includes(path)) ?? null;
}

const isInput = (generator, path) =>
  !generator.outputs.includes(path) && generator.inputs.some((input) => input.endsWith("/") ? path.startsWith(input) : path === input);

// --- Pure parts (scripts/tests/merge-generated.test.mjs) ---------------------

/** The ordered union of three versions of ids.lock.json: base's keys in
 * order (one a side dropped stays in its place: append-only), then ours' new
 * keys, then theirs', each in that side's order. `{ ok: false, keys }` names keys both sides set to
 * different ids. */
export function unionLock(baseText, oursText, theirsText) {
  const parse = (text) => (text.trim() ? JSON.parse(text) : {});
  const base = parse(baseText);
  const ours = parse(oursText);
  const theirs = parse(theirsText);
  const merged = {};
  const clashes = [];
  // Base keys keep their place (a key a side dropped stays where it was),
  // then ours' new keys, then theirs'; ours' id wins over base's.
  for (const key of new Set([...Object.keys(base), ...Object.keys(ours), ...Object.keys(theirs)])) {
    if (key in ours && key in theirs && ours[key] !== theirs[key]) clashes.push(key);
    merged[key] = key in ours ? ours[key] : key in theirs ? theirs[key] : base[key];
  }
  if (clashes.length) return { ok: false, keys: clashes };
  return { ok: true, text: `${JSON.stringify(merged, null, 2)}\n` };
}

const GENERATED_CATALOG_LINE = /^ {2}"(starter_types|container_first)": \[.*\],\r?$/;

/** `text` with catalog.json's generated lines replaced by `base`'s (or
 * dropped when `base` has none), so a text merge sees only the hand-written
 * changes; the generator then rewrites those lines from the merged families. */
export function withBaseGeneratedLines(text, base) {
  const baseLines = new Map();
  for (const line of base.split("\n")) {
    const match = GENERATED_CATALOG_LINE.exec(line);
    if (match) baseLines.set(match[1], line);
  }
  return text
    .split("\n")
    .flatMap((line) => {
      const match = GENERATED_CATALOG_LINE.exec(line);
      if (!match) return [line];
      return baseLines.has(match[1]) ? [baseLines.get(match[1])] : [];
    })
    .join("\n");
}

// AGENTS.md's generated block (scripts/generate-current-state.mjs BEGIN/END;
// its tests check these match).
export const AGENTS_BLOCK = Object.freeze({
  begin: /^<!-- BEGIN GENERATED current-state\b.*-->\r?$/,
  end: /^<!-- END GENERATED current-state -->\r?$/,
});

/** `text` with the lines inside its GENERATED block replaced by `base`'s
 * (none when `base` has no block), so a text merge sees only the
 * hand-written changes; the generator then rewrites the block from the
 * merged rows, in `text`'s own line ending. `text` without one well-formed
 * block is returned unchanged. */
export function withBaseGeneratedBlock(text, base, { begin, end } = AGENTS_BLOCK) {
  const bounds = (lines) => {
    const from = lines.findIndex((line) => begin.test(line));
    const to = lines.findIndex((line) => end.test(line));
    return from >= 0 && to > from ? [from, to] : null;
  };
  const lines = text.split("\n");
  const own = bounds(lines);
  if (!own) return text;
  const baseLines = base.split("\n");
  const theirs = bounds(baseLines);
  // The base's rows take this side's line ending, so a CRLF side merged
  // against an LF base (or the reverse) never ends up with mixed endings.
  const cr = text.includes("\r\n") ? "\r" : "";
  const inside = theirs ? baseLines.slice(theirs[0] + 1, theirs[1]).map((line) => line.replace(/\r$/, "") + cr) : [];
  return [...lines.slice(0, own[0] + 1), ...inside, ...lines.slice(own[1])].join("\n");
}

/** Mixed files: hand-written text around generated parts, and how to give a
 * side's generated parts the merge base's text. */
const MIXED = Object.freeze({
  [CATALOG_PATH]: withBaseGeneratedLines,
  [AGENTS_PATH]: (text, base) => withBaseGeneratedBlock(text, base),
});

/** The other commit of a running `git merge` (GITHEAD_<sha>), or null. */
export function otherCommit(env = process.env) {
  const shas = Object.keys(env).map((key) => /^GITHEAD_([0-9a-f]{40,64})$/.exec(key)?.[1]).filter(Boolean);
  return shas.length === 1 ? shas[0] : null;
}

// --- git and the scratch regeneration ----------------------------------------

function run(cmd, args, { cwd, env, check = true } = {}) {
  const result = spawnSync(cmd, args, { cwd, env: env ?? process.env, encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (check && result.status !== 0) throw new Error(`${cmd} ${args.join(" ")} exited ${result.status}: ${(result.stderr || result.stdout).trim()}`);
  return { code: result.status, stdout: result.stdout, stderr: result.stderr };
}
const git = (args, opts) => run("git", args, opts);

function cacheDir(cwd) {
  const gitDir = git(["rev-parse", "--absolute-git-dir"], { cwd }).stdout.trim();
  const dir = join(gitDir, "brauser-merge-cache");
  mkdirSync(dir, { recursive: true });
  // Results are keyed by commit ids, so they never go stale; prune old ones.
  for (const name of readdirSync(dir)) {
    const file = join(dir, name);
    try {
      if (Date.now() - statSync(file).mtimeMs > 6 * 3600 * 1000) rmSync(file, { force: true });
    } catch {
      // A concurrent prune got it first.
    }
  }
  return dir;
}

/** The generator copies allowed to run, as `{ copies, from }`. With `tools`
 * (the PR doctor) only that directory's copy, an empty one when it is
 * missing so nothing matches. Otherwise the merge base's, HEAD's and
 * origin/main's copies. A generator only the merged-in branch changed is
 * refused; HEAD's copy is trusted because checking a branch out already runs
 * its code. */
function trustedCopies(generator, { tools, ours, other, cwd }) {
  if (tools) {
    const file = join(tools, basename(generator.tool));
    return { copies: [existsSync(file) ? readFileSync(file) : Buffer.alloc(0)], from: "main's copy (BRAUSER_MERGE_TOOLS)" };
  }
  const base = git(["merge-base", ours, other], { cwd, check: false }).stdout.trim();
  const copies = [];
  for (const rev of [base, ours, "refs/remotes/origin/main"]) {
    if (!rev) continue;
    const blob = spawnSync("git", ["show", `${rev}:${generator.tool}`], { cwd, maxBuffer: 64 * 1024 * 1024 });
    if (blob.status === 0) copies.push(blob.stdout);
  }
  return { copies, from: "the merge base's, HEAD's or origin/main's copy" };
}

/** The environment a generator runs in: no credentials, only what node or
 * python needs to start. */
export function generatorEnv(env = process.env) {
  const keep = ["PATH", "HOME", "TMPDIR", "TMP", "TEMP", "LANG", "LC_ALL", "SYSTEMROOT", "SystemRoot", "WINDIR", "USERPROFILE", "PATHEXT", "COMSPEC"];
  return Object.fromEntries(keep.filter((name) => env[name] !== undefined).map((name) => [name, env[name]]));
}

/** Regenerates `generator`'s outputs from the merge of `ours` and `other`:
 * `{ ok: true, files: { path: text } }` or `{ ok: false, reason }`. Cached in
 * the git dir by (ours, other, generator, trusted tool). */
export function regenerate(generator, { ours, other, cwd = process.cwd(), tools = process.env.BRAUSER_MERGE_TOOLS || null }) {
  const trusted = trustedCopies(generator, { tools, ours, other, cwd });
  const digests = trusted.copies.map((copy) => createHash("sha256").update(copy).digest("hex"));
  const key = createHash("sha256").update([ours, other, generator.name, ...digests].join("\0")).digest("hex");
  const cached = join(cacheDir(cwd), `${key}.json`);
  if (existsSync(cached)) return JSON.parse(readFileSync(cached, "utf8"));
  const result = regenerateUncached(generator, { ours, other, cwd, trusted });
  // Only successes: a failure may be this machine's (a missing python), and
  // a retry after fixing it must run again.
  if (result.ok) writeFileSync(cached, JSON.stringify(result));
  return result;
}

function regenerateUncached(generator, { ours, other, cwd, trusted }) {
  const env = { ...process.env, BRAUSER_MERGE_INNER: "1" };
  for (const name of Object.keys(env)) if (name.startsWith("GITHEAD_")) delete env[name];
  const merge = git(["merge-tree", "--write-tree", "--name-only", "--no-messages", ours, other], { cwd, env, check: false });
  if (merge.code !== 0 && merge.code !== 1) return { ok: false, reason: `git merge-tree failed: ${merge.stderr.trim()}` };
  const [tree, ...conflicted] = merge.stdout.split("\n").filter(Boolean);
  const blocking = [...new Set(conflicted)].filter((path) => isInput(generator, path) || generator.outputs.includes(path));
  if (blocking.length) return { ok: false, reason: `conflict in ${blocking.join(", ")}, an input of the ${generator.name} generator; resolve it, then regenerate` };

  const scratch = mkdtempSync(join(tmpdir(), "brauser-merge-"));
  try {
    const present = git(["ls-tree", "--name-only", tree, "--", ...generator.extract], { cwd }).stdout.split("\n").filter(Boolean);
    // A symlink in the extract (#463 review I2) would let the generator read
    // a file outside the scratch tree, or write an output through it: a
    // conflict, before anything is extracted.
    const links = git(["ls-tree", "-r", "-z", tree, "--", ...present, ...generator.outputs], { cwd }).stdout.split("\0")
      .filter((entry) => entry.startsWith("120000 ")).map((entry) => entry.slice(entry.indexOf("\t") + 1));
    if (links.length) return { ok: false, reason: `the merge has a symlink at ${links.join(", ")}, among the ${generator.name} generator's sources or outputs; a generator never runs over a symlink, so resolve it by hand` };
    if (present.length) {
      const archive = join(scratch, ".tree.tar");
      git(["archive", "--format=tar", "-o", archive, tree, "--", ...present], { cwd });
      run("tar", ["-xf", archive, "-C", scratch]);
      rmSync(archive);
    }
    const merged = join(scratch, generator.tool);
    if (!existsSync(merged) || !trusted.copies.some((copy) => readFileSync(merged).equals(copy))) {
      return { ok: false, reason: `the merge changes ${generator.tool}, which is not ${trusted.from}; a merge runs only a generator it already trusts, so resolve it by hand and regenerate` };
    }
    for (const output of generator.outputs) mkdirSync(dirname(join(scratch, output)), { recursive: true });
    for (const check of [false, true]) {
      const [cmd, args] = generator.run(scratch, check);
      const step = run(cmd, args, { cwd: scratch, env: generatorEnv(), check: false });
      if (step.code !== 0) {
        const detail = (step.stdout + step.stderr).trim().split("\n").slice(0, 3).join("; ");
        return { ok: false, reason: `${generator.tool}${check ? " --check" : ""} failed on the merged sources: ${detail}` };
      }
    }
    const files = {};
    for (const output of generator.outputs) {
      const file = join(scratch, output);
      if (existsSync(file)) files[output] = readFileSync(file, "utf8");
    }
    return { ok: true, files };
  } finally {
    rmSync(scratch, { recursive: true, force: true });
  }
}

// --- the driver --------------------------------------------------------------

function textMerge(base, ours, theirs, markerSize) {
  return git(["merge-file", `--marker-size=${markerSize || 7}`, "-L", "ours", "-L", "base", "-L", "theirs", ours, base, theirs], { check: false }).code;
}

/** An ordinary conflict, always: the text merge's markers, or, where the text
 * merges cleanly into something that is still wrong (a lock key set twice),
 * the whole file as one ours/base/theirs conflict. Never exits 0. */
function conflict(basePath, oursPath, theirsPath, markerSize) {
  const ours = readFileSync(oursPath, "utf8");
  if (textMerge(basePath, oursPath, theirsPath, markerSize) !== 0) return 1;
  const marks = (char) => char.repeat(Number(markerSize) || 7);
  const end = (text) => (text.endsWith("\n") ? text : `${text}\n`);
  writeFileSync(oursPath, `${marks("<")} ours\n${end(ours)}${marks("|")} base\n${end(readFileSync(basePath, "utf8"))}${marks("=")}\n${end(readFileSync(theirsPath, "utf8"))}${marks(">")} theirs\n`);
  return 1;
}

function log(path, how) {
  if (process.env.BRAUSER_MERGE_LOG) appendFileSync(process.env.BRAUSER_MERGE_LOG, `${path}\t${how}\n`);
}

function say(path, message) {
  process.stderr.write(`merge-generated: ${path}: ${message}\n`);
}

/** Runs one driver call; returns the exit code (0 resolved, else conflict). */
export function driver([basePath, oursPath, theirsPath, path, markerSize], env = process.env) {
  const generator = generatorFor(path);
  const read = (file) => readFileSync(file, "utf8");
  if (!generator) return textMerge(basePath, oursPath, theirsPath, markerSize);
  // An AGENTS.md from before its table was generated, on every side, has
  // nothing to regenerate: merge it as text, as before.
  if (path === AGENTS_PATH && ![basePath, oursPath, theirsPath].some((file) => read(file).split("\n").some((line) => AGENTS_BLOCK.begin.test(line)))) {
    return textMerge(basePath, oursPath, theirsPath, markerSize);
  }

  let lock = null;
  if (path === LOCK_PATH) {
    lock = unionLock(read(basePath), read(oursPath), read(theirsPath));
    if (!lock.ok) {
      say(path, `both sides set ${lock.keys.join(", ")} to different ids; an id is frozen once locked`);
      return conflict(basePath, oursPath, theirsPath, markerSize);
    }
  }

  if (env.BRAUSER_MERGE_INNER === "1") {
    // Scratch-tree only: regeneration overwrites every output it touches.
    if (lock) writeFileSync(oursPath, lock.text);
    const strip = MIXED[path];
    if (!strip) return 0;
    const base = read(basePath);
    writeFileSync(oursPath, strip(read(oursPath), base));
    writeFileSync(theirsPath, strip(read(theirsPath), base));
    return textMerge(basePath, oursPath, theirsPath, markerSize);
  }

  const other = otherCommit(env);
  let result = { ok: false, reason: "only `git merge` names the other commit to a merge driver; regenerate after resolving" };
  if (other) {
    const ours = git(["rev-parse", "HEAD"]).stdout.trim();
    result = regenerate(generator, { ours, other });
  }
  if (result.ok && path in result.files) {
    writeFileSync(oursPath, result.files[path]);
    log(path, "regenerated");
    return 0;
  }
  if (lock) {
    writeFileSync(oursPath, lock.text);
    log(path, "union");
    return 0;
  }
  say(path, result.ok ? "the generator no longer writes this file" : result.reason);
  // Under `git merge` a failed regeneration is a conflict even when the text
  // merges: the merged sources do not produce that text. Elsewhere this is
  // git's own text merge, as with no driver.
  return other ? conflict(basePath, oursPath, theirsPath, markerSize) : textMerge(basePath, oursPath, theirsPath, markerSize);
}

// --- configuration (scripts/dev/setup-merge-drivers, scripts/pr-doctor.mjs) --

/** The `merge.brauser-generated.driver` command. With `tools`, that
 * directory's copy of this script runs (the PR doctor); otherwise each
 * worktree's own copy, falling back to a plain text merge in a checkout that
 * predates it. */
export function driverCommand({ tools = null } = {}) {
  const args = "%O %A %B %P %L";
  // Git runs the driver under sh, which takes "/" on every platform.
  // Without node (or the script), git's own text merge, so a conflict always has markers.
  const plain = "git merge-file --marker-size=%L -L ours -L base -L theirs %A %O %B";
  const file = tools ? shellQuote(`${tools.replace(/[\\/]+$/, "")}/merge-generated.mjs`) : '"$(git rev-parse --show-toplevel)/scripts/merge-generated.mjs"';
  return `f=${file}; if [ -f "$f" ] && command -v node >/dev/null 2>&1; then node "$f" ${args}; else ${plain}; fi`;
}

const shellQuote = (text) => `'${text.replaceAll("'", "'\\''")}'`;

/** Registers the driver in `cwd`'s repository config (shared by its worktrees). */
export function configure({ cwd = process.cwd(), tools = null } = {}) {
  git(["config", `merge.${DRIVER}.name`, "Brauser generated files: regenerate from the merged sources"], { cwd });
  git(["config", `merge.${DRIVER}.driver`, driverCommand({ tools })], { cwd });
}

export function unconfigure({ cwd = process.cwd() } = {}) {
  git(["config", "--remove-section", `merge.${DRIVER}`], { cwd, check: false });
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
  let code;
  try {
    code = driver(args);
  } catch (error) {
    // Never leave ours in place unmarked: fall back to an ordinary conflict.
    say(args[3] ?? "?", `${error.message.split("\n")[0]}; leaving an ordinary conflict`);
    code = textMerge(args[0], args[1], args[2], args[4]) || 1;
  }
  process.exit(code);
}
