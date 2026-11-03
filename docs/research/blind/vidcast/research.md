# Vidcast by Webex: blind starter research

Date: 2026-11-03. I used only public, logged-out sources and this repository.
I did not use any account, browser history or profile. The only browser runs
were headless Chrome with a fresh, empty, throwaway `--user-data-dir`, loading
public sample links that Vidcast's own help articles publish.

Files: `adapters.toml` (three adapters), `vectors.json` (58 synthetic cases),
`run-vectors.mjs` (TS-engine check), `rust-check/` (Rust-crate check, source
only; build output deleted).

## Sources

| # | Source | What it gave |
|---|---|---|
| S1 | `https://app.vidcast.io/` app shell and `/scripts/app.7686f5ce.js`, `/scripts/runtime.b2e6835f.js` (public, served logged-out) | The full client route table (`path:"/share/:shareId"` …), the per-route allowed query keys, the page-title component (`Vidcast - ${value}`), deep-link mapping, hostnames (`app.`, `alpha.`, `integration.vidcast.io`) |
| S2 | Lazy page chunks from the same origin, such as `/scripts/page/SharedVideoLandingPage.<hash>.js`, `VideoRedirectPage`, `CollectionRedirectPage`, `PlaylistDNDWrapPage`, `MainVideoEditLandingPage`, `MainPageViewLandingPage`, `MainPageEditLandingPage`, `EmbedVideoPage` | Each page's title value (`Video Edit`, `Playlist - ${name}`, `Page Edit`, `Request Access`, `Not Found`), `/video/:id` → `/share/:share_id` redirect, `/collections/share/:id` → `/playlists/:id` redirect |
| S3 | Help center, Vidcast landing `https://help.webex.com/en-us/landing/ld-nwdhbaq` (help.vidcast.io redirects there) and its ~70 articles, in particular Share Your Vidcast `https://help.webex.com/n6t5duk`, Embed your Vidcast `https://help.webex.com/6xiqa8`, Playlists `https://help.webex.com/nrzscx9`, Pages `https://help.webex.com/7ody2g`, Editing Overview `https://help.webex.com/65x64o`, Video Details `https://help.webex.com/b0ca5j`, Trim/Stitch/Cut (`ahax52`, `apz2mm`, `nebm388`), Release notes `https://help.webex.com/nls5b52` | Public share links `https://app.vidcast.io/share/<uuid>` (20 distinct), `?playerMode=vidcast`, `?playlistShareId=<uuid>&autoplay=1`, the embed example `/playlists/embed/playlist_link?index=3&t=60&cc=1`, product semantics (Open link / Restricted / Private, request access, editing via the Edit button, playlists shared by copying the browser URL) |
| S4 | Logged-out `curl` of public sample links from S3 (server HTML `<head>`) | `<title>Vidcast</title>` before hydration; server-side `og:url`/`og:title` show the canonical URL for share, embed and legacy routes; an upper-cased share UUID resolves to the same video; unknown IDs fall back to generic meta |
| S5 | Headless Chrome (fresh temporary profile, logged out) `--dump-dom` of the same public samples | Rendered tab titles: `Vidcast - <video name>`, `Vidcast - Playlist - <playlist name>`, bare `Vidcast` on embed pages, `Vidcast - Sign In` on the editor when logged out, `Vidcast - Not Found` for an unknown UUID |
| S6 | Repo: `adapter-templates.md`, `docs/adapters/{confluence.page,miro.board}.md`, `extension/starter-adapters.ts`, `adapter-templates/src/document.rs` (`SHARED_ORIGINS`), `host/src/adapters.rs`, `scripts/adapter-templates-eval.mjs` | Template language, origin and scope rules, eval harness shape |

Web search engines (DuckDuckGo, Bing) returned nothing useful or a CAPTCHA, so
I took no evidence from search snippets. www.vidcast.io returned a Cloudflare
block to curl. Through WebFetch it showed only sign-in and register links and
a pointer to help.vidcast.io.

## Origin model

- **One origin: `https://app.vidcast.io`** (high). Every share link in S3 uses
  it, and the app builds share links from `window.location.origin`. The only
  exception it makes is rewriting `alpha.vidcast.io` to `app.vidcast.io` (S1).
