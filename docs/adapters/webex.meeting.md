# webex.meeting starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/webex/`, researched blind: public sources only, no
browsing history, cookies or a signed-in Webex account.

History check (2026-11-09): graded against a held-back sample of real
browsing history (real URLs from eight researched sites): 2 real URLs of
this type matched no rule, so 0/2 rules confirmed. Not every rule and
title was confirmed, so this type stays a development draft.

Brauser offers this draft on a tenant's own Webex Meetings site,
`<site>.webex.com` (the `webex` family's strict subdomain origin, `labels:
1`, excluding the product hosts named in its `exclude` list), alongside
Webex recordings. Any combination can be chosen in Capture Site and in the
settings gallery. The draft records one artifact per "Post Meeting Detail"
page — where the site app shows a past meeting occurrence's transcripts,
recordings and AI Assistant output:

`webex.meeting:<site>.webex.com/<meeting instance ID>`

The ID is the meeting instance ID the web app's meeting list builds its
link from. The Meetings API shows instance IDs as `<32 hex>_I_<digits>`
(for example, 53 characters), so the capture is a token of 32 to 128
characters that keeps case, recorded as matched.

```toml
[[artifacts]]
type   = "webex.meeting"
origin = "https://example.webex.com"
match  = [
  { path = "/webappng/sites/{_}/meeting/postinfo/{id:token(32,128)}" },
  { path = "/webappng/sites/{_}/postinfo/{id:token(32,128)}" },
]
id     = "webex.meeting:{host}/{id}"
```

## What each rule covers

Both rules come from the site web app's router table; only the `webappng`
context is matched — there is no public evidence that meeting pages exist
under `/recordingservice`:

1. **Post Meeting Detail**, `/meeting/postinfo/<id>` (route name
   `postMeetingDetail`). The canonical form the meeting list's link builder
   uses.
2. **Short redirect**, `/postinfo/<id>`, a client redirect to rule 1.
   Router table only.

No rule has a rest segment, so a sub-route under the page (if the app ever
adds one) would not match. Query parameters (`?siteurl=`, `?tab=`,
`?from=`, ...) and fragments never change the ID.

## Titles

No title template: the router sets the localized route name, `Post
Meeting Detail - Webex`, and the page's own code never replaces it with
the meeting topic, so every meeting records the same whole tab title
(`title_entry` null in every vector). A Tier 1 field selector for the topic
is a candidate follow-up, not written here because it would need a real
page to confirm.

## Recorded samples

Synthetic equivalent of the recorded shape, on `example.webex.com`:

| URL | Tab title | Records |
|---|---|---|
| `/webappng/sites/example/meeting/postinfo/<32 hex>_I_<digits>` | (none recorded; no title entry) | that meeting instance ID |

The blind research scored 41 synthetic vectors across this family's two
types (8 meeting positives among them), all correctly classified by the
research's own draft and re-verified against this family's own fixture
through both adapter-template engines. The scorecard's 20 held-back,
real Webex visits were all recordings; none exercised a `postinfo` URL
directly, so this type's ID shape and lack of a per-meeting title remain
unverified against a real account (see "Live check still needed").

`samples_shape` is `true`: this type records only one sample, so there is no
second concrete URL for the structural drafter to confuse it with (the
ambiguity that flips Vidcast's video samples to `false` needs two samples
at different depths sharing a route word); one sample cannot create that
ambiguity.

## What the draft does not record

- `/meeting/info/<uuid>` (upcoming-meeting detail): keyed by a different
  meeting UUID, so recording it under this type would split one meeting
  into two IDs;
- `j.php?MTID=`, `j.php?RGID=`, and `/meet/<name>` (join links and personal
  rooms);
- `/meeting/scheduler`, `/meeting/register/<uuid>`, `/weblink/register/...`,
  `/meeting/surveyPage/<id>?token=` (carries a JWT), `/dashboard/*`,
  `/webinar/*`, `/report/*`, and settings and profile routes;
- `postinfo` paths under the `recordingservice` context: no public
  evidence shows meeting pages exist there;
- a token shorter than the 32-character minimum;
- `web.webex.com` and `help.webex.com`: excluded product hosts, not Webex
  Meetings sites, so they are outside this family's origin entirely.

## Known limits

- **Every meeting has the same tab title.** `Post Meeting Detail - Webex`
  is set by the router and never replaced by the meeting's topic, so the
  recorded title never distinguishes one meeting from another; the URL's
  ID is the only distinguishing value today.
- **The ID shape is unverified.** No public `postinfo` URL was found during
  research; the token range accepts both a bare 32-hex UUID and the
  `<hex>_I_<digits>` instance-ID form the Meetings API documents, but if a
  real URL turns out to carry only the bare series UUID, recurring meeting
  occurrences would collapse into one artifact instead of staying distinct.
  This is the first thing to confirm live.
- **Legacy join and registration links are not recorded at all** — by
  design, since they key on a different ID than the instance ID this type
  uses.
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

1. Capture Site: from Meetings → past meetings, open one occurrence's
   detail page; record the `postinfo` URL and its ID's exact shape (bare
   32-hex UUID, or `<hex>_I_<digits>` instance ID?), and the tab title.
2. Open a second occurrence of a recurring meeting and confirm it gets a
   distinct `postinfo` URL and ID from the first.
3. Compare the recorded ID against the Meetings API's `meetingId` for that
   same instance.
4. Open the transcript and AI Assistant tabs on that page; confirm the URL
   path does not change (query-only changes are ignored).
5. Check whether the tab title ever changes away from `Post Meeting Detail
   - Webex`, and in a non-English locale.
6. Try `/postinfo/<id>` (the short redirect form) directly and confirm it
   lands on the canonical `/meeting/postinfo/<id>` URL with the same ID.
7. On a Capture Site review on a meeting page with no saved rules, confirm
   Brauser offers both Webex types — recordings, meeting summaries — with
   the development-draft source line in each type's own description;
   confirm saving one turns the page card's status to "● Artifact
   recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `webex.meeting`'s
`status` in `protocol/starters/webex.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
