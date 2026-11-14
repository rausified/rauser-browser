// Names shared by the PR tools: pr-doctor, the PR keeper, scripts/dev/queue-pr.mjs
// and scripts/dev/pr-watch.mjs. A leaf module, so those can import each other's
// helpers without an import cycle.

/** pr-doctor's sticky comment marker. */
export const DOCTOR_MARKER = "<!-- pr-doctor -->";
/** The PR keeper's sticky comment marker (scripts/pr-keeper.mjs). */
export const KEEPER_MARKER = "<!-- pr-keeper -->";
/** pr-doctor's label for a conflict it could not resolve; excluded by .mergify.yml queue_conditions. */
export const NEEDS_MERGE_LABEL = "needs-merge";
/** Opt a PR out of pr-doctor, the keeper and the Mergify queue. */
export const DOCTOR_SKIP_LABEL = "pr-doctor:skip";
/** The Mergify queue label (worker-contract.json merge.label). */
export const QUEUE_LABEL = "queue";
/** What actually queues a labelled PR (worker-contract.json merge.comment). */
export const QUEUE_COMMENT = "@Mergifyio queue";
export const REFRESH_COMMENT = "@Mergifyio refresh";
/** Cookie's driver holds a PR it must not merge yet (a follow-on, or red past its repair budget). */
export const HELD_LABEL = "held";
export const QUEUE_EXCLUSION_LABELS = Object.freeze([NEEDS_MERGE_LABEL, DOCTOR_SKIP_LABEL]);
/** Whose marker comments the PR tools believe: the workflow's own bot. Callers add the
 * PR_DOCTOR_TOKEN owner (trustedAuthors in scripts/dev/queue-pr.mjs). Anyone can post a
 * `<!-- pr-doctor -->` comment on a public repository. */
export const TRUSTED_AUTHORS = Object.freeze(["github-actions[bot]", "github-actions"]);
