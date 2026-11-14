## 2026-11-13 — Pin the GraphQL query to the fields snapshots read

What: add a test that every field `snapshotFromGraphql` reads (in `scripts/dev/queue-pr.mjs`) appears in its `QUERY`. Or build the PR test fixtures (`scripts/tests/pr-fixtures.mjs` `graphql()`) by filtering a full object through the query's field list, so a fixture can't carry a field the real query never fetches.

Why: in lane K (PR #462), brwr-5yy0's fix read `commit.committedDate`, and the hand-built fixtures supplied it, so all 134 focused tests passed. But `QUERY` didn't ask for the field. Only the keeper's live `--dry-run` showed the problem: a check rerun after a body edit dated #421's head, so the keeper misreported a live queue request as stale. A one-line query test now pins `committedDate`, but the general gap remains for the next field.

Where: `scripts/dev/queue-pr.mjs` (`QUERY`, `snapshotFromGraphql`), `scripts/tests/pr-fixtures.mjs`, `scripts/tests/queue-pr.test.mjs`.
