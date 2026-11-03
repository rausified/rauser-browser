# Splunk Cloud Platform: blind starter research

Date: 2026-11-03. This is a blind test, built only from public, logged-out sources and this repository. I did not use any Splunk account, browser history or live stack. Every sample URL and title in `vectors.json` is synthetic.

Files: `adapters.toml` (the adapter document), `vectors.json` (40 synthetic cases), `validate.py` and `validate-rs/` (throwaway validators, not product code).

## Summary

| Type | What it records | URL forms | ID | Confidence |
|---|---|---|---|---|
| `splunk.dashboard` | Classic (Simple XML) and Dashboard Studio dashboards, plus any other app view not on the built-in list | `/<locale>/app/<app>/<id>`, plus `/edit` | `splunk.dashboard:<host>/<app>/<id>` | URL and ID: medium-high. Edit URL: medium for Classic, low for Studio. Title: low |
| `splunk.app` | An app as a container for its dashboards, plus the app root and its built-in pages (Search, listing pages, report and alert detail, job manager, add-on configuration) | `/<locale>/app/<app>`, `/<locale>/app/<app>/<built-in view>` | `splunk.app:<host>/app/<app>` | Medium. The built-in list is the weakest part |

Not covered: reports, alerts, search jobs, datasets, Settings, login. See "Excluded" below.

## Sources consulted

Public Splunk documentation (help.splunk.com, Splunk Cloud Platform 10.7). docs.splunk.com returned HTTP 403 to logged-out fetches, so I used help.splunk.com, found through its public `sitemap.xml`:

- Simple XML, "Link to a dashboard": https://help.splunk.com/en/splunk-cloud-platform/create-dashboards-and-reports/simple-xml-dashboards/10.7/drilldown-and-dashboard-interactivity/link-to-a-dashboard. It shows `<link target="_blank">/app/search/customer_purchase_removal_map</link>` and the form `[relative path]/[dashboard or form id]?form.[target_token_name]=[$source_value$]&earliest=$earliest$&latest=$latest$`.
- Simple XML, "Token usage in dashboards": https://help.splunk.com/en/splunk-cloud-platform/create-dashboards-and-reports/simple-xml-dashboards/10.7/drilldown-and-dashboard-interactivity/token-usage-in-dashboards. It shows `/app/foursquare_vegas/vegas_badge_1?form.badge=$click.value2$`.
- Simple XML, "Link to a search": https://help.splunk.com/en/splunk-cloud-platform/create-dashboards-and-reports/simple-xml-dashboards/10.7/drilldown-and-dashboard-interactivity/link-to-a-search. It shows `search?q=index=_internal | stats count by sourcetype`, so the search page is a view named `search`.
- Simple XML, "Create a dashboard": https://help.splunk.com/en/splunk-cloud-platform/create-dashboards-and-reports/simple-xml-dashboards/10.7/build-and-edit-dashboards-in-splunk-web/create-a-dashboard. It says "Each dashboard requires a unique ID, but titles do not have to be unique."
- Dashboard Studio, "Create a dashboard in Dashboard Studio": https://help.splunk.com/en/splunk-cloud-platform/create-dashboards-and-reports/dashboard-studio/10.7/create-a-dashboard-in-dashboard-studio/create-a-dashboard-in-dashboard-studio. It says: "By default, the dashboard ID is the same as the dashboard title. You can edit the dashboard title at any time, but you can't change the dashboard ID later." and "Dashboards in different apps, however, can share both titles and IDs."
- Dashboard Studio, "Convert a Simple XML dashboard": https://help.splunk.com/en/splunk-cloud-platform/create-dashboards-and-reports/dashboard-studio/10.7/introduction-to-splunk-dashboard-studio/convert-a-simple-xml-dashboard-to-a-dashboard-studio-dashboard. Converting means "Clone in Dashboard Studio", which keeps the original, so a converted dashboard is a new artifact.
- Dashboard Studio, "Publish dashboards": https://help.splunk.com/en/splunk-cloud-platform/create-dashboards-and-reports/dashboard-studio/10.7/share-dashboard-content/publish-dashboards. Published dashboards are backed by a saved search named `_SnapshotView__<dashboard ID>`. The page does not give the published URL format.
- Dashboard Studio, "Apply view mode settings": https://help.splunk.com/en/splunk-cloud-platform/create-dashboards-and-reports/dashboard-studio/10.7/create-a-dashboard-in-dashboard-studio/create-a-dashboard-in-dashboard-studio/apply-view-mode-settings-for-dashboards. It mentions View and Edit mode and "dashboards that hide the navigation bars using URL parameters", but gives no URL syntax.
- Search Manual, "About jobs and job management" and "Share jobs and export results": https://help.splunk.com/en/splunk-cloud-platform/search/search-manual/10.7/manage-jobs/about-jobs-and-job-management and https://help.splunk.com/en/splunk-cloud-platform/search/search-manual/10.7/manage-jobs/share-jobs-and-export-results. They say: "The default lifetime is 10 minutes", and sharing extends it "to 7 days". A job link is a URL to the job. The pages give no URL example.
- Reporting and Alerting manuals: https://help.splunk.com/en/splunk-cloud-platform/create-dashboards-and-reports/reporting-manual/10.7/report-management/create-and-edit-reports, .../embed-scheduled-reports, and https://help.splunk.com/en/splunk-cloud-platform/alert-and-respond/alerting-manual/10.7/configure-alert-actions/use-tokens-in-email-notifications (`$results_link$`, `$view_link$`). None of them gives the report URL format.
- ACS, "Target a specific search head": https://help.splunk.com/en/splunk-cloud-platform/administer/admin-config-service-manual/10.7/using-the-admin-config-service-acs--api/target-a-specific-search-head-for-acs-operations. It covers search head prefixes (`sh-i-0910d0dfdb9ed913a`, `shc1`), premium search heads, and the stack name in the URL.

