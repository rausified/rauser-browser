## 2026-11-13 — Check ancestry before promising a deploy fast-forward

Change: Have the deploy dry run classify the local checkout as equal, behind, ahead or diverged from the observed remote main commit before saying it would fast-forward.
Report local-only commits and refuse that promise unless local HEAD is an ancestor of the intended target.
If the remote object is not available to a read-only run, report ancestry as unknown instead of inferring it from different SHAs.
Preserve local work and keep the acting deploy's fast-forward-only checks.

Why: A catch-up investigation found a shared main checkout two commits ahead and 93 commits behind its remote.
The local commits preserved a documentation review and a queue remote configuration update, so a fast-forward pull could not succeed.
The existing dry run nevertheless reported that it would fetch and fast-forward based on unequal SHAs, hiding the reconciliation required before deployment.
The expected-SHA acting path checks ancestry already; the dry run needs the same truthful distinction.

Where: the remote observation and dry-run paths in `scripts/dev/deploy-live`, with cases for ahead, diverged and unavailable remote objects in `scripts/tests/test_deploy_live.py`.
