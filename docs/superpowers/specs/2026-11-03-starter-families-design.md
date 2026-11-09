# Starter families: one source file per family, a scaffolder, and research-built starters

Status: design, approved by John 2026-11-03 (stream B of "make adding adapters faster").
Evidence: PR #243 (Confluence) touched 26 files and spent ~1 hour building and ~4 hours on
validation and merging; `docs/research/blind/SCORECARD.md` shows research-built starters recorded
94% of the pages they should, with no false matches, on eight sites graded blind.

## Intent

Adding a starter should be: research the site, fill in one family file, run the scaffolder's
checks, open one PR. Brauser will gain many starters for business sites, most of which nobody
on the team has an account for, so starters built from public research ship as **development
drafts** and are certified later, either by a person's live check or by real browsing
evidence.

## Decisions (John, 2026-11-03)

- **Unchecked starters are offered**, in Capture Site and the settings gallery, with a clear
  "not yet checked on a real account" label. (A wrong rule misses pages; the blind test
  recorded no wrong pages.)
- **Certification by either path, labelled.** `status.state = "certified"` needs
  `status.method` to say which: `"live"` (a person ran the starter's checklist on a real
  account; `checked_by` names them) or `"history"` (real browsing history confirmed every rule
  and title; `checked_by` names the agent session and the evidence summary). The UI shows
  certified starters as certified either way; the docs index shows the method.

## Builder's calls (made by the coordinator, open to John's correction)

The design agent's open decisions, settled:

1. **JSON** family files (Node has no stdlib TOML parser; the vectors are JSON already).
2. **Legacy vectors stay** in `protocol/adapter-templates/vectors.json` against the frozen
   `fixtures.starters`; only new families carry vectors in their family file.
3. **Type names** follow `<vendor>.<thing>` without underscores (the ID grammar rejects `_`).
   Google Workspace is one family (`google-workspace`) with several types.
4. **Grow the host `shared_origins` list** with `drive.google.com` and `calendar.google.com`:
   they mix accounts exactly like `docs.google.com`, so their starters get the every-account
   notice. (Host policy change, its own task.)
5. **Unscoped shared-origin starters are offered in Capture Site** with the every-account
   notice and the draft label.
6. Certification: see Decisions.
7. **New subdomain families are strict**: one label (`labels: 1`) plus an `exclude` list of
   product hosts (`www`, `signin`, ...). Atlassian and SharePoint keep today's matching.
8. **Offer keys** for new types default to `<type>:<origin>`; existing keys are frozen.
9. **The eight research-built starters ship in their own PR** after the format lands (PR D),
   not inside PR C: PR C is already a final-stretch change to shared seams.
10. **Declared container-first overlaps** (added 2026-11-09, for Splunk Cloud). An `overlaps`
   entry may say `expect: "container_first"` with the exact `[a rule, b rule]` pairs that may
   overlap: `b` is within `a` under `a`'s own ID template, `a` is listed first, and `a` wins
   those pairs. The generated `overlap_report` case expects exactly those pairs, so any other
   overlap still fails the build. At runtime `orderContainedFirst` keeps `a` ahead of `b` on the
   family's own origins while the container's declared rules are unedited, and settings hides only
   the declared pairs while both rules are unedited. Why in the family file and not the format: it records how one vendor lays out its
   URLs, reviewed with the family; a format-level precedence key would let any saved or drafted
   adapter declare a winner, which the overlap warning exists to stop. User-written adapters,
   agent drafts and recorded-rule restore keep today's overlap handling; on such a family Capture Site
   offers the starter after the restore overlap refusal, re-checked by the host. The host refuses
   any save that leaves `b` live without `a` ahead of it on its origin (the catalog's generated
   `container_first` line), so a contained contract never holds the container's pages and a later
   container cannot silently re-key them (`protocol/starters/README.md` "Test vectors and overlaps").

## Research method (from the blind test)

Every research-built starter follows `docs/research/blind/<site>/research.md`'s layout and these
checks, each learned from a graded miss:

