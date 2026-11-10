# aha.feature starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/aha/`, researched blind: public sources only, no
browsing history, cookies or an Aha! account.

History check (2026-11-09): graded against the maintainer's full Chrome
history on this family's hosts, plus the earlier held-back sample's URLs
that have since left that history: 5 real URLs (8 visits) matched, 1/2
rules confirmed, and the title right on 5/5, counting a sign-in, loading
or error title that records no title as right. 2 real URLs of a redirect
chain ending on this type matched no rule, as expected. Not every rule was
matched by a real visit, so this type stays a development draft.

Brauser offers this draft on an Aha! account's tenant subdomain,
`<account>.aha.io` (the `aha` family's strict subdomain origin,
`exclude: ["www", "secure", "support", "status", "cdn", "ideas", "identity",
"iad-prod1"]`), alongside Aha! requirements, epics, releases, ideas and
notes. Any combination can be chosen in Capture Site and in the settings
gallery. The draft records one artifact per feature, however it was opened:

`aha.feature:<account>.aha.io/<reference number>`

The ID is the feature's human-readable reference number (for example
`PRJ-123`), assigned from the workspace prefix. It survives renames, moves
between workspaces and workspace-prefix edits.

```toml
[[artifacts]]
type   = "aha.feature"
origin = "https://acme.aha.io"
match  = [
  { path = "/features/{ref:jira_key}" },
  { path = "/develop/features/{ref:jira_key}" },
]
id     = "aha.feature:{host}/{ref}"
title  = ["{ref:jira_key} {title} | Aha!{~}", "{title} | Aha!{~}"]
generic_titles = ["Log in to your workspace", "Login"]
refs   = [{ type = "aha.feature", find = "jira_key" }]
```

`generic_titles` lists the titles shown by Aha!'s sign-in page instead of the
item's own name (`adapter-templates.md` §9.4). A visit whose title is one of
them, ASCII case aside, or is empty or the bare host, records no title, so the
artifact keeps the title it already has ("Untitled" for a new one).

## What each rule covers

1. **Feature record**, `/features/<ref>`, the canonical route (seen in
   Aha!'s own API fixtures and support links).
2. **Aha! Develop form**, `/develop/features/<ref>`, the same reference
   number reached from Aha! Develop's own route tree (route confirmed to
   exist logged-out; the page itself is undocumented, so this rule's
   confidence is medium-low).

Neither rule carries a rest segment, so `/features/<ref>/edit` never
matches, and `jira_key` rejects a route word such as `/features/board`
(not a reference number) and canonicalizes case, since Aha! reference
numbers are uppercase and paths are case-sensitive.

## Titles

Two entries, tried in order:

1. `{ref:jira_key} {title} | Aha!{~}` — only when the leading word of the
   tab title equals the path's reference number.
2. `{title} | Aha!{~}` — otherwise.

`{~}` after `| Aha!` tolerates a product suffix such as `| Aha! Roadmaps` or
`| Aha! Develop`. If neither entry matches, Brauser records the whole tab
title (`title_entry` null). No public source shows an in-app record page's
tab title; the only observed app title is the login page, `Log in to your
workspace | Aha!`, which is why this draft tries a `| Aha!` suffix at all
(Known limits, below).

`refs` recognizes a `jira_key`-shaped mention (such as `PRJ-77`) inside
another record's title as pointing at an `aha.feature`.

## Recorded samples

A synthetic equivalent of the recorded shape, on `acme.aha.io`:

| URL | Tab title | Records |
|---|---|---|
| `/features/<ref>` | `PRJ-123 Bulk export \| Aha! Roadmaps` | that reference number, title `Bulk export` |

The blind research scored 90 synthetic vectors across its seven drafted
types. Six of them are this family's; the seventh, `aha.portalidea`, lives
on the ideas portal host in its own `aha-ideas` family
(`docs/adapters/aha.portalidea.md`). This family carries over 71 of the
research's cases. This type has 27 vectors: 24 carried over as written (11
`aha.feature` cases and the family's 13 shared negatives, `eval.aha.neg.*`,
which are filed with this type), one research negative rewritten onto this
family's sample origin (`eval.aha.feature.host_in_id`), and two new ones:
`eval.aha.feature.login_title`, pinning the generic login title, and
`eval.aha.neg.features_new`, the `/features/new` form. All are correctly
classified by the research's own draft where it had them, and re-verified
against this family's own fixture through both adapter-template engines.
Graded later against 2 real, held-back Aha! browsing visits
(`docs/research/blind/SCORECARD.md`), the research matched both correctly;
title accuracy was not judgeable site-wide, because the only titles observed
in those 2 visits were Aha!'s login page, not a record page.

## What the draft does not record

- `/records/<ref>`, a generic redirector used by GitHub autolinks: the
  browser lands on the typed URL after the redirect, which these rules then
  match instead;
- `/api/v1/features/...`, the API, not a page;
- `/products/<prefix>/features/<ref>`: the route exists for any `<ref>`, so
  it is most likely a workspace list view, not a record page — excluded as
  unverified;
- `/features/board`, the new-feature form `/features/new` and similar route
  words, since `jira_key` requires a reference-number shape;
- `/features` (the list) and `/features/<ref>/edit` (no rest segment).

## Known limits

- **A login tab's title is generic, not the feature's title.** If Aha!'s
  session has expired, the tab title on a feature URL reads
  `Log in to your workspace | Aha!`. Entry 2 (`{title} | Aha!{~}`) matches
  it and extracts `Log in to your workspace`, which `generic_titles` lists,
  so the visit records no title and the feature keeps its own
  (`eval.aha.feature.login_title`). A login page with a different wording
  would still be recorded until it is added to the list.
- **Every app title template is low confidence.** No public source shows an
  in-app record page's tab title; both entries are a guess from the login
  page's own `| Aha!` suffix. A wrong guess only costs title cleanliness,
  never a wrong ID — the whole tab title is kept when neither entry matches.
- **The `/develop/features/<ref>` route is medium-low confidence.** It
  exists logged-out (a 302), but no public page links to it, so its real
  behavior while logged in is unverified.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Aha! account:

1. Capture Site: open a feature from its workspace board; record the URL
   and the exact tab title (`/features/<ref>`, and does it read `<ref>
   <name> | Aha!...` or something else entirely?).
2. Open the same feature from Aha! Develop (`/develop/features/<ref>`);
   confirm it is the same record and record its tab title.
3. Watch the title while it loads, including a login redirect, to see the
   true sequence behind the "login tab" known limit above.
4. Move a feature to another workspace and rename it; confirm the URL and
   reference number are unchanged.
5. Open `/records/<ref>`, `/api/v1/features/<ref>` and
   `/products/<prefix>/features/<ref>`; record where each lands and what it
   shows, to confirm they stay unmatched on purpose.
6. On a Capture Site review on a feature page with no saved rules, confirm
   Brauser offers all six Aha! types — features, requirements, epics,
   releases, ideas and notes — each with its own Aha!-account statement and
   the development-draft source line; confirm saving one turns the page
   card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `aha.feature`'s
`status` in `protocol/starters/aha.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
