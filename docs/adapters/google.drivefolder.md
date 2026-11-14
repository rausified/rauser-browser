# google.drivefolder starter draft

Status: development draft. The URL and tab-title shapes come from real
browsing history (7 folder URLs, 18 visits, on `https://drive.google.com`).
Both adapter-template engines verify the samples below. A live check in the
Capture Site review is still needed before this draft is release-certified.

History check (2026-11-10): graded against a hand-built answer key from
the maintainer's Chrome history in all five Chrome profiles on
`drive.google.com`: 7 real folder URLs (12 history rows across profiles,
19 visits) matched with the right type and ID, 0 real folder URLs missed,
1/2 rules confirmed, and the title right on 12/12, counting a sign-in or
error title, or a never-loaded tab's URL placeholder, that records no
title as right. None of the 10 other real Drive URLs (home, account roots,
sign-in redirects and the PDF viewer's sign-in hop) matched. The
`/drive/u/<n>/folders/<id>` rule matched no real visit, so this type stays
a development draft.

Brauser offers this draft on the exact origin `https://drive.google.com`, as
its own `google-drive` family, beside the Google Workspace family on
`https://docs.google.com`. It records one artifact per folder:

`google.drivefolder:drive.google.com/<folder ID>`

A folder ID is Drive's permanent item ID: 33 characters from token's
alphabet in current folders (`1AbCdEfGhIjKlMnOpQrStUvWxYz012345`,
illustrative), shorter in older ones. It does not change when the folder is
renamed, moved or shared, and it is case-sensitive, so it is recorded as
matched. The bounds `token(20,128)` are the ones the Google Workspace types
use for document IDs.

```toml
[[artifacts]]
type   = "google.drivefolder"
origin = "https://drive.google.com"
match  = [
  { path = "/drive/folders/{id:token(20,128)}" },
  { path = "/drive/u/{_}/folders/{id:token(20,128)}" },
]
id     = "google.drivefolder:{host}/{id}"
title  = ["{title} - Google Drive", "{title} – Google Drive"]
generic_titles = ["Service Not Allowed", "Page Not Found", "Access Denied", "Redirecting", "Sign in - Google Accounts", "Sign in – Google Accounts", "Google Drive: Sign-in", "Login"]
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

Why a separate family, not a second origin on the Google Workspace family:
that family is `unscoped` on `docs.google.com`, and the engine allows
`unscoped` only on its hard-coded exact shared origins, which do not include
`drive.google.com`. A multi-origin family would also offer the five Docs,
Sheets, Slides and Forms types on Drive, where none of their paths exist. A
plain exact-origin family with no scope, the shape Miro uses for its one
origin shared by every account, needs no engine change.

**Scope.** drive.google.com serves every Google account, work and personal,
from one origin, and a folder URL names neither the account nor the
organization (the `/drive/u/<n>/` slot is a per-profile index, not an
account value). The adapter therefore records folders from every Google
account signed in to the Chrome profile, and the type's description, which
the Capture Site review shows, says so. This follows the Miro decision of
2026-11-01 (`docs/adapters/miro.board.md` "Scope"): drive.google.com is not on
the shared-origin list (`adapter-templates.md` §6), so the adapter needs
neither `scope` nor `unscoped`, and the family shows no separate `unscoped`
notice. Listing it as shared would change both engines and the host catalog
and would require an acknowledgment on every existing Drive adapter, for a
disclosure the description already makes. A folder's own sharing settings
control who can see it. Use a separate Chrome profile for personal Drive work
if you do not want it recorded.

## What each rule covers

1. **Folder view**, `/drive/folders/<id>`: a folder opened from Drive's own
   list, a shared-drive folder, or a share link. Share parameters such as
   `?usp=sharing` or `?usp=sharing_eip&ts=…`, a trailing slash and the
   fragment are ignored.
2. **Folder view in another signed-in account**, `/drive/u/<n>/folders/<id>`.
   The account index `<n>` is matched but not used, so the same folder
   opened from two account slots stays one artifact.
   No real visit has used this route yet (History check above); it is kept
   because Drive opens every page of a second signed-in account under
   `/drive/u/<n>/`, and live-check step 2 confirms it.

## Titles

Drive titles folder pages `<folder name> - Google Drive`, and some pages use
an en dash (`<folder name> – Google Drive`); both forms were seen on the same
folder. The two entries take the folder name from either.

When the signed-in account cannot open the folder, Drive shows an error or
sign-in page on the folder's URL (seen titles: `Access denied`,
`Service Not Allowed`, `Login`, `Google Drive: Sign-in`). No entry matches,
and each is in `generic_titles`, so that visit records no title and the
folder keeps its own name.

A folder tab Chrome never finished loading (seen on a synced row with no
visit) keeps its full URL, `https://drive.google.com/drive/folders/<id>`, as
the title. That is the browser's no-title placeholder (`adapter-templates.md`
§9.4), so it records no title either.

