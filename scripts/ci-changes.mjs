// CI change classifier (AGENTS.md "Review and merge cadence"): decides whether a
// pull request or merge group touches only docs and work-queue files, so
// .github/workflows/ci.yml can skip the macOS and Windows builds and run the
// cheap docs/queue checks instead. Nothing builds, tests or ships from these
// paths, so skipping the builds for them is safe:
//
// - Markdown at the repository root (AGENTS.md, DESIGN.md, TESTING.md,
//   README.md, BOSUN_FEATURE_BACKLOG.md, CookieFeature.md,
//   PENDING_AMOS_DOC_UPDATES.md, adapter-templates.md, ...). README.md and
//   the status lines are checked by scripts/check_current_state.py, which the
//   docs job still runs.
// - Anything under docs/ (plans, specs, history, starter-adapter notes).
// - Anything under .beads/ (the work queue).
//
// Everything else is code, including Markdown below the root (fixtures under
// host/tests/, protocol/README.md and protocol/adapter-templates/README.md
// are read by tests or define the protocol). An empty change list is never
// docs-only, so a diff that found nothing (or failed) runs the full builds.
//
// Usage: node scripts/ci-changes.mjs <base> <head>
// Prints the changed paths and `docs_only=true|false`, and appends
// `docs_only=...` to $GITHUB_OUTPUT when it is set. Exits 1 if git fails.

import { execFileSync } from "node:child_process";
import { appendFileSync } from "node:fs";

const DOCS_DIRS = ["docs/", ".beads/"];

/** True when nothing in CI builds or tests from `path`. */
export function isDocsOrQueuePath(path) {
  if (typeof path !== "string" || path === "") return false;
  if (DOCS_DIRS.some((dir) => path.startsWith(dir) && path.length > dir.length)) return true;
  return !path.includes("/") && path.endsWith(".md") && path.length > ".md".length;
}

/** True when every changed path is a docs or queue file and there is at least one. */
export function classify(paths) {
  return paths.length > 0 && paths.every(isDocsOrQueuePath);
}

/** Split `git diff --name-only -z` output; NUL is the only separator. */
export function parseNameList(output) {
  return output.split("\0").filter((path) => path !== "");
}

function main(argv) {
  const [base, head] = argv;
  if (!base || !head) throw new Error("usage: node scripts/ci-changes.mjs <base> <head>");
  // --no-renames: a file moved from host/ into docs/ shows both paths, so it
  // still counts as a code change.
  const output = execFileSync("git", ["diff", "--name-only", "-z", "--no-renames", base, head], {
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  const paths = parseNameList(output);
  for (const path of paths) console.log(`${isDocsOrQueuePath(path) ? "docs " : "code "} ${JSON.stringify(path)}`);
  const docsOnly = classify(paths);
  console.log(`docs_only=${docsOnly}`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `docs_only=${docsOnly}\n`);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
