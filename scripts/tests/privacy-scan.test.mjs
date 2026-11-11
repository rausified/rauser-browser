import assert from "node:assert/strict";
import { execFileSync, spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import {
  addEntry,
  canonical,
  evidenceTokens,
  hashToken,
  parseDenylist,
  scanText,
} from "../privacy-scan.mjs";
import { quietGitEnv } from "./git-test-env.mjs";

const HERE = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(HERE, "..", "privacy-scan.mjs");
const COMMITTED = join(HERE, "..", "privacy-denylist.sha256");
// Synthetic: no real organization, tenant or person.
const SECRET = "Zephyr-Quokka_77";
const SALT = "00112233445566778899aabbccddeeff";

function scratch(t) {
  const dir = mkdtempSync(join(tmpdir(), "privacy-scan-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function denylistWith(dir, ...values) {
  const path = join(dir, "deny.sha256");
  writeFileSync(path, `# test denylist\nsalt ${SALT}\n`);
  for (const value of values) addEntry(path, value);
  return path;
}

function denylist(...values) {
  let text = `salt ${SALT}\n`;
  for (const value of values) {
    const c = canonical(value);
    text += `${hashToken(SALT, c)} ${c.split("-").length}\n`;
  }
  return parseDenylist(text);
}

// Never read the developer's real local person denylist in a test.
const NO_LOCAL = join(tmpdir(), "privacy-scan-no-local-denylist.sha256");

function run(args, opts = {}) {
  const env = { ...process.env, BRAUSER_PRIVACY_DENYLIST_LOCAL: NO_LOCAL, ...opts.env };
  return spawnSync(process.execPath, [SCRIPT, ...args], { encoding: "utf8", ...opts, env });
}

function git(cwd, ...args) {
  return execFileSync("git", args, { cwd, env: quietGitEnv(), encoding: "utf8" });
}

test("canonical form folds case and hyphen, underscore and dot variants together", () => {
  const want = "zephyr-quokka-77";
  for (const v of ["Zephyr-Quokka_77", "zephyr.quokka.77", "ZEPHYR_QUOKKA-77", "zephyr quokka 77"]) {
    assert.equal(canonical(v), want, v);
  }
});

test("a denylisted token is caught in file content, in any separator or case", () => {
  const list = denylist(SECRET);
  const text = "line one\nsee https://example.com/zephyr.quokka.77/repo\r\nclean\nZEPHYR_QUOKKA-77 again\n";
  assert.deepEqual(scanText(text, list), [2, 4]);
  assert.deepEqual(scanText("zephyr quokka seventy-seven\nzephyrquokka77\n", list), []);
});

test("a denylisted token is caught URL-encoded, once or twice", () => {
  const list = denylist(SECRET);
  assert.deepEqual(scanText("?org=zephyr%2Dquokka%5F77&x=1\n", list), [1]);
  assert.deepEqual(scanText("?org=zephyr%252Dquokka%255F77\n", list), [1]);
  assert.deepEqual(scanText("Zephyr%20Quokka%2077\n", list), [1]);
});

test("a multi-word entry matches inside a longer run of words but not a partial one", () => {
  const list = denylist("Northwind Quarterly Plan");
  assert.deepEqual(scanText("Board: Northwind-Quarterly-Plan v2\nNorthwind Quarterly\n", list), [1]);
});

test("the message mode catches a denylisted token in a commit message or PR body", (t) => {
  const dir = scratch(t);
  const deny = denylistWith(dir, SECRET);
  const msg = join(dir, "msg.txt");
  writeFileSync(msg, "fix: thing\n\nSeen on the zephyr-quokka-77 tenant.\n");
  const result = run(["--denylist", deny, "--message", msg]);
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stdout, /msg\.txt:3/);
  assert.doesNotMatch(result.stdout + result.stderr, /zephyr|quokka/i);
  const fromStdin = run(["--denylist", deny, "--message", "-"], { input: "clean message\n" });
  assert.equal(fromStdin.status, 0, fromStdin.stdout + fromStdin.stderr);
});

test("the tree mode scans every tracked file and the staged mode the index", (t) => {
  const dir = scratch(t);
  const deny = denylistWith(dir, SECRET);
  const repo = join(dir, "repo");
  mkdirSync(join(repo, "docs"), { recursive: true });
  git(dir, "init", "-q", repo);
  writeFileSync(join(repo, "docs", "plan.md"), "# Plan\n\nThe org is zephyr_quokka_77.\n");
  writeFileSync(join(repo, "clean.txt"), "nothing here\n");
  writeFileSync(join(repo, "untracked.txt"), "zephyr-quokka-77\n");
  git(repo, "add", "docs/plan.md", "clean.txt");

  const staged = run(["--denylist", deny, "--staged"], { cwd: repo });
  assert.equal(staged.status, 1, staged.stderr);
  assert.match(staged.stdout, /docs\/plan\.md:3/);
  assert.doesNotMatch(staged.stdout, /untracked/);

  const tree = run(["--denylist", deny], { cwd: repo });
  assert.equal(tree.status, 1, tree.stderr);
  assert.match(tree.stdout, /docs\/plan\.md:3/);
  assert.match(tree.stdout, /1 hit/);
  assert.doesNotMatch(tree.stdout + tree.stderr, /zephyr|quokka/i);

  writeFileSync(join(repo, "docs", "plan.md"), "# Plan\n\nThe org is acme-corp.\n");
  git(repo, "add", "docs/plan.md");
  const fixed = run(["--denylist", deny], { cwd: repo });
  assert.equal(fixed.status, 0, fixed.stdout + fixed.stderr);
});

test("--add --class org stores a salted hash in the committed list and never the plain value", (t) => {
  const dir = scratch(t);
  const deny = denylistWith(dir);
  const result = run(["--denylist", deny, "--add", SECRET, "--class", "org"]);
  assert.equal(result.status, 0, result.stderr);
  assert.doesNotMatch(result.stdout + result.stderr, /zephyr|quokka/i);
  const again = run(["--denylist", deny, "--add", "zephyr.quokka.77", "--class", "org"]);
  assert.match(again.stdout, /already/);
  const text = readFileSync(deny, "utf8");
  assert.doesNotMatch(text, /zephyr|quokka/i);
  const list = parseDenylist(text);
  assert.equal(list.entries.size, 1);
  assert.ok(list.entries.has(hashToken(SALT, "zephyr-quokka-77")));
});

test("the committed denylist holds only a salt, hashes and comments", () => {
  const text = readFileSync(COMMITTED, "utf8").replace(/\r\n/g, "\n");
  const lines = text.split("\n").filter((l) => l.trim() && !l.startsWith("#"));
  assert.match(lines[0], /^salt [0-9a-f]{32,}$/);
  const hashes = lines.slice(1);
  assert.ok(hashes.length > 0, "the denylist is seeded");
  for (const line of hashes) assert.match(line, /^[0-9a-f]{64} [1-9]$/);
  // Comments describe classes, never values: no comment line is long enough
  // to hide an identifier list, and the parser accepts the file.
  const list = parseDenylist(text);
  assert.equal(list.entries.size, hashes.length);
});

test("evidence tokens come from host labels, ID-like segments and titles", () => {
  const tokens = evidenceTokens(
    "https://zephyr-tenant.sharepoint.example/sites/Ops/Doc.aspx?id=AB12CD34EF56\tZephyr Ops Weekly Review\n" +
      "plainvalue-unique\n",
  );
  const byValue = new Map(tokens.map((t) => [t.value, t.kind]));
  assert.equal(byValue.get("zephyr-tenant"), "host");
  assert.equal(byValue.get("ab12cd34ef56"), "id");
  assert.equal(byValue.get("zephyr-ops-weekly-review"), "title");
  assert.equal(byValue.get("plainvalue-unique"), "value");
  assert.equal(byValue.has("sharepoint"), false, "a lone public label is not evidence");
});

test("the evidence mode reports the class and line, never the value", (t) => {
  const dir = scratch(t);
  const deny = denylistWith(dir);
  const evidence = join(dir, "history.tsv");
  writeFileSync(evidence, "https://zephyr-tenant.example.com/a\tZephyr Ops Weekly Review\n");
  const msg = join(dir, "notes.md");
  writeFileSync(msg, "ok\nfrom the Zephyr Ops Weekly Review board\n");
  // Evidence mode refuses to run under CI, so this test runs it as local.
  const result = run(["--denylist", deny, "--evidence", evidence, "--message", msg], { env: { ...process.env, CI: "" } });
  assert.equal(run(["--denylist", deny, "--evidence", evidence], { env: { ...process.env, CI: "true" } }).status, 2);
  assert.equal(result.status, 1, result.stderr);
  assert.match(result.stdout, /notes\.md:2 evidence:title/);
  assert.doesNotMatch(result.stdout + result.stderr, /zephyr/i);
});

test("the commit-range and PR-event modes scan commit messages and the PR title and body", (t) => {
  const dir = scratch(t);
  const deny = denylistWith(dir, SECRET);
  const repo = join(dir, "repo");
  git(dir, "init", "-q", repo);
  const commit = (msg) => git(repo, "-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty", "-m", msg);
  commit("base");
  commit("chore: for the Zephyr.Quokka.77 org");
  commit("fix: clean");
  const range = run(["--denylist", deny, "--commits", "HEAD~2..HEAD"], { cwd: repo });
  assert.equal(range.status, 1, range.stderr);
  assert.match(range.stdout, /commit [0-9a-f]{12}:1 denylist/);
  assert.match(range.stdout, /2 commit messages/);
  assert.equal(run(["--denylist", deny, "--commits", "HEAD^!"], { cwd: repo }).status, 0);

  const event = join(dir, "event.json");
  writeFileSync(event, JSON.stringify({ pull_request: { title: "feat: x", body: "Owner: me\n\nTested on zephyr%2Dquokka%2D77.\n" } }));
  const pr = run(["--denylist", deny, "--pr-event"], { env: { ...process.env, GITHUB_EVENT_PATH: event } });
  assert.equal(pr.status, 1, pr.stderr);
  assert.match(pr.stdout, /PR body:3 denylist/);
  assert.doesNotMatch(pr.stdout, /zephyr|quokka/i);
});

test("--add defaults to the local person list, which every scan also checks", (t) => {
  const dir = scratch(t);
  const deny = denylistWith(dir);
  const local = join(dir, "config", "brauser", "local.sha256");
  const env = { BRAUSER_PRIVACY_DENYLIST_LOCAL: local };
  const added = run(["--denylist", deny, "--add", "Jordan Example"], { env });
  assert.equal(added.status, 0, added.stderr);
  assert.match(added.stdout, /local person denylist/);
  assert.equal(parseDenylist(readFileSync(deny, "utf8")).entries.size, 0, "the committed list is untouched");
  const localText = readFileSync(local, "utf8");
  assert.doesNotMatch(localText, /jordan/i);
  assert.equal(parseDenylist(localText).salt === SALT, false, "the local list has its own salt");
  assert.equal(run(["--add", "x", "--class", "team"], { env }).status, 2);

  const msg = join(dir, "msg.txt");
  writeFileSync(msg, "Reviewed with jordan.example today.\n");
  const hit = run(["--denylist", deny, "--message", msg], { env });
  assert.equal(hit.status, 1, hit.stderr);
  assert.match(hit.stdout, /msg\.txt:1 local denylist/);
  assert.match(hit.stdout, /0 denylist entries, 1 local/);
  assert.doesNotMatch(hit.stdout, /jordan/i);
  // Without the local file (as in CI) the committed list alone decides.
  assert.equal(run(["--denylist", deny, "--message", msg]).status, 0);
});