## Recorded samples

Synthetic equivalents of the recorded shapes (shared vectors
`eval.google.drivefolder.*` in `protocol/starters/google-drive.json`):

| URL | Tab title | Records |
|---|---|---|
| `/drive/folders/1AbC…345?usp=sharing` | `Team Templates - Google Drive` | the folder, `Team Templates` |
| `/drive/u/1/folders/1AbC…345` | `Team Templates – Google Drive` | the same folder, `Team Templates` |
| `/drive/folders/1AbC…345` | `Access denied` | the folder, no title (generic) |
| `/drive/folders/1AbC…345` | `Google Drive: Sign-in` | the folder, no title (generic) |
| `/drive/folders/1AbC…345` | `https://drive.google.com/drive/folders/1AbC…345` | the folder, no title (URL placeholder) |
| `/viewerng/noncesigner?…` | (sign-in hop) | nothing |
| `/drive/u/0/?pli=1` | `Google Drive` | nothing |

## What the draft does not record

- Drive home and lists: `/drive/my-drive`, `/drive/u/<n>/home`,
  `/drive/shared-with-me`, `/drive/recent`, search, and the bare origin.
- Files opened in Drive's viewer, `/file/d/<id>/view`. None appeared in the
  history this draft was built from, and a Google Docs file opened there
  would get a second artifact beside its `docs.google.com` one.
- The PDF viewer's sign-in hop, `/viewerng/noncesigner?…`.
- Any sub-path after the folder ID.

## Known limits

- The type records folders from every Google account signed in to this
  Chrome profile; there is no account value in the URL to scope by.
- `{_}` accepts any account-slot segment, not only digits.
- An error or sign-in page on a folder's URL is recorded as a visit to that
  folder with no title, because its title is in `generic_titles` (above). A
  folder tab Chrome never finished loading, whose title is the raw URL, also
  records no title, but through the browser-placeholder rule
  (`adapter-templates.md` §9.4), not the list. A new interstitial title Drive
  starts using would be recorded as the folder's name until it is added to
  `generic_titles`.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Google account:

1. Capture Site: open a folder from My Drive, a shared-drive folder and a
   folder share link; record each URL and tab title. Is the title always
   `<name> - Google Drive` or `<name> – Google Drive`?
2. With two Google accounts signed in, open the same shared folder from
   each; confirm both visits record one ID.
3. Rename a folder; confirm the next visit keeps the same ID and records the
   new name.
4. Check whether Drive ever opens a folder on another path (for example
   `/drive/mobile/folders/<id>` or `/embeddedfolderview?id=<id>`) and decide
   whether it should count.
5. On a Capture Site review on a folder page with no saved rules, confirm
   Brauser offers Google Drive folders with its own statement and the
   development-draft source line; confirm saving it turns the page card's
   status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `google.drivefolder`'s
`status` in `protocol/starters/google-drive.json` to `{ "state":
"certified", "method": "live", "checked_on": "<date>", "checked_by": "John"
}`, and run `npm run gen:starters` (`protocol/starters/README.md`). The
Capture Site source line follows the status.
