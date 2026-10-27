import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

// build-id.mjs is the build's own helper; build-info.js runs after
// build:extension. Neither touches chrome or the DOM.
const { buildId, fillPlaceholders, gitBuildId } = await import("../build-id.mjs");
const { hostBuildText, hostCommit } = await import("../dist/build-info.js");

const extensionDir = dirname(dirname(fileURLToPath(import.meta.url)));

function git(dir, ...args) {
  const result = spawnSync("git", ["-C", dir, ...args], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}

async function scratchRepo(t) {
  const dir = await mkdtemp(join(tmpdir(), "brauser-build-id-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  git(dir, "init", "-q");
  git(dir, "config", "user.email", "test@example.com");
  git(dir, "config", "user.name", "Test");
  git(dir, "config", "commit.gpgsign", "false");
  await writeFile(join(dir, "tracked.txt"), "one\n");
  git(dir, "add", "tracked.txt");
  git(dir, "commit", "-q", "-m", "first");
  return dir;
}

test("buildId_formats_the_commit_and_a_dirty_suffix", () => {
  assert.equal(buildId("abc1234def56\n", ""), "abc1234def56");
  assert.equal(buildId("abc1234def56", " M extension/panel.ts\n"), "abc1234def56-dirty");
  // Without git, or outside a checkout, the build still runs.
  assert.equal(buildId(null, null), "unknown");
  assert.equal(buildId("", ""), "unknown");
  // A commit whose status could not be read is not claimed clean.
  assert.equal(buildId("abc1234def56", null), "abc1234def56-dirty");
});

test("gitBuildId_reads_the_checkout_commit_and_tracked_changes", async (t) => {
  const dir = await scratchRepo(t);
  const commit = git(dir, "rev-parse", "--short=12", "HEAD");
  assert.equal(gitBuildId(dir), commit);
  // Stray untracked files are not part of the build.
  await writeFile(join(dir, "untracked.txt"), "scratch\n");
  assert.equal(gitBuildId(dir), commit);
  await writeFile(join(dir, "tracked.txt"), "two\n");
  assert.equal(gitBuildId(dir), `${commit}-dirty`);
});

test("gitBuildId_outside_a_checkout_is_unknown", async (t) => {
  const dir = await mkdtemp(join(tmpdir(), "brauser-build-id-none-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  // GIT_CEILING_DIRECTORIES keeps git from finding a checkout above tmpdir.
  assert.equal(gitBuildId(dir, { ...process.env, GIT_CEILING_DIRECTORIES: dirname(dir) }), "unknown");
});

test("fillPlaceholders_stamps_the_build_and_refuses_unknown_names", () => {
  const values = { APP_NAME: "Brauser", BUILD_ID: "abc1234def56" };
  assert.equal(fillPlaceholders("{{APP_NAME}} build {{BUILD_ID}}", values, "panel.html"),
    "Brauser build abc1234def56");
  assert.throws(() => fillPlaceholders("{{NOPE}}", values, "panel.html"), /extension\/panel.html/);
});

test("built_pages_carry_this_checkout's_build_id", async () => {
  // Edits since the build may flip the dirty mark, so compare the commit.
  const expected = gitBuildId(dirname(extensionDir)).replace(/-dirty$/, "");
  for (const name of ["panel.html", "options.html"]) {
    const page = await readFile(join(extensionDir, "dist", name), "utf8");
    assert.match(page, new RegExp(`<span id="build-id">${expected}(-dirty)?</span>`), `${name} lacks build ${expected}`);
    assert.ok(page.includes('<span id="host-build"></span>'), `${name} lacks the host build slot`);
  }
});

test("hostCommit_reads_semver_build_metadata", () => {
  assert.equal(hostCommit("0.1.0+abc1234def56"), "abc1234def56");
  assert.equal(hostCommit("0.1.0+abc1234def56-dirty"), "abc1234def56-dirty");
  // An older host reports only its package version.
  assert.equal(hostCommit("0.1.0"), null);
  assert.equal(hostCommit("0.1.0+"), null);
});

test("hostBuildText_names_the_host_build_and_flags_a_different_commit", () => {
  assert.equal(hostBuildText("abc1234def56", "0.1.0+abc1234def56"), " · host abc1234def56");
  // Uncommitted changes on either side are not a commit mismatch.
  assert.equal(hostBuildText("abc1234def56-dirty", "0.1.0+abc1234def56"), " · host abc1234def56");
  assert.equal(hostBuildText("abc1234def56", "0.1.0+abc1234def56-dirty"), " · host abc1234def56-dirty");
  // Abbreviations of the same commit can differ in length.
  assert.equal(hostBuildText("abc1234", "0.1.0+abc1234def56"), " · host abc1234def56");
  assert.equal(hostBuildText("abc1234def56", "0.1.0+0123456789ab"),
    " · host 0123456789ab (not the extension's commit; rebuild both)");
  // Nothing to compare against.
  assert.equal(hostBuildText("unknown", "0.1.0+0123456789ab"), " · host 0123456789ab");
  assert.equal(hostBuildText("abc1234def56", "0.1.0+unknown"), " · host unknown");
  assert.equal(hostBuildText("abc1234def56", "0.1.0"), "");
});
