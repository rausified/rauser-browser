# Google Workspace blind starter: research notes

Date: 2026-11-03. This is a blind test. Everything here comes from public
sources and from this repository. I did not use a browser profile, history,
cookies or a Google account. The only live evidence is logged-out `curl` fetches
of Google's own public API sample files (the Docs, Sheets and Slides quickstart
files and the Forms API's example responder form) and of URL variants built
from them. Every ID in `vectors.json` is synthetic.

Files:

- `adapters.toml`: 8 adapters in format 1, with a comment block on each.
- `vectors.json`: 112 synthetic cases (71 positive, 41 negative).
- `validate.sh`, `toml2json.py`, `run-vectors.mjs`, `rust-check/`: throwaway
  harness that runs both engines. `doc.json` is the generated JSON document.

## Sources consulted

| Source | What it gave |
|---|---|
| https://developers.google.com/workspace/docs/api/concepts/document | `https://docs.google.com/document/d/DOCUMENT_ID/edit`; regex `/document/d/([a-zA-Z0-9-_]+)`; "Document IDs are stable, even if the document name changes." |
| https://developers.google.com/workspace/sheets/api/guides/concepts | `.../spreadsheets/d/SPREADSHEET_ID/edit?gid=SHEET_ID#gid=SHEET_ID`; the spreadsheet ID is stable across renames; `gid` is the sheet (tab) |
| https://developers.google.com/workspace/slides/api/guides/overview | `.../presentation/d/presentationId/edit`; regex `/presentation/d/([a-zA-Z0-9-_]+)`; slide object IDs can change, so they are not identity |
| https://developers.google.com/workspace/forms/api/guides | `.../forms/d/FORM_ID/edit`; formId is "letters, numbers, hyphens, or underscores"; `responderUri` is `.../forms/d/e/1FAIpQLS.../viewform`, a different ID |
| https://developers.google.com/workspace/drive/api/guides/about-files | Drive file IDs are "unique, opaque" and "stable throughout the life of the file, even if the file name changes" |
| https://developers.google.com/workspace/drive/api/guides/folder | folder MIME type; the `root` alias |
| https://knowledge.workspace.google.com/admin/drive/manage-the-link-sharing-security-update-for-files | `resourcekey=` query parameter on some Drive links; Docs, Sheets and Slides files never carry one |
| https://developers.google.com/workspace/calendar/api/v3/reference/events | event `id` is base32hex, 5 to 1024 characters; `htmlLink` is the web UI link (no example) |
| StackOverflow, via the public api.stackexchange.com search: questions 14912599, 16590289, 63689626 (`https://www.google.com/calendar/event?eid=<base64>`), 53928044 (`https://calendar.google.com/calendar/r/eventedit/<eid>`), 74836927, 55744799, 56873568 (`/calendar/r/eventedit?text=...&dates=...` create links) | calendar URL forms |
| Logged-out `curl` (Chrome user agent, no cookies) of the public sample files `docs.google.com/document/d/195j9eDD…/`, `spreadsheets/d/1BxiMVs0…/`, `presentation/d/1EAYk18W…/`, `forms/d/e/1FAIpQLSd0…/viewform`, plus variants | final URL after redirects and the HTML `<title>`, which Chrome uses as the tab title (table below) |
| This repo: `adapter-templates.md`, `docs/adapters/miro.board.md`, `confluence.page.md`, `extension/starter-adapters.ts`, `adapter-templates/src/document.rs` (`SHARED_ORIGINS`), `host/src/adapters.rs`, `scripts/adapter-templates-eval.mjs` | the language, the shared-origin rule, the validation path |

Observed logged out (HTTP status, URL after redirects, `<title>`):

