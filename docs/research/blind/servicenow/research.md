# ServiceNow blind starter: research notes

Date: 2026-11-03. Blind test: built only from public, logged-out sources and
this repository. I did not use a ServiceNow instance, account, or browser
history. Nothing here has been checked against a live instance.

Files: `adapters.toml` (4 adapters), `vectors.json` (44 positive and 51
negative synthetic cases), `tools/` (throwaway validation scripts).

## Sources consulted

Web search was mostly unavailable: DuckDuckGo returned a CAPTCHA, Brave
returned 429, Mojeek returned 403, Bing returned nothing relevant, and the
docs.servicenow.com pages render client-side. The ServiceNow Community search
page works without login (`/community/forums/searchpage/tab/message?q=...`),
so most of the evidence comes from community threads. Each thread is under
`https://www.servicenow.com/`:

- C1 `community/sysadmin-forum/change-browser-tab-title-on-security-incident-form/m-p/3249252`. Tab title "SIR0010002 | Security Incident | ServiceNow". An MVP answer says the instance uses the table's display field and appends "| ServiceNow".
- C2 `community/sysadmin-forum/change-browser-tab-title/m-p/3167002`. Title format `<display> | Problem | <instance>`; "incident is INC123456 | ..... | ...". The last part is the UI16 "browser title" property.
- C3 `community/developer-forum/dynamic-browser-window-title/m-p/3076825`. "SCTASK0224460 | Catalog Task | ServiceNow". The poster says custom title scripts no longer work because ServiceNow now sets the title itself.
- C4 `community/servicenow-ai-platform-forum/browser-tab-title-shows-null/m-p/1250691`. "false | Class Name | Instance Name": the first part is the record's display value.
- C5 `community/training-and-certifications/unable-to-change-browser-tab-title/m-p/3577454`. `glide.product.name` sets the tab-title suffix.
- C6 `community/developer-forum/browser-tab-name/m-p/2368840`. Next Experience list title "HR Cases | <project name>", which sometimes flashes "Classic | Unified Navigation App | Service now".
- C7 `community/service-management-forum/how-to-edit-the-browser-tab-name-when-we-open-the-serviceportal/m-p/404086`. Service Portal titles are `PAGE_TITLE - PORTAL_TITLE` from London onward; the order was reversed before London.
- C8 `community/itsm-forum/can-we-change-the-browser-tab-title-on-the-service-portal-for-a/m-p/761872`. Portal catalog tab "Service Catalog - IT Services".
- C9 `community/developer-forum/classic-ui-polaris-and-url-generation/m-p/2383202`. With `glide.ui.polaris.experience=true`, `nav_to.do?uri=<table>.do?sys_id=<id>` becomes `/now/nav/ui/classic/params/target/cmdb_ci_win_server.do%3Fsys_id%3D...`. Workspace example: `/now/cmdb/record/cmdb_ci_computer/{sys_id}`.
- C10 `community/developer-forum/agent-workspace-direct-url-for-case/m-p/3533896`. `/now/sow/record/sn_customerservice_case/<sys_id>` and `/now/cwf/agent/record/sn_customerservice_case/<sys_id>`. Workspace URLs use sys_id; opening by number needs `<table>.do?sysparm_query=number=...`.
- C11 `community/developer-forum/open-email-approval-links-in-service-operations-workspace/m-p/3496058`. `now/workspace/agent/record/<table>/<sys_id>` (Agent Workspace) and `/now/sow/record/` (SOW).
- C12 `community/incident-management-sow-forum/ui-navigation-redirection/m-p/3356660`. `/incident.do?sys_id=` and `/now/sow/record/incident/<sysId>`.
- C13 `community/sn-employeeworks-forum/need-help-in-url/m-p/3220859`. `/esc?id=ticket&table=sc_req_item&sys_id=...` and `id=standard_ticket`.
- C14 `community/servicenow-ai-platform-forum/using-permalinks-in-service-portal-for-knowledge-articles/m-p/1252454`. `sp?id=kb_article_view&sys_kb_id=` changes with each version; `kb_view.do?sysparm_article=KB0010035` and `sp?id=kb_article_view&sysparm_article=KB...` give the latest version.
- C15 `community/knowledge-managers/base-urls-for-items-in-servicenow/m-p/2553180`. `kb_view.do?sysparm_article=` and `nav_to.do?uri=<table>.do?sys_id=` written unencoded.
- C16 `community/developer-forum/how-to-redirect-users-from-kb-view-do-sys-kb-id-to-service/m-p/1861390`. Code that builds portal links: `?id=kb_article&sys_id=`, `?id=sc_cat_item&sys_id=`.
- C17 `community/developer-forum/redirect-to-now-nav-ui-classic-params-target-kb-3fid-3dkb-home/m-p/3112747`. `/now/nav/ui/classic/params/target/kb%3Fid%3Dkb_home`, which also shows the `/kb` Knowledge portal suffix.
- C18 `community/developer-forum/how-to-set-the-dynamic-page-title-on-service-portal/m-p/1646603`. `sp?id=<ticket page>&table=...&sys_id=...`; the base-system ticket page has a "dynamic page title".
- C19 `community/csm-forum/agent-workspace-title-bar-showing-record-number/m-p/2616307`. The workspace `sessionTabTitle` property defaults to `contact,consumer,number`. That property sets in-app tab titles, not necessarily the browser tab.
- R1 Raycast ServiceNow extension, public source:
  `https://github.com/raycast/extensions/tree/main/extensions/servicenow/src/utils`.
  `extractRecordFromUrl.ts` parses `/now/sow/kb_view/kb_knowledge/<sysId>`,
  `/now/sow/record/<table>/<sysId>`,
  `/now/nav/ui/classic/params/target/<encoded target>`,
  `nav_to.do?uri=<table>.do?sys_id=<sysId>` and `/<table>.do?sys_id=<sysId>`,
  with sys_id `/^[0-9a-f]{32}$/i`. `instanceUrl.ts` lists the domains
  `service-now.com` and `servicenowservices.com` and allows custom domains.
