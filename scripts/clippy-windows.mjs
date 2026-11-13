#!/usr/bin/env node

// Lint the Rust workspace for Windows before pushing (delay-fix rank 11,
// brwr-0524). Windows CI is the only place `#[cfg(windows)]` code and
// Windows-only lints are checked, so a Windows-only clippy failure used to
// surface only after a push, often in the merge queue (#392's "Check Rust
// lints"). This runs the same clippy as build-os.yml's "Check Rust lints", for
// the x86_64-pc-windows-gnu target, from macOS or Linux.
//
// It needs two things, and skips (exit 0, saying what to install) without
// either:
// - the target's standard library: `rustup target add x86_64-pc-windows-gnu`
//   (rust-toolchain.toml pins the toolchain, so run it from this checkout);
// - a MinGW C compiler, because rusqlite's `bundled` feature compiles SQLite
//   in its build script even for `cargo clippy`: `brew install mingw-w64` on
//   macOS, `apt install gcc-mingw-w64-x86-64` on Debian or Ubuntu (or set
//   CC_x86_64_pc_windows_gnu).
// On Windows itself it skips: the README gate's own clippy already lints
// Windows there.
//
// By default it lints only when the change touches Rust (a .rs file, a
// Cargo.toml, Cargo.lock or the toolchain) against --base (origin/main),
// counting staged, unstaged and untracked files too.
//
// Usage: npm run lint:windows [-- options]
//   --base <ref>   compare against <ref>...HEAD (default origin/main)
//   --force        lint even when no Rust changed
//   --require      fail (exit 2) instead of skipping when a prerequisite is missing
//   --dry-run      print the decision and command, run nothing
// Exit codes: 0 clean or skipped, 1 clippy failed or git failed, 2 a missing
// prerequisite with --require, or bad usage.

import { execFileSync, spawnSync } from "node:child_process";
import { existsSync, realpathSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

import { parseNameList } from "./ci-changes.mjs";

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));

export const TARGET = "x86_64-pc-windows-gnu";
export const MINGW_CC = "x86_64-w64-mingw32-gcc";
export const CC_ENV = "CC_x86_64_pc_windows_gnu";
export const CLIPPY = ["cargo", "clippy", "--locked", "--workspace", "--all-targets", "--target", TARGET, "--", "-D", "warnings"];

/** True when `file` can change what clippy reports. */
export function isRustInput(file) {
  return /\.rs$/.test(file) || /(^|\/)Cargo\.(toml|lock)$/.test(file) || file === "rust-toolchain.toml" || file.startsWith(".cargo/");
}

/**
 * What to do: `{ run: true, cmd }`, or `{ run: false, exitCode, reason }`.
 * `platform` is process.platform; `changed` the changed paths (null: unknown,
 * so lint); `hasTarget` whether the target's std is installed; `cc` the C
 * compiler found for it, or null.
 */
export function plan({ platform, changed, force = false, require = false, hasTarget, cc }) {
  if (platform === "win32") return { run: false, exitCode: 0, reason: "on Windows: the README gate's own cargo clippy already lints for Windows" };
  if (!force && changed && !changed.some(isRustInput)) return { run: false, exitCode: 0, reason: "no Rust change (a .rs file, Cargo.toml, Cargo.lock or the toolchain); --force lints anyway" };
  const missing = [];
  if (!hasTarget) missing.push(`the ${TARGET} standard library (rustup target add ${TARGET})`);
  if (!cc) missing.push(`a MinGW C compiler for rusqlite's bundled SQLite (${platform === "darwin" ? "brew install mingw-w64" : "apt install gcc-mingw-w64-x86-64"}, or set ${CC_ENV})`);
  if (missing.length) {
    const reason = `cannot lint for Windows without ${missing.join(" and ")}; Windows CI still runs clippy`;
    return { run: false, exitCode: require ? 2 : 0, reason };
  }
  return { run: true, cmd: CLIPPY };
}

function git(args) {
  return execFileSync("git", args, { cwd: REPO, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, env: { ...process.env, GIT_OPTIONAL_LOCKS: "0" } });
}

function changedFiles(base) {
  return [...new Set([
    ...parseNameList(git(["diff", "--name-only", "-z", "--no-renames", `${base}...HEAD`])),
    ...parseNameList(git(["diff", "--name-only", "-z", "--no-renames", "HEAD"])),
    ...parseNameList(git(["ls-files", "--others", "--exclude-standard", "-z"])),
  ])];
}

function targetInstalled() {
  try {
    const sysroot = execFileSync("rustc", ["--print", "sysroot"], { cwd: REPO, encoding: "utf8" }).trim();
    return existsSync(path.join(sysroot, "lib", "rustlib", TARGET, "lib"));
  } catch {
    return false;
  }
}

function findCc() {
  if (process.env[CC_ENV]) return process.env[CC_ENV];
  for (const dir of (process.env.PATH ?? "").split(path.delimiter)) {
    if (dir && existsSync(path.join(dir, MINGW_CC))) return path.join(dir, MINGW_CC);
  }
  return null;
}

function main(argv) {
  const { values } = parseArgs({ args: argv, options: {
    base: { type: "string", default: "origin/main" }, force: { type: "boolean", default: false },
    require: { type: "boolean", default: false }, "dry-run": { type: "boolean", default: false },
  } });
  let changed = null;
  if (!values.force) {
    try { changed = changedFiles(values.base); } catch (error) { console.error(`lint:windows: git failed: ${error.message}`); return 1; }
  }
  const decision = plan({ platform: process.platform, changed, force: values.force, require: values.require, hasTarget: targetInstalled(), cc: findCc() });
  if (!decision.run) {
    console.log(`lint:windows: ${decision.exitCode ? "FAIL" : "skip"}: ${decision.reason}`);
    return decision.exitCode;
  }
  console.log(`lint:windows: ${decision.cmd.join(" ")}`);
  if (values["dry-run"]) return 0;
  const run = spawnSync(decision.cmd[0], decision.cmd.slice(1), { cwd: REPO, stdio: "inherit" });
  return run.status ?? 1;
}

function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}

if (isEntryPoint()) {
  try {
    process.exit(main(process.argv.slice(2)));
  } catch (error) {
    console.error(`lint:windows: ${error.message}`);
    process.exit(2);
  }
}
