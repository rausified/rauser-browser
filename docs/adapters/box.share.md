# box.share starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/box/`, researched blind: public sources only, no
browsing history, cookies or a Box account.

Brauser offers this draft on an enterprise's custom subdomain,
`<company>.app.box.com` (the `box` family's strict subdomain origin,
`exclude: ["www", "cloud"]`), alongside Box files and Box folders. Any
combination can be chosen in Capture Site and in the settings gallery. The
draft records one artifact per shared link or custom ("vanity") URL root,
however it was opened:

`box.share:<company>.app.box.com/s/<shared name>` or
`box.share:<company>.app.box.com/v/<vanity name>`

The URL alone does not say whether a shared link names a file or a folder,
and a rendered shared page carries no `<meta>` with the item's ID (only
`og:title` and an `og:image` naming a *file version* ID), so this type is
keyed by link kind and name rather than by the file or folder ID
underneath. Both names are case-insensitive on Box, so they are lowered.
The type is named `box.share`, not `box.shared_link`: an ID scheme may not
contain `_` (`adapter-templates.md` §8.1).

```toml
[[artifacts]]
type   = "box.share"
origin = "https://acme.app.box.com"
match  = [{ path = "/{kind:(s|v)}/{link:token(1,100)|lower}" }]
id     = "box.share:{host}/{kind}/{link}"
title  = "{title} | Powered by Box"
```

## What each rule covers

One rule, a captured `(s|v)` alternation: `/s/<shared name>` (a Box share
link) or `/v/<vanity name>` (a custom URL). A pair of separate `s`/`v` rules
with different bounds fails `capture_type_conflict`, so one rule with
`token(1,100)` is used instead; it also admits implausibly short `/s/` names,
which Box answers with its own error page rather than a 404. Query
parameters (`?page=`, `sortColumn`, `sortDirection`, `accessRequestId`, ...)
never change the ID, and the legacy `/shared/<name>` form is a server 301 to
`/s/<name>`, so a visit there settles on this rule under the redirected URL.

## Titles

One entry: `{title} | Powered by Box` strips the app's own title-component
suffix, leaving the shared file's or folder's name (with its extension, for
a file).

A dead or bad link's title, `Box | Simple Online Collaboration`, matches no
entry and is kept whole.

## Recorded samples

A synthetic equivalent of the recorded shape, on `acme.app.box.com`:

| URL | Tab title | Records |
|---|---|---|
| `/s/<32-char share name>` | `Team docs \| Powered by Box` | that shared link, title `Team docs` |

The blind research scored 51 synthetic vectors across this family's three
types (9 `box.share` positives among them), all correctly classified by the
research's own draft and re-verified against this family's own fixture
through both adapter-template engines. Graded later against 5 real,
held-back Box browsing visits (`docs/research/blind/SCORECARD.md`), the
research matched all 5 correctly — Box's site-wide result, not broken out
per-type, since none of those 5 real visits happened to be a bare shared
link. The scorecard separately confirms
`<company>.box.com/s/…` redirects to `<company>.app.box.com/s/…`, so only
the latter needs a rule, as researched.

## What the draft does not record

- the legacy shared-tree form `/s/<name>/1/<folder>/<file>/1`, which returns
  404 today even where `/s/<name>` itself still works;
- `/shared/static/<name>.<ext>`, a direct download, not a page;
- an unknown sub-route under a shared link (`/s/<name>/preview` and the
  like), since the rule has no rest segment;
- a vanity name containing a character outside `token`'s alphabet (for
  example a literal `.`), on the assumption that Box's vanity names stay
  within it;
- the uppercase route `/S/<name>`, a 404 on Box: paths are case-sensitive
  even though shared and vanity *names* are not.

## Known limits

- **This draft does not cover personal `https://app.box.com` or
  `<company>.ent.box.com`** (Open decision 1), the same gap `box.file`'s
  page states in full.
- **This starter is also offered, unintentionally, on Box's upload hosts**
  (`fupload-<n>.app.box.com`), the same `exclude`-is-whole-label-only limit
  `box.file`'s page states in full.
- **A shared name dies when its link is disabled or re-created; a vanity
  name can be edited or reused by the owner.** This ID names the link, not
  the item behind it.
- **A logged-in collaborator opening the same item by `/file/<id>` or
  `/folder/<id>` gets a different artifact** (`box.file` or `box.folder`).
  Only a server redirect from `/s/<name>` to the item's own route — if Box
  ever adds one for a signed-in collaborator — would let the two be joined
  as aliases; none was observed.
- Localized Box UIs may translate "Powered by Box"; not checked.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Box account:

1. Create a shared link to a file and to a folder; record both `/s/<name>`
   URLs and tab titles.
2. Create a custom ("vanity") URL for a folder; record the `/v/<name>` URL
   and tab title.
3. As a collaborator, open a `/s/<name>` link to a file you can access:
   does Box redirect to `/file/<id>`? If so, a server-redirect alias could
   join `box.share` to `box.file`.
4. Disable a shared link, then re-create it; confirm whether the name
   changes.
5. Open a shared link as an unauthorized user, and a deleted or unknown
   share name. Record both titles and decide whether those visits should
   keep being recorded.

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `box.share`'s
`status` in `protocol/starters/box.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
