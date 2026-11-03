import assert from "node:assert/strict";
import test from "node:test";

import { formatSeconds, isInScope, nodeProblem, outOfScope, selectSteps, STEPS, targetDirProblem } from "../gate-adapter.mjs";

const names = (files, options) => selectSteps(files, options).map((step) => step.name);

// The Confluence starter (6036259) and its follow-up (77603d1), and the Miro
// starter (029a398): the changes this gate exists for.
const CONFLUENCE = [
  "adapter-templates/tests/structural.rs",
  "extension/capture-site-panel.ts",
  "extension/capture-site.ts",
  "extension/options.html",
  "extension/options.ts",
  "extension/panel.html",
  "extension/starter-adapters.ts",
  "extension/tests/capture-site.test.mjs",
  "extension/tests/options.test.mjs",
  "extension/tests/panel/capture-explanation.test.mjs",
  "extension/tests/panel/capture-site.test.mjs",
  "extension/tests/site-coverage.test.mjs",
  "extension/tests/starter-adapters.test.mjs",
  "host/tests/fixtures/capture-site-structural-response.json",
  "protocol/adapter-templates/vectors.json",
  "protocol/capture-site/catalog.json",
  "protocol/capture-site/shapes.json",
  "README.md",
  "docs/adapters/confluence.page.md",
];
const MIRO = [
  ".beads/issues.jsonl",
  "BOSUN_FEATURE_BACKLOG.md",
  "DESIGN.md",
  "README.md",
  "adapter-templates.md",
  "adapter-templates/tests/structural.rs",
  "docs/adapters/miro.board.md",
  "extension/starter-adapters.ts",
  "extension/tests/panel/capture-site.test.mjs",
  "extension/tests/starter-adapters.test.mjs",
  "host/tests/fixtures/capture-site-structural-response.json",
  "protocol/adapter-templates/README.md",
  "protocol/adapter-templates/vectors.json",
  "protocol/capture-site/catalog.json",
  "protocol/capture-site/shapes.json",
];

test("the Confluence and Miro starter changes are in scope", () => {
  assert.deepEqual(outOfScope(CONFLUENCE), []);
  assert.deepEqual(outOfScope(MIRO), []);
});

test("docs, queue files and site-draft fixtures are in scope", () => {
  for (const file of [
    "AGENTS.md",
    "TESTING.md",
    "docs/adapters/github.pr.md",
    ".beads/issues.jsonl",
    "extension/capture-site-handoff.ts",
    "extension/site-coverage.ts",
    "extension/tests/a11y/pages.test.mjs",
    "adapter-templates/tests/agent_draft.rs",
    "host/tests/fixtures/site-draft-answers/agent_planted_root_match_is_dropped.json",
  ]) {
    assert.equal(isInScope(file), true, file);
  }
});

test("engine, host code, the panel script, protocol schema and tooling need the full gate", () => {
  for (const file of [
    "adapter-templates/src/structural.rs",
    "adapter-templates/fuzz/fuzz_targets/structural_draft.rs",
    "extension/adapter-templates.ts",
    "extension/panel.ts",
    "extension/worker.ts",
    "extension/build.mjs",
    "host/src/capture_site.rs",
    "host/tests/fixtures/miro-board-urls.json",
    "host/tests/service.rs",
    "protocol/schema.json",
    "protocol/README.md",
    "protocol/strip-params/vectors.json",
    "SUMMARY.md",
    "package.json",
    "Cargo.lock",
    "scripts/gate-adapter.mjs",
    ".github/workflows/ci.yml",
    "extension/capture-site/nested.ts",
    "extension/capture-site.ts.orig",
    "protocol/capture-site/",
    "extension/tests/",
    "",
  ]) {
    assert.equal(isInScope(file), false, JSON.stringify(file));
  }
  assert.deepEqual(outOfScope(["README.md", "host/src/capture_site.rs"]), ["host/src/capture_site.rs"]);
});