- When a product lets customers rename a route (ServiceNow portal suffixes), key the rule
  on the query or record ID and leave the renameable part as `{_}`.
- Before ruling out a fragment-routed product (Gmail), look for print, share and permalink
  forms that use the path or query.
- Title suffix templates list both dashes (` - ` and ` – `).
- A title format read from client code is at most medium confidence until seen in a real tab.

## Grammar limits found (follow-ups, not in this design)

Fragment matching (Gmail threads, 125 real URLs), nested encoded targets (ServiceNow
`nav_to.do?uri=`), slash-containing query captures (Splunk `s=` saved searches), a dashed
reference capture type (Aha! `PRJ-E-12`), and a per-mode title policy (so `Video Edit` never
replaces a good title).

# Technical design (from the design agent, read-only survey of `f02e3bd`)

Everything here was read in `/Users/jrauser/ENDURANCE/work/rb-blind-starters` at `f02e3bd`. Nothing was edited and `research/blind/` was not opened.

## 1. What adding a starter touches today

PR #243 changed 26 distinct files across three commits (`6036259` with 17, `0189532` with 6, `14b4bb2` with 3), and `77603d1` touched 8 of them again. Kinds: **D** = data, **L** = logic that should become data-driven, **G** = generated or derivable, **P** = per-starter prose.

| # | Place | What it holds | Kind |
|---|---|---|---|
| 1 | `extension/starter-adapters.ts:8` `StarterKind` union | family ids | L |
| 2 | `starter-adapters.ts:38-46` `starterKind` | origin model: subdomain suffix (`:41-42`) or exact origin (`:43-44`) | L |
| 3 | `starter-adapters.ts:103-210` `jira()` … `githubRepository()` | adapter templates | D |
| 4 | `starter-adapters.ts:214-304` `starterOffers` | one branch per family: key, label, description, `needs_scope`; SharePoint's label varies by host (`:247-249`); offer key formats differ (`:221,232,244,258,272`) | L→D |
| 5 | `starter-adapters.ts:338-347` `STARTER_HISTORY` | earlier rule sets | D |
| 6 | `starter-adapters.ts:21-25,441,452,485-496`; `site-coverage.ts:14-19` `GITHUB_RECORD_LABELS/ORDER` | GitHub container and label constants | L→D |
| 7 | `starter-adapters.ts:48-101` org parser, reserved routes, owner-from-URL | the GitHub scope kind | code (stays) |
| 8 | `extension/capture-site.ts:22-31` `CaptureChoice` (one boolean per type), `:44` `GITHUB_EXAMPLE_ROUTES`, `:71-95` per-family branches and messages, `:111-115` GitHub example URL, `:243` GitHub tenant check | Capture Site plan | L |
| 9 | `extension/capture-site-panel.ts:161-170` element fields, `:240` listeners, `:365-373` per-family show/hide and owner prefill, `:744` disabling, `:899-903` choice; `:39` one fixed `STARTER_SOURCE` string | panel review | L |
| 10 | `extension/panel.html:88-101` `capture-site-atlassian` / `capture-site-github` checkbox blocks | markup | L→rendered |
| 11 | `extension/options.html:59`, `options.ts:448` "Enter an Atlassian Cloud…" hint; `options.ts:476` `offer.kind === "github"` scope input | gallery | L→D |
| 12 | `protocol/capture-site/catalog.json:108` `starter_types` (read by `host/src/capture_site.rs:132,324` and `adapter-templates/src/policy.rs:164`) | reserved types | G |
| 13 | `protocol/capture-site/shapes.json` accept rows whose note contains `(docs/adapters/<type>.md)` (15 rows) | recorded URL→shape pairs | D |
| 14 | `protocol/adapter-templates/vectors.json:5-…` `fixtures.starters` (positional, and it includes a non-starter `gdoc`), plus 286 cases using it (235 evaluate, 10 overlap, 32 draft, …) | engine vectors | D |
| 15 | `host/tests/fixtures/capture-site-structural-response.json:421-430` `taken_types` (pinned by `capture_site.rs:1925-1937`) | sorted starter types plus `tools.item` | G |
| 16 | `adapter-templates/tests/structural.rs:122-129` hard-coded `6` | starter count | G |
| 17 | `extension/tests/starter-adapters.test.mjs:31-44` (hand list of origins and types), `capture-site.test.mjs`, `options.test.mjs`, `panel/capture-site.test.mjs`, `panel/capture-explanation.test.mjs`, `site-coverage.test.mjs` | per-family assertions | mostly G or generic once data-driven |
| 18 | `docs/adapters/<type>.md` (its `Status:` line is the certification record) | doc | P (header is D) |
| 19 | `README.md:73` (site list and certified/draft split), `DESIGN.md:1057`, `adapter-templates.md:372-` §11 TOML examples, `protocol/adapter-templates/README.md`, `TESTING.md` | prose | P; README's status split is G |
| 20 | `.beads/issues.jsonl`, `BOSUN_FEATURE_BACKLOG.md` (rendered from beads by `scripts/render_backlog.py`), `BRAUSER_IMPROVEMENT_FEED.md` | process records | P / G |
| 21 | `scripts/gate-adapter.mjs:53-78` ALLOWLIST; `scripts/ci-macos.mjs` classifier | change gates | one-time edit |

