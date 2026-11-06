# Research-built starters, PR D2: Box, Webex, Splunk Cloud

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the `box`, `webex` and (subject to Open decision 2) `splunk-cloud` starter families as development drafts.

**Architecture:** Same recipe as D1: one family file per family, one doc page per type, regenerated outputs. All three are strict subdomain families (`labels: 1` plus an `exclude` list) with `scope: null`. A subdomain family can never be unscoped, and none of these hosts is shared. Splunk's research relies on a container adapter winning a reported overlap. Today's generator can't express that, so it is gated on a decision.

**Tech Stack:** JSON family files, Markdown docs, Node 24.14.0 scripts and `node:test`, Rust `adapter-templates` tests.

**Spec:** `docs/superpowers/specs/2026-11-03-starter-families-design.md` ("Decisions", "Builder's calls" 7–9, "Research method"); `protocol/starters/README.md`; `docs/research/blind/SCORECARD.md`; research `docs/research/blind/{box,webex,splunk-cloud}/`.

**Precondition:** D1 (`2026-11-05-starters-d1-google-figma-vidcast.md`) has merged. Start from current `origin/main`.

## Design challenge

- **Challenger:** the plan author's self-challenge.
- **Alternative:** for Box, two families, an exact `https://app.box.com` and a strict `.app.box.com`, so personal and enterprise hosts are both covered. For Splunk, drop `splunk.app` and keep only `splunk.dashboard`, so there is no overlap to declare.
- **Outcome:** both rejected.
  - Box: type names are unique across the registry (`duplicate type … (also in …)`), so two families would need two sets of type names for one product, and one Box file would get two artifact types. One strict family is the simplest model the generator accepts (Open decision 1).
  - Splunk: without `splunk.app`'s list of built-in views, the Search page, Splunk's busiest page, is recorded as a "dashboard" named `search` (research "splunk.app"). That trades a declared overlap for wrong records.

## Open decisions