- **Not a shared origin under §6, and no scope.** `app.vidcast.io` is not in
  `SHARED_ORIGINS`, so `unscoped = "acknowledged"` would fail validation
  (`UnscopedNotShared`). No URL names the organization or account: share,
  playlist and page URLs contain only the object's UUID. A `scope` would have
  nothing to bind to. The handling is the same as Miro's
  (`docs/adapters/miro.board.md`): the adapters record videos from every
  Vidcast account signed in to the profile, including free personal accounts,
  and the video's share settings control who can see it. If Brauser's
  maintainers decide Vidcast should be a shared origin, the only option would
  be `unscoped`, because the URL has no tenant position.
- **Excluded hosts:** `alpha.vidcast.io` and `integration.vidcast.io` are
  internal environments named in the bundle. `www.vidcast.io` and
  `vidcast.io` are marketing pages. `help.vidcast.io` redirects to the help
  center.

## Artifact types

### `vidcast.video` (worth a starter: this is the product's core object)

| URL form | Rule | Mode | Confidence | Evidence |
|---|---|---|---|---|
| `/share/<shareId>` (+ `?t=`, `?playerMode=`, `?playlistShareId=`, `?highlightId=`, `?clip=`, `?autoplay=`, `?accessRequestId=` …) | `/share/{video:uuid}` | view | **high** | S1 route and query table; S3 20 public links; S5 rendered |
| `/share/embed/<shareId>` (+ `t, cc, ccLang, mute, autoplay, theme, playerMode, disableAMA, …`) | `/share/embed/{video:uuid}` | view | **high** for the ID (S4: `og:url` is `/share/<same id>`), medium for usefulness (normally loaded in an iframe on another site, so rarely a top-level tab) | S1, S3 embed article, S4, S5 |
| `/share/edit/<shareId>` (+ `?tab=`) | `/share/edit/{video:uuid}` | edit | **high** for the route (S1). Medium for "the same share ID": the route param is named `shareId`, as on `/share/:shareId`, and the Edit button sits on the playback page (S3), but I did not see the editor's URL built in a logged-in session | S1, S2, S3 |

- **ID:** the share ID, a UUID (high: every public sample is a v4-shaped
  UUID). The ID template is `vidcast.video:{host}/{video}`. The `uuid` capture
  lowercases, which is safe: S4 showed the server resolving an upper-cased
  UUID to the same video. The share ID is not the internal video ID (see the
  `/video/:id` exclusion below).
- **Stability (medium):** the share ID is the public permalink and survives
  edits (trim, stitch, title changes). I found nothing about regenerating or
  revoking a share link, but I cannot rule out that a "duplicate" or
  "transfer" creates a new ID. Duplicating creates a new video, which is
  correct. Transfer is unknown.
- **Title:** `Vidcast - <video name>`, prefix form. High: S2 code
  `` e.trim() ? `Vidcast - ${e}` : "Vidcast" `` and S5 rendered
  `Vidcast - Share Permissions & Discoverability Settings`. Template
  `Vidcast - {title}`. A name containing ` - ` survives intact, because only
  the left-anchored prefix is removed.
