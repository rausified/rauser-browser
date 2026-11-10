# vidcast.page starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/vidcast/`, researched blind: public sources only, no
browsing history, cookies or a Vidcast account.

History check (2026-11-09): graded against the maintainer's full Chrome
history on this family's hosts, plus the earlier held-back sample's URLs
that have since left that history: no real visit of this type, so 0/3
rules confirmed. Not every rule was matched by a real visit, so this type
stays a development draft.

Brauser offers this draft on the exact origin `https://app.vidcast.io`,
alongside Vidcast videos and Vidcast playlists (the `vidcast` family). Any
combination can be chosen in Capture Site and in the settings gallery. The
draft records one artifact per page, however it was opened or edited:

`vidcast.page:app.vidcast.io/<page share ID>`

A Vidcast "Page" is a published showcase of videos, playlists and text
blocks (Library → Pages). **Low confidence on the ID shape:** the research
found no public page link. Video and playlist share IDs are UUIDs from the
same `share_id` model the bundle also uses for pages (`pageShareId`), so
`uuid` is assumed here too. If page share IDs turn out not to be UUIDs,
these rules simply never match — they fail closed, so an unverified
assumption can only under-record this type, never mis-record something else
as a page.

```toml
[[artifacts]]
type   = "vidcast.page"
origin = "https://app.vidcast.io"
match  = [
  { path = "/pages/{page:uuid}", mode = "view" },
  { path = "/pages/{page:uuid}/edit", mode = "edit" },
  { path = "/pages/embed/{page:uuid}", mode = "view" },
]
id     = "vidcast.page:{host}/{page}"
title  = "Vidcast - {title}"
generic_titles = ["Vidcast", "Sign In", "Page Edit"]
```

`generic_titles` lists the titles shown by Vidcast's app shell, sign-in page
and editor instead of the item's own name (`adapter-templates.md` §9.4). A
visit whose title is one of them, ASCII case aside, or is empty or the bare
host, records no title, so the artifact keeps the title it already has
("Untitled" for a new one).

## What each rule covers

1. **Published page**, `/pages/<page share id>`, `mode = "view"`.
2. **Page editor**, `/pages/<page share id>/edit`, `mode = "edit"`.
3. **Embed link opened top-level**, `/pages/embed/<page share id>`, `mode =
   "view"`.

The `/pages` list (no ID, `?tab=`/`?type=`/`?sort=` query only) matches
nothing. `embed` is a literal segment, not a UUID, so rule 1 cannot swallow
rule 3's path, and no rule has a rest segment, so `/pages/<id>/edit/<more>`
matches nothing either.

## Titles

One entry: `Vidcast - {title}` strips the app's title-component prefix, the
same template `vidcast.video` uses, from the bundle's page-title component
(not observed live — medium confidence). The editor shows the fixed title
`Vidcast - Page Edit`; `Page Edit` is a generic title, so an edit visit
records no title (Known limits, below).

## Recorded samples

A synthetic equivalent of the recorded shape, on `app.vidcast.io`:

| URL | Tab title | Records |
|---|---|---|
| `/pages/<36-char share ID>` | `Vidcast - Team updates` | that share ID, view mode, title `Team updates` |

The blind research scored 58 synthetic vectors across this family's three
types (8 `vidcast.page` positives among them), all correctly classified by
the research's own draft and re-verified against this family's own fixture
through both adapter-template engines. None of the 11 real, held-back
browsing visits the scorecard graded (`docs/research/blind/SCORECARD.md`)
happened to be a Vidcast page, so this type's ID shape is untested against
a real URL; the family-wide 11/11 result and stable-ID finding cover
`vidcast.video` and `vidcast.playlist` only.

## What the draft does not record

- the pages list (`/pages` with no ID);
- anything below `/pages/<id>/edit` (no rule has a rest segment there);
- a page share ID that is not a UUID (if the assumption above is wrong, this
  is every real page URL — see Known limits), an empty path segment, or an
  encoded slash inside a segment, the same rejections `vidcast.video` makes.

## Known limits

- **Low confidence on the ID shape.** No public page link was available to
  confirm it; `uuid` is assumed by analogy with video and playlist share
  IDs. If the reviewer prefers only high-confidence starters, this type is
  the one to drop; nothing else in the family depends on it. If page share
  IDs are not UUIDs, every rule above fails closed — nothing is recorded,
  and nothing is mis-recorded as a page.
- **The editor title is fixed.** Every edit visit's tab title is `Vidcast -
  Page Edit`. `Page Edit` is listed in `generic_titles`, so an edit visit
  records no title and the page keeps its own name.
- **Titles are not observed live.** The `Vidcast - <page name>` template
  comes from the app's bundle, not a rendered page.
- Every Vidcast account signed in to the Chrome profile is recorded: no URL
  names the account or organization, so a page's own share settings, not
  this starter, control who can see it.
- The ID template is frozen once shipped (adapter-templates.md §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Vidcast account:

1. Create and publish a Page. Confirm `/pages/<id>` is actually a UUID (or
   fix the capture type here if it is not).
2. Record the view and edit titles, and the embed link's `og:url`.
3. Open a page as a user without access, and an unknown or deleted share ID.
   Record both titles.
4. Rename the page and confirm the tab title and the recorded title follow.
5. On a Capture Site review on a Vidcast Page with no saved rules, Brauser
   offers all three Vidcast types with the every-account statement in each
   type's own description and the development-draft source line; saving
   one turns the page card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `vidcast.page`'s
`status` in `protocol/starters/vidcast.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
