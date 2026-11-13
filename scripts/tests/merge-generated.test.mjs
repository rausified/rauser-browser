import assert from "node:assert/strict";
import { cpSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { ipcDenied } from "./ipc-guard.mjs";
import {
  CATALOG_PATH, LOCK_PATH, configure, driverCommand, generatorFor, otherCommit, unionLock, withBaseGeneratedLines,
} from "../merge-generated.mjs";
import {
  REPO, STARTER_OUTPUTS, addFamily, addRequest, commitAll, genProtocol, genStarters, git, makeFixture, read, unmerged,
} from "./generated-fixture.mjs";

// --- pure parts ---------------------------------------------------------------

const lock = (entries) => `${JSON.stringify(Object.fromEntries(entries), null, 2)}\n`;

test("the lock union keeps ours' order, then theirs' new keys in theirs' order", () => {
  const base = lock([["a", "a:{x}"]]);
  const ours = lock([["a", "a:{x}"], ["b", "b:{x}"], ["c", "c:{x}"]]);
  const theirs = lock([["a", "a:{x}"], ["d", "d:{x}"], ["c", "c:{x}"], ["e", "e:{x}"]]);
  const result = unionLock(base, ours, theirs);
  assert.equal(result.ok, true);
  assert.deepEqual(Object.keys(JSON.parse(result.text)), ["a", "b", "c", "d", "e"]);
});

test("the lock union never drops a key, even one a side deleted", () => {
  const base = lock([["a", "a:{x}"], ["gone", "gone:{x}"]]);
  const result = unionLock(base, lock([["a", "a:{x}"]]), lock([["a", "a:{x}"], ["z", "z:{x}"]]));
  assert.deepEqual(Object.keys(JSON.parse(result.text)), ["a", "z", "gone"]);
});

test("a key both sides lock to different ids is a conflict, named", () => {
  const result = unionLock(lock([]), lock([["n", "n:{one}"]]), lock([["n", "n:{two}"]]));
  assert.deepEqual(result, { ok: false, keys: ["n"] });
});

test("catalog lines take the base's generated text and keep hand-written edits", () => {
  const base = '{\n  "x": 1,\n  "starter_types": ["a"],\n  "container_first": [],\n  "y": 2\n}\n';
  const ours = '{\n  "x": 3,\n  "starter_types": ["a", "b"],\n  "container_first": [{ "container": "b" }],\n  "y": 2\n}\n';
  assert.equal(withBaseGeneratedLines(ours, base), '{\n  "x": 3,\n  "starter_types": ["a"],\n  "container_first": [],\n  "y": 2\n}\n');
  const noPairs = '{\n  "starter_types": ["a"],\n}\n';
  assert.equal(withBaseGeneratedLines(ours, noPairs), '{\n  "x": 3,\n  "starter_types": ["a"],\n  "y": 2\n}\n');
});

test("only one GITHEAD_<sha> names the other commit", () => {
  const sha = "a".repeat(40);
  assert.equal(otherCommit({ [`GITHEAD_${sha}`]: "origin/main", PATH: "/bin" }), sha);
  assert.equal(otherCommit({ PATH: "/bin" }), null);
  assert.equal(otherCommit({ [`GITHEAD_${sha}`]: "x", [`GITHEAD_${"b".repeat(40)}`]: "y" }), null, "an octopus merge is not regenerated");
});

test("every file .gitattributes marks brauser-generated has a generator", () => {
  const marked = git(REPO, ["check-attr", "merge", "--", ...STARTER_OUTPUTS, LOCK_PATH, "protocol/src/generated.rs", "protocol/ts/generated.ts"]).out
    .split("\n").filter((line) => line.endsWith(": merge: brauser-generated")).map((line) => line.split(": merge:")[0]);
  assert.equal(marked.length, STARTER_OUTPUTS.length + 3);
  for (const path of marked) assert.ok(generatorFor(path), path);
  assert.equal(generatorFor("protocol/starters/miro.json"), null);
  assert.equal(generatorFor(CATALOG_PATH).name, "starters");
});

test("the driver command runs the worktree's copy, or a trusted directory's", () => {
  assert.match(driverCommand(), /\$\(git rev-parse --show-toplevel\)\/scripts\/merge-generated\.mjs.*%O %A %B %P %L/);
  assert.match(driverCommand(), /else git merge-file/);
  assert.equal(driverCommand({ tools: "/t/it's" }), "node '/t/it'\\''s/merge-generated.mjs' %O %A %B %P %L");
});

// --- real merges in scratch repositories ---------------------------------------

const scratch = { skip: process.platform === "win32" ? "the driver command runs under sh and the fixture uses tar" : false };

/** Two branches from the fixture's base, each adding one synthetic family and regenerating. */
function twoFamilyBranches(repo) {
  for (const [branch, name] of [["a", "widgeta"], ["b", "widgetb"]]) {
    git(repo, ["checkout", "-q", "-b", branch, "main"]);
    addFamily(repo, name);
    assert.equal(genStarters(repo).code, 0);
    commitAll(repo, `add ${name}`);
  }
  git(repo, ["checkout", "-q", "main"]);
}

function cloneAt(root, repo, name, branch, { driver = true, tools = null } = {}) {
  const dir = join(root, name);
  git(root, ["clone", "-q", repo, dir]);
  git(dir, ["checkout", "-q", "-B", branch, `origin/${branch}`]);
  if (driver) configure({ cwd: dir, tools });
  return dir;
}

test("two family PRs that both regenerate merge cleanly in either order and match a fresh generation", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  twoFamilyBranches(repo);

  // Without the driver the outputs conflict: what this driver exists to fix.
  const plain = cloneAt(root, repo, "plain", "a", { driver: false });
  assert.equal(git(plain, ["merge", "-q", "origin/b"], { ok: [1] }).code, 1);
  assert.ok(unmerged(plain).includes("extension/starter-families.generated.ts"), unmerged(plain).join(","));

  const outputs = {};
  for (const [ours, theirs] of [["a", "b"], ["b", "a"]]) {
    const dir = cloneAt(root, repo, `merge-${ours}`, ours);
    git(dir, ["merge", "-q", "--no-edit", `origin/${theirs}`]);
    assert.equal(genStarters(dir, "--check").code, 0, genStarters(dir, "--check").out);
    // A fresh generation from the merged sources changes nothing.
    assert.equal(genStarters(dir).code, 0);
    assert.equal(git(dir, ["status", "--porcelain"]).out, "");
    outputs[ours] = Object.fromEntries(STARTER_OUTPUTS.map((file) => [file, read(dir, file)]));
    const keys = Object.keys(JSON.parse(read(dir, LOCK_PATH)));
    assert.deepEqual(keys.slice(-2), ours === "a" ? ["widgeta.board", "widgetb.board"] : ["widgetb.board", "widgeta.board"]);
    const base = Object.keys(JSON.parse(git(dir, ["show", `main:${LOCK_PATH}`]).out));
    assert.deepEqual(keys.slice(0, base.length), base, "every base entry, in order");
  }
  assert.deepEqual(outputs.a, outputs.b, "the merge order does not change the generated outputs");
});

