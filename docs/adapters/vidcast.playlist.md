# vidcast.playlist starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/vidcast/`, researched blind: public sources only, no
browsing history, cookies or a Vidcast account.

History check (2026-11-09): graded against real browsing history: no
real visits of this type, so 0/3 rules confirmed. Not every rule and
title was confirmed, so this type stays a development draft.

Brauser offers this draft on the exact origin `https://app.vidcast.io`,
alongside Vidcast videos and Vidcast pages (the `vidcast` family). Any
combination can be chosen in Capture Site and in the settings gallery. The
draft records one artifact per playlist, however it was opened:

`vidcast.playlist:app.vidcast.io/<playlist share ID>`

The ID is the playlist's share ID, a UUID, read from public `playlistShareId`
values and a successful render of a sample playlist page.

```toml
[[artifacts]]
type   = "vidcast.playlist"
origin = "https://app.vidcast.io"
match  = [
  { path = "/playlists/{playlist:uuid}", mode = "view" },
  { path = "/playlists/embed/{playlist:uuid}", mode = "view" },
  { path = "/collections/share/{playlist:uuid}", mode = "view" },
]
id     = "vidcast.playlist:{host}/{playlist}"
title  = "Vidcast - Playlist - {title}"
```

## What each rule covers

1. **Playlist page**, `/playlists/<playlist share id>`, `mode = "view"`. The
   help center describes sharing a playlist by its Share button or by
   copying the browser URL directly. Query parameters the route table allows
   (`accessRequestId`, `pageShareId`, `playerMaxWidth`, `hideWatermark`)
   never change the ID.
2. **Embed link opened top-level**, `/playlists/embed/<playlist share id>`,
   `mode = "view"`. The server's `og:url` for this route points back to
   `/playlists/<same id>`.
3. **Legacy "collections" share link**, `/collections/share/<share id>`,
   `mode = "view"`. The web app client-side redirects this route to
   `/playlists/<same id>`, and the server's `og:url` for it agrees, so both
   URLs are recorded under one ID.

The `/playlists` list (no ID) and `/collections` (the legacy list, which
redirects to `/playlists`) match nothing: every rule needs a UUID. A video
opened inside a playlist, `/share/<video>?playlistShareId=<playlist>`, is
recorded as the video (`vidcast.video`), not the playlist; the query key
names the playlist but does not change which artifact the visit is.

## Titles

One entry: `Vidcast - Playlist - {title}` strips the app's title-component
prefix for a playlist page. Only this exact entry is listed, so a title that
is not a playlist name — bare `Vidcast` (the embed page or the legacy
redirect before it resolves), `Vidcast - Request Access`, or `Vidcast - Not
Found` — matches no entry and is recorded whole rather than mistaken for a
playlist's name.

## Recorded samples

A synthetic equivalent of the recorded shape, on `app.vidcast.io`:

| URL | Tab title | Records |
|---|---|---|
| `/playlists/<36-char share ID>` | `Vidcast - Playlist - Onboarding` | that share ID, view mode, title `Onboarding` |

The blind research scored 58 synthetic vectors across this family's three
types (10 `vidcast.playlist` positives among them), all correctly classified
by the research's own draft and re-verified against this family's own
fixture through both adapter-template engines. Graded later against 11 real,
held-back Vidcast browsing visits (`docs/research/blind/SCORECARD.md`), the
research matched all 11 correctly, with a stable ID across URL forms and the
right title template — Vidcast's site-wide result, not broken out
per-type, since none of those 11 real visits happened to be a playlist.

## What the draft does not record

- the playlists list (`/playlists` with no ID) and the legacy collections
  list (`/collections`), neither of which names a playlist;
- a video opened inside a playlist (`/share/<video>?playlistShareId=…`),
  recorded as the video instead;
- a mobile deep-link helper in the app's bundle maps
  `/collections/share/<id>` to a video-shaped deep link, conflicting with
  the web router's redirect to `/playlists/<id>`; this draft follows the web
  router and the server's `og:url`, what a browser actually sees;
- a playlist share ID that is not a UUID, an empty path segment, or an
  encoded slash inside a segment, the same rejections `vidcast.video` makes.

## Known limits

- **Access and error states are recorded as artifacts.** `Vidcast - Request
  Access`, `Vidcast - Not Found`, and the bare `Vidcast` embed title are all
  recorded whole, under this playlist's ID, because the URL is still a valid
  playlist share URL even when its title says otherwise.
- **Real old `/collections/share/<id>` links are unconfirmed.** The research
  is confident the redirect exists in the bundle, but not confident real
  links of that legacy form still circulate.
- Every Vidcast account signed in to the Chrome profile is recorded: no URL
  names the account or organization, so a playlist's own share settings,
  not this starter, control who can see it.
- The ID template is frozen once shipped (adapter-templates.md §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Vidcast account:

1. Create a playlist. Check `/playlists/<id>`, its embed link, and the tab
   title.
2. Open an old `/collections/share/<id>` link if one can still be found, and
   confirm it lands on `/playlists/<same id>`.
3. Watch a video from inside a playlist and confirm the recorded artifact is
   the video, not the playlist.
4. Open a playlist as a user without access, and an unknown or deleted
   share ID. Record both titles.
5. Rename the playlist and confirm the tab title and the recorded title
   follow, while the share ID stays the same.
6. On a Capture Site review on a playlist page with no saved rules, Brauser
   offers all three Vidcast types with the every-account statement in each
   type's own description and the development-draft source line; saving
   one turns the page card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `vidcast.playlist`'s
`status` in `protocol/starters/vidcast.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
