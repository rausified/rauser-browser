# figma.file starter draft

Status: development draft. The URL and title rules come from
`docs/research/blind/figma/`, researched blind: public sources only, no
browsing history, cookies or a Figma account. Both adapter-template engines
verify the samples below. A live check with a real Figma account is still
needed before this draft is release-certified.

History check (2026-11-09): graded against a held-back sample of real
browsing history (real URLs from eight researched sites): 34 real URLs
(42 visits) matched, 2/4 rules confirmed, the recorded title right on
30/34. Not every rule and title was confirmed, so this type stays a
development draft.

Brauser offers this draft on the exact origin `https://www.figma.com`,
alongside Figma branches (the `figma` family). Either can be chosen alone in
Capture Site and in the settings gallery. The draft records one artifact per
file, however it was opened and whatever Figma product it belongs to:

`figma.file:www.figma.com/<file key>`

The key is Figma's own file key, documented as parseable from any Figma file
URL and shared across every product (design, FigJam, Slides, Sites, Buzz,
Make, Specs, Weave), `token(22,128)`: letters, digits, `-`, `_` and a trailing
`=`, case-sensitive.

```toml
[[artifacts]]
type     = "figma.file"
origin   = "https://www.figma.com"
unscoped = "acknowledged"
match    = [
  { path = "/(file|design|board|slides|site|buzz|make|rev|spec|weave)/{key:token(22,128)}" },
  { path = "/(file|design|board|slides|site|buzz|make|rev|spec|weave)/{key:token(22,128)}/{_}" },
  { path = "/(proto|deck|presenter)/{key:token(22,128)}", mode = "view" },
  { path = "/(proto|deck|presenter)/{key:token(22,128)}/{_}", mode = "view" },
]
id       = "figma.file:{host}/{key}"
title    = ["{~} · {title} - Figma Slides", "{title} – {~}", "{title} - {~}"]
```

## What each rule covers

1. **Editor route, no slug**, `/<route>/<key>`, where `<route>` is one of the
   nine editor words Figma's own URL router builds (`file` the legacy link,
   `design`, `board` FigJam, `slides`, `site` Figma Sites, `buzz` Figma Buzz,
   `make` Figma Make, `rev` an older Make route, `spec` Figma Specs, `weave`
   Figma Weave). No mode is recorded: the same URL is an edit for an editor
   and a view for a view-only seat.
2. **Editor route with a slug**, `/<route>/<key>/<name slug>`. The slug is
   the file name Figma writes into the URL and is never used for matching or
   the ID; Figma can omit it entirely (the `opaque_file_slug_in_url` flag),
   which rule 1 covers.
3. **Viewer route, no slug**, `/(proto|deck|presenter)/<key>`, `mode = "view"`.
   `proto` is the prototype player; `deck` and `presenter` are Figma Slides'
   audience and presenter views.
4. **Viewer route with a slug**, `/(proto|deck|presenter)/<key>/<name slug>`,
   `mode = "view"`.

No rule has a rest segment, so `/<route>/<key>/image/<hash>` (an image asset)
and `/export/<key>` (a video export) match nothing, and
`/<route>/<key>/branch/<branch key>/...` goes to `figma.branch` instead
(below). Query parameters (`node-id`, `t`, `m`, `page-id`,
`starting-point-node-id`, `scaling`, `version-id`, `fullscreen` and the like)
never change the ID or the mode.

## Titles

Three entries, tried in order:

1. `{~} · {title} - Figma Slides` matches Figma Slides' audience and
   presenter titles, `Audience · <name> - Figma Slides` and
   `Presenter · <name> - Figma Slides` (U+00B7 middle dot), skipping the
   localized role label.
2. `{title} – {~}` (U+2013 en dash) strips the final `<space>–<space><product>`
   suffix Figma's editor uses for every other product (` – Figma`,
   ` – FigJam`, ` – Figma Sites`, ` – Figma Buzz`, ` – Figma Make`,
   ` – Figma Specs`). The skip at the end means the suffix text itself
   never has to be listed; a name containing its own ` – ` keeps the nearer
   occurrence (`"Q3 – Roadmap – Figma"` records `"Q3 – Roadmap"`).
3. `{title} - {~}` (a plain hyphen) is tried last, and exists for one real
   shape: a prototype tab reads `<file name> - <page name>` with no leading
   mark (coordinator ruling, correcting the blind research's guess of a
   leading `▶`). The skip strips the trailing `<space>-<space><page name>`
   the same way entry 2 strips a product suffix, recording just the file
   name. Because this entry has no fixed suffix text, it also matches a file
   whose own name contains a plain hyphen with nothing after it to skip
   except the file's own trailing word — entry 2 is tried first and normally
   wins for a real editor title (which always ends in an en-dash suffix), so
   entry 3 only fires when entry 2's en dash is absent, as a real prototype
   tab's title is.

While a file is still loading, the tab title is just `Figma`: no entry
matches (there is no ` · `, ` – ` or ` - ` to anchor on), so the whole title
`Figma` is recorded, the same fallback Miro uses for an unloaded board.

## Recorded samples