Public code:

- npm `@splunk/splunk-utils` 4.1.0 (`url.js`, `config.js`): `createURL('app/search/job')` gives `"/splunk/en-US/app/search/job"`. Its comments show `http://splunk.domain.com/my-root-dir/en-US/app/search/search` and say "Splunk may not be running in the root directory, so the locale may not be the first element". The locale segment is confirmed, and so is the on-premises `root_endpoint` prefix.
- npm `@splunk/react-page` 8.3.2: React pages set `document.title` from the caller's `pageTitle`. It gives no evidence of a common title suffix.
- github.com/splunk/addonfactory-ucc-generator, downloaded as a public codeload tarball: UI tests open `{splunk_web_url}/en-US/app/{ADDON_NAME}/configuration` and `/inputs`. These are the two pages every UCC-generated add-on has.
- github.com/splunk/splunk-app-examples: it has `/en-US/app/setup-example-app/search`.
- github.com/splunk/docker-splunk: Splunk Web runs on port 8000 and splunkd on 8089, with an optional `root_endpoint`.

I tried web search (DuckDuckGo, Bing), Splunk Community search and Stack Overflow, and none of them gave usable evidence. The search engines served bot or generic pages, Splunk Community returned 403, and the Stack Overflow API had no relevant URL examples. So no claim below rests on a forum post. Claims marked "from memory" come from my general knowledge of Splunk Web. I could not confirm them from a fetched public source this session.

## Origin model

**A tenant subdomain family: `https://<stack>.splunkcloud.com`, plus prefixed search-head hosts under the same suffix.** Offer it per exact origin, the way `*.atlassian.net` is offered in `extension/starter-adapters.ts` (`host.endsWith(".splunkcloud.com") && host !== "splunkcloud.com"`).

- Each stack is one customer, so the origin is not shared between tenants. It is not on `SHARED_ORIGINS` and needs no `scope` or `unscoped`.
- Additional and premium search heads (Enterprise Security, ITSI, standalone `sh-i-…`, `shc<N>`) have their own hostnames under `splunkcloud.com`. The ACS page shows the prefixes, but the exact Splunk Web hostname form, such as `es-<stack>.splunkcloud.com` or `<prefix>.<stack>.splunkcloud.com`, is from memory and medium confidence. Each search head (or cluster) has its own apps and knowledge objects, so `{host}` in the ID is correct: the same app and dashboard ID on two search heads can be two different dashboards.
- Same-suffix hosts that are not Splunk Web, such as HEC (`http-inputs-<stack>.splunkcloud.com`) and the REST port `:8089`, never serve these paths, so the family test can stay simple. The vectors check that `:8089` is a separate origin.
- Out of scope: `admin.splunk.com` (ACS API), `login.splunk.com` / `splunk.com` (vendor account), and US Government stacks (believed to be `*.splunkcloudgc.com`, low confidence). Splunk Enterprise on premises can run the same adapter on its own origin (often `:8000`), but only with `root_endpoint` unset. With a root endpoint, every path gains a leading segment, and the templates would need a literal for it.

## Artifact types

### `splunk.dashboard`

URL forms:

