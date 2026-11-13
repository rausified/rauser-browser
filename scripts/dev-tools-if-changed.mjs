#!/usr/bin/env node

// The README gate's Python dev-tools step, run only when the change can
// affect it (delay-fix rank 12, brwr-f8mm). `npm run test:dev-tools` takes
// about 9 minutes on a loaded Mac (c0e86b2d F6: 252 tests in 555 s) and used
// to run for every scripts/ change, JavaScript ones included. This asks
// scripts/ci-changes.mjs's devToolsReasons, the same rule CI's platform builds
// use, and runs the suite only when it names a reason. pr-hygiene.yml's
// "Dev tools" job still runs the whole suite on every pull request.
//
// Usage: npm run test:dev-tools:changed [-- --base <ref>] [-- --force]
// The changed paths are `git diff <base>...HEAD` (default origin/main) plus
// staged, unstaged and untracked files. Exit codes: the suite's own, 0 when
// skipped, 1 when git fails.

import { execFileSync, spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

import { devToolsReasons, parseNameList } from "./ci-changes.mjs";

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const { values } = parseArgs({ options: { base: { type: "string", default: "origin/main" }, force: { type: "boolean", default: false } } });

const git = (args) => execFileSync("git", args, { cwd: REPO, encoding: "utf8", maxBuffer: 64 * 1024 * 1024, env: { ...process.env, GIT_OPTIONAL_LOCKS: "0" } });
let reasons;
try {
  reasons = values.force ? [["--force", "asked for"]] : devToolsReasons([...new Set([
    ...parseNameList(git(["diff", "--name-only", "-z", "--no-renames", `${values.base}...HEAD`])),
    ...parseNameList(git(["diff", "--name-only", "-z", "--no-renames", "HEAD"])),
    ...parseNameList(git(["ls-files", "--others", "--exclude-standard", "-z"])),
  ])]);
} catch (error) {
  console.error(`test:dev-tools:changed: git failed: ${error.message}`);
  process.exit(1);
}
if (reasons.length === 0) {
  console.log(`test:dev-tools:changed: skip: nothing since ${values.base} touches a Python dev tool or a file its tests read (--force runs them)`);
  process.exit(0);
}
for (const [file, why] of reasons.slice(0, 5)) console.log(`test:dev-tools:changed: ${file}: ${why}`);
const run = spawnSync("python3", ["-m", "unittest", "discover", "-s", "scripts/tests", "-p", "test_*.py"], { cwd: REPO, stdio: "inherit" });
process.exit(run.status ?? 1);