| Request | Result |
|---|---|
| `/document/d/<id>` and `/document/d/<id>/` | 200, redirected to `/document/d/<id>/edit`, `Docs API Quickstart - Google Docs` |
| `/document/d/<id>/view`, `/preview`, `/edit?usp=sharing`, `/document/u/0/d/<id>/edit`, `/document/u/1/d/<id>/edit`, `/a/example.com/document/d/<id>/edit` | 200, URL kept, `… - Google Docs` |
| `/document/d/<id>/mobilebasic` | 200, `Docs API Quickstart` (no suffix) |
| `/document/d/<id>/copy` | sign-in redirect (an action) |
| `/document/d/<id>/pub` | 401 |
| `/spreadsheets/d/<id>/edit`, `/view`, `/spreadsheets/u/0/d/<id>/edit` | `Example Spreadsheet - Google Sheets` |
| `/spreadsheets/d/<id>/htmlview`, `/preview` | `Example Spreadsheet - Google Drive` |
| `/spreadsheet/ccc?key=<id>` | redirected to `/spreadsheets/d/<id>/edit` |
| `/presentation/d/<id>/edit`, `/view`, `/preview`, `/present`, `/embed` | `Baby album - Google Slides` |
| `/forms/d/e/<publishedId>/viewform` | `Famous Black Women` (bare form title) |
| `drive.google.com/open?id=<doc id>` and `docs.google.com/open?id=<doc id>` | redirected to `docs.google.com/document/d/<id>/edit?usp=drive_open` |
| `drive.google.com/file/d/<doc id>/view`, `/preview`, `/edit`, `/file/u/0/d/<id>/view` | 200, `Docs API Quickstart - Google Drive` |
| `drive.google.com/drive/u/0/folders/<fake id>` | redirected to `/drive/folders/<fake id>` (404 for the fake ID) |
| `drive.google.com/uc?id=<id>` | redirected to `drive.usercontent.google.com/download` |
| `www.google.com/calendar/event?eid=<eid>` | sign-in, with continue to `calendar.google.com/calendar/r?eid=<eid>` |
| `calendar.google.com/calendar/r/eventedit/<eid>` | sign-in, with continue to the same path |

Caveat: these are logged-out renderings of public files. A signed-in editor
uses the same `<title>` pattern for Docs, Sheets and Slides as far as all
public descriptions go, but I could not observe it.

## Which apps get a starter

| App | Decision | Why |
|---|---|---|
| Docs, Sheets, Slides | yes (`google.doc`, `google.sheet`, `google.slides`) | the primary business documents; documented, stable IDs; titles observed |
| Forms editor | yes (`google.form`) | a document a user owns or edits; same URL shape as the editors |
| Forms responder | yes, as its own type (`google.form.responder`) | a business user opens survey and registration links often. The published ID differs from formId, and nothing in the URL maps one to the other |
| Drive files | yes (`google.drive.file`) | PDFs, Office files and recordings shared as Drive links are common artifacts |
| Drive folders | yes (`google.drive.folder`) | shared project folders are a real "where the work lives" artifact |
| Calendar events | yes, LOW confidence (`google.calendar.event`) | meetings are artifacts, but URL evidence is thin, the title format is unknown and the ID depends on which calendar the event is opened from. Offer only after a live check |
| Gmail threads | no | hash-routed (`mail.google.com/mail/u/0/#inbox/<thread>`). Format 1 ignores fragments (spec §2, §13), so no rule can tell threads apart |
| Sites, Keep, Chat, Meet, Admin, Drive lists or search | no | not documents (Meet recordings land in Drive and are covered there) or out of scope |

## Origin model

- `https://docs.google.com` serves Docs, Sheets, Slides and Forms. It is on
  `SHARED_ORIGINS`, and no URL names the account, so every adapter there needs
  `unscoped = "acknowledged"`. Leaving it out is `shared_origin_unscoped`
  (checked). `/u/<n>/` is the sign-in-order account index and is matched with
  `{_}`, never captured or scoped (spec §6).
- `https://drive.google.com` and `https://calendar.google.com` are just as
  multi-account but are NOT on `SHARED_ORIGINS`. `unscoped` there is a
  validation error (`unscoped_not_shared`, checked), so these adapters carry
  neither key, and the user gets no "every Google account" warning for them.
  **I recommend adding both origins to `SHARED_ORIGINS` and the drafting
  catalog** (tenant `null`, account index `/u/<n>/` at segment 1 for Drive and
  segment 2 for Calendar). This is a product decision outside this folder.
- `/a/<domain>/` (the legacy Google Apps prefix) does name the Workspace
  domain, but only old links carry it, so it cannot be a scope: a scoped
  capture must be bound by every rule. It is matched with `{_}`.

## Per artifact type

### `google.doc`: confidence HIGH

- ID: the document ID, `token(20,128)`, case-sensitive. Google documents it as
  stable across renames, with the regex `[a-zA-Z0-9-_]+`, which is exactly the
  `token` alphabet.
- URL forms: `/document/d/<id>[/edit|/view|/preview|/mobilebasic]`,
  `/document/u/<n>/d/<id>/…` and `/a/<domain>/document/d/<id>/…`. The query
  (`usp`, `tab=t.0`, `ouid`, `resourcekey`) and the fragment (`#heading=`) are
  ignored. Modes: edit, and view for view, preview and mobilebasic.