There is a bug in row 9. `STARTER_SOURCE` (`capture-site-panel.ts:39`) reads "Development starter draft · live-tool certification is still pending." for every starter, but Jira, SharePoint and GitHub are release-certified (`docs/adapters/jira.issue.md:3`, `github.pr.md:3`, `sharepoint.doc.md:3`). Status data fixes this for free.

## 2. Source format: `protocol/starters/<family>.json`

JSON rather than TOML: Node has no stdlib TOML parser and the repo pins its dependencies. The adapter templates are already JSON in the vectors, and Rust tests can read JSON without a Node build.

```jsonc
{
  "starter_format": 1,
  "family": "atlassian",
  "display_name": "Atlassian Cloud (Jira or Confluence)",   // gallery and panel hints
  "review_prompt": "Choose the kinds of work to remember on this Atlassian site.",
  "origin": { "kind": "subdomain", "suffix": ".atlassian.net" },
  //   { "kind": "exact", "origin": "https://miro.com" }
  //   { "kind": "subdomain", "suffix": ".webex.com", "labels": 1, "exclude": ["www", "signin"] }
  //   { "kind": "subdomain", "suffix": ".app.box.com", "labels": 1, "exclude": ["www", "fupload-*"] }
  //   { "kind": "any", "of": [{ "kind": "exact", "origin": "https://app.box.com" }, { "kind": "subdomain", ... }] }
  "scope": null,
  //   { "kind": "github_org", "capture": "owner", "max": 32, "required": true }
  //   { "kind": "tenant_name", "capture": "workspace", "max": 16 }
  //   { "kind": "unscoped", "notice": "Records every Google Doc … from every account" }
  "sample_origin": "https://acme.atlassian.net",
  "sample_scope": [],
  "types": [{
    "type": "jira.issue",
    "key": "jira:{origin}",                 // optional; default "<type>:{origin}"
    "label": "Jira issues",
    "label_variants": [],                    // e.g. [{"host_suffix": "-my.sharepoint.com", "label": "OneDrive for work documents"}]
    "record_label": null,                    // e.g. "pull requests" (replaces GITHUB_RECORD_LABELS)
    "description": "Issue pages and board modals become one artifact per Jira key.",
    "example_path": null,                    // e.g. "/{scope}/example-repository/pull/123"
    "adapter": {                             // adapter-template document shape; origin is filled in
      "type": "jira.issue",
      "match": [ { "path": "/browse/{key:jira_key}" },
                 { "path": "/jira/{*}", "query": { "selectedIssue": "{key:jira_key}" } } ],
      "id": "jira:{host}/{key}",
      "title": "[{key:jira_key}] {title} - Jira",
      "alias": { "from": "tab_title", "capture": "key" },
      "refs": [ { "type": "jira.issue", "find": "jira_key" } ]
      // a scoped family writes "scope": { "owner": { "$scope": true } }
    },
    "history": [],                           // STARTER_HISTORY entries, oldest first
    "status": { "state": "certified", "checked_on": "2026-11-02",
                "checked_by": "John", "method": "live Jira Cloud UI, macOS, real Chrome",
                "engines_verified_on": "2026-10-16", "bead": null },
    "docs": { "path": "docs/adapters/jira.issue.md", "title": "Jira issue starter draft" },
    "samples": [ { "url": "https://acme.atlassian.net/browse/PROJ-42",
                   "title": "[PROJ-42] Fix login - Jira",
                   "shape": "/browse/{key}", "note": "Jira canonical issue" } ],
    "vectors": [ /* existing case shape (id, kind, why, impls, url, expect);
                    the doc is implied: fixture "starter:atlassian" */ ]
  }],
  "overlaps": [ { "a": "jira.issue", "b": "confluence.page", "expect": "disjoint" } ]
}
```

