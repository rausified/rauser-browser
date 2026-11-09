# Starter families

One JSON file per starter family (`atlassian.json`, `github.json`, `miro.json`,
`sharepoint.json`, ...). Each holds the starter types offered for that
family's origin: a built-in draft adapter, its label and description, its
earlier shipped rule sets (`history`), and its certification `status`. See
`docs/superpowers/specs/2026-11-03-starter-families-design.md` §2 for the
design background; this file and `scripts/generate-starters.mjs` are the
source of truth for the current shape.

## File shape

```jsonc
{
  "family": "atlassian",
  "display_name": "Atlassian Cloud (Jira or Confluence)",  // the family's name in the UI
  "review_prompt": "Choose the kinds of work to remember on this Atlassian site.",
  // or null, only when the family has exactly one type and no scope (today: sharepoint, miro)
  "origin": { "kind": "subdomain", "suffix": ".atlassian.net" },
  // or: { "kind": "exact", "origin": "https://miro.com" }
  // a subdomain origin can be strict: { "kind": "subdomain", "suffix": ".strict.test",
  // "labels": 1, "exclude": ["www", "signin"] } claims exactly one label before the
  // suffix (not "a.b.strict.test") and skips the named product hosts (not
  // "www.strict.test" or "signin.strict.test"); both keys are optional and only
  // allowed on a subdomain origin. An exclude entry ending in one "*" is a
  // first-label prefix: "fupload-*" skips "fupload-12.strict.test" (but not
  // "fupload.strict.test"). The prefix must be non-empty and the "*" may only
  // come last ("*", "*wss" and "f*load" are refused). Atlassian and SharePoint
  // stay permissive (any prefix, any number of labels) — strict matching is
  // opt-in per family.
  // or, for a product served on several hosts, a multi-origin family:
  // { "kind": "any", "of": [{ "kind": "exact", "origin": "https://app.box.com" },
  //   { "kind": "subdomain", "suffix": ".app.box.com", "labels": 1 }, ...] }
  // claims an origin when any member does. `of` lists two or more exact or
  // subdomain origins (no nested "any"), none overlapping another.
  "scope": null,
  // or one of the three scope kinds below
  "sample_origin": "https://acme.atlassian.net",  // must sit on the family's own origin
  "sample_scope": [],              // e.g. ["acme"]; non-empty exactly when scope is
                                    // "github_org" or "tenant_name"
  "overlaps": [{ "a": "jira.issue", "b": "confluence.page", "expect": "disjoint" }],
  // every pair of types that is not one the other's `within` container needs an entry
  "types": [{
    "type": "jira.issue",
    "key": "jira:{origin}",
    "label": "Jira issues",
    "label_variants": [],          // e.g. [{"host_suffix": "-my.sharepoint.com", "label": "..."}]
    "description": "...",
    "record_label": null,          // e.g. "pull requests"; the noun a "Remember: …" summary uses
    "example_path": null,          // e.g. "/{scope}/example-repository/pull/123"; must start with "/"
    "adapter": { "type": "jira.issue", "match": [...], "id": "...", ... },
    "history": [],                 // earlier shipped { "match": [...] } entries, oldest first
    "status": { "state": "certified", "method": "live", "checked_on": "2026-11-02", "checked_by": "John" },
    "docs": "docs/adapters/jira.issue.md",
    "samples": [{
      "note": "Jira canonical issue",            // becomes "<note> (<docs>)" in starter-shapes.json
      "url": "https://acme.atlassian.net/browse/PROJ-42",
      "title": "[PROJ-1] Fix the bug - Jira",     // the recorded page's raw <title>, or null
      "shape": "/browse/{key}"
    }],
    "samples_shape": true,         // false when the type has no URL shape worth a
                                    // shapes.json-style row (today: github.repo) or when
                                    // the structural drafter cannot learn it; samples may
                                    // still be non-empty (a titled sample is still needed
                                    // for a type with an "adapter.title" template)
    "vectors": []                  // vectors.json cases for this type, each missing "doc"
  }]
}
```

## Recorded samples

Each type's `samples` are real recorded URL forms of that type, one per shape
worth distinguishing (the SharePoint "sharing form" vs. "site document"
routes, for example). `scripts/generate-starters.mjs` writes every sample
whose type has `samples_shape: true` into the generated
`protocol/capture-site/starter-shapes.json`, in the same `{note, url, shape}`
row shape `protocol/capture-site/shapes.json` uses, so the host's and the
in-page shaper's tests can pin both files the same way. A sample's `url`
must sit on the family's own `origin` (`familyClaims`); the generator
refuses one that doesn't, file and field named. `title` is the page's raw
`<title>` at the time it was recorded, or `null` when the sample has none;
every type whose adapter has a `title` template needs at least one sample
with a non-null `title`.

