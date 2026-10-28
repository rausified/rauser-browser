// Merge hold (AGENTS.md "Review and merge cadence"): while any open PR carries
// the `final-stretch` label, PRs from held branches (Cookie's) get a pending
// `merge-hold` status, so their auto-merge waits instead of landing under a
// big PR's last merge. Every other PR, and the labeled PRs themselves, get
// success, so the required check never deadlocks. Pending, not failure: the
// PR simply waits, and the overnight driver, which only enables auto-merge,
// never sees a failed check.
//
// Run by .github/workflows/merge-hold.yml with GITHUB_TOKEN and
// GITHUB_REPOSITORY set; `decideStatuses` is the pure part the tests pin.

export const STATUS_CONTEXT = "merge-hold";
export const HOLD_LABEL = "final-stretch";
export const HELD_PREFIXES = ["cookie/"];

const MAX_DESCRIPTION = 140;

function labeled(pr) {
  return (pr.labels ?? []).some((label) => label.name === HOLD_LABEL);
}

/** The `merge-hold` status for each open PR's head commit. */
export function decideStatuses(openPrs) {
  const stretching = openPrs.filter(labeled).map((pr) => `#${pr.number}`);
  return openPrs.map((pr) => {
    const held = stretching.length > 0 && !labeled(pr) &&
      HELD_PREFIXES.some((prefix) => pr.head.ref.startsWith(prefix));
    const description = held
      ? `Held while ${stretching.join(", ")} ${stretching.length === 1 ? "is" : "are"} in the final stretch`
      : "No merge hold";
    return {
      sha: pr.head.sha,
      state: held ? "pending" : "success",
      context: STATUS_CONTEXT,
      description: description.slice(0, MAX_DESCRIPTION),
    };
  });
}

async function github(path, init = {}) {
  const response = await fetch(`https://api.github.com${path}`, {
    ...init,
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${process.env.GITHUB_TOKEN}`,
      "X-GitHub-Api-Version": "2022-11-28",
      ...init.headers,
    },
  });
  if (!response.ok) throw new Error(`${init.method ?? "GET"} ${path}: ${response.status} ${await response.text()}`);
  return response.json();
}

async function main() {
  const repo = process.env.GITHUB_REPOSITORY;
  if (!repo || !process.env.GITHUB_TOKEN) throw new Error("GITHUB_REPOSITORY and GITHUB_TOKEN are required");
  const open = [];
  for (let page = 1; ; page += 1) {
    const batch = await github(`/repos/${repo}/pulls?state=open&per_page=100&page=${page}`);
    open.push(...batch);
    if (batch.length < 100) break;
  }
  for (const status of decideStatuses(open)) {
    await github(`/repos/${repo}/statuses/${status.sha}`, {
      method: "POST",
      body: JSON.stringify({ state: status.state, context: status.context, description: status.description }),
    });
    console.log(`${status.sha.slice(0, 7)} ${status.state}: ${status.description}`);
  }
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
