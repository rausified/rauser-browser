# box.file starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/box/`, researched blind: public sources only, no
browsing history, cookies or a Box account.

Brauser offers this draft on an enterprise's custom subdomain,
`<company>.app.box.com` (the `box` family's strict subdomain origin,
`exclude: ["www", "cloud"]`), alongside Box folders and Box shared links.
Any combination can be chosen in Capture Site and in the settings gallery.
The draft records one artifact per file, Box Note or Box Canvas, however it
was opened or edited:

`box.file:<company>.app.box.com/<file ID>`

The ID is Box's permanent file ID (API `files/<id>`, typed `f_<id>`); it
survives renames and moves. Box's web-app bundle routes a file item to
`/notes/<id>` when it is a Box Note and to `/canvas/<id>` when it is a Box
Canvas, always keeping the same numeric file ID, so Notes and Canvas share
this type's namespace rather than getting their own: a note reached by
`/file/<id>` and by `/notes/<id>` stays one artifact.

```toml
[[artifacts]]
type   = "box.file"
origin = "https://acme.app.box.com"
match  = [
  { path = "/file/{id:int}" },
  { path = "/notes/{id:int}" },
  { path = "/canvas/{id:int}" },
  { path = "/s/{_}/file/{id:int}" },
  { path = "/v/{_}/file/{id:int}" },
  { path = "/integrations/officeonline/openOfficeOnline", query = { fileId = "{id:int}" }, mode = "edit" },
]
id     = "box.file:{host}/{id}"
title  = "{title} | Powered by Box"
```

## What each rule covers

1. **File preview**, `/file/<id>`, logged in. The canonical route the web
   app's bundle uses for a plain file item.
2. **Box Note editor**, `/notes/<id>`. Notes open in their own editor but
   keep the file ID.
3. **Box Canvas**, `/canvas/<id>`. An unavailable Canvas 302s to
   `/file/<id>` with the same ID.
4. **File inside a shared-link folder**, `/s/<shared name>/file/<id>`, the
   form seen when a shared folder is browsed down to a file.
5. **File inside a custom-URL (vanity) folder**, `/v/<vanity name>/file/<id>`,
   the vanity-URL equivalent of rule 4.
6. **LOW confidence: Office for the web editing**,
   `/integrations/officeonline/openOfficeOnline?fileId=<id>`, `mode =
   "edit"`. From memory of community posts, not seen in Box's loaded bundle
   chunks. Harmless if wrong: its literal path and query key match nothing
   else.

No rule has a rest segment, so `/file/<id>/<anything>` never matches, and
`/notes/create_note` (an action, not a numeric ID) never matches either.
Query parameters (`?s=<share token>`, `sortColumn`, `sortDirection`, ...)
never change the ID.

## Titles

One entry: `{title} | Powered by Box` strips the app's own title-component
suffix (`enduser.documentTitle.poweredByBoxTitle` in the public bundle),
leaving the file name with its extension.

While the page is untitled, an enterprise user's tab instead reads
`{enterprise name} | Powered by Box` (Known limits, below): this entry still
matches, so the enterprise name is recorded as if it were the file's title.
A dead or bad link's title, `Box | Simple Online Collaboration`, matches no
entry and is kept whole (`title_entry` null).

## Recorded samples

Synthetic equivalents of the recorded shapes, on `acme.app.box.com`:

| URL | Tab title | Records |
|---|---|---|
| `/file/<12-digit ID>` | `Q3 plan.pdf \| Powered by Box` | that file ID, title `Q3 plan.pdf` |
| `/s/<32-char share name>/file/<id>` | `Rollout plan.pdf \| Powered by Box` | that file ID, title `Rollout plan.pdf` |

The blind research scored 51 synthetic vectors across this family's three
types (11 `box.file` positives among them), all correctly classified by the
research's own draft and re-verified against this family's own fixture
through both adapter-template engines. Graded later against 5 real,
held-back Box browsing visits (`docs/research/blind/SCORECARD.md`), the
research matched all 5 correctly, with a stable ID across URL forms
(`/s/…/file/<id>` and `/file/<id>` gave one ID) and the right title template
(`{title} | Powered by Box`) — the only miss recorded for the site was the
known enterprise-name loading-title limit below, named up front.

