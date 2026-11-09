# google.drivefolder starter draft

Status: development draft. The URL and tab-title shapes come from real
browsing history (7 folder URLs, 18 visits, on `https://drive.google.com`).
Both adapter-template engines verify the samples below. A live check in the
Capture Site review is still needed before this draft is release-certified.

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
```

Why a separate family, not a second origin on the Google Workspace family:
that family is `unscoped` on `docs.google.com`, and the engine allows
`unscoped` only on its hard-coded exact shared origins, which do not include
`drive.google.com`. A multi-origin family would also offer the five Docs,
Sheets, Slides and Forms types on Drive, where none of their paths exist. A
plain exact-origin family with no scope, the shape Miro uses for its one
origin shared by every account, needs no engine change.

## What each rule covers

1. **Folder view**, `/drive/folders/<id>`: a folder opened from Drive's own
   list, a shared-drive folder, or a share link. Share parameters such as
   `?usp=sharing` or `?usp=sharing_eip&ts=…`, a trailing slash and the
   fragment are ignored.
2. **Folder view in another signed-in account**, `/drive/u/<n>/folders/<id>`.
   The account index `<n>` is matched but not used, so the same folder
   opened from two account slots stays one artifact.

## Titles

Drive titles folder pages `<folder name> - Google Drive`, and some pages use
an en dash (`<folder name> – Google Drive`); both forms were seen on the same
folder. The two entries take the folder name from either.

When the signed-in account cannot open the folder, Drive shows an error or
sign-in page on the folder's URL (seen titles: `Access denied`,
`Service Not Allowed`, `Login`, `Google Drive: Sign-in`). No entry matches,
so Brauser records the whole tab title for that visit.

## Recorded samples

Synthetic equivalents of the recorded shapes (shared vectors
`eval.google.drivefolder.*` in `protocol/starters/google-drive.json`):

| URL | Tab title | Records |
|---|---|---|
| `/drive/folders/1AbC…345?usp=sharing` | `Team Templates - Google Drive` | the folder, `Team Templates` |
| `/drive/u/1/folders/1AbC…345` | `Team Templates – Google Drive` | the same folder, `Team Templates` |
| `/drive/folders/1AbC…345` | `Access denied` | the folder, whole tab title |

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
- An error page on a folder's URL is recorded as a visit to that folder with
  the error page's title.
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
