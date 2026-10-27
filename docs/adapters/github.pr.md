# GitHub pull request starter draft

Status: development draft. The adapter grammar and recorded examples were
verified by both adapter-template engines on 2026-10-16. A live GitHub UI check
is still required before this draft is release-certified.

GitHub is a shared origin, so the settings gallery refuses to create this
draft until the user names one or more work organizations. Names are normalized
to lowercase, deduplicated, and placed in `scope.owner`; there is no
capture-every-account option in the starter flow.

The draft maps the pull request overview and its sub-routes to
`github.pr:github.com/<owner>/<repo>/<number>`. Files are recorded as review
mode; commits and checks are view mode; other sub-routes remain part of the
same artifact without a mode. Its title template covers the recorded
`Title by Author · Pull Request #N · owner/repo · GitHub` form, and falls back
to the full tab title when it differs. The extracted title may also expose
uppercase Jira-key mentions to the later graph index.

The draft declares `within = { type = "github.repo", id =
"github.repo:{host}/{owner}/{repo}" }`, so each pull request sits within its
repository (adapter-templates.md §8.3). A match names the container, for
example `github.repo:github.com/acme-corp/widgets` for
`https://github.com/acme-corp/widgets/pull/7/files`, and a visit counts once
toward the pull request and once toward the repository. The container ID is frozen
with the pull request ID. The `github.repo` draft (`github.repo.md`) builds the same
ID for the repository's own pages and must come after this one on the origin;
settings keeps that order.

Known limits: personal repositories are not captured unless their owner is
explicitly entered as a scope, enterprise GitHub on a private origin is not
auto-suggested, and organization and repository rename handling waits for the
planned alias flow.
