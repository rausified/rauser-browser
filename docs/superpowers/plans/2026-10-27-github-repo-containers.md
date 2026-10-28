# GitHub Repository Container Artifacts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record every visit inside a GitHub repository against the repository, and also against the PR or issue it is on. The repository is one record whose totals count each second once. Org setup is pre-filled from the URL.

**Architecture:** The adapter-template language gains two general features in both engines (TS worker, Rust host), each pinned by shared conformance vectors:
- a `scope`-restricted required path capture counts as an anchor;
- an optional adapter key `within: {type, id}` names a container artifact.

The host index already derives everything from the append-only logs and has the adapter document available. It re-evaluates each logged visit, and when the match declares `within`, it adds the visit to the container row as well. Existing history therefore rolls up retroactively. Protocol v7 exposes the container and its contained artifacts to the panel card. A new `github.repo` starter, `within` on the PR/issue starters, explicit adapter ordering, and URL-derived org pre-fill complete the user-facing path.

**Tech Stack:** Rust (workspace crates `adapter-templates`, `protocol`, `host`), TypeScript MV3 extension (`extension/`, tests with `node:test` against `extension/dist`), JSON Schema → generated code via `protocol/generate.py`.

**Spec:** `docs/superpowers/specs/2026-10-27-github-repo-artifacts-design.md`

## Global Constraints

- Container ID for GitHub: `github.repo:{host}/{owner}/{repo}`. It is frozen once shipped (adapter-templates.md §8.2).
- Repo rule: `/{owner:name|lower}/{repo:name|lower}/{*}`, scoped by `owner`, on `https://github.com`.
- An `unscoped = "acknowledged"` adapter gets no scope anchor, and an unrestricted capture never anchors.
- The host stays authoritative (§10.2), and container derivation is a host or index concern.
- No double counting: a second on a PR counts once in the PR's totals and once in the repo's totals, and never twice in the repo's totals. Container `days_opened` is a set union.
- Never widen capture silently. Pre-filled orgs are shown and confirmed. Settings never rewrites existing scopes.
- Both engines must accept and reject the same documents and produce the same IDs and container IDs. After any engine change, run `npm run test:adapter-diff`.
- Bump the protocol from 6 to 7 everywhere: all `"protocol_version": {"const": 6}` in `protocol/schema.json`, then regenerate, then `extension/native.ts:9`, then the `protocol/README.md` changelog.
- `github.repo` is labeled a development draft until a live GitHub UI check is recorded in `docs/adapters/`.
- Work only in the worktree `/Users/jrauser/ENDURANCE/work/rauserBrowser-repo-containers`, on branch `feat/github-repo-containers`. Before editing `DESIGN.md`, `host/src/index.rs`, `host/src/config.rs`, or `extension/capture-site*.ts`, run `git fetch && git log HEAD..origin/main --oneline -- <file>`. If there are new commits, rebase first.
- Full gate, in this order: `python3 protocol/generate.py --check`, `cargo fmt --all -- --check`, `cargo clippy --locked --workspace --all-targets -- -D warnings`, `cargo test --locked --workspace`, `npm run typecheck`, `npm run lint`, `npm run build:extension`, `npm run test:extension`, `npm run test:adapter-diff`.

## Review Focus

1. **Day totals in the worklog double count.** If container visit rows are summed alongside their contained rows, a day's total is inflated. Expected: any per-day or overall total counts each visit once. Pinned in Task 4.
2. **John's existing PR and issue adapters have no `within`.** Nothing rolls up until settings adds it. Expected: settings offers repos with the orgs pre-filled and adds `within` to the existing PR/issue adapters in the same save, without tripping the route-history guard. Pinned in Tasks 3 and 9.
3. **A container whose repo page has never been opened directly.** Expected: its card and Recent row show title `owner/repo` and URL `https://github.com/owner/repo`, never "Untitled" or an empty link. Pinned in Task 4.
4. **Retroactive roll-up of logged events whose adapter has since changed or been removed.** Expected: an event rolls up only when re-evaluating its logged URL yields the same `artifact_id`. Otherwise it is left alone, with no crash and no misattribution. Pinned in Task 4.
5. **Owner pre-fill on non-repo GitHub pages.** This covers `/settings`, `/notifications`, `/orgs/<org>/…`, mixed-case owners, and personal accounts. Expected: reserved routes give no pre-fill, `/orgs/Cisco-SBG/x` gives `cisco-sbg`, and any other owner is lowercased and shown for confirmation. Pinned in Task 8.

