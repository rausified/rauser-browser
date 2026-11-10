# Jira issue starter draft

Status: release-certified 2026-11-02. John checked this draft against the live
Jira Cloud UI on macOS with real Chrome on 2026-11-02, after both
adapter-template engines verified its grammar and recorded examples on
2026-10-16.

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

`generic_titles = ["Jira", "Log in with Atlassian account", "Log in to
continue - Log in with Atlassian account", "Choose an account to continue -
Log in with Atlassian account"]` lists the titles shown by Atlassian's sign-in
and account pages and the app shell instead of the item's own name
(`adapter-templates.md` §9.4). A visit whose title is one of them, ASCII case
aside, or is empty or the bare host, records no title, so the artifact keeps
the title it already has ("Untitled" for a new one).

Known limits: the board-modal rule accepts `selectedIssue` on any `/jira/`
route, and installations with customized title formats fall back to the whole
tab title. Jira Server and Data Center origins are not auto-suggested, though a
user can adapt the JSON manually and review its fixed safety probes.
