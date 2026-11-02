# Confluence page starter draft

Status: development draft. The URL and tab-title formats were checked on
2026-11-02 against real Confluence Cloud visits in the maintainer's Chrome
history (pages in team and personal spaces, the editor, and version history).
Both adapter-template engines verify the samples below. A live check in the
Capture Site review and the settings gallery is still needed before this draft
is release-certified (bead `brwr-cf2`).

Brauser offers this draft when an enabled exact HTTPS origin is an Atlassian
Cloud site, such as `https://acme.atlassian.net`, the same origin family as the
Jira issue starter. That origin offers two starters, **Jira issues** and
**Confluence pages**; either can be chosen alone in Capture Site and in the
settings gallery. The draft records one artifact per page:

`confluence.page:acme.atlassian.net/<page ID>`

The ID is the numeric page ID alone, never the space key, so a page moved to
another space keeps its identity and its notes. It is type-qualified, like
`github.pr` and `miro.board`, so later Confluence starters (blog posts,
whiteboards, databases) each get their own namespace on the same origin.

```toml
[[artifacts]]
type   = "confluence.page"
origin = "https://acme.atlassian.net"
match  = [
  { path = "/wiki/spaces/{_}/pages/{id:int}/{*}" },
  { path = "/wiki/spaces/{_}/pages/edit-v2/{id:int}", mode = "edit" },
  { path = "/wiki/spaces/{_}/history/{id:int}/{*}", mode = "view" },
  { path = "/wiki/pages/viewpage.action", query = { pageId = "{id:int}" } },
]
id     = "confluence.page:{host}/{id}"
title  = ["Version history - {title} - {~} - Confluence", "{title} - {~} - Confluence"]
refs   = [{ type = "jira.issue", find = "jira_key" }]
```

## What each rule covers

1. **Page view**, `/wiki/spaces/<space>/pages/<id>/<slug>`. The slug is
   optional: `{*}` takes zero or more segments, so a page with no slug, a
   trailing slash, or a long slug all match. `{_}` takes any space key,
   including a personal space's `~<account ID>` key. No mode is recorded.
2. **Editor**, `/wiki/spaces/<space>/pages/edit-v2/<id>`, recorded in edit
   mode. The rule has no rest segment, so a path below the editor is not
   recorded.
3. **Version history**, `/wiki/spaces/<space>/history/<id>/<slug>`, slug
   optional, recorded in view mode. Looking through a page's history counts
   toward the page itself.
4. **Legacy link**, `/wiki/pages/viewpage.action?pageId=<id>`. Older links and
   some integrations use this form; Confluence then redirects to the page.

Query parameters and fragments never change the ID: page-tree links
(`?atl_f=PAGETREE`), sharing and analytics values (`?xpis=…`,
`?search_id=…`, `?additional_analytics=…`) and heading anchors (`#…`) all
record the same page.

Jira's rules (`/browse/…`, `/jira/…`) and these (`/wiki/…`) never overlap, so
the two starters can share the origin in either order.

## Titles

Confluence tab titles read `<page title> - <space name> - Confluence`. The
second title entry, `{title} - {~} - Confluence`, keeps the page title and
skips the space name. A Jira key in the page title (an uppercase key such as
`OPS-12`) becomes a mention of that Jira issue.

The version-history view reads `Version history - <page title> - <space name> -
Confluence`. Without help, a history visit would rename the page to
`Version history - <page title>` until its next ordinary visit, so the first
entry strips that prefix. **Trade-off:** a real page whose own title starts
with `Version history - ` loses that prefix too (`Version history - Policy` is
recorded as `Policy`). That is rarer than looking at a page's history, so the
entry is kept.

No editor title format has been recorded yet, so neither entry is written for
the editor. If the editor's tab title follows the page form, the second entry
already covers it; the live check records what it actually is.

## Recorded samples

Synthetic equivalents of the recorded shapes, on `acme.atlassian.net`. Each one
is a shared vector (`eval.confluence.*` in
`protocol/adapter-templates/vectors.json`):

