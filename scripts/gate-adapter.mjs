#!/usr/bin/env node

// Focused local gate for adapter-template and starter-adapter changes
// (AGENTS.md "Focused checks per task, the full gate once"). Adding a starter
// such as Confluence (PR #243) or Miro (029a398) touches the starter table,
// Capture Site, settings and panel wiring, the shared vectors, catalog and
// shapes files, the engine's test files, a host fixture and docs. This runs
// only the checks that read those files, sequentially, instead of the whole
// README gate; CI still runs the full gate on every pull request.
//
// It refuses (exit 2) when the change touches anything outside ALLOWLIST
// below: run the full README gate for those.
//
// Usage: npm run gate:adapter [-- options]
//   --base <ref>     compare against <ref>...HEAD (default origin/main)
//   --path <path>    check these paths instead of the git diff (repeatable)
//   --keep-going     run every selected step even after a failure
//   --all-steps      run every step, not just the ones the change triggers
//   --dry-run        print the scope decision and step plan, run nothing
//
// The changed paths are `git diff <base>...HEAD`, plus staged, unstaged and
// untracked files. Exit codes: 0 all steps passed, 1 a step failed or git
// failed, 2 refused (out-of-scope path, wrong Node, or a bad environment).

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, realpathSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

import { isDocsOrQueuePath, parseNameList } from "./ci-changes.mjs";

const USAGE = `Usage: npm run gate:adapter [-- options]
  --base <ref>     compare against <ref>...HEAD (default origin/main)
  --path <path>    check these paths instead of the git diff (repeatable)
  --keep-going     run every selected step even after a failure
  --all-steps      run every step, not just the ones the change triggers
  --dry-run        print the scope decision and step plan, run nothing`;

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

/** Pinned Node major (build-os.yml pins 24.14.0; v22 breaks the extension build). */
export const NODE_MAJOR = 24;

/**
 * Paths an adapter or starter change may touch. Docs and beads config
 * (root Markdown, docs/** except docs/adapters/, .beads/**) are allowed
 * too, through scripts/ci-changes.mjs's isDocsOrQueuePath, so the two
 * classifiers agree on what counts as docs; docs/adapters/ is code there
 * (Open decision 7), so it has its own entry below instead. Anything else
 * needs the full README gate. `exact` matches one path, `prefix` a
 * directory, `pattern` a RegExp.
 */
export const ALLOWLIST = [
  // The starter table: one entry per starter site and its fixtures.
  { exact: "extension/starter-adapters.ts" },
  // Capture Site drafting, its panel and handoff, which list starter types.
  { pattern: /^extension\/capture-site[^/]*\.ts$/ },
  // Settings' coverage table, built from the starter list.
  { exact: "extension/site-coverage.ts" },
  // Settings wiring and copy for starters (options.html is also checked by
  // test:a11y, which this gate then runs).
  { exact: "extension/options.ts" },
  { exact: "extension/options.html" },
  // Panel markup for Capture Site (also triggers test:a11y). panel.ts is not
  // here: it is the whole side panel, so it needs the full gate.
  { exact: "extension/panel.html" },
  // Extension tests; test:extension runs every one (test:a11y for a11y/).
  { prefix: "extension/tests/" },
  // Shared engine vectors and their README, read by both engines' tests.
  { prefix: "protocol/adapter-templates/" },
  // Host-owned Capture Site catalog and the URL shapes both sides pin to.
  { prefix: "protocol/capture-site/" },
  // The engine's tests. adapter-templates/src is not here: the host and the
  // differential test depend on the engine itself, so it needs the full gate.
  { prefix: "adapter-templates/tests/" },
  // Host fixtures the capture_site tests include_str!.
  { exact: "host/tests/fixtures/capture-site-structural-response.json" },
  { prefix: "host/tests/fixtures/site-draft-answers/" },
  // Starter family sources and their generated TypeScript module
  // (scripts/generate-starters.mjs, protocol/starters/README.md).
  { prefix: "protocol/starters/" },
  { exact: "extension/starter-families.generated.ts" },
  // Starter doc pages and the generated status index: no longer exempted
  // through ci-changes.mjs's docs-only path (docs/adapters/ is code there),
  // so the gate needs its own entry to keep them in scope.
  { prefix: "docs/adapters/" },
];