- Title: `{title} - Google Docs`, observed. `mobilebasic` has no suffix, so
  the whole title is recorded, which is the right value.
- Excluded: home (`/document/u/0/`), `/document/create`, `/copy`, `/export`,
  `/pub`, `/template/preview`, and published `/document/d/e/<2PACX…>/pub`. That
  last one is a different artifact with a different ID; the one-letter `e`
  fails the 20-character floor.

### `google.sheet`: confidence HIGH

- ID: the spreadsheet ID. `gid` (the sheet tab) is not part of the identity.
- URL forms: as for Docs, with `edit|view|preview|htmlview`.
- Title: `{title} - Google Sheets`, plus a second entry
  `{title} - Google Drive` for `/preview` and `/htmlview`. Both were observed.
- Excluded: the legacy `/spreadsheet/ccc?key=` (a server redirect to the
  canonical form, observed), published `/spreadsheets/d/e/…/pubhtml`, and the
  home list.

### `google.slides`: confidence HIGH

- ID: the presentation ID. `#slide=id.<obj>` and `?slide=` are slide
  positions. Google says slide object IDs can change, so they are not identity.
- Modes: edit; view for view, preview, present and embed.
- Title: `{title} - Google Slides`, observed on all five forms.

### `google.form`: confidence MEDIUM (URL) / LOW-MEDIUM (title)

- ID: the formId, documented in the Forms API.
- URL forms: `/forms/d/<formId>/edit` (documented), and
  `/forms/d/<formId>/viewform`, a legacy respondent link built on formId
  (medium; widely seen in older links, not observed). `/u/<n>/` and
  `/a/<domain>/` forms are by analogy with Docs (medium).
- Title: `{title} - Google Forms`. This is inferred from the sibling editors,
  because no public editable form was available. The legacy `viewform` title
  is probably the bare form title, which the fallback records whole.

### `google.form.responder`: confidence MEDIUM-HIGH

- ID: the published ID, the API's `responderUri` (`1FAIpQLS…`, about 56
  characters). It is a separate artifact because Brauser cannot map it to the
  formId.
- URL forms: `/forms/d/e/<id>/viewform` (documented and observed), with
  `?usp=sf_link`, `?embedded=true` and `?usp=pp_url&entry.N=` (prefilled). Also
  `/formResponse` (after submitting) and `/closedform` (medium: well-known
  route names, not observed).
- No title template: the observed title is the bare form title.
- Excluded: `/prefill` and the bare `/forms/d/e/<id>`. `forms.gle/<code>`
  short links are a different origin that redirects here.

### `google.drive.file`: confidence MEDIUM-HIGH

- ID: the Drive file ID, documented as stable. `token(20,128)` admits modern
  33-character IDs and legacy `0B…` 28-character IDs.
- URL forms: `/file/d/<id>/view|preview|edit` and `/file/u/<n>/d/<id>/view`.
  All four were observed to render the previewer. The bare `/file/d/<id>` is
  low confidence. Every mode maps to view, because `/edit` was observed to show
  the same previewer.
- Title: `{title} - Google Drive`, observed. The name usually keeps its file
  extension (`report.pdf`).
- Excluded: `/open?id=` (a redirector whose target type is unknown, observed
  to forward to the editor), `/uc?id=` and `drive.usercontent.google.com`
  (downloads), and every list, search and trash route.
- Limit: a native Doc opened in the Drive previewer records as a
  `google.drive.file`, not as its `google.doc`, because the ID's `{host}`
  differs (spec §8.1), even though the ID string is identical.

### `google.drive.folder`: confidence MEDIUM

- ID: the folder ID. A shared drive's root is a folder too, with a shorter
  `0A…` ID (about 19 characters), hence `token(16,128)`. `root` (an API alias)
  falls below the floor.
- URL forms: `/drive/folders/<id>` (widely published) and
  `/drive/u/<n>/folders/<id>`, observed to redirect to the first form logged
  out. `/drive/mobile/folders/<id>` is LOW confidence.
- Title: `{title} - Google Drive`, MEDIUM. This is inferred from the observed
  file title; no public folder was fetched.

### `google.calendar.event`: confidence LOW

- ID: `eid`, unpadded base64 of `<eventId> <calendarId>`, with the calendar
  domain abbreviated. Decoding the public StackOverflow samples confirms this
  (for example `5fb6…sk forrest…@m` and a group calendar `e2pf…s8 o38t…@g`). Recurring instances are believed (unverified, from general knowledge) to encode `_<timestamp>` in
  the event part.
