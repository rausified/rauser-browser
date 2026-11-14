#!/usr/bin/env node
// Is this push to main already covered by a newer push's CI run? ci.yml's
// `changes` job asks this a minute after a main push (Mergify merges a
// batch's PRs seconds apart, one push each). It answers yes only when all of
// these hold, and builds on every other answer, a failure included:
//   1. main's tip is a different commit (git ls-remote);
//   2. the tip contains this push: GitHub's compare of this...tip is
//      "ahead" (a force-push or rewrite never supersedes, review M5);
//   3. the tip's commit message has no skip marker, so GitHub runs workflows
//      for it at all (a Mergify merge commit carries the PR title, and
//      `[skip actions]` titles are used here, review I1);
//   4. a CI push run for the tip exists, or appears within the wait, and is
//      queued, running or green. Any other conclusion builds nothing for the
//      tip: cancelled or skipped leaves both pushes unbuilt, and a run that
//      already failed this soon after the push (no build finishes in 60-120
//      s) is a 0-step billing refusal or a crashed classify job (review N2).
//      A tip run that fails later, after building, still counts.
// Writes `superseded=true` to $GITHUB_OUTPUT when it is superseded.
import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

// GitHub's markers: https://docs.github.com/actions/managing-workflow-runs/skipping-workflow-runs
const SKIP = /\[(skip ci|ci skip|no ci|skip actions|actions skip)\]/i;

export const hasSkipMarker = (message) => SKIP.test(message);

// Tip run states that will build it: not yet concluded ("pending") or green.
const LIVE = new Set(["pending", "success"]);

const build = (reason) => ({ superseded: false, reason });

/**
 * @param {{ sha: string, repo: string, run: (cmd: string, args: string[]) => string,
 *   sleep: (seconds: number) => void, waitSeconds?: number, pollSeconds?: number }} o
 */
export function decideSuperseded({ sha, repo, run, sleep, waitSeconds = 60, pollSeconds = 10 }) {
  const ask = (cmd, args) => {
    try { return { out: run(cmd, args) }; } catch (e) { return { err: String(e.message ?? e).split("\n")[0] }; }
  };
  const remote = ask("git", ["ls-remote", "origin", "refs/heads/main"]);
  if (remote.err) return build(`cannot read main (${remote.err}): building`);
  const tip = (remote.out.trim().split(/\s+/)[0] ?? "").trim();
  if (!/^[0-9a-f]{40}$/.test(tip)) return build("main's tip is unreadable: building");
  if (tip === sha) return build(`main is still ${sha}: building`);
  const cmp = ask("gh", ["api", `repos/${repo}/compare/${sha}...${tip}`, "--jq", ".status"]);
  if (cmp.err) return build(`cannot compare with main's tip ${tip} (${cmp.err}): building`);
  if (cmp.out.trim() !== "ahead") return build(`main's tip ${tip} does not contain ${sha} (compare: ${cmp.out.trim()}): building`);
  const msg = ask("gh", ["api", `repos/${repo}/commits/${tip}`, "--jq", ".commit.message"]);
  if (msg.err) return build(`cannot read ${tip}'s message (${msg.err}): building`);
  if (hasSkipMarker(msg.out)) return build(`main's tip ${tip} carries a skip marker, so it gets no CI run: building`);
  const query = ["api", `repos/${repo}/actions/runs?head_sha=${tip}&event=push&per_page=50`,
    "--jq", '[.workflow_runs[] | select(.path == ".github/workflows/ci.yml") | (.conclusion // "pending")] | join(" ")'];
  for (let waited = 0; ; waited += pollSeconds) {
    const runs = ask("gh", query);
    if (runs.err) return build(`cannot list ${tip}'s runs (${runs.err}): building`);
    const live = runs.out.trim().split(/\s+/).filter((c) => LIVE.has(c));
    if (live.length > 0) return { superseded: true, reason: `superseded: main is now ${tip}, which contains ${sha} and has its own CI push run` };
    if (waited >= waitSeconds) return build(`main's tip ${tip} has no queued, running or green CI push run after ${waitSeconds}s: building`);
    sleep(pollSeconds);
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const run = (cmd, args) => execFileSync(cmd, args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
  const sleep = (s) => Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, s * 1000);
  const d = decideSuperseded({ sha: process.env.GITHUB_SHA ?? "", repo: process.env.GITHUB_REPOSITORY ?? "", run, sleep });
  console.log(d.reason);
  if (d.superseded && process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, "superseded=true\n");
}
