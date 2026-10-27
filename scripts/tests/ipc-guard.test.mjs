import assert from "node:assert/strict";
import test from "node:test";
import { classify, decide, ipcDenied, probe } from "./ipc-guard.mjs";

const denied = { kind: "denied", why: "bind: EPERM: operation not permitted" };

test("an allowed probe runs the test", () => {
  assert.deepEqual(decide({ kind: "allowed" }, false), { action: "run" });
  assert.deepEqual(decide({ kind: "allowed" }, true), { action: "run" });
});

test("a denial skips unless required", () => {
  const skip = decide(denied, false);
  assert.equal(skip.action, "skip");
  assert.match(skip.why, /^skipping: local IPC denied by sandbox/);
  assert.equal(decide(denied, true).action, "fail");
});

test("any other probe error fails", () => {
  const failed = { kind: "failed", why: "bind: EADDRINUSE" };
  assert.equal(decide(failed, false).action, "fail");
  assert.equal(decide(failed, true).action, "fail");
});

test("only permission errors count as denied", () => {
  for (const code of ["EPERM", "EACCES"]) {
    assert.equal(classify("bind", Object.assign(new Error(code), { code })).kind, "denied");
  }
  for (const code of ["EADDRINUSE", "ENAMETOOLONG", "ENOENT", undefined]) {
    assert.equal(classify("bind", Object.assign(new Error(String(code)), { code })).kind, "failed");
  }
});

test("both probes run here unless denied", async (t) => {
  for (const kind of ["socket", "child-pipe"]) {
    if (await ipcDenied(t, kind)) return;
    assert.deepEqual(await probe(kind), { kind: "allowed" });
  }
});

test("an unknown probe kind is an error", () => {
  assert.throws(() => probe("tcp"), /unknown IPC probe/);
});