- Repository: `adapter-templates.md` (§3–§11), `extension/starter-adapters.ts`,
  `docs/adapters/jira.issue.md`, `adapter-templates/src/document.rs`
  (`SHARED_ORIGINS`), `host/src/adapters.rs`, `scripts/adapter-templates-eval.mjs`.

The SN Utils repository (`arnoudkooi/ServiceNow-Utils`) no longer publishes
its source.

## Origin model

- Instances follow `https://<instance>.service-now.com`, a tenant-subdomain family like `*.atlassian.net` (high confidence; R1, every community URL). Each instance is a separate origin, and it is not in `SHARED_ORIGINS`, so no `scope` is needed: the hostname is the tenant.
- Proposed `starterKind` rule: the host ends with `.service-now.com` and is not `service-now.com` itself. Add `.servicenowservices.com` (US government cloud, medium; R1) as the same family. Custom vanity domains (R1: "https://sn.internal.corp") cannot be detected by hostname, so a user would apply the starter by hand.
- Same caveat as Jira: a `*.service-now.com` host may be a ServiceNow-owned instance (`hi`, `nd`, `support` are separate hosts). Those are real instances and only record once the user enables that origin, so that is acceptable.
- `adapters.toml` uses `https://acme.service-now.com` as a placeholder origin.

## Artifact types

Every record ID is `servicenow.<thing>:{host}/{sys_id}`. sys_id is 32 hex
characters (R1 regex), unique within an instance, and is what every record
URL form carries (C9–C13). Numbers (INC…) appear in URLs only as list queries.
Confidence that sys_id is the stable ID: high.

The type names are `servicenow.incident`, `servicenow.ritm`, `servicenow.change`
and `servicenow.kb`. The type doubles as the ID scheme, and the §8.1 scheme
grammar has no `_`. `servicenow.request_item` failed with `id_syntax` in both
engines.

### 1. servicenow.incident (table `incident`)

| URL form | Rule | Confidence | Evidence |
|---|---|---|---|
| `/incident.do?sys_id=<32hex>[&sysparm_*…]` | 1 | high | C12, R1 |
| `/now/sow/record/incident/<32hex>[/…]` | 2 (`/now/{_}/record/…/{*}`) | high for the base path; medium for trailing segments | C10–C12, R1 |
| `/now/<other workspace>/record/incident/<32hex>` | 2 | medium (C9 shows `/now/cmdb/record/`) | C9 |
| `/now/workspace/agent/record/incident/<32hex>` (Agent Workspace, legacy) | 3 | medium-high | C11 |
| `/now/cwf/agent/record/incident/<32hex>` (CSM/FSM workspace) | 3 | medium-high | C10 |
| `/sp` or `/esc` `?id=ticket&table=incident&sys_id=<32hex>` | 4 | high for the shape | C13, C18 |
| same with `id=standard_ticket` | 5 | medium-high | C13 |
| same with `id=form` | 6 | medium (common SP form page; no thread found) | prior knowledge |