`samples_shape` controls only whether a type's samples are carried into
`starter-shapes.json`, not whether the type may have samples. `false` means
either the type's URL shape isn't worth a shapes.json-style row (today:
`github.repo`) or the structural drafter cannot learn the shape at all (a
query-routed type such as a `?sys_id=` form). Either way, `samples` may
still be non-empty: those samples are read only by the registry's title
check (a type with an `adapter.title` template still needs a sample with a
non-null `title`, whether or not `samples_shape` is true). Only
`samples_shape: true` requires a non-empty `samples`.

A sample's `shape` must equal the in-page shaper's own output for its
`url` (`site-patterns.test.mjs` and `shapes.rs` both pin this), so never
hand-write one: after `npm run build:extension`, compute it from the built
extension's own shaper —

```sh
node -e 'import("./extension/dist/site-patterns.js").then(({ inspectSitePatterns }) => console.log(inspectSitePatterns("<url>").current))'
```

— and copy its output into the sample.

`adapter` is an adapter-template document (`adapter-templates.md` §3) with
`origin` omitted: the generator and, at runtime, `starterOffers` fill it in
for the site the starter is offered on. A scoped family writes the
placeholder `{"$scope": true}` where the real scope value goes (GitHub's
`owner`).

`status.state` is `"certified"` or `"development"`. A certified type's
`method` says how: `"live"` (a person ran the starter's checklist on a real
account; `checked_by` names them) or `"history"` (real browsing history
confirmed every rule and title; `checked_by` names the agent session). A
development type has not been certified yet and carries `method: null`,
`checked_on: null`, `checked_by: null`.

## Test vectors and overlaps

`sample_origin` and `sample_scope` are the values `scripts/generate-starters.mjs`
feeds into every type's `adapter` to build the family's `starter:<family>`
fixture in `protocol/adapter-templates/starter-vectors.json` (`synthesizeFixture`):
the same document `starterOffers(sample_origin, sample_scope)` builds for that
origin at runtime. `sample_origin` must sit on the family's own `origin`, the
same rule a recorded sample's `url` follows, and `sample_scope` is non-empty
exactly when `scope` needs account values (`github_org` or `tenant_name`).

`overlaps` declares, for every pair of the family's types that is not one the
other's `within` container, whether their rules may overlap. The generator
refuses a missing pair, naming both types, and refuses an `a`/`b` that does
not name a type in the family or a pair declared twice. `expect` is one of:

- `"disjoint"`: no URL matches both types' rules.
- `"container_first"`: `b` declares `within` `a` with `a`'s own ID template,
  `a` is listed before `b` in `types`, and `pairs` lists every
  `[<a rule>, <b rule>]` that may overlap. Each listed pair is intended, and
  `a` wins it: the container has pages of its own that `b`'s rule would
  otherwise claim. Splunk is the case: `/<locale>/app/<app>/search` is the
  app's Search page, but `splunk.dashboard`'s `/{_}/app/{app}/{view:name}`
  matches it too, and format 1 has no negation.

The declared pairs become the family's single `overlap_report.starter.<family>`
case: `{"pairs": []}` for a family with only disjoint entries, otherwise
exactly the container-first pairs, container first, in the engine's order.
Any other rule pair that may overlap fails the engine vectors, so an
undeclared overlap is refused when the family is built. For each
container-first entry the generator also writes a `starter:<family>/<b>`
fixture with `b` alone (no `within`, as the runtime saves it when the
container is not chosen) and re-runs each of `b`'s matching `evaluate`
vectors against it as `<id>.alone`: `b`'s own pages stay `b`'s.

At runtime the declaration is read from the bundled family, keyed on the
family's own origins (an adapter that reuses the type name elsewhere is
unaffected):

- `orderContainedFirst` (`extension/starter-adapters.ts`) puts `a` ahead of
  `b` on that origin on every save, whether or not `b` still carries its
  `within`, so `a` wins in both engines (first match wins,
  `adapter-templates.md` §3). Settings refuses a move that would reverse
  them. This holds only while `a`'s rules named in `pairs` are exactly as
  shipped: once the user edits one (widening it to a catch-all, say), the
  ordinary contained-first order applies, so `b`'s own pages stay `b`'s.
