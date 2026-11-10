# google.slides starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/google-workspace/`, researched blind: public sources
only, no browsing history, cookies or Google account. Both adapter-template
engines verify the samples below. A live check with a real Google account is
still needed before this draft is release-certified.

History check (2026-11-09): graded against the maintainer's full Chrome
history on this family's hosts, plus the earlier held-back sample's URLs
that have since left that history: 3 real URLs (3 visits) matched, 1/3
rules confirmed, and the title right on 3/3, counting a sign-in, loading
or error title that records no title as right. Not every rule was matched
by a real visit, so this type stays a development draft.

Brauser offers this draft on the exact origin `https://docs.google.com`,
alongside Google Docs, Sheets, Forms editor and Forms responder (the
`google-workspace` family). Any one of the five can be chosen alone in
Capture Site and in the settings gallery. The draft records one artifact per
presentation, however it was opened:

`google.slides:docs.google.com/<presentation ID>`

The ID is Google's own presentation ID, documented as stable across
renames, `token(20,128)`, case-sensitive. A slide position (`#slide=id.<obj>`
or `?slide=...`) is never part of the identity: Google documents slide
object IDs as subject to change.

```toml
[[artifacts]]
type     = "google.slides"
origin   = "https://docs.google.com"
unscoped = "acknowledged"
match    = [
  { path = "/presentation/d/{id:token(20,128)}/{mode?:(edit|view|preview|present|embed)}" },
  { path = "/presentation/u/{_}/d/{id:token(20,128)}/{mode?:(edit|view|preview|present|embed)}" },
  { path = "/a/{_}/presentation/d/{id:token(20,128)}/{mode?:(edit|view|preview|present|embed)}" },
]
modes    = { edit = "edit", view = "view", preview = "view", present = "view", embed = "view" }
id       = "google.slides:{host}/{id}"
title    = ["{title} - Google Slides", "{title} – Google Slides"]
generic_titles = ["Service Not Allowed", "Page Not Found", "Access Denied", "Redirecting", "Sign in - Google Accounts", "Sign in – Google Accounts", "Google Drive: Sign-in", "Login"]
refs     = [{ type = "jira.issue", find = "jira_key" }]
```

`generic_titles` lists the titles shown by Google's sign-in, access and error
pages instead of the item's own name (`adapter-templates.md` §9.4). A visit
whose title is one of them, ASCII case aside, or is empty or the bare host,
records no title, so the artifact keeps the title it already has ("Untitled"
for a new one).

## What each rule covers

1. **Canonical presentation**, `/presentation/d/<id>[/edit|/view|/preview|/present|/embed]`.
   The mode segment is optional: a bare `/presentation/d/<id>` is recorded
   with no mode. `edit` is edit mode; `view`, `preview`, `present`
   (slideshow mode) and `embed` are all view mode.
2. **Multi-account form**, `/presentation/u/<n>/d/<id>/...`, `{_}` and never
   captured (research "Origin model").
3. **Legacy Google Apps form**, `/a/<domain>/presentation/d/<id>/...`,
   matched with `{_}` for the same reason.

`#slide=id.<obj>` fragments and `?slide=...` queries never change the ID or
the mode, whether the slide value is a position token or an object ID.

## Titles

The tab title reads `<name> - Google Slides` on every mode observed: `edit`,
`view`, `preview`, `present` and `embed`. The en dash form, `<name> –
Google Slides`, is also recorded (scorecard miss 3: a sibling Drive title
used the en dash, so every suffix template in this family lists both
dashes).

## Recorded samples

Synthetic equivalent of the recorded shape, on `docs.google.com`:

| URL | Tab title | Records |
|---|---|---|
| `/presentation/d/<44-char id>/edit` | `Kickoff deck - Google Slides` | that ID, edit mode, title `Kickoff deck` |

The blind research for this family scored 17 URLs that should match across
its five kept types, with 15 matched correctly, 0 wrongly matched, and 2
missed (both Gmail message permalinks, outside this family — see "Known
limits"). The only real miss inside the family's own types was a title
suffix (a sibling Drive form used the en dash), which this draft fixes by
listing both dashes for every suffix template.

## What the draft does not record

- the Slides home list (`/presentation/u/0/`);
- `/presentation/d/<id>/export` (a download, not an open);
- any other route below a presentation path that the rule's lack of a rest
  segment excludes.

## Known limits

- **Gmail is not covered.** See `docs/adapters/google.doc.md` "Known
  limits": Gmail is fragment-routed and the message permalink is a possible
  future type, not a rule here.
- **Loading and interstitial titles are recorded as given.** See
  `docs/adapters/google.doc.md` "Known limits": a tab still loading may
  read the bare product name, and a Workspace-blocked or no-access
  interstitial records that string as the title.
- **Drive and Calendar wait** for the shared-origin migration, as for
  `google.doc`.
- The ID template is frozen once shipped (adapter-templates.md §8.2).
  Widening a rule later goes through `STARTER_HISTORY` in
  `extension/starter-adapters.ts`.

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Google account:

1. Open one owned and one shared presentation from the home list, from a
   Drive double-click, and from a "Copy link" share link. Record the final
   URL and the tab title of each.
2. Repeat signed in to a second account to confirm the `/u/1/` form lands on
   the same artifact.
3. Open the same presentation view-only, in `/preview`, in `/present`
   (slideshow) and in `/embed`; confirm each records view mode and the same
   title suffix, and that a slide position (`#slide=` or `?slide=`) does not
   change the recorded artifact.
4. The home list and `/export` each stay "○ Not an artifact".
5. Record the tab title while the presentation is loading, as an account
   without access, and on a Workspace account where the app is disabled;
   note whether the worker's title hold hides the loading title.
6. On a Capture Site review on a presentation page with no saved rules,
   Brauser offers all five Google Workspace types with the every-account
   statement and the development-draft source line; saving one turns the
   page card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `google.slides`'s
`status` in `protocol/starters/google-workspace.json` to `{ "state":
"certified", "method": "live", "checked_on": "<date>", "checked_by": "John"
}`, and run `npm run gen:starters` (`protocol/starters/README.md`). The
Capture Site source line follows the status.
