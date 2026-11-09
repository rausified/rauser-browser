# Miro board starter draft

Status: development draft. The URL and tab-title formats were checked on
2026-11-01 against 12 real board visits in the maintainer's Chrome history
(Miro web app, `https://miro.com`). Both adapter-template engines verify the
samples below. A live check in the Capture Site review is still needed before
this draft is release-certified.

Brauser offers this draft on the exact origin `https://miro.com`. It records
one artifact per board:

`miro.board:miro.com/<board ID>`

A board ID is 12 characters from token's alphabet, ending in base64 padding:
`uXjVM0uuBLo=` (current boards) or `o9J_kpXb3Lw=` (the older `o9J_` form,
illustrative). Every board seen on 2026-11-01 had exactly 12 characters with
one `=`, so the rule is `{board:token(12)}`. `token` accepts at most two
trailing `=` (adapter-templates.md §7.1). The ID is case-sensitive and is
recorded as matched.

```toml
[[artifacts]]
type   = "miro.board"
origin = "https://miro.com"
match  = [{ path = "/app/board/{board:token(12)}" }]
id     = "miro.board:{host}/{board}"
title  = "{title} - Miro"
```

Recorded samples. Each one is a shared vector (`eval.miro.*` in
`protocol/adapter-templates/vectors.json`):

- `https://miro.com/app/board/uXjVM0uuBLo=/`: a board opened directly. The
  trailing slash Miro always adds is dropped before matching (§5.1).
- `https://miro.com/app/board/uXjVM0uuBLo=/?share_link_id=687498250990`: a
  board opened from a share link.
- `https://miro.com/app/board/uXjVM0uuBLo=/?moveToWidget=3458764680052296737&cot=14`:
  a board opened at a widget, for example from a comment or a mention.
  Query parameters never change the ID.
- Tab title `SSE ZTA SWOT - Miro`: the title entry records `SSE ZTA SWOT`.

The draft deliberately does not record:

- the dashboard and team board lists (`/app/dashboard/…`), the app root
  (`/app/`), sign-in (`/login/`) or SSO (`/sso/saml/`);
- anything below a board path, such as `/app/board/<id>/export`. The rule has
  no rest segment, so only the board itself counts;
- a board ID that is not 12 characters.

**Scope.** miro.com serves every Miro account, work and personal, from one
origin, and board URLs do not name the account or team. The adapter therefore
records boards from every Miro account signed in to the Chrome profile, and the
Capture Site review says so. miro.com is not on the shared-origin list
(adapter-templates.md §6), so the adapter needs neither `scope` nor
`unscoped`. That was decided on 2026-11-01: a board's own sharing settings
control who can see it, and listing miro.com as shared would have required an
acknowledgment on every existing Miro adapter. Use a separate Chrome profile
for personal Miro work if you do not want it recorded.

**Known limits.**

- While a board loads, Miro's tab title is `Miro - Miro`, which the title
  entry would record as `Miro`. The worker holds a new visit briefly and takes
  its title from a later title update (DESIGN.md §5.1), so the board name
  usually wins. A visit that ends before the board finishes loading can still
  be recorded as `Miro`. An untitled board's title is `Untitled - Miro` and is
  recorded as `Untitled`.
- Embedded boards (`/app/live-embed/<id>/`) and board links on other origins
  are not recorded. Each needs its own recorded sample before it joins the
  starter.
- The ID template is frozen once shipped (adapter-templates.md §8.2).
  Widening the rule later (another ID length, or another route) goes through
  `STARTER_HISTORY` in `extension/starter-adapters.ts`.

**Summaries and tasks.** A board's content is drawn on a canvas, so page
extraction reads only Miro's interface text. The service recognises a board's
URL and tells the agent to read that one board with its own Miro tools
(DESIGN.md §4.5 "Miro boards"). That works only when the agent has a Miro
connector, Miro's MCP server, or Miro API credentials of its own; otherwise
the summary says the board could not be read.

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Miro account, the way the Jira, SharePoint and
GitHub starters were certified on 2026-11-02:

1. On a board page with no saved Miro rules, **Capture Site** offers **Miro
   boards** with the every-account statement and the development-draft source
   line. Saving it turns the page card's status to "● Artifact recognized",
   and settings' starter gallery then shows Miro boards as **Already
   configured**.
2. The same board opened directly, from a share link (`?share_link_id=…`) and
   at a widget (`?moveToWidget=…`, for example from a comment) lands on one
   artifact: Recent shows one row for it, and a page note written on one form
   shows on the others.
3. Once the board has loaded, the recorded title is the board's name, not
   `Miro`. Note the real tab title of an untitled board, and add a title entry
   or vector if it is not `Untitled - Miro`.
4. The dashboard (`/app/dashboard/…`), the app root (`/app/`), sign-in
   (`/login/`) and SSO stay "○ Not an artifact", and an embedded board
   (`/app/live-embed/<id>/`) records nothing.
5. If you can open a board with an older `o9J_` ID, it records with its ID
   exactly as matched (case kept); otherwise say it was not checked.

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set the Miro type's `status`
in `protocol/starters/miro.json` to `{ "state": "certified", "method": "live",
"checked_on": "<date>", "checked_by": "John" }`, and run `npm run
gen:starters` (`protocol/starters/README.md`). The Capture Site source line
follows the status.
