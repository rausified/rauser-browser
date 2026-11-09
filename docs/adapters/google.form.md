# google.form starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/google-workspace/`, researched blind: public sources
only, no browsing history, cookies or Google account. Both adapter-template
engines verify the samples below. A live check with a real Google account is
still needed before this draft is release-certified.

History check (2026-11-09): graded against real browsing history: no
real visits of this type, so 0/3 rules confirmed. Not every rule and
title was confirmed, so this type stays a development draft.

Brauser offers this draft on the exact origin `https://docs.google.com`,
alongside Google Docs, Sheets, Slides and Forms responder (the
`google-workspace` family). Any one of the five can be chosen alone in
Capture Site and in the settings gallery. The draft records one artifact per
form you own or edit, however it was opened:

`google.form:docs.google.com/<form ID>`

The ID is the form's own `formId`, documented in the Forms API as "letters,
numbers, hyphens, or underscores", `token(20,128)`, case-sensitive. A
respondent's published link carries a *different* ID with no mapping back to
this one — see `docs/adapters/google.formresponse.md`.

```toml
[[artifacts]]
type     = "google.form"
origin   = "https://docs.google.com"
unscoped = "acknowledged"
match    = [
  { path = "/forms/d/{id:token(20,128)}/{mode?:(edit|viewform)}" },
  { path = "/forms/u/{_}/d/{id:token(20,128)}/{mode?:(edit|viewform)}" },
  { path = "/a/{_}/forms/d/{id:token(20,128)}/{mode?:(edit|viewform)}" },
]
modes    = { edit = "edit", viewform = "view" }
id       = "google.form:{host}/{id}"
title    = ["{title} - Google Forms", "{title} – Google Forms"]
```

## What each rule covers

1. **Canonical form**, `/forms/d/<formId>[/edit|/viewform]`. The mode
   segment is optional: a bare `/forms/d/<formId>` is recorded with no mode.
   `edit` is edit mode. `viewform` on the `formId` itself is the legacy
   respondent link built on the editor's own ID (not the published
   `/forms/d/e/<id>/viewform` form — that is `google.formresponse`), and
   maps to view mode.
2. **Multi-account form**, `/forms/u/<n>/d/<formId>/...`, `{_}` and never
   captured (research "Origin model").
3. **Legacy Google Apps form**, `/a/<domain>/forms/d/<formId>/...`, matched
   with `{_}` for the same reason.

The one-character `e` segment of `/forms/d/e/<publishedId>/...` is shorter
than this type's 20-character ID floor, so a responder link can never be
mistaken for this type (confirmed by the `google.formresponse.viewform`
vector, which proves this adapter's rules never claim it even though
`google.form` is checked first — adapter order below). `#responses` and
`#settings` fragments and other query noise never change the ID or the mode.

## Titles

The tab title is inferred as `<name> - Google Forms`, by analogy with the
sibling Docs/Sheets/Slides editors — confidence LOW-MEDIUM, since no public
editable form was available to the blind research. The en dash form,
`<name> – Google Forms`, is also recorded (scorecard miss 3, applied
pre-emptively here since the suffix itself is still unconfirmed). The legacy
`viewform` link's tab title is the bare form name with no product suffix
(observed on the Forms API's own example form), so neither entry matches and
the whole tab title is recorded as given — which is already the form's name
on that view.

## Recorded samples

Synthetic equivalent of the recorded shape, on `docs.google.com`:

| URL | Tab title | Records |
|---|---|---|
| `/forms/d/<44-char id>/edit` | `Team survey - Google Forms` | that ID, edit mode, title `Team survey` |

The blind research for this family scored 17 URLs that should match across
its five kept types, with 15 matched correctly, 0 wrongly matched, and 2
missed (both Gmail message permalinks, outside this family — see "Known
limits"). The scorecard rates this type's own URL confidence MEDIUM and its
title confidence LOW-MEDIUM, since no public editable form could be
observed; nothing about this type's rules was a scored miss.

## What the draft does not record

- the Forms home list (`/forms/u/0/`);
- a published responder link (`/forms/d/e/<id>/...`), which is the separate
  `google.formresponse` type — the one-character `e` segment keeps it below
  this type's ID floor, so the two never collide.

## Known limits

- **The editor's title format is unconfirmed.** It is inferred from the
  sibling Docs/Sheets/Slides editors, not observed directly (research
  confidence LOW-MEDIUM). The live check below records the real title.
- **Gmail is not covered.** See `docs/adapters/google.doc.md` "Known
  limits": Gmail is fragment-routed and the message permalink is a possible
  future type, not a rule here.
- **Loading and interstitial titles are recorded as given.** See
  `docs/adapters/google.doc.md` "Known limits": a tab still loading may
  read the bare product name, and a Workspace-blocked or no-access
  interstitial records that string as the title.
- **Drive and Calendar wait** for the shared-origin migration, as for
  `google.doc`.
- The ID template is frozen once shipped (adapter-templates.md §8.2).
  Widening a rule later goes through `STARTER_HISTORY` in
  `extension/starter-adapters.ts`.

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Google account:

1. Open one owned and one shared form you can edit, from the home list and
   from a "Copy link" share link. Record the final URL and the real tab
   title; confirm it against the `- Google Forms` prediction above and add
   or correct the title entry if it differs.
2. Repeat signed in to a second account to confirm the `/u/1/` form lands on
   the same artifact.
3. Open the legacy `/forms/d/<formId>/viewform` link (the respondent view
   built on the editor's own ID) and confirm it records view mode with the
   bare form title.
4. The home list stays "○ Not an artifact", and a published responder link
   on the same form lands on `google.formresponse`, never on this type.
5. Record the tab title while the form editor is loading, as an account
   without access, and on a Workspace account where the app is disabled;
   note whether the worker's title hold hides the loading title.
6. On a Capture Site review on a form editor page with no saved rules,
   Brauser offers all five Google Workspace types with the every-account
   statement and the development-draft source line; saving one turns the
   page card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `google.form`'s
`status` in `protocol/starters/google-workspace.json` to `{ "state":
"certified", "method": "live", "checked_on": "<date>", "checked_by": "John"
}`, and run `npm run gen:starters` (`protocol/starters/README.md`). The
Capture Site source line follows the status.
