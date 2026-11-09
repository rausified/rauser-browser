# aha.portalidea starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/aha/`, researched blind: public sources only, no
browsing history, cookies or an Aha! account. The idea page and its title
were observed logged-out on a public ideas portal.

History check (2026-11-09): graded against a held-back sample of real
browsing history (real URLs from eight researched sites): no visits of
this type in the sample, so 0/1 rules confirmed. Not every rule and
title was confirmed, so this type stays a development draft.

Brauser offers this draft on an Aha! ideas portal, `<portal>.ideas.aha.io`
(the `aha-ideas` family's strict subdomain origin, `labels: 1`). It is its
own family, next to the `aha` family on `<account>.aha.io`: the `aha`
family's `labels: 1` means it can never claim a two-label portal host such
as `acme.ideas.aha.io`, so the generator's origin-collision check lets the
two coexist (`protocol/starters/README.md`, "Origin collisions"). The draft
records one artifact per idea opened on the portal:

`aha.portalidea:<portal>.ideas.aha.io/<reference number>`

The ID is the idea's reference number (for example `PRJ-I-45`), the same
number the in-app `aha.idea` uses. The host differs, so the same idea seen
in-app (`aha.idea:acme.aha.io/PRJ-I-45`) and on a portal
(`aha.portalidea:acme.ideas.aha.io/PRJ-I-45`) is two artifacts: they are two
views for two audiences, not a duplicate.

```toml
[[artifacts]]
type   = "aha.portalidea"
origin = "https://acme.ideas.aha.io"
match  = [
  { path = "/ideas/{ref:name(5,100)|upper}" },
]
id     = "aha.portalidea:{host}/{ref}"
title  = "{title} | {~}"
```

## What each rule covers

1. **Portal idea page**, `/ideas/<ref>`. Observed directly; the page's
   canonical link is the same URL, and a trailing slash or a query string
   serves the same idea.

`|upper` folds a hand-typed lowercase ref into the same ID. A lowercase ref
does not resolve on the portal (it redirects to login), so folding never
merges two distinct ideas.

## Titles

One entry: `{title} | {~}`. A portal idea's tab reads `<idea name> |
<portal title>` (observed: `… | Aha! Big Ideas`), and the portal title is
chosen per portal, so `{~}` skips it back to the last ` | `. An idea name
that contains ` | ` is kept whole. A title with no ` | ` at all matches no
entry and is kept whole (`title_entry` null).

## Recorded samples

A synthetic equivalent of the observed shape, on `acme.ideas.aha.io`:

| URL | Tab title | Records |
|---|---|---|
| `/ideas/<ref>` | `Allow hiding unused fields \| Acme Ideas` | that reference number, title `Allow hiding unused fields` |

The blind research's portal cases are carried over as this type's vectors
and verified against this family's own fixture through both
adapter-template engines. None of the real, held-back Aha! browsing visits
in `docs/research/blind/SCORECARD.md` was on a portal.

## What the draft does not record

- `/ideas/new`, the new-idea form (too short for `name(5,100)`);
- `/ideas` and `/ideas/?page=<n>`, the idea list;
- `/ideas/<ref>/idea_comments/…` and `/ideas/<ref>/idea_endorsements/…`,
  comment and vote forms under an idea;
- `/portal_session/new`, the portal login;
- the portal home, `/?category=…`;
- the in-app path `/ideas/ideas/<ref>` on a portal host.

## Known limits

- **Custom-domain portals are not offered.** A portal can use a customer
  CNAME such as `ideas.example.com`, which no suffix rule can detect. The
  paths are the same there, so a user who sets the adapter up on that origin
  by hand gets the same rules.
- **A portal title that contains ` | ` leaves its first part in the
  recorded title**, since `{~}` only skips back to the last ` | `.
- **Private portals are unverified.** Only a public portal was observed.
- **The in-app and portal views of one idea are not linked.** Whether they
  should be (a `within` or alias relation) is open.
- A route word of 5 or more characters directly under `/ideas/` would be
  recorded as an idea. None turned up: unknown words redirect to login, and
  `/ideas/search` redirects to `/ideas`.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Aha! ideas portal:

1. Capture Site: open an idea on a portal; record the URL and the exact tab
   title (`/ideas/<ref>`, `<idea name> | <portal title>`?).
2. Repeat on a private portal and, if one is available, on a custom-domain
   portal; confirm the URL and the title format match the public portal.
3. Open `/ideas/new`, the idea list and the portal home; confirm none is
   offered as an artifact.
4. On a Capture Site review on a portal idea page with no saved rules,
   confirm Brauser offers Aha! portal ideas with its Aha!-portal statement
   and the development-draft source line; confirm saving it turns the page
   card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `aha.portalidea`'s
`status` in `protocol/starters/aha-ideas.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
