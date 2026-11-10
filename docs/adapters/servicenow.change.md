# servicenow.change starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/servicenow/`, researched blind: public sources only
(ServiceNow Community threads and the public Raycast ServiceNow extension
source), no browsing history, cookies or a ServiceNow account.

History check (2026-11-09): graded against the maintainer's full Chrome
history on this family's hosts, plus the earlier held-back sample's URLs
that have since left that history: no real visit of this type, so 0/5
rules confirmed. Not every rule was matched by a real visit, so this type
stays a development draft.

Brauser offers this draft on a customer instance, `<instance>.service-now.com`
(the `servicenow` family's strict subdomain origin, `exclude: ["www"]`),
alongside incidents, requested items and knowledge articles. Any
combination can be chosen in Capture Site and in the settings gallery. The
draft records one artifact per change request, however it was opened:

`servicenow.change:<instance>.service-now.com/<sys_id>`

The ID is the record's 32-hex sys_id, unique within the instance and carried
by every record URL form. The human number (`CHG0030001`) appears in URLs
only in list queries, so it cannot be the ID.

```toml
[[artifacts]]
type   = "servicenow.change"
origin = "https://acme.service-now.com"
match  = [
  { path = "/change_request.do", query = { sys_id = "{id:hex(32)}" }, absent = ["XML", "PDF", "CSV", "EXCEL", "XLSX", "JSON", "JSONv2", "UNL"] },
  { path = "/now/{_}/record/change_request/{id:hex(32)}/{*}" },
  { path = "/now/{_}/{_}/record/change_request/{id:hex(32)}/{*}" },
  { path = "/(sp|esc)", query = { id = "ticket", table = "change_request", sys_id = "{id:hex(32)}" } },
  { path = "/(sp|esc)", query = { id = "form", table = "change_request", sys_id = "{id:hex(32)}" } },
]
id     = "servicenow.change:{host}/{id}"
title  = "{title} | Change Request | {~}"
generic_titles = ["Login"]
```

`generic_titles` lists the titles shown by ServiceNow's sign-in page and
portal instead of the item's own name (`adapter-templates.md` §9.4). A visit
whose title is one of them, ASCII case aside, or is empty or the bare host,
records no title, so the artifact keeps the title it already has ("Untitled"
for a new one).

## What each rule covers

1. **Classic form opened directly**, `/change_request.do?sys_id=<sys_id>`. The
   `absent` keys skip the platform's export processors, which reuse the
   same URL (`?XML`, `?PDF`, `?CSV`, …). The new-record form
   (`sys_id=-1`) fails `hex(32)` and is not recorded.
2. **Workspace record page with one workspace segment**, such as Service
   Operations Workspace, `/now/sow/record/change_request/<sys_id>`. The trailing
   `{*}` admits the workspace's `/params/…` and sub-tab segments.
3. **Workspace record page with two segments**: the legacy Agent Workspace
   (`/now/workspace/agent/record/change_request/<sys_id>`) and the CSM/FSM
   configurable workspace (`/now/cwf/agent/record/change_request/<sys_id>`).
4. **Portal ticket page**, `/sp` or `/esc` with
   `?id=ticket&table=change_request&sys_id=<sys_id>`. The `table` literal keeps
   other tables out. Change requests are mostly worked in the platform UI
   and in workspaces; the portal pages are kept because the `table` literal
   makes them unambiguous. There is no `standard_ticket` rule.
5. **Portal form page**, the same with `id=form`, in any parameter order.

## Titles

One entry: `{title} | Change Request | {~}`. Platform record forms title the tab
`<display value> | <table label> | <product name>`, for example
`CHG0030001 | Change Request | ServiceNow`. The display value of task tables is
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
| `/now/sow/record/change_request/<32 hex>` | `CHG0030001 \| Change Request \| ServiceNow` | that sys_id, title `CHG0030001` |

The scorecard (`docs/research/blind/SCORECARD.md`) saw no change request visits in
the held-back browsing, so this type's rules are graded only by the
research's synthetic vectors, re-verified against this family's own fixture
through both adapter-template engines.

`samples_shape` is `true`: the structural drafter's test does not name this
sample.

## What the draft does not record

- lists (`change_request_list.do`) and records opened by number query
  (`change_request.do?sysparm_query=number=…`), which carry no sys_id;
- the new-record form (`sys_id=-1`) and workspace new-record placeholders
  (`/now/sow/record/change_request/-1_uid_3`);
- export processors (`?XML`, `?PDF`, `?CSV`, …) and a duplicated `sys_id`;
- portal ticket pages without a `sys_id`, or for another table;
- incidents, problems and other tables.

## Known limits

- **Classic-UI visits inside the Next Experience shell are not recorded.**
  The address bar shows
  `/now/nav/ui/classic/params/target/change_request.do%3Fsys_id%3D<sys_id>` or
  `/nav_to.do?uri=change_request.do%3Fsys_id%3D<sys_id>`. The record is inside one
  encoded segment or query value, and the format cannot match a nested URL.
  This is the biggest gap for users who open records from classic lists.
- **Ticket pages on a customer-renamed portal are not recorded.** The portal
  rules list only `/sp` and `/esc`; an instance that serves its portal under
  its own path (`/helpzone?id=ticket&table=change_request&sys_id=…`) gets no match,
  and a vector (`eval.servicenow.change.neg.renamed_portal_ticket_page`) pins
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

1. Open a change request from the classic list, from Service Operations Workspace
   and from the portal; record each URL and tab title. Is the classic form
   reached directly, or only through `/now/nav/ui/classic/…`?
2. Confirm the workspace trailing segments (`/params/…`) and whether any
   sub-tab opens a different record.
3. Confirm the platform title is `CHG… | Change Request | <product name>`, and
   record the workspace and portal titles.
4. Note the portal path the instance uses; if it is not `/sp` or `/esc`,
   this draft records no portal ticket pages there.
5. On a Capture Site review on a change request page with no saved rules, confirm
   Brauser offers all four ServiceNow types, each with its own statement and
   the development-draft source line; confirm saving one turns the page
   card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set
`servicenow.change`'s `status` in `protocol/starters/servicenow.json` to
`{ "state": "certified", "method": "live", "checked_on": "<date>",
"checked_by": "John" }`, and run `npm run gen:starters`
(`protocol/starters/README.md`). The Capture Site source line follows the
status.
