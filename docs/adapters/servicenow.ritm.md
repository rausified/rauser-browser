# servicenow.ritm starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/servicenow/`, researched blind: public sources only
(ServiceNow Community threads and the public Raycast ServiceNow extension
source), no browsing history, cookies or a ServiceNow account.

Brauser offers this draft on a customer instance, `<instance>.service-now.com`
(the `servicenow` family's strict subdomain origin, `exclude: ["www"]`),
alongside incidents, change requests and knowledge articles. Any
combination can be chosen in Capture Site and in the settings gallery. The
draft records one artifact per requested item (RITM), however it was opened:

`servicenow.ritm:<instance>.service-now.com/<sys_id>`

The ID is the record's 32-hex sys_id, unique within the instance and carried
by every record URL form. The human number (`RITM0010001`) appears in URLs
only in list queries, so it cannot be the ID.

```toml
[[artifacts]]
type   = "servicenow.ritm"
origin = "https://acme.service-now.com"
match  = [
  { path = "/sc_req_item.do", query = { sys_id = "{id:hex(32)}" }, absent = ["XML", "PDF", "CSV", "EXCEL", "XLSX", "JSON", "JSONv2", "UNL"] },
  { path = "/now/{_}/record/sc_req_item/{id:hex(32)}/{*}" },
  { path = "/now/{_}/{_}/record/sc_req_item/{id:hex(32)}/{*}" },
  { path = "/(sp|esc)", query = { id = "ticket", table = "sc_req_item", sys_id = "{id:hex(32)}" } },
  { path = "/(sp|esc)", query = { id = "standard_ticket", table = "sc_req_item", sys_id = "{id:hex(32)}" } },
  { path = "/(sp|esc)", query = { id = "form", table = "sc_req_item", sys_id = "{id:hex(32)}" } },
]
id     = "servicenow.ritm:{host}/{id}"
title  = "{title} | Requested Item | {~}"
```

## What each rule covers

1. **Classic form opened directly**, `/sc_req_item.do?sys_id=<sys_id>`. The
   `absent` keys skip the platform's export processors, which reuse the
   same URL (`?XML`, `?PDF`, `?CSV`, …). The new-record form
   (`sys_id=-1`) fails `hex(32)` and is not recorded.
2. **Workspace record page with one workspace segment**, such as Service
   Operations Workspace, `/now/sow/record/sc_req_item/<sys_id>`. The trailing
   `{*}` admits the workspace's `/params/…` and sub-tab segments.
3. **Workspace record page with two segments**: the legacy Agent Workspace
   (`/now/workspace/agent/record/sc_req_item/<sys_id>`) and the CSM/FSM
   configurable workspace (`/now/cwf/agent/record/sc_req_item/<sys_id>`).
4. **Portal ticket page**, `/sp` or `/esc` with
   `?id=ticket&table=sc_req_item&sys_id=<sys_id>`. The `table` literal keeps
   other tables out.
5. **Portal `standard_ticket` page**, the same with `id=standard_ticket`.
6. **Portal form page**, the same with `id=form`, in any parameter order.

## Titles

One entry: `{title} | Requested Item | {~}`. Platform record forms title the tab
`<display value> | <table label> | <product name>`, for example
`RITM0010001 | Requested Item | ServiceNow`. The display value of task tables is
the number by default, so the recorded title is the number, not the short
description. The product-name suffix is configurable per instance, hence
`{~}`, and a suffix that itself contains ` | ` is still skipped from the
end.

Workspace and portal tab titles are not documented, so no entry is guessed
for them and Brauser records the whole tab title there.

## Recorded samples

Synthetic equivalents of the recorded shapes, on `acme.service-now.com`:

| URL | Tab title | Records |
|---|---|---|
| `/now/sow/record/sc_req_item/<32 hex>` | `RITM0010001 \| Requested Item \| ServiceNow` | that sys_id, title `RITM0010001` |

The scorecard (`docs/research/blind/SCORECARD.md`) saw no requested item visits in
the held-back browsing, so this type's rules are graded only by the
research's synthetic vectors, re-verified against this family's own fixture
through both adapter-template engines.

`samples_shape` is `false`: the in-page shaper keeps a path segment as a
route word only when it is at most 20 characters of `a-z`, `-` and `_` with
at most one `-` or `_` (`literal` in `inspectSitePatterns`,
`extension/site-patterns.ts`). `sc_req_item` has two underscores, so the
sample's shape is `/now/sow/record/{slug}/{hex32}`: the table name becomes a
`{slug}` placeholder, a rule drafted from that shape would not keep the
table literal, and the shape cannot tell requested items from any other
workspace record. `incident` and `change_request` (one underscore) stay
route words, so those samples keep their table names and stay `true`.

## What the draft does not record

- lists (`sc_req_item_list.do`) and records opened by number query
  (`sc_req_item.do?sysparm_query=number=…`), which carry no sys_id;
- the new-record form (`sys_id=-1`) and workspace new-record placeholders
  (`/now/sow/record/sc_req_item/-1_uid_3`);
- export processors (`?XML`, `?PDF`, `?CSV`, …) and a duplicated `sys_id`;
- portal ticket pages without a `sys_id`, or for another table;
- the parent request (`sc_request`, REQ): a requested-item URL does not carry
  the request's sys_id, so the two cannot be linked;
- incidents, cases and other tables.

## Known limits

- **Classic-UI visits inside the Next Experience shell are not recorded.**
  The address bar shows
  `/now/nav/ui/classic/params/target/sc_req_item.do%3Fsys_id%3D<sys_id>` or
  `/nav_to.do?uri=sc_req_item.do%3Fsys_id%3D<sys_id>`. The record is inside one
  encoded segment or query value, and the format cannot match a nested URL.
  This is the biggest gap for users who open records from classic lists.
- **Ticket pages on a customer-renamed portal are not recorded.** The portal
  rules list only `/sp` and `/esc`; an instance that serves its portal under
  its own path (`/helpzone?id=ticket&table=sc_req_item&sys_id=…`) gets no match,
  and a vector (`eval.servicenow.ritm.neg.renamed_portal_ticket_page`) pins
  that. The format now lets a literal query condition plus the `sys_id`
  capture anchor a `{_}` portal rule (`adapter-templates.md` §4.1), as the
  knowledge-article rules do, but a `{_}` ticket rule also covers every
  other type's classic `/<table>.do?sys_id=…` path. Proving the two disjoint
  needs `absent = ["id"]` on the classic rules, which already list the eight
  export processors, the most `absent` allows. The follow-up is bead
  `brwr-e1m3`. Until then, add the instance's portal name to the portal
  rules' `(sp|esc)` alternation in settings' advanced rule editor, for
  example `/(sp|esc|helpzone)`; the literal names anchor the rule.
- The workspace rules accept any one- or two-segment workspace and any
  trailing segments. A workspace sub-route that opens a related record would
  be attributed to this one (medium risk; the trailing shape is assumed).
- An instance whose display value or table label differs records that value,
  or the whole title.
- Instances on `servicenowservices.com` (US government cloud) and custom
  domains are not offered this starter.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real ServiceNow account:

1. Open a requested item from the classic list, from Service Operations Workspace
   and from the portal; record each URL and tab title. Is the classic form
   reached directly, or only through `/now/nav/ui/classic/…`?
2. Confirm the workspace trailing segments (`/params/…`) and whether any
   sub-tab opens a different record.
3. Confirm the platform title is `RITM… | Requested Item | <product name>`, and
   record the workspace and portal titles.
4. Note the portal path the instance uses; if it is not `/sp` or `/esc`,
   this draft records no portal ticket pages there.
5. On a Capture Site review on a requested item page with no saved rules, confirm
   Brauser offers all four ServiceNow types, each with its own statement and
   the development-draft source line; confirm saving one turns the page
   card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set
`servicenow.ritm`'s `status` in `protocol/starters/servicenow.json` to
`{ "state": "certified", "method": "live", "checked_on": "<date>",
"checked_by": "John" }`, and run `npm run gen:starters`
(`protocol/starters/README.md`). The Capture Site source line follows the
status.