| URL | Tab title | Records |
|---|---|---|
| `/wiki/spaces/ENG/pages/1493076381/Release+Plan` | `Release plan - Engineering - Confluence` | `…/1493076381`, title `Release plan` |
| `/wiki/spaces/OPS/pages/1509755905` (no slug) | same form | `…/1509755905` |
| `/wiki/spaces/~0123456789abcdef0123456789abcdef012345/pages/1538895180/Some+Page` | `Some Page - Alex Example - Confluence` (a personal space is named after its owner) | `…/1538895180`, title `Some Page` |
| `/wiki/spaces/ENG/pages/1493076381/Release+Plan?atl_f=PAGETREE`, `?xpis=…`, `?search_id=…&additional_analytics=…`, `#0a1b2c3d4e5f` | | `…/1493076381` |
| `/wiki/spaces/OPS/pages/edit-v2/1509755905` | not recorded | `…/1509755905`, edit mode |
| `/wiki/spaces/ENG/history/1511794830/Release+Plan` | `Version history - Release plan - Engineering - Confluence` | `…/1511794830`, view mode, title `Release plan` |
| `/wiki/pages/viewpage.action?pageId=1493076381` | `OPS-12 rollout plan - Engineering - Confluence` | `…/1493076381`, title `OPS-12 rollout plan`, mentions `OPS-12` |

## What the draft does not record

These Confluence routes are not pages, or are other content types left for
their own starters; each is a negative vector:

- the Confluence root and feeds: `/wiki`, `/wiki/home`, `/wiki/home/<space>`;
- search: `/wiki/search?text=…`;
- space-level routes: `/wiki/spaces/<space>`, `/wiki/spaces/<space>/overview`,
  `/wiki/spaces/<space>/pages` (the page list);
- databases (`/wiki/spaces/<space>/database/<id>`), whiteboards
  (`…/whiteboard/<id>`), blog posts (`…/blog/<yyyy>/<mm>/<dd>/<id>/<slug>`)
  and folders (`…/folder/<id>`);
- short links (`/wiki/x/<code>`), which name no page ID in the URL; Confluence
  redirects them to the page, and that visit is recorded;
- a legacy link without `pageId` (`?spaceKey=…&title=…`), with a repeated
  `pageId`, or with a non-numeric one.

**Scope.** None is needed. `<site>.atlassian.net` is one company's site, not a
shared origin (adapter-templates.md §6), so the adapter records every page you
open on that site, personal spaces included.

## Known limits

- **A space name containing ` - `.** The title entry skips back to the nearest
  ` - `, so for `Rollout tracker - Phase 1 - TEAM - Platform Commons -
  Confluence` it records `Rollout tracker - Phase 1 - TEAM`: the first part of
  the space name stays in the title.
- **Interstitial and SPA-lag titles.** While Confluence loads or asks you to
  sign in, a page URL can carry `For you - Confluence`, `Log in with Atlassian
  account`, or `Choose an account to continue - Log in with Atlassian account`.
  No entry matches, so the whole tab title is recorded. A history tab titled
  only `Confluence` is recorded as `Confluence`. The worker holds a new visit
  briefly and takes its title from a later title update (DESIGN.md §5.1), so
  the page title usually wins.
- **Editor titles** are unchecked (see Titles).
- **Short links and other origins.** `/wiki/x/` links and Confluence links on
  other origins (custom domains, Data Center) are not recorded.
- The ID template is frozen once shipped (adapter-templates.md §8.2). Widening
  a rule later goes through `STARTER_HISTORY` in
  `extension/starter-adapters.ts`.

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Atlassian Cloud site (bead `brwr-cf2`):

1. On a Confluence page with no saved rules for the site, **Capture Site**
   shows **Jira issues** and **Confluence pages**, both checked; unchecking one
   saves only the other.
2. On a site that already has the Jira starter, settings' starter gallery shows
   Jira issues as **Already configured** and offers Confluence pages once.
3. A page view, a page with no slug, a personal-space page, the editor
   (`edit-v2`, edit mode) and version history (view mode) each land on one
   artifact per page ID, with the page title (not `Version history - …`), and a
   page moved to another space keeps its notes.
4. Record the editor's real tab title and add a title entry or vector if it
   differs from the page form.
5. Overviews, databases, whiteboards, blog posts, search and `/wiki/home` stay
   "Not an artifact".

## Adding other Confluence content types

Blog posts, whiteboards, databases and space overviews are separate starters,
not new rules here:

1. Record real URLs and tab titles for the type first.
2. Give it its own type and ID namespace, for example `confluence.blogpost`
   with `confluence.blogpost:{host}/{id}`, so its IDs never collide with pages.
3. Add it to the `atlassian` offers in `starterOffers`
   (`extension/starter-adapters.ts`) with its own checkbox in the Capture Site
   review (`extension/panel.html`, `CaptureChoice` in `extension/capture-site.ts`),
   to `starter_types` in `protocol/capture-site/catalog.json`, to the starters
   fixture and vectors, and give it its own `docs/adapters/<type>.md`.
4. Its routes must stay disjoint from the page rules above (they are today:
   `blog`, `whiteboard`, `database` and `overview` are different literals from
   `pages` and `history`), and the negative vectors above become its positive
   ones.
