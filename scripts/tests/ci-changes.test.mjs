import assert from "node:assert/strict";
import { existsSync, readdirSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { classify, devToolsReasons, isDocsOrQueuePath, parseNameList } from "../ci-changes.mjs";

test("root Markdown, docs/ and .beads/ are docs paths (check_current_state.py refuses a committed queue file)", () => {
  for (const path of [
    "AGENTS.md",
    "BOSUN_FEATURE_BACKLOG.md",
    "CookieFeature.md",
    "DESIGN.md",
    "PENDING_AMOS_DOC_UPDATES.md",
    "README.md",
    "TESTING.md",
    "adapter-templates.md",
    "docs/history.md",
    "docs/superpowers/plans/2026-10-28-task-bar.md",
    ".beads/issues.jsonl",
    ".beads/config.yaml",
  ]) {
    assert.equal(isDocsOrQueuePath(path), true, path);
  }
});

test("everything else counts as code, including SUMMARY.md and nested Markdown that tests or builds read", () => {
  for (const path of [
    "host/src/native.rs",
    "SUMMARY.md",
    "host/tests/fixtures/pre-home-layout/README.md",
    "protocol/README.md",
    "protocol/adapter-templates/README.md",
    "protocol/schema.json",
    // Open decision 7 (starter families PR C): a starter doc's Status: line
    // is read by the generator's check 3 and the registry test, both code.
    "docs/adapters/jira.issue.md",
    "docs/adapters/README.md",
    ".github/workflows/ci.yml",
    "scripts/check_queue_consistency.py",
    "package.json",
    "Cargo.lock",
    "LICENSE",
    "README.md.orig",
    "docs",
    "docsx/a.md",
    ".beads",
    ".beadsx/issues.jsonl",
    "",
  ]) {
    assert.equal(isDocsOrQueuePath(path), false, JSON.stringify(path));
  }
});

test("a change is docs-only only when every path is a docs or queue file", () => {
  assert.equal(classify(["AGENTS.md", ".beads/issues.jsonl", "docs/history.md"]), true);
  assert.equal(classify(["AGENTS.md", "host/src/native.rs"]), false);
});

test("docs/adapters/ is carved out of docs/ (Open decision 7): a doc edit always runs the code builds", () => {
  assert.equal(classify(["docs/adapters/jira.issue.md"]), false);
  assert.equal(classify(["docs/history.md"]), true);
});

test("an empty change list is not docs-only, so a failed diff never skips the builds", () => {
  assert.equal(classify([]), false);
});

test("NUL-separated git output keeps unusual names intact and drops the trailing terminator", () => {
  assert.deepEqual(parseNameList("AGENTS.md\0docs/café notes.md\0"), ["AGENTS.md", "docs/café notes.md"]);
  assert.deepEqual(parseNameList(""), []);
});

test("a path with a newline in its name is one path, not two", () => {
  // `docs/a\nhost/src/x.rs` is a single file under docs/, not a docs file plus a host file.
  assert.deepEqual(parseNameList("docs/a\nhost/src/x.rs\0"), ["docs/a\nhost/src/x.rs"]);
});

// The Python dev-tools tests (build-os.yml "Test the queue tools and dev
// scripts", npm run test:dev-tools) run only when something they read
// changed (delay-fix rank 12, brwr-f8mm; c0e86b2d F6: 9 minutes locally for
// a change that touched no Python).
test("Python dev-tool sources, their tests and the files they read need the dev-tools tests", () => {
  for (const path of [
    "scripts/work_queue.py",
    "scripts/check_current_state.py",
    "scripts/tests/test_work_queue.py",
    "scripts/tests/git_test_env.py",
    "scripts/tests/fixtures/mergify-comments.json",
    "scripts/dev/queue",
    "scripts/dev/deploy-live",
    "scripts/dev/worktree-hygiene",
    "scripts/dev/queue-seed/brwr-9eio.jsonl",
    "worker-contract.json",
    "worker-contract.schema.json",
    ".gitattributes",
    "BRAUSER_IMPROVEMENT_FEED.md",
    "docs/improvement-feed/2026-11-12-some-entry.md",
    "docs/backlog/frame.md",
    "docs/backlog/notes/2026-11-12-note.md",
    ".beads/metadata.json",
    // The definitions that run them, and this classifier itself.
    ".github/workflows/ci.yml",
    ".github/workflows/build-os.yml",
    ".github/workflows/pr-hygiene.yml",
    "scripts/ci-changes.mjs",
    "package.json",
  ]) {
    assert.equal(devToolsReasons([path]).length, 1, path);
  }
});

test("a change that touches no Python dev tool skips the dev-tools tests", () => {
  assert.deepEqual(devToolsReasons([
    "host/src/native.rs",
    "extension/panel.ts",
    "scripts/dev/pr-watch.mjs",
    "scripts/tests/pr-watch.test.mjs",
    "scripts/ci-macos.mjs",
    "scripts/smoke-macos.mjs",
    "DESIGN.md",
    "README.md",
    "Cargo.lock",
  ]), []);
});

test("each dev-tools reason names the path that needs the tests", () => {
  assert.deepEqual(devToolsReasons(["extension/panel.ts", "scripts/work_queue.py", "worker-contract.json"]), [
    ["scripts/work_queue.py", "a Python dev tool or its tests"],
    ["worker-contract.json", "read by the dev-tools tests"],
  ]);
});

test("an empty change list runs the dev-tools tests, so a failed diff never skips them", () => {
  assert.deepEqual(devToolsReasons([]), [["(no changed paths)", "an empty or failed diff runs everything"]]);
});

// Codex review of #431: test_worker_contract.py reads scripts/dev/queue-pr.mjs
// and scripts/pr-constants.mjs, which the first cut of devToolsReasons missed.
// Every checkout path a Python test names (`ROOT / "x"`, `SCRIPTS / "dev" /
// "queue"`, a "scripts/..." literal) must select the dev-tools tests.
// The checkout paths one Python test file names, relative to the repository
// root. `base` is ROOT, REPO, SCRIPTS or SCRIPTS.parent, or a local alias of
// one (`root = SCRIPTS.parent`, then `root / ".beads" / "README.md"`: #443
// read .beads/README.md that way, and this guard missed it).
function namedCheckoutPaths(name, source) {
  const named = new Set();
  const bases = { ROOT: "", REPO: "", SCRIPTS: "scripts", "SCRIPTS.parent": "" };
  for (const [, alias, base] of source.matchAll(/^\s*(\w+)\s*=\s*(ROOT|REPO|SCRIPTS\.parent|SCRIPTS)\s*$/gm)) {
    bases[alias] = bases[base];
  }
  const names = Object.keys(bases).sort((a, b) => b.length - a.length).map((b) => b.replace(".", "\\."));
  const chainRe = new RegExp(`(?<![\\w.])(${names.join("|")})((?:\\s*\\/\\s*["'][^"']+["'])+)`, "g");
  for (const [, base, chain] of source.matchAll(chainRe)) {
    const parts = [...chain.matchAll(/["']([^"']+)["']/g)].map((m) => m[1]);
    named.add([bases[base], ...parts].filter(Boolean).join("/"));
  }
  for (const [, literal] of source.matchAll(/["'](scripts\/[^"'\s]+)["']/g)) named.add(literal);
  // `NAME = Path(__file__).resolve()<up>[ / "a" / "b"]`: resolve it ourselves, and
  // refuse a base name the extractor above does not follow (#431 review M4), so a
  // test reading the checkout through, say, REPO_ROOT cannot slip past this guard.
  for (const [, variable, up, chain] of source.matchAll(/^(\w+)\s*=\s*(?:pathlib\.)?Path\(__file__\)\.resolve\(\)((?:\.parents\[\d\]|\.parent)+)((?:\s*\/\s*["'][^"']+["'])*)/gm)) {
    const depth = [...up.matchAll(/\.parent(?:s\[(\d)\])?/g)].reduce((sum, m) => sum + (m[1] === undefined ? 1 : Number(m[1]) + 1), 0);
    const base = ["scripts/tests", "scripts", ""][depth - 1];
    assert.notEqual(base, undefined, `${name}: ${variable} climbs ${depth} levels above scripts/tests`);
    const parts = [...chain.matchAll(/["']([^"']+)["']/g)].map((m) => m[1]);
    if (parts.length) named.add([base, ...parts].filter(Boolean).join("/"));
    else assert.equal(base, { ROOT: "", REPO: "", SCRIPTS: "scripts" }[variable],
      `${name}: ${variable} (${base || "the repository root"}) is a checkout base this guard does not follow; name it ROOT or REPO (root) or SCRIPTS (scripts/)`);
  }
  return named;
}

// The Dev tools job's sparse-checkout list in pr-hygiene.yml. Windows
// checkouts use CRLF, so normalize line endings before matching on "\n".
function devToolsSparseCheckout(text) {
  const workflow = text.replace(/\r\n/g, "\n");
  const job = workflow.slice(workflow.indexOf("\n  dev-tools:"));
  const block = job.match(/sparse-checkout: \|\n((?:\s{12}\S.*\n)+)/);
  assert.ok(block, "the Dev tools job has a sparse-checkout list");
  return block[1].split("\n").map((line) => line.trim()).filter(Boolean);
}

// Whether a cone-mode sparse checkout of `dirs` holds `file`: cone mode
// always brings the files at the repository root.
function sparseHolds(dirs, file) {
  return !file.includes("/") || dirs.some((dir) => file === dir || file.startsWith(`${dir}/`));
}

test("the checkout path extractor follows local aliases of the repository root", () => {
  const source = 'class T:\n    def test(self):\n        root = SCRIPTS.parent\n' +
    '        text = (root / ".beads" / "README.md").read_text()\n        x = (SCRIPTS / "dev" / "queue")\n';
  assert.deepEqual([...namedCheckoutPaths("t.py", source)].sort(), [".beads/README.md", "scripts/dev/queue"]);
});

test("a cone-mode sparse checkout holds its directories and the root files only", () => {
  const dirs = ["scripts", "docs/backlog"];
  assert.ok(sparseHolds(dirs, "worker-contract.json"));
  assert.ok(sparseHolds(dirs, "scripts/dev/queue"));
  assert.ok(sparseHolds(dirs, "docs/backlog/frame.md"));
  assert.ok(!sparseHolds(dirs, ".beads/README.md"));
  assert.ok(!sparseHolds(dirs, "docs/backlogs/x.md"));
});

test("every checkout path the Python dev-tools tests name selects them", () => {
  const repo = path.dirname(path.dirname(path.dirname(fileURLToPath(import.meta.url))));
  const tests = readdirSync(path.join(repo, "scripts", "tests")).filter((name) => name.endsWith(".py"));
  const named = new Set();
  for (const name of tests) {
    for (const file of namedCheckoutPaths(name, readFileSync(path.join(repo, "scripts", "tests", name), "utf8"))) named.add(file);
  }
  const existing = [...named].filter((file) => existsSync(path.join(repo, file)));
  assert.ok(existing.includes("worker-contract.json") && existing.includes("scripts/dev/queue-pr.mjs"), [...named].join(", "));
  // ...and that the Dev tools job checks it out (#443: .beads/README.md was outside it).
  const sparse = devToolsSparseCheckout(readFileSync(path.join(repo, ".github", "workflows", "pr-hygiene.yml"), "utf8"));
  for (const file of existing) {
    const probe = statSync(path.join(repo, file)).isDirectory() ? `${file}/probe.py` : file; // a directory: what a test imports or reads from it
    assert.notDeepEqual(devToolsReasons([probe]), [], `${file} is read by a Python test but does not select the dev-tools tests`);
    assert.ok(sparseHolds(sparse, file), `${file} is read by a Python test but the Dev tools job's sparse checkout (${sparse.join(", ")}) leaves it out`);
  }
});
