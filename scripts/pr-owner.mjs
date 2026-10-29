// PR owner check (AGENTS.md "PR ownership and handoff"): every PR body names
// the session that owns it on an `Owner:` line, so a PR whose session died can
// be found and reassigned by the coordinator instead of being orphaned. Cookie's
// overnight driver PRs are exempt: the driver opens them on `cookie/` branches
// with a "Cookie cycle N:" title, and Cookie itself is their owner. The author
// login is not used for the exemption, because every session and Cookie push as
// the same account.
//
// Run by .github/workflows/pr-owner.yml, which reads the pull_request event
// payload from GITHUB_EVENT_PATH; `checkPrOwner` is the pure part the tests pin.

import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const COOKIE_BRANCH = /^cookie\//;
const COOKIE_TITLE = /^cookie cycle\b/i;
// A line that starts with optional list marker and bold, then `Owner:`.
const OWNER_LINE = /^[ \t]*(?:[-*+][ \t]+)?(?:\*\*|__)?Owner(?:\*\*|__)?[ \t]*:(?:\*\*|__)?[ \t]*(?<owner>.*)$/im;

/** True for a PR opened by Cookie's overnight driver. */
export function isCookiePr(pr) {
  return COOKIE_BRANCH.test(pr.head?.ref ?? "") || COOKIE_TITLE.test((pr.title ?? "").trim());
}

/** The body with HTML comments and fenced code blocks removed. */
function visibleText(body) {
  return body
    .replace(/<!--[\s\S]*?(?:-->|$)/g, "")
    .replace(/^[ \t]*(```|~~~)[\s\S]*?(?:^[ \t]*\1[ \t]*$|(?![\s\S]))/gm, "");
}

/** Whether a PR names its owner session; `{ ok, owner?, exempt?, reason? }`. */
export function checkPrOwner(pr) {
  if (isCookiePr(pr)) return { ok: true, exempt: true };
  const match = OWNER_LINE.exec(visibleText(pr.body ?? ""));
  const owner = match?.groups.owner.trim() ?? "";
  if (owner && !/^<[^>]*>$/.test(owner)) return { ok: true, owner };
  return {
    ok: false,
    reason:
      "The PR body has no filled-in 'Owner:' line. Add 'Owner: <your session>' " +
      "(see .github/pull_request_template.md and AGENTS.md \"PR ownership and handoff\").",
  };
}

function main() {
  const path = process.env.GITHUB_EVENT_PATH;
  if (!path) throw new Error("GITHUB_EVENT_PATH is required");
  const event = JSON.parse(readFileSync(path, "utf8"));
  if (!event.pull_request) throw new Error("not a pull_request event");
  const result = checkPrOwner(event.pull_request);
  if (result.exempt) console.log("Cookie driver PR: owner check skipped.");
  else if (result.ok) console.log(`Owner: ${result.owner}`);
  else {
    console.log(`::error title=PR owner missing::${result.reason}`);
    process.exitCode = 1;
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) main();
