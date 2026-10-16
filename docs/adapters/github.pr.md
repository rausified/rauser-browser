# GitHub pull request starter draft

Status: development draft. The adapter grammar and recorded examples were
verified by both adapter-template engines on 2026-10-16. A live GitHub UI check
is still required before this draft is release-certified.

GitHub is a shared origin, so the settings gallery refuses to create this
draft until the user names one or more work organizations. Names are normalized
to lowercase, deduplicated, and placed in `scope.owner`; there is no
capture-every-account option in the starter flow.

The draft maps pull request overview, files, commits, and checks routes to
`github.pr:github.com/<owner>/<repo>/<number>`. Files are recorded as review
mode; commits and checks are view mode. Its title template covers the recorded
`Title by Author · Pull Request #N · owner/repo · GitHub` form, and falls back
to the full tab title when it differs. The extracted title may also expose
uppercase Jira-key mentions to the later graph index.

Known limits: personal repositories are not captured unless their owner is
explicitly entered as a scope, enterprise GitHub on a private origin is not
auto-suggested, and sub-routes outside the listed modes remain unmatched.
Organization and repository rename handling waits for the planned alias flow.