function matches(entry, file) {
  if (entry.exact !== undefined) return file === entry.exact;
  if (entry.prefix !== undefined) return file.startsWith(entry.prefix) && file.length > entry.prefix.length;
  return entry.pattern.test(file);
}

/** True when `file` is a docs/beads-config file or on ALLOWLIST. */
export function isInScope(file) {
  if (typeof file !== "string" || file === "") return false;
  return isDocsOrQueuePath(file) || ALLOWLIST.some((entry) => matches(entry, file));
}

/** The changed paths this gate cannot vouch for. */
export function outOfScope(files) {
  return files.filter((file) => !isInScope(file));
}

const anyOf = (...tests) => (files) => files.some((file) => tests.some((t) => (typeof t === "string" ? file.startsWith(t) : t.test(file))));

const RUST = /\.rs$/;
// No extension test reads the capture-site host fixtures, so they are not
// here. The starter registry test (extension/tests/starter-registry.test.mjs)
// reads the family sources and their doc pages directly, so both join the
// set that reruns test:extension.
const EXTENSION_INPUTS = anyOf(
  "extension/",
  "protocol/adapter-templates/",
  "protocol/capture-site/",
  "protocol/starters/",
  "docs/adapters/",
);
// agent_draft.rs reads the site-draft answers.
const ENGINE_TEST_INPUTS = anyOf(
  "adapter-templates/",
  "protocol/adapter-templates/",
  "protocol/capture-site/",
  "host/tests/fixtures/site-draft-answers/",
);
const HOST_TEST_INPUTS = anyOf("protocol/capture-site/catalog.json", "host/tests/fixtures/");
// A family source, a doc page (Open decision 7: a Status: line edit must
// still run the generator's check 3), or the generated TypeScript twin;
// checked before anything else reads any of them, so a stale generated file
// is reported first.
const STARTER_INPUTS = anyOf("protocol/starters/", "extension/starter-families.generated.ts", "docs/adapters/");

/**
 * Every step, in run order. `when(files)` says whether a change triggers it;
 * the comment on each says which changed files it covers.
 */
export const STEPS = [
  // The family JSON sources must regenerate extension/starter-families.generated.ts
  // and the catalog's starter_types line (scripts/generate-starters.mjs); run
  // first so a stale generated file is reported before anything else runs.
  { name: "starter sources", cmd: ["npm", "run", "-s", "gen:starters", "--", "--check"], when: STARTER_INPUTS },
  // README/status consistency; cheap, and README.md changes with most starters.
  // It also fails a change that commits the work queue, which lives in bd's
  // Dolt database (bd edits, scripts/dev/queue sync publishes them).
  { name: "status docs", cmd: ["python3", "scripts/check_current_state.py"], when: () => true },
  // The extension tests read vectors, catalog, shapes and host fixtures, so
  // any of those (or extension code) runs the whole extension group. The full
  // test:extension takes about a minute, so it is not subset.
  { name: "typecheck", cmd: ["npm", "run", "-s", "typecheck"], when: EXTENSION_INPUTS },
  { name: "lint", cmd: ["npm", "run", "-s", "lint"], when: EXTENSION_INPUTS },
  { name: "build:extension", cmd: ["npm", "run", "-s", "build:extension"], when: EXTENSION_INPUTS },
  { name: "test:extension", cmd: ["npm", "run", "-s", "test:extension"], when: EXTENSION_INPUTS },
  // Built pages with axe and keyboard navigation: only page markup and styles.
  {
    name: "test:a11y",
    cmd: ["npm", "run", "-s", "test:a11y"],
    when: anyOf(/^extension\/[^/]+\.(html|css)$/, "extension/tests/a11y/"),
  },
  { name: "cargo fmt", cmd: ["cargo", "fmt", "--all", "--", "--check"], when: anyOf(RUST) },
  // vectors.rs, shapes.rs and structural.rs read the protocol files, and the
  // policy tests include_str! the catalog.
  { name: "adapter-templates tests", cmd: ["cargo", "test", "--locked", "-p", "adapter-templates"], when: ENGINE_TEST_INPUTS },
  {
    name: "adapter-templates clippy",
    cmd: ["cargo", "clippy", "--locked", "-p", "adapter-templates", "--all-targets", "--", "-D", "warnings"],
    when: (files) => files.some((file) => file.startsWith("adapter-templates/") && RUST.test(file)),
  },
  // The host include_str!s the catalog (capture_site, and the site-draft
  // prompt in service::prompt) and the structural fixture (capture_site),
  // and the site-draft answers (capture_site and the service's site_draft
  // tests). Filtered to those: the rest of the host is unchanged here, and
  // the process-kill tests elsewhere are load-sensitive (brwr-kpg).
  {
    name: "host capture_site tests",
    cmd: ["cargo", "test", "--locked", "-p", "brauser", "--lib", "--", "capture_site", "service::prompt", "site_draft"],
    when: HOST_TEST_INPUTS,
  },
];

