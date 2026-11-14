# google.doc starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/google-workspace/`, researched blind: public sources
only, no browsing history, cookies or Google account. Both adapter-template
engines verify the samples below. A live check with a real Google account is
still needed before this draft is release-certified.

History check (2026-11-09): graded against the maintainer's full Chrome
history on this family's hosts, plus the earlier held-back sample's URLs
that have since left that history: 9 real URLs (23 visits) matched, 2/3
rules confirmed, and the title right on 9/9, counting a sign-in, loading
or error title that records no title as right. Not every rule was matched
by a real visit, so this type stays a development draft.

Brauser offers this draft on the exact origin `https://docs.google.com`,
alongside Google Sheets, Slides, Forms editor and Forms responder (the
`google-workspace` family). Any one of the five can be chosen alone in
Capture Site and in the settings gallery. The draft records one artifact per
document, however it was opened:

`google.doc:docs.google.com/<document ID>`

The ID is Google's own document ID, documented as stable across renames,
`token(20,128)`: letters, digits, `-` and `_`, case-sensitive.

```toml
[[artifacts]]
type     = "google.doc"
origin   = "https://docs.google.com"
unscoped = "acknowledged"
match    = [
  { path = "/document/d/{id:token(20,128)}/{mode?:(edit|view|preview|mobilebasic)}" },
  { path = "/document/u/{_}/d/{id:token(20,128)}/{mode?:(edit|view|preview|mobilebasic)}" },
  { path = "/a/{_}/document/d/{id:token(20,128)}/{mode?:(edit|view|preview|mobilebasic)}" },
]
modes    = { edit = "edit", view = "view", preview = "view", mobilebasic = "view" }
id       = "google.doc:{host}/{id}"
title    = ["{title} - Google Docs", "{title} – Google Docs"]
generic_titles = ["Service Not Allowed", "Page Not Found", "Access Denied", "Redirecting", "Sign in - Google Accounts", "Sign in – Google Accounts", "Google Drive: Sign-in", "Login"]
refs     = [{ type = "jira.issue", find = "jira_key" }]
```

`generic_titles` lists the titles shown by Google's sign-in, access and error
pages instead of the item's own name (`adapter-templates.md` §9.4). A visit
whose title is one of them, ASCII case aside, or is empty or the bare host,
records no title, so the artifact keeps the title it already has ("Untitled"
for a new one).

One list covers both the title the template extracts and the whole-title
fallback (§9.4), so an item whose own name is `Login`, `Redirecting`, `Access
Denied`, `Page Not Found` or `Service Not Allowed` also records no title. No
recorded title had such a name when the list was set.

## What each rule covers

1. **Canonical document**, `/document/d/<id>[/edit|/view|/preview|/mobilebasic]`.
   The mode segment is optional: a bare `/document/d/<id>` (the form Google
   itself redirects to `/edit`) is recorded with no mode. `edit` is edit
   mode; `view`, `preview` and `mobilebasic` are all view mode.
2. **Multi-account form**, `/document/u/<n>/d/<id>/...`. `/u/<n>/` is the
   sign-in-order account index Google inserts for a multi-login browser; it
   is matched with `{_}` and never captured, because it depends on sign-in
   order, not identity, and cannot be a scope (research "Origin model").
3. **Legacy Google Apps form**, `/a/<domain>/document/d/<id>/...`. Google
   still serves this prefix for old links; it is matched with `{_}` for the
   same reason — most links omit it, so it cannot be a scope either.

Query parameters (`usp=sharing`, `usp=drive_open`, `tab=t.0`, `ouid`,
`resourcekey`) and the fragment (`#heading=...`) never change the ID or the
mode: Google adds and removes them freely between visits to the same
document.

## Titles

The signed-in editor's tab title is `<name> - Google Docs`. The en dash form,
`<name> – Google Docs`, is also recorded (scorecard miss 3: a sibling Drive
title used the en dash, so every suffix template in this family lists both
dashes). `mobilebasic`'s lightweight HTML view has no product suffix at all,
so neither entry matches and the whole tab title is recorded as given — which
is already the document's own name on that view, so the fallback is correct
here, not a trade-off.

