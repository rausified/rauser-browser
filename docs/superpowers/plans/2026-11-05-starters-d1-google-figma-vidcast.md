# Research-built starters, PR D1: Google Workspace, Figma, Vidcast

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship three research-built starter families, `google-workspace`, `figma` and `vidcast`, as development drafts: family files, doc pages and regenerated outputs only.

**Architecture:** Each family is one `protocol/starters/<family>.json` plus one `docs/adapters/<type>.md` per type, scaffolded with `npm run new:starter` and filled from `docs/research/blind/<site>/`. The scorecard's fixes are applied (both dashes in title suffixes, no `▶` on Figma prototypes). `npm run gen:starters` writes everything else. No engine, host or extension code changes.

**Tech Stack:** JSON family files, Markdown docs, Node 24.14.0 (`scripts/generate-starters.mjs`, `scripts/new-starter.mjs`, `scripts/adapter-templates-eval.mjs`, `node:test`), Rust `adapter-templates` integration tests.

**Spec:** `docs/superpowers/specs/2026-11-03-starter-families-design.md` ("Decisions", "Builder's calls" 3, 5, 7–9, "Research method"); `protocol/starters/README.md` (the family-file contract); `docs/research/blind/SCORECARD.md`; research inputs `docs/research/blind/{google-workspace,figma,vidcast}/`.

**Precondition:** PR #322 (`feat/starter-families-c`, the scaffolder and the registry checks) has merged to main. Start from current `origin/main`.

## Design challenge

