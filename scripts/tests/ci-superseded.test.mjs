// scripts/ci-superseded.mjs: a main push skips its builds only when main's
// newer tip contains it, carries no skip marker and has a CI push run of its
// own (review I1, M5 of #490). Every other answer, including any failure,
// builds.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { decideSuperseded, hasSkipMarker } from "../ci-superseded.mjs";

const SHA = "a".repeat(40);
const TIP = "b".repeat(40);
const REPO = "owner/repo";

/** A fake `run(cmd, args)` answering from a table keyed by the args' API path. */
function fake({ tip = TIP, status = "ahead", message = "Merge pull request #2 from x/y\n\nfeat: thing", runs = ["pending"], fail = [] } = {}) {
  const calls = [];
  let runCalls = 0;
  const run = (cmd, args) => {
    calls.push([cmd, ...args].join(" "));
    const key = cmd === "git" ? "ls-remote" : args[1].startsWith(`repos/${REPO}/compare/`) ? "compare"
      : args[1].startsWith(`repos/${REPO}/commits/`) ? "commit" : args[1].startsWith(`repos/${REPO}/actions/runs`) ? "runs" : "?";
    if (fail.includes(key)) throw new Error(`${key} failed`);
    if (key === "ls-remote") return tip === null ? "" : `${tip}\trefs/heads/main\n`;
    if (key === "compare") return `${status}\n`;
    if (key === "commit") return message;
    if (key === "runs") return `${runs[Math.min(runCalls++, runs.length - 1)]}\n`;
    throw new Error(`unexpected ${cmd} ${args.join(" ")}`);
  };
  return { run, calls, sleeps: [] };
}

const decide = (f, opts = {}) => decideSuperseded({ sha: SHA, repo: REPO, run: f.run, sleep: (s) => f.sleeps.push(s), ...opts });

test("a newer tip that contains this push and has its own CI push run supersedes it", () => {
  const f = fake();
  const d = decide(f);
  assert.equal(d.superseded, true, d.reason);
  assert.match(d.reason, new RegExp(TIP));
  assert.ok(f.calls.some((c) => c.includes(`repos/${REPO}/actions/runs?head_sha=${TIP}&event=push`)));
});

test("main still at this push, or unreadable, builds", () => {
  assert.equal(decide(fake({ tip: SHA })).superseded, false);
  assert.equal(decide(fake({ tip: null })).superseded, false);
  assert.equal(decide(fake({ fail: ["ls-remote"] })).superseded, false);
});

test("M5: a tip that does not contain this push (a force-push, a rewrite) never supersedes it", () => {
  for (const status of ["diverged", "behind", "identical"]) {
    assert.equal(decide(fake({ status })).superseded, false, status);
  }
  assert.equal(decide(fake({ fail: ["compare"] })).superseded, false);
});

test("I1: a tip whose commit message carries a skip marker gets no run, so this push builds", () => {
  for (const marker of ["[skip ci]", "[ci skip]", "[no ci]", "[skip actions]", "[actions skip]", "[SKIP ACTIONS]", "[Skip CI]"]) {
    const f = fake({ message: `Merge pull request #9 from x/y\n\ndocs: tidy ${marker}` });
    assert.equal(decide(f).superseded, false, marker);
    assert.ok(!f.calls.some((c) => c.includes("actions/runs")), "no need to look for a run");
  }
  assert.equal(decide(fake({ fail: ["commit"] })).superseded, false);
});

test("I1: no CI push run for the tip within the wait builds; one that appears late still supersedes", () => {
  const none = fake({ runs: [""] });
  assert.equal(decide(none, { waitSeconds: 30, pollSeconds: 10 }).superseded, false);
  assert.deepEqual(none.sleeps, [10, 10, 10]);
  const late = fake({ runs: ["", "", "pending"] });
  assert.equal(decide(late, { waitSeconds: 30, pollSeconds: 10 }).superseded, true);
  assert.deepEqual(late.sleeps, [10, 10]);
  assert.equal(decide(fake({ fail: ["runs"] }), { waitSeconds: 0 }).superseded, false);
});

