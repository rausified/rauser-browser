# box.folder starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/box/`, researched blind: public sources only, no
browsing history, cookies or a Box account.

History check (2026-11-09): graded against the maintainer's full Chrome
history on this family's hosts, plus the earlier held-back sample's URLs
that have since left that history: 2 real URLs (3 visits) matched, 1/3
rules confirmed, and the title right on 2/2, counting a sign-in, loading
or error title that records no title as right. Not every rule was matched
by a real visit, so this type stays a development draft.

Brauser offers this draft on every Box web-app host: personal
`https://app.box.com`, `<company>.app.box.com` and `<company>.ent.box.com`
(the `box` family's multi-origin `origin`; `box.file`'s page lists its
members), alongside Box files and Box shared links. Any combination can be
chosen in Capture Site and in the settings gallery. The
draft records one artifact per folder, however it was opened:

`box.folder:<host>/<folder ID>`, for example
`box.folder:acme.app.box.com/123456789`

The ID is Box's permanent folder ID (API `folders/<id>`, typed `d_<id>`).

```toml
[[artifacts]]
type   = "box.folder"
origin = "https://acme.app.box.com"
match  = [
  { path = "/folder/{id:int(2,19)}" },
  { path = "/s/{_}/folder/{id:int(2,19)}" },
  { path = "/v/{_}/folder/{id:int(2,19)}" },
]
id     = "box.folder:{host}/{id}"
title  = "{title} | Powered by Box"
generic_titles = ["Box | Login"]
```

`generic_titles` lists the titles shown by Box's sign-in page instead of the
item's own name (`adapter-templates.md` §9.4). A visit whose title is one of
them, ASCII case aside, or is empty or the bare host, records no title, so the
artifact keeps the title it already has ("Untitled" for a new one).

Known limit: while a page loads, Box can title the tab with the enterprise's
name, `Acme | Powered by Box`. The template extracts `Acme`, which no list
can tell apart from an item that shares the enterprise's name, so that
loading title is still recorded until the real one replaces it.

## What each rule covers

1. **Folder view**, `/folder/<id>`, logged in. `int(2,19)` leaves out
   `/folder/0`, the "All Files" root: it lists
   everything rather than being a folder anyone works in, and real folder
   IDs run 9–12+ digits.
2. **Subfolder of a shared folder**, `/s/<shared name>/folder/<id>`, reached
   by browsing down from a shared-link root. The title may carry the share
   root's name rather than the subfolder's (Known limits).
3. **Subfolder of a custom-URL (vanity) folder**, `/v/<vanity name>/folder/
   <id>`, the vanity-URL equivalent of rule 2.

Every rule uses the same `int(2,19)` bound: one capture keeps one type
across an adapter's rules (`capture_type_conflict`). No rule has a rest
segment, so `/folder/<id>/search` (searching inside a folder) never matches.
Query parameters (`?tc=…`, `sortColumn`, `sortDirection`, ...) never change
the ID.

## Titles

One entry: `{title} | Powered by Box` strips the app's own title-component
suffix, leaving the folder name.

While the page is untitled, an enterprise user's tab instead reads
`{enterprise name} | Powered by Box` (same limit as `box.file`): this entry
still matches, so the enterprise name is recorded as if it were the
folder's name. A dead or bad link's title, `Box | Simple Online
Collaboration`, matches no entry and is kept whole.

## Recorded samples

A synthetic equivalent of the recorded shape, on `acme.app.box.com`:

| URL | Tab title | Records |
|---|---|---|
| `/folder/<12-digit ID>` | `Press materials \| Powered by Box` | that folder ID, title `Press materials` |

The blind research scored 51 synthetic vectors across this family's three
types (8 `box.folder` positives among them), all correctly classified by the
research's own draft and re-verified against this family's own fixture
through both adapter-template engines. Graded later against 5 real,
held-back Box browsing visits (`docs/research/blind/SCORECARD.md`), the
research matched all 5 correctly — Box's site-wide result, not broken out
per-type, since none of those 5 real visits happened to be a folder.

## What the draft does not record

- `/folder/0`, the "All Files" root, deliberately excluded by `int(2,19)`
  since it lists everything rather than being a folder anyone works in;
- `/folder/<id>/search`, searching inside a folder, since no rule carries a
  rest segment;
- Box's lists and chrome: `/recents`, `/trash`, `/files/favorites`, the
  Admin Console, and the login host `account.box.com` — none of them is a
  folder.

## Known limits

- **The same folder on two Box hosts is two artifacts**, since the ID
  carries `{host}`; `box.file`'s page states this in full.
- **Shared-subfolder titles may carry the share root's name**, not the
  subfolder's own name: the one rendered sample observed showed the share
  root's title on a subfolder page.
- **The enterprise-name loading title is recorded as the folder's name**,
  the same limit `box.file` carries, predicted and confirmed by the
  scorecard.
- Localized Box UIs may translate "Powered by Box"; not checked.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Box account:

1. Open a folder from All Files; record the URL and tab title
   (`/folder/<id>`, `<name> | Powered by Box`?).
2. Open a subfolder inside a shared link while logged in, and check whether
   its title is the subfolder's own name or the share root's.
3. Confirm `/folder/0` (All Files) is never offered as an artifact.
4. Watch the title while the enterprise name flashes before the real title
   loads, and in a non-English locale.
5. Rename a folder and confirm the tab title follows while the folder ID
   does not change.
6. On a Capture Site review on a folder page with no saved rules, confirm
   Brauser offers all three Box types — files, folders, shared links — each
   with its own Box-account statement and the development-draft source
   line; confirm saving one turns the page card's status to "● Artifact
   recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `box.folder`'s
`status` in `protocol/starters/box.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