test("the lock merges as a union in a rebase too, where nothing can regenerate", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  const append = (key) => {
    const entries = JSON.parse(read(repo, LOCK_PATH));
    entries[key] = `${key}:{host}/{id}`;
    writeFileSync(join(repo, LOCK_PATH), `${JSON.stringify(entries, null, 2)}\n`);
  };
  for (const [branch, key] of [["a", "one.thing"], ["b", "two.thing"]]) {
    git(repo, ["checkout", "-q", "-b", branch, "main"]);
    append(key);
    commitAll(repo, key);
  }
  const dir = cloneAt(root, repo, "rebase", "a");
  git(dir, ["rebase", "-q", "origin/b"]);
  assert.deepEqual(Object.keys(JSON.parse(read(dir, LOCK_PATH))).slice(-2), ["two.thing", "one.thing"]);
});

test("a conflict in a family file leaves ordinary conflicts, never a side", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  for (const branch of ["a", "b"]) {
    git(repo, ["checkout", "-q", "-b", branch, "main"]);
    const file = join(repo, "protocol/starters/miro.json");
    writeFileSync(file, read(repo, "protocol/starters/miro.json").replace('"display_name": "Miro"', `"display_name": "Miro ${branch}"`));
    assert.equal(genStarters(repo).code, 0);
    commitAll(repo, `rename ${branch}`);
  }
  const dir = cloneAt(root, repo, "conflict", "a");
  const merge = git(dir, ["merge", "-q", "origin/b"], { ok: [1] });
  assert.match(merge.out, /conflict in protocol\/starters\/miro\.json, an input of the starters generator/);
  const left = unmerged(dir);
  assert.ok(left.includes("protocol/starters/miro.json"));
  for (const file of left.filter((path) => path !== "protocol/starters/miro.json")) {
    assert.match(read(dir, file), /^<{7} ours$/m, `${file} keeps conflict markers`);
  }
  assert.ok(left.includes("extension/starter-families.generated.ts"), left.join(","));
});

