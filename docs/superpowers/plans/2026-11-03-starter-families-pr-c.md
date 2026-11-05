# Starter families, PR C: generated shapes and vectors, the registry guards and the scaffolder

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A new starter family is one family file, one doc page and regenerated files. Recorded samples and test vectors live in the family file. The generator names every place a starter is missing from or inconsistent with, in one pass. `npm run new:starter` lays out the files.

**Architecture:** The family files gain samples, vectors and overlaps. `scripts/generate-starters.mjs` writes four more outputs from them: `protocol/capture-site/starter-shapes.json`, `protocol/adapter-templates/starter-vectors.json` (with one synthesized `starter:<family>` fixture per family), `docs/adapters/README.md` and new `ids.lock.json` entries. Its validation also runs the pure registry checks of spec §4: check 2's reverse direction, checks 3 and 5, and the shared-origin policy (checks 6 and 7). Check 1 needs no separate assertion, because the generated outputs and `--check` already cover it. The Rust and TS test loaders read each generated file beside its hand-kept twin. Only the checks that need the built engine stay in a test: `extension/tests/starter-registry.test.mjs` runs check 4 (vector and title coverage) and check 8 (`--check` is clean). Subdomain families can now be strict (`labels`, `exclude`). The host's pinned `taken_types` is computed from the catalog.

**Tech Stack:** Node 24.14.0 scripts and `node:test`, TypeScript (`tsc` via `extension/build.mjs`), Rust integration tests (`adapter-templates`, `brauser` lib tests), JSON.

**Spec:** `docs/superpowers/specs/2026-11-03-starter-families-design.md`: Builder's calls 2, 4, 7 and 8; §2 (Generated files, CI wiring, Compatibility); §4 (Scaffolder, Registration-completeness test); §5 PR C steps 7–10. It also covers PR A's deferred step 3 (`docs/superpowers/plans/2026-11-03-starter-families-pr-a.md`). PR B's final review lists nine "PR D readiness gaps"; this plan maps each one to a task, or to a reasoned deferral (table below). Builds on PR B (#296, branch `feat/starter-families-b`). It also assumes the separate notice fix (`docs/superpowers/plans/2026-11-03-starter-families-notice-fix.md`, readiness gaps 3 and 5) has merged first. Rebase onto it before Task 1, since both edit `extension/tests/starter-fixtures.mjs`.

## Design challenge