/** Steps a change triggers, in run order (all of them with `all`). */
export function selectSteps(files, { all = false } = {}) {
  return STEPS.filter((step) => all || step.when(files));
}

/** Null when `version` (like "v24.14.0") is the pinned major, else a refusal message. */
export function nodeProblem(version) {
  const major = Number(/^v(\d+)\./.exec(version ?? "")?.[1]);
  if (major === NODE_MAJOR) return null;
  return (
    `Node ${version} is not v${NODE_MAJOR}; the extension build and tests need the pinned Node.\n` +
    `Run: export PATH=~/.nvm/versions/node/v24.14.0/bin:$PATH`
  );
}

/** Null when CARGO_TARGET_DIR is unset or inside the repo, else a refusal message. */
export function targetDirProblem(targetDir, repo) {
  if (!targetDir) return null;
  const relative = path.relative(repo, path.resolve(repo, targetDir));
  if (relative === "" || (!relative.startsWith("..") && !path.isAbsolute(relative))) return null;
  return (
    `CARGO_TARGET_DIR=${targetDir} is outside this checkout; a shared target dir from another worktree ` +
    `gives false host test failures. Unset it so this checkout's ./target is used.`
  );
}

export function formatSeconds(ms) {
  if (ms < 59_950) return `${(ms / 1000).toFixed(1)}s`;
  const seconds = Math.round(ms / 1000);
  return `${Math.floor(seconds / 60)}m${seconds % 60}s`;
}

function git(args) {
  return execFileSync("git", args, { cwd: REPO, encoding: "utf8", maxBuffer: 64 * 1024 * 1024 });
}

/** Committed changes since `base`, plus staged, unstaged and untracked files. */
function changedFiles(base) {
  const names = [
    ...parseNameList(git(["diff", "--name-only", "-z", "--no-renames", `${base}...HEAD`])),
    ...parseNameList(git(["diff", "--name-only", "-z", "--no-renames", "HEAD"])),
    ...parseNameList(git(["ls-files", "--others", "--exclude-standard", "-z"])),
  ];
  return [...new Set(names)].sort();
}

/** The PATH the steps run with: this Node first, then the pinned Rust toolchain if cargo is missing. */
function stepPath() {
  const parts = [path.dirname(process.execPath), process.env.PATH ?? ""];
  if (spawnSync("cargo", ["--version"], { stdio: "ignore" }).status !== 0) {
    const channel = /channel\s*=\s*"([^"]+)"/.exec(readFileSync(path.join(REPO, "rust-toolchain.toml"), "utf8"))?.[1];
    const toolchains = path.join(homedir(), ".rustup", "toolchains");
    const found = channel && existsSync(toolchains) && readdirSync(toolchains).find((name) => name.startsWith(`${channel}-`));
    if (found) parts.splice(1, 0, path.join(toolchains, found, "bin"));
  }
  return parts.join(path.delimiter);
}