test("the Confluence change runs every extension, engine and host check plus a11y for its HTML", () => {
  assert.deepEqual(names(CONFLUENCE), [
    "status docs",
    "typecheck",
    "lint",
    "build:extension",
    "test:extension",
    "test:a11y",
    "cargo fmt",
    "adapter-templates tests",
    "adapter-templates clippy",
    "host capture_site tests",
  ]);
});

test("the Miro change checks the queue and skips a11y (no page markup changed)", () => {
  const steps = names(MIRO);
  assert.ok(steps.includes("queue consistency"));
  assert.ok(!steps.includes("test:a11y"));
  assert.ok(steps.includes("host capture_site tests"));
});

test("a docs-only change runs only the cheap docs checks", () => {
  assert.deepEqual(names(["docs/adapters/miro.board.md", "README.md"]), ["status docs"]);
  assert.deepEqual(names([".beads/issues.jsonl"]), ["status docs", "queue consistency"]);
});

test("a vectors-only change skips the host and Rust lints", () => {
  assert.deepEqual(names(["protocol/adapter-templates/vectors.json"]), [
    "status docs",
    "typecheck",
    "lint",
    "build:extension",
    "test:extension",
    "adapter-templates tests",
  ]);
});

test("an extension-only change skips cargo entirely", () => {
  const steps = names(["extension/starter-adapters.ts", "extension/tests/starter-adapters.test.mjs"]);
  assert.deepEqual(steps, ["status docs", "typecheck", "lint", "build:extension", "test:extension"]);
});

test("a site-draft fixture runs the engine's and the host's tests but not the extension's", () => {
  assert.deepEqual(names(["host/tests/fixtures/site-draft-answers/x.json"]), [
    "status docs",
    "adapter-templates tests",
    "host capture_site tests",
  ]);
});

test("--all-steps selects every step in order", () => {
  assert.deepEqual(names([], { all: true }), STEPS.map((step) => step.name));
});

test("only Node 24 passes the version check", () => {
  assert.equal(nodeProblem("v24.14.0"), null);
  assert.equal(nodeProblem("v24.0.1"), null);
  for (const version of ["v22.12.0", "v25.0.0", "v2.4.0", "", undefined]) {
    assert.match(nodeProblem(version), /not v24/, String(version));
  }
});

test("a CARGO_TARGET_DIR outside the checkout is refused", () => {
  assert.equal(targetDirProblem(undefined, "/repo"), null);
  assert.equal(targetDirProblem("", "/repo"), null);
  assert.equal(targetDirProblem("target/x", "/repo"), null);
  assert.equal(targetDirProblem("/repo/target", "/repo"), null);
  assert.match(targetDirProblem("/other-worktree/target", "/repo"), /outside this checkout/);
  assert.match(targetDirProblem("../sibling/target", "/repo"), /outside this checkout/);
});

test("durations print as seconds under a minute and minutes after", () => {
  assert.equal(formatSeconds(1234), "1.2s");
  assert.equal(formatSeconds(125_000), "2m5s");
  assert.equal(formatSeconds(59_960), "1m0s");
  assert.equal(formatSeconds(119_700), "2m0s");
});

test("runs as the entry point from a checkout path with a space (PR #264, Codex P2)", async () => {
  const { mkdtempSync, copyFileSync, mkdirSync, rmSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join, dirname } = await import("node:path");
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const here = dirname(fileURLToPath(import.meta.url));
  const root = mkdtempSync(join(tmpdir(), "gate adapter "));
  try {
    mkdirSync(join(root, "scripts"));
    for (const f of ["gate-adapter.mjs", "ci-changes.mjs"]) copyFileSync(join(here, "..", f), join(root, "scripts", f));
    const r = spawnSync(process.execPath, [join(root, "scripts", "gate-adapter.mjs"), "--dry-run", "--path", "host/src/main.rs"], { cwd: root, encoding: "utf8" });
    assert.equal(r.status, 2, `expected a scope refusal, got ${r.status}: ${r.stdout}${r.stderr}`);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
