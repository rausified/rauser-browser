# Webex blind starter: research notes

Date: 2026-11-03. Blind test: built only from public, logged-out sources and
this repository. No account, no browser history, no sign-in.

Files in this folder:

- `adapters.toml`: two adapters, `webex.recording` and `webex.meeting`.
- `vectors.json`: 41 synthetic cases (13 recording, 8 meeting, 20 negative).
- `toml2json.py`, `validate.mjs`, `rscheck/`: throwaway validation harnesses
  (see "Validation").

## Sources consulted

| # | Source | What it gave |
|---|---|---|
| S1 | Webex developer API reference, embedded OpenAPI data in `https://developer.webex.com/docs/api/v1/recordings/list-recordings` (also `.../meetings/list-meetings`, `.../meeting-transcripts/list-meeting-transcripts`, `.../meeting-summaries/get-summary-by-meeting-id`), fetched as raw HTML | Recording `id` is 32 hex digits. `playbackUrl` = `https://site4-example.webex.com/site4/ldr.php?RCID=<32 hex>`, `downloadUrl` = `.../lsr.php?RCID=<32 hex>`. "Share a Recording Link" `webShareLink` examples: `https://example.webex.com/recordingservice/sites/example/recording/playback/<32 hex>` and `https://example.webex.com/webappng/sites/example/recording/<32 hex>/playback`. Meeting IDs: series `<32 hex>`, instance `<32 hex>_I_<digits>`, scheduled `<32 hex>_<timestamp>Z`. Join links `j.php?MTID=m<hex>`, `/meet/<name>`. A worked example in the meetings guide shows one recording with three different values: `id` 724a81a6..., playback RCID 61976669..., download RCID 77ab059a.... |
| S2 | yt-dlp Cisco Webex extractor, `https://raw.githubusercontent.com/yt-dlp/yt-dlp/master/yt_dlp/extractor/ciscowebex.py` | Working URL grammar: `<sub>.webex.com/<site>/(ldr|lsr).php?...RCID=<32 hex>` and `<sub>.webex.com/(recordingservice|webappng)/sites/<site>/recording/(playback/|play/)?<32 hex>`. The RCID page is fetched and the real recording URL is read *out of the page*, then resolved: RCID pages forward to the recordingservice player. The 32-hex ID is what the stream API (`/webappng/api/v1/recordings/<id>/stream`) takes. Canonical `webpage_url` = `/recordingservice/sites/<site>/recording/playback/<id>`. Host label and site path may differ (`demosubdomain` / `demositeurl`). |
| S3 | Public recording links in open-source docs found via Sourcegraph public search (`recordingservice/sites`, `webappng/sites`): NVIDIA/NVFlare docs (`/recordingservice/sites/<site>/recording/<id>/playback`), kubernetes/community (`/recordingservice/sites/<site>/recording/play/<id>`), webmastersmith/WGU_Cloud_Computing (`.../recording/<id>/playback`), webex/webex-js-sdk test fixture (`/webappng/sites/<site>/recording/playback/<id>`), IETF RFC reference (`/webappng/sites/ietf/dashboard?siteurl=ietf`), CMPackager (`/webappng/sites/cisco/dashboard/download`), kubestellar (`/webappng/sites/ibm/meeting/scheduler`) | Real-world confirmation of every recording form in rules 0–2, and of the `/<context>/sites/<site>/` layout. Real IDs are not reused in vectors. |
| S4 | The site web app's JavaScript as served logged-out by a public recording link (`https://<site>.webex.com/recordingservice/sites/<site>/recording/<id>/playback` from S3): `webappng/scripts/hub.<hash>.js`, `webappng/i18n/localization_en_US.<hash>.js`, `webappng/scripts/chunks/recording-chunk.9285.<hash>.js`, chunk 7783 (post-meeting detail) | The Vue router table (history mode, `base = getSitePath()` = `/<context>/sites/<siteUrl>`, or `/<context>/hub` for the user hub). Recording routes: `mixInPlayer /recording/:uuid/playback` (title "Replay Recorded Meeting"), `share /recording/playback/:uuid` ("Recorded Meeting Password"), `download /recording/download/:uuid`, redirects `/recording/play/:uuid`, `/recording/recording/:id`, `/:uuid/playback` → player and `/playback/:uuid` → share, `CallingPlayer /convergedRecording/:uuid/playback`, `RecordingList /recording/home`. Meeting routes: `meetingDetial /meeting/info/:uuid` ("Meeting Detail"), `postMeetingDetail /meeting/postinfo/:uuid` ("Post Meeting Detail"), redirect `/postinfo/:uuid`. Title code: router sets `document.title = <localized route name> + " - " + (windowTitle || "Webex")`; the recording player then calls `document.title = recordName + " - " + (windowTitle || "Webex")` when the recording loads; the Calling player sets `document.title = record.title` with no suffix. The meeting list builds post-meeting links as `<sitePath>/meeting/postinfo/<meetingInstanceID>` and upcoming ones as `/meeting/info/<meetingUUID>`. The post-meeting chunk shows transcripts and never sets `document.title`. |
| S5 | `https://web.webex.com/` shell and its public bundles | Client routes `/spaces`, `/spaces/<id>`, `/teams/<btoa(team url)>`, `/meetingcontainer/<globalId>/messages/<id>`, `/spaces/meetinginfo/<id>`, `/meetings/meetingContent` (tabs about/messages/recordings/content/summary/transcript). Browser titles are category strings only (`app.browserTitle.*`: "Messaging", "Meeting recap", "Meetings"...). Used only to decide what to leave out. |
| S6 | Repository: `adapter-templates.md`, `docs/adapters/confluence.page.md`, `docs/adapters/miro.board.md`, `extension/starter-adapters.ts`, `adapter-templates/src/document.rs` (`SHARED_ORIGINS`), `extension/adapter-templates.ts`, `protocol/adapter-templates/vectors.json` | Spec and conventions. |

