## 2026-11-03 — Choose a focus session when sharing work

Add an explicit session selector to Recent's selected-work preview (`brwr-1jeb`,
discovered from `brwr-ued`). Named sessions now group the full worklog, but the
curation picker still selects artifact-days. The new regression uses one
artifact in two sessions within the same hour: selecting its day cannot express
which deliberate block to share. Query direct activity by session ID and show an
exact copy preview, retaining date bounds, omission notices and container
exclusion. Extend `host/src/index/query.rs`, the panel's worklog curation and
its focused tests; do not infer session membership from time windows.
