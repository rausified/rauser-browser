# aha.note starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/aha/`, researched blind: public sources only, no
browsing history, cookies or an Aha! account.

History check (2026-11-09): graded against the maintainer's full Chrome
history on this family's hosts, plus the earlier held-back sample's URLs
that have since left that history: no real visit of this type, so 0/3
rules confirmed. Not every rule was matched by a real visit, so this type
stays a development draft.

Brauser offers this draft on an Aha! account's tenant subdomain,
`<account>.aha.io` (the `aha` family's strict subdomain origin,
`exclude: ["www", "secure", "support", "status", "cdn", "ideas", "identity",
"iad-prod1"]`), alongside Aha! features, requirements, epics, releases and
ideas. Any combination can be chosen in Capture Site and in the settings
gallery. The draft records one artifact per Aha! Notes / Knowledge document
(a "page"), however it was opened:

`aha.note:<account>.aha.io/<reference number>`

The ID is the page's reference number (for example `PRJ-N-67`), seen in
thousands of Aha!'s own support-site links and in Aha!'s public `aha-mcp`
README.

```toml
[[artifacts]]
type   = "aha.note"
origin = "https://acme.aha.io"
match  = [
  { path = "/pages/{ref:name(5,100)|upper}" },
  { path = "/products/{_}/pages/{ref:name(5,100)|upper}" },
  { path = "/develop/pages/{ref:name(5,100)|upper}" },
]
id     = "aha.note:{host}/{ref}"
title  = ["{ref:name} {title} | Aha!{~}", "{title} | Aha!{~}"]
generic_titles = ["Log in to your workspace", "Login"]
refs   = [{ type = "aha.feature", find = "jira_key" }]
```

`generic_titles` lists the titles shown by Aha!'s sign-in page instead of the
item's own name (`adapter-templates.md` §9.4). A visit whose title is one of
them, ASCII case aside, or is empty or the bare host, records no title, so the
artifact keeps the title it already has ("Untitled" for a new one).

One list covers both the title the template extracts and the whole-title
fallback (§9.4), so an item whose own name is `Login` also records no title.
No recorded title had such a name when the list was set.

## What each rule covers

1. **Page record**, `/pages/<ref>` (thousands of Aha!'s own support-site
   links, plus the `aha-mcp` README).
2. **Workspace-prefixed page link**, `/products/<prefix>/pages/<ref>` (the
   most common form in Aha!'s own support links). The workspace segment is
   `{_}` (skipped), so a note moved between workspaces keeps its ID.
3. **Aha! Develop form**, `/develop/pages/<ref>` (route confirmed to exist
   logged-out; undocumented, so low confidence).

Fragments (`#administrator-roles`, `#top`, and so on) are heading anchors
and are ignored by every rule, the same as query noise.

## Titles

Two entries, tried in order:

1. `{ref:name} {title} | Aha!{~}` — only when the leading word of the tab
   title equals the path's reference number.
2. `{title} | Aha!{~}` — otherwise.

Low confidence: no public source shows an in-app page's tab title, the same
reasoning `aha.feature`'s page gives in full (Known limits, below).

## Recorded samples

A synthetic equivalent of the recorded shape, on `acme.aha.io`:

| URL | Tab title | Records |
|---|---|---|
| `/pages/<ref>` | `PRJ-N-67 Q3 planning notes \| Aha!` | that reference number, title `Q3 planning notes` |

The blind research scored 90 synthetic vectors across this family's seven
drafted types; this PR carries over 10 `aha.note` cases, all correctly
classified by the research's own draft and re-verified against this
family's own fixture through both adapter-template engines. Graded later
against 2 real, held-back Aha! browsing visits
(`docs/research/blind/SCORECARD.md`), the research matched Aha!'s site-wide
result of 2 of 2 correct — not broken out per type, since neither of those 2
real visits happened to be a note.

## What the draft does not record

- `/pages/new` and any other route word shorter than 5 characters directly
  under `/pages/`, `/products/<prefix>/pages/` or `/develop/pages/`.

## Known limits

- **Aha!'s own internal numeric page ID is a second artifact for the same
  note.** Aha!'s own links sometimes use `/pages/<19-digit internal id>`
  (seen in Aha!'s own support site) instead of the reference-number form.
  `name(5,100)|upper` also matches that numeric form, so it is recorded as
  its own `aha.note` artifact, keyed by the numeric ID rather than the
  reference number — a vector pins this known limit. There is no way to map
  one ID to the other with this engine's template language; whether Aha!
  server-redirects `/pages/<numeric>` to `/pages/<ref>` in practice is an
  open question for the live check below.
- **Every title template is low confidence**, the same limit
  `aha.feature`'s page states in full.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Aha! account:

1. Capture Site: open a page from Knowledge; record the URL and the exact
   tab title (`/pages/<ref>`, and does it read `<ref> <name> | Aha!...`?).
2. Open the same page through its workspace-prefixed form
   (`/products/<prefix>/pages/<ref>`) and from Aha! Develop; confirm both
   land on the same record.
3. Move the page to another workspace and confirm its reference number and
   URL are unchanged.
4. Find a real `/pages/<19-digit id>` link (if one is in reach) and check
   whether it redirects to the `/pages/<ref>` form; if it does, the known
   limit above (a second artifact per note) does not apply in practice.
5. On a Capture Site review on a page with no saved rules, confirm Brauser
   offers all six Aha! types — features, requirements, epics, releases,
   ideas and notes — each with its own Aha!-account statement and the
   development-draft source line; confirm saving one turns the page card's
   status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `aha.note`'s `status`
in `protocol/starters/aha.json` to `{ "state": "certified", "method":
"live", "checked_on": "<date>", "checked_by": "John" }`, and run `npm run
gen:starters` (`protocol/starters/README.md`). The Capture Site source line
follows the status.
