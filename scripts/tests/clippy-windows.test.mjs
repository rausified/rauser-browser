import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { CLIPPY, isRustInput, plan, TARGET } from "../clippy-windows.mjs";
import { ipcDenied } from "./ipc-guard.mjs";

const SCRIPT = path.join(path.dirname(path.dirname(fileURLToPath(import.meta.url))), "clippy-windows.mjs");
const ready = { platform: "darwin", hasTarget: true, cc: "/opt/homebrew/bin/x86_64-w64-mingw32-gcc" };

test("a host Rust change lints the workspace for the Windows target with -D warnings", () => {
  const decision = plan({ ...ready, changed: ["host/src/fsutil.rs"] });
  assert.equal(decision.run, true);
  assert.deepEqual(decision.cmd, CLIPPY);
  assert.deepEqual(CLIPPY.slice(0, 6), ["cargo", "clippy", "--locked", "--workspace", "--all-targets", "--target"]);
  assert.equal(CLIPPY[6], TARGET);
  assert.deepEqual(CLIPPY.slice(-3), ["--", "-D", "warnings"]);
});

test("Rust inputs: .rs files, crate manifests, the lockfile and the toolchain", () => {
  for (const file of ["host/src/lib.rs", "adapter-templates/tests/shapes.rs", "host/Cargo.toml", "Cargo.toml", "Cargo.lock", "rust-toolchain.toml", ".cargo/config.toml"]) {
    assert.equal(isRustInput(file), true, file);
  }
  for (const file of ["extension/panel.ts", "README.md", "host/tests/fixtures/config.toml", "scripts/ci-changes.mjs"]) {
    assert.equal(isRustInput(file), false, file);
  }
});

test("no Rust change skips unless --force; an unknown change set lints", () => {
  const skip = plan({ ...ready, changed: ["extension/panel.ts", "README.md"] });
  assert.equal(skip.run, false);
  assert.equal(skip.exitCode, 0);
  assert.match(skip.reason, /no Rust change/);
  assert.equal(plan({ ...ready, changed: ["extension/panel.ts"], force: true }).run, true);
  assert.equal(plan({ ...ready, changed: null }).run, true);
});

test("a missing target or MinGW compiler skips with the install command", () => {
  const noTarget = plan({ ...ready, changed: ["host/src/lib.rs"], hasTarget: false });
  assert.equal(noTarget.run, false);
  assert.equal(noTarget.exitCode, 0);
  assert.match(noTarget.reason, /rustup target add x86_64-pc-windows-gnu/);
  const noCc = plan({ ...ready, changed: ["host/src/lib.rs"], cc: null });
  assert.match(noCc.reason, /brew install mingw-w64/);
  assert.match(noCc.reason, /CC_x86_64_pc_windows_gnu/);
  assert.match(plan({ ...ready, platform: "linux", changed: ["host/src/lib.rs"], cc: null }).reason, /apt install gcc-mingw-w64-x86-64/);
  const both = plan({ ...ready, changed: ["host/src/lib.rs"], hasTarget: false, cc: null });
  assert.match(both.reason, /standard library .* and a MinGW C compiler/);
});

test("--require turns a missing prerequisite into exit 2", () => {
  const decision = plan({ ...ready, changed: ["host/src/lib.rs"], cc: null, require: true });
  assert.equal(decision.run, false);
  assert.equal(decision.exitCode, 2);
});

test("on Windows it skips: native clippy already covers it", () => {
  const decision = plan({ ...ready, platform: "win32", changed: ["host/src/lib.rs"] });
  assert.equal(decision.run, false);
  assert.equal(decision.exitCode, 0);
  assert.match(decision.reason, /on Windows/);
});

test("--dry-run with --force prints a decision and runs nothing", async (t) => {
  if (await ipcDenied(t, "child-pipe")) return;
  const run = spawnSync(process.execPath, [SCRIPT, "--force", "--dry-run"], { encoding: "utf8" });
  assert.equal(run.status, 0, run.stderr);
  assert.match(run.stdout, /^lint:windows: (skip: |cargo clippy )/m);
});
