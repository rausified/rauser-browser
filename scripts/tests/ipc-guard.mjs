// Test-only guard for script tests that need local IPC: a Unix socket, or a
// child process whose output comes back over a pipe (Node's child pipes are
// Unix socketpairs on Linux). Some development sandboxes, such as Codex's
// workspace-write sandbox on Linux, deny these with EPERM or EACCES. A test
// that needs them starts with
//
//   if (await ipcDenied(t, "socket")) return;      // binds or connects a Unix socket
//   if (await ipcDenied(t, "child-pipe")) return;  // reads a child's output
//
// Each kind is probed once per process (a Unix socket bound and connected
// in a fresh /tmp directory; a child's stdout read back) and cached. A
// permission denial skips the test with a one-line reason on stderr, unless
// BRAUSER_REQUIRE_IPC_TESTS=1 (CI always sets it), which fails it instead.
// Any other probe error fails the test. BRAUSER_TEST_SIMULATE_IPC_DENIED=1
// makes the probe report a denial without trying. The Rust tests use the
// same rules (host/src/ipc_test_guard.rs).
import { spawnSync } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import net from "node:net";
import path from "node:path";

export const REQUIRE_IPC_TESTS_ENV = "BRAUSER_REQUIRE_IPC_TESTS";
export const SIMULATE_IPC_DENIED_ENV = "BRAUSER_TEST_SIMULATE_IPC_DENIED";

const DENIED_CODES = new Set(["EPERM", "EACCES"]);

/** Classify an error from a probe step: only a permission denial may skip. */
export function classify(step, error) {
  const why = `${step}: ${error?.code ?? "no code"}: ${error?.message ?? String(error)}`;
  return DENIED_CODES.has(error?.code) ? { kind: "denied", why } : { kind: "failed", why };
}

/** Decide from a probe result and whether BRAUSER_REQUIRE_IPC_TESTS=1. */
export function decide(probe, require) {
  switch (probe.kind) {
    case "allowed":
      return { action: "run" };
    case "denied":
      return require
        ? {
          action: "fail",
          why: `local IPC is denied (${probe.why}) but ${REQUIRE_IPC_TESTS_ENV}=1 forbids skipping socket tests; run them where local IPC is allowed`,
        }
        : { action: "skip", why: `skipping: local IPC denied by sandbox (${probe.why}); set ${REQUIRE_IPC_TESTS_ENV}=1 to fail instead` };
    default:
      return { action: "fail", why: `the local IPC probe failed: ${probe.why}` };
  }
}

async function socketProbe() {
  // Windows has no Unix-socket paths here; its pipes are named pipes.
  if (process.platform === "win32") return { kind: "allowed" };
  let directory;
  try {
    directory = await mkdtemp("/tmp/bipc-");
  } catch (error) {
    return { kind: "failed", why: `creating a /tmp directory: ${error.message}` };
  }
  const socket = path.join(directory, "p.sock");
  const server = net.createServer((connection) => connection.end());
  try {
    try {
      await new Promise((resolve, reject) => {
        server.once("error", reject);
        server.listen(socket, resolve);
      });
    } catch (error) {
      return classify("bind", error);
    }
    try {
      await new Promise((resolve, reject) => {
        const client = net.connect(socket);
        client.once("error", reject);
        client.once("connect", () => {
          client.destroy();
          resolve();
        });
      });
    } catch (error) {
      return classify("connect", error);
    }
    return { kind: "allowed" };
  } finally {
    await new Promise((resolve) => (server.listening ? server.close(resolve) : resolve()));
    await rm(directory, { recursive: true, force: true });
  }
}

function childPipeProbe() {
  const result = spawnSync(process.execPath, ["-e", "process.stdout.write('ok')"], { encoding: "utf8" });
  // A denied pipe can come back as status 0 with an error and no output, so
  // the error is checked before the status.
  if (result.error) return classify("child pipe", result.error);
  if (result.status !== 0 || result.stdout !== "ok") {
    return { kind: "failed", why: `child pipe: status ${result.status}, stdout ${JSON.stringify(result.stdout)}` };
  }
  return { kind: "allowed" };
}

const PROBES = { socket: socketProbe, "child-pipe": childPipeProbe };
const cached = new Map();

/** The cached probe result for one kind of IPC: "socket" or "child-pipe". */
export function probe(kind) {
  const run = PROBES[kind];
  if (!run) throw new Error(`unknown IPC probe ${JSON.stringify(kind)}`);
  if (!cached.has(kind)) {
    cached.set(
      kind,
      process.env[SIMULATE_IPC_DENIED_ENV] === "1"
        ? Promise.resolve({ kind: "denied", why: `simulated by ${SIMULATE_IPC_DENIED_ENV}=1` })
        : Promise.resolve().then(run),
    );
  }
  return cached.get(kind);
}

/**
 * True when the calling test should return because the IPC it needs
 * ("socket" or "child-pipe") is denied; the test is then marked skipped.
 * Throws when the denial must not be skipped or the probe failed.
 */
export async function ipcDenied(t, kind) {
  const decision = decide(await probe(kind), process.env[REQUIRE_IPC_TESTS_ENV] === "1");
  if (decision.action === "run") return false;
  if (decision.action === "fail") throw new Error(decision.why);
  process.stderr.write(`${t.name}: ${decision.why}\n`);
  t.skip(decision.why);
  return true;
}