- URL forms: `/calendar/r?eid=`, where the API's `htmlLink` lands (observed
  through the sign-in `continue`); `/calendar/event?eid=`;
  `/calendar/r/eventedit/<eid>` (StackOverflow); and `/u/<n>/` variants (by
  analogy).
- Not known: whether the signed-in app rewrites `/r?eid=` to another route,
  and what the tab title says while an event is open. It may just be the
  calendar view title, which is why no title template is given.
- Limits: the same meeting has a different eid in each attendee's calendar. An
  eid containing `+` or `/` is dropped. A calendar ID long enough to push the
  eid past 128 characters is dropped. Treat this adapter as a candidate, not a
  starter.

## Validation

Run from the worktree root:

```sh
sh research/blind/google-workspace/validate.sh
```

It converts the TOML to JSON with Python's `tomllib`. It compiles the
extension's `extension/adapter-templates.ts` alone with the repo's `tsc` into
a scratch `.build/`, then runs `run-vectors.mjs`. It then builds `rust-check/`,
a throwaway crate in its own workspace with a path dependency on
`adapter-templates`, and runs `Document::from_value`, `overlap_report` and
`evaluate` on every vector. Result on 2026-11-03:

```
== TypeScript engine (extension/adapter-templates.ts)
validated: 8 adapters
112/112 vectors pass
== Rust engine (adapter-templates crate)
validated: 8 adapters
overlap warnings: 0
112/112 vectors pass
```

Both engines also confirmed the shared-origin behaviour: removing `unscoped`
from `google.doc` gives `shared_origin_unscoped`, and adding it to
`google.drive.file` gives `unscoped_not_shared`. I used the engines directly
rather than `brauser adapters eval`, which would have meant building the whole
host on a loaded machine. Its JSON request shape is the same `doc` object that
`doc.json` holds. I deleted the build outputs (`.build/` and
`rust-check/target`) afterwards.

## Known limits and open questions

1. Account mixing: docs.google.com records every Google account in the Chrome
   profile, and the URL cannot separate them. Drive and Calendar do the same
   with no acknowledgment, because they are missing from `SHARED_ORIGINS`.
2. Cross-app identity: one Drive file ID can appear as `google.doc` (on docs)
   and as `google.drive.file` (on drive) under two record keys. The
   `/open?id=` redirect is a server redirect, so Brauser's redirect aliases
   may link some cases, but not a direct previewer visit.
3. Tab titles for the signed-in editor, Forms editor, Drive folders and
   Calendar events were not observed. While loading, an editor may briefly
   show a generic title such as `Google Docs`; the worker's title-update hold
   should cover it, as for Miro.
4. Google sometimes A/B tests new routes (for example Docs tabs moved into
   `?tab=`). Any new path suffix after the ID, such as `/edit/…`, fails closed
   because no rule has a rest.
5. Docs `/template/preview`, Sheets `/pubhtml` on non-`e` IDs, and Slides
   `/htmlpresent` were not researched; they are left unmatched.
6. Shortcut files in Drive have their own ID that redirects to the target.
   Not researched.

## How I would verify with a real account

1. In a separate Chrome profile, open one owned and one shared Doc, Sheet,
   Slides deck and Form, each from the home list, from a Drive double-click
   and from a "Copy link" share link. Record the final URL and the tab title
   of each.
2. Repeat with a second signed-in account to confirm the `/u/1/` form, and
   with a Workspace account to check whether `/a/<domain>/` still appears.
3. For each editor, also open view-only, `/preview`, Slides present mode and
   Sheets `htmlview`, then confirm the title entries and modes.
4. Open a Forms responder link, submit it (`formResponse`) and then close the
   form (`closedform`). Record titles.
5. In Drive, open an uploaded PDF, an Office file, a folder, a folder in a
   shared drive, a shared drive root and a file with a `resourcekey`. Confirm
   the folder title and the shared-drive ID length.
6. In Calendar, open an event from the grid, from an invitation email and from
   the API `htmlLink`. Record each URL, whether `/r?eid=` is rewritten, and
   the tab title. Decide whether Calendar stays in.
7. Check that list, search, home, create and settings pages give no match, by
   running the visited URLs through `validate.sh`, or through
   `brauser adapters eval` once the host is built.
8. Settle whether drive.google.com and calendar.google.com join
   `SHARED_ORIGINS` before shipping these two origins.
