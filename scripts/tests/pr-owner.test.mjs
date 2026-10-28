import assert from "node:assert/strict";
import test from "node:test";

import { checkPrOwner, isCookiePr } from "../pr-owner.mjs";
import { ipcDenied } from "./ipc-guard.mjs";

function pr({ body = "", head = "feat/x", title = "feat: x", author = "rausified" } = {}) {
  return { body, title, head: { ref: head }, user: { login: author } };
}

test("a PR body with an Owner line passes", () => {
  const result = checkPrOwner(pr({ body: "Owner: claude session 2e52fe72 (lane D)\n\n## Summary\n" }));
  assert.equal(result.ok, true);
  assert.equal(result.owner, "claude session 2e52fe72 (lane D)");
});

test("the Owner line may be bold or a list item, as the template renders it", () => {
  assert.equal(checkPrOwner(pr({ body: "**Owner:** codex session abc" })).ok, true);
  assert.equal(checkPrOwner(pr({ body: "- Owner: John" })).ok, true);
  assert.equal(checkPrOwner(pr({ body: "**Owner**: John" })).ok, true);
});

test("a PR body without an Owner line fails and says how to fix it", () => {
  const result = checkPrOwner(pr({ body: "## Summary\nDoes things.\n" }));
  assert.equal(result.ok, false);
  assert.match(result.reason, /Owner:/);
});

test("an empty or missing body fails", () => {
  assert.equal(checkPrOwner(pr({ body: "" })).ok, false);
  assert.equal(checkPrOwner({ title: "t", head: { ref: "feat/x" }, body: null }).ok, false);
});

test("the unfilled template placeholder does not count as an owner", () => {
  const body = "Owner: <!-- the session that owns this PR -->\nLane / coordinator: <!-- lane -->\n";
  assert.equal(checkPrOwner(pr({ body })).ok, false);
});

test("an Owner line with only whitespace or angle-bracket placeholder text fails", () => {
  assert.equal(checkPrOwner(pr({ body: "Owner:   \n" })).ok, false);
  assert.equal(checkPrOwner(pr({ body: "Owner: <session>\n" })).ok, false);
});

test("Owner mentioned mid-sentence is not an Owner line", () => {
  assert.equal(checkPrOwner(pr({ body: "The Owner: field is described in AGENTS.md" })).ok, false);
});

test("an Owner line inside a fenced code block does not count", () => {
  const body = "Example:\n```\nOwner: someone\n```\n";
  assert.equal(checkPrOwner(pr({ body })).ok, false);
});

test("Cookie driver PRs are exempt by branch prefix", () => {
  const result = checkPrOwner(pr({ head: "cookie/cookie-overnight/cycle-28-35393eb", title: "feat: y" }));
  assert.equal(result.ok, true);
  assert.equal(result.exempt, true);
});

test("Cookie driver PRs are exempt by a 'Cookie cycle' title", () => {
  assert.equal(isCookiePr(pr({ title: "Cookie cycle 28: feat: review capture" })), true);
  assert.equal(checkPrOwner(pr({ title: "Cookie cycle 3: fix" })).ok, true);
});

test("a branch that merely contains 'cookie' is not exempt", () => {
  assert.equal(isCookiePr(pr({ head: "feat/cookie-banner", title: "feat: cookie banner" })), false);
  assert.equal(checkPrOwner(pr({ head: "feat/cookie-banner", title: "feat: cookie banner" })).ok, false);
});

test("the CLI fails the job for a PR event without an owner and passes one with it", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { mkdtempSync, writeFileSync } = await import("node:fs");
  const { tmpdir } = await import("node:os");
  const { join } = await import("node:path");
  const { spawnSync } = await import("node:child_process");
  const script = new URL("../pr-owner.mjs", import.meta.url).pathname;
  const dir = mkdtempSync(join(tmpdir(), "pr-owner-"));
  const run = (pull_request) => {
    const path = join(dir, `event-${Math.random()}.json`);
    writeFileSync(path, JSON.stringify({ pull_request }));
    return spawnSync(process.execPath, [script], { env: { ...process.env, GITHUB_EVENT_PATH: path }, encoding: "utf8" });
  };
  const missing = run(pr({ body: "no owner here" }));
  assert.equal(missing.status, 1);
  assert.match(missing.stdout, /::error/);
  const named = run(pr({ body: "Owner: lane D session" }));
  assert.equal(named.status, 0, named.stderr);
  assert.match(named.stdout, /Owner: lane D session/);
});