1. **Box is one strict subdomain family on `.app.box.com`** (`labels: 1`, `exclude: ["www", "cloud"]`). **Status:** builder's call (this plan; the coordinator asked for the simplest model the generator accepts). It covers `<company>.app.box.com`, the host the scorecard's five real Box pages used. Pages under the bare `<company>.box.com` form redirect there. It does not cover personal `https://app.box.com` (`familyClaims` never claims the suffix's own host) or `<company>.ent.box.com`. **Default if rejected:** an exact `https://app.box.com` family instead, which loses the enterprise hosts. Covering all three hosts needs either a permissive `.box.com` family, which builder's call 7 rules out, or an origin model with several origins, which needs code. Record that as a follow-up bead.
2. **Splunk's container-first overlap.** **Status:** resolved by the coordinator, 2026-11-05: option (b), defer Splunk. `splunk.app` must win over `splunk.dashboard` on purpose, which the engine reports as an overlap (settings warning; site restore refuses). Accepting declared intentional overlaps is a design change of its own, not part of a data PR. D2 ships Box and Webex; Task 3 is skipped; a bead records "declared container-first overlaps (Splunk)". Cost if wrong: Splunk ships one PR later.
   - the generated `overlap_report.starter.splunk-cloud` case, which always expects `{"pairs": []}`, fails;
   - settings shows "1 adapter rule overlap needs review" for every Splunk user (`extension/options.ts` ~l.224);
   - site restore refuses overlapping rules (`host/src/config.rs` ~l.2523).

   The choices:
   - (a) Teach the generator a second `overlaps` expectation for a declared container-first pair, and accept the settings warning and the restore refusal. That is code in `scripts/generate-starters.mjs`, which `gate:adapter` does not cover.
   - (b) Ship Splunk later, in its own PR, once the engine has a way to declare an intended overlap.

   **Default if unresolved:** (b). D2 ships Box and Webex only (Tasks 1, 2, 4), and Task 3 is skipped and recorded as a follow-up bead.
3. **Webex's recording title keeps the research's single ` - ` suffix template.** **Status:** builder's call. The coordinator's two-dash rule names Google and Figma, and the scorecard confirmed Webex titles once loaded. **Default if rejected:** add `"{title} – {~}"` as a second entry.
4. **All families ship `development`.** **Status:** resolved (coordinator, 2026-11-05).

## Global Constraints

- Worktree: `git -C /Users/jrauser/ENDURANCE/work/rb-starter-c fetch origin && git -C /Users/jrauser/ENDURANCE/work/rb-starter-c worktree add /Users/jrauser/ENDURANCE/work/rb-starters-d2 -b feat/starters-d2 origin/main`. Never touch `/Users/jrauser/ENDURANCE/work/rauserBrowser`, and never `git stash`.
- Node 24.14.0 (`source ~/.nvm/nvm.sh && nvm use 24.14.0`). Cargo: `export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH`, with the worktree's own `./target`.
- Every rule in D1's "Global Constraints" from "Type names" through "Doc pages" applies unchanged: type-name grammar, the fixed per-type fields, adapters copied from the research TOML minus `origin`, synthetic data only, the vector shape and workflow (TS engine against `synthesizeFixture` via `scripts/adapter-templates-eval.mjs`), samples with computed shapes and the `samples_shape` flip rule, and the doc-page layout with `Status: development draft.`. On this plan's main branch they are in `docs/superpowers/plans/2026-11-05-starters-d1-google-figma-vidcast.md`.
- **Subdomain origins.** Research URLs on another host are rewritten to the family's `sample_origin`, and their expected artifact IDs follow. A negative only counts toward coverage when its URL is on `sample_origin` exactly. URLs on excluded hosts (`www.box.com`, `web.webex.com`, `help.webex.com`) aren't on the family's origin, so they are not carried as vectors. The doc lists them under "What the draft does not record".
- Commit trailer `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The PR body ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`. Merge only through the queue with the `queue` label plus `@Mergifyio queue`, never `gh pr merge`.

## Review Focus

1. A Box file reached as `/s/<share>/file/<id>`, `/v/<vanity>/file/<id>` and `/file/<id>` must get one ID (Task 1 keeps the research's ID-stability vectors).
2. `/folder/0`, the "All Files" root, must match nothing (Task 1 negative).
3. A Webex recording opened through any of the six player routes, in either the `recordingservice` or the `webappng` context and in either hex case, must get one lowercase ID (Task 2 keeps those vectors).
4. Webex's interstitial titles `Recorded Meeting Password` and `Replay Recorded Meeting` are recorded whole until the real title loads. The doc must say so, and a vector pins the whole-title fallback (Task 2).
5. Splunk's `/en-US/app/search/search` must be `splunk.app`, never `splunk.dashboard` (Task 3, if it runs).

---

### Task 1: `box` family (files, folders, shared links)

**Files:**
- Create: `protocol/starters/box.json`, `docs/adapters/box.file.md`, `docs/adapters/box.folder.md`, `docs/adapters/box.share.md`
- Regenerate: `extension/starter-families.generated.ts`, `protocol/capture-site/catalog.json`, `protocol/capture-site/starter-shapes.json`, `protocol/adapter-templates/starter-vectors.json`, `docs/adapters/README.md`, `protocol/starters/ids.lock.json` (append)

**Interfaces:**
- Consumes: research `docs/research/blind/box/` (the `acme.app.box.com` block of `adapters.toml`).
- Produces: family `box`, types `box.file` (0), `box.folder` (1), `box.share` (2).

Family fields: `display_name` `"Box"`. `review_prompt` `"Choose the kinds of Box items to remember on this Box site."`. `origin` `{ "kind": "subdomain", "suffix": ".app.box.com", "labels": 1, "exclude": ["www", "cloud"] }`. `scope` `null`. `sample_origin` `"https://acme.app.box.com"`, `sample_scope` `[]`. `overlaps`: 3 pairs `"disjoint"`. Labels `Box files` / `Box folders` / `Box shared links`. record_labels `files` / `folders` / `shared links`. Adapters are verbatim from the research, including `title` `"{title} | Powered by Box"` and the `officeonline` query rule.

- [ ] **Step 1: Scaffold and see it fail.** Run `npm run new:starter -- box box.file --subdomain .app.box.com`, then `npm run new:starter -- box box.folder` and `npm run new:starter -- box box.share`. Expected: the fill-in list. The scaffolder writes `labels: 1`, `exclude: ["www"]`. Extend the exclude list as above.
- [ ] **Step 2: Fill the family file.**
- [ ] **Step 3: Write vectors** from the 51 research cases. Rewrite the `app.box.com` URLs onto `acme.app.box.com`, and drop `account.box.com` and `www.box.com`. Keep `/folder/0`, `/notes/create_note`, `/file/<id>/<anything>` and `/folder/<id>/search` as negatives.
- [ ] **Step 4: Add samples:** `/file/<12-digit id>` titled `Q3 plan.pdf | Powered by Box`, `/s/<32-char share name>/file/<id>`, `/folder/<id>` titled `Press materials | Powered by Box`, and `/s/<share name>` for `box.share`.
- [ ] **Step 5: Write the three doc pages.** The `box.file` page's "Known limits" states what Open decision 1 leaves out (`app.box.com`, `*.ent.box.com`) and the enterprise-name loading title.
- [ ] **Step 6: Generate and run the focused checks.**
  Run: `npm run gen:starters && node scripts/generate-starters.mjs --check && npm run build:extension && node --test extension/tests/starter-registry.test.mjs extension/tests/adapter-templates.test.mjs extension/tests/site-patterns.test.mjs extension/tests/starter-families.test.mjs && cargo test --locked -p adapter-templates --test vectors --test structural --test shapes && npm run test:adapter-diff`
  Expected: three `appended lock entry` lines, then all pass, including `overlap_report.starter.box`.
- [ ] **Step 7: Commit** with message `feat(starters): box development starters (<company>.app.box.com)`.

### Task 2: `webex` family (recordings, meetings)

**Files:**
- Create: `protocol/starters/webex.json`, `docs/adapters/webex.recording.md`, `docs/adapters/webex.meeting.md`
- Regenerate: the Task 1 outputs

**Interfaces:**
- Consumes: research `docs/research/blind/webex/`.
- Produces: family `webex`, types `webex.recording` (0) and `webex.meeting` (1).

Family fields: `display_name` `"Webex Meetings"`. `review_prompt` `"Choose the kinds of Webex work to remember on this Webex site."`. `origin` `{ "kind": "subdomain", "suffix": ".webex.com", "labels": 1, "exclude": ["www", "web", "teams", "help", "developer", "admin", "status", "cart", "idbroker", "identity", "apphub", "signin", "settings", "eaf-sdk", "web-sdk", "downloadservicebts", "mrsprod"] }` (research "Origin model"). `scope` `null`. `sample_origin` `"https://example.webex.com"`. `overlaps` `[{ "a": "webex.recording", "b": "webex.meeting", "expect": "disjoint" }]`. Labels `Webex recordings` / `Webex meeting summaries`. record_labels `recordings` / `meetings`. Adapters are verbatim from the research (`webex.meeting` has no `title`).

- [ ] **Step 1: Scaffold and see it fail.** Run `npm run new:starter -- webex webex.recording --subdomain .webex.com`, then `npm run new:starter -- webex webex.meeting`.
- [ ] **Step 2: Fill the family file.**
- [ ] **Step 3: Write vectors** from the 41 research cases. The `site4-example.webex.com` case stays as a positive on its own host (it is on the family's origin, but does not count toward coverage). Drop `web.webex.com` and `help.webex.com`. Add `eval.webex.recording.password_title`: title `Recorded Meeting Password`, where `expect.title` is `Recorded Meeting Password` and `title_entry` is `null`. The template needs ` - `, so the title is kept whole.
- [ ] **Step 4: Add samples:** a `/recordingservice/sites/example/recording/<32 hex>/playback` URL titled `Design review - Example Site`, and a `/webappng/sites/example/meeting/postinfo/<token>` URL with `title: null`.
- [ ] **Step 5: Write the two doc pages.** "Known limits" covers the interstitial titles, which are 15 of the scorecard's 20 visits, and the legacy `ldr.php` forwards, which are not recorded.
- [ ] **Step 6: Generate and run the focused checks:** Task 1's Step 6 command.
- [ ] **Step 7: Commit** with message `feat(starters): webex development starters`.

### Task 3 (only if Open decision 2 resolves to (a)): generator accepts a container-first pair, then the `splunk-cloud` family

**Files:**
- Modify: `scripts/generate-starters.mjs`: `validateOverlaps` (~l.625–670) and `renderStarterVectors` (~l.963–990)
- Test: `scripts/tests/generate-starters.test.mjs`
- Modify: `protocol/starters/README.md`, "Test vectors and overlaps"
- Create: `protocol/starters/splunk-cloud.json`, `docs/adapters/splunk.app.md`, `docs/adapters/splunk.dashboard.md`
- Regenerate: the Task 1 outputs

**Interfaces:**
- Produces: a new overlaps entry form, `{ "a": <container type>, "b": <contained type>, "expect": "container_first", "pairs": [[<a rule>, <b rule>], ...] }`. It is valid only when `b`'s `adapter.within.type` is `a`, `a` comes before `b` in `types`, and every rule index is in range. `renderStarterVectors` emits, for each such pair, `{ "first": [<index of a>, <a rule>], "second": [<index of b>, <b rule>] }` in the family's `overlap_report` `expect.pairs`, sorted by `first` then `second`. `"disjoint"` entries are unchanged.
- Family `splunk-cloud`, types `splunk.app` (0), then `splunk.dashboard` (1, `within` `splunk.app`).

- [ ] **Step 1: Write the failing generator tests** in `scripts/tests/generate-starters.test.mjs`, using its existing `tmpRoot` harness:
  - `container_first pair becomes an expected overlap_report pair`: the family's `overlap_report` case `expect` deep-equals `{ pairs: [{ first: [0, 1], second: [1, 0] }] }`;
  - `container_first needs b within a`: the error is `overlaps[0].expect "container_first" needs <b> within <a>`;
  - `container_first needs a before b`: the error is `overlaps[0].expect "container_first" needs <a> before <b>`;
  - `container_first rule index out of range`: the error is `overlaps[0].pairs[0] names no rule`;
  - `disjoint still refuses pairs`: the error is `overlaps[0].pairs is only for "container_first"`.
- [ ] **Step 2: Run them to see them fail.** Run `node --test scripts/tests/generate-starters.test.mjs`. Expected: the five new tests FAIL, with today's `must be "disjoint"`.
- [ ] **Step 3: Implement** the entry form in `validateOverlaps` and the expected pairs in `renderStarterVectors`. Add `pairs` to `OVERLAP_KEYS` as optional. Document the form in the README.
- [ ] **Step 4: Run them to see them pass.** Run `node --test scripts/tests/generate-starters.test.mjs scripts/tests/new-starter.test.mjs`. Expected: PASS.
- [ ] **Step 5: Scaffold and fill `splunk-cloud`.** Run `npm run new:starter -- splunk-cloud splunk.app --subdomain .splunkcloud.com`, then `… splunk-cloud splunk.dashboard`. Family fields: `display_name` `"Splunk Cloud"`. `review_prompt` `"Choose the kinds of Splunk work to remember on this Splunk Cloud stack."`. `origin` `{ "kind": "subdomain", "suffix": ".splunkcloud.com", "labels": 1, "exclude": ["www"] }`. `scope` `null`. `sample_origin` `"https://acme.splunkcloud.com"`. `overlaps` `[{ "a": "splunk.app", "b": "splunk.dashboard", "expect": "container_first", "pairs": [[1, 0]] }]`. Labels `Splunk apps` / `Splunk dashboards`. record_labels `apps` / `dashboards`. Adapters are verbatim from the research, keeping `splunk.dashboard`'s `within` and both `title` entries. Vectors come from the 40 research cases (the `:8089` case is carried, since it is on the origin). They must include `/en-US/app/search/search` → `splunk.app` rule 1. Samples: `/en-US/app/search/search` (app) and `/en-US/app/search/ops_overview` titled `Ops overview | Splunk Cloud`. The docs' "Known limits" states that `<prefix>.<stack>.splunkcloud.com` search heads aren't claimed under `labels: 1`, and that settings shows one overlap warning by design.
- [ ] **Step 6: Generate and run the focused checks.** Run Task 1's Step 6 command, plus `cargo test --locked -p brauser capture_site`. Expected: `overlap_report.starter.splunk-cloud` passes with its one declared pair.
- [ ] **Step 7: Commit** the generator change and the family separately: `feat(starters): overlaps may declare a container-first pair` and `feat(starters): splunk-cloud development starters`.

### Task 4: README starter list, gate, PR

**Files:**
- Modify: `README.md`, the step-5 starter sentence
- Maybe create: `docs/improvement-feed/2026-11-05-<slug>.md`

- [ ] **Step 1: Extend the README sentence** to name Box, Webex Meetings and (if shipped) Splunk Cloud pages, keeping the pointer to `docs/adapters/README.md`.
- [ ] **Step 2: Add an improvement-feed entry only for a new lesson.** If Open decision 2 went to (b), the lesson is: "research must check that its intended overlaps are expressible in the generator's `overlaps`". If no new lesson appeared, skip and say so in the PR body.
- [ ] **Step 3: File follow-up beads** with `scripts/dev/create-bead` for whatever Open decisions 1 and 2 deferred: Box `app.box.com` and `*.ent.box.com` coverage, and, under (b), Splunk.
- [ ] **Step 4: Run the gate once.** Run `npm run gate:adapter`. Expected: PASS. If Task 3 ran, it exits 2 (`scripts/` is out of scope). Then run the README gate.
- [ ] **Step 5: Run one Codex review** with `codex exec review --base origin/main`, then push and open the PR with `gh pr create --title "Starters D2: Box, Webex, Splunk Cloud (development drafts)"`. Use `Box, Webex` in the title if Splunk was skipped. The body states the Box origin choice and the Splunk decision outcome.
- [ ] **Step 6: Queue and watch.** Run `gh pr edit <n> --add-label queue && gh pr comment <n> --body "@Mergifyio queue"`, then `node scripts/dev/pr-watch.mjs <n>` until merged. D3 starts from main afterwards.
