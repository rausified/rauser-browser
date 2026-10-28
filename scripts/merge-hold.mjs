// Merge hold (AGENTS.md "Review and merge cadence"): while any open PR carries
// the `final-stretch` label, every other open PR (Cookie's and every
// session's) gets a pending `merge-hold` status, a required check, so neither
// auto-merge nor a manual merge lands under a big PR's last merge. Every
// labeled PR gets success, so the check never deadlocks, even with two final
// stretches at once. Pending, not failure: the PR simply waits, and the
// overnight driver, which only enables auto-merge, never sees a failed check.
//
// Merge queue: `merge-hold` is a required check, so it must also report on the
// queue's temporary merge_group commit. A PR can only enter the queue once its
// head passed the hold, so in a merge_group run (`--merge-group <sha>`) this
// posts success on that commit and nothing else. A pending hold on a PR head
// still keeps it out of the queue.
//
// Run by .github/workflows/merge-hold.yml with GITHUB_TOKEN and
// GITHUB_REPOSITORY set; `decideStatuses`, `mergeGroupStatus` and `parseArgs`
// are the pure parts the tests pin.

export const STATUS_CONTEXT = "merge-hold";
export const HOLD_LABEL = "final-stretch";

const MAX_DESCRIPTION = 140;

function labeled(pr) {
  return (pr.labels ?? []).some((label) => label.name === HOLD_LABEL);
}

/** The `merge-hold` status for each open PR's head commit. */
export function decideStatuses(openPrs) {
  const stretching = openPrs.filter(labeled).map((pr) => `#${pr.number}`);
  return openPrs.map((pr) => {
    const held = stretching.length > 0 && !labeled(pr);
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

/** The `merge-hold` status for a merge queue entry's merge_group commit. */
export function mergeGroupStatus(sha) {
  if (!/^[0-9a-f]{40}$/.test(sha)) throw new Error(`merge_group needs a full commit SHA, got ${JSON.stringify(sha)}`);
  return {
    sha,
    state: "success",
    context: STATUS_CONTEXT,
    description: "Merge queue entry: its PR passed the merge hold to be enqueued",
  };
}

/** `[]` sweeps every open PR; `["--merge-group", sha]` reports one merge_group commit. */
export function parseArgs(argv) {
  if (argv.length === 0) return { mergeGroupSha: null };
  if (argv[0] !== "--merge-group") throw new Error(`unknown argument ${JSON.stringify(argv[0])}`);
  if (argv.length !== 2 || !argv[1]) throw new Error("--merge-group needs a commit SHA");
  return { mergeGroupSha: argv[1] };
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

async function post(repo, status) {
  await github(`/repos/${repo}/statuses/${status.sha}`, {
    method: "POST",
    body: JSON.stringify({ state: status.state, context: status.context, description: status.description }),
  });
  console.log(`${status.sha.slice(0, 7)} ${status.state}: ${status.description}`);
}

async function main() {
  const { mergeGroupSha } = parseArgs(process.argv.slice(2));
  const repo = process.env.GITHUB_REPOSITORY;
  if (!repo || !process.env.GITHUB_TOKEN) throw new Error("GITHUB_REPOSITORY and GITHUB_TOKEN are required");
  if (mergeGroupSha !== null) {
    await post(repo, mergeGroupStatus(mergeGroupSha));
    return;
  }
  const open = [];
  for (let page = 1; ; page += 1) {
    const batch = await github(`/repos/${repo}/pulls?state=open&per_page=100&page=${page}`);
    open.push(...batch);
    if (batch.length < 100) break;
  }
  for (const status of decideStatuses(open)) await post(repo, status);
}

if (import.meta.url === `file://${process.argv[1]}`) {
  main().catch((error) => {
    console.error(error.message);
    process.exit(1);
  });
}
