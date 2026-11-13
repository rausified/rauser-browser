// CI change classifier (AGENTS.md "Review and merge cadence"): decides whether a
// pull request or merge group touches only docs and beads configuration, so
// .github/workflows/ci.yml can skip the macOS and Windows builds and run the
// cheap docs checks instead. Nothing builds, tests or ships from these
// paths, so skipping the builds for them is safe:
//
// - Markdown at the repository root (AGENTS.md, DESIGN.md, TESTING.md,
//   README.md, CookieFeature.md, PENDING_AMOS_DOC_UPDATES.md,
//   adapter-templates.md, ...). README.md and the status lines are checked by
//   scripts/check_current_state.py, which the docs job still runs.
// - Anything under docs/ (plans, specs, history, starter-adapter notes).
// - Anything under .beads/ (bd's configuration; the queue itself lives in
//   bd's Dolt database, synced through refs/dolt/data, and
//   BOSUN_FEATURE_BACKLOG.md is rendered, never committed). A change that commits either queue file
//   to main is still docs-only here, and check_current_state.py, which every
//   CI path runs, fails it (AGENTS.md "Work queue").
//
// Everything else is code, including SUMMARY.md at the root (the native host
// compiles it in as the page-summary prompt, so its tests must run) and
// Markdown below the root (fixtures under
// host/tests/, protocol/README.md and protocol/adapter-templates/README.md
// are read by tests or define the protocol). docs/adapters/ is carved back
// out of docs/ for the same reason (Open decision 7, starter families PR C):
// scripts/generate-starters.mjs's check 3 and the registry test
// (extension/tests/starter-registry.test.mjs) both read a starter doc's
// `Status:` line, and only the code builds run them. An empty change list is
// never docs-only, so a diff that found nothing (or failed) runs the full
// builds.
//
// It also decides, through scripts/ci-macos.mjs, whether a code change needs
// the macOS build, and (devToolsReasons) whether it needs the Python
// dev-tools tests in the platform builds.
//
// Usage: node scripts/ci-changes.mjs <base> <head>
// Prints the changed paths, `docs_only=true|false`, `macos=true|false` and
// `dev_tools=true|false`, and appends them to $GITHUB_OUTPUT when it is set.
// Exits 1 if git fails.

import { execFileSync } from "node:child_process";
import { appendFileSync, readFileSync, realpathSync } from "node:fs";
import { pathToFileURL } from "node:url";

import { macosReasons, parseChangedLines } from "./ci-macos.mjs";

const DOCS_DIRS = ["docs/", ".beads/"];
/** Root Markdown that a build reads, so it is code. */
const BUILT_ROOT_MARKDOWN = ["SUMMARY.md"];
/** Open decision 7 (starter families PR C): a starter doc's `Status:` line
 * is read by scripts/generate-starters.mjs's own check 3 and by the
 * registry test (extension/tests/starter-registry.test.mjs, part of
 * test:extension), both of which only run in the code builds. Carving this
 * directory out of docs/ keeps that edit from skipping them. */
const DOCS_EXCEPTIONS = ["docs/adapters/"];

/** True when nothing in CI builds or tests from `path`. */
export function isDocsOrQueuePath(path) {
  if (typeof path !== "string" || path === "") return false;
  if (DOCS_EXCEPTIONS.some((dir) => path.startsWith(dir) && path.length > dir.length)) return false;
  if (DOCS_DIRS.some((dir) => path.startsWith(dir) && path.length > dir.length)) return true;
  if (BUILT_ROOT_MARKDOWN.includes(path)) return false;
  return !path.includes("/") && path.endsWith(".md") && path.length > ".md".length;
}

/** Files outside scripts/ that the Python dev-tools tests read, and the
 * definitions that run them (a change to the classifier or the workflows
 * must prove the step still runs). */
