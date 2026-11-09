# google.formresponse starter draft

Status: development draft. The URL rules come from
`docs/research/blind/google-workspace/`, researched blind: public sources
only, no browsing history, cookies or Google account (there, this type is
named `google.form.responder`; it ships here as `google.formresponse` to fit
the type-name rule — a type name may hold only one dot). Both
adapter-template engines verify the samples below. A live check with a real
Google account is still needed before this draft is release-certified.

History check (2026-11-09): graded against real browsing history: 3 real
URLs (3 visits) matched, 1/3 rules confirmed, the recorded title right
on 3/3. Not every rule and title was confirmed, so this type stays a
development draft.

Brauser offers this draft on the exact origin `https://docs.google.com`,
alongside Google Docs, Sheets, Slides and the Forms editor (the
`google-workspace` family). Any one of the five can be chosen alone in
Capture Site and in the settings gallery. The draft records one artifact per
form you fill in, however the link was opened:

`google.formresponse:docs.google.com/<published ID>`

The ID is the form's published ID, the Forms API's `responderUri` value
(`1FAIpQLS…`, about 56 characters) — a *different* ID than the form's own
`formId` (`docs/adapters/google.form.md`), with nothing in the URL mapping
one to the other. It is its own artifact type for exactly that reason, not a
second ID for `google.form`.

```toml
[[artifacts]]
type     = "google.formresponse"
origin   = "https://docs.google.com"
unscoped = "acknowledged"
match    = [
  { path = "/forms/d/e/{id:token(20,128)}/(viewform|formResponse|closedform)", mode = "view" },
  { path = "/forms/u/{_}/d/e/{id:token(20,128)}/(viewform|formResponse|closedform)", mode = "view" },
  { path = "/a/{_}/forms/d/e/{id:token(20,128)}/(viewform|formResponse|closedform)", mode = "view" },
]
id       = "google.formresponse:{host}/{id}"
```

## What each rule covers

1. **Canonical responder link**, `/forms/d/e/<publishedId>/(viewform|formResponse|closedform)`,
   always view mode: `viewform` is the form itself, `formResponse` is the
   confirmation page after submitting, and `closedform` is a form that is no
   longer accepting responses (well-known route names; `closedform` is not
   independently observed, so it is MEDIUM confidence).
2. **Multi-account form**, `/forms/u/<n>/d/e/<publishedId>/...`, `{_}` and
   never captured (research "Origin model").
3. **Legacy Google Apps form**, `/a/<domain>/forms/d/e/<publishedId>/...`,
   matched with `{_}` for the same reason.

`?usp=sf_link` (share link), `?embedded=true` and prefilled-entry query
noise (`?usp=pp_url&entry.N=...`) never change the ID or the mode.

**Adapter order proves the two Forms types stay distinct.** `google.doc`,
`google.sheet`, `google.slides`, `google.form` and `google.formresponse` are
checked in that order (adapter indices 0–4), first match wins. A responder
URL's `/d/e/<id>/...` segment never satisfies `google.form`'s rules — the
one-character `e` falls below that type's 20-character ID floor — so even
though `google.form` is checked first, it never claims a responder link; the
`google.formresponse.viewform` vector pins exactly this.

## Titles

No title template. The respondent's tab title is the bare form title with no
product suffix (observed on the Forms API's public example form), so
Brauser records the whole tab title, as it does for any URL with no
matching title entry.

## Recorded samples

Synthetic equivalent of the recorded shape, on `docs.google.com`:

| URL | Tab title | Records |
|---|---|---|
| `/forms/d/e/<56-char id>/viewform` | (no sample title; see "Titles") | that ID, view mode |

The blind research for this family scored 17 URLs that should match across
its five kept types, with 15 matched correctly, 0 wrongly matched, and 2
missed (both Gmail message permalinks, outside this family — see "Known
limits"). This type's own URL confidence is MEDIUM-HIGH; nothing about its
rules was a scored miss.

## What the draft does not record

- `/forms/d/e/<id>/prefill` (a prefill-link builder, not an open of the
  form itself);
- the bare `/forms/d/e/<id>` with no route word after the published ID
  (unverified: Google may or may not serve this form at all);
- `forms.gle/<code>` short links, which live on a different origin — Google
  redirects them here, and that landing visit is what gets recorded.

## Known limits

- **`closedform` is unverified.** It is a well-known route name, not an
  observed one; the live check below should open an actually-closed form if
  one is available.
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

1. Open a form's published responder link (`viewform`), submit it
   (`formResponse`), and, if one is available, open an actually-closed form
   (`closedform`). Record each URL and the real tab title.
2. Repeat signed in to a second account to confirm the `/u/1/` form lands on
   the same artifact.
3. Open the link with `?usp=sf_link` (share link), `?embedded=true` and a
   prefilled entry query; confirm none of them change the recorded artifact.
4. `/prefill` and the bare published-ID path each stay "○ Not an artifact",
   and the sibling `google.form` type never claims a responder link (see
   "What each rule covers").
5. Record the tab title while the responder page is loading, as an account
   without access, and on a Workspace account where the app is disabled;
   note whether the worker's title hold hides the loading title.
6. On a Capture Site review on a responder page with no saved rules, Brauser
   offers all five Google Workspace types with the every-account statement
   and the development-draft source line; saving one turns the page card's
   status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `google.formresponse`'s
`status` in `protocol/starters/google-workspace.json` to `{ "state":
"certified", "method": "live", "checked_on": "<date>", "checked_by": "John"
}`, and run `npm run gen:starters` (`protocol/starters/README.md`). The
Capture Site source line follows the status.
