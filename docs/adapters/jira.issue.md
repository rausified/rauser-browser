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
is a two-entry list (`adapter-templates.md` §9.1): `[PROJ-42] Repair export -
Jira` matches the first entry and extracts `Repair export`. A tenant that has
renamed the product (one reading `… - Acme Corp Jira`, for example) never
matches that exact literal, so the second entry, `[{key:jira_key}] {title} -
{~} Jira`, is tried next: it skips back from the end to the nearest ` - `
before `Jira` and drops the company name, extracting `Repair export` the same
way. The first entry stays listed first and exact (no skip) so a title with
its own internal ` - ` (`Design - implementation review - Jira`) still keeps
both halves; only a title the first entry cannot match at all falls through to
the skip entry. Either entry can find uppercase Jira keys mentioned in other
adapter titles (refs) and a mismatched bracketed key (a moved issue's old key,
suggested as an alias); before the second entry was added, a renamed-product
tenant's titles fell back to the whole tab title and lost both (`brwr-njyw`,
history check below). The title key is retained as suggested-alias evidence
for the later alias workflow; it does not silently merge records today.

History check (2026-11-09): of 96 real `jira.issue` URLs (104 recorded
titles) on a renamed-product tenant, 22 titles were genuinely generic
(sign-in interstitials or the bare host) and correctly recorded no title with
either the old or new title list. Of the other 82, the old single-entry list
extracted 0 correctly, because every real title ends ` - <Company> Jira`,
not ` - Jira`; the new second entry extracts 81 of them correctly (key,
title, and any refs or alias), including 26 that mention another issue's key
inline. The one remaining title does not regress this change: it is a board
page opened with `?selectedIssue=`, whose tab title never updates away from
the board's own title (already named below, and in the family file's "Jira
board modal" sample, which records `title: null` for exactly this reason).
This is a title-extraction fix, not a match-rule or ID change: the frozen ID
template and the `/browse/{key}` and `/jira/{*}?selectedIssue={key}` rules
that John certified live on 2026-11-02 are unchanged, so this type keeps its
`release-certified` status rather than reverting to a development draft.

`generic_titles = ["Jira", "Log in with Atlassian account", "Log in to
continue - Log in with Atlassian account", "Choose an account to continue -
Log in with Atlassian account"]` lists the titles shown by Atlassian's sign-in
and account pages and the app shell instead of the item's own name
(`adapter-templates.md` §9.4). A visit whose title is one of them, ASCII case
aside, or is empty or the bare host, records no title, so the artifact keeps
the title it already has ("Untitled" for a new one).

One list covers both the title the template extracts and the whole-title
fallback (§9.4), so an issue whose own summary is `Jira` also records no
title. No recorded title had such a name when the list was set.

Known limits: the board-modal rule accepts `selectedIssue` on any `/jira/`
route, and its tab title does not update to the issue at all (the sample
above records `title: null` for it), so that visit's title is always the
whole tab title. The second title entry only drops one company name between
the issue title and `Jira`; an installation whose title format differs more
than that (no bracketed key, or more than one renamed segment) still falls
back to the whole tab title. Jira Server and Data Center origins are not
auto-suggested, though a user can adapt the JSON manually and review its
fixed safety probes.
