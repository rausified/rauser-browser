# GitHub repository starter draft

Status: development draft. The adapter grammar and the sample URLs below were
verified by both adapter-template engines on 2026-10-27 through the shared
vectors (`eval.github.repo_*` and the other `starters` GitHub cases in
`protocol/adapter-templates/vectors.json`). A live GitHub UI check is still
required before this draft is release-certified; record it here when done.

GitHub is a shared origin, so this draft is created only for organizations the
user names. Names are normalized to lowercase, deduplicated, and placed in
`scope.owner`; there is no capture-every-account option in the starter flow.

The draft has one rule, `/{owner:name|lower}/{repo:name|lower}/{*}`, and maps
every page inside a repository to `github.repo:github.com/<owner>/<repo>`.
That ID is frozen once shipped (adapter-templates.md §8.2). The rule has no
fixed word: it is valid only because `owner` is a scoped path capture, which
anchors the template (adapter-templates.md §4.1, §6). An `unscoped` variant of
this rule has no anchor and does not validate.

The adapter has no `title` templates and no `refs`. The host derives a
repository's title from its ID (`owner/repo`).

## Containers: `within`

The `github.pr` and `github.issue` starters declare
`within = { type = "github.repo", id = "github.repo:{host}/{owner}/{repo}" }`,
the same ID this adapter builds. A visit to a pull request or issue counts
once toward that artifact and once toward its repository, never twice toward
the repository (adapter-templates.md §8.3, DESIGN.md §5).

First match wins on an origin, so the pull request and issue adapters must
come before this catch-all, or it would claim their pages. Settings and
Capture Site keep that order on every save (`orderContainedFirst` in
`extension/starter-adapters.ts`).

## Sample URLs

With `scope.owner = ["acme-corp"]` and the PR and issue starters ahead of it:

| URL | Result |
|---|---|
| `https://github.com/acme-corp/widgets` | `github.repo:github.com/acme-corp/widgets` |
| `https://github.com/acme-corp/widgets/blob/feature/x/README.md` | the same repository |
| `https://github.com/acme-corp/widgets/tree/0123456789abcdef0123456789abcdef01234567` | the same repository |
| `https://github.com/acme-corp/widgets/pulls` | the same repository |
| `https://github.com/acme-corp/widgets/pull/7/files` | `github.pr:github.com/acme-corp/widgets/7` (review mode), within the repository |
| `https://github.com/acme-corp/widgets/issues/3` | `github.issue:github.com/acme-corp/widgets/3`, within the repository |
| `https://github.com/acme-corp` | no match (no repository segment) |
| `https://github.com/other/x` | out of scope |
| `https://github.com/settings/profile` | out of scope (`settings` is not a listed organization) |

## Known limits

- A scope miss is terminal (adapter-templates.md §6). A pull request outside
  the PR adapter's scope but inside this adapter's scope is dropped, not
  counted toward its repository. The starters use one scope for all three
  adapters, so this only arises after editing one scope by hand.
- Files, branches, commits, and other repository pages are not separate
  artifacts. They count toward the repository only.
- Organization and repository renames create a new ID. There are no rename
  aliases yet; they wait for the planned alias flow.
- Personal repositories are captured only when their owner is entered as a
  scope, and enterprise GitHub on a private origin is not auto-suggested.
- Because this catch-all overlaps every GitHub path with two or more segments,
  settings reports its overlap with the PR and issue rules (they win), and a
  Tier 2 draft for another GitHub route is dropped as overlapping.
