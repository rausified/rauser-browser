# webex.recording starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/webex/`, researched blind: public sources only, no
browsing history, cookies or a signed-in Webex account.

History check (2026-11-09): graded against a held-back sample of real
browsing history (real URLs from eight researched sites): 20 real URLs
(39 visits) matched, 2/6 rules confirmed, the recorded title right on
7/20, and 11 real URLs of this type matched no rule. Not every rule and
title was confirmed, so this type stays a development draft.

Brauser offers this draft on a tenant's own Webex Meetings site,
`<site>.webex.com` (the `webex` family's strict subdomain origin, `labels:
1`, excluding the product hosts named in its `exclude` list), alongside
Webex meeting summaries. Any combination can be chosen in Capture Site and
in the settings gallery. The draft records one artifact per cloud
recording, however it is opened:

`webex.recording:<site>.webex.com/<recording ID>`

The ID is the recording's 32-hex-digit ID, the same value the Recordings
API and the web app's own stream API (`/webappng/api/v1/recordings/<id>/
stream`) use. It is fixed-shape and is lowercased canonically, so case
variants and the two router contexts, `recordingservice` and `webappng`,
give one ID. The site name segment in the path (`/sites/<site>/`) usually
matches the host's first label but need not; it is matched with `{_}` and
never used in the ID, so the host alone identifies the tenant.

```toml
[[artifacts]]
type   = "webex.recording"
origin = "https://example.webex.com"
match  = [
  { path = "/(recordingservice|webappng)/sites/{_}/recording/{id:hex(32)}/playback" },
  { path = "/(recordingservice|webappng)/sites/{_}/recording/playback/{id:hex(32)}" },
  { path = "/(recordingservice|webappng)/sites/{_}/recording/play/{id:hex(32)}" },
  { path = "/(recordingservice|webappng)/sites/{_}/recording/recording/{id:hex(32)}" },
  { path = "/(recordingservice|webappng)/sites/{_}/playback/{id:hex(32)}" },
  { path = "/(recordingservice|webappng)/sites/{_}/{id:hex(32)}/playback" },
]
id     = "webex.recording:{host}/{id}"
title  = "{title} - {~}"
generic_titles = ["Recorded Meeting Password", "Replay Recorded Meeting", "Sign In"]
```

`generic_titles` lists the titles shown by Webex's sign-in page and recording
password gate instead of the item's own name (`adapter-templates.md` §9.4). A
visit whose title is one of them, ASCII case aside, or is empty or the bare
host, records no title, so the artifact keeps the title it already has
("Untitled" for a new one).

## What each rule covers

All six rules come straight from the site web app's own Vue router table
(base = context + site):

1. **The player**, `/recording/<id>/playback` (route name `mixInPlayer`).
   The canonical form: the Recordings API's `webShareLink` example, a
   yt-dlp test URL, and several public recording links all use it.
2. **The share link**, `/recording/playback/<id>` (route name `share`).
   The Recordings API's other `webShareLink` form; shows a password gate
   first when the recording has one.
3. **Older player link**, `/recording/play/<id>` (route name `newPlayer`),
   a client redirect to rule 1's router target (the player, rule 1 above).
4. **Detail redirect**, `/recording/recording/<id>` (route name `detail`),
   a client redirect to the player. Seen only in the router table, not in
   the wild.
5. **Short share redirect**, `/playback/<id>`, a client redirect to the
   share link. Router table only.
6. **Short player redirect**, `/<id>/playback`, a client redirect to the
   player. Router table only.

Rules 3–6 rest on the router table alone (no URL seen in the wild); they
are cheap and safe because the 32-hex ID is fixed-shape and sits behind
literal path segments, so a wrong guess there matches nothing else.

No rule has a rest segment, so `/recording/download/<id>` (the download
page), `/recording/error/<id>`, `/recording/home` (the list) and the
webinar-recording registration pages (`/recording/<id>/registration/...`)
never match. Query parameters (`?siteurl=`, `?from=`, `?t=`, ...) and
fragments never change the ID.

## Titles

One entry: `{title} - {~}` drops everything after the last ` - `, the
site's configured brand (default `Webex`) that the player and the router
both append.

The vectors show what happens before a recording has set its real title:

- **Interstitials record no title.** Before a recording loads, and at the
  share link's password gate, the router's own route title still carries
  the site's brand as ` - Webex` (`Replay Recorded Meeting - Webex`,
  `Recorded Meeting Password - Webex`), so the template strips it, and the
  result is a generic title, so the visit records no title and the
  recording keeps its own name
  (`eval.webex.recording.player_loading_title`,
  `eval.webex.recording.share_recordingservice`). The password gate's title
  before any brand is attached, `Recorded Meeting Password`, matches no
  split point and is generic the same way
  (`eval.webex.recording.password_title`).
- **An unbranded page title is kept whole,** `title_entry` null: a dead or
  unbranded page title, `Cisco Webex Meetings`
  (`eval.webex.recording.title_no_suffix_kept_whole`), matches no split
  point and is recorded exactly as shown.

## Recorded samples

Synthetic equivalent of the recorded shape, on `example.webex.com`:

| URL | Tab title | Records |
|---|---|---|
| `/recordingservice/sites/example/recording/<32 hex>/playback` | `Design review - Example Site` | that recording ID, title `Design review` |

The blind research scored 41 synthetic vectors across this family's two
types (13 recording positives among them), all correctly classified by the
research's own draft and re-verified against this family's own fixture
through both adapter-template engines. Graded later against 20 real,
held-back Webex browsing visits (`docs/research/blind/SCORECARD.md`), the
research matched all 20 correctly, with one ID stable across both player
forms and both router contexts, and the right title template once a
recording had loaded — 15 of those 20 visits recorded an interstitial title
(`Recorded Meeting Password` / `Replay Recorded Meeting`) first, the known
limit named above.

One research case, `rec.player.site_path_differs_from_host.clean`, opens
the player on a second real host, `site4-example.webex.com`, with a site
path segment (`site4`) that does not match the host's own first label —
confirming the `{_}` site-path capture is correctly ignored and the ID
depends on the host alone. It is not carried as a vector here: the
synthesized `starter:webex` fixture this family's vectors run against is
pinned to one origin (`sample_origin`), so a second host can only be
exercised in the standalone research harness, not through this file's
vectors.

## What the draft does not record

- `/<site>/ldr.php?RCID=<hex>` and `/<site>/lsr.php?RCID=<hex>`, the legacy
  `playbackUrl`/`downloadUrl` the Recordings API returns: `RCID` is a
  different value from the recording ID (one API example shows three
  different values for one recording), and the page forwards to the
  recordingservice player, which rules 1/2 already record; matching `RCID`
  would split one recording across two IDs;
- `/recording/download/<id>` (download page), `/recording/error/<id>`,
  `/recording/home` (list), and the webinar-recording registration pages;
- `/convergedRecording/<id>/playback` (Webex Calling recordings): its ID
  shape is undocumented and its title has no brand suffix;
- a bare `/recording/<id>` with no further segment: no router route uses
  it;
- an ID shorter than 32 hex digits;
- a percent-encoded `/` inside the site segment;
- `web.webex.com` and `help.webex.com`: excluded product hosts, not Webex
  Meetings sites, so they are outside this family's origin entirely.

## Known limits

- **Generic titles before load.** A recording's tab title starts as the
  localized route name and becomes `<recordName> - <brand>` only after the
  recording loads, or after a password is entered. A short visit, or one
  that stops at the password gate, records `Replay Recorded Meeting` or
  `Recorded Meeting Password` instead — confirmed in 15 of the scorecard's
  20 held-back visits, so expect it routinely, not as an edge case.
- **Brand suffix.** The skip drops everything after the last ` - `. If a
  site's custom brand itself contains ` - ` (for example `Acme - Webex`),
  the first part of that brand stays in the recorded title.
- **Localized route titles.** The pre-load titles are localized; the
  template is language-neutral, but the recorded placeholder text varies by
  locale.
- **Legacy RCID links are not recorded directly**; they count only once
  they reach the player (Open question in the research: whether that
  forward is an HTTP redirect or a script navigation).
- **The Webex Calling (`convergedRecording`) rule is not written at all**,
  for lack of a real URL; a later rule would need one.
- **A Webex media or infrastructure host the research never named is
  still offered this starter.** The one media host it names,
  `nsj1wss.webex.com` (`docs/research/blind/webex/research.md`, "Origin
  model"), is excluded. Its name puts the varying part first (a data-centre
  code and number, then `wss`), so an `exclude` prefix such as `fupload-*`
  cannot cover its siblings, and excluding a guessed prefix could hide a
  real customer site. Those hosts serve no Meetings site pages, so none of
  this type's rules is expected to match anything there.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Webex Meetings site:

1. Capture Site: open a recording from the Recordings list; note the URL
   form and the tab title once it finishes loading. Confirm `<recordName> -
   <brand>`, and what the site's own brand value is.
2. Copy the recording's share link; open it in a new tab, with and without
   a password; note the URL and the title at each step (password gate vs.
   loaded).
3. Open the API `playbackUrl` (`ldr.php?RCID=`) and watch the Network
   panel: HTTP redirect or script navigation? Confirm the final URL's
   32-hex ID equals the API `id`.
4. Compare the ID in rules 1 and 2 for the same recording opened from the
   list, from an email, and from the Webex App.
5. Try a Webex Calling recording (`convergedRecording`) if one is
   available, and record its real URL and ID shape.
6. Watch the title while the pre-load route name flashes before the real
   title loads, and in a non-English locale.
7. On a Capture Site review on a recording page with no saved rules,
   confirm Brauser offers both Webex types — recordings, meeting
   summaries — with the development-draft source line in each type's own
   description; confirm saving one turns the page card's status to "●
   Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `webex.recording`'s
`status` in `protocol/starters/webex.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