Search engines (DuckDuckGo, Bing) were tried and returned nothing useful
(captcha, irrelevant results). No help.webex.com article was found that
quotes a playback URL or a tab title.

## Origin model

**Tenant subdomain family: `https://<site>.webex.com`**, like `*.atlassian.net`.
Each Webex Meetings site has its own host, so the origin is not shared between
tenants and needs neither `scope` nor `unscoped` (§6). `webex.com` hosts are
not on `SHARED_ORIGINS`, and should not be.

Recommended `starterKind` test: `host.endsWith(".webex.com")`, exactly one
label before `webex.com`, and not one of the product hosts that are not
Meetings sites: `www`, `web`, `teams`, `help`, `developer`, `admin`, `status`,
`cart`, `idbroker`, `identity`, `apphub`, `signin`, `settings`,
`eaf-sdk`, `web-sdk`, `downloadservicebts`, `mrsprod` (list from S1/S5; not
exhaustive). Multi-label hosts (`<site>.dmz.webex.com`, media hosts such as
`nsj1wss.webex.com`) appear in S1 but are internal/media; a wrong offer there
is harmless because no rule would match, but they should not be offered.

The path also names the site (`/sites/<siteUrl>/`). It usually equals the
host's first label but need not (S1 `site4-example.webex.com/site4/`, S2
`demosubdomain`/`demositeurl`), so it is matched with `{_}` and not used in
IDs. The host already identifies the tenant.

## Artifact types

### webex.recording — confidence: high (URL and ID), medium (title)

URL forms (base `/(recordingservice|webappng)/sites/<site>`):

| Rule | Path after base | Role | Evidence |
|---|---|---|---|
| 0 | `/recording/<id>/playback` | the player | S1 webShareLink, S2 test, S3 (several), S4 `mixInPlayer` |
| 1 | `/recording/playback/<id>` | share link; password gate when set | S1 webShareLink, S2 canonical `webpage_url`, S3 js-sdk, S4 `share` |
| 2 | `/recording/play/<id>` | older link, client redirect to 0 | S2 regex, S3 kubernetes, S4 `newPlayer` |
| 3 | `/recording/recording/<id>` | client redirect to 0 | S4 `detail` only |
| 4 | `/playback/<id>` | client redirect to 1 | S4 only |
| 5 | `/<id>/playback` | client redirect to 0 | S4 only |

Rules 3–5 rest on the router table alone (no URL seen in the wild); they are
cheap and safe because the 32-hex ID is fixed-shape and behind literals.

**ID:** `webex.recording:{host}/{id}`, `id` = `hex(32)` (lowercased). It is
the Recordings API `id` and the stream API key (S1, S2). Both contexts and all
six forms give one ID per recording.

