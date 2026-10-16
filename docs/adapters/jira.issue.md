# Jira issue starter draft

Status: development draft. The adapter grammar and recorded examples were
verified by both adapter-template engines on 2026-10-16. A live Jira Cloud UI
check is still required before this draft is release-certified.

Brauser offers this draft when an enabled exact HTTPS origin is a Jira Cloud
tenant, such as `https://acme.atlassian.net`. It recognizes both the canonical
issue page and a board modal:

- `https://acme.atlassian.net/browse/PROJ-42`
- `https://acme.atlassian.net/jira/software/projects/PROJ/boards/7?selectedIssue=PROJ-42`

Both forms produce the frozen ID `jira:acme.atlassian.net/PROJ-42`. The title
template understands `[PROJ-42] Repair export - Jira`, extracts `Repair
export`, and can find uppercase Jira keys mentioned in other adapter titles.
The title key is retained as suggested-alias evidence for the later alias
workflow; it does not silently merge records today.

Known limits: the board-modal rule accepts `selectedIssue` on any `/jira/`
route, and installations with customized title formats fall back to the whole
tab title. Jira Server and Data Center origins are not auto-suggested, though a
user can adapt the JSON manually and review its fixed safety probes.