Synthetic equivalents of the recorded shapes, on `www.figma.com`:

| URL | Tab title | Records |
|---|---|---|
| `/design/<28-char key>/Checkout-Flow` | `Checkout Flow – Figma` | that key, no mode, title `Checkout Flow` |
| `/proto/<same key>/Checkout-Flow?node-id=...` | (untitled sample) | the same artifact, view mode |

The blind research scored 52 synthetic vectors across this family's two
types (19 `figma.file` and 9 `figma.branch` positives, 24 negatives), all
correctly classified by the research's own draft. This draft's one real
change from that draft is the title list: the coordinator's live-informed
ruling replaced the leading `▶` prototype entry (which this draft's rules
would otherwise have recorded as `▶ <file>`, scorecard miss 4) with the
hyphen entry above, because a real prototype tab has no `▶` mark and instead
reads `<file> - <page>`.

## What the draft does not record

- the file-browser lists (`/files/recents-and-sharing`, `/files/project/<id>`,
  `/files/team/<id>/...`) — note the plural `files`, never matched by the
  singular route words;
- the create routes (`/design/new`, `/board/new`, `/slides/new`, ... and
  every other `/<route>/new`), excluded by the 22-character key floor;
- an image asset under a file (`/design/<key>/image/<hash>`) or a video
  export (`/export/<key>`), neither of which is a rule with a rest segment;
- the community listing (`/community/file/<id>/<slug>`), a different, public
  ID space, not a user's own file;
- the embed wrapper (`/embed?url=...`) and `embed.figma.com` itself, a
  different origin deliberately left out (research "Origin handling");
- a key shorter than 22 characters, a key containing a character outside the
  token alphabet, an empty path segment, or the plain `http://` scheme.

## Known limits

- **Title is set client-side.** The server's own `<title>` is just `Figma`;
  until the real title loads, the whole string `Figma` is recorded (Review
  Focus item 4).
- **Multi-page prototypes.** A prototype's page name is part of the recorded
  shape (`<file> - <page>`) but the hyphen entry strips it, so only the file
  name is recorded; the page name itself is never kept anywhere.
- **Entry 3 splits at the title's last ` - `.** A prototype tab,
  `<file> - <page>`, is split correctly: the text after the last ` - ` is
  discarded as the page name and only `<file>` is recorded. But any other
  tab title that lacks an en-dash product suffix (so entry 2 never fires)
  and still contains a plain ` - ` is split the same way: a design file
  literally named `Q3 - Roadmap` would record as just `Q3`, losing
  ` - Roadmap` from its own name, not a prototype's page name at all
  (probe: `Q3 - Roadmap` → `Q3`). This is believed rare — a real editor
  title always carries the en-dash suffix entry 2 strips first — and is a
  trade-off, not a bug, per the Global Constraints.
- **Branch titles use the same three entries**, but whether a branch's
  `<name>` is the branch's own name or the main file's name is unconfirmed
  (see `docs/adapters/figma.branch.md`).
- The ID template is frozen once shipped (adapter-templates.md §8.2).
- A future Figma route this draft does not list (`/<new word>/<key>`) fails
  closed because no rule's alternation covers it.

## Live check still needed

Before this page's status line changes to release-certified, check on macOS
with real Chrome and a real Figma account:

1. Open one file of each product (Design, FigJam, Slides, Sites, Make, Buzz)
   from the file browser; record the URL and `document.title` after load,
   and while loading.
2. In a Design file: Copy link, Copy link to selection, switch to Dev Mode,
   open version history, open Present (prototype), and check each URL keeps
   the same key.
3. Confirm a prototype tab reads `<file> - <page>` and records `<file>`.
4. In Slides: Present (audience) and Presenter view; confirm `/deck/` vs
   `/presenter/` and the `Audience · … - Figma Slides` title.
5. Open a legacy `/file/<key>` link to a FigJam board: confirm it redirects
   to `/board/<key>` with the same key.
6. Confirm whether `opaque_file_slug_in_url` is on (URLs with no slug) in the
   account.
7. Check a file whose name starts with an emoji and one containing ` – `.
8. Check a non-prototype file whose own name contains a plain ` - ` (not
   the en-dash ` – `), such as `Q3 - Roadmap`; confirm whether its title
   gets mis-split at the last ` - ` as entry 3 predicts (Known limits).
9. Check that `/files/…`, `/files/project/<id>` and the community pages are
   not recorded, and that file keys seen are 22 characters.
10. Repeat titles with the UI language set to something other than English.
11. On a Capture Site review on a Figma file page with no saved rules,
    Brauser offers Figma files and Figma branches with the every-account
    statement and the development-draft source line; saving one turns the
    page card's status to "● Artifact recognized".

Then record the date in this page's status line (`Status: release-certified
<date>. …`, as in `docs/adapters/jira.issue.md`), set `figma.file`'s `status`
in `protocol/starters/figma.json` to `{ "state": "certified", "method":
"live", "checked_on": "<date>", "checked_by": "John" }`, and run `npm run
gen:starters` (`protocol/starters/README.md`). The Capture Site source line
follows the status.