**Tab title:** `<recordName> - <brand>`, brand = the site's configured
`windowTitle`, default `Webex` (S4, read from code, not observed in a real
tab). Template `{title} - {~}` drops the text after the last ` - `.
recordName defaults to the meeting topic plus a date-time suffix
(S1 example `Test meeting-20210514 2025-1`). Medium confidence: the code path
is clear, but the brand value per site and the exact timing are unobserved.

**Excluded:**

- `/<site>/ldr.php?RCID=` and `lsr.php?RCID=` (API `playbackUrl`/`downloadUrl`).
  RCID is not the recording ID (S1 worked example), so matching it would give
  one recording two IDs. The page forwards to the recordingservice player
  (S2), which rules 0/1 record. If that forward turns out to be an HTTP
  redirect, Brauser's server-redirect alias would also cover it; either way
  nothing is lost except a visit that never reaches the player.
- `/recording/download/<id>` (download page, an action, like Google `/export`),
  `/recording/error/<id>`, `/recording/home` (list), `/recording/<id>/registration/...`
  (webinar-recording registration forms), `/recording/downloadMediaRes`.
- `/convergedRecording/<id>/playback` (Webex Calling recordings): ID shape
  unknown (could be a dashed UUID), title has no brand suffix. Candidate for a
  later rule once a real URL is seen.
- Bare `/recording/<id>` (S2's regex tolerates it, but the router has no such
  route; `/recording/*` is a catch-all).

### webex.meeting — confidence: medium (route), medium-low (ID shape), low value title

The "Post Meeting Detail" page of one past meeting occurrence, where the site
app shows transcripts, recordings and AI Assistant output for that meeting.

| Rule | Path | Evidence |
|---|---|---|
| 0 | `/webappng/sites/<site>/meeting/postinfo/<instanceId>` | S4 router + link builder |
| 1 | `/webappng/sites/<site>/postinfo/<instanceId>` | S4 redirect only |

**ID:** `webex.meeting:{host}/{id}`, `id` = `token(32,128)`, case kept. The
link builder uses `meetingInstanceID`; S1 shows instance IDs as
`<32 hex>_I_<18 digits>` (53 chars). I never saw a real postinfo URL, so the
exact value in the URL (instance ID vs. bare 32-hex UUID, case) is
unconfirmed. `token(32,128)` accepts both shapes; if the URL turns out to
carry only the 32-hex UUID, the rule still works but recurring meetings may
collapse to the series. That is the first thing to verify.

**Tab title:** the router's `Post Meeting Detail - Webex` (localized), never
replaced by the topic (S4: chunk 7783 never touches `document.title`). So
there is no title template; every meeting records the same tab title. A
Tier 1 field selector for the topic would fix this, but writing one blind
would be a guess.

**Excluded:** `/meeting/info/<uuid>` (upcoming-meeting detail; keyed by
`meetingUUID`, a different value from the instance ID, so one meeting would
split into two IDs, and it is mostly a join/schedule page), `j.php?MTID=`,
`j.php?RGID=`, `/meet/<name>` (join links and personal rooms),
`/meeting/scheduler`, `/meeting/register/<uuid>`, `/weblink/register/...`,
`/meeting/surveyPage/<id>?token=` (carries a JWT), `/dashboard/*`,
`/webinar/*`, `/report/*`, settings and profile routes. Meeting pages under
`/recordingservice` are not matched (no evidence they exist).

### Types considered and left out

- **AI Assistant summary as its own artifact.** In the site app the summary is
  a panel inside the recording player (S4: `generateAISummaryUrl`,
  `showAISummary` in the player chunk) or part of the post-meeting page; it
  has no URL of its own there. The Meeting Summaries and Transcripts APIs
  (S1) are API resources, not browser pages.
- **web.webex.com (Webex App in the browser): spaces, teams, meeting recap.**
  One origin for every organization (a shared-origin candidate with no tenant
  in the URL), tab titles are generic category strings (`Messaging`,
  `Meeting recap`), some routes embed base64 global IDs built with `btoa`
  that can contain `/` and `+` (an encoded `/` is refused by §5.1), and
  `/meetings/meetingContent` shows no per-item ID in the path. A
  `webex.space` starter (`/spaces/{id:uuid}`) may be possible, but it would
  need a real URL sample and the shared-origin decision first.
- **Webex user hub (`/<context>/hub/...`)**: the router supports it as a base,
  but I found no public URL using it. Open question below.
- **Vidcast, Slido, Webex Events**: separate products and origins.

## Validation

Both engines in the repository accept the document and agree on all 41
vectors. No product code was edited.

