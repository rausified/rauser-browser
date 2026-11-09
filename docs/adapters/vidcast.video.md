# vidcast.video starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/vidcast/`, researched blind: public sources only, no
browsing history, cookies or a Vidcast account.

History check (2026-11-09): graded against a held-back sample of real
browsing history (real URLs from eight researched sites): 11 real URLs
(26 visits) matched, 2/3 rules confirmed, the recorded title right on
8/11. Not every rule and title was confirmed, so this type stays a
development draft.

Brauser offers this draft on the exact origin `https://app.vidcast.io`,
alongside Vidcast playlists and Vidcast pages (the `vidcast` family). Any
combination can be chosen in Capture Site and in the settings gallery. The
draft records one artifact per video, however it was opened or edited:

`vidcast.video:app.vidcast.io/<video share ID>`

The ID is the video's public share ID, a UUID. It is not the internal video
ID the `/video/<id>` route looks up before client-side navigating to
`/share/<share id>` (excluded below): the two IDs differ, so recording the
internal ID would split one video into two artifacts, and a client-side
navigation is not a server redirect the host can learn an alias from. The
`uuid` capture lowercases, which is safe: an upper-cased share UUID resolves
to the same video server-side.

```toml
[[artifacts]]
type   = "vidcast.video"
origin = "https://app.vidcast.io"
match  = [
  { path = "/share/{video:uuid}", mode = "view" },
  { path = "/share/embed/{video:uuid}", mode = "view" },
  { path = "/share/edit/{video:uuid}", mode = "edit" },
]
id     = "vidcast.video:{host}/{video}"
title  = "Vidcast - {title}"
```

## What each rule covers

1. **Share link, no mode recorded as a route but tagged `view`**,
   `/share/<share id>`. This is the link Vidcast's own Share dialog and every
   help article hand out. Query parameters the route table allows (`t`,
   `accessRequestId`, `playlistShareId`, `pageShareId`, `autoplay`,
   `highlightId`, `playerMode`, `clip`, `playerMaxWidth`, `hideWatermark`)
   never change the ID: a video watched inside a playlist
   (`?playlistShareId=<id>`) is still that video, not the playlist.
2. **Embed link opened top-level**, `/share/embed/<share id>`, `mode =
   "view"`. The server's `og:url` for this route points back to
   `/share/<same id>`, so it is the same video. The embed page is normally
   loaded in an iframe on another site, so a top-level visit is rare, but the
   ID is the same when it happens.
3. **Editor**, `/share/edit/<share id>`, `mode = "edit"`. Reached from the
   Edit button on the playback page. Query (`?tab=`) never changes the ID.

No rule has a rest segment, so `/share/<id>/<anything>` matches nothing, and
`/share/edit-script/<id>/<upload id>` (the AI script editor for an upload)
also matches nothing: `edit-script` is not the literal `edit`, and the extra
segment has no rule either way.

## Titles

One entry: `Vidcast - {title}` strips the app's own title-component prefix
(`` `Vidcast - ${value}` `` for a non-empty value, bare `Vidcast` otherwise).
The prefix is matched once from the left, so a video whose own name starts
with `Vidcast - ` keeps the inner occurrence (a name `"Vidcast - weekly
update"` is recorded as `Vidcast - weekly update`, not `weekly update`).

While a video is loading, before the server HTML, or on the embed page, the
tab title is bare `Vidcast`: no entry matches (there is no `Vidcast - ` to
anchor on), so the whole title `Vidcast` is recorded, the same fallback Miro
and Figma use.

## Recorded samples

Synthetic equivalents of the recorded shapes, on `app.vidcast.io`:

| URL | Tab title | Records |
|---|---|---|
| `/share/<36-char share ID>` | `Vidcast - Q3 roadmap walkthrough` | that share ID, view mode, title `Q3 roadmap walkthrough` |
| `/share/edit/<same share ID>` | `Vidcast - Video Edit` | the same artifact, edit mode, title `Video Edit` (Known limits, below) |

The blind research scored 58 synthetic vectors across this family's three
types (14 `vidcast.video` positives among them), all correctly classified by
the research's own draft and re-verified against this family's own fixture
through both adapter-template engines. Graded later against 11 real,
held-back Vidcast browsing visits (`docs/research/blind/SCORECARD.md`), the
research matched all 11 correctly, with a stable ID across URL forms and the
right title template (`Vidcast - {title}`) — the only miss recorded for the
site was the known loading/editor title limit below, which every researcher
named up front.

