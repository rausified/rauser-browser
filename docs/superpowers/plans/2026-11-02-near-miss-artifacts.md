# Near-miss artifacts: explain "Not an artifact" and offer the missing rule

Beads: brwr-qn4 (explain why a page on a captured site is not an artifact) and brwr-0t1 (near-miss detection with a one-click rule offer). Branch `feat/near-miss-artifacts`, based on main `f5a2c3e` (PR #233).

## Goal

On an origin that has capture rules, a page the rules do not recognize gets a sentence saying what the site *does* capture, and when a configured rule set is one known rule short of covering the page, a button that opens that rule in the settings editor for review. Nothing is saved from the panel.

## Non-goals

- Changing the host verdict, `get_artifact`, `ArtifactResult`, the Rust engine, or `adapter-templates.ts`. No new protocol field, no new shared vectors.
- Widening scope from the panel. A GitHub repository in an organization outside `scope.owner` (`out_of_scope`) is explained, not offered. Scope is the privacy boundary, and it stays a settings-only edit.
- Drafting new rules from a page path. The recurring-pages hint and Capture Site's drafted flow already do that.
- Reading page content or making network calls. Everything here uses the URL, the policy lease and the starter catalog.

## Where things stand (main f5a2c3e)

- The status chip `○ Not an artifact` comes from the host's `recognized` flag (`extension/panel.ts` `renderArtifact`). It stays as it is: it is the host's verdict.
- `renderCaptureExplanation` (`panel.ts`) sits directly under the chip in `#capture-explanation`. Behind its fail-closed guards (config or adapter issue, no home folder, pending removal, pause, credentials in the URL, expired lease) it evaluates the URL against the worker lease (`capturePolicy.restore(leaseFor(config, revision))` plus `compiledFor`) and switches on the reason. Both features add to the `no_rule` and `out_of_scope` branches. Nothing else changes.
- The only reviewed save path for rules is the settings adapter editor: `openAdapterEditor` → `saveAdapter` → `saveAdapterDocument`. The host re-validates the save.

## Design

### 1. New pure module `extension/site-coverage.ts`

It has no DOM and no Chrome APIs. It takes raw `AdapterObject[]` (the lease's `adapters.artifacts`) and a URL. It builds on the exported `compileDocument`, `evaluate`, `canonicalCapture`, `matchingStarter`, `starterRuleAddition` (section 3) and `canonicalJson`. Because the host never consumes it, it needs no Rust twin.

`GITHUB_RECORD_LABELS`, `GITHUB_RECORD_ORDER` and `joinRecordedTypes` move here from `capture-site-panel.ts`, which imports them back. That move changes no behaviour.

```ts
export interface Coverage {
  host: string;          // new URL(url).host
  types: string[];       // plain-language nouns, deduped, in display order
  accounts: string[];    // scope values shown after "for", up to 3
  moreAccounts: number;  // further scope values beyond those 3
}
export function siteCoverage(artifacts: readonly AdapterObject[], url: string): Coverage | null;
export function coverageSentence(coverage: Coverage, reason: "no_rule" | "out_of_scope"): string | null;

export type NearMiss =
  | { kind: "starter_rule"; adapter: number; label: string; rule: AdapterRule }
  | { kind: "identifier"; adapter: number; label: string; key: string };
export function nearMiss(artifacts: readonly AdapterObject[], url: string): NearMiss | null;
```

**Type nouns.** These come from the adapters whose `origin` equals the URL's origin, in config order:

- a `github.repo`, `github.pr` or `github.issue` adapter: `GITHUB_RECORD_LABELS` ("repositories", "pull requests", "issues"), sorted by `GITHUB_RECORD_ORDER`;
- any other adapter `matchingStarter` ties to a starter: that starter's `label` ("Jira issues", "SharePoint documents", "OneDrive for work documents", "Miro boards");
- otherwise the adapter's `type` string, for example `example.page`.

If no adapter on the origin has a type, there is no coverage and the result is `null`.

**Accounts.** These are shown only when every adapter on the origin has an object `scope`. Take the union of the string values across all its keys, lowercased and deduped in first-seen order. Show the first three and count the rest. With a mixed or unscoped origin (`unscoped: "acknowledged"`, or no scope), `accounts` is `[]`, because naming accounts that some rules ignore would mislead.

**Sentences.** These are exact. The list joins with `joinRecordedTypes` (no Oxford comma), the account list joins the same way, and with `moreAccounts` the account list ends "and N more".

| reason | accounts | text |
| --- | --- | --- |
| `no_rule` | some | `<host> captures <types> for <accounts>; this page isn't one of them.` |
| `no_rule` | none | `<host> captures <types>; this page isn't one of them.` |
| `out_of_scope` | some | `<host> captures <types> for <accounts>; this page belongs to another account.` |
| `out_of_scope` | none | `null` (keep today's sentence) |

Example: `github.com captures repositories, pull requests and issues for cisco-sbg; this page isn't one of them.`

**Near-miss detection** (brwr-0t1). The caller runs it only after the full lease document returned `no_rule` for this URL. The first hit wins, in this order:

1. **Starter rule (offer).** For each adapter on the origin, in config order, with `current = matchingStarter(adapter)?.adapter`: for each rule of `current.match`, in order, whose `canonicalJson` is not among the adapter's rules, compute `addition = starterRuleAddition(adapter, rule)`. A hit is when `addition` is non-null and `evaluate(compileDocument({format: 1, artifacts: [addition.adapter]}).doc, url).match` is true. The adapter's own scope applies. An `out_of_scope` here is not a hit, and neither is any other non-match. The full document matched no rule, so the added rule would be the page's first match after the save. The hit's `label` is the starter's label. The sites involved come from the starter catalog (`starterOffers`), never from a hard-coded list in this module.
2. **Identifier (explain, no ready rule).** For each adapter on the origin, look at each rule whose `query` is an object. A query entry counts when its value is exactly one capture `{name:ctype}` whose base type (before any `|` modifier) is fixed-shape, matching the anchor definition in `isAnchorSegment`: `uuid`, or `hex(N)` / `hex(N,N)` with N ≥ 32. It is a hit when the URL has exactly one value for that key and `canonicalCapture(ctype, value)` is non-null. The hit's `label` is the adapter's type noun (as above, singular is not needed) and `key` is the query key. A `Doc.aspx` link with no sourcedoc, or with an invalid one, and a `file=`-only link are therefore not near-misses, as `docs/adapters/sharepoint.doc.md` requires.

Path-only identity (a GitHub repository path, a Jira key in a path) is never guessed. It is covered by tier 1 when a starter has the rule, and otherwise by the coverage sentence.

### 2. Panel (`extension/panel.ts`)

These changes are in `renderCaptureExplanation` only, after the existing guards, using the same `lease` it already restored. The lease and `session.config` share a revision, so adapter indices and identities agree with what settings will read.

- In `case "no_rule"`, compute `coverage = siteCoverage(lease.adapters.artifacts, url)` and `miss = nearMiss(lease.adapters.artifacts, url)`. The text is `coverageSentence(coverage, "no_rule")`, or today's sentence when there is no coverage, followed by:
  - `starter_rule`: ` The current <label> starter has a rule for pages like this one.` The button reads **Review rule**, with `dataset.action = "adapter-review"`.
  - `identifier`: ` This page has a <label> ID (<key>), but its address is not one of the routes the rules cover.` The button reads **Edit rules**, with `dataset.action = "adapter-review"`.
  - none: ` Only matching pages are remembered.` The button stays **Open settings**.
- In `case "out_of_scope"`, use `coverageSentence(coverage, "out_of_scope") ?? <today's sentence>`, followed by ` Review this site's scope in settings.` The button stays **Open settings**.
- The text is still set only through `captureExplanationText.textContent`. Scope values and types are user config and never page text, but they are still never put in innerHTML.
- The click handler for `adapter-review` stores the pending `miss` (adapter identity plus rule, or `null` for an identifier) and calls `openAdapterReview(identity, rule)`. That opens `chrome.tabs.create({url: chrome.runtime.getURL("options.html#adapter-review=" + encodeURIComponent(JSON.stringify({adapter: identity, rule})))})`. Here `identity` is `[origin, type, id]`, the same triple `options.ts` `adapterIdentity` uses. The hash carries no URL, path, query value or title, only config and catalog data. If it fails, the panel uses `show("Could not open settings: …", true)`.
- The chip `● Artifact recognized` / `○ Not an artifact` and `renderArtifact` are unchanged.

### 3. `extension/starter-adapters.ts`: `starterRuleAddition`

```ts
/** `adapter` with one rule of its current starter appended: the rule must be
 * in the current starter's `match`, absent from the adapter, keep the adapter
 * within 16 rules, and compile. `modes` gains only what the rule's `mode`
 * capture needs. Nothing else about the adapter changes. */
export function starterRuleAddition(adapter: AdapterObject, rule: AdapterRule): StarterRuleUpdate | null;
```

It returns `{adapter, added: [rule], removed: []}`. Factor the modes-merge loop now inside `starterRuleUpdate` into a private `withStarterModes(updated, own, current, added)` and have both functions use it. Unlike `starterRuleUpdate`, this function is not gated by `STARTER_HISTORY`. It is deliberately per-page and per-rule, and the user confirms it in the editor, so a hand-narrowed adapter is offered only the one rule that covers the page in front of them. It is never offered a wholesale widening.

### 4. Settings (`extension/options.ts`): `#adapter-review=`

`openHandedOffAdapterReview()` runs beside `openHandedOffDraft()` at startup, when the session is connected. It parses `location.hash` with `/^#adapter-review=(.{1,4096})$/`, then `decodeURIComponent`, then `JSON.parse`, all inside try/catch, and expects `{adapter: [string, string, string], rule: object | null}`. Then:

1. It finds the saved adapter whose `adapterIdentity` equals `JSON.stringify(adapter)`. If there is none, it calls `show("This site's rules changed since the panel offered this. Open the panel on the page again.", true)`.
2. With `rule === null`, it calls `beginAdapterEdit(index)`, the existing Edit and preview.
3. With a rule:
   - If `starterRuleUpdate(saved)?.added` contains the rule (by `canonicalJson`), it calls `beginStarterUpdate(index)`, the existing **Update to current rules** review, which also retires the superseded rules. This is the stale OneDrive case behind PR #161.
   - Otherwise, if `starterRuleAddition(saved, rule)` is non-null, it calls the new `beginStarterRuleAddition(index, rule)`. That opens the editor with `{kind: "existing", identity, snapshot, starterUpdate: true}`, titled `Add a rule to <adapterLabel>`, with review line `Adds 1 rule from the current <label> starter for the page you were on:`, item `Adds <ruleDescription(rule, null)>`, and example `{...addition.adapter, match: [rule]}`.
   - Otherwise it calls `show("This rule can no longer be added here; edit the rules by hand.", true)`, then `beginAdapterEdit(index)`.
4. It clears the hash with `history.replaceState`, so a reload does not reopen the editor.

Saving is the editor's existing **Save**: `saveAdapter`, then `saveAdapterDocument` with `explainStarterUpdateFailure`. Nothing saves on load. A hand-crafted hash can open the review of a starter rule that already exists in the catalog, and nothing more. Arbitrary rules are refused in step 3.

## Acceptance criteria

**brwr-qn4**

1. On `https://github.com/search?q=work`, with the acme-scoped GitHub starters, `#capture-explanation-text` reads `github.com captures repositories, pull requests and issues for acme; this page isn't one of them. Only matching pages are remembered.` The chip still reads `○ Not an artifact`.
2. On `https://github.com/personal/repo/pull/12`, the same config gives `github.com captures repositories, pull requests and issues for acme; this page belongs to another account. Review this site's scope in settings.`
3. On a Jira starter origin, a non-issue page reads `<host> captures Jira issues; this page isn't one of them. …`, with no "for".
4. A custom adapter of type `example.page` shows `example.page` as its noun. More than three scope values show the first three and "and N more". A mixed scoped and unscoped origin shows no "for".
5. Every existing guard still wins: pause, pending removal, expired lease, repair, credentials, no home. A recognized page still hides the explanation, and the button still opens settings. No config write happens (`updates(page)` is unchanged).
6. Capture Site's confirmation copy is unchanged after the label move (the existing capture-site tests pass).

**brwr-0t1**

1. A saved pre-2026-10-28 SharePoint adapter (the `staleOneDrive` fixture) on `https://<tenant>-my.sharepoint.com/:w:/r/personal/u/_layouts/15/Doc.aspx?sourcedoc={GUID}` shows the coverage sentence, then ` The current OneDrive for work documents starter has a rule for pages like this one.`, and the button **Review rule**. Clicking it opens `options.html#adapter-review=…` in one new tab. The hash contains neither the page path nor the GUID. The panel calls no `save_config`.
2. Opening that hash in settings shows the **Update to current rules** review for that adapter, the same as clicking the card button. Save writes it. Cancel writes nothing.
3. A hand-narrowed SharePoint adapter that holds only `/sites/{_}/_layouts/15/Doc.aspx`, on a `/personal/u/_layouts/15/Doc.aspx?sourcedoc={GUID}` page, is offered exactly the starter's `/personal/…` rule. Settings opens `Add a rule to sharepoint.doc` with one `Adds …` item. Save appends that one rule and changes nothing else.
4. A custom adapter that requires `sourcedoc={doc:uuid}` on `/a/Doc.aspx`, on `/b/Doc.aspx?sourcedoc={GUID}`, reads `… This page has a <type> ID (sourcedoc), but its address is not one of the routes the rules cover.` Its button is **Edit rules**, and settings opens Edit and preview for that adapter.
5. These are not near-misses: `Doc.aspx` with no sourcedoc, a non-GUID sourcedoc, a `file=`-only link, two sourcedoc values, a GitHub repo outside scope (`out_of_scope`), and a single-segment GitHub page. Each gets the plain coverage text and **Open settings**.
6. A stale hash (adapter removed, or rule already present or not in the catalog) shows the plain message above and saves nothing. A malformed hash is ignored.

## Tests

- `extension/tests/site-coverage.test.mjs` (new, added to `test:extension`) covers the unit tests for `siteCoverage`, `coverageSentence` and `nearMiss` from `dist`: GitHub, Jira, SharePoint and custom adapters, account truncation, mixed scope, every non-near-miss case above, and the requirement that tier 1 yields no hit when the adapter's scope rejects the page.
- `extension/tests/starter-adapters.test.mjs`, beside the `starterRuleUpdate` cases: add `starterRuleAddition` (accepts a catalog rule, refuses a rule not in the catalog, a duplicate, the 17th rule and a modes merge), and check that `starterRuleUpdate` output is unchanged after the refactor.
- `extension/tests/panel/capture-explanation.test.mjs`: update the "unmatched page" and "outside scope" expectations, and add qn4 criteria 1-5 and the 0t1 panel criteria (1 and 3-5), asserting the `tabs.create` URL and no config write.
- `extension/tests/options.test.mjs`, beside the Update to current rules tests: open `#adapter-review=` for the starter-update, addition, identifier (rule null), stale and malformed cases, and assert that nothing saves before Save.

Run `npm run build:extension`, `npm run typecheck`, `npm run lint` and `npm run test:extension`. No Rust change, so cargo is untouched.

## Docs

- DESIGN.md §5.7, "Capture exclusions" paragraph: add the coverage sentence and the near-miss line and button. Restate that they are local, URL-only and lease-based, never change the chip or the host verdict, and that saving happens only through the settings review.
- DESIGN.md §5.7 state table, row "Enabled origin, page not an artifact": add the coverage sentence and the **Review rule** / **Edit rules** offers.
- DESIGN.md §5.1: one sentence noting that an unrecognized page on a configured origin is explained in terms of the artifact types the site captures.
- No AMOS inventory or briefing change: this is a UX refinement inside the existing capture capability.

## Work split (parallel)

- **A, qn4** (no dependencies): `site-coverage.ts` with `siteCoverage` and `coverageSentence`, the label move out of `capture-site-panel.ts`, the panel `no_rule`/`out_of_scope` text, its tests, and the DESIGN.md coverage text.
- **B, settings** (no dependencies): `starterRuleAddition` plus the modes refactor in `starter-adapters.ts`, `#adapter-review=` and `beginStarterRuleAddition` in `options.ts`, and the starter-adapters and options tests.
- **C, 0t1 panel** (after A and B): `nearMiss` in `site-coverage.ts`, the panel button and `openAdapterReview`, the capture-explanation near-miss tests, and the DESIGN.md near-miss text.

A and B touch disjoint files. C touches `site-coverage.ts` and `panel.ts` after A lands.