- **Challenger:** an independent reviewer (Opus, assigned by the coordinator, 2026-11-03), after the plan author's own self-challenge (the first alternative below).
- **Alternative:** generate nothing new. Leave the 15 starter rows in `shapes.json` and the vectors in `vectors.json`. The registry test would only cross-check the family files against those hand-kept files. That means no Rust or TS loader changes, no edits to shared-seam tests, and three fewer generated outputs.
- **Outcome:** keep the plan. Under the alternative, a new family still hand-edits `shapes.json` and writes `vectors.json` cases with positional adapter indices into a fixture someone has to assemble. Those are spec §1 rows 13–14, the touch points this work exists to remove, and spec acceptance step 10 (the diff is only the family file, its doc and generated files) could never pass. Two parts of the alternative are adopted:
  - legacy vectors stay where they are (builder's call 2);
  - the loaders only concatenate files; they don't synthesize anything. The generator writes fixtures already resolved, so neither engine's tests learns about family files.
- **Independent challenge, adopted:**
  - (b) Fold the pure registry checks into the generator's validation, so the scaffolder's output lists everything in one pass and each message is tested in the generator's `tmpRoot` harness. The checks are check 2 reverse, 3, 5, 6 and 7, and gaps 1, 6 and 8. Drop check 1 as a separate assertion. The registry test keeps only checks 4 and 8.
  - (c) Keep the lock append; write mode prints `appended lock entry <type> → <id>`.
  - (d) The notice placement and draft-test work (gaps 3 and 5) moves to its own small plan and PR, which ships first.
  - (e2) The host test keeps a literal floor of today's types.
  - (e3) The moved-rows guard is a permanent test.
- **Independent challenge, rejected:** (e1) that the merge rule in Global Constraints is wrong. Main's AGENTS.md "Review and merge cadence" says to enqueue with `gh pr edit <n> --add-label queue` and never to run `gh pr merge`, and Mergify Merge Protections have been active since 2026-11-03. The objection read a stale AGENTS.md.

## PR D readiness gaps (PR B final review, `/tmp/pr-b-final-review.md`)

| Gap | Where |
|---|---|
| 1 `unscoped` off a shared origin | Task 5 (generator policy) |
| 2 three shared-origin lists | Task 3 pins them together; the two new origins are deferred (Open decision 2) |
| 3 multi-type unscoped notice | the notice-fix plan (ships first) |
| 4 strict subdomain matching | Task 4 |
| 5 draft tests and fixtures on origins PR D claims | the notice-fix plan (`draftModeAt`), Task 5 (fixture move and collision check) |
| 6 tenant scope key vs host tenant capture | Task 5 |
| 7 gallery hint length | deferred to PR D (Open decision 9) |
| 8 `github_org` outside GitHub | Task 5 |
| 9 `record_label` grouping documented | Task 6 (scaffolder placeholder and "Adding a starter") |

## Open decisions

Resolved by John (2026-11-03, in the spec): certification uses `status.method` `"live"` or `"history"`, and the docs index shows the method. The builder's calls below are made in this plan and are open to John's correction. None blocks Task 1, except that Task 6 assumes 1.

1. **The generator appends new lock entries.** **Status:** builder's call. The spec lists `ids.lock.json` among the generated files. `npm run gen:starters` (write mode) appends an entry for any type or `#within` key missing from the lock. An entry that differs still fails, and `--check` still prints `no lock entry for <type>`. For each new entry, write mode prints `appended lock entry <type> → <id>` (design challenge c). The lock freezes an ID at the first generation; before merging, an author may delete their own unmerged entry and regenerate. PR A's `checkLock` tests stay unchanged. **Default if rejected:** the scaffolder prints "add the lock entry by hand", and Task 6's acceptance diff is unchanged, but the author makes the lock edit.
2. **Adding `drive.google.com` and `calendar.google.com` (builder's call 4) is dropped for now.** **Status:** resolved by the coordinator, 2026-11-03. Adding them to the shared-origin lists would invalidate saved hand-written adapters on those origins on upgrade (`adapter-templates.md` §6) and need a migration. Instead, PR D's Google Workspace family ships only its `docs.google.com` types (Docs, Sheets, Slides, both Forms types: the high-confidence ones, and that origin is already shared); Drive files and folders and Calendar events (Calendar was low confidence) wait for a later PR that pairs the list change with a migration. Cost if wrong: one later PR.
3. **Overlaps are declared as `"disjoint"` only.** Each family gets one generated `overlap_report.starter.<family>` case expecting `{"pairs": []}`. Any other `expect` is refused: `overlaps[N].expect must be "disjoint"`. The legacy rule-pair overlap cases stay in `vectors.json`.
4. **Samples carry a `title`** (string or null). Legacy samples copy their titles from the type's existing titled vectors (`eval.jira.title`, `eval.miro.title`, …) or from the doc's "Titles" section. `starter-shapes.json` rows stay `{note, url, shape}`, so the shape format doesn't change.
5. **The origin-collision check (spec §4 check 6) is conservative.** A suffix inside another suffix is always a collision, whatever `labels` or `exclude` say. An exact origin collides with a suffix family only when that family would claim it (`familyClaims`). A repeated exact origin always collides.
6. **The doc `Status:` line has three exact openings:** `Status: release-certified <checked_on>.` (live), `Status: certified <checked_on> from real browsing history.` (history), and `Status: development draft.` Today's seven docs already match.
7. **`docs/adapters/` counts as code in CI.** `scripts/ci-changes.mjs` keeps it off the docs-only path, so a doc's `Status:` edit runs the generator check (check 3) and the registry test in CI.
8. **Strict subdomains allow only `labels: 1`.** `exclude` is a list of single lowercase DNS labels. The scaffolder writes `labels: 1` and `exclude: ["www"]` for a new `--subdomain` family (builder's call 7). Atlassian and SharePoint stay permissive.
9. **The gallery hint length (gap 7) is deferred to PR D**, which first has enough families to judge the copy.
10. **Test-only data stays out of the bundle.** `renderTs` drops `sample_origin`, `sample_scope`, `overlaps`, `samples`, `samples_shape` and `vectors`, so `extension/starter-families.generated.ts`'s `STARTER_FAMILIES` data is unchanged through Task 3.
11. **Fixture origins.** `EXAMPLE_UNSCOPED_FAMILY` moves from `https://www.figma.com` (PR D's Figma family) to `https://linear.app` (Task 5). The notice-fix plan's two-type fixture uses `https://www.notion.so` and its `draftModeAt` helper; that plan has its reasons.
12. **The scaffolder makes no bead.** It prints the `scripts/dev/create-bead` command (AGENTS.md "Work queue").
13. **The registry checks report on the type, in one pass.** `main` gathers validation, policy and registry problems into one list before it exits. Only a file that isn't valid JSON, or an object whose shape is too broken to read further, stops the later checks for that file. A check 2/3 problem reads `<path>: <message>`, where `<path>` is the doc or shapes file.
14. **Design challenge.** Independent reviewer (Opus, 2026-11-03). Adopted b, c, d, e2 and e3; rejected e1, which came from a stale AGENTS.md reading. **Status:** resolved by the coordinator, 2026-11-03.

## Global Constraints

- Node 24.14.0 for every script and test (`export PATH=~/.nvm/versions/node/v24.14.0/bin:$PATH`); no new npm or Cargo dependencies. Cargo files stay untouched.
- Today's seven starter types behave identically:
  - `extension/tests/starter-adapters.test.mjs` passes unchanged.
  - `STARTER_FAMILIES` data in the generated TS is byte-identical (only Task 4 changes its interface text).
  - `ids.lock.json`'s eight entries are unchanged.
- Legacy vectors stay in `protocol/adapter-templates/vectors.json` against `fixtures.starters` (builder's call 2). `vectors.json` is not edited.
- The 15 moved shape rows keep their `note`, `url` and `shape` byte for byte. `shapes.json`'s other rows keep their order.
- `node scripts/generate-starters.mjs --check` is clean after every task. Every generated file starts with, or carries, the generator's "Do not edit" marker: a `"generated"` key in the JSON files and an HTML comment in the Markdown.
- Shared seams: Tasks 1 and 2 edit `adapter-templates/tests/`, and Task 3 edits `host/src/capture_site.rs` (test module only) and its fixture. Before this PR's final merge, tell the other active sessions (AGENTS.md "Review and merge cadence"). Merge through the Mergify `queue` label, never `gh pr merge`.
- macOS CI: Task 6 edits `.github/workflows/build-os.yml` and `scripts/ci-changes.mjs`, which are both `MACOS_PATHS`, so this PR runs the macOS job. No task touches Unix-only host code.

## Review Focus

1. **A sample or vector URL on an origin the family does not claim**, such as a typo, a `-my.sharepoint.com` URL in the wrong family, or an excluded `www.` host. The generator refuses it and names the file and field. Tests in Tasks 1, 2 and 4.
2. **Moving the rows must not shrink coverage.** The combined accept rows stay at 40 or more (exactly 40 today), and today's 15 starter rows stay pinned literally. `drafted_rules_pass_the_draft_checks_again_and_never_match_the_root` still visits all 15 starter shapes, and the structural one-adapter-per-kind test still sees six types. Test in Task 1.
3. **Hosts at the edge of a strict suffix:** `www.`/`signin.` (excluded), `a.b.` (two labels), the bare suffix, a port, a trailing dot, and uppercase. Each claims what the permissive matcher claims for the same edge today, or nothing, and the generator's `familyClaims` agrees with `starterFamily` on every one. Test in Task 4.
4. **Two families claiming one origin** (nested suffixes, an exact origin under a suffix, a repeated exact origin), including a test fixture that collides with a real family. The generator names both files. Test in Task 5.
5. **Scaffolder misuse:** a type that already exists, origin flags on an existing family, a new family with no origin flag, an underscore in the type, `--scope unscoped` with `--subdomain`. Each refuses with exit 2, names the problem, and writes nothing. Test in Task 6.

---

### Task 1: Samples in the family files, generated `starter-shapes.json` and its loaders

**Shared seam: `adapter-templates/tests/`.**

**Files:**
- Modify: `protocol/starters/{atlassian,github,miro,sharepoint}.json` (per type: `samples`, `samples_shape`), `protocol/starters/README.md`
- Modify: `scripts/generate-starters.mjs` (`TYPE_KEYS`, validation, `renderTs` key filter, new output, `main`)
- Create (generated): `protocol/capture-site/starter-shapes.json`
- Modify: `protocol/capture-site/shapes.json` (delete the 15 rows whose note contains `(docs/adapters/`)
- Modify: `adapter-templates/tests/shapes.rs` (`rows`), `adapter-templates/tests/common/mod.rs:10`, `adapter-templates/tests/structural.rs:76-129` (`fixture_rows`, the count), `extension/tests/site-patterns.test.mjs:183`
- Modify: `extension/tests/starter-fixtures.mjs` (add `samples: []`, `samples_shape: false` to every fixture type)
- Test: `scripts/tests/generate-starters.test.mjs` (append; `tmpRoot` also copies `shapes.json` and `starter-shapes.json`)

**Interfaces:**
- Produces, in `scripts/generate-starters.mjs`:
  - `export function familyClaims(familyOrigin: object, origin: string): boolean`. Same semantics as `starterFamily` in `extension/starter-adapters.ts`: an exact origin is equal; a subdomain needs an `https:` URL whose `origin` equals the input, a lower-cased hostname ending in `suffix`, and not the bare suffix. Task 4 extends it.
  - `export function renderStarterShapes(families, starterTypes): string`. The JSON text (2-space indent, trailing newline) of `{ "format": 1, "generated": "<HEADER sentence>", "description": "Starter recorded samples (protocol/starters/*.json); same row format as shapes.json.", "types": [<every type with samples_shape true, catalog order>], "accept": [{ "note": "<sample.note> (<type.docs>)", "url", "shape" }...], "reject": [] }`. Rows follow catalog type order, then sample order.
  - Constant `STARTER_SHAPES = "protocol/capture-site/starter-shapes.json"`.
- Family file type fields: `samples: { url: string; title: string | null; shape: string; note: string }[]`, `samples_shape: boolean`.

- [ ] **Step 1: Write the failing generator tests** (append to `scripts/tests/generate-starters.test.mjs`). Each one edits a `tmpRoot` copy, runs `--check`, and expects exit 1 with these exact lines:
  - `protocol/starters/miro.json: types[0].samples is required`
  - `protocol/starters/miro.json: types[0].samples[0].url is not on this family's origin` (url set to `https://example.com/app/board/x=/`)
  - `protocol/starters/github.json: types[2].samples must be empty when samples_shape is false` (one sample added to `github.repo`)
  - `protocol/starters/miro.json: types[0].samples must not be empty when samples_shape is true`
  - `out of date: protocol/capture-site/starter-shapes.json` (a row's `shape` edited in the generated file)
  - `today's 15 starter shape rows are pinned` (permanent; design challenge e3).
    - `TODAY_STARTER_ROWS` is a literal array of the 15 `{note, url, shape}` objects, copied byte for byte from `shapes.json` at `3d39a6d`.
    - Every row appears exactly once, deep-equal, in `starter-shapes.json` `accept`. None of them appears in `shapes.json`.
    - `shapes.json.accept.length + starterShapes.accept.length >= 40` (exactly 40 today).
    - Same pattern as the existing "today's eight lock entries" test.
  - `familyClaims agrees with starterKind on the 40 origins`: import `ORIGIN_CASES`. Export it from `extension/tests/starter-families.test.mjs` into a new `extension/tests/starter-origin-cases.mjs`, and have the families test import it from there. For every case, `familyClaims(family.origin, origin)` over the loaded families gives the expected family id, or none.
- [ ] **Step 2: Run** `node --test scripts/tests/generate-starters.test.mjs`. Expected: FAIL.
- [ ] **Step 3: Move the rows into the family files.**
  - Each of the 15 rows becomes a sample on its type, with the `note` minus its ` (docs/adapters/<type>.md)` suffix, and `url` and `shape` verbatim.
  - Titles follow Open decision 4. `jira.issue`, `confluence.page`, `github.pr`, `github.issue` and `miro.board` each need at least one titled sample (Task 5's check 4 needs it). `sharepoint.doc` and `github.repo` have no title template, so they get `title: null`.
  - `github.repo` gets `samples: []` and `samples_shape: false`; every other type gets `true`.
  - Delete the 15 rows from `shapes.json`.
- [ ] **Step 4: Extend the generator.**
  - `TYPE_KEYS` gains `samples` and `samples_shape`, with the validation that gives Step 1's messages.
  - `renderTs` writes each family without the test-only keys (Open decision 10).
  - `main` writes `starter-shapes.json`, and `--check` compares it after CRLF normalisation, like the TS.
  - Run `npm run gen:starters`.
- [ ] **Step 5: Check the generated TS is unchanged:** `git diff --exit-code extension/starter-families.generated.ts`. The move itself is proved by the permanent `today's 15 starter shape rows are pinned` test.
- [ ] **Step 6: Point the loaders at both files.**
  - `shapes.rs`: add `const STARTER_SHAPES: &str = include_str!("../../protocol/capture-site/starter-shapes.json");`. `rows(kind)` returns the rows of `shapes.json` followed by those of `starter-shapes.json`.
  - `common/mod.rs`: add `pub const STARTER_SHAPES` next to `SHAPES`.
  - `structural.rs` `fixture_rows()`: chain both files.
  - `structural.rs:122-129`: replace `6` with the length of `starter-shapes.json`'s `types` array, compared as sets: `starters == types`, with message `"every starter with samples has a recorded URL"`.
  - `site-patterns.test.mjs`: `shapeFixture` becomes `{ accept: [...a.accept, ...b.accept], reject: [...a.reject, ...b.reject] }`.
  - Add one Rust assertion to `shapes.rs` (`starter_shape_rows_all_name_a_doc`): every `starter-shapes.json` accept note contains `(docs/adapters/`, and no `shapes.json` note does.
- [ ] **Step 7: Run** `node --test scripts/tests/generate-starters.test.mjs`, then `node scripts/generate-starters.mjs --check` (exit 0), `cargo test --locked -p adapter-templates`, `cargo fmt --all -- --check`, `cargo clippy --locked -p adapter-templates --all-targets -- -D warnings`, and `npm run build:extension && node --test extension/tests/site-patterns.test.mjs extension/tests/starter-families.test.mjs`. Expected: all PASS. `structural.rs` prints no change in the visited corpus (Review Focus 2).
- [ ] **Step 8: Commit:** `feat(starters): recorded samples live in the family files; starter-shapes.json is generated`.

### Task 2: Vectors and overlaps in the family files, generated `starter-vectors.json` and its loaders

**Shared seam: `adapter-templates/tests/`.**

**Files:**
- Modify: `protocol/starters/*.json` (family: `sample_origin`, `sample_scope`, `overlaps`; type: `vectors: []`), `protocol/starters/README.md`, `protocol/adapter-templates/README.md` (one paragraph on the second vectors file)
- Modify: `scripts/generate-starters.mjs`
- Create (generated): `protocol/adapter-templates/starter-vectors.json`
- Modify: `adapter-templates/tests/vectors.rs:17,424-440`, `extension/tests/adapter-templates.test.mjs:15-25,86-89`, `extension/tests/site-patterns.test.mjs:49-60`
- Modify: `extension/tests/starter-fixtures.mjs` (add `sample_origin`, `sample_scope: []`, `overlaps: []`, and per type `vectors: []`)
- Test: `scripts/tests/generate-starters.test.mjs`, `extension/tests/starter-families.test.mjs` (append)

**Interfaces:**
- Consumes: `familyClaims` (Task 1).
- Family fields: `sample_origin: string` (the family must claim it); `sample_scope: string[]`, non-empty exactly when the scope kind is `github_org` or `tenant_name` (GitHub: `["acme-corp"]`); `overlaps: { a: string; b: string; expect: "disjoint" }[]`. Type field: `vectors: object[]`, each a `vectors.json` case without `doc`.
- Produces, in `scripts/generate-starters.mjs`:
  - `export function synthesizeFixture(family): { format: 1; artifacts: object[] }`. Each type's `adapter` in source order, with `origin: sample_origin` and every `{"$scope": true}` replaced by `sample_scope`. These are the same documents `starterOffers(sample_origin, sample_scope)` builds.
  - `export function renderStarterVectors(families, starterTypes): string`. The JSON text of `{ "vectors_format": 1, "generated": "<HEADER sentence>", "fixtures": { "starter:<family>": synthesizeFixture(f) ... }, "cases": [...] }`. Per family (catalog order) the cases are: `validate.starter.<family>` (`impls ["rust","ts"]`, expect `{ok: true}`); `overlap_report.starter.<family>` (`impls ["rust"]`, expect `{pairs: []}`); then each type's vectors with `doc: {"$fixture": "starter:<family>"}` added.
  - Constant `STARTER_VECTORS = "protocol/adapter-templates/starter-vectors.json"`.

- [ ] **Step 1: Write the failing tests.** Generator tests (`--check` on a `tmpRoot`, exit 1, exact lines):
  - `protocol/starters/github.json: sample_scope must not be empty for a scoped family`
  - `protocol/starters/miro.json: sample_scope must be empty for an unscoped family`
  - `protocol/starters/miro.json: sample_origin is not on this family's origin` (`https://example.com`)
  - `protocol/starters/atlassian.json: overlaps[0].expect must be "disjoint"`
  - `protocol/starters/atlassian.json: overlaps[0].b "jira.page" is not a type in this family`
  - `protocol/starters/atlassian.json: jira.issue and confluence.page have no overlaps entry` (spec §4 check 5; `overlaps: []`). The same edit on github lists only `github.pr and github.issue`: a pair is exempt when one type is the other's `within` container.
  - `protocol/starters/miro.json: types[0].vectors[0] must not carry "doc" (the family fixture is implied)`
  - `protocol/starters/miro.json: types[0].vectors[0].url is not on this family's origin`
  - `protocol/starters/miro.json: duplicate vector id "eval.miro.title" (also in protocol/adapter-templates/vectors.json)`

  Append to `extension/tests/starter-families.test.mjs`: `starter-vectors fixtures are the runtime offers`. For every family file, `starter-vectors.json` `fixtures["starter:<family>"].artifacts` deep-equals `starterOffers(sample_origin, sample_scope).map((o) => o.adapter)`.
- [ ] **Step 2: Run both.** Expected: FAIL.
- [ ] **Step 3: Fill in today's families.**
  - atlassian: `sample_origin` `https://acme.atlassian.net`, `overlaps: [{a:"jira.issue", b:"confluence.page", expect:"disjoint"}]`.
  - github: `https://github.com`, `["acme-corp"]`, `overlaps: [{a:"github.pr", b:"github.issue", expect:"disjoint"}]`. `github.repo` is the others' `within` container, so it is exempt.
  - sharepoint: `https://acme.sharepoint.com`; miro: `https://miro.com`. Both have `[]` and `[]`.
  - Every type gets `vectors: []`.
- [ ] **Step 4: Extend the generator.** Add the keys and validations, including the check 5 pair rule, plus a duplicate-id check against `vectors.json` case ids and across families. Write and `--check` `starter-vectors.json`. Run `npm run gen:starters`.
- [ ] **Step 5: Load both files.**
  - `vectors.rs`: add `const STARTER_VECTORS: &str = include_str!(...)`. Merge the two `fixtures` maps, panicking with `fixture <name> defined twice` on a repeat. Chain the `cases`, and assert that ids are unique across both files (`duplicate vector id <id>`).
  - `adapter-templates.test.mjs`: `vectors` becomes the same merge; the existing unique-ids test then covers both files.
  - `site-patterns.test.mjs:51`: also read `starter-vectors.json`, and filter cases whose `doc.$fixture` is `"starters"` or starts with `"starter:"`.
- [ ] **Step 6: Run** `node --test scripts/tests/generate-starters.test.mjs`, `--check`, `cargo test --locked -p adapter-templates` (the `vectors:` line shows 8 more passed: 4 validate + 4 overlap_report), `npm run build:extension && node --test extension/tests/adapter-templates.test.mjs extension/tests/site-patterns.test.mjs extension/tests/starter-families.test.mjs extension/tests/starter-adapters.test.mjs`, and `npm run test:adapter-diff`. Expected: all PASS, and `git diff --exit-code protocol/adapter-templates/vectors.json extension/starter-families.generated.ts`.
- [ ] **Step 7: Commit:** `feat(starters): family vectors and overlaps; starter-vectors.json with starter:<family> fixtures`.

### Task 3: Host `taken_types` computed; the three shared-origin lists pinned together

**Shared seam: host (`host/src/capture_site.rs`, test module only).** Independent of Tasks 1–2; it can be built in parallel.

**Files:**
- Modify: `host/src/capture_site.rs:1908-1937` (`structural_draft_site_response_is_unchanged_by_the_refactor`)
- Modify: `host/tests/fixtures/capture-site-structural-response.json:421-430`
- Modify: `extension/tests/adapter-templates.test.mjs` (append one test)
- Modify: `adapter-templates.md` §6 ¶3 (one sentence: the engine lists, `document.rs` and `adapter-templates.ts` `SHARED_ORIGINS`, and the catalog are pinned equal by tests). `catalog.json` itself is not edited.

**Interfaces:** none new.

- [ ] **Step 1: Write the failing TS test** `the engine's SHARED_ORIGINS equal the host catalog's shared_origins`. Extract the quoted strings inside `const SHARED_ORIGINS: ReadonlySet<string> = new Set([` … `]);` from the already-loaded `engineSource`, and deep-equal them, sorted, with `Object.keys(catalog.shared_origins).sort()`. Fail with a message naming the missing and the extra origins. Rust already pins `document.rs` against the catalog (`capture_site.rs:1151`), so all three are tied together. Prove the test bites: temporarily drop `"https://linear.app"` from the TS set, see it FAIL, then restore.
- [ ] **Step 2: Change the host test.**
  - Before comparing, assert `json["taken_types"]` equals `json!(expected)`. `expected` is `catalog().starter_types` plus `"tools.item"`, collected into a `BTreeSet<String>` (sorted, as the response orders it).
  - Also assert a literal floor (design challenge e2). Every one of `["confluence.page", "github.issue", "github.pr", "github.repo", "jira.issue", "miro.board", "sharepoint.doc", "tools.item"]` is in the computed `taken_types`, with message `"a legacy starter type left taken_types: {t}"`. A broken catalog read can then never pass with an empty list.
  - Then set `json["taken_types"] = json!("<taken_types>")`.
  - In the fixture, replace the `taken_types` array with `"<taken_types>"`, the same placeholder pattern the test uses for `revision`.
- [ ] **Step 3: Run** `cargo test --locked -p brauser --lib -- capture_site`, `cargo fmt --all -- --check`, `cargo clippy --locked -p brauser --all-targets -- -D warnings`, and `npm run build:extension && node --test extension/tests/adapter-templates.test.mjs`. Expected: PASS. Then add a throwaway type to a family file in a scratch copy of the worktree, regenerate, and confirm the host test still passes. That proves the fixture no longer needs an edit per starter. Discard the scratch copy.
- [ ] **Step 4: Commit:** `test(host): compute the structural fixture's taken_types; pin the engine shared-origin lists to the catalog`.

### Task 4: Strict subdomain families (`labels`, `exclude`)

**Files:**
- Modify: `scripts/generate-starters.mjs` (`ORIGIN_KEYS.subdomain` optional keys, validation, `INTERFACES` origin type, `familyClaims`)
- Regenerate: `extension/starter-families.generated.ts` (interface text only)
- Modify: `extension/starter-adapters.ts:83-99` (`starterFamily`)
- Modify: `extension/tests/starter-fixtures.mjs` (add `EXAMPLE_STRICT_FAMILY`)
- Modify: `protocol/starters/README.md` (origin kinds)
- Test: `extension/tests/starter-families.test.mjs`, `extension/tests/starter-origin-cases.mjs`, `scripts/tests/generate-starters.test.mjs`

**Interfaces:**
- Generated origin type: `{ kind: "exact"; origin: string } | { kind: "subdomain"; suffix: string; labels?: 1; exclude?: string[] }`.
- `starterFamily` and `familyClaims` have the same rule. Once the permissive test passes, take `prefix = host.slice(0, -suffix.length)`. If `labels` is set, `prefix` must contain no `.`. If `exclude` is set, `prefix`'s first label must not be in `exclude`.
- Produces: `EXAMPLE_STRICT_FAMILY`, family `strict`, origin `{ kind: "subdomain", suffix: ".strict.test", labels: 1, exclude: ["www", "signin"] }`, `scope: null`, `review_prompt: null`. It has one development type, `strict.item` (label `Strict items`, match `/item/{id:int}`, id `strict.item:{host}/{id}`), and carries every Task 1–2 field (`sample_origin` `https://acme.strict.test`).
- Produces, in `starter-origin-cases.mjs`: `STRICT_ORIGIN_CASES`, a list of `[origin, "strict" | null]`.

- [ ] **Step 1: Write the failing tests.**
  - `STRICT_ORIGIN_CASES` holds these pairs:
    - `https://acme.strict.test` → `"strict"`
    - `https://www.strict.test` → null
    - `https://signin.strict.test` → null
    - `https://a.b.strict.test` → null
    - `https://strict.test` → null
    - `https://acme.strict.test:8443` → `"strict"` (same port rule as Atlassian's sweep row)
    - `https://acme.strict.test.` → null
    - `http://acme.strict.test` → null
    - `https://ACME.strict.test` → null
    - `https://acme-www.strict.test` → `"strict"`
  - `strict subdomain families claim one label and skip excluded hosts`: with `setStarterFamiliesForTest([...STARTER_FAMILIES, EXAMPLE_STRICT_FAMILY])`, every pair in `STRICT_ORIGIN_CASES` holds. Every `ORIGIN_CASES` pair keeps its expected family, so the real families don't change.
  - Generator: `familyClaims` agrees on `STRICT_ORIGIN_CASES`. It also refuses these, with exact lines: `protocol/starters/atlassian.json: origin.labels must be 1`, `protocol/starters/atlassian.json: origin.exclude must be a list of lowercase DNS labels`, and `protocol/starters/miro.json: origin unknown field "labels"` (labels on an exact origin).
  - Add `EXAMPLE_STRICT_FAMILY` to the fixture-validation test (`generate-starters.test.mjs:318`).
- [ ] **Step 2: Run them.** Expected: FAIL.
- [ ] **Step 3: Implement** the validation, the interface text, `familyClaims` and `starterFamily` as specified. Run `npm run gen:starters`.
- [ ] **Step 4: Run** `node --test scripts/tests/generate-starters.test.mjs`, `--check`, and `npm run build:extension && npm run typecheck && npm run lint && node --test extension/tests/starter-families.test.mjs extension/tests/starter-adapters.test.mjs extension/tests/capture-site.test.mjs`. Expected: PASS. Also run `git diff extension/starter-families.generated.ts`, which shows only the interface lines.
- [ ] **Step 5: Commit:** `feat(starters): strict subdomain families with labels and exclude`.

### Task 5: Registry checks and shared-origin policy in the generator; the engine coverage test

**Files:**
- Modify: `scripts/generate-starters.mjs`. Add `checkPolicy` and `checkRegistry`, and export `coverageProblems`. `main` gathers every problem in one pass (Open decision 13).
- Create: `extension/tests/starter-registry.test.mjs`
- Modify: `package.json` (`test:extension` gains `extension/tests/starter-registry.test.mjs`)
- Modify: `extension/tests/starter-fixtures.mjs` (`EXAMPLE_UNSCOPED_FAMILY` origin → `https://linear.app`, and its comment)
- Modify: `protocol/starters/README.md` (the policy and registry rules; also fix the file name PR B's final review flagged as Minor 3)
- Test: `scripts/tests/generate-starters.test.mjs` (`tmpRoot` now also copies `docs/adapters/`)

**Interfaces:**
- Consumes: `familyClaims`, `synthesizeFixture`, `STARTER_SHAPES`, `STARTER_VECTORS` (Tasks 1, 2, 4).
- Produces: `export function checkPolicy(entries: { file: string; family: object }[], catalog: object): string[]`. Each problem reads `<file>: <message>`:
  - **Check 6:** `origin overlaps <other file>` (Open decision 5). It is reported on the file that sorts later.
  - **Check 7:** `origin <o> is a shared origin, so scope must not be null`.
  - **Gap 1:** `scope "unscoped" needs an exact origin in protocol/capture-site/catalog.json shared_origins`.
  - **Gap 8:** `scope "github_org" is only for https://github.com`.
  - **Gap 6:** `types[N].adapter.scope must use "<capture>", the host's tenant capture for <o>`.
  - **Gap 6:** `scope "tenant_name" cannot be used on <o>, whose URLs name no tenant` (a shared origin whose `tenant` is null).
- Produces: `export function checkRegistry(root: string, entries): string[]`, the checks that read files outside `protocol/starters/`:
  - **Check 2, reverse direction:**
    - `docs/adapters/<x>.md: no family type` covers every `docs/adapters/*.md` except `README.md`.
    - `protocol/capture-site/shapes.json: row "<note>" names a starter doc; it belongs in a family file's samples`.
    - `protocol/capture-site/starter-shapes.json: row "<note>" names no family type`. That is a hand edit `--check` would also flag, but this message names the row.
  - **Check 3:**
    - `<file>: types[N].docs docs/adapters/<type>.md is missing`
    - `docs/adapters/<type>.md: first Status line does not match status (expected "<opening>")` (Open decision 6: state and `checked_on`)
- Produces: `export function coverageProblems({ families, vectors, starterVectors }, engine: { compileDocument, evaluate }): string[]` (spec §4 check 4). It is pure, takes the engine as a parameter, and the generator never calls it. Each problem reads `<type>: <message>`:
  - `match rule <i> has no positive evaluate vector`
  - `no negative evaluate vector`
  - `no sample whose title the title template accepts`

  How it counts:
  - Legacy cases count through `fixtures.starters`, mapping each artifact index to its `type`. Family cases count through `starter:<family>`, where the index is the type's position.
  - A negative case is an evaluate case on the type's adapter origin whose expect is not `{match: true, type: <type>}`.
  - The title check compiles `synthesizeFixture(family)`. It needs `evaluate(doc, sample.url, sample.title)` to match that type with `title_entry !== null`. Types without `adapter.title` are exempt.

- [ ] **Step 1: Write the failing generator tests.** Each edits a `tmpRoot`, runs `--check`, and expects exit 1 with the exact line.
  - Policy:
    - miro set to `{kind:"unscoped", notice:"x"}` with `unscoped: "acknowledged"` gives the gap 1 line.
    - a copy of `sharepoint.json` as `onedrive.json` (family `onedrive`, suffix `.my.sharepoint.com`, types renamed) gives `protocol/starters/sharepoint.json: origin overlaps protocol/starters/onedrive.json`.
    - an exact `https://acme.atlassian.net` family gives the check 6 line.
    - a `github_org` scope on miro gives the gap 8 line.
    - a `tenant_name` family on `https://www.notion.so` that scopes `team` gives the first gap 6 line, naming `"workspace"`.
    - a `tenant_name` family on `https://docs.google.com` gives the second gap 6 line.
    - a `scope: null` family on `https://linear.app` gives the check 7 line.
  - Registry:
    - a stray `docs/adapters/stray.md` gives `docs/adapters/stray.md: no family type`.
    - a starter-noted row put back into `shapes.json` gives the shapes.json line.
    - deleting `docs/adapters/miro.board.md` gives the missing-doc line.
    - changing the date on `jira.issue.md`'s Status line gives the mismatch line with `expected "Status: release-certified 2026-11-02."`.
    - setting `confluence.page`'s doc to `Status: release-certified 2026-11-02.` while its state is development gives `expected "Status: development draft."`.
  - One pass: a single `tmpRoot` with three problems (a validation error in miro, the gap 8 policy error and a stale doc) prints all three lines.
  - `fixtures do not collide with real families` (gap 5): `checkPolicy` on the real families plus every family export of `starter-fixtures.mjs` is `[]`. To see it bite, add a `https://www.figma.com` family to a `tmpRoot` and run `checkPolicy` with the unmoved fixture. The check 6 line should name `<fixture EXAMPLE_UNSCOPED_FAMILY>`. That is why the fixture moves now, before PR D's Figma family.
- [ ] **Step 2: Write the failing coverage tests** in `extension/tests/starter-registry.test.mjs`. They need `npm run build:extension` first.
  - `every starter rule has positive and negative vectors and a title sample`: `coverageProblems` over the repository's files, using `dist/adapter-templates.js`, deep-equals `[]`.
  - `the generator check is clean` (check 8): `node scripts/generate-starters.mjs --check` exits 0.
  - One test per coverage message. Each mutates a `structuredClone` of the inputs and asserts the exact string:
    - drop `eval.jira.board`-style cases for `jira.issue` rule 1 → `jira.issue: match rule 1 has no positive evaluate vector`
    - drop every Miro no-match case → `miro.board: no negative evaluate vector`
    - null every Miro sample title → `miro.board: no sample whose title the title template accepts`
- [ ] **Step 3: Run both files.** Expected: FAIL.
- [ ] **Step 4: Implement** `checkPolicy`, `checkRegistry` and `coverageProblems`. `main` prints all their problems with the validation problems and then writes nothing. Move `EXAMPLE_UNSCOPED_FAMILY` to `https://linear.app`. If the coverage check finds a legacy rule with no positive or negative vector, stop and report it to the coordinator, and do not edit `vectors.json` (Global Constraints). The survey at `3d39a6d` found every legacy rule covered.
- [ ] **Step 5: Run:**
  - `node --test scripts/tests/generate-starters.test.mjs`
  - `node scripts/generate-starters.mjs --check`
  - `npm run build:extension && node --test extension/tests/starter-registry.test.mjs extension/tests/capture-site.test.mjs extension/tests/panel/capture-site.test.mjs`
  - `npm run lint`

  Expected: all PASS.
- [ ] **Step 6: Commit:** `feat(starters): registry and shared-origin checks in the generator; engine coverage test`.

### Task 6: `npm run new:starter`, the generated docs index, lock entries, gates and CI; acceptance

**Files:**
- Create: `scripts/new-starter.mjs`, `scripts/tests/new-starter.test.mjs`
- Modify: `package.json` (`"new:starter": "node scripts/new-starter.mjs"`; `test:scripts` gains the new test)
- Modify: `scripts/generate-starters.mjs`:
  - `renderAdaptersIndex`
  - `appendLock`
  - a placeholder check: any string that is exactly `"TODO"` gives `<path> is still a placeholder`
- Create (generated): `docs/adapters/README.md`
- Modify: `scripts/gate-adapter.mjs`:
  - ALLOWLIST gains `{ prefix: "docs/adapters/" }`
  - `STARTER_INPUTS` and `EXTENSION_INPUTS` gain `"docs/adapters/"` and `"protocol/starters/"`
- Modify: `scripts/ci-changes.mjs` (`docs/adapters/` is not docs-only), `.github/workflows/build-os.yml` (a `node scripts/generate-starters.mjs --check` step after `setup-node`)
- Modify: `scripts/tests/gate-adapter.test.mjs`, `scripts/tests/ci-changes.test.mjs`, `scripts/tests/ci-macos.test.mjs` (append)
- Modify: `AGENTS.md` (a new "Adding a starter" section after "Local checks"; the repository-map line at `:114`), `README.md:73` (its last sentence points to `docs/adapters/README.md`), `protocol/starters/README.md` ("Adding a type" rewritten around the scaffolder)

**Interfaces:**
- Produces: `renderAdaptersIndex(families, starterTypes): string`. A `<!-- Generated by scripts/generate-starters.mjs … Do not edit. -->` line, `# Starter adapters`, then a table in catalog order: `| Type | Family | Status | Method | Checked on | Checked by |`. A row reads, for example, `| [jira.issue](jira.issue.md) | atlassian | certified | live | 2026-11-02 | John |`, and development rows show `| development | — | — | — |`.
- Produces: `appendLock(families, lock): { lock: Record<string, string>; appended: [key: string, id: string][] }`. It appends missing keys in catalog order. In write mode only, `main` prints `appended lock entry <key> → <id>` for each one (Open decision 1).
- Produces: the `new-starter.mjs` CLI, `<family> <type> [--exact <origin> | --subdomain <suffix>] [--scope github_org|tenant_name|unscoped] [--root <dir>]`. It exits 0 when it creates files and 2 when it refuses.
  - The family must match `^[a-z][a-z0-9-]*$`, the type `^[a-z][a-z0-9]*\.[a-z][a-z0-9]*$`.
  - A new family needs exactly one origin flag. An existing family takes none, and its type must be new.
  - `--subdomain` writes `labels: 1, exclude: ["www"]`. `--scope unscoped` needs `--exact`.
  - Placeholders are `"TODO"` for every free string (`label`, `description`, `record_label`, `adapter.id`, a sample's `url`/`title`/`shape`/`note`), plus `match: []` and `status.state: "development"`.
  - It writes `docs/adapters/<type>.md` with `# <type> starter draft`, `Status: development draft.`, and the six headings from spec §4.
  - Then it runs the generator. That fails on the placeholders and writes nothing. Its one-pass output (validation, policy and registry problems; Open decision 13) is printed under `Fill these in, then run npm run gen:starters:`. After that the CLI prints the `scripts/dev/create-bead` command.
  - The `record_label` placeholder line in `protocol/starters/README.md` explains that grouping into "Remember: A and B" needs it (gap 9).

- [ ] **Step 1: Write the failing tests.**
  - `scripts/tests/new-starter.test.mjs`, on a `tmpRoot` that copies `protocol/`, `extension/starter-families.generated.ts` and `docs/adapters/`, then runs `git init` plus a commit (the scaffolder and generator run from this repo with `--root`):
    - `scaffolds a family file and doc, nothing else`. `aha aha.feature --subdomain .aha.io` exits 0 and prints `types[0].adapter.match must be a non-empty array`. `git status --porcelain` lists exactly `protocol/starters/aha.json` and `docs/adapters/aha.feature.md`.
    - `acceptance: a filled-in family changes only its file, its doc and generated files` (spec task 10). It scaffolds, then replaces the placeholders in `aha.json`:
      - a valid adapter (`/features/{key:jira_key}`, id `aha.feature:{host}/{key}`, title `{title} | Aha!`)
      - one titled sample, one positive and one negative vector
      - `sample_origin` `https://acme.aha.io`, and a doc `Status:` line already valid

      The family is `scope: null`. A subdomain family can never be `unscoped` (gap 1), whatever the spec's "Aha! (subdomain, unscoped)" example says. `node scripts/generate-starters.mjs --root <root>` exits 0. `git status --porcelain` is exactly `aha.json`, `aha.feature.md`, `ids.lock.json`, `extension/starter-families.generated.ts`, `protocol/capture-site/catalog.json`, `protocol/capture-site/starter-shapes.json`, `protocol/adapter-templates/starter-vectors.json` and `docs/adapters/README.md`. The generator also prints `appended lock entry aha.feature → aha.feature:{host}/{key}`. `coverageProblems` over the root's files is `[]`; it uses this repository's built `extension/dist/adapter-templates.js`, and the test skips with a message when that is absent.
    - The five Review Focus 5 refusals each exit 2, each with its own message, and `git status --porcelain` stays empty.
  - `generate-starters.test.mjs`:
    - `renderAdaptersIndex` gives the jira row above and a development row for `confluence.page`.
    - write mode appends a missing `aha.feature` lock entry and prints `appended lock entry aha.feature → <id>`; `--check` still prints `no lock entry for aha.feature`.
    - an edited lock entry still fails in write mode.
  - `gate-adapter.test.mjs`: `docs/adapters/jira.issue.md` is in scope and selects `starter sources` and `test:extension`.
  - `ci-changes.test.mjs`: `classify(["docs/adapters/jira.issue.md"]) === false`, and `classify(["docs/history.md"]) === true`.
  - `ci-macos.test.mjs`: `protocol/starters/aha.json`, `protocol/capture-site/starter-shapes.json`, `protocol/adapter-templates/starter-vectors.json`, `docs/adapters/README.md` and `scripts/new-starter.mjs` do not need macOS.
- [ ] **Step 2: Run** `npm run test:scripts`. Expected: FAIL.
- [ ] **Step 3: Implement** the scaffolder, `renderAdaptersIndex`, `appendLock`, the placeholder check, and the gate, classifier and CI edits. Run `npm run gen:starters` to write `docs/adapters/README.md`, and check that `git diff --exit-code protocol/starters/ids.lock.json` passes, since no new type exists.
- [ ] **Step 4: Write `AGENTS.md` "Adding a starter"** (at most 12 lines):
  - research per the spec's "Research method";
  - `npm run new:starter -- <family> <type> …`, then fill in every placeholder the generator lists;
  - `npm run gen:starters`, then `npm run gate:adapter`;
  - one PR, batched per "Review and merge cadence";
  - certification later, by `status.method` `"live"` or `"history"`;
  - a new scope kind still needs code (spec §3 "What still needs code").

  Update `protocol/starters/README.md` "Adding a type" to match, and `README.md:73`.
- [ ] **Step 5: Run the gate once**, `npm run gate:adapter`. If it refuses because `scripts/` and `.github/` paths are out of scope, run the README gate instead (AGENTS.md "Local checks"). Then run `python3 scripts/check_current_state.py`. Expected: PASS.
- [ ] **Step 6: Commit:** `feat(starters): new:starter scaffolder, generated docs index, lock entries and CI wiring`.
