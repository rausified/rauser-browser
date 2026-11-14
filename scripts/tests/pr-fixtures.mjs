// Shared PR and check fixtures for the PR tool tests (pr-watch, queue-pr, pr-keeper, the
// Mergify replay), in the recorded shape of queue-pr.mjs's GraphQL query. Not a test file.
import { readFileSync } from "node:fs";

import { snapshotFromGraphql } from "../dev/queue-pr.mjs";

/** Mergify's own comments, recorded from this repository (fixtures/mergify-comments.json `_source`). */
export const FX = JSON.parse(readFileSync(new URL("./fixtures/mergify-comments.json", import.meta.url), "utf8"));

export const NOW = "2026-11-02T20:00:00Z";
export const ago = (m, from = NOW) => new Date(Date.parse(from) - m * 60000).toISOString();

export const run = (name, status, conclusion, { required = false, runId = 41500842838, startedAt = "2026-11-02T18:33:19Z", completedAt = status === "COMPLETED" ? ago(60) : null } = {}) =>
  ({ __typename: "CheckRun", name, status, conclusion, isRequired: required, startedAt, completedAt, checkSuite: { workflowRun: { databaseId: runId } } });
export const done = (name, conclusion = "SUCCESS", opts) => run(name, "COMPLETED", conclusion, opts);
export const status = (context, state, required = true) => ({ __typename: "StatusContext", context, state, isRequired: required, createdAt: ago(60) });

/** Every check green, finished an hour before NOW (or `at`). */
export function greenChecks(at = ago(60)) {
  const opts = (more = {}) => ({ completedAt: at, ...more });
  return [
    done("Classify changes", "SUCCESS", opts()),
    done("PR owner", "SUCCESS", opts({ required: true, runId: 41500842845 })),
    done("Rust dependency policy", "SUCCESS", opts({ required: true })),
    done("Docs and queue checks", "SKIPPED", opts()),
    done("Windows / Build and check (windows-latest)", "SUCCESS", opts()),
    done("Build and check", "SUCCESS", opts({ required: true })),
  ];
}
export function rollup(overrides = [], base = greenChecks()) {
  const names = new Set(overrides.map((node) => node.name ?? node.context));
  return [...base.filter((node) => !names.has(node.name ?? node.context)), ...overrides];
}
const gql = (c) => ({ author: { login: c.author }, body: c.body, createdAt: c.createdAt, updatedAt: c.updatedAt ?? c.createdAt });
/** `committedDate`: the head commit's committer date; `checkSuites`: its check suites' creation
 * times (set at the push). With neither, the head's time is unknown: no head-time checks. */
export function graphql({ pr = {}, contexts = greenChecks(), comments = [], labels = ["queue"], committedDate = null, checkSuites = [] } = {}) {
  return { repository: { pullRequest: {
    number: 243, state: "OPEN", isDraft: false, mergeable: "MERGEABLE", mergeStateStatus: "CLEAN",
    headRefOid: "6629a6b3a95ba330a0a564eb5f22a0e9fe7a843a", headRefName: "feat/example", baseRefName: "main", isCrossRepository: false, autoMergeRequest: null,
    labels: { nodes: labels.map((name) => ({ name })) }, comments: { nodes: comments.map(gql) },
    commits: { nodes: [{ commit: { committedDate, checkSuites: { nodes: checkSuites.map((createdAt) => ({ createdAt })) }, statusCheckRollup: { contexts: { nodes: contexts } } } }] },
    ...pr,
  } } };
}
export const snap = (opts) => snapshotFromGraphql(graphql(opts), { now: opts?.now });
/** A queue command from a (synthetic) session. */
export const command = (body, createdAt, author = "dev-session") => ({ author, body, createdAt, updatedAt: createdAt });