- Must not match: `incident_list.do`; `sys_id=-1` (new record); `sysparm_query=number=…` (no sys_id); the export processors `?XML`, `?PDF`, `?CSV`, `?EXCEL`, `?XLSX`, `?JSON`, `?JSONv2`, `?UNL` (excluded with `absent`; medium, from memory of platform processors); a duplicated `sys_id`; SOW `-1_uid_*` placeholders; portal pages without `sys_id` or with another `table`.
- Tab title in the platform UI is `<display value> | Incident | <glide.product.name>`, default `INC0010001 | Incident | ServiceNow` (medium-high; C1–C5). Template: `{title} | Incident | {~}`. The extracted title is the number, because the display field of task tables is `number` (C1, C2). It is not the short description. An instance whose display field or display-value override differs (C2) yields that field instead. A renamed table label gets no template match, so Brauser records the whole title.
- Workspace tab titles are unknown. No evidence was found, and C19 covers in-app tabs. No entry was added, so the whole title is recorded.
- Portal tab titles are `<page title> - <portal title>` (C7, high for the general format). The ticket page's own title is not documented (C18 says it is dynamic). No entry was added, because guessing `{title} - {~}` would extract "Ticket" from "Ticket - Employee Center".

### 2. servicenow.ritm (table `sc_req_item`, "Requested Item", RITM…)

Same rule shapes and confidence as incident, with the table literal swapped.
The Employee Center form `esc?id=ticket&table=sc_req_item&sys_id=` is
directly evidenced (C13). The title is `{title} | Requested Item | {~}`;
the label is from memory of the base-system table label (medium).

### 3. servicenow.change (table `change_request`, "Change Request", CHG…)

The same shapes apply, without `standard_ticket`. Changes are mostly opened in
the platform UI and in SOW. The title is `{title} | Change Request | {~}`
(label: medium).

### 4. servicenow.kb (knowledge article by number)

| URL form | Rule | Confidence | Evidence |
|---|---|---|---|
| `/kb_view.do?sysparm_article=KB0010035` | 1 | high | C14, C15 |
| `/(sp\|esc\|kb)?id=kb_article_view&sysparm_article=KB…` | 3 | high (sp); medium (esc, kb) | C14, C17 |
| `/(sp\|esc\|kb)?id=kb_article&sysparm_article=KB…` | 2 | medium | C16 shows `kb_article` with sys_id |

- ID: `servicenow.kb:{host}/{number}`. The number stays the same across versions and opens the latest version the user may read (C14, high). The capture is `token(3,40)|upper`. KB prefixes are configurable (for example `HRKB…`), and `token` cannot require a prefix. `|upper` assumes number lookup is case-insensitive (low confidence).
- Deliberately not recorded: `kb_view.do?sys_kb_id=`, `?id=kb_article_view&sys_kb_id=`, `?id=kb_article&sys_id=`, `/now/sow/kb_view/kb_knowledge/<sys_id>` (R1), and `kb_knowledge.do?sys_id=`. Each names one version's sys_id (C14), and one adapter cannot build the number-based ID from it. A separate `servicenow.kbversion` adapter keyed by sys_id would record them as distinct artifacts. It is left out so the same article is not split across two types.
- Must not match: knowledge home and search (`kb_home`, `kb_search`, `kb_find.do`), a duplicated `sysparm_article`, or a missing article parameter.
- Title: unknown for both `kb_view.do` and the portal pages, so no template. That is low confidence either way.

## Types left out, and why

- Requests (`sc_request`, REQ): a user works on the RITM, and the REQ is a container. A RITM URL does not carry the REQ sys_id, so `within` cannot express the relationship.
- Cases (`sn_customerservice_case`) and HR cases (`sn_hr_core_case`): product-specific (CSM, HR), with their own portals (`/csm?id=csm_ticket…`, `/esc?id=hrm_ticket_page…`, unverified). They are easy to add later by copying the incident rules (C10 evidences the workspace URLs).
- Catalog items (`sc_cat_item`): they are order forms, not artifacts the user tracks.
- Problems, tasks and other task tables: valuable but lower volume. The rules depend only on the table, so a starter could be generated per table.
- A single generic "any table" adapter is not possible. `/{table:name}.do` cannot be a segment, because `incident.do` is one segment and a `name` capture is not an anchor. The ID would also need the table, which gives an ugly ID shape.

## Grammar limits hit

