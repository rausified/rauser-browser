// Build identity for extension/build.mjs: the checkout's commit, marked
// "-dirty" when tracked files differ from it. The host's copy is
// host/src/build_id.rs, run from host/build.rs; keep the two in step.
import { spawnSync } from "node:child_process";

// Commits are abbreviated to a fixed length so both builds print the same.
const COMMIT_LENGTH = 12;

export function buildId(commit, status) {
  const short = commit?.trim();
  if (!short) return "unknown";
  // A status git could not report is not claimed clean.
  return status === "" ? short : `${short}-dirty`;
}

function gitOutput(dir, args, env) {
  const result = spawnSync("git", ["-C", dir, ...args], { encoding: "utf8", env });
  return result.status === 0 ? result.stdout : null;
}

export function gitBuildId(dir, env = process.env) {
  const commit = gitOutput(dir, ["rev-parse", `--short=${COMMIT_LENGTH}`, "HEAD"], env);
  if (commit === null) return buildId(null, null);
  // Untracked files are not built, so stray scratch files are not "dirty".
  return buildId(commit, gitOutput(dir, ["status", "--porcelain", "--untracked-files=no"], env));
}

export function fillPlaceholders(source, values, name) {
  const filled = source.replaceAll(/\{\{([A-Z_]+)\}\}/g, (match, key) => values[key] ?? match);
  if (filled.includes("{{")) throw new Error(`Unknown placeholder in extension/${name}`);
  return filled;
}
