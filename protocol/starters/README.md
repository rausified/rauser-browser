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
  "origin": { "kind": "subdomain", "suffix": ".atlassian.net" },
  // or: { "kind": "exact", "origin": "https://miro.com" }
  "scope": null,
  // or: { "kind": "github_org" }
  "types": [{
    "type": "jira.issue",
    "key": "jira:{origin}",
    "label": "Jira issues",
    "label_variants": [],          // e.g. [{"host_suffix": "-my.sharepoint.com", "label": "..."}]
    "description": "...",
    "adapter": { "type": "jira.issue", "match": [...], "id": "...", ... },
    "history": [],                 // earlier shipped { "match": [...] } entries, oldest first
    "status": { "state": "certified", "method": "live", "checked_on": "2026-11-02", "checked_by": "John" },
    "docs": "docs/adapters/jira.issue.md"
  }]
}
```

`adapter` is an adapter-template document (`adapter-templates.md` §3) with
`origin` omitted: the generator and, at runtime, `starterOffers` fill it in
for the site the starter is offered on. A scoped family (GitHub's `owner`)
writes the placeholder `{"$scope": true}` where the real scope value goes.

`status.state` is `"certified"` or `"development"`. A certified type's
`method` says how: `"live"` (a person ran the starter's checklist on a real
account; `checked_by` names them) or `"history"` (real browsing history
confirmed every rule and title; `checked_by` names the agent session). A
development type has not been certified yet and carries `method: null`,
`checked_on: null`, `checked_by: null`.

## Adding a type

1. Add a `types[]` entry to the family's JSON file (or a new family file, named `<family>.json`, for a tool with no existing entry) following the shape above.
2. Add the type's `adapter.id` template (and any `within.id`) to `ids.lock.json`; `generate-starters.mjs` fails closed with "no lock entry for `<type>`" until it is there.
3. Run `npm run gen:starters` and commit the regenerated `extension/starter-families.generated.ts` and `protocol/capture-site/catalog.json` alongside the source change.
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
npm run gen:starters          # write extension/starter-families.generated.ts
                               # and the starter_types line of
                               # protocol/capture-site/catalog.json
node scripts/generate-starters.mjs --check   # fail if either is out of date,
                                              # or a lock problem exists
```

The generator also validates each family file's shape (allowed keys,
required keys) and reports problems as `<file>: <path> <message>`, for
example `protocol/starters/atlassian.json: types[0].adapter.id is required`.
