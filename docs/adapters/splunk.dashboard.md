# splunk.dashboard starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/splunk-cloud/`, researched blind: public sources only,
no browsing history, cookies or a Splunk account.

History check (2026-11-09): graded against a held-back sample of real
browsing history (real URLs from eight researched sites): 5 real URLs
(10 visits) matched, 1/1 rules confirmed, the recorded title right on
4/5. Not every rule and title was confirmed, so this type stays a
development draft.

Brauser offers this draft on a Splunk Cloud stack's own host,
`<stack>.splunkcloud.com` (the `splunk` family's strict subdomain origin,
`labels: 1`, `exclude: ["www"]`), alongside Splunk apps. Either or both can
be chosen in Capture Site and in the settings gallery. The draft records
one artifact per Classic (Simple XML) or Dashboard Studio dashboard:

`splunk.dashboard:<stack>.splunkcloud.com/<app>/<dashboard ID>`

Splunk documents that a dashboard's ID cannot change after creation while
its title can, and that dashboards in different apps may share an ID, so
the app is part of the ID. With Splunk apps chosen too, the dashboard sits
`within` its app (`adapter-templates.md` §8.3), and its time also counts
toward the app.

```toml
[[artifacts]]
type   = "splunk.dashboard"
origin = "https://acme.splunkcloud.com"
match  = [
  { path = "/{_}/app/{app:name}/{view:name}/{mode?:(edit)}" },
]
modes  = { edit = "edit" }
id     = "splunk.dashboard:{host}/{app}/{view}"
title  = ["{title} | Splunk {~}", "{title} | Splunk"]
generic_titles = ["Dashboard"]
within = { type = "splunk.app", id = "splunk.app:{host}/app/{app}" }
```

`generic_titles` lists the titles shown by Splunk's app shell while a
dashboard loads instead of the item's own name (`adapter-templates.md` §9.4).
A visit whose title is one of them, ASCII case aside, or is empty or the bare
host, records no title, so the artifact keeps the title it already has
("Untitled" for a new one).

## What each rule covers

1. **Dashboard**, `/<locale>/app/<app>/<dashboard ID>`, in view mode, and
   `/<locale>/app/<app>/<dashboard ID>/edit` in edit mode (`mode = edit`).
   Drilldown and token parameters (`?form.<token>=…`, `earliest=`,
   `latest=`), Dashboard Studio tab and display parameters (`tab=`,
   `hideChrome=`) and fragments never change the ID.

The locale is `{_}`, not part of the ID. App names and dashboard IDs keep
their case. A view name containing `@`, `:` or a space is not a `name` and
is not recorded, and `/<dashboard ID>/<anything but edit>` never matches.

**This rule also matches Splunk's own app pages,** such as
`/en-US/app/search/search`, because the template language has no "any word
but these" segment. The family declares that overlap with Splunk apps'
built-in-page rule as `container_first` (`protocol/starters/README.md`
"Test vectors and overlaps"): Brauser keeps Splunk apps ahead of this
adapter on the stack, so those pages are recorded as app time. Settings
does not list this overlap for review while both rules are as shipped.

## Titles

Two entries, tried in order:

1. `{title} | Splunk {~}` drops ` | Splunk ` and whatever follows, such as
   a version (`Web Traffic Overview | Splunk 9.3.2411.104` records `Web
   Traffic Overview`). A dashboard title that itself contains ` | Splunk `
   keeps everything before the last occurrence.
2. `{title} | Splunk` covers a title with no version.

A title matching neither entry, such as `Loading...`, is kept whole
(`title_entry` null). While a dashboard loads, its tab can read `Dashboard |
Splunk <version>`; `Dashboard` is listed in `generic_titles`, so that visit
records no title. A dashboard really named `Dashboard` is therefore never
titled by Brauser, and keeps "Untitled" until it is renamed. **Low confidence:** the ` | Splunk <version>` suffix
is from memory, not seen on a live stack or in Dashboard Studio.

## Recorded samples

Synthetic equivalents of the researched shapes, on `acme.splunkcloud.com`:

| URL | Tab title | Records |
|---|---|---|
| `/en-US/app/search/web_traffic_overview` | `Web Traffic Overview \| Splunk 9.3.2411.104` | `search/web_traffic_overview`, title `Web Traffic Overview` |
| `/en-US/app/search/web_traffic_overview/edit` | `Web Traffic Overview \| Splunk 9.3.2411.104` | the same ID in edit mode |

The blind research scored 40 synthetic vectors across this family's two
types (12 `splunk.dashboard` positives among them), all correctly
classified by the research's own draft through both adapter-template
engines, and re-verified against this family's own fixture. They have not
been graded against real browsing history.

`samples_shape` is `false`: a dashboard ID is a `name`, not an identifier
capture, so the structural drafter cannot learn this shape.

## What the draft does not record

- Splunk's built-in app pages (Search, listing pages, report and alert
  detail, the job manager, add-on setup): Splunk apps record those;
- published Dashboard Studio dashboards, whose URL form is not documented
  in the sources read;
- Settings > User interface > Views (`/<locale>/manager/…`) and the splunkd
  proxy, which name views but are not dashboards;
- URLs without a locale (`/app/…`), which Splunk Web redirects.

## Known limits

- **Choose with Splunk apps.** Alone, this type records Search and the
  other built-in pages as dashboards named `search`, `reports`, and so on,
  so Capture Site offers it only with Splunk apps. Once saved, the two keep
  their order; see `docs/adapters/splunk.app.md`.
- **The built-in view list is Splunk apps'.** A built-in page missing from
  it, such as Enterprise Security's `incident_review`, is recorded as a
  dashboard; a dashboard whose ID is a listed word (most often `home`) is
  recorded as app time.
- **Dashboard Studio's edit URL is unverified** (low confidence). If Studio
  edits under another URL, the view rule still records the dashboard,
  without a mode.
- **Two users' private dashboards with one ID in one app** map to one
  artifact ID; Brauser is single-user, so in practice this is one user's
  view of that URL. Moving a dashboard to another app changes its ID.
- **A built-in view's `/edit` path is recorded here**; see
  `docs/adapters/splunk.app.md`.
- **Prefixed search heads** (`<prefix>.<stack>.splunkcloud.com`) are not
  offered under `labels: 1`; see `docs/adapters/splunk.app.md`.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Splunk Cloud stack:

1. Open a Classic dashboard and a Dashboard Studio dashboard, in `search`
   and in a custom app. Copy the address bar in view mode, after **Edit**,
   after changing an input (the `form.*` parameters), and after switching a
   Studio tab.
2. Record the exact tab title in each state: view, edit, while loading,
   and with an input changed. Check whether the ` | Splunk <version>`
   suffix is present on Splunk Cloud and in Studio.
3. Create a dashboard whose ID contains `-` and uppercase letters. Confirm
   that the URL keeps the case, and whether the opposite case resolves to
   the same dashboard; if it does, consider `|lower` before certifying,
   because the ID is frozen.
4. Publish a Studio dashboard and record its URL.
5. On a Capture Site review on a dashboard with no saved rules, confirm
   Brauser offers both Splunk types, each with the development-draft source
   line; save both and confirm the page card shows "● Artifact recognized"
   as a Splunk dashboard, and that the Search page is recorded as its app.

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `splunk.dashboard`'s
`status` in `protocol/starters/splunk.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