- **Bad-title cases (high, from code and render):** the editor is always
  `Vidcast - Video Edit` and is recorded as `Video Edit`. The no-access page
  is `Vidcast - Request Access`. A logged-out viewer of a restricted video
  sees `Vidcast - Sign In`, and a dead link shows `Vidcast - Not Found`. The
  page shows the video ID as its title while loading (`Vidcast - <shareId>`
  is plausible from S2's loading branch, low confidence). The embed page and
  the server HTML are bare `Vidcast`, which matches no entry, so the whole
  title `Vidcast` is recorded. The worker's hold-for-a-later-title behaviour
  (DESIGN.md §5.1) should cover most of these on the share page, but not on
  the editor.

### `vidcast.playlist` (worth a starter: playlists are shared by URL and the help center describes them as a primary sharing unit)

| URL form | Rule | Confidence | Evidence |
|---|---|---|---|
| `/playlists/<playlistShareId>` (+ `accessRequestId, pageShareId, playerMaxWidth, hideWatermark`) | `/playlists/{playlist:uuid}` | **high** | S1, S3 ("copy/pasting the link from your browser"), public `playlistShareId=<uuid>` in S3, S5 rendered |
| `/playlists/embed/<playlistShareId>` (+ `index, expand, t, cc, videoShareId, …`) | `/playlists/embed/{playlist:uuid}` | **high** for the ID (S4 `og:url`) | S1, S3, S4 |
| `/collections/share/<shareId>` (legacy "collections") | `/collections/share/{playlist:uuid}` | **high** that it is the same ID: S2 redirects to `/playlists/<same id>` and S4 `og:url` agrees. Medium that real old links exist in the wild | S1, S2, S4 |

- **ID:** a UUID (high, from public `playlistShareId` values and a successful
  render). The ID template is `vidcast.playlist:{host}/{playlist}`.
- **Title:** `Vidcast - Playlist - <playlist name>` (high: S2
  `` `Playlist - ${Re}` `` plus S5 render `Vidcast - Playlist - Training/Demos`).
  Only that entry is listed, so `Vidcast - Request Access`, `Vidcast - Not
  Found` and bare `Vidcast` (embed and legacy redirect) are recorded whole
  rather than taken for a playlist name.
- **Conflict found:** the mobile deep-link helper in S1 maps
  `/collections/share/<id>` to `vidcast://share/<id>`, a video, while the web
  router redirects it to a playlist. The web router and the server's `og:url`
  are what a browser sees, so I followed them.
- A video opened inside a playlist, `/share/<video>?playlistShareId=<playlist>`,
  is recorded as the video. The playlist query key is ignored.

### `vidcast.page` (included, **low confidence on the ID shape**)

Vidcast "Pages" are curated showcase pages (S3 Pages article) with view, edit
and embed routes (S1).

| URL form | Rule | Mode | Confidence |
|---|---|---|---|
| `/pages/<pageShareId>` | `/pages/{page:uuid}` | view | route high, ID shape **low** |
| `/pages/<pageShareId>/edit` | `/pages/{page:uuid}/edit` | edit | route high, ID shape low |
| `/pages/embed/<pageShareId>` | `/pages/embed/{page:uuid}` | view | route high, ID shape low |

- I found no public page link, so I assumed `uuid` because videos and
  playlists use UUID `share_id`s from the same model and pages are addressed
  by `pageShareId`. If page IDs are not UUIDs, these rules match nothing.
  They fail closed and never record a wrong artifact.
- **Title:** `Vidcast - <page name>` from S2 (`value: Z.info.name`, default
  prefix), not seen live (medium). The editor shows `Vidcast - Page Edit`
  (high, from code).
- If the reviewer prefers only high-confidence starters, drop this adapter.
  Nothing else depends on it.

## Routes deliberately excluded

- `/video/<id>`: an internal video ID that the app looks up and then
  client-side navigates to `/share/<share_id>` (S2). The ID differs from the
  share ID, so giving it the video's ID is impossible, and giving it another
  ID would split one video in two. A client-side navigation is not a server
  redirect, so no alias would be learned either. The visit is recorded once
  the app lands on `/share/…`.
- `/share/edit-script/<shareId>/<uploadId>`: AI "script" editing of an upload
  (Doc2Video). It is niche, and I could not confirm its relation to a
  published video.
- `/webex-stream?recordingUUID=…&siteUrl=…`: a Webex meeting recording
  streamed in Vidcast. Its identity is a Webex recording and belongs to a
  Webex adapter. Its query keys would also be the only ID source.
- `/event/<eventShareId>/…`, `/events`, `/studio`: Vidcast Studio live events,
  which are not "per-video" business artifacts. A later starter could cover
  them, but I found no public samples.
- Lists and app chrome: `/`, `/library`, `/playlists`, `/pages`,
  `/collections`, `/favorites`, `/recommended`, `/trending`, `/search`,
  `/notifications`, `/trash`, `/analytics` (including `?videoShareId=`),
  `/insights`, `/settings`, `/admin/*`, `/author/<userId>`, `/group/<id>`,
  `/groups`, `/webex*`, `/ms-teams`, `/coaching*`, `/uploads/<id>`,
  `/integrations`, `/apps`, `/sign-in`, `/sign-up`, `/register*`,
  `/app-redirect` (carries tokens), `/not-found`.
- Anything below an artifact path: no rule has a rest segment.

## Validation

Both engines in the repo accept the document, and all 58 vectors pass in both
with identical results:

```sh
cd /Users/jrauser/ENDURANCE/work/rb-blind-starters
export PATH=~/.nvm/versions/node/v24.14.0/bin:$PATH
node research/blind/vidcast/run-vectors.mjs
#   document valid: 3 adapters
#   58/58 cases pass

# Same requests through the repo's Node eval harness and the Rust crate:
node research/blind/vidcast/run-vectors.mjs --emit-requests > /tmp/req.jsonl
node scripts/adapter-templates-eval.mjs < /tmp/req.jsonl > /tmp/ts.jsonl
export PATH=~/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
(cd research/blind/vidcast/rust-check && cargo build --offline -q)
research/blind/vidcast/rust-check/target/debug/vidcast-rust-check < /tmp/req.jsonl > /tmp/rust.jsonl
#   rust: 58/58 pass; rust vs ts differences: 0
#   non-match reasons seen: no_adapter, no_rule, path_rejected
```

`rust-check/` is a standalone crate (its own `[workspace]` and copied
`Cargo.lock`) with a path dependency on `adapter-templates`. It makes the same
`Document::from_value` and `evaluate` calls as `host/src/adapters.rs`, without
building the host. I deleted its `target/` (181 MB) after the run. I edited no
product code.

## Known limits and open questions

1. The editor titles are fixed: `Video Edit` and `Page Edit`. An edit visit
   records a useless title. If Brauser keeps the latest title per artifact,
   editing can overwrite a good title. The engine has no per-rule title, so
   splitting this off would need a separate adapter type or a host rule such
   as "do not let an edit-mode title replace a view-mode title".
2. Access and error states (`Request Access`, `Sign In`, `Not Found`) are
   recorded as artifacts with those titles, because the URL is a valid share
   URL. A dead link is still recorded.
3. Embed URLs give a bare `Vidcast` title.
4. The page ID shape is unverified (low).
5. Is the editor URL's ID really the share ID (medium)? Is the router case
   sensitive? I assumed yes and kept `path_case` sensitive, so a hand-typed
   `/Share/<id>` would be missed. Do Vidcast share IDs ever change, for
   example on transfer of ownership?
6. Account mixing: no URL separates work and personal Vidcast accounts.
7. The loading title may briefly be `Vidcast - <shareId>`. That comes from
   reading S2's loading branch and is not observed. The worker's title hold
   probably hides it.
8. A Vidcast MCP server exists (help article `nepa9as`), which a summary
   flow could use, like Miro's. I did not investigate it.

## How I would verify this with a real account

1. Record a test vidcast. Copy the Share dialog link and the Embed link, and
   confirm both carry the same UUID as the address bar on the playback page.
2. Click Edit. Confirm the URL is `/share/edit/<same uuid>` and record the
   tab title, including after switching editor tabs (`?tab=`).
3. Open the video from the Library, from a Webex space post, from an email
   invite and from a notification. Record every URL form. Look for any
   `/video/<id>` link in the wild and confirm it lands on `/share/<share_id>`.
4. Rename the video. Confirm the tab title follows and the share ID does not
   change. Transfer it to another user and check whether the ID changes.
5. Create a playlist. Check `/playlists/<uuid>`, its embed link and the tab
   title. Open an old `/collections/share/<id>` link if one exists.
6. Create and publish a Page. Confirm `/pages/<id>` is a UUID (or fix the
   capture type), and record view and edit titles.
7. Open a restricted video as an unauthorised user and a deleted video.
   Record titles and decide whether those visits should be dropped.
8. Run the samples through `node research/blind/vidcast/run-vectors.mjs`
   after adding them as vectors, then do the Capture Site live check.