test("merged sources the generator rejects stop the merge, even where the text merges", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  // Each branch is valid alone; together two families claim one origin. The
  // PR moving Miro edits the middle of the outputs and the new family their
  // end, so the outputs text-merge cleanly into a state no generator wrote.
  git(repo, ["checkout", "-q", "-b", "a", "main"]);
  const miro = join(repo, "protocol/starters/miro.json");
  writeFileSync(miro, read(repo, "protocol/starters/miro.json").replaceAll("https://miro.com", "https://shared.example.com"));
  assert.equal(genStarters(repo).code, 0, genStarters(repo).out);
  commitAll(repo, "move miro");
  git(repo, ["checkout", "-q", "-b", "b", "main"]);
  addFamily(repo, "widgetb");
  const family = join(repo, "protocol/starters/widgetb.json");
  writeFileSync(family, read(repo, "protocol/starters/widgetb.json").replaceAll("https://widgetb.example.com", "https://shared.example.com"));
  assert.equal(genStarters(repo).code, 0, genStarters(repo).out);
  commitAll(repo, "add widgetb");
  const plain = cloneAt(root, repo, "plain", "a", { driver: false });
  git(plain, ["merge", "-q", "--no-commit", "origin/b"], { ok: [0, 1] });
  assert.ok(!unmerged(plain).includes("extension/starter-families.generated.ts"), "the generated module text-merges cleanly");
  const dir = cloneAt(root, repo, "rejected", "a");
  const merge = git(dir, ["merge", "-q", "origin/b"], { ok: [1] });
  assert.match(merge.out, /generate-starters\.mjs failed on the merged sources/);
  assert.ok(unmerged(dir).includes("extension/starter-families.generated.ts"), unmerged(dir).join(","));
});

test("with trusted tools, a merge that changes the generator is refused", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  twoFamilyBranches(repo);
  const tools = mkdtempSync(join(tmpdir(), "merge-tools-"));
  t.after(() => rmSync(tools, { recursive: true, force: true }));
  for (const file of ["scripts/merge-generated.mjs", "scripts/generate-starters.mjs"]) cpSync(join(repo, file), join(tools, file.split("/").pop()));

  // Main's generator: the merge regenerates.
  const same = cloneAt(root, repo, "trusted", "a", { tools });
  git(same, ["merge", "-q", "--no-edit", "origin/b"], { env: { BRAUSER_MERGE_TOOLS: tools } });
  assert.equal(genStarters(same, "--check").code, 0);

  // The PR changes the generator: only main's copy may run, so it refuses.
  git(repo, ["checkout", "-q", "b"]);
  writeFileSync(join(repo, "scripts/generate-starters.mjs"), `${read(repo, "scripts/generate-starters.mjs")}// changed by the PR\n`);
  commitAll(repo, "touch the generator");
  const changed = cloneAt(root, repo, "untrusted", "a", { tools });
  const merge = git(changed, ["merge", "-q", "origin/b"], { ok: [1], env: { BRAUSER_MERGE_TOOLS: tools } });
  assert.match(merge.out, /the merge changes scripts\/generate-starters\.mjs; only main's copy runs here/);
  assert.ok(unmerged(changed).includes("extension/starter-families.generated.ts"));
});

test("two schema PRs regenerate the protocol's generated types on merge", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  const requests = [
    ["a", { name: "ZzARequest", type: "zz_a", beforeDefinition: "EvidenceSaved", afterRef: "SaveEvidenceRequest" }],
    ["b", { name: "ZzBRequest", type: "zz_b", beforeDefinition: "HelloResult", afterRef: "RecordVisitRequest" }],
  ];
  for (const [branch, request] of requests) {
    git(repo, ["checkout", "-q", "-b", branch, "main"]);
    addRequest(repo, request);
    assert.equal(genProtocol(repo).code, 0);
    commitAll(repo, `schema ${branch}`);
  }
  const plain = cloneAt(root, repo, "plain", "a", { driver: false });
  assert.equal(git(plain, ["merge", "-q", "origin/b"], { ok: [0, 1] }).code, 1, "without the driver the generated types conflict");
  const dir = cloneAt(root, repo, "protocol", "a");
  git(dir, ["merge", "-q", "--no-edit", "origin/b"]);
  assert.equal(genProtocol(dir, "--check").code, 0);
  for (const name of ["ZzARequest", "ZzBRequest"]) assert.match(read(dir, "protocol/ts/generated.ts"), new RegExp(`export type Request = [^;]*\\b${name}\\b`));
});

test("a checkout that predates the driver script falls back to a plain text merge", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  git(repo, ["rm", "-q", "scripts/merge-generated.mjs"]);
  commitAll(repo, "no driver script");
  twoFamilyBranches(repo);
  const dir = cloneAt(root, repo, "old", "a");
  git(dir, ["merge", "-q", "origin/b"], { ok: [1] });
  assert.match(read(dir, "extension/starter-families.generated.ts"), /^<{7} ours$/m);
});
