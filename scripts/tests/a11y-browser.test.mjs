import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import {
  LINUX_CHROMIUM, MACOS_CHROME, SANDBOX_LAUNCH_TIMEOUT_MS, a11yBrowser, chromePath, decide, launchFailure, launchOptions, resetSandboxProbe,
} from "../a11y-browser.mjs";
import { ipcDenied } from "./ipc-guard.mjs";

const LAUNCHER = fileURLToPath(new URL("../a11y-browser.mjs", import.meta.url));
const SANDBOX = { CODEX_SANDBOX_NETWORK_DISABLED: "1" };
const REQUIRE = { BRAUSER_REQUIRE_A11Y_TESTS: "1" };

/** A stand-in for node:test's TestContext that records skips. */
function fakeTest() {
  const skips = [];
  return { name: "fake", skips, skip: (why) => skips.push(why) };
}

test("CHROME_PATH wins on every platform", () => {
  for (const platform of ["linux", "darwin", "win32"]) {
    assert.equal(chromePath({ CHROME_PATH: "/x/chrome" }, platform), "/x/chrome");
  }
});

test("platform defaults: the worker symlink on Linux, the Chrome app on macOS, none on Windows", () => {
  assert.equal(chromePath({}, "linux"), LINUX_CHROMIUM);
  assert.equal(chromePath({}, "darwin"), MACOS_CHROME);
  assert.equal(chromePath({}, "win32"), null);
});

test("launch options are headless with --no-sandbox", () => {
  assert.deepEqual(launchOptions("/x/chrome"), { executablePath: "/x/chrome", headless: true, args: ["--no-sandbox"] });
});

test("inside the Codex sandbox a found browser is still tried, marked sandboxed", () => {
  for (const env of [SANDBOX, { ...SANDBOX, ...REQUIRE }]) {
    assert.deepEqual(decide({ env, platform: "linux", exists: true }), { action: "run", executablePath: LINUX_CHROMIUM, sandboxed: true });
  }
});

test("a missing browser skips, and names where it looked", () => {
  const verdict = decide({ env: {}, platform: "linux", exists: false });
  assert.equal(verdict.action, "skip");
  assert.match(verdict.why, /no browser at \/opt\/browsers\/chromium/);
  assert.equal(decide({ env: {}, platform: "win32", exists: false }).action, "skip");
});

test("BRAUSER_REQUIRE_A11Y_TESTS=1 turns a missing browser into a failure, inside the sandbox or out", () => {
  for (const input of [
    { env: REQUIRE, platform: "linux", exists: false },
    { env: { ...REQUIRE, ...SANDBOX }, platform: "linux", exists: false },
    { env: REQUIRE, platform: "win32", exists: false },
  ]) {
    const verdict = decide(input);
    assert.equal(verdict.action, "fail");
    assert.match(verdict.why, /BRAUSER_REQUIRE_A11Y_TESTS=1 forbids skipping/);
  }
});

test("a launch failure is blocked-by-sandbox inside the sandbox and a real failure outside it", () => {
  const error = new Error("browserType.launch: Target page, context or browser has been closed\nBrowser logs: ...");
  const inside = launchFailure({ sandboxed: true, error });
  assert.equal(inside.action, "skip");
  assert.match(inside.why, /^blocked by sandbox: Chromium cannot start inside the Codex sandbox \(CODEX_SANDBOX_NETWORK_DISABLED=1\): browserType\.launch: Target page/);
  assert.doesNotMatch(inside.why, /Browser logs/);
  const outside = launchFailure({ sandboxed: false, error });
  assert.equal(outside.action, "fail");
  assert.match(outside.why, /^Chromium failed to launch: browserType\.launch/);
});

test("a11yBrowser: under the require flag in the sandbox, a failed launch skips once and is cached", async () => {
  resetSandboxProbe();
  const launches = [];
  const launch = async (options) => { launches.push(options); throw new Error("sandbox_host_linux.cc: shutdown: Operation not permitted"); };
  const deps = { launch, exists: async () => true };
  const env = { ...SANDBOX, ...REQUIRE };
  const first = fakeTest();
  const second = fakeTest();
  assert.equal(await a11yBrowser(first, env, "linux", deps), null);
  assert.equal(await a11yBrowser(second, env, "linux", deps), null);
  assert.equal(launches.length, 1, "the sandbox probe launches Chromium once per process");
  assert.equal(launches[0].timeout, SANDBOX_LAUNCH_TIMEOUT_MS);
  for (const t of [first, second]) {
    assert.equal(t.skips.length, 1);
    assert.match(t.skips[0], /^blocked by sandbox: .*Operation not permitted/);
  }
  resetSandboxProbe();
});

