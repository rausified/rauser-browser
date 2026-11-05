# figma.branch starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/figma/`, researched blind: public sources only, no
browsing history, cookies or a Figma account. Both adapter-template engines
verify the samples below. A live check with a real Figma account is still
needed before this draft is release-certified.

Brauser offers this draft on the exact origin `https://www.figma.com`,
alongside Figma files (the `figma` family). Either can be chosen alone in
Capture Site and in the settings gallery. The draft records one artifact per
branch, however it was opened:

`figma.branch:www.figma.com/<branch key>`

A branch (an Organization/Enterprise feature) is its own artifact, separate
from the main file it was branched from. Figma's REST API accepts a branch
key wherever a file key is accepted, and Figma's own desktop router and its
MCP/Code Connect guidance both treat the branch key as the file key for a
branch URL, so the branch key alone — not the main file's key — identifies
it, `token(22,128)`, same alphabet as `figma.file`.

```toml
[[artifacts]]
type     = "figma.branch"
origin   = "https://www.figma.com"
unscoped = "acknowledged"
match    = [
  { path = "/(file|design|board|slides|site|buzz|make|rev|spec|weave)/{_}/branch/{branch:token(22,128)}" },
  { path = "/(file|design|board|slides|site|buzz|make|rev|spec|weave)/{_}/branch/{branch:token(22,128)}/{_}" },
  { path = "/(proto|deck|presenter)/{_}/branch/{branch:token(22,128)}", mode = "view" },
  { path = "/(proto|deck|presenter)/{_}/branch/{branch:token(22,128)}/{_}", mode = "view" },
]
id       = "figma.branch:{host}/{branch}"
title    = ["{~} · {title} - Figma Slides", "{title} – {~}", "{title} - {~}"]
```

## What each rule covers

1. **Editor route, no slug**, `/<route>/<main key>/branch/<branch key>`. The
   main file's key is matched with `{_}` and never recorded: it adds nothing
   to the ID (branch keys are globally unique on their own), and this family
   does not roll a branch up into its main file's artifact (see "What the
   draft does not record").
2. **Editor route with a slug**, `/<route>/<main key>/branch/<branch
   key>/<slug>`. The slug Figma writes here is the *main file's* name, not
   the branch's, so it is never used.
3. **Viewer route, no slug**, `/(proto|deck|presenter)/<main
   key>/branch/<branch key>`, `mode = "view"`.
4. **Viewer route with a slug**, `/(proto|deck|presenter)/<main
   key>/branch/<branch key>/<slug>`, `mode = "view"`.

Every route word `figma.file` matches is matched here too: a branch exists
under every editor and viewer route Figma's router builds branch links for.
Query parameters and fragments never change the ID, same as `figma.file`.

## Titles

The same three entries as `figma.file` (see that page's "Titles" for the
full explanation): `{~} · {title} - Figma Slides` for a Slides branch viewed
as audience or presenter, `{title} – {~}` for every other editor product's
en-dash suffix, and `{title} - {~}` for a branch's prototype tab, which
reads `<name> - <page>` with no `▶` mark (coordinator ruling). What `<name>`
is on a branch — the branch's own name or the main file's name — is not
confirmed; this draft records whichever tab title Figma actually sends,
whole, because the template does not depend on knowing which.

## Recorded samples

A synthetic equivalent of the recorded shape, on `www.figma.com`:

| URL | Tab title | Records |
|---|---|---|
| `/design/<main key>/branch/<26-char branch key>/Checkout-Flow` | `Checkout Flow – Figma` | that branch key, no mode, title `Checkout Flow` |

9 of the blind research's 52 synthetic vectors are `figma.branch` positives,
all correctly classified by the research's own draft; the 3 prototype cases
among them get the same title fix as `figma.file`'s (see that page's
"Recorded samples").

`samples_shape` is `false`: this type's one sample shares `figma.file`'s own
`/design/<key>/...` route word, differing only by the literal `branch`
segment inserted after the key. `cargo test --locked -p adapter-templates
--test structural starter_recorded_urls_draft_to_matching_adapters_one_per_kind`
names this sample, because the generic structural drafter buckets every
`www.figma.com` sample that starts with the same route word into one
candidate rule and, given only one example of each shape, generalizes to the
more specific (branch) form, which then no longer matches `figma.file`'s
plain design-file sample. Dropping this sample from the shared shapes pool
(by setting `samples_shape` to `false`) removes the ambiguity without
changing what the real adapter matches; the `match` rules above, not the
drafter, are what Brauser actually ships.

## What the draft does not record

- a branch is never rolled up into its main file's artifact: doing so would
  require the host to name every `figma.file` row from a branch's ID
  (adapter-templates.md §8.3), which would need a link back to
  `figma.file:www.figma.com/<key>` with no key known, so this draft keeps
  the branch as its own, separate artifact instead;
- a short branch key (fewer than 22 characters), the same way `figma.file`
  excludes a short file key;
- anything `figma.file`'s own "What the draft does not record" already
  excludes before the `/branch/` segment is even reached (the file-browser
  lists, create routes, the embed origin, and so on).

## Known limits

- **Branch titles are unverified.** The research could not confirm, from
  public sources alone, whether a branch's tab title uses the branch's own
  name or the main file's name (research.md "Known limits and open
  questions" item 9).
- **Title is set client-side**, the same `Figma`-while-loading limit as
  `figma.file`.
- The ID template is frozen once shipped (adapter-templates.md §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Figma account, on an Organization or Enterprise
plan:

1. Create a branch of a Design file; record its URL (main key, branch key,
   slug) and its tab title, and whether the branch key in the URL matches
   the API's `branches[].key`.
2. Confirm a branch's prototype tab reads `<file> - <page>` and records
   `<file>`, the same correction as `figma.file`'s step 3.
3. Rename the branch and the main file separately; check which name the
   branch's tab title actually uses, resolving research's open question.
4. Open the branch through the legacy `/file/<key>/branch/<branch key>/...`
   route and confirm it lands on the same branch key.
5. Open the branch in FigJam and in Slides (presenter and audience view) and
   confirm each still reads only the branch key, never the main file's.
6. On a Capture Site review on a Figma branch page with no saved rules,
   Brauser offers Figma files and Figma branches with the every-account
   statement and the development-draft source line; saving one turns the
   page card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `figma.branch`'s
`status` in `protocol/starters/figma.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
