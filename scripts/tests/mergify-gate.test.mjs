// The merge queue's gate (.mergify.yml): queue_conditions are checked on each
// PR before it enters; merge_conditions are checked on the queue draft (main
// plus the batch) before anything merges. Without merge_conditions, Mergify
// merged on the PRs' own checks alone (drafts #464 to #485, 2026-11-14).
// The file is read as text: no YAML parser is a dependency.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const MERGIFY = readFileSync(new URL("../../.mergify.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");

/** The `- check...` lines of the key `name` at `indent` spaces, or null.
 * Anchored on a whole line, so `merge_conditions` never matches
 * `auto_merge_conditions`. */
export function conditions(yml, name, indent = 4) {
  const pad = " ".repeat(indent);
  const start = yml.indexOf(`\n${pad}${name}:\n`);
  if (start < 0) return null;
  const lines = yml.slice(start + 1).split("\n").slice(1);
  const out = [];
  for (const line of lines) {
    if (line.trim() === "" || line.trimStart().startsWith("#")) continue;
    const lead = line.length - line.trimStart().length;
    if (lead <= indent) break;
    const m = line.match(/^\s*- (.+)$/);
    if (m) out.push(m[1].trim());
  }
  return out;
}

const rule = () => {
  const body = MERGIFY.split("\nqueue_rules:\n")[1];
  assert.ok(body, "no queue_rules");
  assert.match(body, /^  - name: default\n/);
  return `\n${body.split(/\n(?=[a-z_]+:)/)[0]}`;
};

test("the parser reads only an exact key: auto_merge_conditions is not merge_conditions (review M3)", () => {
  const only = "queue_rules:\n  - name: default\n    queue_conditions:\n      - label = queue\nmerge_protections_settings:\n  auto_merge_conditions:\n    - label = queue\n";
  assert.equal(conditions(only, "merge_conditions"), null);
  const yml = "x:\n    merge_conditions:\n      # a comment\n      - check-success = A\n\n      - check-success = B\n    other: 1\n      - check-success = C\n";
  assert.deepEqual(conditions(yml, "merge_conditions"), ["check-success = A", "check-success = B"]);
});

test("the queue draft must pass Build and check, Rust dependency policy, PR owner and Dev tools", () => {
  const merge = conditions(rule(), "merge_conditions");
  assert.ok(merge, ".mergify.yml's default rule has no merge_conditions: the queue merges on the PRs' own checks");
  for (const check of ["Build and check", "Rust dependency policy", "PR owner", "Dev tools"]) {
    assert.ok(merge.includes(`check-success = ${check}`), `merge_conditions lack check-success = ${check}`);
  }
  // Every merge condition is a required check that the draft runs; nothing
  // weaker (check-skipped, check-neutral) may stand in for one.
  for (const c of merge) assert.match(c, /^check-success = /, c);
});

test("merge_conditions differ from queue_conditions, so the rule is two-step and never checks in place", () => {
  const queue = conditions(rule(), "queue_conditions");
  const merge = conditions(rule(), "merge_conditions");
  assert.ok(queue && queue.length > 0);
  assert.ok(merge, "no merge_conditions: Mergify may check in place and merge on the PR's own checks");
  assert.notDeepEqual(merge, queue);
  // Each queue check stays required on the PR too.
  for (const check of ["Build and check", "Rust dependency policy", "PR owner"]) {
    assert.ok(queue.includes(`check-success = ${check}`), check);
  }
  // Mergify checks in place only with batch_size 1, max_parallel_checks 1,
  // mode serial, max_checks_retries 0 and no separate merge_conditions.
  assert.doesNotMatch(rule(), /\n    batch_size: 1\n/);
  assert.match(rule(), /\n    max_checks_retries: [1-9]/);
  assert.match(MERGIFY, /\n  max_parallel_checks: [2-9]/);
});