```sh
cd research/blind/webex
export PATH=~/.nvm/versions/node/v24.14.0/bin:$PATH
python3 -I toml2json.py adapters.toml > /tmp/webexres/webex-doc.json
node --experimental-transform-types --no-warnings validate.mjs /tmp/webexres/webex-doc.json vectors.json
#   validated: 2 adapters, 8 rules
#   41/41 vectors pass

export PATH=~/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
cd rscheck && cargo run --offline -q -- /tmp/webexres/webex-doc.json ../vectors.json
#   validated (rust): 2 adapters
#   41/41 vectors pass (rust)
```

- `validate.mjs` imports `extension/adapter-templates.ts` (`compileDocument`,
  `evaluate`). Node 24's strip-only mode rejects that file's constructor
  parameter properties, so `--experimental-transform-types` is needed.
- `rscheck/` is a throwaway crate (own `[workspace]`, a copy of the repo's
  `Cargo.lock`, `--offline`) that calls `adapter_templates::Document::from_value`
  and `Document::evaluate`. Its `target/` (181 MB) was deleted after the run.
- The TOML follows §3 (`[adapters] format = 1` plus `[[artifacts]]`). The
  engines take `{"format": 1, "artifacts": [...]}`, which `toml2json.py` builds.
- The host CLI `brauser adapters eval` (`host/src/adapters.rs`, JSON Lines on
  stdin) would also work, but building the host on a shared, loaded machine
  was avoided.

## Known limits

- **Generic titles before load.** A recording's tab title starts as the
  localized route name (`Replay Recorded Meeting - Webex`, or
  `Recorded Meeting Password - Webex` on a protected share link) and becomes
  `<recordName> - Webex` only after the recording loads, or after the password
  is entered. A short visit, or one that stops at the password gate, records
  `Replay Recorded Meeting` / `Recorded Meeting Password`. Same pattern as
  Miro's `Miro - Miro`.
- **Brand suffix.** The skip drops everything after the last ` - `. If a site's
  custom `windowTitle` itself contains ` - ` (for example `Acme - Webex`),
  its first part stays in the title.
- **Localized route titles.** The pre-load titles are localized; the template
  is language-neutral, but the recorded placeholder text varies by locale.
- **webex.meeting titles are all the same** (no topic in the tab title).
- **webex.meeting ID shape is unverified** (see above).
- **Legacy RCID links are not recorded directly**; they count only once they
  reach the player.
- The site path segment is ignored, so a URL that names a different site than
  the host is still recorded under the host (the server would refuse or
  redirect such a URL anyway).

## Open questions

1. Does `ldr.php?RCID=` reach the player by an HTTP redirect (giving a free
   server-redirect alias) or by script?
2. Is the `/meeting/postinfo/<id>` value the `<hex>_I_<digits>` instance ID,
   and is it case-stable?
3. Does any real page use the `/webappng/hub/...` base (the "user hub")? If so,
   it needs its own rules (no `sites/<site>` segments).
4. What ID shape does `/convergedRecording/<id>/playback` use?
5. Which non-site hosts on `webex.com` must the starter offer exclude, and do
   `<site>.my.webex.com`-style or government (`webexgov`) hosts need their
   own family?
6. Should `web.webex.com` become a shared origin with a `webex.space` starter?

## How I would verify this with a real account

1. Open a recording from the Recordings list; note the URL form and the tab
   title once it plays. Confirm `<recordName> - Webex` (or the site brand).
2. Copy the recording's share link (Share → copy link); open it in a new tab,
   with and without a password; note URL changes and titles at each step.
3. Open the API `playbackUrl` (`ldr.php?RCID=`) and watch the Network panel:
   HTTP 30x or script navigation? Confirm the final URL's 32-hex ID equals
   the API `id`.
4. Compare the ID in rules 0/1 for the same recording opened from the list,
   from an email, and from the Webex App.
5. From Meetings → past meeting, open the meeting detail page; record the
   `postinfo` URL, its ID shape (compare with the API `meetingId` for that
   instance), and the tab title. Open two occurrences of one recurring
   meeting and confirm they get different IDs.
6. Open the AI summary and transcript tabs on that page and in the player;
   confirm the URL path does not change (query-only changes are ignored).
7. Try a Webex Calling recording (`convergedRecording`) and a webinar
   recording, and check whether any `/hub/` URL appears.
8. Add every observed URL and title as a vector, then rerun both harnesses.