### Generated files

All are written by `scripts/generate-starters.mjs` (Node 24, no dependencies; `npm run gen:starters`). `--check` fails and prints `out of date: <path>`, like `protocol/generate.py:316-319`.

| Output | Replaces | How |
|---|---|---|
| `extension/starter-families.generated.ts` | rows 1-6 (data) | whole file holding a `STARTER_FAMILIES` const |
| `protocol/capture-site/catalog.json` `starter_types` line | row 12 | rewrites that one line in place. Order is legacy order with new types appended, so host and policy code don't change. |
| `protocol/capture-site/starter-shapes.json` | starter rows of row 13 | whole file; rows noted `"<note> (docs/adapters/<type>.md)"` so `structural.rs:113` keeps working |
| `protocol/adapter-templates/starter-vectors.json` | new families' vectors | whole file: fixtures `starter:<family>` plus cases |
| `docs/adapters/README.md` | README's status split (row 19) | generated index table: type, family, status, date, checked by |
| `protocol/starters/ids.lock.json` | — | append-only `type → id template`; `--check` fails if any type's `id` differs from the lock |

The generated TS has to sit under `extension/`. Every existing `protocol/ts/generated.js` import is `import type` (for example `extension/activity.ts:1`), and `extension/build.mjs` only runs `tsc`, so nothing guarantees a runtime module outside `extension/` gets emitted into `dist`.

### CI wiring

- Add `gen:starters --check` next to `python protocol/generate.py --check` in `.github/workflows/build-os.yml:40`.
- Add it as the first step in `scripts/gate-adapter.mjs` STEPS.
- Add `protocol/starters/` and the generated paths to the ALLOWLIST (`gate-adapter.mjs:53`) and to the CI macOS classifier.

### Compatibility

- **Frozen IDs.** `ids.lock.json` is seeded with the 7 current types from today's code.
- **Byte-identical behaviour.** The existing test at `starter-adapters.test.mjs:31-44` deep-compares generated offers with the legacy `fixtures.starters`. It stays unchanged and is the proof.
- **Saved user adapters.** `matchingStarter` (`:322-327`) still keys on origin, type and the `id` string, and `history` moves over verbatim, so saved adapters keep matching and keep getting rule updates.

## 3. Driving the extension from the family list

