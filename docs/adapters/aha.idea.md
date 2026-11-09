# aha.idea starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/aha/`, researched blind: public sources only, no
browsing history, cookies or an Aha! account.

History check (2026-11-09): graded against a held-back sample of real
browsing history (real URLs from eight researched sites): no visits of
this type in the sample, so 0/2 rules confirmed. Not every rule and
title was confirmed, so this type stays a development draft.

Brauser offers this draft on an Aha! account's tenant subdomain,
`<account>.aha.io` (the `aha` family's strict subdomain origin,
`exclude: ["www", "secure", "support", "status", "cdn", "ideas", "identity",
"iad-prod1"]`), alongside Aha! features, requirements, epics, releases and
notes. Any combination can be chosen in Capture Site and in the settings
gallery. The draft records one artifact per idea opened inside the Aha!
application, however it was opened:

`aha.idea:<account>.aha.io/<reference number>`

The ID is the idea's reference number (for example `PRJ-I-45`).

```toml
[[artifacts]]
type   = "aha.idea"
origin = "https://acme.aha.io"
match  = [
  { path = "/ideas/ideas/{ref:name(5,100)|upper}" },
  { path = "/develop/ideas/ideas/{ref:name(5,100)|upper}" },
]
id     = "aha.idea:{host}/{ref}"
title  = ["{ref:name} {title} | Aha!{~}", "{title} | Aha!{~}"]
refs   = [{ type = "aha.feature", find = "jira_key" }]
```

## What each rule covers

1. **In-app idea record**, `/ideas/ideas/<ref>` (route confirmed to exist
   logged-out; reference format from the public API docs and a portal
   notification-email variable table).
2. **Aha! Develop form**, `/develop/ideas/ideas/<ref>` (route confirmed to
   exist logged-out; undocumented).

The doubled `ideas/ideas` path is deliberate: the single-level
`/ideas/<word>` answers for any word at all — it is the Ideas module's own
navigation (`/ideas/overview`, `/ideas/portals`, and so on) — so only the
two-level form is matched.

## Titles

Two entries, tried in order:

1. `{ref:name} {title} | Aha!{~}` — only when the leading word of the tab
   title equals the path's reference number.
2. `{title} | Aha!{~}` — otherwise.

Low confidence: no public source shows an in-app idea page's tab title, the
same reasoning `aha.feature`'s page gives in full (Known limits, below).
This is distinct from the *portal* idea title, which public sources do
show in full (Known limits, below).

## Recorded samples

A synthetic equivalent of the recorded shape, on `acme.aha.io`:

| URL | Tab title | Records |
|---|---|---|
| `/ideas/ideas/<ref>` | `PRJ-I-45 Dark mode \| Aha! Ideas` | that reference number, title `Dark mode` |

The blind research scored 90 synthetic vectors across this family's seven
drafted types; this PR carries over 10 `aha.idea` cases, all correctly
classified by the research's own draft and re-verified against this
family's own fixture through both adapter-template engines. Graded later
against 2 real, held-back Aha! browsing visits
(`docs/research/blind/SCORECARD.md`), the research matched Aha!'s site-wide
result of 2 of 2 correct — not broken out per type, since neither of those 2
real visits happened to be an in-app idea.

## What the draft does not record

- `/ideas/overview`, `/ideas/portals` and any other single-level
  `/ideas/<word>` page, the Ideas module's own navigation, not a record;
- a route word shorter than 5 characters directly under `/ideas/ideas/` or
  `/develop/ideas/ideas/`.

## Known limits

- **Portal ideas are a separate type.** Aha!'s ideas portals live on a
  second tenant subdomain, `https://<portal>.ideas.aha.io`, covered by the
  `aha-ideas` family's `aha.portalidea` (`docs/adapters/aha.portalidea.md`).
  The same idea gets a *different* ID there than it does in-app — the portal
  page is a separate audience's view of the same idea, not a duplicate to be
  merged.
- **Every in-app title template is low confidence**, the same limit
  `aha.feature`'s page states in full.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Aha! account:

1. Capture Site: open an idea from the in-app Ideas module; record the URL
   and the exact tab title (`/ideas/ideas/<ref>`, and does it read `<ref>
   <name> | Aha!...`?).
2. Open the same idea from Aha! Develop; confirm it is the same record and
   record its tab title.
3. Open `/ideas/overview` and `/ideas/portals`; confirm neither is offered
   as an artifact.
4. On a Capture Site review on an idea page with no saved rules, confirm
   Brauser offers all six Aha! types — features, requirements, epics,
   releases, ideas and notes — each with its own Aha!-account statement and
   the development-draft source line; confirm saving one turns the page
   card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `aha.idea`'s `status`
in `protocol/starters/aha.json` to `{ "state": "certified", "method":
"live", "checked_on": "<date>", "checked_by": "John" }`, and run `npm run
gen:starters` (`protocol/starters/README.md`). The Capture Site source line
follows the status.
