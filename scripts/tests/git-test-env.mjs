// Quiet temporary git repositories for the Node script tests; the same
// settings as scripts/tests/git_test_env.py. git's background maintenance
// (`git maintenance run --auto --detach`, a detached `gc --auto`) can create
// and delete .git/objects/maintenance.lock while a test removes its scratch
// repository (PR #267, 2026-11-03), so it is turned off through
// GIT_CONFIG_COUNT/KEY/VALUE, which every child git (and bd/Dolt's) inherits.

export const QUIET_CONFIG = Object.freeze([
  ["maintenance.auto", "false"],
  ["gc.auto", "0"],
  ["gc.autoDetach", "false"],
]);

/** `base` plus the quiet settings, appended after any GIT_CONFIG_* entries it has. */
export function quietGitEnv(base = process.env) {
  const out = { ...base };
  let count = Number(out.GIT_CONFIG_COUNT || 0);
  for (const [key, value] of QUIET_CONFIG) {
    out[`GIT_CONFIG_KEY_${count}`] = key;
    out[`GIT_CONFIG_VALUE_${count}`] = value;
    count += 1;
  }
  out.GIT_CONFIG_COUNT = String(count);
  return out;
}