---

### Task 1: Scoped tenants count as anchors (both engines)

**Files:**
- Modify: `extension/adapter-templates.ts`. Today `isAnchorSegment` (:550) is called in `compileRule` (:590). Have `compileRule` return its required path segments, and run the anchor check in `compileAdapter` after step 7 (scope, :418-453).
- Modify: `adapter-templates/src/path.rs`. Remove the check from `PathTemplate::parse` (:138-140) and make `is_anchor` `pub(crate)`.
- Modify: `adapter-templates/src/document.rs`. Add the check in `validate_adapter` after step 7 (scope, :652-693).
- Modify: `protocol/adapter-templates/README.md`. Remove 4.3.3.5 and add 4.7.9: "for each rule, `no_literal_segment` unless some required path segment is an anchor (§4.1) or is a required path capture whose name is a `scope` key". Update the §4.1 prose reference and the case counts.
- Modify: `protocol/adapter-templates/vectors.json`. Add the cases below.
- Modify: `adapter-templates.md` §4.1 and §6. Add the rule in plain words, with GitHub as the worked example (the spec's goal 5 text), and say it applies to any shared origin with a tenant capture (Notion and Linear workspaces).

**Interfaces:**
- Produces: the error code `no_literal_segment` is unchanged. It is now reported at step 4.7.9 with `{adapter, rule}`.

- [ ] **Step 1: Write failing vectors** (kind `validate`, `impls: ["rust","ts"]`, origin `https://github.com`, type `github.repo`, id `github.repo:{host}/{owner}/{repo}`, path `/{owner:name|lower}/{repo:name|lower}/{*}`):
  - `validate.anchor.scoped_tenant_accepted`: with `scope: {owner: ["acme-corp"]}`, expect valid.
  - `validate.anchor.unscoped_tenant_rejected`: with `unscoped: "acknowledged"`, expect `{code: "no_literal_segment", adapter: 0, rule: 0}`.
  - `validate.anchor.scoped_optional_rejected`: path `/{owner?:name|lower}`, scoped. Expect the existing optional-scope error (`scope_capture_not_bound`), which shows that the order is unchanged for it.
  - `validate.anchor.scope_from_query_rejected`: path `/{repo:name}` with `query: {o: "{owner:name|lower}"}` and scope on `owner`. Expect `no_literal_segment` (a query binding is not a path anchor).
  - `eval.anchor.scoped_tenant_home`: URL `https://github.com/acme-corp/widgets` gives `artifact_id: "github.repo:github.com/acme-corp/widgets"`. `…/other/widgets` gives `{match: false, reason: "out_of_scope"}`.
  - The 4 existing `no_literal_*` vectors stay as they are.
- [ ] **Step 2:** `cargo test --locked -p adapter-templates` and `npm run build:extension && node --test extension/tests/adapter-templates.test.mjs`. Expected: the new accept/eval vectors FAIL with `no_literal_segment`.
- [ ] **Step 3:** Implement the moved check in both engines as described under Files.
- [ ] **Step 4:** Re-run both commands, then `cargo build --locked --workspace && npm run test:adapter-diff`. Expected: all pass.
- [ ] **Step 5:** Commit `feat(adapters): scoped tenant captures anchor path templates`.

### Task 2: `within` container declaration (both engines)

**Files:**
- Modify: `extension/adapter-templates.ts`. Add `within` to `ADAPTER_KEYS` (:74) and to `optionalTypes` (:320, which needs a table). Add `within?: {type: string; scheme: string; idParts: IdPart[]}` to `CompiledAdapter` (:111). Add the validation step. In `evaluate` (:139), add `within?: {type: string; artifact_id: string}` to the match result.
- Modify: `adapter-templates/src/document.rs`. Add `"within"` to `ADAPTER_KEYS` (:39). Add `within: Option<Container>` to `Adapter` (:260), plus a pub accessor `within()`. Add the validation step. Make `IdTemplate::build` usable for the container.
- Modify: `adapter-templates/src/eval.rs`. Add `pub within: Option<ContainerMatch>` to `Match` (:82), and populate it after the ID is built.
- Modify: `adapter-templates/src/error.rs`. Add the new codes and a `Field::Within` variant.
- Modify: `adapter-templates/src/lib.rs`. Export `Container` and `ContainerMatch`.
- Modify: `host/src/adapters.rs` `match_json` (:114). Emit `"within": {"type", "artifact_id"}` only when present.
- Modify: `extension/adapter-review.ts:42`. Add a human message for each new code.
- Modify: `protocol/adapter-templates/README.md`. Add `within` to the key list (:24), the code table, and a new step 4.12 after fields.
- Modify: `protocol/adapter-templates/vectors.json`.
- Modify: `adapter-templates.md`. Add §3 (the key), §8.3 (container IDs, including stability: changing `within` on an adapter with history is a contract change), and the roll-up semantics in one paragraph referencing DESIGN.md §5.

**Interfaces:**
- Produces (Rust): `pub struct Container { artifact_type: String, id: IdTemplate }`; `pub struct ContainerMatch { pub artifact_type: String, pub artifact_id: String }`; `Match.within: Option<ContainerMatch>`.
- Produces (TS): an evaluate match gains `within?: {type: string; artifact_id: string}`. The key is **absent** when the adapter declares no `within`, so existing match vectors stay byte-identical.
- Validation (step 4.12, in order):
  1. `wrong_type`, in step 4.1, unless `within` is a table with exactly the string keys `type` and `id`.
  2. `within_bad_type`: `type` fails the adapter-type grammar or is `web.page`.
  3. `within_self`: `type` equals the adapter's own `type`.
  4. `within_id_syntax`: the id fails the §8.1 grammar.
  5. `within_id_reserved_scheme`: the scheme is `web`.
  6. `within_unknown_capture`, reported per rule: a `{name}` is not a capture used by the adapter's `id` or listed in `scope`, or is `mode`.

  Because of rule 6, `extra_capture` is unaffected.
- Evaluate: if the container ID exceeds 512 bytes, the match is still returned, without `within`.

- [ ] **Step 1: Write failing vectors:**
  - one per new code;
  - `validate.within.accepted`: a GitHub PR adapter with `within: {type: "github.repo", id: "github.repo:{host}/{owner}/{repo}"}`;
  - `eval.within.pr`: `https://github.com/acme-corp/widgets/pull/7/files` gives `within: {type: "github.repo", artifact_id: "github.repo:github.com/acme-corp/widgets"}`;
  - `eval.within.absent_without_key`: assert there is no `within` key.
- [ ] **Step 2:** Run both engine test commands from Task 1. Expected: the new vectors FAIL, with `unknown_key` for `within`.
- [ ] **Step 3:** Implement as described under Files.
- [ ] **Step 4:** `cargo test --locked --workspace`, then `npm run build:extension && npm run test:extension`, then `npm run test:adapter-diff`. Expected: all pass.
- [ ] **Step 5:** Commit `feat(adapters): within declares a container artifact`.

### Task 3: Route-history guard allows a declared container

**Files:**
- Modify: `host/src/config.rs`.
  - Add `"within"` to `adapter_contract` KEYS (:2174).
  - In `validate_adapter_transition_with_history` (:2105), skip the bail when `previous` declares `within` with `type == candidate.type` and `within.id == candidate.id` (exact template string).
- Test: `host/src/config.rs` tests module. Model the tests on `removed_adapter_cannot_return_with_a_different_id_template` (:3671), using the helpers `adapter` (:3186) and `adapter_config` (:3195) with `scope` added.

**Interfaces:**
- Consumes: Task 2's accepted `within` key.
- Produces: `fn declared_container(previous: &serde_json::Value) -> Option<(&str, &str)>` → `(type, id)`.

- [ ] **Step 1: Write failing tests:**
  - `a_declared_container_may_overlap_retired_contained_routes`: history holds a retired `github.pr` contract (scope `["a"]`, `within` repo). Adding the `github.repo` catch-all (scope `["a"]`) via `store.update` gives `Ok`.
  - `an_undeclared_overlap_still_cannot_return_under_a_new_type`: the same, but the retired PR has no `within`. Expect an error containing "cannot change until artifact migrations".
  - `adding_within_to_an_existing_adapter_is_a_same_identity_edit`: the current config has PR without `within`, and next has PR with `within` plus the repo adapter. Expect `Ok`.
- [ ] **Step 2:** `cargo test --locked -p brauser config::`. Expected: the 1st and 3rd tests FAIL.
- [ ] **Step 3:** Implement the change described under Files.
- [ ] **Step 4:** Re-run. Expected: PASS, with all existing `config::` tests green.
- [ ] **Step 5:** Commit `feat(host): a declared container may claim retired contained routes`.

### Task 4: Index roll-up into containers

**Files:**
- Modify: `host/src/index.rs`.
  - In `derive` (:1400), Activity (:1407-1521) and Legacy (:1534) branches: after the normal accumulation, call `container_for(adapters, &event.url, &event.artifact_id)`. On `Some`, accumulate the same focused/background/opens/day into the container's `ArtifactRow`, push a `VisitRow` with `artifact_id = container` and a `contained: true` marker, and `add_edge(child, container, "within", &day)`.
  - Container `ArtifactRow` defaults when it is not visited directly: `artifact_type` = the container type, title = `container_title(id)`, url = `container_url(adapter_origin, id)`.
  - On adapter fingerprint change (`load_current_state` :745-751), mark all artifacts dirty, then apply `keep_unchanged_files_clean` (:515).
  - `worklog` (:1149) and any day-total computation skip `contained` visit rows when summing totals, but still list the container row.
- Test: `host/src/index.rs` tests module, modeled on `mentions_resolve_through_the_adapter_document` (:4439).

**Interfaces:**
- Consumes: `Document::evaluate(url, None) -> Result<Match, NoMatch>`, `Match.within` (Task 2).
- Produces:
  - `fn container_for(adapters: Option<&Document>, url: &str, artifact_id: &str) -> Option<(String /*type*/, String /*id*/, String /*origin*/)>`. It returns `Some` only when `evaluate` yields `artifact_id == artifact_id` and `within` is present.
  - `fn container_title(id: &str) -> String`: the native part after `<scheme>:<host>/`, e.g. `cisco-sbg/cloudsec-skills-marketplace`.
  - `fn container_url(origin: &str, id: &str) -> String`: `origin + "/" + native part`.
  - Container rows always use `container_title`, even after direct visits. Their direct-visit title never overrides it. This is goal 4, done host-side with no language change.
  - VisitRow gains `contained: bool`.

- [ ] **Step 1: Write failing tests** (document = Task 7's three GitHub starters with scope `["acme-corp"]`):
  - `contained_visits_roll_up_once_into_the_container`:
    - PR event: 125s focused / 61s background, 1 open, day D1.
    - Repo blob event: 30s / 0s, 1 open, day D1.
    - Expect repo row focused 155, background 61, opens 2, days_opened 1, and the PR row unchanged at 125/61/1.
  - `container_days_are_a_union`: PR on D1 and repo page on D2 give repo days_opened 2. Two PRs on D1 give days_opened 1.
  - `a_container_never_visited_directly_has_a_usable_title_and_url`: expect title `acme-corp/widgets` and url `https://github.com/acme-corp/widgets`.
  - `container_title_ignores_sub_page_titles`: a blob visit with title "widgets/README.md at main · acme-corp/widgets" leaves the title `acme-corp/widgets`.
  - `events_that_no_longer_match_their_id_do_not_roll_up`: logged artifact_id `github.pr:github.com/acme-corp/widgets/7` with url `https://github.com/acme-corp/widgets/issues/7`. Expect no repo row and no panic.
  - `within_edges_link_child_to_container`: `sqlite_check` on `edges` gives kind `within`.
  - `worklog_day_totals_count_each_visit_once`: the D1 total equals 155 focused, not 280.
  - `adding_within_regenerates_container_files`: reindex under the old document, then reopen with the new one. Expect the container artifact file to exist.
- [ ] **Step 2:** `cargo test --locked -p brauser index::`. Expected: the new tests FAIL.
- [ ] **Step 3:** Implement as described under Files and Interfaces.
- [ ] **Step 4:** Re-run, then run the whole `cargo test --locked --workspace`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(index): roll contained visits up into their container`.

### Task 5: Protocol v7: container on the artifact view

**Files:**
- Modify: `protocol/schema.json`. Make all `"protocol_version"` consts `7`. To `ArtifactView` (:1283), add required fields:
  - `within`: either `null` or `{artifact_id, artifact_type, title, url}`, all strings with maxLength 512 or 2048;
  - `contained`: an array, maxItems 20, of `{artifact_id, artifact_type, title, url, focused_seconds}`, sorted by focused_seconds descending.
- Run: `python3 protocol/generate.py`. This regenerates `protocol/src/generated.rs` and `protocol/ts/generated.ts`.
- Modify: `extension/native.ts:9` (set to `7`), `extension/protocol-shape.ts` `isArtifact` (:129) to validate the new fields, and the `protocol/README.md` changelog ("version 7: artifact containers").
- Modify: `host/src/index.rs` `artifact_view` (:1288) and `host/src/native.rs` `get_artifact` fallback (:1752-1774), to fill `within` from the edge and `contained` from inbound `within` edges. The fallback view for an artifact whose type is some adapter's `within.type` uses `container_title`/`container_url` (Task 4) instead of the tab title.
- Test: `host/src/index.rs` (`artifact_view_names_its_container_and_contents`), `extension/tests/panel.test.mjs` (shape validation rejects a view missing `contained`).

**Interfaces:**
- Consumes: Task 4's `within` edges.
- Produces: `ArtifactView.within: ArtifactRefView | null`; `ArtifactView.contained: ContainedView[]` (generated type names from generate.py).

- [ ] **Step 1:** Write the failing tests named above.
- [ ] **Step 2:** `cargo test --locked -p brauser index::artifact_view` and `npm run build:extension && node --test extension/tests/panel.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement. Then fix every compile and test fixture that builds an `ArtifactView`: give the new fields `null`/`[]`, and replace constants 6 with 7 in fixtures.
- [ ] **Step 4:** `python3 protocol/generate.py --check`, `cargo test --locked --workspace`, `npm run typecheck`, `npm run test:extension`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(protocol): v7 exposes artifact containers`.

### Task 6: Panel card shows container and contents

**Files:**
- Read first: `DESIGN.md` (panel and card style sections). This is required before any frontend change.
- Modify: `extension/panel.html`. In `#artifact-card`, after `#artifact-links` (:95), add `<p id="artifact-within" hidden></p>` and `<ul id="artifact-contained" class="plain-list" hidden></ul>`.
- Modify: `extension/panel.ts` `renderArtifact` (:151).
- Test: `extension/tests/panel.test.mjs`.

**Interfaces:**
- Consumes: Task 5's `ArtifactView.within` and `.contained`.
- Copy: `In <a>{within.title}</a>`. The contained list has the heading `Worked on inside` and rows of `title · {duration(focused_seconds)}`, linked to `url`. Both are hidden when null or empty.

- [ ] **Step 1: Failing tests:**
  - `artifact card links a PR to its repository`: the `#artifact-within` text is `In acme-corp/widgets` and its href is the repo url;
  - `repository card lists contained work`: 2 rows, ordered as given;
  - `card hides container rows when absent`.
- [ ] **Step 2:** `npm run build:extension && node --test extension/tests/panel.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement with `textContent` and DOM nodes only (no innerHTML), matching existing card idioms.
- [ ] **Step 4:** Re-run. Expected: PASS.
- [ ] **Step 5:** Commit `feat(panel): show an artifact's container and contents`.

### Task 7: `github.repo` starter, `within` on PR/issue, and ordering

**Files:**
- Modify: `extension/starter-adapters.ts`.
  - Add `githubRepository(organizations)`: rule `/{owner:name|lower}/{repo:name|lower}/{*}`, `scope.owner`, id as in Global Constraints, no `title` templates (the host derives container titles), no refs.
  - Add `within: {type: "github.repo", id: "github.repo:{host}/{owner}/{repo}"}` to `githubPullRequest` and `githubIssue`.
  - `starterOffers` returns `[github.pr, github.issue, github.repo]`. The repo offer's label is `GitHub repositories`, and its description is "Every page inside a repository counts toward it; pull requests and issues count toward both."
  - Add `orderContainedFirst(artifacts: AdapterObject[]): AdapterObject[]`: a stable reorder so that, on each origin, any adapter whose `within.type` is T comes before every adapter of type T.
- Modify: `extension/capture-site.ts:63` and `extension/options.ts` `saveAdapterDocument` (:638). Pass `artifacts` through `orderContainedFirst` before validation and save.
- Modify: `protocol/adapter-templates/vectors.json` `fixtures.starters`. Add the repo adapter and `within` on PR/issue, and add `eval.github.repo_*` cases:
  - home page → repo;
  - `blob/feature/x/README.md` → repo;
  - `tree/<40-hex sha>` → repo;
  - `pulls` → repo;
  - `pull/7/files` → PR with `within`;
  - `issues/3` → issue with `within`;
  - `/acme-corp` → not a match;
  - `/other/x` → out_of_scope;
  - `/settings/profile` → not a match.

  Also update README `starters` description (:316) and the case counts.
- Modify: `extension/tests/starter-adapters.test.mjs` `expectedTypes` (:30).
- Create: `docs/adapters/github.repo.md`, to the `github.pr.md` standard. Include sample URLs, the frozen ID, the `within` relationship, the scope-anchor rule, the development-draft status, and known limits: a terminal scope miss drops a PR that is outside the PR scope but inside the repo scope; files are not separate artifacts; no rename aliases.
- Modify: `docs/adapters/github.pr.md` and `github.issue.md` to cover `within`.

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces: `orderContainedFirst`; the starter key `"github.repo"`.

- [ ] **Step 1: Failing tests:**
  - the vectors above;
  - `orderContainedFirst moves a later PR adapter ahead of the repo catch-all`;
  - `orderContainedFirst is stable and leaves other origins alone`;
  - `capture plan puts PR and issue before repo`.
- [ ] **Step 2:** Run both engine suites, then `node --test extension/tests/starter-adapters.test.mjs extension/tests/capture-site.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Full engine suites plus `npm run test:adapter-diff`. Expected: PASS.
- [ ] **Step 5:** Commit `feat(starters): GitHub repositories as container artifacts`.

### Task 8: Capture Site: one org field, pre-filled from the URL

**Files:**
- Modify: `extension/starter-adapters.ts`. Add `githubOwnerFromUrl(url: string): string | null`.
  - It returns the lowercased first path segment of `https://github.com/<owner>/…`, or the segment after `/orgs/`.
  - It returns `null` for a non-GitHub origin, the root, a value that `parseGitHubOrganizations` rejects, and the reserved routes `settings, notifications, search, orgs (no org), explore, marketplace, topics, trending, collections, sponsors, login, logout, signup, new, pulls, issues, codespaces, dashboard, features, enterprise, pricing, about, apps, users, account, stars, watching`.
- Modify: `extension/capture-site.ts`.
  - `CaptureChoice` becomes `{organizations: string; repositories: boolean; pullRequests: boolean; issues: boolean}`.
  - One parsed scope is used for every checked GitHub type.
  - Add a repo example URL: `${origin}/${owner}/example-repository`.
- Modify: `extension/capture-site-panel.ts` and `extension/panel.html:59-82`.
  - Replace the two org fields with one `#capture-site-orgs` input and three checkboxes: `#capture-site-repos`, `#capture-site-prs`, `#capture-site-issues`. All are checked on open.
  - `open()` pre-fills from `githubOwnerFromUrl(this.page.url)`.
  - The preview line reads `Records repositories, pull requests and issues owned by: {orgs}.`
- Modify: `extension/tests/capture-site.test.mjs` (replace "GitHub choices keep PR and issue scopes separate", :39-53) and `extension/tests/panel.test.mjs:585-608`.

**Interfaces:**
- Consumes: Task 7's offers.
- Produces: `githubOwnerFromUrl`.

- [ ] **Step 1: Failing tests:**
  - `githubOwnerFromUrl`: `https://github.com/Cisco-SBG/x/blob/main/a.md` → `cisco-sbg`; `https://github.com/orgs/cisco-sbg/repositories` → `cisco-sbg`; `https://github.com/settings/profile` → `null`; `https://github.com/` → `null`; `https://example.com/cisco-sbg` → `null`; `https://github.com/rausified/dotfiles` → `rausified`;
  - `one organization field scopes every checked GitHub type`;
  - panel `Capture Site pre-fills the organization from the current GitHub page`;
  - panel `unchecking repositories omits the repo adapter`.
- [ ] **Step 2:** `npm run build:extension && node --test extension/tests/capture-site.test.mjs extension/tests/panel.test.mjs extension/tests/starter-adapters.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Re-run. Expected: PASS.
- [ ] **Step 5:** Commit `feat(capture-site): one GitHub organization field, pre-filled from the page`.

### Task 9: Settings: add repositories to an existing GitHub setup

**Files:**
- Modify: `extension/options.ts`.
  - On render, seed `githubScopeText` (:95) with the comma-joined union of `scope.owner` across configured `https://github.com` adapters, but only while the user has not edited it.
  - When the chosen starter is `github.repo` and configured PR/issue adapters lack `within`, the review text adds: "Also links your existing pull request and issue rules to their repositories." Saving builds one document: the existing adapters, with `within` added and scope untouched, plus the repo adapter, passed through `orderContainedFirst`. It is saved via `saveAdapterDocument`.
- Test: `extension/tests/options.test.mjs`.

**Interfaces:**
- Consumes: Task 7's `orderContainedFirst` and starter builders; Task 3's guard allowance.
- Produces: `withContainerLinks(artifacts: AdapterObject[], repo: AdapterObject): AdapterObject[]`, which adds `within` to same-origin `github.pr`/`github.issue` adapters that lack it.

- [ ] **Step 1: Failing tests:**
  - `GitHub gallery pre-fills organizations from existing GitHub rules`, with PR `["a"]` and issue `["b"]` → the input value is `a, b`;
  - `adding repositories links existing PR and issue rules without changing their scope`, asserting on the saved document: PR and issue each have `within`, their scope is unchanged, and they come before the repo;
  - `github.repo is offered when only PR and issue rules exist`.
- [ ] **Step 2:** `npm run build:extension && node --test extension/tests/options.test.mjs`. Expected: FAIL.
- [ ] **Step 3:** Implement.
- [ ] **Step 4:** Re-run. Expected: PASS.
- [ ] **Step 5:** Commit `feat(settings): add GitHub repositories to an existing setup in one step`.

### Task 10: Design docs and capability registry

**Files:**
- Modify: `DESIGN.md`.
  - §5: artifacts can sit within a container, with the roll-up rules (each second once, days as a union, the `within` edge, title derived from the ID) and the starter list adding `github.repo`.
  - §5.1: the guard amendment and the scope-anchor rule (a pointer to adapter-templates.md §4.1/§6).
  - §6.2: container IDs as record keys.
  - The M1.5a / step 6.8 notes.
- Modify: `adapter-templates.md` §11, to add the repo starter alongside PR and issue.
- Modify: `/Users/jrauser/ENDURANCE/AMOS/inventory/` Brauser entry. Read its instructions and schema first, update the entry, and run its validator.

- [ ] **Step 1:** Make the edits. Use `grep -n "github.pr\|GitHub pull requests and issues" DESIGN.md adapter-templates.md` to find every starter list that must now name repositories.
- [ ] **Step 2:** Run the inventory validator named in its instructions. Expected: valid.
- [ ] **Step 3:** Commit `docs: GitHub repository containers and scoped-tenant anchors`.

### Task 11: Integration, review, merge, deploy

- [ ] **Step 1:** Run the full gate (Global Constraints). Expected: every command exits 0. Report EPERM socket failures listed in TESTING.md separately; they are not regressions.
- [ ] **Step 2:** Push the branch and open a PR to `rausified` (per the owner map). The body summarizes the spec, the Review Focus items, and whether existing history rolls up (it does, via index-time derivation).
- [ ] **Step 3:** Run `codex exec review` on the branch, then fix and re-run until clean. Wait for CI to be green, then merge.
- [ ] **Step 4:** Deploy following the `browser-extension-deployment` skill:
  - in the live checkout `work/rauserBrowser`, fast-forward `main`;
  - run `npm run build:extension`;
  - build the host release that matches the native host manifest;
  - verify that the manifest points at the protocol-7 build.

  Then ask John to reload Brauser in `chrome://extensions`, open settings, and add GitHub repositories.
- [ ] **Step 5:** After John reloads: on `github.com/cisco-sbg/cloudsec-skills-marketplace`, the panel shows "● Artifact recognized" with `cisco-sbg/cloudsec-skills-marketplace`. On a PR in that repo, the card shows "In cisco-sbg/cloudsec-skills-marketplace".