- **`starterKind(origin)`** returns `family: string`. Lookup order: exact origins first, then the longest subdomain suffix, using the same `secureHostname` and `endsWith`/`!==` test as today (`:27-42`). Atlassian and SharePoint keep their permissive any-prefix matching; new families can set `labels: 1` and `exclude` (whole first labels, or first-label prefixes ending in one `*`). A multi-origin family (`kind: "any"`) takes part through each of its members.
- **`starterOffers(origin, scopes: Record<family, string[]>)`** becomes one generic loop over `types`: fill `origin`, substitute `{"$scope": true}`, apply `label_variants`, and set `adapter: null` when the scope is required and empty (today's `:269,279`). `STARTER_HISTORY` is read from data. `matchingStarter`, `starterRuleUpdate`, `starterRuleAddition` and `hasAdapter` don't change.
- **Containers.** `GITHUB_REPOSITORY`, `LINKABLE_TO_REPOSITORY` (`:441`) and `GITHUB_RECORD_*` (`site-coverage.ts:14-19`) are derived: a type is linkable if its template declares `within.type` and that container is a type in the same family. `orderContainedFirst` and `linkedToPresentContainers` (`:511-559`) are already generic.
- **Capture Site.**
  - `CaptureChoice` (`capture-site.ts:22-31`) becomes `{ types: Set<string>; scopes: Record<string, string> }`.
  - `planCaptureSite` checks "at least one type" with a message built from the labels ("Choose Jira issues, Confluence pages, or both."), and checks the scope through its scope kind.
  - Example URLs come from `example_path`, falling back to `exampleUrl` (`capture-site.ts:110`).
  - The panel renders one fieldset per family from data instead of the `panel.html:88-101` blocks.
  - `STARTER_SOURCE` comes from `status.state`.
- **Settings gallery.** The hint text joins `display_name`s (`options.ts:448`, `options.html:59`), and the scope input is rendered per scope kind instead of `offer.kind === "github"` (`options.ts:476`).
- **Already-configured detection** stays generic: `hasAdapter` checks type and origin (`:436-438`), and `siteConfigured` refuses any origin that already has an adapter (`capture-site.ts:56`). Overlap checks stay in `validateAdapterDocument` (`capture-site.ts:101`).

### What still needs code

- **Scope kinds**, as a small TS registry of `{ parse(value), tenantFromUrl(url) }`:
  - `github_org`: today's `parseGitHubOrganizations`, `GITHUB_RESERVED_ROUTES` and `githubOwnerFromUrl` (`:48-101`), also used at `capture-site.ts:243` and `capture-site-panel.ts:373`.
  - `tenant_name`: `parseTenantList` (`:60-69`).
  - `unscoped`: no parser; the family file sets `"unscoped": "acknowledged"` in the template and carries the every-account notice.
  - A new kind needs code plus tests; picking an existing kind needs neither.
- The adapter-template language itself, and the host's `shared_origins` list (`catalog.json:109-133`). That list is host policy, not starter data.

## 4. Scaffolder and completeness test

### Scaffolder

`npm run new:starter -- <family> <type> [--exact <origin> | --subdomain <suffix>] [--scope github_org|tenant_name|unscoped]` runs `scripts/new-starter.mjs`. It:

- creates the family file, or adds the type to an existing one;
- writes deliberately invalid placeholders (`match: []`, `"TODO"` strings), so compile and the completeness test fail until they're filled in;
- sets `status.state = "development"`;
- writes `docs/adapters/<type>.md` with the `Status:` line generated from the status, plus the section headings from `docs/adapters/confluence.page.md:38-156` (What each rule covers, Titles, Recorded samples, What the draft does not record, Known limits, Live check still needed);
- runs `gen:starters`;
- prints the `bd create` command for the starter's bead.

### Registration-completeness test

`extension/tests/starter-registry.test.mjs`, run by `test:extension`. Every failure names the type and the missing place:

1. Every family type appears in catalog `starter_types`, `ids.lock.json`, the generated TS, and `starter-shapes.json`. A type with no drafter-visible form must say `"samples_shape": false`; today that's `github.repo`.
2. Reverse direction: every `starter_types` entry, every `docs/adapters/*.md`, and every `(docs/adapters/X.md)` note in either shapes file has a family type.
3. The doc exists and its first `Status:` line agrees with `status` (state and date).
4. Every `match` rule index is hit by at least one positive evaluate vector; every type has at least one negative vector and at least one sample whose title the type's title template accepts.
5. Every pair of types in a family has an `overlaps` entry, unless one is the other's `within` container. A `within` pair may instead carry a `"container_first"` entry (Builder's call 10).
6. No two families' origin models can claim the same origin: the same suffix, a suffix inside a suffix (unless the shallower family has `labels: 1`, which proves the two disjoint), an exact origin under a suffix, or a repeated exact origin. A multi-origin family is checked member by member.
7. A family whose exact origin is in catalog `shared_origins` has a scope kind other than `null` (spec `adapter-templates.md:190-200`).
8. `gen:starters --check` is clean.