- **Challenger:** the plan author's self-challenge. A data-only plan of four tasks needs no outside reviewer (AGENTS.md "Before building").
- **Alternative:** paste each research `adapters.toml` into the family files unchanged, with one family per research folder, so the PR stays a mechanical port.
- **Outcome:** kept the plan. A straight port would fail the generator: `google.form.responder` breaks the type-name rule, and Drive and Calendar sit off `docs.google.com`. It would also ship the two title misses the scorecard found (the en dash and Figma's `▶`). The port is still adopted where the research holds: rules, ID templates and modes are copied verbatim except for the changes listed per task.

## Open decisions

- **Google Workspace ships only `docs.google.com` types; Drive, Calendar and Gmail wait.** **Status:** resolved (coordinator, 2026-11-05; PR C Open decision 2). Gmail's message permalink (`mail/u/<n>/?ik=…&view=pt&permmsgid=msg-f:…`) is recorded in the doc as a future type, not a rule.
- **All three families ship `status.state: "development"`, `method: null`.** **Status:** resolved (coordinator, 2026-11-05, from John's decision 2).
- **Figma titles gain a hyphen entry `{title} - {~}`, so a prototype tab `<file> - <page>` records `<file>`.** **Status:** builder's call (this plan), open to coordinator correction. **Default if rejected:** drop the entry, and prototypes record the whole `<file> - <page>` title, which the scorecard called acceptable.
- **Google Docs, Sheets and Slides keep the research's `refs: [{ "type": "jira.issue", "find": "jira_key" }]`.** **Status:** builder's call. **Default if rejected:** remove `refs` from the three adapters.
- **`vidcast.page` ships despite low confidence in its UUID ID shape.** **Status:** builder's call. If page IDs aren't UUIDs, the rules never match (fail closed). **Default if rejected:** drop the type.
- **The settings gallery hint (PR C Open decision 9) stays as is in D1.** **Status:** deferred to D3, which has all twelve families.

## Global Constraints

- Work in a new worktree, never in `/Users/jrauser/ENDURANCE/work/rauserBrowser`, and never `git stash`: `git -C /Users/jrauser/ENDURANCE/work/rb-starter-c fetch origin && git -C /Users/jrauser/ENDURANCE/work/rb-starter-c worktree add /Users/jrauser/ENDURANCE/work/rb-starters-d1 -b feat/starters-d1 origin/main`.
- Use Node 24.14.0 (`source ~/.nvm/nvm.sh && nvm use 24.14.0`; `/usr/local/bin/node` is v22). Cargo: `export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH`. Use the worktree's own `./target`. Never point `CARGO_TARGET_DIR` at a sibling worktree.
- Type names match `^[a-z][a-z0-9]*\.[a-z][a-z0-9]*$`: one dot, no `_` or `-` after it.
- Every type: `"key": "<type>:{origin}"` (builder's call 8), `"label_variants": []`, `"example_path": null`, `"history": []`, `"status": { "state": "development", "method": null, "checked_on": null, "checked_by": null }`, `"docs": "docs/adapters/<type>.md"`. `record_label` is a plural noun, never `"TODO"`.
- An `adapter` is the research TOML artifact converted to JSON, with `origin` removed and `"unscoped": "acknowledged"` kept on unscoped families. Change it only as each task says.
- **Synthetic data only.** Sample and vector URLs, IDs and titles are invented or come from the research's own synthetic vectors. Never use an ID or title from the scorecard's answer key. Doc pages cite the scorecard's counts, never its URLs.
- **Vectors.** Each type's `vectors` are evaluate cases in the `eval.miro.*` shape of `protocol/adapter-templates/vectors.json`, without `doc`. A positive's `expect` is `{match: true, adapter, rule, type, captures, artifact_id, mode}`, plus `title, title_entry, alias_id, refs` when the case has a `title`. A negative's `expect` is `{match: false, reason}`. Each `id` is `eval.<family>.<short case name>`, unique, and `impls` is `["rust", "ts"]`. Every match rule gets at least one positive. Every type gets at least one negative whose URL is on the family's `sample_origin`.
- **Vector workflow.** Carry over every research case on the family's origin whose URL belongs to a kept type, or to no type (a negative). Run them through the TS engine against the synthesized fixture, check each result against the research's own expectation fields (type, ID, mode, extracted title), and fix the family file, not the expectation, where they disagree. The only exceptions are the deliberate changes in each task. Then copy the engine's `adapter`, `rule` and `captures` into the vector. To get results (after `npm run build:extension`), write a throwaway script under `/tmp`, never committed. It reads the family file, builds `doc` with `synthesizeFixture` from `scripts/generate-starters.mjs`, and prints one `{"doc", "url", "title"?}` request per line into `node scripts/adapter-templates-eval.mjs`, which prints one evaluate result per line.
- **Samples.** One sample per URL form worth distinguishing, with a non-null `title` on at least one sample of every type that has a `title` template. Compute each `shape` with `node -e 'import("./extension/dist/site-patterns.js").then(({ inspectSitePatterns }) => console.log(inspectSitePatterns("<url>").current))'`. Never hand-write one. Start with `samples_shape: true`. Flip a type to `false` only if `cargo test --locked -p adapter-templates --test structural starter_recorded_urls` names that type's samples, and say why in the doc's "Recorded samples".
- **Doc pages.** Line 3 is `Status: development draft.`, followed by one sentence naming the research folder and saying it was built blind, from public sources only. The page keeps the scaffolder's six `## ` sections (`What each rule covers`, `Titles`, `Recorded samples`, `What the draft does not record`, `Known limits`, `Live check still needed`), none left as `TODO`. "Recorded samples" lists the synthetic samples and the scorecard result for that site. "Live check still needed" is the research's "How I would verify … with a real account" list, corrected for this plan's changes. It ends with the certification steps in `docs/adapters/miro.board.md`'s last paragraph.
- Commit messages end with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. The PR body ends with `🤖 Generated with [Claude Code](https://claude.com/claude-code)`.
- Merge only through the queue: `gh pr edit <n> --add-label queue` and `gh pr comment <n> --body "@Mergifyio queue"`. Never `gh pr merge`.

## Review Focus

1. A Google file reached at `/u/<n>/…` or `/a/<domain>/…` must get the same artifact ID as the plain `/document/d/<id>/edit` form. The research's multi-account vectors must survive the port (Task 1).
2. A Google tab ending in ` – Google Docs` (en dash) must record the bare name, not the whole title (Task 1 adds an en-dash vector per type).
3. A responder link `/forms/d/e/<id>/viewform` must be `google.formresponse` and never `google.form` (Task 1's negative for `google.form`).
4. A Figma file still loading has the tab title `Figma`, which must be recorded whole, and `/design/new` must match nothing (Task 2).
5. A Vidcast editor visit records the fixed title `Video Edit`. The doc must state this limit, and `/share/edit-script/<id>/<upload>` must match nothing (Task 3).

---

### Task 1: `google-workspace` family (Docs, Sheets, Slides, Forms editor, Forms responder)

**Files:**
- Create: `protocol/starters/google-workspace.json`
- Create: `docs/adapters/google.doc.md`, `docs/adapters/google.sheet.md`, `docs/adapters/google.slides.md`, `docs/adapters/google.form.md`, `docs/adapters/google.formresponse.md`
- Regenerate: `extension/starter-families.generated.ts`, `protocol/capture-site/catalog.json`, `protocol/capture-site/starter-shapes.json`, `protocol/adapter-templates/starter-vectors.json`, `docs/adapters/README.md`, `protocol/starters/ids.lock.json` (append only)

**Interfaces:**
- Consumes: research `docs/research/blind/google-workspace/{adapters.toml,vectors.json,research.md}`.
- Produces: family `google-workspace`, types in this order (adapter indices 0–4): `google.doc`, `google.sheet`, `google.slides`, `google.form`, `google.formresponse`.

Family fields: `display_name` `"Google Workspace (Docs, Sheets, Slides, Forms)"`. `review_prompt` `"Choose the kinds of Google files to remember."`. `origin` `{ "kind": "exact", "origin": "https://docs.google.com" }`. `scope` `{ "kind": "unscoped", "notice": "Records every Google document, spreadsheet, presentation and form from every Google account in this Chrome profile." }`. `sample_origin` `"https://docs.google.com"`, `sample_scope` `[]`. `overlaps`: all 10 pairs `"disjoint"`.

| Type | label | record_label | adapter change from research |
|---|---|---|---|
| google.doc | Google Docs documents | documents | `title` → `["{title} - Google Docs", "{title} – Google Docs"]` |
| google.sheet | Google Sheets spreadsheets | spreadsheets | `title` → `["{title} - Google Sheets", "{title} – Google Sheets", "{title} - Google Drive", "{title} – Google Drive"]` (the four-entry limit) |
| google.slides | Google Slides presentations | presentations | `title` → `["{title} - Google Slides", "{title} – Google Slides"]` |
| google.form | Google Forms you edit | forms | `title` → `["{title} - Google Forms", "{title} – Google Forms"]` |
| google.formresponse | Google Forms you fill in | form responses | research `google.form.responder` renamed: `type` and `id` → `google.formresponse:{host}/{id}`; no `title` |

Descriptions: one sentence each, saying the item becomes one artifact however it was opened, and that the starter records from every Google account in the profile.

- [ ] **Step 1: Scaffold and see it fail.** Run `npm run new:starter -- google-workspace google.doc --exact https://docs.google.com --scope unscoped`, then `npm run new:starter -- google-workspace <type>` for each of the other four. Expected: each exits 0 and prints a "Fill these in" list that includes `TODO` placeholders and `match` being empty.
- [ ] **Step 2: Fill the family file** per the table, the Global Constraints and the research rules. Drop the research's `google.drive.file`, `google.drive.folder` and `google.calendar.event`.
- [ ] **Step 3: Write vectors** per the vector workflow, from the research's `docs.google.com` cases only. Drive, Calendar and Gmail cases are on other origins and are not carried. Add these:
  - per titled type, one en-dash title vector: for example `eval.google-workspace.doc.endash` with title `Team plan – Google Docs`, whose `expect.title` is `Team plan` and `title_entry` is `1`;
  - `eval.google-workspace.formresponse.viewform`: `https://docs.google.com/forms/d/e/<56-char synthetic id>/viewform` must match `google.formresponse` (adapter 4, mode `view`). First match wins in adapter order, and `google.form` (adapter 3) comes earlier, so this positive also proves `google.form` never claims a responder link;
  - one negative per type on `https://docs.google.com`: the home list `/document/u/0/`, `/spreadsheets/d/<id>/copy`, `/presentation/d/<id>/export`, `/forms/u/0/` and `/forms/d/e/<id>/prefill`, each `{match: false, reason: "no_rule"}`.
- [ ] **Step 4: Add samples:** an edit URL for each type (44-character synthetic IDs, for example `Team plan - Google Docs`), a `/u/1/` form for `google.doc`, and a `/forms/d/e/<id>/viewform` sample with `title: null` for `google.formresponse`. Compute shapes from the built shaper.
- [ ] **Step 5: Write the five doc pages** per the Global Constraints. The `google.doc` page's "Known limits" also states two things. First, Gmail is not covered, and the message permalink form `mail.google.com/mail/u/<n>/?ik=…&view=pt&permmsgid=msg-f:…` is a possible future type (scorecard miss 2). Second, Drive and Calendar wait for the shared-origin migration.
- [ ] **Step 6: Generate and run the focused checks.**
  Run: `npm run gen:starters && node scripts/generate-starters.mjs --check && npm run build:extension && node --test extension/tests/starter-registry.test.mjs extension/tests/adapter-templates.test.mjs extension/tests/site-patterns.test.mjs extension/tests/starter-families.test.mjs && cargo test --locked -p adapter-templates --test vectors --test structural --test shapes && npm run test:adapter-diff`
  Expected: `gen:starters` prints `appended lock entry google.<x> → google.<x>:{host}/{id}` five times. Everything after it passes. `overlap_report.starter.google-workspace` passes with `{"pairs": []}`.
- [ ] **Step 7: Commit:** `git add protocol/starters docs/adapters extension/starter-families.generated.ts protocol/capture-site protocol/adapter-templates/starter-vectors.json && git commit -m "feat(starters): google-workspace development starters (docs.google.com)"`

### Task 2: `figma` family (files and branches)

**Files:**
- Create: `protocol/starters/figma.json`, `docs/adapters/figma.file.md`, `docs/adapters/figma.branch.md`
- Regenerate: the same outputs as Task 1

**Interfaces:**
- Consumes: research `docs/research/blind/figma/`.
- Produces: family `figma`, types `figma.file` (0) and `figma.branch` (1).

Family fields: `display_name` `"Figma"`. `review_prompt` `"Choose the kinds of Figma work to remember."`. `origin` `{ "kind": "exact", "origin": "https://www.figma.com" }`. `scope` `{ "kind": "unscoped", "notice": "Records every Figma file and branch from every Figma account in this Chrome profile." }`. `sample_origin` `"https://www.figma.com"`. `overlaps` `[{ "a": "figma.file", "b": "figma.branch", "expect": "disjoint" }]`. Labels `Figma files` / `Figma branches`. record_labels `files` / `branches`.

Adapter change from research, both types: `title` → `["{~} · {title} - Figma Slides", "{title} – {~}", "{title} - {~}"]`. This drops `"▶ {title}"` (scorecard miss 4) and adds the hyphen entry (Open decision 3).

- [ ] **Step 1: Scaffold and see it fail.** Run `npm run new:starter -- figma figma.file --exact https://www.figma.com --scope unscoped`, then `npm run new:starter -- figma figma.branch`. Expected: the fill-in list.
- [ ] **Step 2: Fill the family file.**
- [ ] **Step 3: Write vectors** per the vector workflow, from the research's `positive` and `negative` lists. Fix up the research's prototype cases that expect `▶`: the tab title becomes `Checkout Flow - Onboarding`, with `expect.title` `Checkout Flow` and `title_entry` `2`. Also add:
  - `eval.figma.file.loading`: title `Figma`, where `expect.title` is `Figma` and `title_entry` is `null`;
  - `eval.figma.file.hyphen_name`: title `A - B – Figma`, where `expect.title` is `A - B` and `title_entry` is `1`;
  - negatives on `https://www.figma.com`, including `/design/new`, `/files/recents-and-sharing` and `/design/<22-char key>/image/<hash>`.
- [ ] **Step 4: Add samples:** a `/design/<key>/<slug>` file titled `Checkout Flow – Figma`, a `/proto/<key>` prototype, and a `/design/<main>/branch/<branch>/<slug>` branch.
- [ ] **Step 5: Write the two doc pages.** In "Live check still needed", replace research step 3 (`▶ name`) with: "Confirm a prototype tab reads `<file> - <page>` and records `<file>`."
- [ ] **Step 6: Generate and run the focused checks:** Task 1's Step 6 command. Expected: two lock appends, then all pass.
- [ ] **Step 7: Commit** with message `feat(starters): figma development starters`.

### Task 3: `vidcast` family (videos, playlists, pages)

**Files:**
- Create: `protocol/starters/vidcast.json`, `docs/adapters/vidcast.video.md`, `docs/adapters/vidcast.playlist.md`, `docs/adapters/vidcast.page.md`
- Regenerate: the same outputs as Task 1

**Interfaces:**
- Consumes: research `docs/research/blind/vidcast/`.
- Produces: family `vidcast`, types `vidcast.video` (0), `vidcast.playlist` (1), `vidcast.page` (2).

Family fields: `display_name` `"Vidcast"`. `review_prompt` `"Choose the kinds of Vidcast work to remember."`. `origin` `{ "kind": "exact", "origin": "https://app.vidcast.io" }`. `scope` `null`: not a shared origin, like Miro. `sample_origin` `"https://app.vidcast.io"`. `overlaps`: 3 pairs `"disjoint"`. Labels `Vidcast videos` / `Vidcast playlists` / `Vidcast pages`. record_labels `videos` / `playlists` / `pages`. Each description ends with "Records videos from every Vidcast account signed in to this Chrome profile." (Miro's wording). Adapters are copied verbatim from the research, including the prefix-form titles `Vidcast - {title}` and `Vidcast - Playlist - {title}`.

- [ ] **Step 1: Scaffold and see it fail.** Run `npm run new:starter -- vidcast vidcast.video --exact https://app.vidcast.io`, then the other two types.
- [ ] **Step 2: Fill the family file.**
- [ ] **Step 3: Write vectors** from all 58 research cases. The `www.vidcast.io` and `alpha.vidcast.io` cases are off-origin and are not carried. Keep `/share/edit-script/<id>/<upload>`, `/video/<id>` and `/playlists` as negatives.
- [ ] **Step 4: Add samples:** `/share/<uuid>` titled `Vidcast - Q3 roadmap walkthrough`, `/share/edit/<uuid>`, `/playlists/<uuid>` titled `Vidcast - Playlist - Onboarding`, and `/pages/<uuid>` titled `Vidcast - Team updates`.
- [ ] **Step 5: Write the three doc pages.** `vidcast.page`'s "Known limits" states the low-confidence UUID assumption and that its rules fail closed. `vidcast.video`'s states the fixed `Video Edit` editor title.
- [ ] **Step 6: Generate and run the focused checks:** Task 1's Step 6 command.
- [ ] **Step 7: Commit** with message `feat(starters): vidcast development starters`.

### Task 4: README starter list, gate, PR

**Files:**
- Modify: `README.md`, the sentence in step 5 that names the starter sites (`On a Jira or Confluence Cloud …, or Miro board page`)
- Maybe create: `docs/improvement-feed/2026-11-05-<slug>.md`

- [ ] **Step 1: Extend the README sentence** to name Google Docs, Sheets, Slides and Forms, Figma and Vidcast pages. Keep its pointer to `docs/adapters/README.md`, the generated status index, which now lists all 17 types.
- [ ] **Step 2: Add an improvement-feed entry only if Tasks 1–3 taught something the scorecard's four lessons don't already cover.** One example would be a type whose samples the structural drafter couldn't learn. Use the `## YYYY-MM-DD — Title` file format (AGENTS.md "Brauser Improvement Feed"). Otherwise skip this step and say so in the PR body.
- [ ] **Step 3: Run the full adapter gate once.**
  Run: `npm run gate:adapter`
  Expected: every step PASS, `Total …: PASS`. If it exits 2 (out of scope), run the README gate instead.
- [ ] **Step 4: Run one Codex review.** Run `codex exec review --base origin/main` once. Fix P1s test-first, and file P2s and P3s as beads (AGENTS.md "Review and merge cadence").
- [ ] **Step 5: Commit, push, open the PR.** Run `git push -u origin feat/starters-d1` and `gh pr create --title "Starters D1: Google Workspace, Figma, Vidcast (development drafts)"`. The body lists the 10 types, the scorecard result per site, and the open decisions above.
- [ ] **Step 6: Queue and watch.** Run `gh pr edit <n> --add-label queue && gh pr comment <n> --body "@Mergifyio queue"`, then `node scripts/dev/pr-watch.mjs <n>`. Expected: it exits at merge. Act on any exit code it prints. D2 starts from main after this merges.