function main(argv) {
  const { values } = parseArgs({
    args: argv,
    options: {
      base: { type: "string", default: "origin/main" },
      path: { type: "string", multiple: true },
      "keep-going": { type: "boolean", default: false },
      "all-steps": { type: "boolean", default: false },
      "dry-run": { type: "boolean", default: false },
      help: { type: "boolean", default: false },
    },
  });
  if (values.help) {
    console.log(USAGE);
    return 0;
  }

  const nodeRefusal = nodeProblem(process.version);
  if (nodeRefusal) {
    console.error(`REFUSED: ${nodeRefusal}`);
    return 2;
  }
  const targetRefusal = targetDirProblem(process.env.CARGO_TARGET_DIR, REPO);
  if (targetRefusal) {
    console.error(`REFUSED: ${targetRefusal}`);
    return 2;
  }

  let files;
  try {
    files = values.path ? [...new Set(values.path)].sort() : changedFiles(values.base);
  } catch (error) {
    console.error(`git failed: ${error.message}`);
    return 1;
  }
  const source = values.path ? "--path" : `${values.base}...HEAD plus uncommitted and untracked`;
  console.log(`Changed paths (${source}): ${files.length}`);
  for (const file of files) console.log(`  ${isInScope(file) ? "in " : "OUT"} ${file}`);

  const outside = outOfScope(files);
  if (outside.length > 0) {
    console.error(
      `\nREFUSED: ${outside.length} path(s) are outside the adapter gate's scope:\n` +
        outside.map((file) => `  ${file}`).join("\n") +
        `\nRun the full gate in README.md "Development" instead (or split the change).`,
    );
    return 2;
  }

  const steps = selectSteps(files, { all: values["all-steps"] });
  console.log(`\nSteps: ${steps.map((step) => step.name).join(", ") || "(none)"}`);
  if (values["dry-run"]) return 0;

  const env = { ...process.env, PATH: stepPath() };
  const started = Date.now();
  const results = [];
  for (const [index, step] of steps.entries()) {
    console.log(`\n==> [${index + 1}/${steps.length}] ${step.name}: ${step.cmd.join(" ")}`);
    const stepStarted = Date.now();
    const run = spawnSync(step.cmd[0], step.cmd.slice(1), { cwd: REPO, env, stdio: "inherit" });
    const ok = run.status === 0;
    const elapsed = Date.now() - stepStarted;
    results.push({ name: step.name, ok, elapsed });
    console.log(`${ok ? "PASS" : "FAIL"} ${step.name} (${formatSeconds(elapsed)})${run.error ? `: ${run.error.message}` : ""}`);
    if (!ok && !values["keep-going"]) break;
  }

  console.log("\nSummary:");
  for (const result of results) console.log(`  ${result.ok ? "PASS" : "FAIL"} ${result.name.padEnd(26)} ${formatSeconds(result.elapsed)}`);
  for (const step of steps.slice(results.length)) console.log(`  SKIP ${step.name} (after a failure; --keep-going runs it)`);
  const failed = results.filter((result) => !result.ok).length;
  console.log(`Total ${formatSeconds(Date.now() - started)}: ${failed === 0 && results.length === steps.length ? "PASS" : "FAIL"}`);
  return failed === 0 ? 0 : 1;
}

// Compare real paths: argv[1] may go through a symlink (macOS /var -> /private/var)
// or contain characters a file URL escapes (spaces, Windows drive letters).
function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}

if (isEntryPoint()) {
  try {
    process.exit(main(process.argv.slice(2)));
  } catch (error) {
    console.error(error.message);
    process.exit(2);
  }
}