const DEV_TOOLS_INPUTS = [
  "worker-contract.json", "worker-contract.schema.json", ".gitattributes", "BRAUSER_IMPROVEMENT_FEED.md",
  "docs/improvement-feed/", "docs/backlog/", ".beads/",
  // test_worker_contract.py reads the merge command's tool and its constants;
  // test_deploy_live.py names the dev-host registration script deploy-live runs.
  "scripts/dev/queue-pr.mjs", "scripts/pr-constants.mjs", "scripts/register-dev-host.mjs",
  ".github/workflows/ci.yml", ".github/workflows/build-os.yml", ".github/workflows/pr-hygiene.yml",
  "scripts/ci-changes.mjs", "package.json",
];

/**
 * Why a change needs the Python dev-tools tests (`python -m unittest discover
 * -s scripts/tests`, npm run test:dev-tools): `[path, reason]` pairs, empty
 * when none does. A Python dev tool is any .py file under scripts/, anything
 * under scripts/dev/ except its .mjs files (the extensionless tools such as
 * `queue`, `deploy-live` and `worktree-hygiene` are Python), and the test
 * fixtures under scripts/tests/fixtures/. Those tests fake every cargo, npm
 * and node call, so JavaScript, Rust and extension changes never reach them.
 * An empty change list (a diff that found nothing or failed) runs them.
 */
export function devToolsReasons(paths) {
  if (paths.length === 0) return [["(no changed paths)", "an empty or failed diff runs everything"]];
  const reasons = [];
  for (const path of paths) {
    const python = path.startsWith("scripts/") && (path.endsWith(".py")
      || (path.startsWith("scripts/dev/") && !path.endsWith(".mjs"))
      || path.startsWith("scripts/tests/fixtures/"));
    if (python) reasons.push([path, "a Python dev tool or its tests"]);
    else if (DEV_TOOLS_INPUTS.some((input) => input.endsWith("/") ? path.startsWith(input) : path === input)) reasons.push([path, "read by the dev-tools tests"]);
  }
  return reasons;
}

/** True when every changed path is a docs or queue file and there is at least one. */
export function classify(paths) {
  return paths.length > 0 && paths.every(isDocsOrQueuePath);
}

/** Split `git diff --name-only -z` output; NUL is the only separator. */
export function parseNameList(output) {
  return output.split("\0").filter((path) => path !== "");
}

/** The commit under test as scripts/ci-macos.mjs reads it: the checkout is `head`. */
function gitRepo(base) {
  const git = (...args) => execFileSync("git", args, { encoding: "utf8", maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "ignore"] });
  return {
    read: (file) => { try { return readFileSync(file, "utf8"); } catch { return null; } },
    readBase: (file) => { try { return git("show", `${base}:${file}`); } catch { return null; } },
    hostRust: () => parseNameList(git("ls-files", "-z", "--", "host/*.rs", "host/**/*.rs")),
  };
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
  let macos = false;
  if (!docsOnly) {
    const rustDiff = execFileSync("git", ["diff", "-U0", "--no-renames", "--no-color", base, head, "--", "host/*.rs", "host/**/*.rs"], {
      encoding: "utf8",
      maxBuffer: 256 * 1024 * 1024,
    });
    const reasons = macosReasons(paths, gitRepo(base), parseChangedLines(rustDiff));
    for (const [path, why] of reasons) console.log(`macos ${JSON.stringify(path)}: ${why}`);
    macos = reasons.length > 0;
  }
  const devTools = devToolsReasons(paths);
  for (const [path, why] of devTools) console.log(`dev-tools ${JSON.stringify(path)}: ${why}`);
  console.log(`docs_only=${docsOnly}`);
  console.log(`macos=${macos}`);
  console.log(`dev_tools=${devTools.length > 0}`);
  if (process.env.GITHUB_OUTPUT) appendFileSync(process.env.GITHUB_OUTPUT, `docs_only=${docsOnly}\nmacos=${macos}\ndev_tools=${devTools.length > 0}\n`);
}

// Compare real paths: argv[1] may go through a symlink or contain characters
// a file URL escapes (spaces, Windows drive letters).
function isEntryPoint() {
  try { return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href; } catch { return false; }
}

if (isEntryPoint()) {
  try {
    main(process.argv.slice(2));
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