test("Codex P2, review N2: only a queued, running or green replacement run supersedes", () => {
  // A run already failed 60-120 s after its push built nothing: a 0-step
  // billing refusal or a crashed classify job.
  for (const runs of ["cancelled", "skipped", "cancelled skipped", "startup_failure", "failure", "timed_out", "action_required", "neutral", "stale"]) {
    assert.equal(decide(fake({ runs: [runs] }), { waitSeconds: 0 }).superseded, false, runs);
  }
  for (const runs of ["pending", "success", "cancelled pending", "failure success"]) {
    assert.equal(decide(fake({ runs: [runs] }), { waitSeconds: 0 }).superseded, true, runs);
  }
});

test("hasSkipMarker matches GitHub's markers only", () => {
  assert.equal(hasSkipMarker("fix: thing"), false);
  assert.equal(hasSkipMarker("skip ci for docs"), false, "no brackets, no marker");
  assert.equal(hasSkipMarker("x\n\n[skip  ci]"), false, "GitHub needs one space");
  assert.equal(hasSkipMarker("x [no ci] y"), true);
});

test("ci.yml runs the script after the wait, with the token and actions: read it needs", () => {
  const ci = readFileSync(new URL("../../.github/workflows/ci.yml", import.meta.url), "utf8").replace(/\r\n/g, "\n");
  const changes = ci.split("\n  changes:\n")[1].split(/\n  [a-z][a-z0-9-]*:\n/)[0];
  assert.match(changes, /\n    permissions:\n      contents: read\n      actions: read\n/);
  const step = changes.split("- name: Skip a main push a newer push replaced")[1];
  assert.match(step, /if: github\.event_name == 'push' && github\.ref == 'refs\/heads\/main'/);
  assert.match(step, /GH_TOKEN: \$\{\{ github\.token \}\}/);
  assert.match(step, /sleep 60\n\s+node scripts\/ci-superseded\.mjs/);
});

// Review N4: the command itself (env, execFileSync, the GITHUB_OUTPUT append)
// with stub git and gh first on PATH. The stubs answer by URL; the real jq
// filter was checked against the live runs API by hand (PR body).
test("the command writes superseded=true for a live tip run, and nothing when it builds", { skip: process.platform === "win32" && "sh stubs; the decision itself is tested above on every platform" }, async () => {
  const { mkdtempSync, writeFileSync, readFileSync: readText, chmodSync, existsSync, rmSync } = await import("node:fs");
  const { join } = await import("node:path");
  const { tmpdir } = await import("node:os");
  const { spawnSync } = await import("node:child_process");
  const { fileURLToPath } = await import("node:url");
  const script = fileURLToPath(new URL("../ci-superseded.mjs", import.meta.url));
  const runCase = ({ tip, message = "Merge pull request #2 from x/y" }) => {
    const dir = mkdtempSync(join(tmpdir(), "ci-superseded-"));
    try {
      const stub = (name, body) => { writeFileSync(join(dir, name), `#!/bin/sh\n${body}\n`); chmodSync(join(dir, name), 0o755); };
      stub("git", `printf '%s\\trefs/heads/main\\n' ${tip}`);
      stub("gh", [
        'case "$2" in',
        `  repos/${REPO}/compare/*) echo ahead ;;`,
        `  repos/${REPO}/commits/*) echo "${message}" ;;`,
        `  repos/${REPO}/actions/runs*) echo pending ;;`,
        '  *) echo "unexpected $*" >&2; exit 1 ;;',
        "esac",
      ].join("\n"));
      const out = join(dir, "output");
      const r = spawnSync(process.execPath, [script], {
        encoding: "utf8",
        env: { PATH: `${dir}:${process.env.PATH}`, GITHUB_SHA: SHA, GITHUB_REPOSITORY: REPO, GITHUB_OUTPUT: out },
      });
      return { status: r.status, stdout: r.stdout + r.stderr, output: existsSync(out) ? readText(out, "utf8") : "" };
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  };
  const live = runCase({ tip: TIP });
  assert.equal(live.status, 0, live.stdout);
  assert.equal(live.output, "superseded=true\n");
  assert.match(live.stdout, /^superseded: main is now b{40}/);
  // Building writes nothing to GITHUB_OUTPUT and still exits 0.
  const still = runCase({ tip: SHA });
  assert.deepEqual([still.status, still.output], [0, ""], still.stdout);
  assert.match(still.stdout, /main is still a{40}: building/);
  const marked = runCase({ tip: TIP, message: "docs: tidy [skip actions]" });
  assert.deepEqual([marked.status, marked.output], [0, ""], marked.stdout);
});
