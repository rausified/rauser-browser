# Research-built starters, PR D3: Aha! and ServiceNow

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `aha` and `servicenow` starter families as development drafts, with the scorecard's ServiceNow portal fix applied.

**Architecture:** Same recipe as D1 and D2: one family file per family, one doc page per type, regenerated outputs. Both are strict subdomain families with `scope: null`. ServiceNow's knowledge-article rules key on the query and leave the portal path as `{_}`. An `absent: ["sys_id"]` keeps them disjoint from the record-form rules.

**Tech Stack:** JSON family files, Markdown docs, Node 24.14.0 scripts and `node:test`, Rust `adapter-templates` tests.

**Spec:** `docs/superpowers/specs/2026-11-03-starter-families-design.md` ("Decisions", "Builder's calls" 7–9, "Research method", first bullet); `protocol/starters/README.md`; `docs/research/blind/SCORECARD.md` (miss 1); research `docs/research/blind/{aha,servicenow}/`; `adapter-templates.md` §10.3 and `adapter-templates/src/overlap.rs` `queries_disjoint`.

**Precondition:** D2 (`2026-11-05-starters-d2-box-webex-splunk.md`) has merged. Start from current `origin/main`.

## Design challenge

- **Challenger:** the plan author's self-challenge.
- **Alternative:** apply the scorecard's `{_}` portal fix to every ServiceNow portal rule (incident, RITM and change tickets as well as KB), not only KB, since a renamed portal also hides ticket pages.
- **Outcome:** kept KB only, as the coordinator directed. A ticket portal rule on `/{_}` shares the `sys_id` key with the `/<table>.do?sys_id=` rules of the *other* tables. `queries_disjoint` only proves disjointness by a differing literal on a shared key or by a required-vs-absent key, so the generated `overlap_report` would flag six pairs. Fixing that would need `absent` lists on every `.do` rule, a design nobody has checked against real ticket URLs. It is recorded as a follow-up (Open decision 3).

## Open decisions

1. **`aha.portalidea` is deferred.** **Status:** builder's call (this plan). The coordinator asked for `<portal>.ideas.aha.io` as a second family "if needed". But the generator's collision check refuses a suffix inside another suffix whatever `labels` says (`protocol/starters/README.md` "Origin collisions"), so `.ideas.aha.io` can't sit beside `.aha.io`. **Default if rejected:** none within a data PR. A generator change that lets a `labels: 1` family coexist with a deeper suffix is its own PR. File a bead.
2. **ServiceNow KB rules: path `/{_}`, query `id=kb_article|kb_article_view` plus `sysparm_article`, and `absent: ["sys_id"]`.** **Status:** the query-keyed `{_}` form was resolved by the coordinator (2026-11-05, scorecard miss 1). The `absent` is a builder's call: without it, `/{_}` overlaps every `/<table>.do?sys_id=` rule. The cost is that a KB URL that also carries `sys_id` isn't recorded. The research already excludes `?id=kb_article&sys_id=` (a version ID, not the article number).
3. **ServiceNow ticket portal rules stay on `/(sp|esc)`.** **Status:** builder's call (Design challenge). A ticket page under a renamed portal is not recorded. This goes in the doc's "Known limits" and gets a follow-up bead.
4. **Settings gallery hint copy** (PR C Open decision 9, deferred here). **Status:** resolved by the coordinator, 2026-11-05: the hint names the first three families in catalog order and then "and N more", e.g. `Enter an Atlassian Cloud (Jira or Confluence), SharePoint/OneDrive, GitHub or one of N more work tools' site addresses above to see starter drafts.` Exact copy is the implementer's within that shape, derived from data (no literal family list in tests beyond today's first three). Task 3 runs. Cost if wrong: a copy tweak.
5. **All families ship `development`.** **Status:** resolved (coordinator, 2026-11-05).

## Global Constraints

