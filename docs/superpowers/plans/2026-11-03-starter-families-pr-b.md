# Starter families, PR B: Capture Site and the settings gallery driven by the family list

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Capture Site, the settings starter gallery and the site-coverage wording read everything they say about a starter from `protocol/starters/<family>.json`, so a new family with an existing scope kind needs no TypeScript, and every starter says whether it is certified or not yet checked on a real account.

**Architecture:** The family files gain the UI fields (`display_name`, `review_prompt`, per-type `record_label` and `example_path`, and three scope kinds). The generator validates them and emits a typed family-id union. A new pure module, `extension/starter-scopes.ts`, is the scope-kind registry (parser, copy, URL prefill). `planCaptureSite` takes `{ types, scopes }` and works the same way for every family. The panel builds one choice group per family from data in a new view module, `extension/starter-choice-view.ts`, which replaces the hard-coded `panel.html` blocks. The gallery and `site-coverage.ts` read labels, order and status from the same accessors in `starter-adapters.ts`.

**Tech Stack:** TypeScript (extension, `tsc` via `extension/build.mjs`), Node 24.14.0 scripts and `node:test`, the panel/options fake-DOM harnesses, Playwright + axe (`npm run test:a11y`).

**Spec:** `docs/superpowers/specs/2026-11-03-starter-families-design.md` ("Decisions", "Builder's calls" 5, §2, §3 "Driving the extension from the family list" and "What still needs code", §5 PR B steps 4–6). Builds on PR A (#275, plan `docs/superpowers/plans/2026-11-03-starter-families-pr-a.md`) and on `a902b01` (#277), which already made the Capture Site source line follow `starterStatus`. This plan only rewords that line; it does not rebuild it.

## Design challenge

- **Challenger:** the plan author (a self-challenge, not an independent one). The unscoped handling below touched the account-privacy boundary; the coordinator ruled it (Open decision 2) instead of an independent five-minute challenge before Task 1.
- **Alternative:** keep the `panel.html` blocks and the gallery's `offer.kind === "github"` branch. Drive only the labels, the "Choose …" messages and the status words from data, and show or hide the static blocks by family id. That is a smaller diff with no test-id churn.
- **Outcome:** kept the plan. PR D adds eight families, some with a `tenant_name` scope and some `unscoped`. Static blocks would mean new markup, listeners and disabling code for each one, which are the per-family touch points this work exists to remove (spec §1 rows 8–11). Rendering already has a tested precedent: the drafted review builds its checkboxes the same way (`capture-site-panel.ts:554-584`).

## Open decisions

Resolved by John (2026-11-03, in the spec): unchecked starters are offered with a clear "not yet checked on a real account" label in Capture Site and the gallery. Certified starters say certified whatever the method; only the docs index (PR C) shows the method.

The builder's calls below are made in this plan and are open to John's correction. None blocks Task 1.

1. **Status copy (exact).**
   - Per type, a suffix after the checkbox text: ` · certified` or ` · not yet checked on a real account`.
   - Gallery card line: `Certified` or `Not yet checked on a real account`.
   - Capture Site source line, replacing `a902b01`'s strings:
     - all certified: `Certified starter rules · checked on real pages.`
     - none certified: `Development starter draft · not yet checked on a real account.`
     - a mix: `Certified starter rules, except <labels joined by joinRecordedTypes>: not yet checked on a real account.`
   - "Checked on real pages" is true of both `live` and `history` certification. "Live tool" was not.
2. **Unscoped starters show the every-account notice; no extra checkbox.** (Coordinator ruling, 2026-11-03, replacing the planner's acknowledgement checkbox.) Capture Site and the gallery show the family's `notice` with the type's status label, and Save works as for any unscoped starter, exactly as Miro's every-account description works today. Reasons: the spec's builder's call 5 ("offered in Capture Site with the every-account notice and the draft label"), consistency with Miro, which records every account with no checkbox, and John's note that Brauser privacy is not a constraint. The template's `"unscoped": "acknowledged"` marker (required by the engine for `SHARED_ORIGINS`) remains the config-level acknowledgement. Cost if wrong: one checkbox to add later.
3. **`starterOffers(origin, scope: readonly string[])` keeps its PR A signature** (only the parameter is renamed). The spec sketched `Record<family, string[]>`, but one origin belongs to exactly one family. `starter-adapters.test.mjs` stays unchanged.
4. **UI family order is catalog order.** The generator emits `STARTER_FAMILY_IDS` ordered by each family's first type in `catalog.json` `starter_types`: Atlassian, SharePoint, GitHub, Miro, then new families as they are appended. This reproduces today's hint order. `STARTER_FAMILIES` keeps its file-name order, so PR A's generator test stays unchanged.
5. **Checkbox text is `record_label ?? label` in sentence case**, which gives "Repositories", "Pull requests", "Issues", "Jira issues" and "Confluence pages", exactly as today. There is no new `choice_label` field.
6. **Display order within a family puts containers first, then source order**: `github.repo`, `github.pr`, `github.issue`. This is today's checkbox order and `GITHUB_RECORD_ORDER`. Offers, the "Remember: …" line and gallery cards keep source order, as today.
7. **A family with one type and no scope renders no choice group**, as today for SharePoint and Miro. `planCaptureSite` ignores `choice.types` for a one-type family. The source line carries its status.
8. **The group is a `div role="group" aria-labelledby` around today's `p` + `.field` markup**, not a `<fieldset>`. The look is unchanged and no CSS is needed.
9. **Scope-kind copy lives in the TS registry.** `github_org` keeps today's GitHub strings verbatim. `tenant_name` copy is built from the family's `scope.label` and `display_name`. The `unscoped` notice comes from the family file.
10. **No URL prefill for `tenant_name`** in PR B: the field starts empty. The `github_org` prefill (`githubOwnerFromUrl`) is unchanged.
11. **Typed family ids.** The generated `StarterFamilyId` union becomes `StarterKind`, and `StarterFamilyData.family` is typed with it, so a misspelt `starterKind(x) === "githb"` fails `tsc`. After this PR, no family-id literal is compared anywhere in `extension/*.ts`.
12. **Panel test ids change.** `capture-site-atlassian`, `-github`, `-jira`, `-confluence`, `-repos`, `-prs`, `-issues` and `-orgs` disappear. The rendered controls carry `data-starter-type` / `data-starter-scope`, and the tests find them through new harness helpers (Task 3).
13. **`options.html:58` and `:59` copy.** The gallery intro no longer claims every starter is uncertified, and the static hint is removed because `renderStarterGallery` writes it on load (`options.ts:1203`).
14. **A test seam `setStarterFamiliesForTest(families | null)`** in `starter-adapters.ts` swaps the family list. The synthetic families in tests (exact origin with `tenant_name`, exact origin with `unscoped`) go through it. `extension/` has no precedent for this; the name and doc comment mark it as test-only.

## Global Constraints

- Node 24.14.0 for every script and test (`export PATH=~/.nvm/versions/node/v24.14.0/bin:$PATH`); no new npm dependencies.
- No changes to `host/`, `adapter-templates/`, `protocol/schema.json`, `protocol/capture-site/catalog.json` or `ids.lock.json`. No new starter types (that is PR D).
- Stay off the macOS-CI paths in `scripts/ci-macos.mjs:36-51` (`extension/native.ts`, `extension/manifest.json`, the smoke and dev-service scripts). Nothing in this plan touches them.
- Generated files keep PR A's header and determinism. `node scripts/generate-starters.mjs --check` must be clean after every task.
- Today's UI copy for the four families is reproduced byte for byte except the deliberate status copy (Open decision 1) and the `options.html:58` sentence. These strings must survive:
  - `Choose Jira issues, Confluence pages, or both.`
  - `Choose repositories, pull requests, issues, or some combination.`
  - `Enter 1–32 comma-separated GitHub organizations, or uncheck every GitHub page type.`
  - `Enter an Atlassian Cloud (Jira or Confluence), SharePoint/OneDrive, GitHub, or Miro site address above to see starter drafts.`
  - `Work organizations`, `Work organizations (comma-separated)`, placeholders `acme-corp, partner` (panel) and `acme-corp, partner-org` (gallery), `maxlength` 1312
  - `Required: GitHub is shared across accounts, so the starter never captures every organization.`
  - `Records <nouns> owned by: <orgs>.`
- These tests pass **unchanged** (`git diff --exit-code` on each at the end): `starter-adapters.test.mjs`, `starter-families.test.mjs` (only new tests appended), `site-coverage.test.mjs`, `panel/capture-explanation.test.mjs`, `panel/capture-site-agent.test.mjs`, `panel/capture-site-recurring.test.mjs`, `panel/capture-site-draft-save.test.mjs`, `options.test.mjs` (only new tests appended), and every `extension/tests/a11y/*.test.mjs`.

## Review Focus

1. **Re-opening Capture Site on another family's origin** (GitHub, cancel, then Atlassian): no GitHub checkbox or typed organization may survive into the Atlassian review. `close()` must blank a typed scope value before removing it, because the value is account data. Test in Task 3.
2. **Messy tenant lists** (`" Acme, acme ,PARTNER"`; `max + 1` names; `bad/name`): they parse to `["acme", "partner"]`, or are refused with the family's exact `invalid` message. A typed list is never sent half-parsed. Test in Task 2.
3. **An unscoped family**: the notice is visible before Save, the plan is produced without any extra input, and the saved adapters carry `unscoped: "acknowledged"`. Tests in Task 2 (plan) and Task 3 (panel).
4. **Two scoped origins in the gallery at once** (GitHub plus a synthetic `tenant_name` family): each origin gets its own input. Typing in one never re-enables or disables the other origin's buttons. Test in Task 4.
5. **A family file that contradicts its scope kind**: a `$scope` marker in an unscoped family, `{scope}` in an unscoped family's `example_path`, or two types with `review_prompt: null`. The generator must fail with the file and field named, not render an unusable form. Tests in Task 1.

---

### Task 1: UI fields in the family files, typed family ids, data accessors

**Files:**
- Modify: `protocol/starters/{atlassian,github,miro,sharepoint}.json`, `protocol/starters/README.md` (file shape and the three scope kinds)
- Modify: `scripts/generate-starters.mjs:32-48` (interfaces), `:86-96` (key lists), `:112-123` (`validateScope`), `:199-239` (type/family validation), `:292-294` (`renderTs`), `main` (pass the catalog order to `renderTs`)
- Regenerate: `extension/starter-families.generated.ts`
- Modify: `extension/starter-adapters.ts:6-12` (`StarterKind`), `:60-92` (export the family lookup, generalize `starterStatus`), `:377-388` (`configuredGitHubOrganizations` → `configuredScope`)
- Create: `extension/tests/starter-fixtures.mjs` (the synthetic families every later task injects)
- Test: `scripts/tests/generate-starters.test.mjs` (append), `extension/tests/starter-families.test.mjs` (append)

**Interfaces:**
- Produces, in `extension/starter-families.generated.ts`:
  ```ts
  export const STARTER_FAMILY_IDS = ["atlassian", "sharepoint", "github", "miro"] as const; // catalog order
  export type StarterFamilyId = (typeof STARTER_FAMILY_IDS)[number];
  export type StarterScopeData =
    | { kind: "github_org" }
    | { kind: "tenant_name"; max: number; label: string; placeholder: string }
    | { kind: "unscoped"; notice: string };
  // StarterTypeData gains: record_label: string | null; example_path: string | null;
  // StarterFamilyData: family: StarterFamilyId; display_name: string; review_prompt: string | null; scope: StarterScopeData | null;
  ```
- Produces: `renderTs(families, starterTypes?: string[])`. The second argument is the catalog order. It defaults to the families' own type order, so PR A's `renderTs(families)` test still holds.
- Produces, in `extension/starter-adapters.ts`:
  - `type StarterKind = StarterFamilyId`
  - `starterFamily(origin: string): StarterFamilyData | null` (today's private `familyFor`, exported)
  - `starterFamilies(): readonly StarterFamilyData[]` (in `STARTER_FAMILY_IDS` order; while the seam is set, the injected list in its own order)
  - `starterType(type: string): StarterTypeData | null`; `starterStatus(type)` is unchanged and reads it
  - `displayTypes(family: StarterFamilyData): StarterTypeData[]` (types that are another type's `within.type` first, then source order)
  - `choiceNoun(type: StarterTypeData): string` (`record_label ?? label`)
  - `recordLabel(type: string): string | null`
  - `recordOrder(): string[]` (every `displayTypes` entry that has a `record_label`, family by family)
  - `isContainerType(type: string): boolean`
  - `STARTER_STATUS_TEXT: Readonly<Record<"certified" | "development", string>>` = `{ certified: "certified", development: "not yet checked on a real account" }`
  - `configuredScope(artifacts: AdapterObject[], origin: string): string[]` (lower-cased, deduplicated string values of every scope table entry on `origin`)
  - `setStarterFamiliesForTest(families: readonly StarterFamilyData[] | null): void`. Every function above, plus `starterKind`, `starterOffers` and `starterStatus`, reads the swapped list.

- [ ] **Step 1: Write the failing generator tests** in `scripts/tests/generate-starters.test.mjs`, each editing a `tmpRoot` copy and running `--check`, with exit 1 and these exact lines expected:
  - `protocol/starters/atlassian.json: display_name is required`
  - `protocol/starters/atlassian.json: review_prompt must be a string when a family has more than one type or a scope` (atlassian with `review_prompt: null`)
  - `protocol/starters/github.json: scope.kind must be "github_org", "tenant_name" or "unscoped"`
  - `protocol/starters/miro.json: scope.max must be an integer from 1 to 32` (miro set to `{kind:"tenant_name", max: 0, label:"Workspaces", placeholder:"acme"}`)
  - `protocol/starters/miro.json: types[0].adapter has a {"$scope": true} marker but the family is unscoped`, and `types[0].example_path uses {scope} but the family has no scope values` (miro set to `{kind:"unscoped", notice:"x"}` with a marker and `example_path: "/{scope}"`)
  - `protocol/starters/miro.json: types[0].adapter.unscoped must be "acknowledged" for an unscoped family` (unscoped with no marker and no `unscoped`)
  - `protocol/starters/github.json: types[0].adapter needs a {"$scope": true} marker for a scoped family` (the marker removed from `github.pr`)
  - `protocol/starters/atlassian.json: types[0].example_path must start with "/"`
  - Plus `STARTER_FAMILY_IDS follows catalog order`: the rendered TS contains `export const STARTER_FAMILY_IDS = ["atlassian", "sharepoint", "github", "miro"] as const;`.
- [ ] **Step 2: Write the failing accessor tests**, appended to `extension/tests/starter-families.test.mjs`:
  - `displayTypes puts containers first`: the github family gives `["github.repo","github.pr","github.issue"]`; the atlassian family gives `["jira.issue","confluence.page"]`.
  - `record labels and order come from the family files`: `recordLabel("github.pr") === "pull requests"`, `recordLabel("jira.issue") === null`, and `recordOrder()` deep-equals `["github.repo","github.pr","github.issue"]`.
  - `isContainerType`: `github.repo` is true; `github.pr` and `jira.issue` are false.
  - `configuredScope reads every scope value on the origin`: two GitHub adapters scoped `["A"]` and `["b","a"]` give `["a","b"]`, and an Atlassian adapter is ignored.
  - First write `extension/tests/starter-fixtures.mjs`, which exports two fixtures:
    - `EXAMPLE_TENANT_FAMILY`: `{ family: "example", display_name: "Example", review_prompt: "Choose what to remember on Example.", origin: {kind:"exact", origin:"https://example.test"}, scope: {kind:"tenant_name", max: 4, label: "Workspaces", placeholder: "acme"} }` with two types. `example.item` (label `Example items`) matches `/{workspace:name|lower}/item/{id:int}`, has `scope: { workspace: {"$scope": true} }` and id `example.item:{host}/{workspace}/{id}`. `example.note` (label `Example notes`) is the same with `note`. Both are `development`, with `record_label`/`example_path` `null`.
    - `EXAMPLE_UNSCOPED_FAMILY`: family `everyone`, display name `Everyone`, origin `https://everyone.test`, `scope: {kind:"unscoped", notice:"Records every Everyone page from every account in this Chrome profile."}`, and one `development` type `everyone.page` (label `Everyone pages`, `"unscoped": "acknowledged"`, match `/p/{id:int}`).
  - `the test seam swaps and restores the family list`: `setStarterFamiliesForTest([EXAMPLE_TENANT_FAMILY])`. Then `starterKind("https://example.test") === "example"`, `starterKind("https://github.com") === null`, `starterOffers("https://example.test", ["acme"])[0].adapter.scope` deep-equals `{ workspace: ["acme"] }`, and after `setStarterFamiliesForTest(null)` the first is `null` and the second `"github"`.
- [ ] **Step 3: Run both** (`node --test scripts/tests/generate-starters.test.mjs`; `npm run build:extension && node --test extension/tests/starter-families.test.mjs`). Expected: FAIL.
- [ ] **Step 4: Add the fields to the four family files.**
  - atlassian: `display_name: "Atlassian Cloud (Jira or Confluence)"`, `review_prompt: "Choose the kinds of work to remember on this Atlassian site."`.
  - github: `display_name: "GitHub"`, `review_prompt` = the text of `panel.html:96` verbatim; `record_label` `"pull requests"`, `"issues"`, `"repositories"`; `example_path` `"/{scope}/example-repository/pull/123"`, `"/{scope}/example-repository/issues/123"`, `"/{scope}/example-repository"`.
  - sharepoint: `"SharePoint/OneDrive"`, `review_prompt: null`.
  - miro: `"Miro"`, `review_prompt: null`.
  - Every other type gets `record_label: null` and `example_path: null`.
- [ ] **Step 5: Extend the generator.**
  - `FAMILY_KEYS` and `TYPE_KEYS` gain the new keys (all required; `null` allowed where the interface says so).
  - `ADAPTER_KEYS` gains `unscoped`.
  - `validateScope` gains the per-kind keys.
  - A new `validateScopeUse(problems, file, family)` produces the marker, `unscoped` and `{scope}` problems from Step 1.
  - `renderTs` writes `STARTER_FAMILY_IDS` and the new interfaces.
  - Regenerate with `npm run gen:starters`.
- [ ] **Step 6: Implement the `starter-adapters.ts` accessors** listed in Interfaces. Update the `StarterKind` doc comment (`:6-11`) so it no longer quotes the family ids. Replace `configuredGitHubOrganizations` with `configuredScope` and update its one caller (`options.ts:455`) to `configuredScope(artifacts, "https://github.com")`, so the gallery's behaviour does not change until Task 4. The seam is a module-level `let families` that defaults to `STARTER_FAMILIES`. `LINKABLE_TO_REPOSITORY` stays computed at import (synthetic families declare no `within`).
- [ ] **Step 7: Run the tests:** `node --test scripts/tests/generate-starters.test.mjs`, `node scripts/generate-starters.mjs --check` (exit 0), then `npm run build:extension && npm run test:extension && npm run typecheck && npm run lint`. Expected: all PASS, and `git diff --exit-code extension/tests/starter-adapters.test.mjs protocol/capture-site/catalog.json`.
- [ ] **Step 8: Commit:** `feat(starters): UI fields, scope kinds and typed family ids in the family files`.

### Task 2: Scope-kind registry and a generic `planCaptureSite`

**Files:**
- Create: `extension/starter-scopes.ts`
- Modify: `extension/capture-site.ts:22-31` (`CaptureChoice`), `:44` (delete `GITHUB_EXAMPLE_ROUTES`), `:70-95` (offer selection), `:108-118` (examples), `:241-243` (`tenantPrefill`'s GitHub check)
- Modify: `extension/capture-site-panel.ts:911-918` (bridge only: build the new choice from today's static inputs; Task 3 replaces it)
- Test: `extension/tests/capture-site.test.mjs`

**Interfaces:**
- Consumes: the Task 1 accessors, `parseGitHubOrganizations`, `parseTenantList`, `githubOwnerFromUrl`.
- Produces, in `extension/starter-scopes.ts`:
  ```ts
  export interface ScopeInput {
    label: string; placeholder: string; maxLength: number;          // panel field
    galleryLabel: string; galleryPlaceholder: string; guidance: string; // settings gallery
    invalid: string;                                                 // planCaptureSite refusal
    parse(value: string): string[] | null;
    fromUrl: ((url: string) => string | null) | null;               // panel prefill; never applied unseen
    records(nouns: string, values: string): string;                 // panel preview line
  }
  export function scopeInput(family: StarterFamilyData): ScopeInput | null; // null for scope null or unscoped
  export function unscopedNotice(family: StarterFamilyData): string | null;
  ```
  - `github_org`: the strings in Global Constraints; `maxLength: 1312`, `parse: parseGitHubOrganizations`, `fromUrl: githubOwnerFromUrl`, `records: (n, v) => \`Records ${n} owned by: ${v}.\``.
  - `tenant_name`, with `L = scope.label` and `D = display_name`:
    - `label: L`, `placeholder: scope.placeholder`, `maxLength: scope.max * 41`
    - `galleryLabel: \`${L} (comma-separated)\``, `galleryPlaceholder: scope.placeholder`
    - `guidance: \`Required: ${D} is shared across accounts, so the starter records only the ${L.toLowerCase()} you name.\``
    - `invalid: \`Enter 1–${scope.max} comma-separated ${L.toLowerCase()}, or uncheck every ${D} page type.\``
    - `parse: (v) => parseTenantList(v, scope.max)`, `fromUrl: null`, `records: (n, v) => \`Records ${n} only for: ${v}.\``
- Produces, in `extension/capture-site.ts`:
  - `export interface CaptureChoice { types: Set<string>; scopes: Record<string, string> }`. `scopes` is keyed by family id: the typed list for a scoped family; unscoped families have no entry.
  - `export function chooseMessage(nouns: readonly string[]): string`: one noun gives `Choose A.`, two give `Choose A, B, or both.`, three or more give `Choose A, B, C, or some combination.`

- [ ] **Step 1: Convert the existing choice literals.** Add `choiceOf({ organizations = "", repositories, pullRequests, issues, jiraIssues, confluencePages })` at the top of `capture-site.test.mjs`. It returns `{ types: new Set([...]), scopes: { github: organizations } }` with `github.repo` / `github.pr` / `github.issue` / `jira.issue` / `confluence.page` for the true flags. Wrap every literal at `:13`, `:56-116` in it. **No assertion changes.**
- [ ] **Step 2: Write the failing tests** in `capture-site.test.mjs`, using `setStarterFamiliesForTest` and restoring with `t.after`:
  - `chooseMessage builds the at-least-one message from labels`: the two- and three-noun cases equal the two Global Constraints strings, and the one-noun case is `Choose Miro boards.`.
  - `a tenant_name family plans through its parser`: `EXAMPLE_TENANT_FAMILY` at `https://example.test/acme/item/1`.
    - `scopes.example = " Acme, acme ,PARTNER"` gives a plan whose adapters' `scope.workspace` deep-equals `["acme","partner"]`.
    - `"a,b,c,d,e"` (more than `max`), `"bad/name"` and `""` each give `{ ok: false, message: "Enter 1–4 comma-separated workspaces, or uncheck every Example page type." }`.
    - `types: new Set()` gives `Choose Example items, Example notes, or both.`
  - `an unscoped family plans without extra input`: `EXAMPLE_UNSCOPED_FAMILY` at `https://everyone.test/p/1` with no `scopes.everyone` gives an ok plan whose adapters carry `unscoped: "acknowledged"`.
  - `example URLs come from example_path`: the GitHub plan's `examples` URLs equal today's: `https://github.com/acme-corp/example-repository/pull/123`, `…/issues/123` and `https://github.com/acme-corp/example-repository`.
- [ ] **Step 3: Run** `npm run build:extension && node --test extension/tests/capture-site.test.mjs`. Expected: FAIL (`chooseMessage` / `setStarterFamiliesForTest` behaviour missing).
- [ ] **Step 4: Implement** `starter-scopes.ts`, then rewrite `planCaptureSite`'s offer selection:
  - `family = starterFamily(origin)!`.
  - `chosen` is all of `family.types` for a one-type family, else `displayTypes(family)` filtered by `choice.types`. If it is empty, refuse with `chooseMessage(displayTypes(family).map(choiceNoun))`.
  - Then check the scope: `scopeInput(family)?.parse(choice.scopes[family.family] ?? "")`, refusing with `invalid`; an unscoped family needs no scope value.
  - `offers = starterOffers(origin, values).filter(chosen)`, which keeps source order.
  - Examples: `origin + example_path.replace("{scope}", () => values[0]!)` when `example_path` is set, else `exampleUrl`.
  - `tenantPrefill:243` becomes `const input = family && scopeInput(family); if (input?.fromUrl && input.fromUrl(url) !== canonical) return "";`.
- [ ] **Step 5: Bridge the panel's `updatePreview`.** Build `types` from the six static checkboxes and `scopes: { github: this.orgs.value }`. This is behaviour-neutral; Task 3 deletes it.
- [ ] **Step 6: Run** `npm run build:extension && npm run test:extension && npm run typecheck && npm run lint`. Expected: PASS, with every `panel/*.test.mjs` unchanged.
- [ ] **Step 7: Commit:** `refactor(capture-site): plan starters generically through scope kinds`.

### Task 3: The panel renders each family's choice group from data

**Files:**
- Create: `extension/starter-choice-view.ts`
- Modify: `extension/panel.html:88-106` → `<div id="capture-site-starter"></div>`
- Modify: `extension/capture-site-panel.ts`:
  - `:15,18` imports
  - `:39-53` source copy
  - `:178-185` fields removed
  - `:257-259` listeners
  - `:379-387` render instead of show/hide
  - `:420-424` restore: `recordLabel`
  - `:473` clear
  - `:761-764` disabling
  - `:911-918` the choice
  - `:948-956` the records line from `scopeInput`
- Modify: `extension/tests/panel/harness.mjs` (helpers), `extension/tests/panel/capture-site.test.mjs`, `capture-site-draft.test.mjs:44`, `capture-site-restore.test.mjs:40,71,175-178`

**Interfaces:**
- Consumes: `starterFamily`, `displayTypes`, `choiceNoun`, `starterStatus`, `STARTER_STATUS_TEXT`, `recordLabel` (Task 1); `scopeInput`, `unscopedNotice`, `CaptureChoice` (Task 2).
- Produces, in `extension/starter-choice-view.ts`:
  ```ts
  export interface StarterChoiceView {
    choice(): CaptureChoice;
    /** Every control off while saving or stale; a scope control also while none of a multi-type family's boxes is checked. */
    setDisabled(disabled: boolean): void;
    /** Blanks typed scope values, then removes the controls. */
    clear(): void;
  }
  export function renderStarterChoice(container: HTMLElement, origin: string, url: string, onInput: () => void): StarterChoiceView;
  ```
  - The family is `starterFamily(origin)`. A one-type family with no scope renders nothing, and its `choice()` returns `{ types: new Set(), scopes: {} }`.
  - Markup, in this order (the keyboard order matches today's):
    - `div.starter-choice[role=group][aria-labelledby=capture-site-starter-prompt][data-family]`
    - `p#capture-site-starter-prompt` holding `review_prompt`
    - for an unscoped family, `p` holding the notice
    - when there are two or more types, `div.field` with one `label.check-label > input[type=checkbox][checked][data-starter-type] + span` per `displayTypes` entry. The span text is the sentence-cased `choiceNoun` + ` · ` + `STARTER_STATUS_TEXT[state]`.
    - for a scope input, `div.field > label[for=capture-site-starter-scope] + input#capture-site-starter-scope[type=text][maxlength][placeholder][autocomplete=off][spellcheck=false][data-starter-scope]`, pre-filled with `fromUrl(url) ?? ""`
    - for an unscoped family, a `p.hint` with `unscopedNotice(family)` and no input
- Produces, in `harness.mjs`:
  - `starterGroup(page)`: the first child of `#capture-site-starter`, or `null`
  - `starterBox(page, type)`: the `INPUT` whose `dataset.starterType === type`, or `null`
  - `starterScope(page, family)`: the `INPUT` whose `dataset.starterScope === family`, or `null`

- [ ] **Step 1: Migrate the panel tests mechanically.**
  - `page.el("capture-site-jira")` → `starterBox(page, "jira.issue")`, and likewise for `confluence.page`, `github.repo`, `github.pr` and `github.issue`.
  - `page.el("capture-site-orgs")` → `starterScope(page, "github")`.
  - `page.el("capture-site-github").hidden === true` (and `-atlassian`) → `starterGroup(page) === null`, or `starterGroup(page).dataset.family === "<family>"` where today it asserts `hidden === false`.
  - Change the three source-line assertions at `capture-site.test.mjs:121-129` to the Open decision 1 copy. **Nothing else in the assertions changes.**
- [ ] **Step 2: Write the failing panel tests** in `panel/capture-site.test.mjs`:
  - `each starter checkbox says whether it is certified`: on Atlassian, `starterBox(page,"jira.issue")`'s label text is `Jira issues · certified` and Confluence's is `Confluence pages · not yet checked on a real account`. On GitHub the three read `Repositories · certified`, `Pull requests · certified` and `Issues · certified`, in that order.
  - `closing a GitHub review leaves nothing for the next site`: type `sensitive-org`, keep a reference to the input, cancel, and open on `https://acme.atlassian.net/browse/WORK-1`. The old input's `value === ""`, `starterScope(page, "github") === null`, and the group's `dataset.family === "atlassian"`.
  - `an unscoped starter shows its every-account notice` (`EXAMPLE_UNSCOPED_FAMILY` via the seam, imported from `../../dist/starter-adapters.js` before `install`, at `https://everyone.test/p/1`): the notice text is shown, there is no scope input (`starterScope(page,"everyone") === null`), and Save enables after the preflight.
  - `a tenant_name starter renders its own field` (`EXAMPLE_TENANT_FAMILY`, `https://example.test/acme/item/1`): the label text is `Workspaces`, the input is empty, the boxes read `Example items · not yet checked on a real account` and `Example notes · not yet checked on a real account`, and typing `acme` gives the preview line `Records Example items and Example notes only for: acme.`.
- [ ] **Step 3: Run** `npm run build:extension && node --test extension/tests/panel/capture-site.test.mjs`. Expected: FAIL (`No element capture-site-starter in panel.html`).
- [ ] **Step 4: Implement** `starter-choice-view.ts` and wire the panel to it:
  - `begin()` calls `this.starterView = renderStarterChoice(...)` only for `mode === "starter"`, else it clears.
  - restore and `close()` call `clear()`.
  - `updateControls` calls `setDisabled(this.saving || stale)`.
  - `updatePreview` uses `this.starterView.choice()`.
  - Delete the Task 2 bridge and the removed fields.
  - Source copy per Open decision 1, with `joinRecordedTypes` for the mix.
  - Restore names come from `recordLabel(type) ?? type.replace(/[._]/g, " ")`.
  - The records line: when `scopeInput(family)` exists, `records(joinRecordedTypes(displayTypes(family).filter(checked).map(choiceNoun)), values.join(", "))`.
- [ ] **Step 5: Run** `npm run build:extension && npm run test:extension && npm run test:a11y && npm run typecheck && npm run lint`. Expected: PASS. `capture-explanation`, `capture-site-agent`, `-recurring`, `-draft-save` and `a11y/*` are unchanged (`git diff --exit-code` on them). The a11y run's `panel/capture-site-review` axe audit (`keyboard.test.mjs:108-116`) passes on the rendered Atlassian group.
- [ ] **Step 6: Commit:** `feat(capture-site): render starter choices from the family list and label unchecked starters`.

### Task 4: Settings gallery and site coverage from data

**Files:**
- Modify: `extension/options.ts:142-145` (scope state per origin), `:434-441` (`githubScope` → `scopeValues(origin)`), `:452-516` (`renderStarterGallery`), `:511` (`isContainerType`)
- Modify: `extension/options.html:58-59`
- Modify: `extension/site-coverage.ts:12-19,58-63,89-98` (delete `GITHUB_RECORD_LABELS`/`ORDER`; use `recordLabel`/`recordOrder()`)
- Modify: `DESIGN.md` (the Capture Site / starter gallery paragraph: status labels and the unscoped every-account notice, two sentences); add `docs/improvement-feed/2026-11-03-<slug>.md` for the PR's closeout suggestion
- Test: `extension/tests/options.test.mjs` (append); `site-coverage.test.mjs` unchanged

**Interfaces:**
- Consumes: `starterFamilies`, `starterFamily`, `starterStatus`, `STARTER_STATUS_TEXT`, `configuredScope`, `isContainerType`, `recordLabel`, `recordOrder` (Task 1); `scopeInput`, `unscopedNotice` (Task 2).
- Produces:
  - `starterGalleryHint(): string` (in `options.ts`) = `` `Enter an ${joinOr(starterFamilies().map((f) => f.display_name))} site address above to see starter drafts.` ``. `joinOr` gives `A`, `A or B`, or `A, B, …, or Z`.
  - State: `scopeTexts: Map<string, string>` and `scopeEdited: Set<string>`, keyed by origin. A not-yet-edited origin is refilled from `configuredScope(artifacts, origin).join(", ")` on each render, as GitHub is today.

- [ ] **Step 1: Write the failing tests** in `options.test.mjs`:
  - `the gallery hint joins the family display names`: with no site typed, `nodeText(starter-gallery)` equals the Global Constraints hint exactly.
  - `gallery cards say whether each starter is certified`: for `https://acme.atlassian.net`, the Jira card contains `Certified` and the Confluence card contains `Not yet checked on a real account`.
  - `each scoped origin gets its own scope input`: `setStarterFamiliesForTest([...STARTER_FAMILIES, EXAMPLE_TENANT_FAMILY])`, configure one adapter on each of `https://github.com` and `https://example.test`, and open settings. There are two inputs, labelled `Work organizations (comma-separated)` and `Workspaces (comma-separated)`. Typing an invalid value in the Workspaces input leaves the `github.pr` button's `dataset.available` as it was.
  - `an unscoped card shows its notice and no input`: with `EXAMPLE_UNSCOPED_FAMILY` and `https://everyone.test` typed, the card text contains the notice and `Not yet checked on a real account`, and the gallery has no `input`.
- [ ] **Step 2: Run** `npm run build:extension && node --test extension/tests/options.test.mjs`. Expected: FAIL.
- [ ] **Step 3: Implement the gallery.**
  - For each card, add `addText(card, "small", <sentence-cased STARTER_STATUS_TEXT[state]>)` after the description.
  - On the first card of each origin whose family has `scopeInput`, render today's label/input/guidance block from the registry strings.
  - On an unscoped family's card, render `addText(card, "small", notice)`.
  - The `input` listener refreshes only the buttons whose `dataset.siteOrigin === origin` (as today).
  - `beginAdapterDraft(..., isContainerType(selected.artifact_type))`.
  - In `options.html`, line 58 becomes `Start from a readable template for common work tools, then inspect its rules and examples before you save it. Each starter says whether it has been checked on a real account.`, and line 59 is an empty `<div id="starter-gallery" class="starter-gallery"></div>`.
- [ ] **Step 4: Implement site coverage.** `typeNoun` uses `recordLabel(type) ?? matchingStarter(adapter)?.label ?? type`. The grouped nouns in `siteCoverage` use `recordOrder()`. Delete the two `GITHUB_RECORD_*` exports; nothing imports them after Task 3 (`grep -rn GITHUB_RECORD extension --include='*.ts'` is empty).
- [ ] **Step 5: Run the full extension gate:** `npm run build:extension && npm run test:extension && npm run test:scripts && npm run test:a11y && npm run typecheck && npm run lint && node scripts/generate-starters.mjs --check`. Expected: all PASS. `git diff origin/main --exit-code -- extension/tests/starter-adapters.test.mjs extension/tests/site-coverage.test.mjs extension/tests/panel/capture-explanation.test.mjs extension/tests/a11y` is clean, and `grep -nE '"(atlassian|github|miro|sharepoint)"' extension/*.ts | grep -v generated` is empty.
- [ ] **Step 6: Commit:** `feat(options): starter gallery and coverage labels from the family list`.
