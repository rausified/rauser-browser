import assert from "node:assert/strict";
import test from "node:test";
import { LINUX_CHROMIUM, MACOS_CHROME, chromePath, decide, launchOptions } from "../a11y-browser.mjs";

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

test("inside the Codex sandbox the test skips, even when a browser exists", () => {
  const verdict = decide({ env: { CODEX_SANDBOX_NETWORK_DISABLED: "1" }, platform: "linux", exists: true });
  assert.equal(verdict.action, "skip");
  assert.match(verdict.why, /^skipping: Chromium cannot start inside the Codex sandbox/);
});

test("a missing browser skips, and names where it looked", () => {
  const verdict = decide({ env: {}, platform: "linux", exists: false });
  assert.equal(verdict.action, "skip");
  assert.match(verdict.why, /no browser at \/opt\/browsers\/chromium/);
  assert.equal(decide({ env: {}, platform: "win32", exists: false }).action, "skip");
});

test("BRAUSER_REQUIRE_A11Y_TESTS=1 turns every skip into a failure", () => {
  const env = { BRAUSER_REQUIRE_A11Y_TESTS: "1" };
  for (const input of [
    { env: { ...env, CODEX_SANDBOX_NETWORK_DISABLED: "1" }, platform: "linux", exists: true },
    { env, platform: "linux", exists: false },
    { env, platform: "win32", exists: false },
  ]) {
    const verdict = decide(input);
    assert.equal(verdict.action, "fail");
    assert.match(verdict.why, /BRAUSER_REQUIRE_A11Y_TESTS=1 forbids skipping/);
  }
});

test("a found browser outside the sandbox runs", () => {
  assert.deepEqual(decide({ env: {}, platform: "linux", exists: true }), { action: "run", executablePath: LINUX_CHROMIUM });
  assert.deepEqual(decide({ env: { CHROME_PATH: "/x/chrome" }, platform: "darwin", exists: true }), { action: "run", executablePath: "/x/chrome" });
});