- Settings leaves the host's warning for a declared pair out of the overlap
  list, but only while both adapters are exactly as the starter ships them
  (every `match` rule and `path_case`) and `a` comes first. One predicate,
  `isContainerFirstPair`, decides both this and the order above. An edited
  rule on either adapter, a pair not in `pairs`, or the pair on another
  origin is warned about and ordered as before.
- Restoring recorded rules (`site_restore`) still refuses any overlap, a
  declared pair included, because recorded contracts carry no family. On a
  family that declares `"container_first"`, Capture Site treats that one
  refusal as "offer the starter instead": it says why, and offers the
  shipped starter. The host checks the starter against every recorded
  contract before saving and again on save, so a recorded identity the
  starter would change is still refused. Any other restore refusal still
  blocks the starter. Re-adding a removed container and its contained type
  in one save is allowed, because the host treats a recorded contract that
  the save puts back exactly as live again, not retired.
- Nothing else changes: user-written adapters keep the ordinary §10.3
  warning, and agent drafts still drop an overlapping rule.
- The host never lets `b` be live without `a` on its origin. `gen:starters`
  writes every declared pair, with both types' labels, into the
  `container_first` line of `protocol/capture-site/catalog.json`. That line
  is generated only, and a host that finds it malformed refuses to start.
  Every save (settings, Capture Site and its preflight, and saving restored
  rules) is refused when it would leave a `b` adapter with no `a` adapter
  on the same origin: `b` alone, or `a` removed under `b`. The refusal reads
  "Splunk dashboards need Splunk apps saved with them on <origin>; add
  Splunk apps, or remove Splunk dashboards too." The order stays the
  extension's (`orderContainedFirst` above), so an edited pair may save
  contained first; edited rules are the user's own, and such a pair can
  re-key as any user-written container can. A config that already has the
  gap is not blocked from unrelated saves. This is a check on the type names, so it applies on any
  origin, as starter type names are reserved.

  Why: alone, `b`'s rule records `a`'s own pages (Splunk's built-in views)
  under `b`'s identity. Adding `a` later would then re-key those records
  without a migration, live or retired. The host's history guard lets a
  container take a retired contained contract's routes (`declares_container`,
  as a repository takes a removed pull request's pages), so the guarantee
  has to come from `b` never holding them. Recording a per-contract
  "was alone" flag would need an adopted-contracts format change, and the
  record's order cannot tell, because it keeps each contract once. A family
  without a `"container_first"` entry is unaffected: a pull request rule may
  still stand alone.

Why a declaration in the family file, not in the format: the overlap is a
property of how one vendor lays out its URLs, reviewed when the family is
reviewed. Putting it in the format (an adapter-level `precedence` key, say)
would let any saved or drafted adapter claim a winner, which is what the
overlap warning exists to stop.

Each type's own `vectors` are `protocol/adapter-templates/vectors.json`
(`adapter-templates.md` §12) cases for that type, minus `doc`: the generator
adds `doc: {"$fixture": "starter:<family>"}` itself, so a vector that already
carries one is refused. A vector naming a `url` must sit on the family's own
origin, and its `id` must not already exist in `vectors.json` or another
family's vectors — the generator names the file and the duplicate id either
way.

## Scope kinds

`scope` is `null` (the family has no account-scoped values at all, e.g.
SharePoint, Miro, Atlassian) or one of three kinds:

- `{ "kind": "github_org" }` — today's GitHub organization scope. No extra
  fields; the UI's organization field copy is built in
  `extension/starter-scopes.ts`.
- `{ "kind": "tenant_name", "max": 4, "label": "Workspaces", "placeholder": "acme" }`
  — a typed, comma-separated list of 1 to `max` (1-32) shared-origin tenant
  names (workspaces, accounts). `label` and `placeholder` drive the field's
  UI copy.
- `{ "kind": "unscoped", "notice": "Records every … from every account in
  this Chrome profile." }` — the family has no scope values; every account
  signed in to the browser profile is recorded, and `notice` is shown
  instead of a scope field or acknowledgement checkbox. The engine only
  allows `unscoped` on one of its hard-coded shared origins
  (`extension/adapter-templates.ts` `SHARED_ORIGINS`), so a new family using
  this kind must sit on one of those origins, never a subdomain or any other
  host.

A scoped family (`github_org` or `tenant_name`) must mark every type's
adapter with a `{"$scope": true}` placeholder somewhere in its `scope`
table; an unscoped family (`scope: null` or `{"kind": "unscoped"}`) must
have neither that marker nor `{scope}` in an `example_path`. A `"kind":
"unscoped"` family's adapters must also each carry `"unscoped":
"acknowledged"`, the config-level marker the engine requires for shared
origins. `scripts/generate-starters.mjs` enforces all of this, file and
field named.

## Registry and policy checks

`scripts/generate-starters.mjs`'s validation (plain generation and
`--check`) also runs the pure registry checks of
`docs/superpowers/specs/2026-11-03-starter-families-design.md` §4, in the
same pass as the family-file shape validation above: a family file's own
validation problems are still reported, and every other file, and these
checks, still run. A file is left out of the pass only when it is not valid
JSON, or its shape is too broken to read further (not even a plain object
with a `types` array). A file whose shape validates that far still gets its
policy and registry checks even if it has an unrelated validation problem
elsewhere (for example an unknown top-level field): each check reads only
the specific field it needs — the origin-collision and shared-origin
checks below read a family's `origin` and `scope` once each has validated
cleanly on its own, regardless of what else in the family might be wrong —
so a real cross-file collision is never hidden behind an unrelated typo.

**Shared-origin policy** (a family's `origin` against
`protocol/capture-site/catalog.json`'s `shared_origins`):

- **Origin collisions.** Two families' origins may not overlap. Two
  subdomain families on the same suffix always collide, whatever `labels`
  or `exclude` say. A suffix nested inside another (`.ideas.aha.io` inside
  `.aha.io`) collides unless the shallower family has `labels: 1`: every
  host the deeper family claims has at least two labels before the
  shallower suffix, which `labels: 1` refuses, so the two are provably
  disjoint (`aha` and `aha-ideas` rely on this). `exclude` never proves it,
  and `labels: 1` on only the deeper family proves nothing. An exact origin
  collides with a suffix family only when that family would claim it
  (`familyClaims`); a repeated exact origin always collides. A multi-origin
  family collides when any one of its members does. The problem is
  reported on whichever file sorts later:
  `<file>: origin overlaps <other file>`.
- **A shared origin always needs account values.** Any `shared_origins` entry
  this family's `origin` claims — its own exact origin, or, for a subdomain
  family, any shared origin its suffix covers (`.google.com` covers
  `docs.google.com`) — may not have `scope: null`
  (`<file>: origin <o> is a shared origin, so scope must not be null`).
- **`unscoped` needs an exact origin `shared_origins` lists.** The engine
  (`extension/adapter-templates.ts` `SHARED_ORIGINS`) only allows the
  `unscoped` marker on one of its hard-coded shared origins
  (`<file>: scope "unscoped" needs an exact origin in
  protocol/capture-site/catalog.json shared_origins`).
- **`github_org` is only for GitHub itself**
  (`<file>: scope "github_org" is only for https://github.com`).
- **`tenant_name` must use the host's own tenant capture.** This check also
  runs against every shared origin a subdomain family's suffix covers, not
  only its own exact origin. A shared origin whose `tenant` is `null`
  (today: `docs.google.com`, `www.figma.com`) names no tenant at all, so no
  family there may use `tenant_name`
  (`<file>: scope "tenant_name" cannot be used on <o>, whose URLs name no
  tenant`). Otherwise every type's adapter `scope` table must use exactly
  the origin's `tenant.capture` as its one key — the same capture
  `host/src/capture_site.rs`'s `taken_types` reads
  (`<file>: types[N].adapter.scope must use "<capture>", the host's tenant
  capture for <o>`).

**Registry cross-checks** (spec §4 check 2's reverse direction, and check 3):

- Every `docs/adapters/*.md` file (other than its own `README.md`) must be
  some type's own `docs` (`docs/adapters/<x>.md: no family type`).
- A `protocol/capture-site/shapes.json` row whose note names a starter doc
  belongs in a family file's `samples` instead
  (`protocol/capture-site/shapes.json: row "<note>" names a starter doc; it
  belongs in a family file's samples`).
- A `protocol/capture-site/starter-shapes.json` row must still name a real
  family type (`protocol/capture-site/starter-shapes.json: row "<note>"
  names no family type`); `--check`'s own byte comparison already flags a
  hand edit there, but this message names the row.
- Every type's `docs` file must exist
  (`<file>: types[N].docs docs/adapters/<type>.md is missing`), and its
  first `Status:` line must open with the text its `status` implies, one of
  three exact openings: `Status: release-certified <checked_on>.` (certified, live
  method), `Status: certified <checked_on> from real browsing history.`
  (certified, history method), or `Status: development draft.` (not yet
  certified) — otherwise `docs/adapters/<type>.md: first Status line does
  not match status (expected "<opening>")`.
- None of the doc's six `## `-headed sections (`npm run new:starter`'s
  placeholders, "Adding a type" below) may still be exactly `TODO`
  (`docs/adapters/<type>.md: section "<heading>" is still a placeholder`).

Two checks need the built engine instead of the generator's own JSON
validation, so they live in `extension/tests/starter-registry.test.mjs`:
spec §4 check 4 (every match rule has a positive evaluate vector, every
*type* has at least one negative evaluate vector — a sibling type's
positive on a shared origin does not count as this type's negative — and a
sample whose title the title template accepts) and check 8 (`node
scripts/generate-starters.mjs --check` is clean).

## Adding a type

`npm run new:starter -- <family> <type> [--exact <origin> | --subdomain <suffix>] [--scope <kind>]`
(`scripts/new-starter.mjs`) scaffolds this for you: it writes the family
file (a new one, or a new `types[]` entry on an existing family) and
`docs/adapters/<type>.md`, each with `"TODO"` placeholders and `match: []`,
then runs the generator in `--check` mode and prints every problem under
"Fill these in, then run `npm run gen:starters`" — the record of what is
still missing, in one pass, so there is nothing else to separately track.
By hand, the steps it automates are:

1. Add a `types[]` entry to the family's JSON file (or a new family file, named `<family>.json`, for a tool with no existing entry) following the shape above.
2. Add the type's doc page, `docs/adapters/<type>.md` (`adapter-templates.md` §11.1), and its test vectors to the type's `vectors` ("Test vectors and overlaps" above).
3. Run `npm run gen:starters`. Write mode appends an `ids.lock.json` entry for a type missing one, keyed to the `adapter.id` (and any `within.id`) the family file currently declares, and prints `appended lock entry <type> → <id>`; `--check` still fails closed with "no lock entry for `<type>`" until that has happened. An entry that already exists but no longer matches still fails either way ("id template changed for `<type>`").
4. Commit the regenerated `extension/starter-families.generated.ts`, `protocol/capture-site/catalog.json`, (when the type's `samples_shape` is true) `protocol/capture-site/starter-shapes.json`, `protocol/adapter-templates/starter-vectors.json`, `docs/adapters/README.md` and the `ids.lock.json` append, alongside the source change.

A type's `record_label` (the noun a "Remember: A and B" summary uses when a
site offers more than one starter) is worth getting right the first time:
`npm run new:starter` leaves it `"TODO"` rather than `null`, even though
`null` validates, so a fresh scaffold's "fill these in" list never lets it
pass unnoticed.

## Frozen IDs

`ids.lock.json` maps each type's `id` template (and `<container>#within` for
a container reference such as `github.repo#within`) to the string shipped
today. It is append-only: adding a type adds an entry; nothing already
listed may change, because a saved adapter's `id` is how a user's artifact
stays matched to its starter (`extension/starter-adapters.ts` `matchingStarter`).
`scripts/generate-starters.mjs --check` fails with `id template changed for
<type>` if a type's `adapter.id` (or a container's `within.id`) no longer
matches its lock entry. Before your PR merges, you may still delete your
own type's unmerged entry from `ids.lock.json` and run `npm run gen:starters`
again to freeze a corrected `adapter.id` (Open decision 1); once merged,
that entry is permanent like every other.

## Generating

```sh
npm run gen:starters          # write extension/starter-families.generated.ts,
                               # the starter_types line of
                               # protocol/capture-site/catalog.json,
                               # protocol/capture-site/starter-shapes.json,
                               # protocol/adapter-templates/starter-vectors.json,
                               # docs/adapters/README.md (the status index),
                               # and append any missing ids.lock.json entry
node scripts/generate-starters.mjs --check   # fail if any is out of date,
                                              # or a lock problem exists
```

The generator also validates each family file's shape (allowed keys,
required keys) and reports problems as `<file>: <path> <message>`, for
example `protocol/starters/atlassian.json: types[0].adapter.id is required`.