test("a11yBrowser: in the sandbox, a browser that starts is used", async () => {
  resetSandboxProbe();
  const browser = { fake: true };
  const t = fakeTest();
  assert.equal(await a11yBrowser(t, { ...SANDBOX, ...REQUIRE }, "linux", { launch: async () => browser, exists: async () => true }), browser);
  assert.deepEqual(t.skips, []);
});

test("a11yBrowser: outside the sandbox a failed launch throws, with or without the require flag", async () => {
  resetSandboxProbe();
  for (const env of [{}, REQUIRE]) {
    const t = fakeTest();
    await assert.rejects(
      a11yBrowser(t, env, "linux", { launch: async () => { throw new Error("boom"); }, exists: async () => true }),
      /Chromium failed to launch: boom/,
    );
    assert.deepEqual(t.skips, []);
  }
});

// End to end with the real playwright-core and a "browser" that exits at
// once, as Chromium does in the Codex sandbox: the exact command Cookie runs
// (BRAUSER_REQUIRE_A11Y_TESTS=1) passes with labelled skips inside the
// sandbox, and fails outside it.
test("a real launch failure: skipped as blocked inside the sandbox, red outside it", async (t) => {
  if (process.platform === "win32") return t.skip("no POSIX stand-in browser on Windows");
  if (await ipcDenied(t, "child-pipe")) return;
  const dir = await mkdtemp(path.join(os.tmpdir(), "brauser-a11y-"));
  t.after(() => rm(dir, { recursive: true, force: true }));
  const fakeChrome = path.join(dir, "chrome");
  await writeFile(fakeChrome, "#!/bin/sh\necho 'sandbox_host_linux.cc: shutdown: Operation not permitted' >&2\nexit 1\n", { mode: 0o755 });
  const suite = path.join(dir, "suite.test.mjs");
  await writeFile(suite, `import test from "node:test";
import { a11yBrowser } from ${JSON.stringify(LAUNCHER)};
for (const n of [1, 2]) {
  test(\`browser case \${n}\`, async (t) => {
    const browser = await a11yBrowser(t);
    if (!browser) return;
    await browser.close();
  });
}
`);
  const run = (extra) => {
    const env = { ...process.env, ...REQUIRE, CHROME_PATH: fakeChrome, ...extra };
    if (!extra.CODEX_SANDBOX_NETWORK_DISABLED) delete env.CODEX_SANDBOX_NETWORK_DISABLED;
    delete env.NODE_TEST_CONTEXT; // else the child reports in the parent runner's binary protocol
    return spawnSync(process.execPath, ["--test", "--test-isolation=none", "--test-reporter=tap", suite], { encoding: "utf8", env, timeout: 60_000, killSignal: "SIGKILL" });
  };
  const inside = run(SANDBOX);
  assert.equal(inside.status, 0, `inside the sandbox the suite must pass:\n${inside.stdout}\n${inside.stderr}`);
  assert.match(inside.stdout, /# skipped 2/);
  assert.match(inside.stdout, /# SKIP blocked by sandbox: Chromium cannot start inside the Codex sandbox/);
  const outside = run({});
  assert.notEqual(outside.status, 0, `outside the sandbox the suite must fail:\n${outside.stdout}`);
  assert.match(outside.stdout, /# fail 2/);
  assert.match(outside.stdout, /Chromium failed to launch/);
});

test("a found browser outside the sandbox runs", () => {
  assert.deepEqual(decide({ env: {}, platform: "linux", exists: true }), { action: "run", executablePath: LINUX_CHROMIUM, sandboxed: false });
  assert.deepEqual(decide({ env: { CHROME_PATH: "/x/chrome" }, platform: "darwin", exists: true }), { action: "run", executablePath: "/x/chrome", sandboxed: false });
});
