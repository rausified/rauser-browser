# servicenow.kb starter draft

Status: development draft. The URL rules come from
`docs/research/blind/servicenow/`, researched blind: public sources only
(ServiceNow Community threads and the public Raycast ServiceNow extension
source), no browsing history, cookies or a ServiceNow account.

History check (2026-11-09): graded against a held-back sample of real
browsing history (real URLs from eight researched sites): 4 real URLs
(11 visits) matched, 2/3 rules confirmed, the recorded title right on
2/4. Not every rule and title was confirmed, so this type stays a
development draft.

Brauser offers this draft on a customer instance, `<instance>.service-now.com`
(the `servicenow` family's strict subdomain origin, `exclude: ["www"]`),
alongside incidents, requested items and change requests. Any combination
can be chosen in Capture Site and in the settings gallery. The draft records
one artifact per knowledge article:

`servicenow.kb:<instance>.service-now.com/<article number>`

The ID is the article number (`KB0010035`), not a sys_id. The number stays
the same across article versions and always opens the latest version the
user may read, while each version has its own sys_id. So two visits to the
same article, before and after a new version is published, stay one
artifact.

```toml
[[artifacts]]
type   = "servicenow.kb"
origin = "https://acme.service-now.com"
match  = [
  { path = "/kb_view.do", query = { sysparm_article = "{number:token(3,40)|upper}" } },
  { path = "/{_}", query = { id = "kb_article", sysparm_article = "{number:token(3,40)|upper}" }, absent = ["sys_id"] },
  { path = "/{_}", query = { id = "kb_article_view", sysparm_article = "{number:token(3,40)|upper}" }, absent = ["sys_id"] },
]
id     = "servicenow.kb:{host}/{number}"
```

## What each rule covers

1. **Platform article view**, `/kb_view.do?sysparm_article=<number>`. A
   trailing slash (`/kb_view.do/`) and extra parameters such as
   `sysparm_language` are ignored.
2. **Portal article page, `kb_article`**, on any portal: the default
   Service Portal (`/sp`), Employee Center (`/esc`), Knowledge portal
   (`/kb`), or a portal the instance has renamed, such as `/helpzone`, with
   `?id=kb_article&sysparm_article=<number>`. The portal path is `{_}`, one
   segment of any name. The rule is anchored by its query
   (`adapter-templates.md` §4.1): the literal `id=kb_article` routes the page
   and the `sysparm_article` capture names the article.
3. **Portal article page, `kb_article_view`**, the same on any portal with
   `?id=kb_article_view&sysparm_article=<number>`. Search-ranking noise
   (`sysparm_rank`, `sysparm_tsqueryId`) and the fragment are ignored.

Both portal rules require `sys_id` to be absent. That `absent` key proves
the portal rules disjoint from the incident, requested item and change
classic forms (`/incident.do?sys_id=…`), whose one-segment paths `{_}`
covers; without it the family's overlap report would list six pairs. So a
portal link that carries a `sys_id` as well as the number, such as
`/sp?id=kb_article&sysparm_article=KB0010035&sys_id=<sys_id>`, is not
recorded.

A portal link that carries the number and a `sys_kb_id`, such as
`/esc?id=kb_article_view&sysparm_article=KB0010035&sys_kb_id=<sys_kb_id>`,
is recorded as the article its number names, the same way rule 1 records
`kb_view.do?sysparm_article=…&sys_kb_id=…`. The ID is the number, which
covers every version of the article, so recording such a link never splits
one article across two IDs, whichever version the page shows. No classic
form requires `sys_kb_id`, so the overlap proof does not need it. (An
earlier draft also required `sys_kb_id` to be absent; that left these
links unrecorded on the portals, which the classic form records.)

The number capture is `token(3,40)|upper`. KB prefixes are configurable per
instance (`HRKB0001001` is recorded too), and `|upper` folds `kb0010035`
into `KB0010035`, on the assumption that number lookup is case-insensitive.

## Titles

No title template. The tab titles of `kb_view.do` and of the portal article
pages are not documented in any public source the research found, so
Brauser records the whole tab title.

## Recorded samples

Synthetic equivalents of the recorded shapes, on `acme.service-now.com`:

| URL | Tab title | Records |
|---|---|---|
| `/esc?id=kb_article_view&sysparm_article=KB0012345` | (not known) | `KB0012345`, with the whole tab title |

Graded against real, held-back browsing (`docs/research/blind/SCORECARD.md`),
the research matched **0 of 4** ServiceNow URLs. All four were knowledge
articles on customer-renamed portal paths: the instance served its portal
under its own names, not `/sp`, `/esc` or `/kb`. The portal rules now take
any one-segment portal path, so this draft records that form: synthetic
`/helpzone?id=kb_article_view&sysparm_article=KB0012345` and
`/policyhub?id=kb_article&sysparm_article=KB0012345` are vectors that match.

`samples_shape` is `false`: the article is routed by the query, so the
structural drafter's path shapes cannot tell article pages from other portal
pages.

## What the draft does not record

- version links by sys_id: `kb_view.do?sys_kb_id=…`,
  `?id=kb_article_view&sys_kb_id=…`, `?id=kb_article&sys_id=…`,
  `/now/sow/kb_view/kb_knowledge/<sys_id>` and `kb_knowledge.do?sys_id=…`.
  Each names one version, and one adapter cannot build the number-based ID
  from it, so recording them would split one article across two IDs. A
  portal link with both the number and a `sys_id` is not recorded either
  (above);
- knowledge home and search, on any portal: `?id=kb_home`, `?id=kb_search`,
  `kb_find.do`;
- a portal path of more than one segment (`/helpzone/extra?id=kb_article&…`);
- a portal article page with no `sysparm_article`, a duplicated
  `sysparm_article`, or a value containing `/`.

## Known limits

- The portal rules accept any one-segment path with `id=kb_article` or
  `id=kb_article_view`. A non-portal page that carries those parameters
  would be recorded as the article it names (low risk: the parameters are
  the portal's own page selector).
- `token` cannot require the `KB` prefix, so any `sysparm_article` value on
  these routes is accepted.
- `|upper` assumes number lookup is case-insensitive (low confidence).
- Articles opened inside the Next Experience shell through
  `/now/nav/ui/classic/params/target/…` or `nav_to.do?uri=…` are not
  recorded: the target is a nested, encoded URL, which the format cannot
  match.
- Instances on `servicenowservices.com` (US government cloud) and custom
  domains are not offered this starter.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real ServiceNow account:

1. Open an article from `kb_view.do`, from a portal search result and from a
   shared link; record each URL and tab title. Is the number always in
   `sysparm_article`, or do search results link by `sys_kb_id`?
2. Note the portal path the instance uses, and confirm an article opened
   there is recorded whether or not the path is `/sp`, `/esc` or `/kb`.
   Check whether an article link ever carries a `sys_id` or `sys_kb_id`
   beside `sysparm_article`; if it does, note which version the page shows
   (a `sys_kb_id` one records as the article's number).
3. Open a lowercase `sysparm_article` link; confirm the instance opens the
   same article (the `|upper` assumption).
4. Publish or find a second version of one article; confirm both visits
   record one ID.
5. Record the tab title of an article page, and decide whether a title
   template is worth adding.
6. On a Capture Site review on an article page with no saved rules, confirm
   Brauser offers all four ServiceNow types, each with its own statement and
   the development-draft source line; confirm saving one turns the page
   card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `servicenow.kb`'s
`status` in `protocol/starters/servicenow.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