| Form | Example (synthetic) | Confidence | Evidence |
|---|---|---|---|
| View | `/en-US/app/search/web_traffic_overview` | High for Classic, medium-high for Studio | Classic: drilldown docs (`/app/search/customer_purchase_removal_map`) plus the locale prefix from `@splunk/splunk-utils`. Studio: the docs describe Studio dashboards as app-scoped IDs like Classic ones. I found no Studio URL example, but from memory Studio uses the same route |
| View with tokens and time | `…/web_traffic_overview?form.host=web-01&earliest=-24h@h&latest=now` | High | Drilldown docs. Query parameters are ignored by the rule |
| Trailing slash, fragment | `…/web_traffic_overview/`, `…#panel3` | High (engine behaviour) | §5.1 |
| Edit | `…/web_traffic_overview/edit` | Medium for Classic (from memory), low for Studio | No public example found. If Studio edits under another URL, the view rule still records the visit, just without `mode = edit` |
| Locale | `/ja-JP/app/…`, `/en-GB/app/…` | High | `@splunk/splunk-utils` `locale`. The locale is `{_}` and stays out of the ID |

ID: `splunk.dashboard:{host}/{app}/{view}`. Confidence: high that `(app, id)` names a dashboard on a search head. Splunk documents that the ID cannot change while the title can, and that IDs are unique only within an app. Known exceptions:

- Two users' private dashboards in one app can have the same ID. Splunk resolves `/app/<app>/<id>` per user, so two people would map to one artifact ID. Brauser is single-user, so in practice this means one user's view of that URL.
- Moving a dashboard to another app (Settings > User interface > Views > Move) changes its URL and so its ID. Cloning or converting to Studio creates a new dashboard and a new ID, which is correct.

Dashboard IDs: Splunk Web restricts them to letters, digits, `_` and `-` (from memory, medium). That fits `name`. App names are directory names (`Splunk_SA_CIM`, `TA-foo`), also `name`. Case is preserved, not folded, because app directories are case-sensitive on Splunk's Linux hosts (medium).

Title: `["{title} | Splunk {~}", "{title} | Splunk"]`. **Low confidence.** From memory, Splunk Web tab titles are `<page title> | Splunk <version>`, for example `Search | Splunk 9.1.2`. On Splunk Cloud the version would be a cloud build such as `9.3.2411.104`. I found no public source that shows a tab title. Possible variants I could not check: Dashboard Studio titles (Studio is a React page, and `@splunk/react-page` sets `document.title` to whatever the page passes), edit-mode titles, and stacks that hide the version. If no entry matches, Brauser records the whole tab title, so a wrong guess degrades but loses nothing.

`within = splunk.app:{host}/app/{app}`, so dashboard time also rolls up into its app (§8.3).

### `splunk.app`

| Rule | Example | Confidence |
|---|---|---|
| App root | `/en-US/app/search` | High (route). From memory, Splunk opens the app's default view, often redirecting to it |
| Built-in views | `search`, `dashboards`, `reports`, `alerts`, `datasets`, `report`, `alert`, `dataset`, `job_manager`, `analytics_workspace`, `pivot`, `field_extractor`, `home`, `configuration`, `inputs` | `search`: high (docs). `configuration` and `inputs`: high (UCC tests). The rest: from memory, medium (`dashboards`, `reports`, `alerts`, `report`, `alert`, `job_manager`, `home`) to low (`dataset`, `datasets`, `analytics_workspace`, `pivot`, `field_extractor`) |

ID: `splunk.app:{host}/app/{app}`. The literal `app` is there so the host's container title and link (§8.3: title = ID after the host, link = origin + `/` + title) come out as `app/search` and `https://<stack>.splunkcloud.com/app/search`. Splunk Web redirects that link to the locale-prefixed app (from memory, medium-high).

Why this adapter exists: dashboards and built-in pages share `/app/<app>/<view>`, and format 1 has no negation. Without it, the Search page, the busiest page in Splunk, would be recorded as a "dashboard" named `search`. The built-in list must be tried before `splunk.dashboard`, so the engine's overlap report flags `splunk.app` rule 1 against `splunk.dashboard` rule 0. That pair is the "reverse order" case §10.3 still reports, and it is intended.

Trade-off: an app's own dashboard whose ID is one of the built-in words (most often `home`, sometimes `search` or `configuration`) is recorded as app time, not as a dashboard.

## Excluded, and why