`samples_shape` is `false`: this type's two samples share the `/share/`
route word at two different depths (`/share/<id>` and
`/share/edit/<id>`). `cargo test --locked -p adapter-templates --test
structural starter_recorded_urls_draft_to_matching_adapters_one_per_kind`
names both, because the generic structural drafter, given only these two
examples, drafts a rule shape that matches neither concrete URL. Dropping
these samples from the shared shapes pool (by setting `samples_shape` to
`false`) removes the ambiguity without changing what the real adapter
matches; the `match` rules above, not the drafter, are what Brauser actually
ships. Both samples stay in the family file (the title check still reads
them), and the engine's own vectors above independently confirm every rule.

## What the draft does not record

- the app's lists and chrome: the root/home feed, `/library`, `/playlists`
  (the list, not a playlist), `/pages` (the list), `/collections` (the legacy
  list), `/search`, `/settings`, `/sign-in`, `/author/<id>`, `/analytics`,
  `/event/<id>` and the like — none of them is a share link;
- `/video/<internal id>`, the internal-ID redirect excluded above;
- `/share/edit-script/<id>/<upload id>`, the AI script editor;
- `/webex-stream?recordingUUID=…&siteUrl=…`, a Webex meeting recording
  streamed in Vidcast — its identity is a Webex recording, a different
  artifact;
- a share ID that is not a UUID, an empty path segment, an encoded slash
  inside a segment, a route reached by a different HTTP scheme, or a
  differently-cased route word (`/Share/<id>`), since the router's own
  routes are lowercase and the match is case-sensitive on the path.

## Known limits

- **The editor title is fixed.** Every edit visit's tab title is `Vidcast -
  Video Edit`, recorded as the useless title `Video Edit` (Review Focus item
  5). If a later visit's share-page title would otherwise be kept as the
  artifact's best title, an edit visit can overwrite it with `Video Edit`;
  the engine has no per-rule title policy, so this is a host-level decision,
  not something this adapter can fix alone.
- **Error and access states are recorded as artifacts.** A no-access viewer
  sees `Vidcast - Request Access`, a logged-out viewer of a restricted video
  sees `Vidcast - Sign In`, and a dead link shows `Vidcast - Not Found`; each
  is a valid share URL, so each is recorded, with that string as its title.
- **The embed page's title is bare `Vidcast`.** It matches no entry, so the
  whole string `Vidcast` is recorded whenever an embed URL is opened
  top-level before any worker title-hold intervenes.
- **Loading title (low confidence, not observed live).** The share page may
  briefly show `Vidcast - <share id>` while the video's name is still
  loading, from reading the research's loading-state code path; this was not
  confirmed against a live visit.
- Every Vidcast account signed in to the Chrome profile is recorded: no URL
  names the account or organization, so a video's own share settings, not
  this starter, control who can see it.
- The ID template is frozen once shipped (adapter-templates.md §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Vidcast account:

1. Record a test Vidcast. Copy the Share dialog link and the Embed link, and
   confirm both carry the same share ID as the address bar on the playback
   page.
2. Click Edit. Confirm the URL is `/share/edit/<same share id>`, record the
   tab title, and check whether switching editor tabs (`?tab=`) changes it.
3. Open the video from the Library, from a Webex space post, from an email
   invite and from a notification. Record every URL form, and look for any
   `/video/<id>` link in the wild, confirming it lands on `/share/<share
   id>`.
4. Rename the video. Confirm the tab title follows and the share ID does not
   change.
5. Open a restricted video as an unauthorized user, and a deleted or unknown
   share ID. Record both titles and decide whether those visits should keep
   being recorded.
6. Confirm the loading-state title (`Vidcast - <share id>`, if real) and
   whether the worker's title hold hides it in practice.
7. On a Capture Site review on a video page with no saved rules, Brauser
   offers all three Vidcast types with the every-account statement in each
   type's own description and the development-draft source line; saving
   one turns the page card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `vidcast.video`'s
`status` in `protocol/starters/vidcast.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
