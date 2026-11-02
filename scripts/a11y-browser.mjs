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
// CODEX_SANDBOX_NETWORK_DISABLED=1, so the launcher skips there with a
// one-line reason. Cookie's cycles run inside it; the unsandboxed commit gate
// and CI run the tests for real. A missing browser outside the sandbox also
// skips, unless BRAUSER_REQUIRE_A11Y_TESTS=1, which turns both cases into
// failures.
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

/**
 * Decide whether to run. `exists` says whether the executable was found.
 * Returns { action: "run", executablePath } or { action: "skip" | "fail", why }.
 */
export function decide({ env = process.env, platform = process.platform, exists }) {
  const require = env[REQUIRE_A11Y_TESTS_ENV] === "1";
  const blocked = (why) => (require ? { action: "fail", why: `${why} (${REQUIRE_A11Y_TESTS_ENV}=1 forbids skipping)` } : { action: "skip", why: `skipping: ${why}` });
  if (env[SANDBOX_ENV] === "1") return blocked("Chromium cannot start inside the Codex sandbox (CODEX_SANDBOX_NETWORK_DISABLED=1)");
  const executablePath = chromePath(env, platform);
  if (!executablePath) return blocked(`no default browser on ${platform}; set ${CHROME_PATH_ENV}`);
  if (!exists) return blocked(`no browser at ${executablePath}; set ${CHROME_PATH_ENV}`);
  return { action: "run", executablePath };
}

async function executable(path) {
  try {
    await access(path, constants.X_OK);
    return true;
  } catch {
    return false;
  }
}

/** Launch Chromium for test `t`, or skip/fail it per decide(); returns the browser or null. */
export async function a11yBrowser(t, env = process.env, platform = process.platform) {
  const candidate = chromePath(env, platform);
  const verdict = decide({ env, platform, exists: candidate ? await executable(candidate) : false });
  if (verdict.action === "fail") throw new Error(verdict.why);
  if (verdict.action === "skip") {
    process.stderr.write(`${verdict.why}\n`);
    t.skip(verdict.why);
    return null;
  }
  const { chromium } = await import("playwright-core");
  return chromium.launch(launchOptions(verdict.executablePath));
}