- **Reports and alerts.** From memory, medium-high: report and alert detail pages are `/<locale>/app/<app>/report?s=<encoded REST path>` and `/alert?s=…`, where the decoded value is `/servicesNS/<owner>/<app>/saved/searches/<name>`. Email `$view_link$` and `$results_link$` links use `/app/<app>/@go?s=…` and `@go?sid=…`. The `s` value contains `/` and usually spaces, and no format-1 capture type accepts either, so a report ID cannot be expressed. These pages are recorded as `splunk.app` visits. The `@go` redirector is not recorded at all, but it redirects to a page that is. **This is a gap in the format, not a choice.** A "REST path" capture, or a query capture split on `/` into typed parts (owner, app, name with a percent-decoded text type), would make `splunk.report:{host}/{app}/{name}` possible.
- **Search jobs (`sid`).** A sid is capturable as `name` (`1700000000.12345`, `scheduler__nobody__search__RMD5…_at_1700000000_42`). But Splunk writes a new sid into the search page URL every time a search runs (from memory, high), and jobs expire after 10 minutes by default (docs). A job adapter would create one short-lived artifact per ad-hoc search. Search pages are recorded as app time instead.
- **Datasets, data models, pivots, lookups.** These are niche for a business user. The listing pages fall into `splunk.app`.
- **Published Dashboard Studio dashboards.** The URL format is not documented in what I could read. Add them once a sample is seen.
- **Settings** (`/<locale>/manager/…`), **login** (`/<locale>/account/…`), the **splunkd proxy** (`/<locale>/splunkd/__raw/…`) and **static assets** (`/<locale>/static/…`). None of these has `app` as its second segment, so neither adapter matches them (covered in the vectors).
- **URLs without a locale** (`/app/…`). Splunk Web redirects these to `/<locale>/app/…` (from memory, high), so a tab never stays on them. Adding rules for them would double the rule count for no recorded visit.

## Validation

Both of the repo's engines, run on Node 24.14.0 and Rust 1.98.1:

```sh
cd /Users/jrauser/ENDURANCE/work/rb-blind-starters
export PATH=~/.nvm/versions/node/v24.14.0/bin:~/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
(cd research/blind/splunk-cloud/validate-rs && cargo build --offline --quiet)   # own ./target inside validate-rs; deleted after the run
python3 -I research/blind/splunk-cloud/validate.py
```

`validate.py` converts the TOML with `tomllib` to the engine document `{format, artifacts}`. It then runs every vector through `scripts/adapter-templates-eval.mjs`, the TypeScript twin from the existing `extension/dist` build, and through `validate-rs`, a 50-line binary that uses the `adapter-templates` crate by path: `Document::from_value`, `evaluate` and `overlap_report`. It checks each result against its expectation and the two engines against each other. Result on 2026-11-03:

```
rust: {"adapters": 2, "valid": true}
rust: {"overlap": {"first": ["splunk.app", 1], "second": ["splunk.dashboard", 0]}}
splunk.dashboard: 12 passed, 0 failed
splunk.app: 12 passed, 0 failed
negative: 16 passed, 0 failed
engines agree on every case
```

The single overlap is the intended one described above. The vectors are synthetic, so passing them shows the rules do what this document says, not that Splunk behaves as described.

## Known limits and open questions

1. **Tab titles are unverified** (low). This is the biggest open item.
2. **Dashboard Studio edit URL** (low): `/edit`, a query flag, or something else.
3. **The built-in view list** is incomplete and partly from memory. A missing built-in view, especially from premium apps such as Enterprise Security's `incident_review` or ITSI's service analyzer, is recorded as a "dashboard". In those apps that is often acceptable, because many of their pages are dashboards.
4. **Premium search-head hostnames** (medium). They only affect how the starter is offered, not the rules.
5. **Private same-ID dashboards** and **moved dashboards**: see the ID section.
6. **Possible engine idea** (not used): a scope miss is terminal (§11 known limits). A "sink" adapter with an impossible scope could match and drop routes such as `/app/<app>/search` without recording them. That relies on an implementation detail, so I used the honest app container instead.

## How I would verify with a real account

1. Open a Classic dashboard and a Dashboard Studio dashboard in `search` and in a custom app. Copy the address bar in view mode, after **Edit**, after changing an input (the `form.*` parameters), and after switching a Studio tab.
2. Record the exact tab title in each state: view, edit, while loading, and with an input changed. Check whether the ` | Splunk <version>` suffix is present on Splunk Cloud and in Studio.
3. Visit `/en-US/app/search` and `/app/search/<dashboard>`, and record the redirect targets.
4. Open Search, Dashboards, Reports, Alerts, Datasets, Analytics, a report detail page, an alert detail page, Activity > Jobs, and Splunk Home. Confirm each view name against the `splunk.app` alternation, and add any missing built-in names (16 alternatives maximum).
5. Open a report's "Open in Search" and a shared job link. Confirm the `s=` and `sid=` forms, and confirm that a new sid appears on every run.
6. Create a dashboard whose ID contains `-` and uppercase letters. Confirm that the URL keeps the case, and whether the opposite case resolves to the same dashboard. If it does, consider `|lower` before shipping, because the ID is frozen.
7. If the stack has a premium (ES or ITSI) search head, note its hostname shape for the starter offer.
8. Publish a Studio dashboard and record its URL.
