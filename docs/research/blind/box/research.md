# Box blind starter: research notes

Date: 2026-11-03. Blind test: built only from public, logged-out sources and this
repository. No Box account, browser history, cookies or profiles were used. Every
page render used a new, empty headless-Chrome profile under `/tmp`.

Files: `adapters.toml` (the adapters), `vectors.json` (51 synthetic cases, made by
`gen_vectors.py`), `check.py` + `validate.sh` + `rust-check/` (throwaway checkers
that run the repo's own engines).

## Sources consulted

| Source | What it gave |
|---|---|
| developer.box.com/reference/resources/file--full/ | shared-link object samples: `https://www.box.com/s/vspke7y05sb214wjokpk`, download `…/shared/static/<name>.jpeg`, vanity `https://acme.app.box.com/v/my_url/` |
| developer.box.com/guides/shared-links/find-for-item/ | shared links resolve to items only through the API (`BoxApi: shared_link=…`), not from the URL |
| developer.box.com/guides/embed/box-embed/ | `/embed/s/<name>`, `/embed/folder/0`, `cloud.app.box.com/preview/expiring_embed/…` (iframe forms, excluded) |
| support.box.com "Enterprise Settings: Custom Setup Tab" (14392994462227) | custom subdomain "as an alternative to app.box.com", example `acme.box.com` |
| support.box.com "New Box Zones Domains" (16726358708371) | `*.ent.box.com` twins of `*.app.box.com` hosts |
| support.box.com "Creating a Box Note" (360044195473) | `https://app.box.com/notes/create_note` (an action, not a note) |
| support.box.com "Introducing / Configuring Box Canvas" | Box Canvas exists as a file type (no URL given) |
| Wikipedia external-link API (`list=exturlusage`, ~450 public Box URLs cited in articles) | real URL shapes: `/s/<name>`, `/s/<name>/file/<id>`, `/s/<name>/folder/<id>`, `/v/<vanity>`, `/v/<vanity>/file/<id>`, `/v/<vanity>/folder/<id>`, legacy `/shared/<name>`, legacy `/s/<name>/1/<folder>/<file>/1`, legacy `/files/0/f/<folder>/1/f_<file>`, root vanities `/<Name>`, hosts `app.box.com`, `<co>.app.box.com`, `<co>.box.com`, `<co>.ent.box.com` |
| `curl` (logged out) against a sample of those public links | status codes and server redirects (below) |
| Headless Chrome `--dump-dom` of public shared links (logged out, fresh profile) | the JS-set tab titles |
| Box's public web-app bundle `https://cdn01.boxcdn.net/enduser/main.*.js` and the chunks a shared page loads | the title component and the client routes (below) |

## Key evidence

**Title component (high confidence for the format).** The public bundle defines
`enduser.documentTitle.poweredByBoxTitle = "{title} | Powered by Box"` and
`enduser.documentTitle.defaultTitle = "Box | Simple Online Collaboration"`. The
component renders `{title} | Powered by Box` when the page has a title; otherwise
`{enterprise name} | Powered by Box` when the user has an enterprise; otherwise the
default. Rendered titles matched:

- `…/s/<name>` (a file): `Red Cross Opposition … 1996 Hearing.pdf | Powered by Box`
- `…/s/<name>/file/<id>`: `Rousing Welcome … Plaskett.pdf | Powered by Box`
- `…/v/media/file/<id>`: `Generation adidas Cup.png | Powered by Box`
- `…/s/<name>` (a folder): `Carolyn Plaskett | Powered by Box`, `Press Materials | Powered by Box`
- `…/v/media` (a folder): `Media | Powered by Box`
- `…/v/media/folder/<subfolder id>`: `Media | Powered by Box` (the share root's name, not the subfolder's; one sample)
- dead or bad links: `Box | Simple Online Collaboration`
- the static HTML before JS runs: `Box`

File titles include the extension. Only logged-out shared pages were seen; the
logged-in `/file/<id>` and `/folder/<id>` pages use the same bundle and component,
so the same format is expected there (**medium**), but it was not observed. The
message is localized; non-English UIs may translate "Powered by Box" (not checked).

**Client routes (high).** The bundle routes a search result or recent item like so:
`folder → /folder/<id>`, `web_link → /web_link/<id>`, `file → /notes/<id>` when the
extension is `boxnote`, `/canvas/<id>` when it is `boxcanvas` (and Canvas is enabled),
else `/file/<id>`. Other route strings present: `/folder/<id>/search?…`, `/folder/0`,
`/hubs/<id>`, `/files/favorites`, `/master/…` (admin), `/s/…`, `/v/…`. Typed IDs are
`f_<id>` (file), `d_<id>` (folder), `l_<id>` (web link), `cao_<id>` (hub).

**Server behaviour (high, logged out).**

- `/shared/<name>` → 301 `/s/<name>`; `box.com/s/…` → `app.box.com/s/…`;
  `<co>.box.com/…` → 301 `<co>.app.box.com/…` (zynga, ucsf, mlssoccer).
  `weforum.ent.box.com/s/…` stays on `ent.box.com`.
- `/S/<name>` (uppercase route) → 404; `/s/<NAME IN CAPS>` and `/v/<VANITY IN CAPS>`
  render the same item as lowercase: names are case-insensitive, routes are not.
- `/v/<vanity>` exists only on its enterprise host (`app.box.com/v/media` → login).
- `/file/<id>?s=<name>` (logged out) → 302 `/s/<name>`.
- `/canvas/<id>` (logged out, unknown ID) → 302 `/file/<id>`.
- Legacy `/s/<name>/1/<folder>/<file>/1` and `/v/<vanity>/1/<folder>` → 404 today,
  even where `/s/<name>` itself still works.
- `/file/<id>`, `/folder/<id>`, `/recents`, `/trash`, `/web_link/<id>` → login.
- No rendered shared page carries a `<meta>` with the item ID (only `og:title` and an
  `og:image` with a *file version* ID), so a meta alias from link to file is not possible.

## Artifact types

### `box.file` — files, Box Notes and Box Canvas (one namespace)

ID: `box.file:<host>/<file id>`. The numeric file ID is Box's permanent item ID
(API `files/<id>`, typed `f_<id>`); it survives renames and moves. Notes and Canvas
are files with the same ID, so they are rules of `box.file`, not separate types, and a
note reached by `/file/<id>` or `/notes/<id>` stays one artifact.

| URL form | Confidence | Evidence |
|---|---|---|
| `/file/<id>` | high (route), medium (title) | bundle routes; login redirect |
| `/notes/<id>` | high (route), low (title) | bundle routes; Notes open in a new tab and may set their own title |
| `/canvas/<id>` | high (route), low (title) | bundle routes; 302 to `/file/<id>` when unavailable |
| `/s/<name>/file/<id>` | high | Wikipedia samples, rendered |
| `/v/<vanity>/file/<id>` | high | Wikipedia samples, rendered |
| `/integrations/officeonline/openOfficeOnline?fileId=<id>` (mode `edit`) | **low** | from memory of community posts; not in the loaded bundle chunks; harmless if wrong |

Title: `{title} | Powered by Box` → the file name with extension.

### `box.folder` — folders

ID: `box.folder:<host>/<folder id>` (typed `d_<id>`). Rules: `/folder/<id>`,
`/s/<name>/folder/<id>`, `/v/<vanity>/folder/<id>`, all `int(2,19)` so `/folder/0`
("All Files", the root list) is excluded; real IDs are 9–12+ digits. Confidence: high
for the URL forms; medium for titles (and the one shared-subfolder sample showed the
share root's name, so shared-subfolder titles may be wrong).

### `box.share` — the root of a shared link or custom URL

ID: `box.share:<host>/s/<shared name>` or `box.share:<host>/v/<vanity name>`, both
lowercased. Needed because `/s/<name>` and `/v/<vanity>` do not say whether they name
a file or a folder, and the page exposes no item ID. Confidence: high for the URL forms
and title; medium for stability. Trade-offs:

- A shared name dies when the link is disabled or re-created; a vanity name can be
  edited or reused by the owner. The ID names the link, not the item.
- A logged-in collaborator opening the same item by `/file/<id>` gets a different
  artifact (`box.file`). Only a server redirect (if Box redirects a collaborator from
  `/s/<name>` to `/file/<id>`, unverified) would corroborate an alias.
- Type named `box.share` because an ID scheme may not contain `_` (§8.1); the first
  draft's `box.shared_link` failed `id_syntax`.
- One rule `/{kind:(s|v)}/{link:token(1,100)|lower}`: separate `s` and `v` rules with
  different bounds fail `capture_type_conflict`, so implausibly short `/s/` names also
  match (Box shows an error page for those).

## Origin model

Not a shared origin per §6: no Box URL names a tenant, so there is nothing to scope, and
`app.box.com` is not on `SHARED_ORIGINS` (so `unscoped` is rejected). This follows the
Miro precedent: it records whatever Box account is signed in. Offer the starter family
on:

- `https://app.box.com`;
- `https://<company>.app.box.com` (custom subdomains; `<company>.box.com` redirects
  here);
- `https://<company>.ent.box.com` (observed serving shared links).

Exclude `www.box.com` (marketing), `account.box.com` (login), `developer.box.com`,
`support.box.com`, `api.box.com`, `upload.box.com`, and the `fupload-*` upload hosts.
In `starterKind` terms: `host === "app.box.com" || host.endsWith(".app.box.com") ||
host.endsWith(".ent.box.com")`, minus `fupload-*`. Adding `<company>.box.com` too
costs nothing, but every probe redirected away from it. The same item on `app.box.com`
and on `acme.app.box.com` gets two IDs because `{host}` is in the ID; I expect a user
with a custom subdomain to stay on it, but did not verify that.

## Deliberately excluded

- `/folder/0`, `/folder/<id>/search`, `/recents`, `/files/favorites`, `/trash`,
  `/master/…` (Admin Console), `/notes/create_note`, login (`account.box.com`).
- `/web_link/<id>`: a Box bookmark that opens another site.
- `/hubs/<id>` (Box Hubs): route confirmed and IDs are numeric, but no title evidence;
  a good next starter (`box.hub`), probably as a container.
- `/embed/…`, `/preview/expiring_embed/…`, `/embed_widget_preview?shared_name=…`: iframe
  forms, not tabs.
- `/shared/static/<name>.<ext>`: direct downloads.
- Legacy `/files/0/f/<folder>/1/f_<file>` (the `f_` prefix cannot be split off: `@tail`
  splits only on `-`) and the hash-routed `/files#/…` forms; legacy `/s/<name>/1/…`
  trees (404 today); root-level vanity paths like `/<Custom-Name>` (no anchor possible).
- Box Sign, Box Relay, Box AI pages: not document artifacts.

## Known limits

- While a page is untitled, an enterprise user's tab reads `<Enterprise> | Powered by
  Box`, which the title entry records as the enterprise name. A dead link's title,
  `Box | Simple Online Collaboration`, matches no entry and is kept whole. The worker's
  wait for a later title update usually covers the first case.
- A file name containing ` | Powered by Box` loses that suffix (theoretical).
- Shared-subfolder titles may carry the share root's name.
- Box Notes and Canvas titles are unverified; if Notes use another suffix, the whole tab
  title is recorded until a second title entry is added.
- Office-for-the-web and Google-editor integration URLs are unverified (one low-confidence
  rule; no Google rule).
- Localized UIs may translate "Powered by Box".

## Validation

`research/blind/box/validate.sh`, which:

1. converts `adapters.toml` to the engine's document JSON with Python `tomllib`
   (`{format: adapters.format, artifacts}`, the shape the host builds);
2. runs every vector through the TypeScript twin with
   `node scripts/adapter-templates-eval.mjs` (Node v24.14.0, `extension/dist` built
   02:58 after the 02:56 source change);
3. runs the same requests through the Rust `adapter-templates` crate using a throwaway
   binary in `rust-check/` (its own `[workspace]`, `cargo build --offline`, toolchain
   1.98.1) that calls `Document::from_value` and `Document::evaluate`, plus
   `overlap_report` / `overlapping_rules`;
4. compares the results with the expectations and the two engines with each other.

Result (2026-11-03): document valid (6 adapters); 51/51 vectors pass in both engines
(28 positive: 11 file, 8 folder, 9 share; 23 negative); no disagreement between the
engines in match, ID, mode, title, title entry or rejection reason; no rule overlaps.
Two fixes came out of validation: `capture_type_conflict` (folder bounds, and split
`s`/`v` rules) and `id_syntax` (`_` in the ID scheme).

## How I would verify this with a real account

1. Open a file preview from All Files; record URL and tab title (`/file/<id>`,
   `<name.ext> | Powered by Box`?). Repeat for a folder, a Box Note (does it stay on
   `/notes/<id>`, what is its title?) and a Box Canvas.
2. As a collaborator, open a `/s/<name>` link to a file you can access: does Box
   redirect to `/file/<id>`? If so, a server-redirect alias joins `box.share` to
   `box.file` automatically.
3. With a custom-subdomain enterprise account, open `app.box.com/file/<id>`: does it
   redirect to `<co>.app.box.com`? Do links copied in the UI use `<co>.app.box.com`?
4. Open a file in Office for the web and in Google Docs from Box; record both URLs, and
   fix or drop rule 5 of `box.file`.
5. Watch the title during load (enterprise-name flash) and in a non-English locale.
6. Open a subfolder inside a shared link (logged in) and check its title.
7. Check `/file/<id>` sub-routes the UI uses (versions, details sidebar) and whether any
   should count; today the rules have no rest.
8. Open a Box Hub and note URL and title for a `box.hub` follow-up.
