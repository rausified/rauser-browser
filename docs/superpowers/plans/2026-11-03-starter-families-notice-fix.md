# Starter families notice fix: multi-type unscoped notice, and draft tests that don't depend on starter families

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Before Google Workspace (a multi-type unscoped family) arrives, the every-account notice must be seen before choosing and on every settings card. The drafted-unscoped tests on `docs.google.com` must also keep working once a real family claims that origin.

**Architecture:** Two small extension changes and a test helper.
- `extension/starter-choice-view.ts` renders an unscoped family's notice directly after the prompt, before the checkboxes.
- `extension/options.ts` draws the notice on every card of an unscoped family. Its `scopeShown` guard still limits the scope input to one card per origin.
- A test helper, `draftModeAt`, uses PR B's `setStarterFamiliesForTest` seam to remove whichever family claims an origin, so those tests stay in draft mode.

**Tech Stack:** TypeScript (`tsc` via `extension/build.mjs`), Node 24.14.0 `node:test`, the panel and options fake-DOM harnesses, Playwright + axe (`npm run test:a11y`).

**Spec:** `docs/superpowers/specs/2026-11-03-starter-families-design.md`, builder's calls 3 and 5. This plan closes gaps 3 and 5 from PR B's final review (`/tmp/pr-b-final-review.md` "PR D readiness gaps"), which were split out of the PR C plan (`docs/superpowers/plans/2026-11-03-starter-families-pr-c.md`, design challenge d). It is built on `feat/starter-families-b` (PR #296) and ships as its own PR before PR C.

## Design challenge

Not required: two tasks, no shared seam, no security boundary or migration (AGENTS.md "Before building").

The simpler alternative was to keep the notice after the checkboxes and only repeat it on the gallery cards. It was rejected. With Google's three to five checkboxes, a notice after them is read after the choice is made, which is what the PR B review objected to.

## Open decisions

1. **Notice position:** directly after the review prompt, before the checkboxes and any scope field. **Status:** builder's call, from PR B's final-review triage (resolved by the coordinator, 2026-11-03). **Default:** as stated.
2. **Draft tests keep `docs.google.com`.** The drafted-unscoped flow needs a shared origin with no tenant capture (catalog `tenant: null`). There are two, `docs.google.com` and `www.figma.com`, and PR D claims both. So the tests can't move to another origin; they stay and remove the claiming family through the seam. **Status:** builder's call, open to John. **Default:** as stated.
3. **The two-type fixture sits on `https://www.notion.so`.** The engine accepts `unscoped` only on a shared origin, and no planned family claims Notion. **Status:** builder's call.

## Global Constraints

- Node 24.14.0 (`export PATH=~/.nvm/versions/node/v24.14.0/bin:$PATH`); no new dependencies.
- No changes to `host/`, `adapter-templates/`, `protocol/`, `extension/native.ts` or `extension/manifest.json`. The macOS CI job does not run for this PR.
- Today's four families render exactly as before. None is unscoped, so the notice code path is reached only by fixtures. Existing `options.test.mjs` and `panel/capture-site.test.mjs` tests pass unchanged; new tests are appended.
- The draft tests change only by adding the `draftModeAt` call; none of their assertions change.
- Each new fixture passes the generator's `validateFamily` (the existing fixture-validation test in `scripts/tests/generate-starters.test.mjs:318` lists it).
- Merge through the Mergify `queue` label (`gh pr edit <n> --add-label queue`), never `gh pr merge`.

## Review Focus

1. **A one-type unscoped family** (today's `EXAMPLE_UNSCOPED_FAMILY`, no checkboxes): the notice still shows once, after the prompt, and the existing test's assertions hold. Test in Task 1.
2. **A gallery with an unscoped and a scoped origin at once:** the notice appears on each unscoped card, and the scope input still appears once, on the scoped origin's first card. Test in Task 1.
3. **Re-opening Capture Site on another origin after the pair family:** the old notice is gone. `clear()` replaces the container, so no stale `.hint` remains. Test in Task 1.
4. **`draftModeAt` on an origin no family claims:** the family list is unchanged, and the test seam still resets after the test. Test in Task 2.
5. **A test that forgets `draftModeAt` once PR D lands:** this cannot be tested here. Task 2's synthetic-claim test shows the helper works, and PR D's own run of the draft tests is the backstop.

---

### Task 1: The notice comes first, and on every card

**Files:**
- Modify: `extension/starter-choice-view.ts:59-111`. Build the `unscopedNotice` `.hint` straight after `prompt` and append it before the checkbox field. The scope-field branch is unchanged.
- Modify: `extension/options.ts:533-550`. The notice branch no longer checks or sets `scopeShown`, so it runs for every card; `scopeShown` guards only `renderScopeField`.
- Modify: `extension/tests/starter-fixtures.mjs` (add `EXAMPLE_UNSCOPED_PAIR_FAMILY`)
- Modify: `scripts/tests/generate-starters.test.mjs:318` (add the new fixture to the validated list)
- Test: `extension/tests/panel/capture-site.test.mjs`, `extension/tests/options.test.mjs` (append)

**Interfaces:**
- Produces: `EXAMPLE_UNSCOPED_PAIR_FAMILY`:
  - family `pair`, `display_name` `Pair`, origin `{kind:"exact", origin:"https://www.notion.so"}`
  - `review_prompt: "Choose what to remember on Pair."`
  - `scope: {kind:"unscoped", notice:"Records every Pair page and board from every account in this Chrome profile."}`
  - two `development` types, in the same shape as `EXAMPLE_UNSCOPED_FAMILY`'s type:
    - `pair.page`: label `Pair pages`, match `/p/{id:int}`, id `pair.page:{host}/{id}`
    - `pair.board`: label `Pair boards`, match `/b/{id:int}`, id `pair.board:{host}/{id}`
  - both types: `unscoped: "acknowledged"`, `record_label: null`, `example_path: null`

- [ ] **Step 1: Write the failing tests.**
  - `panel/capture-site.test.mjs`: `an unscoped family with several types shows its notice before the checkboxes`.
    - Set `setStarterFamiliesForTest([EXAMPLE_UNSCOPED_PAIR_FAMILY])` and reset it in `t.after`.
    - Open Capture Site on `https://www.notion.so/p/1`. Inside the starter group, the `.hint` whose text is the notice comes before the first `input[data-starter-type]`. Check the order through the children of `#capture-site-starter-prompt`'s parent.
    - Exactly two checkboxes are present and checked.
    - After closing and opening on `https://github.com/acme/x/pull/1` with the real families, no element has the Pair notice text (Review Focus 3).
  - `options.test.mjs`: `every card of an unscoped family carries its notice`.
    - Use the seam with `[EXAMPLE_UNSCOPED_PAIR_FAMILY, EXAMPLE_TENANT_FAMILY]`, and enter both origins.
    - Both Pair cards contain the notice.
    - The tenant origin has exactly one scope input (Review Focus 2).
  - Fixture validation now includes `EXAMPLE_UNSCOPED_PAIR_FAMILY`.
- [ ] **Step 2: Run** `npm run build:extension && node --test extension/tests/panel/capture-site.test.mjs extension/tests/options.test.mjs`. Expected: the two new tests FAIL, and the existing ones (including the one-type unscoped test, Review Focus 1) PASS.
- [ ] **Step 3: Implement** both placements as described under Files.
- [ ] **Step 4: Run** the Step 2 command, then `node --test scripts/tests/generate-starters.test.mjs`, `npm run typecheck`, `npm run lint` and `npm run test:a11y`. Expected: all PASS.
- [ ] **Step 5: Commit:** `fix(starters): show the every-account notice before the choices and on every gallery card`.

### Task 2: Draft tests pin draft mode with `draftModeAt`

**Files:**
- Modify: `extension/tests/starter-fixtures.mjs` (add `draftModeAt`)
- Modify, adding one `draftModeAt` call each:
  - `extension/tests/panel/capture-site-draft.test.mjs`, the test around `:265` that drafts on `docs.google.com`
  - `extension/tests/panel/capture-site-draft-save.test.mjs`, the test around `:130`
  - `extension/tests/capture-site.test.mjs`, every test that uses `DOCS` (`:240` onward)
- Test: `extension/tests/capture-site.test.mjs` (append)

**Interfaces:**
- Produces: `export async function draftModeAt(t, origin: string): Promise<void>`. It dynamically imports `new URL("../dist/starter-adapters.js", import.meta.url)` and `../dist/starter-families.generated.js`. Those are the same module instances the panel and plain tests import, because the specifier resolves to the same file URL. It then calls `setStarterFamiliesForTest(STARTER_FAMILIES.filter((f) => f !== starterFamily(origin)))` and registers `t.after(() => setStarterFamiliesForTest(null))`.

- [ ] **Step 1: Write the failing tests** in `capture-site.test.mjs`:
  - `draftModeAt removes whichever family claims the origin`.
    - Seam in `[...STARTER_FAMILIES, CLAIMS_DOCS]`, where `CLAIMS_DOCS` is a one-type copy of `EXAMPLE_UNSCOPED_FAMILY` with origin `https://docs.google.com`. This stands in for PR D's Google family.
    - Assert `starterKind(DOCS) === CLAIMS_DOCS.family`.
    - Call `draftModeAt` the way a test would, then assert `starterKind(DOCS) === null` and `starterKind("https://github.com") === "github"`.
  - `draftModeAt leaves the list alone for an unclaimed origin` (Review Focus 4): after `draftModeAt(t, "https://tools.example.com")`, `starterFamilies().length === STARTER_FAMILIES.length`.
- [ ] **Step 2: Run** `npm run build:extension && node --test extension/tests/capture-site.test.mjs`. Expected: FAIL (`draftModeAt` is not exported).
- [ ] **Step 3: Implement** `draftModeAt`. Add `await draftModeAt(t, "https://docs.google.com")` at the top of each listed test, and turn a test into `(t) => …` where it isn't already. No assertion changes.
- [ ] **Step 4: Run** `node --test extension/tests/capture-site.test.mjs extension/tests/panel/capture-site-draft.test.mjs extension/tests/panel/capture-site-draft-save.test.mjs`, then `npm run test:extension` and `npm run lint`. Expected: all PASS. `git diff` on the two panel draft files shows only the added call and signature lines.
- [ ] **Step 5: Commit:** `test(starters): draft-flow tests pin draft mode instead of relying on no family claiming docs.google.com`.
