// Scratch repositories for the generated-file merge driver tests
// (merge-generated.test.mjs, pr-doctor.test.mjs). Each holds this
// repository's real generators and their inputs, so a merge regenerates with
// the code that ships; the families the tests add are synthetic.

import { spawnSync } from "node:child_process";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { quietGitEnv } from "./git-test-env.mjs";

export const REPO = join(dirname(fileURLToPath(import.meta.url)), "../..");

export const FIXTURE_PATHS = Object.freeze([
  ".gitattributes",
  "protocol",
  "docs/adapters",
  "extension/starter-families.generated.ts",
  "scripts/generate-starters.mjs",
  "scripts/merge-generated.mjs",
]);

export const STARTER_OUTPUTS = Object.freeze([
  "extension/starter-families.generated.ts",
  "protocol/capture-site/catalog.json",
  "protocol/capture-site/starter-shapes.json",
  "protocol/adapter-templates/starter-vectors.json",
  "docs/adapters/README.md",
]);

export const ENV = quietGitEnv({
  ...process.env,
  GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@example.com", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@example.com",
});
for (const key of Object.keys(ENV)) if (key.startsWith("BRAUSER_MERGE_") || key.startsWith("GITHEAD_")) delete ENV[key];

/** Runs `cmd`; `{ code, out }`, or throws unless `ok` allows the code. */
export function run(cmd, args, { cwd, env = {}, ok = [0] } = {}) {
  const result = spawnSync(cmd, args, { cwd, encoding: "utf8", env: { ...ENV, ...env }, maxBuffer: 64 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (!ok.includes(result.status)) throw new Error(`${cmd} ${args.join(" ")} (in ${cwd}) exited ${result.status}:\n${result.stdout}${result.stderr}`);
  return { code: result.status, out: result.stdout + result.stderr };
}
export const git = (cwd, args, opts = {}) => run("git", args, { cwd, ...opts });

/** A scratch root with `repo`, a base commit of the fixture paths on main. */
export function makeFixture() {
  const root = realpathSync(mkdtempSync(join(tmpdir(), "merge-generated-")));
  const repo = join(root, "repo");
  git(root, ["init", "-q", "-b", "main", repo]);
  const files = git(REPO, ["ls-files", "-z", "--", ...FIXTURE_PATHS]).out.split("\0").filter(Boolean);
  for (const file of files) {
    mkdirSync(dirname(join(repo, file)), { recursive: true });
    cpSync(join(REPO, file), join(repo, file));
  }
  // The worktree's own copies, so uncommitted driver or attribute changes are tested.
  for (const file of [".gitattributes", "scripts/merge-generated.mjs"]) cpSync(join(REPO, file), join(repo, file));
  commitAll(repo, "base");
  return { root, repo, cleanup: () => rmSync(root, { recursive: true, force: true }) };
}

export function commitAll(repo, message) {
  git(repo, ["add", "-A"]);
  git(repo, ["commit", "-q", "-m", message]);
  return git(repo, ["rev-parse", "HEAD"]).out.trim();
}

/** A synthetic one-type family `name` (origin https://<name>.example.com) with its doc. */
export function addFamily(repo, name) {
  const family = JSON.parse(readFileSync(join(repo, "protocol/starters/miro.json"), "utf8"));
  const type = family.types[0];
  const origin = `https://${name}.example.com`;
  Object.assign(family, { family: name, display_name: `Synthetic ${name}`, origin: { kind: "exact", origin }, sample_origin: origin });
  Object.assign(type, {
    type: `${name}.board`, key: `${name}.board`, label: `Synthetic ${name} boards`, docs: `docs/adapters/${name}.board.md`,
    status: { state: "development", method: null, checked_on: null, checked_by: null },
  });
  Object.assign(type.adapter, { type: `${name}.board`, id: `${name}.board:{host}/{board}`, generic_titles: [], title: `{title} - Synthetic ${name}` });
  type.samples = type.samples.map((sample) => ({ ...sample, url: sample.url.replace("https://miro.com", origin), title: `Board - Synthetic ${name}` }));
  writeFileSync(join(repo, `protocol/starters/${name}.json`), `${JSON.stringify(family, null, 2)}\n`);
  writeFileSync(join(repo, `docs/adapters/${name}.board.md`), `# Synthetic ${name} board\n\nStatus: development draft.\n`);
}

export const genStarters = (repo, ...flags) => run(process.execPath, [join(repo, "scripts/generate-starters.mjs"), "--root", repo, ...flags], { cwd: repo, ok: [0, 1] });
export const genProtocol = (repo, ...flags) => run("python3", [join(repo, "protocol/generate.py"), ...flags], { cwd: repo, ok: [0, 1] });

/** Adds a request message `name` (wire type `type`) to protocol/schema.json,
 * as a copy of GetConfigRequest placed before `beforeDefinition`, and lists it
 * in the Request union after `afterRef`, the way a protocol PR does. */
export function addRequest(repo, { name, type, beforeDefinition, afterRef }) {
  const file = join(repo, "protocol/schema.json");
  const lines = readFileSync(file, "utf8").split("\n");
  const start = lines.indexOf('    "GetConfigRequest": {');
  const end = lines.findIndex((text, index) => index > start && text === "    },");
  const copy = lines.slice(start, end + 1).map((text) => text
    .replace('"GetConfigRequest"', `"${name}"`)
    .replace('"const": "get_config"', `"const": "${type}"`));
  const ref = lines.findIndex((text) => text.includes(`"$ref": "#/$defs/${afterRef}"`));
  const close = lines[ref].trim().startsWith("{") ? ref : ref + 1;
  lines.splice(close + 1, 0, `        { "$ref": "#/$defs/${name}" },`);
  const before = lines.indexOf(`    "${beforeDefinition}": {`);
  if (start < 0 || end < 0 || ref < 0 || before < 0) throw new Error("schema.json no longer has the fixture's anchors");
  lines.splice(before, 0, ...copy);
  writeFileSync(file, lines.join("\n"));
}

export const unmerged = (repo) => git(repo, ["diff", "--name-only", "--diff-filter=U"]).out.split("\n").filter(Boolean);
export const read = (repo, file) => readFileSync(join(repo, file), "utf8");