`structural.rs:122-129` changes from `6` to the count of distinct types in `starter-shapes.json`, compared against `starter_types` minus the `samples_shape: false` types.

## 5. Migration and task list

Existing types move over with their templates, labels, history, samples and status copied verbatim. Their 286 legacy cases stay in `vectors.json` against the frozen `fixtures.starters` (moving them would mean rewriting adapter indices; that is Open decision 2). Only new families put vectors in their family file.

### PR A: data source, no behaviour change

1. Add `protocol/starters/schema.json`, the generator with `--check`, and the four family files (atlassian, sharepoint, github, miro) transcribed from `starter-adapters.ts`, plus `ids.lock.json`. Test: generated templates deep-equal `starterOffers(...)` for every sample origin and scope, with the old code still live (branch by abstraction).
2. Generate `starter-families.generated.ts`. Rebuild `starterKind`, `starterOffers` and `STARTER_HISTORY` on it and delete the hand-written branches. Tests: the existing `starter-adapters.test.mjs` passes unchanged, plus a sweep showing the new `starterKind` agrees with the old one on 40 sample origins, including ports, `http:` and bare `atlassian.net`.
3. Generate the catalog `starter_types` line. In the host test at `capture_site.rs:1925-1933`, replace the fixture's `taken_types` with a computed placeholder (`sorted(starter_types ∪ {"tools.item"})`). **Shared seam (host); label `final-stretch`.**

### PR B: UI driven by the family list

4. `CaptureChoice` becomes a set plus scopes, and `planCaptureSite` becomes generic. Tests: the existing `capture-site.test.mjs`, plus one synthetic test family (exact origin, `tenant_name` scope) injected through a test seam.
5. The panel renders family fieldsets and `STARTER_SOURCE` comes from status (fixes the certified-shown-as-pending bug). Tests: `panel/capture-site.test.mjs`, `panel/capture-explanation.test.mjs`, `test:a11y`.
6. The gallery hint and scope input come from data, and site-coverage labels from `record_label`. Tests: `options.test.mjs`, `site-coverage.test.mjs`.

### PR C: vectors, shapes, guards

7. Generate `starter-shapes.json` and move the 15 starter rows out of `shapes.json`. Update `adapter-templates/tests/shapes.rs`, `structural.rs`, `tests/common/mod.rs:10` and `extension/tests/site-patterns.test.mjs` to include it. **Shared seam (`adapter-templates` tests).**
8. Generate `starter-vectors.json` and load it beside `vectors.json` in `adapter-templates/tests/vectors.rs:17,424` and `extension/tests/adapter-templates.test.mjs:15`, synthesizing the `starter:<family>` fixtures. Run `npm run test:adapter-diff`. **Shared seam (`adapter-templates`).**
9. Add the registry test, the scaffolder, the `docs/adapters/README.md` index, the gate and classifier entries, and a short "Adding a starter" section in `AGENTS.md`.
10. Acceptance: scaffold one real new family, for example Aha! (subdomain, unscoped). The diff should be the family file, the doc and the generated files only.

PR A step 3 and PR C need the `final-stretch` label (`AGENTS.md:153,157`). Land PR A first, the same day, as the mechanical prerequisite (`AGENTS.md:161`). New starters can start on the scaffolder once PR C is in; before then they still pay the old touch points, minus whatever PR A and B removed.

## 6. Design challenge

