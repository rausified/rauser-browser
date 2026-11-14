// Actions-minutes rules (brwr-3zqd, 2026-11-14): Windows runs on Mergify's
// queue drafts and on main, not on every PR push; the queue's merge
// conditions are checked on the draft, so Windows still gates every merge;
// a main push that a newer push already replaced skips its builds; the PR
// keeper runs after a green CI only. These tests read the workflow files as
// text (no YAML parser is a dependency) and run the Build and check gate's
// shell script with each combination of job results.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";

const read = (rel) => readFileSync(new URL(`../../${rel}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");
const CI = read(".github/workflows/ci.yml");
const MERGIFY = read(".mergify.yml");
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

const WINDOWS_WHEN = "(github.event_name != 'pull_request' || startsWith(github.head_ref, 'mergify/merge-queue/'))";

test("Windows builds run on queue drafts, main pushes and merge groups, never on a plain PR push", () => {
  const cond = jobIf("build-windows");
  assert.ok(cond.includes(WINDOWS_WHEN), cond);
  assert.ok(cond.includes("needs.changes.outputs.docs_only != 'true'"), cond);
  assert.ok(cond.includes("needs.changes.outputs.superseded != 'true'"), cond);
  // The gate requires Windows under exactly the same rule.
  assert.ok(job("build-gate").includes(`WINDOWS_NEEDED: \${{ ${WINDOWS_WHEN} && 'true' || 'false' }}`));
});

test("the queue's merge conditions are checked on the draft and require its Build and check", () => {
  const rule = MERGIFY.split("queue_rules:")[1];
  const merge = rule.split("merge_conditions:")[1];
  assert.ok(merge, ".mergify.yml has no merge_conditions: the queue would merge on the PR's own (Windows-less) checks");
  const conds = merge.split(/\n    [a-z_]+:/)[0];
  assert.match(conds, /- check-success = Build and check\n/);
  assert.match(conds, /- check-success = Rust dependency policy\n/);
  // Mergify checks a PR in place (on its own head, no draft) only with
  // batch_size 1, max_parallel_checks 1, max_checks_retries 0 and no
  // separate merge_conditions. Each of these alone forces a draft.
  assert.doesNotMatch(rule, /batch_size: 1\n/);
  assert.match(rule, /max_checks_retries: [1-9]/);
  assert.match(MERGIFY, /max_parallel_checks: [2-9]/);
});

test("a main push already replaced by a newer one skips every build, and its gate", () => {
  assert.match(job("changes"), /superseded: \$\{\{ steps\.supersede\.outputs\.superseded \}\}/);
  const step = job("changes").split("- name: Skip a main push a newer push replaced")[1];
  assert.ok(step, "changes has no supersede step");
  assert.match(step, /github\.event_name == 'push'/);
  assert.match(step, /git ls-remote/);
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
    CHANGES: "success", DOCS_ONLY: "false", MACOS_NEEDED: "false", WINDOWS_NEEDED: "false",
    DOCS: "skipped", A11Y: "success", FUZZ: "success", PRIVACY: "success",
    MACOS: "skipped", WINDOWS: "skipped", STARVED: "", FALLBACK: "skipped",
  };
  const r = spawnSync("bash", ["-e", "-c", gateScript()], { env: { PATH: process.env.PATH, ...base, ...env }, encoding: "utf8" });
  return r.status;
}

test("the gate: a plain PR passes without Windows, a draft or main needs it", { skip: process.platform === "win32" && "bash -e semantics are checked on macOS and Linux" }, () => {
  assert.equal(gate({}), 0, "plain PR, Windows skipped");
  assert.notEqual(gate({ WINDOWS: "success" }), 0, "a plain PR that ran Windows means the if drifted from the gate");
  assert.equal(gate({ WINDOWS_NEEDED: "true", WINDOWS: "success" }), 0, "draft with Windows green");
  assert.notEqual(gate({ WINDOWS_NEEDED: "true", WINDOWS: "skipped" }), 0, "draft without Windows");
  assert.notEqual(gate({ WINDOWS_NEEDED: "true", WINDOWS: "failure" }), 0, "draft with Windows red");
  assert.notEqual(gate({ WINDOWS_NEEDED: "true", WINDOWS: "cancelled" }), 0);
  assert.equal(gate({ WINDOWS_NEEDED: "true", WINDOWS: "success", MACOS_NEEDED: "true", MACOS: "success" }), 0, "main push");
  assert.notEqual(gate({ WINDOWS_NEEDED: "true", WINDOWS: "success", MACOS_NEEDED: "true", MACOS: "skipped" }), 0);
  assert.notEqual(gate({ A11Y: "failure" }), 0);
  assert.equal(gate({ DOCS_ONLY: "true", DOCS: "success", A11Y: "skipped", FUZZ: "skipped", WINDOWS_NEEDED: "true" }), 0, "docs-only draft");
  assert.notEqual(gate({ PRIVACY: "failure" }), 0);
});

test("the PR doctor's cron is hourly and CI completions wake the keeper only when CI passed", () => {
  assert.match(DOCTOR, /cron: "7 \* \* \* \*"/);
  assert.doesNotMatch(DOCTOR, /cron: "\*\/15/);
  const cond = DOCTOR.split("\n  keeper:\n")[1].split("concurrency:")[0];
  assert.match(cond, /github\.event\.workflow_run\.conclusion == 'success'/);
});

test("every PR-triggered workflow cancels its superseded runs", () => {
  for (const rel of [".github/workflows/ci.yml", ".github/workflows/pr-hygiene.yml", ".github/workflows/queue-check.yml"]) {
    const top = read(rel).split("\njobs:\n")[0];
    assert.match(top, /\nconcurrency:\n\s+group: .*pull_request\.number/, `${rel} has no per-PR concurrency group`);
    assert.match(top, /cancel-in-progress: /, rel);
  }
});