`samples_shape` may be flipped to `false` if the structural drafter cannot
tell `/file/<id>` and `/s/<share>/file/<id>` apart from these two examples
alone (the same ambiguity Vidcast's video samples hit, `docs/adapters/
vidcast.video.md` "Recorded samples"); see the generated-checks step for
whether that applied here.

## What the draft does not record

- Box's lists and chrome: `/recents`, `/trash`, `/files/favorites`, the
  Admin Console (`/master/…`), and the login host `account.box.com` — none
  of them is a file;
- `/notes/create_note`, a create-note action, not a file ID;
- `/file/<id>/<anything>`, since no rule carries a rest segment;
- the legacy hash-era form `/files/0/f/<folder>/1/f_<file>`: the `f_` prefix
  cannot be separated by `@tail`, which splits only on `-`;
- `/web_link/<id>`, a Box bookmark that opens another site, a different
  artifact if any;
- `/embed/…` and `/preview/expiring_embed/…`, iframe forms, not tabs;
- a non-numeric file ID.

## Known limits

- **This draft does not cover personal `https://app.box.com` or
  `<company>.ent.box.com`** (Open decision 1). The family's strict
  `.app.box.com` subdomain origin claims only `<company>.app.box.com`
  hosts; `familyClaims` never claims the suffix's own bare host, so a
  personal Box account without a custom subdomain is not offered this
  starter, and Box Zones' `*.ent.box.com` twins (observed serving shared
  links, e.g. `weforum.ent.box.com`) are a separate suffix this family does
  not claim either. Covering all three needs either a permissive `.box.com`
  family (ruled out: builder's call 7) or a multi-origin model, which needs
  engine code; it is recorded as a follow-up bead.
- **This starter is also offered, unintentionally, on Box's upload hosts,**
  `fupload-<n>.app.box.com` (named in `docs/research/blind/box/
  adapters.toml`): `origin.exclude` matches a whole first label exactly, and
  cannot express a `fupload-*` prefix pattern, so the strict `.app.box.com`
  family claims these hosts too. Those hosts serve file uploads, not the web
  app's pages, so none of this type's rules is expected to match anything
  there in practice; this is a known limit of the data, not a behavior
  change, and is tracked as a follow-up (an `exclude` prefix-pattern form in
  the generator).
- **The enterprise-name loading title is recorded as the file's title.**
  While a page is untitled, an enterprise user's tab reads `{enterprise
  name} | Powered by Box` (Review Focus; predicted and confirmed by the
  scorecard). The worker's wait for a later title update usually covers
  this, but a fast enough save can still catch it.
- **Box Notes and Canvas titles are unverified.** Notes open in a new tab
  and may set their own title; if Notes use another suffix, the whole tab
  title is recorded until a second title entry is added.
- **The Office-for-the-web rule is low confidence** and unverified against
  a live account; it is harmless if wrong.
- A file name containing `| Powered by Box` loses that literal suffix
  (theoretical; not observed).
- Localized Box UIs may translate "Powered by Box"; not checked.
- The ID template is frozen once shipped (`adapter-templates.md` §8.2).

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Box account:

1. Capture Site: open a file preview from All Files; record the URL and tab
   title (`/file/<id>`, `<name.ext> | Powered by Box`?). Repeat for a Box
   Note (does it stay on `/notes/<id>`, and what is its title?) and a Box
   Canvas.
2. As a collaborator, open a `/s/<name>/file/<id>` link to a file you can
   access; confirm the recorded ID matches the same file's `/file/<id>` ID.
3. With a custom-subdomain enterprise account, open
   `app.box.com/file/<id>`; does it redirect to `<company>.app.box.com`? Do
   links copied in the UI use the custom subdomain?
4. Open a file in Office for the web from Box; record the URL, and fix or
   drop rule 6 if it does not match `/integrations/officeonline/
   openOfficeOnline?fileId=…`.
5. Watch the title while the enterprise name flashes before the real title
   loads, and in a non-English locale.
6. Check `/file/<id>` sub-routes the UI uses (versions, details sidebar) and
   whether any should count; today the rules have no rest segment.

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `box.file`'s `status`
in `protocol/starters/box.json` to `{ "state": "certified", "method":
"live", "checked_on": "<date>", "checked_by": "John" }`, and run `npm run
gen:starters` (`protocol/starters/README.md`). The Capture Site source line
follows the status.
