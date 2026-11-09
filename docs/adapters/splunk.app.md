# splunk.app starter draft

Status: development draft. The URL rules come from
`docs/research/blind/splunk-cloud/`, researched blind: public sources only,
no browsing history, cookies or a Splunk account.

Brauser offers this draft on a Splunk Cloud stack's own host,
`<stack>.splunkcloud.com` (the `splunk` family's strict subdomain origin,
`labels: 1`, `exclude: ["www"]`), alongside Splunk dashboards. Either or
both can be chosen in Capture Site and in the settings gallery. The draft
records one artifact per Splunk app on the stack:

`splunk.app:<stack>.splunkcloud.com/app/<app>`

An app is a container (`adapter-templates.md` §8.3): when Splunk dashboards
are chosen too, each dashboard declares `within` its app, so a dashboard's
time also counts toward the app it sits in.

```toml
[[artifacts]]
type   = "splunk.app"
origin = "https://acme.splunkcloud.com"
match  = [
  { path = "/{_}/app/{app:name}" },
  { path = "/{_}/app/{app:name}/(search|dashboards|reports|alerts|datasets|report|alert|dataset|job_manager|analytics_workspace|pivot|field_extractor|home|configuration|inputs)" },
]
id     = "splunk.app:{host}/app/{app}"
```

## What each rule covers

1. **App root**, `/<locale>/app/<app>`. Splunk Web opens the app's default
   view from here.
2. **The app's built-in pages**, `/<locale>/app/<app>/<built-in view>`:
   Search (with or without `q=`, `s=` and `sid=`), the Dashboards, Reports,
   Alerts and Datasets listing pages, report and alert detail pages
   (`?s=<REST path>`), the job manager, Analytics Workspace, Pivot, the field
   extractor, `home` (Splunk Home in the launcher app), and the
   `configuration` and `inputs` pages every UCC-generated add-on has.

The locale (`en-US`, `ja-JP`, ...) is `{_}`, not part of the ID, so one app
opened in two locales is one artifact. App names are directory names
(`TA-acme.ops`, `Splunk_SA_CIM`) and keep their case. No rule has a rest
segment, so `/<locale>/app/<app>/reports/<anything>` never matches.

**Rule 2 overlaps Splunk dashboards on purpose.** Dashboards and built-in
pages share `/<locale>/app/<app>/<view>`, and the template language has no
"any word but these" segment, so the dashboard rule also matches
`/en-US/app/search/search`. The family declares this one rule pair as a
`container_first` overlap (`protocol/starters/README.md` "Test vectors and
overlaps"): Brauser keeps Splunk apps ahead of Splunk dashboards on the
stack, so the built-in pages are recorded as app time, never as a
"dashboard" named `search`. Settings does not list this overlap for review
while both rules are as shipped; edit either rule and it is listed again.

The ID's literal `app` part is deliberate: the host names a container row
from its ID and links it as `<origin>/<title>` (§8.3), so the row reads
`app/<app>` and links to `https://<stack>.splunkcloud.com/app/<app>`, which
Splunk Web redirects to the locale-prefixed app.

## Titles

No title template. A container's title comes from its ID, never from one
page's tab title (§8.3), so tab titles such as `Search | Splunk
9.3.2411.104` are not parsed here.

## Recorded samples

Synthetic equivalents of the researched shapes, on `acme.splunkcloud.com`:

| URL | Tab title | Records |
|---|---|---|
| `/en-US/app/search` | none recorded | app `search` |
| `/en-US/app/search/search` | `Search \| Splunk 9.3.2411.104` | app `search` (rule 2, ahead of the dashboard rule) |

The blind research scored 40 synthetic vectors across this family's two
types (12 `splunk.app` positives among them), all correctly classified by
the research's own draft through both adapter-template engines, and
re-verified against this family's own fixture. They have not been graded
against real browsing history.

`samples_shape` is `false`: an app name is a `name`, not an identifier
capture, so the structural drafter cannot learn this shape.

## What the draft does not record

- Settings (`/<locale>/manager/…`), login (`/<locale>/account/…`), the
  splunkd proxy (`/<locale>/splunkd/…`) and static files
  (`/<locale>/static/…`): none has `app` as its second segment;
- URLs without a locale (`/app/…`): Splunk Web redirects them to a
  locale-prefixed URL, so a tab never stays on them;
- the `@go` job redirector (`/<locale>/app/<app>/@go?sid=…`): `@` is not a
  `name` character, and it redirects to a page that is recorded;
- reports, alerts and search jobs as artifacts of their own: a report's
  identity is a REST path with `/` and spaces in the `s` query value, which
  no capture type accepts, and a new search job ID appears on every run.
  Their pages count as app time instead.

## Known limits

- **The built-in view list is incomplete and partly from memory.** A missing
  built-in view, such as Enterprise Security's `incident_review`, is
  recorded as a dashboard. An app's own dashboard whose ID is one of the
  listed words (most often `home`) is recorded as app time instead.
- **Dashboards need apps.** Brauser refuses to save Splunk dashboards
  without Splunk apps ahead of them on the stack, or to remove apps while
  dashboards stay. Without the app rule, Search and the other built-in
  pages would be recorded as dashboards named `search`, `reports`, and so
  on, and adding apps later would move those records without a migration.
- **Prefixed search heads are not offered.** `labels: 1` claims
  `<stack>.splunkcloud.com` and one-label hosts such as
  `es-<stack>.splunkcloud.com`, but not `<prefix>.<stack>.splunkcloud.com`.
  Same-suffix hosts that are not Splunk Web, such as HEC's
  `http-inputs-<stack>.splunkcloud.com`, are claimed but serve none of
  these paths.
- **US Government stacks** (believed to be `*.splunkcloudgc.com`) and
  on-premises Splunk Enterprise are other origins. Enterprise with a
  `root_endpoint` adds a leading path segment these rules do not expect.
- **Removed, then added again.** After Splunk rules are removed, Capture
  Site cannot restore the recorded pair as it was, because the two rules
  overlap on purpose. It says so and offers the starter instead. The host
  checks the starter against the recorded identities, so existing notes keep
  theirs.
- **Non-web hosts on the suffix are claimed.** The starter is also offered
  on `https://<stack>.splunkcloud.com:8089` (the REST port) and other
  same-suffix hosts. None of them serve `/app/` pages, so nothing is
  recorded there.
- **A built-in view's `/edit` is a dashboard.** `/<locale>/app/<app>/search/edit`
  and other built-in views followed by `/edit` are recorded as a dashboard
  named after the view, in edit mode, because this rule has no mode slot.
  Splunk Web does not link to these paths.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Splunk Cloud stack:

1. Visit `/en-US/app/search` and record where it redirects; confirm the
   app is recorded once.
2. Open Search, Dashboards, Reports, Alerts, Datasets, Analytics, a report
   detail page, an alert detail page, Activity > Jobs and Splunk Home.
   Confirm each view name against rule 2's list, and add any missing
   built-in names (16 alternatives at most).
3. Open a dashboard; confirm its time also counts toward its app in Recent.
4. Open a container row's link (`https://<stack>.splunkcloud.com/app/<app>`)
   and confirm Splunk Web redirects it to the app.
5. If the stack has a premium (ES or ITSI) search head, note its hostname
   shape for the starter offer.
6. On a Capture Site review on a Splunk page with no saved rules, confirm
   Brauser offers both Splunk types, each with the development-draft source
   line; save both and confirm settings shows no overlap to review and the
   Search page's card shows "● Artifact recognized" as a Splunk app.

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `splunk.app`'s
`status` in `protocol/starters/splunk.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
