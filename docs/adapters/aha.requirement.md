# aha.requirement starter draft

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
"iad-prod1"]`), alongside Aha! features, epics, releases, ideas and notes.
Any combination can be chosen in Capture Site and in the settings gallery.
The draft records one artifact per requirement, however it was opened:

`aha.requirement:<account>.aha.io/<reference number>`

The ID is the requirement's own reference number (for example
`PRJ-123-1`, a child of feature `PRJ-123`). It is not folded into its
parent feature: the single path segment `PRJ-123-1` cannot be split into a
feature reference and a child index with this engine's template language,
so a requirement is its own artifact rather than a `within` of its feature.

```toml
[[artifacts]]
type   = "aha.requirement"
origin = "https://acme.aha.io"
match  = [
  { path = "/requirements/{ref:name(5,100)|upper}" },
  { path = "/develop/requirements/{ref:name(5,100)|upper}" },
]
id     = "aha.requirement:{host}/{ref}"
title  = ["{ref:name} {title} | Aha!{~}", "{title} | Aha!{~}"]
generic_titles = ["Log in to your workspace", "Login"]
refs   = [{ type = "aha.feature", find = "jira_key" }]
```

`generic_titles` lists the titles shown by Aha!'s sign-in page instead of the
item's own name (`adapter-templates.md` §9.4). A visit whose title is one of
them, ASCII case aside, or is empty or the bare host, records no title, so the
artifact keeps the title it already has ("Untitled" for a new one).

One list covers both the title the template extracts and the whole-title
fallback (§9.4), so an item whose own name is `Login` also records no title.
No recorded title had such a name when the list was set.

## What each rule covers

1. **Requirement record**, `/requirements/<ref>` (API fixture URLs and
   Aha!'s own support links, including the noise query
   `?full_screen=<report id>`, which never changes the ID).
2. **Aha! Develop form**, `/develop/requirements/<ref>` (route confirmed to
   exist logged-out; undocumented, so medium-low confidence).

`name(5,100)|upper` requires at least 5 characters, which keeps a short
route word such as `new` out, and folds a hand-typed lowercase reference
into the same uppercase ID Aha! itself uses.

## Titles

Two entries, tried in order:

1. `{ref:name} {title} | Aha!{~}` — only when the leading word of the tab
   title equals the path's reference number.
2. `{title} | Aha!{~}` — otherwise.

Low confidence everywhere: no public source shows an in-app requirement
page's tab title (Known limits, below, and `aha.feature`'s page for the
shared reasoning). `refs` recognizes a `jira_key`-shaped mention inside a
requirement's title as pointing at an `aha.feature`.

## Recorded samples

A synthetic equivalent of the recorded shape, on `acme.aha.io`:

| URL | Tab title | Records |
|---|---|---|
| `/requirements/<ref>` | `PRJ-123-1 Validate card number \| Aha!` | that reference number, title `Validate card number` |

The blind research scored 90 synthetic vectors across this family's seven
drafted types. This type has 9 vectors: the research's 8 requirement
cases (`req.*` there, `eval.aha.requirement.*` here), all correctly
classified by the research's own draft, and one new negative,
`eval.aha.neg.short_word_requirement` (`/requirements/new`). All 9 are
re-verified against this family's own fixture through both adapter-template
engines. Graded later
against 2 real, held-back Aha! browsing visits
(`docs/research/blind/SCORECARD.md`), the research matched Aha!'s site-wide
result of 2 of 2 correct — not broken out per type, since neither of those 2
real visits happened to be a requirement.

## What the draft does not record

- `/requirements/new` and any other route word shorter than 5 characters
  (not a reference number);
- `/requirements/<ref>/<anything>` (no rest segment carried by either
  rule).

## Known limits

- **Every title template is low confidence**, the same limit
  `aha.feature`'s page states in full: no public source shows an in-app
  requirement page's tab title, so both entries are a guess from the login
  page's own `| Aha!` suffix.
- **The `/develop/requirements/<ref>` route is medium-low confidence**, the
  same `aha.feature`-page limit: it exists logged-out, but its logged-in
  behavior is unverified.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Aha! account:

1. Capture Site: open a requirement from its parent feature; record the URL
   and the exact tab title (`/requirements/<ref>`, and does it read `<ref>
   <name> | Aha!...`?).
2. Open the same requirement from Aha! Develop; confirm it is the same
   record and record its tab title.
3. Move the parent feature to another workspace and confirm the
   requirement's own reference number and URL are unchanged.
4. On a Capture Site review on a requirement page with no saved rules,
   confirm Brauser offers all six Aha! types — features, requirements,
   epics, releases, ideas and notes — each with its own Aha!-account
   statement and the development-draft source line; confirm saving one
   turns the page card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `aha.requirement`'s
`status` in `protocol/starters/aha.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
