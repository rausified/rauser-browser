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
//      overwrites it), catalog.json's generated lines take the merge base's
//      text before an ordinary text merge, and ids.lock.json takes the union.
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
// the ordered union of both sides' entries (ours first, then theirs' new ones),
// and only a key both sides set to different ids is a conflict.
//
// With BRAUSER_MERGE_TOOLS=<dir> (the PR doctor: main's tools, never the PR's)
// a merge that changes a generator, so that the merged tree's copy differs
// from <dir>'s, is refused instead of run. BRAUSER_MERGE_LOG=<file> gets one
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

/** The ordered union of three versions of ids.lock.json: ours' keys in order,
 * then each key only theirs has, in theirs' order; a base key a side dropped
 * stays (append-only). `{ ok: false, keys }` names keys both sides set to
 * different ids. */
export function unionLock(baseText, oursText, theirsText) {
  const parse = (text) => (text.trim() ? JSON.parse(text) : {});
  const base = parse(baseText);
  const ours = parse(oursText);
  const theirs = parse(theirsText);
  const merged = {};
  const clashes = [];
  for (const side of [ours, theirs, base]) {
    for (const [key, value] of Object.entries(side)) {
      if (!(key in merged)) merged[key] = value;
      else if (merged[key] !== value && side !== base) clashes.push(key);
    }
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

function trustedTool(tools, generator) {
  if (!tools) return null;
  const file = join(tools, basename(generator.tool));
  return existsSync(file) ? readFileSync(file) : Buffer.alloc(0);
}

/** Regenerates `generator`'s outputs from the merge of `ours` and `other`:
 * `{ ok: true, files: { path: text } }` or `{ ok: false, reason }`. Cached in
 * the git dir by (ours, other, generator, trusted tool). */
export function regenerate(generator, { ours, other, cwd = process.cwd(), tools = process.env.BRAUSER_MERGE_TOOLS || null }) {
  const trusted = trustedTool(tools, generator);
  const key = createHash("sha256").update([ours, other, generator.name, trusted ? createHash("sha256").update(trusted).digest("hex") : "-"].join("\0")).digest("hex");
  const cached = join(cacheDir(cwd), `${key}.json`);
  if (existsSync(cached)) return JSON.parse(readFileSync(cached, "utf8"));
  const result = regenerateUncached(generator, { ours, other, cwd, trusted });
  writeFileSync(cached, JSON.stringify(result));
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
    if (present.length) {
      const archive = join(scratch, ".tree.tar");
      git(["archive", "--format=tar", "-o", archive, tree, "--", ...present], { cwd });
      run("tar", ["-xf", archive, "-C", scratch]);
      rmSync(archive);
    }
    if (trusted) {
      const merged = join(scratch, generator.tool);
      if (!existsSync(merged) || !readFileSync(merged).equals(trusted)) {
        return { ok: false, reason: `the merge changes ${generator.tool}; only main's copy runs here, so regenerate by hand` };
      }
    }
    for (const output of generator.outputs) mkdirSync(dirname(join(scratch, output)), { recursive: true });
    for (const check of [false, true]) {
      const [cmd, args] = generator.run(scratch, check);
      const step = run(cmd, args, { cwd: scratch, check: false });
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

  let lock = null;
  if (path === LOCK_PATH) {
    lock = unionLock(read(basePath), read(oursPath), read(theirsPath));
    if (!lock.ok) {
      say(path, `both sides set ${lock.keys.join(", ")} to different ids; an id is frozen once locked`);
      return textMerge(basePath, oursPath, theirsPath, markerSize);
    }
  }

  if (env.BRAUSER_MERGE_INNER === "1") {
    // Scratch-tree only: regeneration overwrites every output it touches.
    if (lock) writeFileSync(oursPath, lock.text);
    if (path !== CATALOG_PATH) return 0;
    const base = read(basePath);
    writeFileSync(oursPath, withBaseGeneratedLines(read(oursPath), base));
    writeFileSync(theirsPath, withBaseGeneratedLines(read(theirsPath), base));
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
  const status = textMerge(basePath, oursPath, theirsPath, markerSize);
  // Under `git merge` a failed regeneration is a conflict even when the text
  // merges: the merged sources do not produce that text. Elsewhere this is
  // git's own text merge, as with no driver.
  return other ? status || 1 : status;
}

// --- configuration (scripts/dev/setup-merge-drivers, scripts/pr-doctor.mjs) --

/** The `merge.brauser-generated.driver` command. With `tools`, that
 * directory's copy of this script runs (the PR doctor); otherwise each
 * worktree's own copy, falling back to a plain text merge in a checkout that
 * predates it. */
export function driverCommand({ tools = null } = {}) {
  const args = "%O %A %B %P %L";
  if (tools) return `node ${shellQuote(join(tools, "merge-generated.mjs"))} ${args}`;
  return `f="$(git rev-parse --show-toplevel)/scripts/merge-generated.mjs"; if [ -f "$f" ]; then node "$f" ${args}; else git merge-file --marker-size=%L -L ours -L base -L theirs %A %O %B; fi`;
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
