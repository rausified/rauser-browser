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
  "scope": null,
  // or one of the three scope kinds below
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
    "samples_shape": true          // false (and samples: []) when the type has no URL shape
                                    // worth a shapes.json-style row (today: github.repo)
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

## Adding a type

1. Add a `types[]` entry to the family's JSON file (or a new family file, named `<family>.json`, for a tool with no existing entry) following the shape above.
2. Add the type's `adapter.id` template (and any `within.id`) to `ids.lock.json`; `generate-starters.mjs` fails closed with "no lock entry for `<type>`" until it is there.
3. Run `npm run gen:starters` and commit the regenerated `extension/starter-families.generated.ts`, `protocol/capture-site/catalog.json` and (when the type's `samples_shape` is true) `protocol/capture-site/starter-shapes.json` alongside the source change.
4. Add the type's doc page, `docs/adapters/<type>.md` (`adapter-templates.md` §11.1), and its shared test vectors.

## Frozen IDs

`ids.lock.json` maps each type's `id` template (and `<container>#within` for
a container reference such as `github.repo#within`) to the string shipped
today. It is append-only: adding a type adds an entry; nothing already
listed may change, because a saved adapter's `id` is how a user's artifact
stays matched to its starter (`extension/starter-adapters.ts` `matchingStarter`).
`scripts/generate-starters.mjs --check` fails with `id template changed for
<type>` if a type's `adapter.id` (or a container's `within.id`) no longer
matches its lock entry.

## Generating

```sh
npm run gen:starters          # write extension/starter-families.generated.ts,
                               # the starter_types line of
                               # protocol/capture-site/catalog.json, and
                               # protocol/capture-site/starter-shapes.json
node scripts/generate-starters.mjs --check   # fail if any is out of date,
                                              # or a lock problem exists
```

The generator also validates each family file's shape (allowed keys,
required keys) and reports problems as `<file>: <path> <message>`, for
example `protocol/starters/atlassian.json: types[0].adapter.id is required`.