- Worktree: `git -C /Users/jrauser/ENDURANCE/work/rb-starter-c fetch origin && git -C /Users/jrauser/ENDURANCE/work/rb-starter-c worktree add /Users/jrauser/ENDURANCE/work/rb-starters-d3 -b feat/starters-d3 origin/main`. Never touch `/Users/jrauser/ENDURANCE/work/rauserBrowser`, and never `git stash`.
- Node 24.14.0 (`source ~/.nvm/nvm.sh && nvm use 24.14.0`). Cargo: `export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH`, with the worktree's own `./target`.
- Every rule in D1's "Global Constraints" from "Type names" through "Doc pages" applies unchanged: type-name grammar, the fixed per-type fields, adapters copied from the research TOML minus `origin`, synthetic data only, the vector shape and workflow (TS engine against `synthesizeFixture` via `scripts/adapter-templates-eval.mjs`), samples with computed shapes and the `samples_shape` flip rule, and the doc-page layout with `Status: development draft.`. On this plan's main branch they are in `docs/superpowers/plans/2026-11-05-starters-d1-google-figma-vidcast.md`. D2's "Subdomain origins" rule also applies: rewrite research URLs onto `sample_origin`, and count negatives only on `sample_origin`.
- Commit trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The PR body ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`. Merge only through the queue with the `queue` label plus `@Mergifyio queue`, never `gh pr merge`.

## Review Focus

1. A KB article under a customer-renamed portal (`/helpzone?id=kb_article_view&sysparm_article=KB0012345`, `/policyhub?id=kb_article&sysparm_article=KB0012345`) must be recorded as `servicenow.kb` with number `KB0012345` (Task 2 positives, scorecard miss 1).
2. A KB search or home page (`/helpzone?id=kb_search`, `/kb?id=kb_home`) and a version link (`/helpzone?id=kb_article&sys_id=<hex>`) must match nothing (Task 2 negatives).
3. `/incident.do?sys_id=<hex>&XML` (an export) must match nothing (Task 2 keeps the research's `absent` negatives).
4. An Aha! feature `/features/PRJ-123` and its Develop form `/develop/features/PRJ-123` must get one ID, and `/features/new` must match nothing (Task 1).
5. An Aha! login tab (`Log in to your workspace | Aha!`) on a record URL extracts `Log in to your workspace`. The doc must name this as the known loading-title limit, and a vector pins it (Task 1).

---

### Task 1: `aha` family (features, requirements, epics, releases, ideas, notes)

**Files:**
- Create: `protocol/starters/aha.json`, `docs/adapters/aha.feature.md`, `docs/adapters/aha.requirement.md`, `docs/adapters/aha.epic.md`, `docs/adapters/aha.release.md`, `docs/adapters/aha.idea.md`, `docs/adapters/aha.note.md`
- Regenerate: `extension/starter-families.generated.ts`, `protocol/capture-site/catalog.json`, `protocol/capture-site/starter-shapes.json`, `protocol/adapter-templates/starter-vectors.json`, `docs/adapters/README.md`, `protocol/starters/ids.lock.json` (append)

**Interfaces:**
- Consumes: research `docs/research/blind/aha/`.
- Produces: family `aha`, types `aha.feature` (0), `aha.requirement` (1), `aha.epic` (2), `aha.release` (3), `aha.idea` (4), `aha.note` (5).

Family fields: `display_name` `"Aha!"`. `review_prompt` `"Choose the kinds of Aha! records to remember on this Aha! account."`. `origin` `{ "kind": "subdomain", "suffix": ".aha.io", "labels": 1, "exclude": ["www", "secure", "support", "status", "cdn", "ideas", "identity", "iad-prod1"] }`. `scope` `null`. `sample_origin` `"https://acme.aha.io"`. `overlaps`: all 15 pairs `"disjoint"`. Labels `Aha! features` / `requirements` / `epics` / `releases` / `ideas` / `notes`, each prefixed `Aha! `. The record_labels are the same plural nouns. Adapters are verbatim from the research, including the two-entry `| Aha!{~}` titles and `refs`. Drop `aha.portalidea` (Open decision 1).

- [ ] **Step 1: Scaffold and see it fail.** Run `npm run new:starter -- aha aha.feature --subdomain .aha.io`, then the five other types. Expected: the fill-in list. Replace the scaffolded `exclude` with the list above.
- [ ] **Step 2: Fill the family file.**
- [ ] **Step 3: Write vectors** from the research's `acme.aha.io` cases (70). Drop the `acme.ideas.aha.io`, `secure`, `www` and `support` cases. The `other.aha.io` case is on the origin and carries as a positive on its own host. Add `eval.aha.feature.login_title`: on `/features/PRJ-123`, title `Log in to your workspace | Aha!`, where `expect.title` is `Log in to your workspace` and `title_entry` is `1`.
- [ ] **Step 4: Add samples**, one per type, for example `/features/PRJ-123` titled `PRJ-123 Bulk export | Aha! Roadmaps`, `/requirements/PRJ-123-1`, `/epics/PRJ-E-12`, `/releases/PRJ-R-3`, `/ideas/ideas/PRJ-I-45` and `/pages/PRJ-N-67`. Each type with a title template needs one titled sample.
- [ ] **Step 5: Write the six doc pages.** "Recorded samples" cites the scorecard's Aha! result: 2 of 2 matched, titles not judgeable because only login titles were seen. `aha.idea`'s "Known limits" states that portal ideas (`<portal>.ideas.aha.io`) are not covered, and why.
- [ ] **Step 6: Generate and run the focused checks.**
  Run: `npm run gen:starters && node scripts/generate-starters.mjs --check && npm run build:extension && node --test extension/tests/starter-registry.test.mjs extension/tests/adapter-templates.test.mjs extension/tests/site-patterns.test.mjs extension/tests/starter-families.test.mjs && cargo test --locked -p adapter-templates --test vectors --test structural --test shapes && npm run test:adapter-diff`
  Expected: six `appended lock entry` lines, then all pass, including `overlap_report.starter.aha`.
- [ ] **Step 7: Commit** with message `feat(starters): aha development starters`.

### Task 2: `servicenow` family (incidents, requested items, changes, knowledge articles)

**Files:**
- Create: `protocol/starters/servicenow.json`, `docs/adapters/servicenow.incident.md`, `docs/adapters/servicenow.ritm.md`, `docs/adapters/servicenow.change.md`, `docs/adapters/servicenow.kb.md`
- Regenerate: the Task 1 outputs

**Interfaces:**
- Consumes: research `docs/research/blind/servicenow/`.
- Produces: family `servicenow`, types `servicenow.incident` (0), `servicenow.ritm` (1), `servicenow.change` (2), `servicenow.kb` (3).

Family fields: `display_name` `"ServiceNow"`. `review_prompt` `"Choose the kinds of ServiceNow records to remember on this instance."`. `origin` `{ "kind": "subdomain", "suffix": ".service-now.com", "labels": 1, "exclude": ["www"] }`. `scope` `null`. `sample_origin` `"https://acme.service-now.com"`. `overlaps`: all 6 pairs `"disjoint"`. Labels `ServiceNow incidents` / `requested items` / `change requests` / `knowledge articles`, each prefixed `ServiceNow `. record_labels `incidents` / `requested items` / `changes` / `knowledge articles`. Incident, RITM and change adapters are verbatim from the research. `servicenow.kb` `match` is exactly:

```json
[
  { "path": "/kb_view.do", "query": { "sysparm_article": "{number:token(3,40)|upper}" } },
  { "path": "/{_}", "query": { "id": "kb_article", "sysparm_article": "{number:token(3,40)|upper}" }, "absent": ["sys_id"] },
  { "path": "/{_}", "query": { "id": "kb_article_view", "sysparm_article": "{number:token(3,40)|upper}" }, "absent": ["sys_id"] }
]
```

`servicenow.kb` keeps `id` `servicenow.kb:{host}/{number}` and has no `title`.

- [ ] **Step 1: Scaffold and see it fail.** Run `npm run new:starter -- servicenow servicenow.incident --subdomain .service-now.com`, then the other three types.
- [ ] **Step 2: Fill the family file.**
- [ ] **Step 3: Write vectors** from the research's 44 `positives` and 51 `negatives`. Research KB positives on `/sp`, `/esc` and `/kb` keep matching (rules 1 and 2). Add:
  - positives `eval.servicenow.kb.renamed_portal_view` (`/helpzone?id=kb_article_view&sysparm_article=KB0012345`, rule 2) and `eval.servicenow.kb.renamed_portal_article` (`/policyhub?id=kb_article&sysparm_article=kb0012345`, rule 1, captured `KB0012345`);
  - negatives `eval.servicenow.kb.renamed_portal_search` (`/helpzone?id=kb_search&query=vpn`), `eval.servicenow.kb.version_link` (`/helpzone?id=kb_article&sys_id=<32 hex>`) and `eval.servicenow.kb.both_keys` (`/helpzone?id=kb_article&sysparm_article=KB0012345&sys_id=<32 hex>`), each `{match: false, reason: "no_rule"}`.
  If the eval helper gives a different `reason` for a negative, copy the engine's value and check that it is a no-match reason.
- [ ] **Step 4: Add samples:** `/now/sow/record/incident/<32 hex>` titled `INC0010001 | Incident | ServiceNow`, the RITM and change equivalents with their titles, and `/helpzone?id=kb_article_view&sysparm_article=KB0012345` with `title: null`. Set `servicenow.kb` `samples_shape: false` up front, since it is query-routed. Apply the flip rule to the others.
- [ ] **Step 5: Write the four doc pages.** "Recorded samples" cites scorecard miss 1: 0 of 4 KB articles were matched on custom portal paths before the fix. The four were KB articles, and `servicenow.kb`'s new rules 1 and 2 cover that form. "Known limits" carries Open decisions 2 and 3, and `nav_to.do?uri=` (nested encoded targets, not expressible).
- [ ] **Step 6: Generate and run the focused checks:** Task 1's Step 6 command. Expected: four lock appends, and `overlap_report.starter.servicenow` passes with `{"pairs": []}`. If it reports a pair, the `absent` in Open decision 2 is missing or wrong. Fix the family file, never the expectation.
- [ ] **Step 7: Commit** with message `feat(starters): servicenow development starters (query-keyed KB portal rules)`.

### Task 3 (only if Open decision 4 is resolved with new copy): settings gallery hint

**Files:**
- Modify: `extension/options.ts` ~l.464–468 (the no-family hint)
- Test: `extension/tests/options.test.mjs`, beside the existing hint test

- [ ] **Step 1: Update the hint's test** to assert the coordinator's exact copy, then run `npm run build:extension && node --test extension/tests/options.test.mjs`. Expected: FAIL on the old copy.
- [ ] **Step 2: Return the new copy** from the hint function. Rerun the same command. Expected: PASS.
- [ ] **Step 3: Commit** with message `fix(options): shorter starter gallery hint`.

### Task 4: README starter list, follow-ups, gate, PR

**Files:**
- Modify: `README.md`, the step-5 starter sentence
- Maybe create: `docs/improvement-feed/2026-11-05-<slug>.md`

- [ ] **Step 1: Extend the README sentence** to name Aha! and ServiceNow pages. The sentence now lists every family, so check that it still reads as one sentence. If it doesn't, make it name a few examples and point to `docs/adapters/README.md` for the full generated list.
- [ ] **Step 2: Add an improvement-feed entry only for a new lesson.** The candidate here: "a scorecard fix that widens a path to `{_}` can create overlaps with sibling types; check `overlap_report` while researching". Add it only if Task 2's first run actually reported a pair. Otherwise skip, and say so in the PR body.
- [ ] **Step 3: File follow-up beads** with `scripts/dev/create-bead`: Aha! portal ideas (Open decision 1), ServiceNow ticket rules on renamed portals (Open decision 3), and the gallery hint if Task 3 didn't run.
- [ ] **Step 4: Run the gate once.** Run `npm run gate:adapter`. Expected: PASS. It stays in scope even with Task 3, since `extension/options.ts` and `extension/tests/` are on its allowlist.
- [ ] **Step 5: Run one Codex review** with `codex exec review --base origin/main`, then push and open the PR with `gh pr create --title "Starters D3: Aha!, ServiceNow (development drafts)"`.
- [ ] **Step 6: Queue and watch.** Run `gh pr edit <n> --add-label queue && gh pr comment <n> --body "@Mergifyio queue"`, then `node scripts/dev/pr-watch.mjs <n>` until merged.
