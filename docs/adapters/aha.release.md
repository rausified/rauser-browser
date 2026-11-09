# aha.release starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/aha/`, researched blind: public sources only, no
browsing history, cookies or an Aha! account.

History check (2026-11-09): graded against real browsing history: no
real visits of this type, so 0/1 rules confirmed. Not every rule and
title was confirmed, so this type stays a development draft.

Brauser offers this draft on an Aha! account's tenant subdomain,
`<account>.aha.io` (the `aha` family's strict subdomain origin,
`exclude: ["www", "secure", "support", "status", "cdn", "ideas", "identity",
"iad-prod1"]`), alongside Aha! features, requirements, epics, ideas and
notes. Any combination can be chosen in Capture Site and in the settings
gallery. The draft records one artifact per release (or "schedule" in a
non-product workspace), however it was opened:

`aha.release:<account>.aha.io/<reference number>`

The ID is the release's reference number (for example `PRJ-R-3`), seen in
two separate public API fixtures.

```toml
[[artifacts]]
type   = "aha.release"
origin = "https://acme.aha.io"
match  = [
  { path = "/releases/{ref:name(5,100)|upper}" },
]
id     = "aha.release:{host}/{ref}"
title  = ["{ref:name} {title} | Aha!{~}", "{title} | Aha!{~}"]
refs   = [{ type = "aha.feature", find = "jira_key" }]
```

## What each rule covers

1. **Release record**, `/releases/<ref>`, confirmed by two independent
   public API fixtures and a logged-out route probe.

There is no Aha! Develop form for releases in the public sources, so this
type has a single rule, unlike `aha.feature`, `aha.requirement`, `aha.epic`
and `aha.idea`.

## Titles

Two entries, tried in order:

1. `{ref:name} {title} | Aha!{~}` — only when the leading word of the tab
   title equals the path's reference number.
2. `{title} | Aha!{~}` — otherwise.

Low confidence: no public source shows an in-app release page's tab title,
the same reasoning `aha.feature`'s page gives in full (Known limits,
below). `refs` recognizes a `jira_key`-shaped mention (such as a feature
reference named in a release's own title) as pointing at an `aha.feature`.

## Recorded samples

A synthetic equivalent of the recorded shape, on `acme.aha.io`:

| URL | Tab title | Records |
|---|---|---|
| `/releases/<ref>` | `PRJ-R-3 Winter launch \| Aha!` | that reference number, title `Winter launch` |

The blind research scored 90 synthetic vectors across this family's seven
drafted types; this PR carries over 10 `aha.release` cases, all correctly
classified by the research's own draft and re-verified against this
family's own fixture through both adapter-template engines. Graded later
against 2 real, held-back Aha! browsing visits
(`docs/research/blind/SCORECARD.md`), the research matched Aha!'s site-wide
result of 2 of 2 correct — not broken out per type, since neither of those 2
real visits happened to be a release.

## What the draft does not record

- `/products/<prefix>/releases/<ref>`: the route exists for any `<ref>`, so
  it is most likely a workspace list view rather than the release record
  itself — excluded as unverified;
- `/releases/<ref>/<anything>` (a 404 today; no rest segment carried);
- `/releases/new` and any other route word shorter than 5 characters.

## Known limits

- **Every title template is low confidence**, the same limit
  `aha.feature`'s page states in full.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Aha! account:

1. Capture Site: open a release from a roadmap or releases list; record the
   URL and the exact tab title (`/releases/<ref>`, and does it read `<ref>
   <name> | Aha!...`?).
2. Open `/products/<prefix>/releases/<ref>`; confirm it is a list view, not
   a single release record, and stays unmatched.
3. On a Capture Site review on a release page with no saved rules, confirm
   Brauser offers all six Aha! types — features, requirements, epics,
   releases, ideas and notes — each with its own Aha!-account statement and
   the development-draft source line; confirm saving one turns the page
   card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `aha.release`'s
`status` in `protocol/starters/aha.json` to `{ "state": "certified",
"method": "live", "checked_on": "<date>", "checked_by": "John" }`, and run
`npm run gen:starters` (`protocol/starters/README.md`). The Capture Site
source line follows the status.
