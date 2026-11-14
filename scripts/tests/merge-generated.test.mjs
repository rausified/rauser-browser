import assert from "node:assert/strict";
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import { ipcDenied } from "./ipc-guard.mjs";
import {
  AGENTS_BLOCK, AGENTS_PATH, CATALOG_PATH, LOCK_PATH, configure, driverCommand, generatorEnv, generatorFor, otherCommit, unionLock,
  withBaseGeneratedBlock, withBaseGeneratedLines,
} from "../merge-generated.mjs";
import { BEGIN, END } from "../generate-current-state.mjs";
import {
  REPO, STARTER_OUTPUTS, addFamily, addRequest, commitAll, genCurrentState, genProtocol, genStarters, git, makeFixture, read, unmerged,
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
  assert.deepEqual(Object.keys(JSON.parse(result.text)), ["a", "gone", "z"], "a dropped key keeps its place");
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

test("AGENTS.md's table block takes the base's rows and keeps hand-written edits", () => {
  assert.ok(AGENTS_BLOCK.begin.test(BEGIN) && AGENTS_BLOCK.end.test(END), "the driver's markers are the generator's");
  const doc = (intro, rows, outro) => `${intro}\n${BEGIN}\n${rows.join("\n")}\n${END}\n${outro}\n`;
  const base = doc("intro", ["| a |"], "outro");
  const ours = doc("intro, edited", ["| a |", "| b |"], "outro");
  assert.equal(withBaseGeneratedBlock(ours, base), doc("intro, edited", ["| a |"], "outro"));
  // A base from before the split has no block: the side's block empties.
  assert.equal(withBaseGeneratedBlock(ours, "intro\n| a |\noutro\n"), `intro, edited\n${BEGIN}\n${END}\noutro\n`);
  // A side without a block is left alone.
  assert.equal(withBaseGeneratedBlock("intro\n| a |\n", base), "intro\n| a |\n");
  // A CRLF side keeps CRLF, even for the base's rows taken from an LF base:
  // never mixed endings.
  const crlf = (text) => text.replace(/\n/g, "\r\n");
  assert.equal(withBaseGeneratedBlock(crlf(ours), base), crlf(doc("intro, edited", ["| a |"], "outro")));
  assert.equal(withBaseGeneratedBlock(ours, crlf(base)), doc("intro, edited", ["| a |"], "outro"));
  // CRLF checkouts keep their line endings.
  assert.equal(withBaseGeneratedBlock(ours.replace(/\n/g, "\r\n"), base.replace(/\n/g, "\r\n")), doc("intro, edited", ["| a |"], "outro").replace(/\n/g, "\r\n"));
});

test("only one GITHEAD_<sha> names the other commit", () => {
  const sha = "a".repeat(40);
  assert.equal(otherCommit({ [`GITHEAD_${sha}`]: "origin/main", PATH: "/bin" }), sha);
  assert.equal(otherCommit({ PATH: "/bin" }), null);
  assert.equal(otherCommit({ [`GITHEAD_${sha}`]: "x", [`GITHEAD_${"b".repeat(40)}`]: "y" }), null, "an octopus merge is not regenerated");
});

test("every file .gitattributes marks brauser-generated has a generator", () => {
  const marked = git(REPO, ["check-attr", "merge", "--", ...STARTER_OUTPUTS, LOCK_PATH, "protocol/src/generated.rs", "protocol/ts/generated.ts", AGENTS_PATH]).out
    .split("\n").filter((line) => line.endsWith(": merge: brauser-generated")).map((line) => line.split(": merge:")[0]);
  assert.equal(marked.length, STARTER_OUTPUTS.length + 4);
  assert.equal(generatorFor(AGENTS_PATH).name, "current-state");
  for (const path of marked) assert.ok(generatorFor(path), path);
  assert.equal(generatorFor("protocol/starters/miro.json"), null);
  assert.equal(generatorFor(CATALOG_PATH).name, "starters");
});

test("the driver command runs the worktree's copy, or a trusted directory's", () => {
  assert.match(driverCommand(), /\$\(git rev-parse --show-toplevel\)\/scripts\/merge-generated\.mjs.*%O %A %B %P %L/);
  assert.match(driverCommand(), /else git merge-file/);
  const plain = "git merge-file --marker-size=%L -L ours -L base -L theirs %A %O %B";
  assert.equal(driverCommand({ tools: "/t/it's" }),
    `f='/t/it'\\''s/merge-generated.mjs'; if [ -f "$f" ] && command -v node >/dev/null 2>&1; then node "$f" %O %A %B %P %L; else ${plain}; fi`);
  assert.match(driverCommand({ tools: "C:\\t\\" }), /^f='C:\\t\/merge-generated\.mjs';/);
});

test("generators run with no credentials in their environment", () => {
  const env = generatorEnv({ PATH: "/bin", HOME: "/h", GH_TOKEN: "x", GITHUB_TOKEN: "x", PR_DOCTOR_TOKEN: "x", BRAUSER_MERGE_TOOLS: "/t", GIT_CONFIG_COUNT: "1" });
  assert.deepEqual(env, { PATH: "/bin", HOME: "/h" });
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

test("one lock key both sides set to different ids is a conflict, wherever each side put it", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  // The reviewer's case: ours inserts the key after the first entry, theirs
  // appends it, so the text merges cleanly into a lock with the key twice.
  const entries = () => Object.entries(JSON.parse(read(repo, LOCK_PATH)));
  const write = (list) => writeFileSync(join(repo, LOCK_PATH), `${JSON.stringify(Object.fromEntries(list), null, 2)}\n`);
  git(repo, ["checkout", "-q", "-b", "a", "main"]);
  const list = entries();
  write([list[0], ["clash.thing", "clash.thing:{host}/{one}"], ...list.slice(1)]);
  commitAll(repo, "insert");
  git(repo, ["checkout", "-q", "-b", "b", "main"]);
  write([...entries(), ["clash.thing", "clash.thing:{host}/{two}"]]);
  commitAll(repo, "append");
  for (const command of [["merge", "-q", "origin/b"], ["rebase", "-q", "origin/b"]]) {
    const dir = cloneAt(root, repo, `clash-${command[0]}`, "a");
    const result = git(dir, command, { ok: [1] });
    assert.match(result.out, /both sides set clash\.thing to different ids/);
    assert.ok(unmerged(dir).includes(LOCK_PATH), `${command[0]}: ${unmerged(dir).join(",")}`);
    assert.match(read(dir, LOCK_PATH), /^<{7} ours$/m, `${command[0]} leaves markers`);
  }
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

// brwr-p2a7 (#463 review I2): a symlink a PR commits inside a generator's
// extract would let the generator read, or write through it, a file outside
// the scratch tree. Any symlink there is a conflict; the generator never runs.
test("a symlink in a generator's extract is a conflict, and nothing outside is read", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  twoFamilyBranches(repo);
  const outside = join(root, "outside.md");
  writeFileSync(outside, "# Outside\n\n## Research\n\nTODO-OUTSIDE-MARKER\n\nStatus: outside\n");
  git(repo, ["checkout", "-q", "b"]);
  rmSync(join(repo, "docs/adapters/widgetb.board.md"));
  symlinkSync(outside, join(repo, "docs/adapters/widgetb.board.md"));
  commitAll(repo, "doc becomes a symlink");
  git(repo, ["checkout", "-q", "main"]);
  const dir = cloneAt(root, repo, "symlink", "a");
  const merge = git(dir, ["merge", "-q", "origin/b"], { ok: [1] });
  assert.match(merge.out, /symlink at docs\/adapters\/widgetb\.board\.md/);
  assert.doesNotMatch(merge.out, /TODO-OUTSIDE-MARKER|outside\.md/);
  assert.ok(unmerged(dir).includes("extension/starter-families.generated.ts"), unmerged(dir).join(","));
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
  const cache = join(dir, ".git", "brauser-merge-cache");
  assert.deepEqual(existsSync(cache) ? readdirSync(cache) : [], [], "a failed regeneration is not cached");
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
  assert.match(merge.out, /the merge changes scripts\/generate-starters\.mjs, which is not main's copy \(BRAUSER_MERGE_TOOLS\)/);
  assert.ok(unmerged(changed).includes("extension/starter-families.generated.ts"));
});

test("a local merge never runs a generator only the other branch changed", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  twoFamilyBranches(repo);
  const marker = join(root, "ran");
  const changeGenerator = (branch) => {
    git(repo, ["checkout", "-q", branch]);
    // Branch code that would leave a trace if it ran.
    const source = read(repo, "scripts/generate-starters.mjs");
    writeFileSync(join(repo, "scripts/generate-starters.mjs"),
      source.replace('import path from "node:path";', `import path from "node:path";\nimport { writeFileSync as touch } from "node:fs";\ntouch(${JSON.stringify(marker)}, "x");`));
    commitAll(repo, `touch the generator on ${branch}`);
  };
  changeGenerator("b");
  const theirs = cloneAt(root, repo, "theirs-changed", "a");
  const merge = git(theirs, ["merge", "-q", "origin/b"], { ok: [1] });
  assert.match(merge.out, /the merge changes scripts\/generate-starters\.mjs, which is not the merge base's, HEAD's or origin\/main's copy/);
  assert.ok(unmerged(theirs).includes("extension/starter-families.generated.ts"));
  assert.equal(existsSync(marker), false, "the other branch's generator never ran");

  // Your own branch's generator (HEAD's copy) is yours to run.
  const ours = cloneAt(root, repo, "ours-changed", "b");
  git(ours, ["merge", "-q", "--no-edit", "origin/a"]);
  assert.equal(existsSync(marker), true);
  assert.equal(genStarters(ours, "--check").code, 0);
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
  const outputs = {};
  for (const [ours, theirs] of [["a", "b"], ["b", "a"]]) {
    const dir = cloneAt(root, repo, `protocol-${ours}`, ours);
    git(dir, ["merge", "-q", "--no-edit", `origin/${theirs}`]);
    assert.equal(genProtocol(dir, "--check").code, 0);
    for (const name of ["ZzARequest", "ZzBRequest"]) assert.match(read(dir, "protocol/ts/generated.ts"), new RegExp(`export type Request = [^;]*\\b${name}\\b`));
    outputs[ours] = ["protocol/ts/generated.ts", "protocol/src/generated.rs"].map((file) => read(dir, file));
  }
  assert.deepEqual(outputs.a, outputs.b, "the merge order does not change the generated types");
});

/** Adds row file `name` to AGENTS.md's Current state and regenerates. */
function addRow(repo, name, component) {
  writeFileSync(join(repo, "docs/agents/current-state", name), `# ${component}\n\nBuilt: a synthetic component;\nnothing real.\n\nPointer: DESIGN.md §0\n`);
  assert.equal(genCurrentState(repo).code, 0);
}

/** `text` with the hand-written line starting `prefix` given `suffix`. */
function editLine(repo, prefix, suffix) {
  const file = join(repo, AGENTS_PATH);
  const lines = read(repo, AGENTS_PATH).split("\n");
  const at = lines.findIndex((line) => line.startsWith(prefix));
  assert.ok(at >= 0, prefix);
  lines[at] += suffix;
  writeFileSync(file, lines.join("\n"));
}

test("two PRs that each add a Current state row merge cleanly in either order and match a fresh generation", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  // Both rows land after the last one, so the table lines collide; each PR
  // also edits a different hand-written sentence, which must survive.
  for (const [branch, name, component, prefix] of [
    ["a", "900-widget-a.md", "Widget A", "Brauser is its own product"],
    ["b", "901-widget-b.md", "Widget B", "- Treat all extension messages"],
  ]) {
    git(repo, ["checkout", "-q", "-b", branch, "main"]);
    addRow(repo, name, component);
    editLine(repo, prefix, ` (${branch})`);
    commitAll(repo, `add ${component}`);
  }
  git(repo, ["checkout", "-q", "main"]);

  // Without the driver the table conflicts: what the row files and the driver fix.
  const plain = cloneAt(root, repo, "plain", "a", { driver: false });
  assert.equal(git(plain, ["merge", "-q", "origin/b"], { ok: [1] }).code, 1);
  assert.deepEqual(unmerged(plain), [AGENTS_PATH]);

  const merged = {};
  for (const [ours, theirs] of [["a", "b"], ["b", "a"]]) {
    const dir = cloneAt(root, repo, `merge-${ours}`, ours);
    git(dir, ["merge", "-q", "--no-edit", `origin/${theirs}`]);
    assert.equal(genCurrentState(dir, "--check").code, 0, genCurrentState(dir, "--check").out);
    const text = read(dir, AGENTS_PATH);
    assert.match(text, /^\| Widget A \| Built: a synthetic component; nothing real\. \| DESIGN\.md §0 \|\n\| Widget B \|/m);
    assert.match(text, /^Brauser is its own product and its own project\. \(a\)$/m);
    assert.match(text, /^- Treat all extension messages .* \(b\)$/m);
    merged[ours] = text;
  }
  assert.equal(merged.a, merged.b, "the merge order does not change AGENTS.md");
});

test("a hand-written AGENTS.md conflict stays an ordinary conflict, never a side", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  for (const branch of ["a", "b"]) {
    git(repo, ["checkout", "-q", "-b", branch, "main"]);
    addRow(repo, `90${branch === "a" ? 0 : 1}-widget-${branch}.md`, `Widget ${branch}`);
    editLine(repo, "Brauser is its own product", ` (${branch})`);
    commitAll(repo, `edit ${branch}`);
  }
  const dir = cloneAt(root, repo, "merge", "a");
  git(dir, ["merge", "-q", "origin/b"], { ok: [1] });
  assert.deepEqual(unmerged(dir), [AGENTS_PATH]);
  assert.match(read(dir, AGENTS_PATH), /^<{7} ours$/m);
});

test("an AGENTS.md from before the generated table still merges as text", scratch, async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const { root, repo, cleanup } = makeFixture();
  t.after(cleanup);
  // No markers and no generator anywhere: nothing to regenerate.
  writeFileSync(join(repo, AGENTS_PATH), read(repo, AGENTS_PATH).split("\n").filter((line) => !/GENERATED current-state/.test(line)).join("\n"));
  git(repo, ["rm", "-q", "-r", "scripts/generate-current-state.mjs", "docs/agents/current-state"]);
  commitAll(repo, "before the split");
  for (const [branch, prefix] of [["a", "Brauser is its own product"], ["b", "- Treat all extension messages"]]) {
    git(repo, ["checkout", "-q", "-b", branch, "main"]);
    editLine(repo, prefix, ` (${branch})`);
    commitAll(repo, `edit ${branch}`);
  }
  const dir = cloneAt(root, repo, "merge", "a");
  git(dir, ["merge", "-q", "--no-edit", "origin/b"]);
  assert.match(read(dir, AGENTS_PATH), /\(a\)$/m);
  assert.match(read(dir, AGENTS_PATH), /\(b\)$/m);
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

// --- real conflicts from this repository's history ------------------------------
// Each is a past merge commit whose parents conflict in generated files; the
// driver's result must be byte-identical to what a person committed. They
// need the full history, so a shallow clone (CI's checkout) skips them.

const HISTORY = [
  {
    commit: "4ad210a9f6831b3d58add4acd4d92d7177ae51b9",
    conflicts: [],
    generated: ["extension/starter-families.generated.ts", "protocol/adapter-templates/starter-vectors.json"],
  },
  {
    commit: "37ef5ec97a3b5e7f353f35e73d6398dc23be6d4f",
    conflicts: ["TESTING.md", "docs/history.md", "package.json"],
    generated: ["protocol/src/generated.rs", "protocol/ts/generated.ts"],
  },
];

for (const { commit, conflicts, generated } of HISTORY) {
  test(`re-merging ${commit.slice(0, 8)} regenerates exactly what was committed by hand`, scratch, async (t) => {
    if (await ipcDenied(t, "child-pipe")) return;
    const have = git(REPO, ["cat-file", "-e", `${commit}^2^{commit}`], { ok: [0, 1, 128] }).code === 0;
    if (!have) {
      t.skip(`needs ${commit.slice(0, 12)} and its parents in this clone (a shallow checkout lacks them); run in a full clone`);
      return;
    }
    const root = mkdtempSync(join(tmpdir(), "merge-history-"));
    t.after(() => rmSync(root, { recursive: true, force: true }));
    const dir = join(root, "clone");
    git(root, ["clone", "-q", "--shared", "--no-checkout", REPO, dir]);
    git(dir, ["-c", "advice.detachedHead=false", "checkout", "-q", `${commit}^1`]);
    // Today's rules and driver, as the PR doctor applies them to an older branch.
    cpSync(join(REPO, ".gitattributes"), join(git(dir, ["rev-parse", "--absolute-git-dir"]).out.trim(), "info", "attributes"));
    const tools = join(root, "tools");
    mkdirSync(tools);
    cpSync(join(REPO, "scripts/merge-generated.mjs"), join(tools, "merge-generated.mjs"));
    configure({ cwd: dir, tools });
    // The second parent is the main these branches merged in: origin/main then.
    git(dir, ["update-ref", "refs/remotes/origin/main", `${commit}^2`]);
    const plain = git(dir, ["-c", "merge.brauser-generated.driver=false", "merge-tree", "--write-tree", "--name-only", "--no-messages", "HEAD", `${commit}^2`], { ok: [1] });
    for (const file of generated) assert.ok(plain.out.split("\n").includes(file), `${file} conflicts without the driver`);
    git(dir, ["merge", "-q", "--no-edit", `${commit}^2`], { ok: [conflicts.length ? 1 : 0] });
    assert.deepEqual(unmerged(dir), conflicts);
    for (const file of generated) {
      assert.equal(read(dir, file), git(dir, ["show", `${commit}:${file}`]).out, `${file} matches ${commit.slice(0, 8)}`);
    }
  });
}