**Strongest simpler alternative:** keep TS as the source of truth. Turn `starterOffers` into a TS data table (`extension/starters/<family>.ts`, type-checked), and have a Node script import the built `dist/starter-adapters.js` to emit the catalog line, the host fixture and the structural count. Vectors and shapes stay hand-placed.

- **For it:** no JSON schema, the compiler checks the data, a smaller first PR, and no Rust loader changes.
- **Against it:**
  - `cargo test` would depend on a Node build to see starter data.
  - Vectors, samples and doc status still live in three separate files, so the "learn the touch points from the last diff" problem stays for half the list.
  - Reviewers can't audit one family in one place.

**Recommendation: JSON family files.** Steps 4-6 (the data-driven UI) are worth doing under either option. If the maintainer wants the smaller change, do PR B on a TS table and drop PR C. That removes about 12 of the 26 touch points; the full design removes about 20. What's left either way: the doc prose, the bead, the feed entry, and any new scope kind.

## 7. Open decisions (maintainer only)

1. JSON source files (recommended) or TOML.
2. Move the 286 legacy starter cases into family files (an index rewrite) or freeze them in `vectors.json`.
3. Type names for new families. `fixtures.starters` already uses `gdoc` / `gdoc:{host}/{id}` as an example adapter. Also whether Google Workspace is one family with doc, sheet and slide types.
4. Whether to grow the host `shared_origins` list (for example `app.vidcast.io`). Spec §6 calls it host-owned (`adapter-templates.md:186`).
5. Whether unscoped shared-origin starters (Google, Figma) are offered in Capture Site, and what acknowledgement they need now that the native confirmation is gone.
6. Certification rules: who may set `certified`, whether `checked_by` plus a date is enough, and whether development starters get a UI badge or stay out of Capture Site.
7. How strict new subdomain families are: one label only, plus excluded hosts such as `www.webex.com` and `signin.webex.com`.
8. The offer-key convention for new types (today's keys are inconsistent: `jira:<origin>`, `confluence.page:<origin>`, `miro.board`, `github.pr`).
9. Whether the first new families can ship in the same PR as PR C, under the batching rule (`AGENTS.md:157`).

## Origin model decisions (2026-11-09)

- **`exclude` prefixes.** An `exclude` entry ending in a single `*` names a first-label prefix (`fupload-*`). Only a trailing `*` after a non-empty DNS-label prefix is accepted. A leading or inner `*` is refused: the one Webex media host on record (`nsj1wss`) puts its varying part first, and a suffix pattern such as `*wss` could also exclude a real customer site, so it is excluded by its whole label instead.
- **Nested suffixes.** A family on a suffix nested inside another's may coexist with it when the shallower family has `labels: 1`, because every host the deeper family claims then has at least two labels before the shallower suffix. That is a proof from the origin models alone, so the check stays a pure registry check. `exclude` is not accepted as a proof, since the deeper family's first label varies. The Aha! ideas portal (`aha-ideas`, `.ideas.aha.io`) is a second family next to `aha` (`.aha.io`, `labels: 1`), not a second origin on `aha`: otherwise every in-app type would also be offered on portals.
- **Multi-origin families.** Box serves the same web app on personal `app.box.com` (an exact host), `<company>.app.box.com` and `<company>.ent.box.com`. A family `origin` may now be `{"kind": "any", "of": [...]}`: two or more exact or subdomain origins, no nesting, no overlap between members. This keeps one family and the shipped type names, so `box.file`, `box.folder` and `box.share` keep their frozen `ids.lock.json` templates. Separate families were ruled out because a type name may appear in only one family, and new type names would split one Box item's artifacts across schemes. The ID templates carry `{host}`, so the same item on two Box hosts still gets two IDs; making them identical would mean changing a frozen template, which `ids.lock.json` forbids.

## Follow-ups

- Maintainer answers the open decisions.
- Label PR A step 3 and PR C `final-stretch`.
- File a bead for certified starters being shown as pending (`capture-site-panel.ts:39`).
