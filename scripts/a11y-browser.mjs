// Shared launcher for accessibility tests (axe-core through playwright-core)
// against a real headless Chromium. A test starts with
//
//   const browser = await a11yBrowser(t);
//   if (!browser) return;                          // skipped, reason on stderr
//   const context = await browser.newContext();    // AxeBuilder needs a context page,
//   const page = await context.newPage();          // not browser.newPage()
//   ...
//   await browser.close();
//
// Chromium cannot start inside Codex's workspace-write sandbox on Linux,
// whatever flags it gets: the sandbox's seccomp filter returns EPERM for
// setsockopt and shutdown on Unix socketpairs, and Chromium aborts at startup
// (content/browser/sandbox_host_linux.cc). That sandbox sets
// CODEX_SANDBOX_NETWORK_DISABLED=1. There the launcher still tries Chromium
// once per process (bounded); if it cannot start, every browser test skips
// with a "blocked by sandbox" reason, even under BRAUSER_REQUIRE_A11Y_TESTS=1.
// Cookie's cycles run the same `BRAUSER_REQUIRE_A11Y_TESTS=1 npm run
// test:a11y` command inside that sandbox as a health check, and must not read
// the sandbox as a red suite. If Chromium does start there, the tests run.
//
// Outside the sandbox (CI, the worker's unsandboxed commit gate, a dev
// machine) a launch failure fails the test, and a missing browser skips
// unless BRAUSER_REQUIRE_A11Y_TESTS=1, which fails it. The require flag
// never lets a missing browser pass, inside the sandbox or out.
//
// The browser comes from CHROME_PATH, else /opt/browsers/chromium on Linux (a
// stable symlink to Playwright's Chrome for Testing build, installed on the
// worker VMs), else the Google Chrome app on macOS. Nothing downloads a
// browser: install with PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 and `npm ci`.
import { access, constants } from "node:fs/promises";

export const CHROME_PATH_ENV = "CHROME_PATH";
export const REQUIRE_A11Y_TESTS_ENV = "BRAUSER_REQUIRE_A11Y_TESTS";
export const SANDBOX_ENV = "CODEX_SANDBOX_NETWORK_DISABLED";
export const LINUX_CHROMIUM = "/opt/browsers/chromium";
export const MACOS_CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

/** The browser executable to launch, or null when the platform has no default. */
export function chromePath(env = process.env, platform = process.platform) {
  const configured = env[CHROME_PATH_ENV];
  if (configured) return configured;
  if (platform === "linux") return LINUX_CHROMIUM;
  if (platform === "darwin") return MACOS_CHROME;
  return null;
}

/** Launch options: headless, and --no-sandbox, which is harmless where Chrome's own sandbox works. */
export function launchOptions(executablePath) {
  return { executablePath, headless: true, args: ["--no-sandbox"] };
}

export const SANDBOX_LAUNCH_TIMEOUT_MS = 30_000;

/** True inside Codex's workspace-write sandbox. */
export function inCodexSandbox(env = process.env) {
  return env[SANDBOX_ENV] === "1";
}

/**
 * Decide whether to try a launch. `exists` says whether the executable was found.
 * Returns { action: "run", executablePath, sandboxed } or { action: "skip" | "fail", why }.
 */
export function decide({ env = process.env, platform = process.platform, exists }) {
  const require = env[REQUIRE_A11Y_TESTS_ENV] === "1";
  const missing = (why) => (require ? { action: "fail", why: `${why} (${REQUIRE_A11Y_TESTS_ENV}=1 forbids skipping)` } : { action: "skip", why: `skipping: ${why}` });
  const executablePath = chromePath(env, platform);
  if (!executablePath) return missing(`no default browser on ${platform}; set ${CHROME_PATH_ENV}`);
  if (!exists) return missing(`no browser at ${executablePath}; set ${CHROME_PATH_ENV}`);
  return { action: "run", executablePath, sandboxed: inCodexSandbox(env) };
}

/**
 * What a failed launch means. Inside the Codex sandbox it is the sandbox, so
 * the test skips (whatever BRAUSER_REQUIRE_A11Y_TESTS says); anywhere else it
 * is a real failure. Returns { action: "skip" | "fail", why }.
 */
export function launchFailure({ sandboxed, error }) {
  const detail = String(error?.message ?? error).split("\n").find((line) => line.trim()) ?? "unknown error";
  if (sandboxed) {
    return {
      action: "skip",
      why: `blocked by sandbox: Chromium cannot start inside the Codex sandbox (${SANDBOX_ENV}=1): ${detail.trim()}; the unsandboxed commit gate and CI run these tests for real`,
    };
  }
  return { action: "fail", why: `Chromium failed to launch: ${detail.trim()}` };
}

async function executable(path) {
  try {
    await access(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

async function playwrightLaunch(options) {
  const { chromium } = await import("playwright-core");
  return chromium.launch(options);
}

// A sandboxed launch failure, once seen, is the answer for the whole process.
let sandboxBlocked = null;

/** Forget a cached sandbox launch failure (tests only). */
export function resetSandboxProbe() {
  sandboxBlocked = null;
}

function skip(t, why) {
  process.stderr.write(`${t.name ?? "a11y"}: ${why}\n`);
  t.skip(why);
  return null;
}

/**
 * Launch Chromium for test `t`, or skip/fail it per decide() and
 * launchFailure(); returns the browser or null. `launch` and `exists` are
 * injectable for tests.
 */
export async function a11yBrowser(t, env = process.env, platform = process.platform, { launch = playwrightLaunch, exists = executable } = {}) {
  const candidate = chromePath(env, platform);
  const verdict = decide({ env, platform, exists: candidate ? await exists(candidate) : false });
  if (verdict.action === "fail") throw new Error(verdict.why);
  if (verdict.action === "skip") return skip(t, verdict.why);
  if (verdict.sandboxed && sandboxBlocked) return skip(t, sandboxBlocked);
  const options = launchOptions(verdict.executablePath);
  if (verdict.sandboxed) options.timeout = SANDBOX_LAUNCH_TIMEOUT_MS;
  try {
    return await launch(options);
  } catch (error) {
    const failure = launchFailure({ sandboxed: verdict.sandboxed, error });
    if (failure.action === "fail") throw new Error(failure.why, { cause: error });
    sandboxBlocked = failure.why;
    return skip(t, failure.why);
  }
}
