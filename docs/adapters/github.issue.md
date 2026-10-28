# GitHub issue starter draft

Status: development draft. The adapter grammar and recorded examples were
verified by both adapter-template engines on 2026-10-16. A live GitHub UI check
is still required before this draft is release-certified.

GitHub is a shared origin, so the settings gallery refuses to create this
draft until the user names one or more work organizations. Names are normalized
to lowercase, deduplicated, and placed in `scope.owner`; there is no
capture-every-account option in the starter flow.

The draft maps only `/<owner>/<repo>/issues/<number>` to
`github.issue:github.com/<owner>/<repo>/<number>`. Its title template covers
the recorded `Title · Issue #N · owner/repo · GitHub` form, and falls back to
the full tab title when it differs. The extracted title may also expose
uppercase Jira-key mentions to the later graph index.

The draft declares `within = { type = "github.repo", id =
"github.repo:{host}/{owner}/{repo}" }`, so each issue sits within its
repository (adapter-templates.md §8.3). Capture Site and settings keep that
declaration only when a `github.repo` rule is kept on the origin; without one,
the saved adapter has no `within` and records no repositories. A match names the container, for
example `github.repo:github.com/acme-corp/widgets` for
`https://github.com/acme-corp/widgets/issues/3`, and a visit counts once
toward the issue and once toward the repository. The container ID is frozen
with the issue ID. The `github.repo` draft (`github.repo.md`) builds the same
ID for the repository's own pages and must come after this one on the origin;
settings keeps that order.

Known limits: personal repositories are not captured unless their owner is
explicitly entered as a scope, enterprise GitHub on a private origin is not
auto-suggested, and pull requests are deliberately handled by the separate
`github.pr` draft. Organization and repository rename handling waits for the
planned alias flow.
