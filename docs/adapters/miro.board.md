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