A Jira key in the title (an uppercase key such as `PROJ-123`) becomes a
mention of that Jira issue, the same `refs` rule Jira and Confluence use.

## Recorded samples

Synthetic equivalents of the recorded shapes, on `docs.google.com`:

| URL | Tab title | Records |
|---|---|---|
| `/document/d/<44-char id>/edit` | `Team plan - Google Docs` | that ID, edit mode, title `Team plan` |
| `/document/u/1/d/<same id>/edit` | (untitled sample) | the same artifact, from a second signed-in account |

The blind research for this family scored 17 URLs that should match across
its five kept types, with 15 matched correctly, 0 wrongly matched, and 2
missed (both Gmail message permalinks, which this family does not cover —
see "Known limits"). The only real miss inside the family's own types was a
title suffix (a sibling Drive form used the en dash), which this draft fixes
by listing both dashes for every suffix template.

## What the draft does not record

- the Docs home list (`/document/u/0/`) and the create route
  (`/document/create`, `/document/u/0/create`);
- `/document/d/<id>/copy` (a copy action) and `/export` (a download), neither
  of which opens the document itself;
- `/document/d/<id>/template/preview` (an unverified route) and the
  published-to-web copy `/document/d/e/<published id>/pub`, which carries a
  different ID and is a different artifact;
- an ID shorter than the 20-character floor, an encoded or doubled path
  separator, a different case (`/Document/...`), or the plain `http://`
  scheme — none of these are rules Google itself serves for a real document.

## Known limits

- **Gmail is not covered.** Gmail threads are hash-routed
  (`mail.google.com/mail/u/<n>/#inbox/<thread>`), which this adapter format
  cannot express (it ignores URL fragments). The message permalink and print
  view, `mail.google.com/mail/u/<n>/?ik=…&view=pt&permmsgid=msg-f:…`, use the
  query string instead and are a possible future type (scorecard miss 2),
  not a rule in this draft.
- **Loading and interstitial titles are recorded as given.** A tab still
  loading may read `Google Docs` (or the product name) before the real
  title loads, and a Workspace-blocked or no-access page (`Service Not
  Allowed`, a sign-in or request-access page on the same URL) records that
  string as the title (scorecard "Predicted limits").
- **Drive and Calendar wait.** `drive.google.com` and `calendar.google.com`
  are just as multi-account as `docs.google.com` but are not on Brauser's
  shared-origin list, so neither can carry the `unscoped` acknowledgment
  today. They wait for that shared-origin migration rather than shipping
  without the every-account notice.
- The ID template is frozen once shipped (adapter-templates.md §8.2).
  Widening a rule later goes through `STARTER_HISTORY` in
  `extension/starter-adapters.ts`.
- A Workspace domain's own custom routes, and any new path suffix Google
  adds after the ID (for example a future Docs tabs query form), fail closed
  because no rule has a rest segment.

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Google account:

1. Open one owned and one shared document from the home list, from a Drive
   double-click, and from a "Copy link" share link. Record the final URL and
   the tab title of each.
2. Repeat signed in to a second account to confirm the `/u/1/` form lands on
   the same artifact, and with a Workspace account to check whether
   `/a/<domain>/` still appears.
3. Open the same document view-only, in `/preview`, and in `/mobilebasic`;
   confirm the recorded mode and that `mobilebasic`'s whole tab title is the
   document's bare name.
4. The home list, `/document/create`, `/copy`, `/export` and a published
   `/pub` link each stay "○ Not an artifact".
5. Record the tab title while the document is loading, as an account
   without access, and on a Workspace account where the app is disabled;
   note whether the worker's title hold hides the loading title.
6. On a Capture Site review on a document page with no saved rules, Brauser
   offers all five Google Workspace types with the every-account statement
   and the development-draft source line; saving one turns the page card's
   status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `google.doc`'s `status`
in `protocol/starters/google-workspace.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
