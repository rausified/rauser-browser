# aha.epic starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/aha/`, researched blind: public sources only, no
browsing history, cookies or an Aha! account.

History check (2026-11-09): graded against the maintainer's full Chrome
history on this family's hosts, plus the earlier held-back sample's URLs
that have since left that history: no real visit of this type, so 0/2
rules confirmed. Not every rule was matched by a real visit, so this type
stays a development draft.

Brauser offers this draft on an Aha! account's tenant subdomain,
`<account>.aha.io` (the `aha` family's strict subdomain origin,
`exclude: ["www", "secure", "support", "status", "cdn", "ideas", "identity",
"iad-prod1"]`), alongside Aha! features, requirements, releases, ideas and
notes. Any combination can be chosen in Capture Site and in the settings
gallery. The draft records one artifact per epic (formerly "master
feature"), however it was opened:

`aha.epic:<account>.aha.io/<reference number>`

The ID is the epic's reference number (for example `PRJ-E-12`).

```toml
[[artifacts]]
type   = "aha.epic"
origin = "https://acme.aha.io"
match  = [
  { path = "/epics/{ref:name(5,100)|upper}" },
  { path = "/develop/epics/{ref:name(5,100)|upper}" },
]
id     = "aha.epic:{host}/{ref}"
title  = ["{ref:name} {title} | Aha!{~}", "{title} | Aha!{~}"]
generic_titles = ["Log in to your workspace", "Login"]
refs   = [{ type = "aha.feature", find = "jira_key" }]
```

`generic_titles` lists the titles shown by Aha!'s sign-in page instead of the
item's own name (`adapter-templates.md` §9.4). A visit whose title is one of
them, ASCII case aside, or is empty or the bare host, records no title, so the
artifact keeps the title it already has ("Untitled" for a new one).

## What each rule covers

1. **Epic record**, `/epics/<ref>`. The route is confirmed to exist
   logged-out (a 302), and the reference format `PRJ-E-12` comes from the
   public API docs and Aha!'s own GitHub extension code; no public page
   links to a live epic URL, so overall confidence is medium.
2. **Aha! Develop form**, `/develop/epics/<ref>` (route confirmed to exist
   logged-out; undocumented).

`name(5,100)|upper` requires at least 5 characters, which excludes a short
route word, and folds a hand-typed lowercase reference to uppercase.

## Titles

Two entries, tried in order:

1. `{ref:name} {title} | Aha!{~}` — only when the leading word of the tab
   title equals the path's reference number.
2. `{title} | Aha!{~}` — otherwise.

Low confidence: no public source shows an in-app epic page's tab title, the
same reasoning `aha.feature`'s page gives in full (Known limits, below).

## Recorded samples

A synthetic equivalent of the recorded shape, on `acme.aha.io`:

| URL | Tab title | Records |
|---|---|---|
| `/epics/<ref>` | `PRJ-E-12 Payments platform \| Aha!` | that reference number, title `Payments platform` |

The blind research scored 90 synthetic vectors across this family's seven
drafted types; this PR carries over 9 `aha.epic` cases, all correctly
classified by the research's own draft and re-verified against this
family's own fixture through both adapter-template engines. Graded later
against 2 real, held-back Aha! browsing visits
(`docs/research/blind/SCORECARD.md`), the research matched Aha!'s site-wide
result of 2 of 2 correct — not broken out per type, since neither of those 2
real visits happened to be an epic.

## What the draft does not record

- `/epics/<ref>/features`, an epic's sub-list of features (route exists but
  is not the epic record itself, and no rule carries that rest segment);
- `/master_features/<ref>`, the legacy name (a 404 today);
- a route word shorter than 5 characters.

## Known limits

- **No public page links to a live epic URL.** The route's existence was
  confirmed by a logged-out probe only; its reference format is drawn from
  API docs and GitHub code, not an observed record page.
- **Every title template is low confidence**, the same limit
  `aha.feature`'s page states in full.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Aha! account:

1. Capture Site: open an epic from a roadmap or board; record the URL and
   the exact tab title (`/epics/<ref>`, and does it read `<ref> <name> |
   Aha!...`?).
2. Open the same epic from Aha! Develop; confirm it is the same record and
   record its tab title.
3. Open `/epics/<ref>/features`; confirm it stays unmatched (a sub-list, not
   the epic record).
4. On a Capture Site review on an epic page with no saved rules, confirm
   Brauser offers all six Aha! types — features, requirements, epics,
   releases, ideas and notes — each with its own Aha!-account statement and
   the development-draft source line; confirm saving one turns the page
   card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `aha.epic`'s `status`
in `protocol/starters/aha.json` to `{ "state": "certified", "method":
"live", "checked_on": "<date>", "checked_by": "John" }`, and run `npm run
gen:starters` (`protocol/starters/README.md`). The Capture Site source line
follows the status.
