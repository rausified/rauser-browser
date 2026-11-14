// Actions-minutes rules (brwr-3zqd, 2026-11-14): a main push that a newer
// push already replaced skips its builds (scripts/ci-superseded.mjs decides,
// tested in ci-superseded.test.mjs); the PR doctor's cron is hourly; the PR
// keeper wakes on every CI completion but a cancelled one. These tests read
// the workflow files as text (no YAML parser is a dependency) and run the
// Build and check gate's shell script with each combination of job results.
// The queue's merge conditions are pinned in mergify-gate.test.mjs.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const read = (rel) => readFileSync(new URL(`../../${rel}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");
const CI = read(".github/workflows/ci.yml");
const DOCTOR = read(".github/workflows/pr-doctor.yml");

/** The text of one top-level job of ci.yml, from its key to the next job. */
function job(id) {
  const parts = CI.split(/\n(?=  [a-z][a-z0-9-]*:\n)/);
  const found = parts.find((p) => p.startsWith(`  ${id}:\n`));
  assert.ok(found, `ci.yml has no job ${id}`);
  return found;
}

/** A job's `if:` expression, folded onto one line. */
function jobIf(id) {
  const m = job(id).match(/\n    if: (>-\n(?:      .*\n)+|.*\n)/);
  assert.ok(m, `job ${id} has no if`);
  return m[1].replace(/^>-\n/, "").replace(/\s+/g, " ").trim();
}

test("a main push already replaced by a newer one skips every build, and its gate", () => {
  assert.match(job("changes"), /superseded: \$\{\{ steps\.supersede\.outputs\.superseded \}\}/);
  assert.match(job("changes"), /- name: Skip a main push a newer push replaced\n\s+id: supersede\n/);
  for (const id of ["docs-checks", "a11y", "build-windows", "build-macos", "fuzz-check"]) {
    assert.ok(jobIf(id).includes("needs.changes.outputs.superseded != 'true'"), `${id} still builds a replaced push`);
  }
  assert.ok(jobIf("build-gate").includes("needs.changes.outputs.superseded != 'true'"));
  // The privacy scan and cargo-deny stay unconditional (cheap, and the scan
  // reads this push's own commit range).
  assert.doesNotMatch(job("privacy-scan"), /\n    if:/);
  assert.doesNotMatch(job("cargo-deny"), /\n    if:/);
});

const gateScript = () => {
  const run = job("build-gate").split("        run: |\n")[1];
  return run.split("\n").map((l) => l.replace(/^ {10}/, "")).join("\n");
};

function gate(env) {
  const base = {
    CHANGES: "success", DOCS_ONLY: "false", MACOS_NEEDED: "false",
    DOCS: "skipped", A11Y: "success", FUZZ: "success", PRIVACY: "success",
    MACOS: "skipped", WINDOWS: "success", STARVED: "", FALLBACK: "skipped",
  };
  const r = spawnSync("bash", ["-e", "-c", gateScript()], { env: { PATH: process.env.PATH, ...base, ...env }, encoding: "utf8" });
  return r.status;
}

test("the gate still requires Windows on every code change, macOS on main", { skip: process.platform === "win32" && "bash -e semantics are checked on macOS and Linux" }, () => {
  assert.equal(gate({}), 0, "PR with Windows green");
  assert.notEqual(gate({ WINDOWS: "skipped" }), 0, "Windows skipped on a code change");
  assert.notEqual(gate({ WINDOWS: "failure" }), 0);
  assert.notEqual(gate({ WINDOWS: "cancelled" }), 0);
  assert.equal(gate({ MACOS_NEEDED: "true", MACOS: "success" }), 0, "main push");
  assert.notEqual(gate({ MACOS_NEEDED: "true", MACOS: "skipped" }), 0);
  assert.notEqual(gate({ A11Y: "failure" }), 0);
  assert.equal(gate({ DOCS_ONLY: "true", DOCS: "success", A11Y: "skipped", FUZZ: "skipped", WINDOWS: "skipped" }), 0, "docs-only");
  assert.notEqual(gate({ PRIVACY: "failure" }), 0);
});

test("the PR doctor's cron is hourly and every CI completion but a cancelled one wakes the keeper", () => {
  assert.match(DOCTOR, /cron: "7 \* \* \* \*"/);
  assert.doesNotMatch(DOCTOR, /cron: "\*\/15/);
  const cond = DOCTOR.split("\n  keeper:\n")[1].split("concurrency:")[0];
  assert.match(cond, /github\.event\.workflow_run\.conclusion != 'cancelled'/);
  assert.doesNotMatch(cond, /conclusion == 'success'/, "a red queued PR must wake the keeper (review I3)");
});

test("every PR-triggered workflow cancels its superseded runs", () => {
  for (const rel of [".github/workflows/ci.yml", ".github/workflows/pr-hygiene.yml", ".github/workflows/queue-check.yml"]) {
    const top = read(rel).split("\njobs:\n")[0];
    assert.match(top, /\nconcurrency:\n\s+group: .*pull_request\.number/, `${rel} has no per-PR concurrency group`);
    assert.match(top, /cancel-in-progress: /, rel);
  }
});