1. **Nested, URL-encoded URIs cannot be expressed.** These are the most common Next Experience and UI16 address-bar forms:
   `/now/nav/ui/classic/params/target/incident.do%3Fsys_id%3D<hex>` (C9) and
   `/nav_to.do?uri=incident.do%3Fsys_id%3D<hex>` (or unencoded `uri=incident.do?sys_id=…`, C15).
   The decoded segment or query value is `incident.do?sys_id=<hex>[&…]`. No capture type admits `?`, `=` or `&`; literals cannot contain `?`; `@tail` splits only on `-` and only for `int`/`hex`; and query conditions match one top-level key. With Next Experience on, `nav_to.do` redirects to the `/now/nav/ui/classic/...` form (C9), so **classic-UI record visits inside the Next Experience shell are not recorded**. Both forms are in `vectors.json` as negatives so the gap stays visible. Expressing them would take a future format feature, such as a "nested target" rule `{ path = "/now/nav/ui/classic/params/target/{target:uri}" , target = { path = "/incident.do", query = {sys_id = "{id:hex(32)}"} } }`, applied to one decoded segment or one query value. Until then, users get whatever the Brauser web.page fallback records.
2. Query values cannot be an alternation of literals, except as a `mode` capture. So `id=ticket|standard_ticket|form` takes three rules per portal set (up to 6 of the 16-rule budget).
3. The ID scheme grammar has no underscore, which forced `servicenow.ritm`.
4. Title captures must be named after a URL capture, so the record number cannot be extracted separately from the title, and it cannot serve as an alias (the ID is the sys_id).
5. Custom portal suffixes, such as `/myportal?id=ticket…`, need a literal per suffix. Only `sp`, `esc` (and `kb` for knowledge) are listed.

## Validation (both engines, no product code edited)

```sh
cd research/blind/servicenow
python3 -I tools/toml2json.py adapters.toml /tmp/sn-research/doc.json   # TOML -> engine JSON
PATH=~/.nvm/versions/node/v24.14.0/bin:$PATH \
  node tools/run-vectors.mjs /tmp/sn-research/doc.json vectors.json        # TS twin (extension/dist)
# document valid: 4 adapters
# positives 44, negatives 51, failures 0
cd tools/rust-eval && PATH=~/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH \
  cargo run --offline -q -- /tmp/sn-research/doc.json ../../vectors.json  # Rust crate via path dependency
# document valid: 4 adapters
# positives 44, negatives 51, failures 0
```

The Rust runner is a throwaway crate (`tools/rust-eval`, its own empty
`[workspace]`, and a copy of the repository's `Cargo.lock`). Its `target/` was
deleted after the run. The first attempt failed in both engines with
`id_syntax` on `servicenow.request_item` (see limit 3).

## Known limits and open questions

- The biggest gap is limit 1. Without nested-URI support, a Next Experience user who opens records from lists in the classic UI is not recorded; only workspace, portal and direct-`.do` visits are.
- The SOW trailing-segment shape (`/params/...`) is assumed, and `{*}` admits any suffix. If SOW puts a different record in a sub-route (for example a related record opened as a sub-tab), this rule would attribute it to the parent. Medium risk.
- `/now/{_}/record/<table>/<id>` accepts any one-segment workspace. A workspace that uses `record/<table>/<id>` for something other than the record would be misattributed (low risk).
- Title confidence is medium-high for platform forms and unknown for workspaces, portals and KB.
- The `|upper` on KB numbers assumes case-insensitive lookup.
- Instances with custom table labels, a non-number display field, or a translated UI (labels are localized) get the whole tab title instead.
- Hash-routed or legacy UI11 `/home.do`-style shells are not considered.

## How I would verify this with a real account

1. On a PDI or work instance, with Next Experience off (UI16) and then on, open an incident from a list. Record the address-bar URL and `document.title` for the direct `incident.do`, `nav_to.do` and `/now/nav/ui/classic/...` forms.
2. Open the same incident in SOW, Agent Workspace (if present) and the CSM/FSM workspace. Record the URLs, including any `/params/...` suffix after switching tabs, and the tab titles. Add title entries if they are stable.
3. Open it on `/sp` and `/esc` (ticket and standard_ticket) and record the tab titles.
4. Repeat steps 1–3 for a RITM and a change, and confirm the table labels "Requested Item" and "Change Request".
5. KB: open an article by `kb_view.do?sysparm_article=`, from portal search (note `sysparm_rank`/`sysparm_tsqueryId` noise), from SOW `kb_view`, and as an older version. Confirm the lowercase number resolves, and record the tab titles.
6. Confirm that the export processors (`?XML`, `?PDF`, `?CSV`, `?EXCEL`) render or download rather than showing a form.
7. Check that `glide.product.name` and a localized UI change only the suffix and label, as the templates assume.
8. Run the captured samples through `tools/run-vectors.mjs` and the Rust runner, then write `docs/adapters/servicenow.*.md` pages per §11.1.
