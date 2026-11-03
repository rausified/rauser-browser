# Figma blind starter: research notes

Researched 2026-11-03 from public, logged-out sources only. No browser history,
no account, no sign-in. Files: `adapters.toml`, `vectors.json`, `check/`.

## Sources consulted

| # | Source | What it gave |
|---|---|---|
| S1 | https://developers.figma.com/docs/rest-api/file-endpoints/ | "The file key can be parsed from any Figma file url: `https://www.figma.com/:file_type/:file_key/:file_name`", `?node-id=:id`; a branch key is accepted wherever a file key is (`GET /v1/files/:key`, `branch_data`). |
| S2 | https://developers.figma.com/docs/embeds/embed-figma-file/ | Embed URLs `https://embed.figma.com/{design\|board\|proto}/:file_key`; "design, proto, board, slides, or deck corresponds to the type of file"; example `www.figma.com/board/<key>/Example-FigJam-board`, `www.figma.com/proto/<key>/...?node-id=2654-15`; query params `node-id`, `m`/`mode=dev`, `embed-host`. |
| S3 | https://raw.githubusercontent.com/figma/mcp-server-guide/main/skills/figma-code-connect/SKILL.md (Figma's own repo) | URL table: `figma.com/design/:fileKey/:name?node-id=X-Y`, `figma.com/file/:fileKey/:name?node-id=X-Y`, `figma.com/design/:fileKey/branch/:branchKey/:name` → "use `:branchKey`"; node-id `X-Y` in URLs is `X:Y` in the API; example key `QiEF6w564ggoW8ftcLvdcu` (22 chars). |
| S4 | same repo, `skills/figma-use/SKILL.md`, `figma-use-figjam`, `figma-use-slides` | "Design = `figma.com/design/...`, FigJam = `figma.com/board/...`, Slides = `figma.com/slides/...`". |
| S5 | https://raw.githubusercontent.com/figma/code-connect/main/cli/src/connect/helpers.ts | `parseFileKey`: `/(?:file\|design)\/([a-zA-Z0-9]+)/`; node IDs `I?[0-9]+:[0-9]+(;…)*`. |
| S6 | Public web-app JavaScript served logged-out by `https://www.figma.com/design/<public docs example key>/` (bundles under `/webpack-artifacts/assets/`, gzip; `figma_app-9b837f4ccec188a9.min.js`, `6413-8e031fdc4fad2bf8.min.js`, string table `figma_app-b446bd325f74b39b.min.en.json`) | The authoritative evidence: Figma's file-key regex, its desktop URL router (every editor route, branch form, slug handling), the URL builder, and the tab-title function. Quotes below. |
| S7 | `curl` of `https://figma.com/design/<22 A's>/x` | `301 → https://www.figma.com/...`: the apex is not a second origin. |
| S8 | `curl` of a public example file from S2 under `/design/`, `/file/`, `/board/` | All three routes serve the same file; `og:url` canonicalizes to `/file/<key>/<slug>`. Server `<title>` is just `Figma` (the real tab title is set client-side). |
| S9 | https://help.figma.com/hc/en-us/articles/360040531773 (Share files and prototypes), 360040318013 (Play prototypes), 360039827134 (Embed), 32132100833559 (MCP) | Copy link behavior ("If you have a frame selected, the copied URL will link directly to that frame"); no concrete URL grammar. |

Search engines were not usable from this host (bot wall), so there are no blog
or forum sources; S6 replaced them and is stronger than screenshots would be.

### Key quotes from S6 (Figma's client code, minified)

- File-key validation: `/^\/(?:file|design|board|slides|site|buzz|rev|make|spec)\/([0-9a-zA-Z]{22,128})(?:\/|$)/` and `iK="[0-9a-zA-Z]{22,128}"`.
- Desktop URL router (`iq(prefix, …)`), generated for `file`, `design`, `board`, `slides`, `site`, `buzz`, `rev`, `weave`, `spec`, `make`, `proto`, `deck`, `presenter`, `export`:
  `${e}/(${iK})/branch/(${iK})(?:/([^/]+))?` with `fileKey: ["match",2]` (the **branch** key), and `${e}/(${iK})(?:/([^/]+))?` with `fileKey: ["match",1]`; title from the slug with `-` → space. Every editor route is normalized to `/file/<key>`, with `type=design|whiteboard|slides|sites|cooper|figmake|weave|figspec`.
- Create routes: `file/new`, `design/new`, `board/new|jam`, `slides/new`, `site/new`, `buzz/new`, `rev/new`, `make/new`, `weave/new`, `spec/new`.
- Not files: `(file|design|…)/<key>/image/<hash>` → open external; `files/…`, `user`, `org`, `team` → file browser; `c|community|@<handle>` → community; `export/<key>` → video export.
- URL builder: `` `/${type}/${key}${opaqueSlugFlag||!name?"":`/${slug(name)}`}` ``, so the slug can be absent (`opaque_file_slug_in_url` flag); branch URLs are `` `${default_file_key}/branch/${branch.key}` `` with the main file's name as slug; `/file/` links get `?type=design`, `/design/` links `?m=auto`.
- Tab title, editor view: `` `${prefix}${name}${suffix}` `` with suffix `" – FigJam"` (Whiteboard), `" – Figma Slides"`, `" – Figma Sites"`, `" – Figma Buzz"` (Cooper), `" – Figma Make"`, `" – Figma Specs"`, default `" – Figma"`. Prefix defaults to `""`.
- Tab title, prototype view: `` `▶ ${name} - ${pageName}` `` when the prototype has more than one page, else `` `▶ ${name}` ``; Slides in prototype view: `` `${Presenter|Audience} \xb7 ${name} - Figma Slides` `` (strings `slides.general.presenter` = "Presenter", `slides.general.slide_deck` = "Audience", `slides.general.slides` = "Figma Slides").
- No file open / loading: `newTitle:"Figma"`. A third variant, `` `⌥ ${name}` `` (no suffix), is used under a condition I could not decode.
- Emoji favicon: in browsers other than Safari (and not in the desktop app), a leading emoji in the title is moved to the favicon and removed from `document.title`.

## Artifact types

### `figma.file` — one Figma file of any product (confidence: high for URLs and ID, medium-high for titles)

URL forms (all on `https://www.figma.com`, key `[0-9a-zA-Z]{22,128}`):

| Form | Example (synthetic) | Mode | Evidence |
|---|---|---|---|
| Design editor / Dev Mode | `/design/<key>/<slug>?node-id=1-2&t=…&m=dev` | none | S1, S3, S6 |
| Legacy | `/file/<key>/<slug>?type=design` | none | S3, S5, S6, S8 |
| FigJam | `/board/<key>/<slug>` | none | S2, S4, S6 |
| Slides editor | `/slides/<key>/<slug>` | none | S4, S6 |
| Sites, Buzz, Make (+ legacy `rev`), Specs, Weave | `/site/…`, `/buzz/…`, `/make/…?fullscreen=1`, `/rev/…`, `/spec/…`, `/weave/…` | none | S6 only (medium: Specs and Weave look feature-gated) |
| Prototype player | `/proto/<key>/<slug>?node-id=…&starting-point-node-id=…` | view | S2, S6 |
| Slides audience / presenter | `/deck/<key>/<slug>`, `/presenter/<key>/<slug>` | view | S2 (deck), S6 |
| No slug | any of the above as `/<route>/<key>` | — | S6 URL builder |

- **ID**: `figma.file:{host}/{key}`. The key is Figma's file key: one key space across products (S6 router normalizes them all to `/file/<key>`; S8 shows one file under three routes). Case-sensitive, never the slug.
- **Why one type**: a design file and its prototype (`/proto/`), a Slides file and its `/deck/`, and a legacy `/file/` link to a FigJam board each share one key. Product-specific types would split one file's history, and a legacy `/file/` link could not be typed at all. The tab-title suffix still names the product.
- **Mode**: `view` for `proto`, `deck` and `presenter`. Editor routes get no mode: the same URL is edit for an editor and view for a view-only seat, and Dev Mode (`m=dev`) is a query flag. Could be refined later with a `query = { m = "dev" }` rule.
- **Titles**: `["▶ {title}", "{~} · {title} - Figma Slides", "{title} – {~}"]`. The last entry strips the final ` – <product>` with one skip (seven suffixes, four-entry limit), so a name containing ` – ` keeps it. Confidence: high that this is the code path (S6), medium that nothing else (A/B flags, localization) changes it for a given user; titles are English-UI only.

### `figma.branch` — a branch of a file (confidence: high for URL and ID, low for title)

- URL: `/<route>/<main key>/branch/<branch key>[/<slug>]` for every route above (S3, S6).
- **ID**: `figma.branch:{host}/{branch}`. The branch key is a file key in its own right (S1: "branch key" accepted as `:key`; S3 and S6 both take the branch key as the file key). The main key is matched with `{_}` and not recorded.
- Separate type, not `within = figma.file`: §8.3 would make the host name every `figma.file` row from its ID (`figma.file:www.figma.com/<key>` → title `<key>`, link `https://www.figma.com/<key>`, which is broken), so a container roll-up costs more than it gives now.
- Title: same function, but what `name` is on a branch (branch name vs main file name) is not confirmed.

## Origin handling

One adapter origin, `https://www.figma.com`:

- `https://figma.com` 301-redirects to www (S7), so it needs no adapter.
- `www.figma.com` is in `SHARED_ORIGINS` (`adapter-templates/src/document.rs`) and the catalog records no tenant position for it (§6). File URLs carry no team, org or account. So both adapters are `unscoped = "acknowledged"`; a scope is impossible by URL. The Capture Site wording should say: "Records every Figma file you open in this Chrome profile, from every Figma account signed in to it."
- `embed.figma.com` (iframes on other sites, S2) is a different origin and not a tab a user "visits"; deliberately left out.

## Routes deliberately excluded

- File browser and lists: `/files/…` (recents, projects, teams, drafts, trash, search, admin, workspaces), `/files/project/<id>`, `/files/team/<id>/…`. Projects and teams are folders, not documents; a `figma.project` starter could come later (`/files/project/<int>` is a clean numeric ID) but is low value.
- Create: `/design/new`, `/board/new`, `/slides/new`, `/site/new`, `/make/new`, `/rev/new`, `/buzz/new`, `/spec/new`, `/weave/new`, `/file/new`, `/jam` (excluded by the 22-character key minimum and by the routes).
- Marketing pages that reuse product words: `/design/`, `/figjam/`, `/slides/`, `/prototyping/` (no key segment).
- Community: `/community/file/<numeric id>/<slug>` (public listings, a different ID space; not a user's own work), `/@<handle>`.
- Sub-resources: `/<route>/<key>/image/<hash>`, `/export/<key>` (video export).
- Embeds: `www.figma.com/embed?url=…` (the key is inside a URL-valued query parameter, which no capture type accepts) and `embed.figma.com`.
- Settings, login, admin, plugins, `/make-local`.

## Known limits and open questions

1. **Title is set client-side.** The server title is `Figma`; until the file loads, no entry matches and `Figma` is recorded (as with Miro's `Miro - Miro`). The worker's wait for a later title update should cover most visits.
2. **Multi-page prototypes** keep ` - <page name>` in the extracted title, and the page part changes as the viewer navigates. `"▶ {title} - {~}"` would fix that but would also cut single-page prototypes whose names contain ` - `; I chose not to lose name text.
3. **Leading `▶`/`Audience · `**: a design file whose own name starts with `▶ ` loses that mark from the extracted title (and keeps ` – Figma`); a name containing ` · ` and ending ` - Figma Slides` would be mis-split. Both are rare.
4. **Emoji-prefixed file names** appear in Chrome tabs without the emoji (moved to the favicon), so the recorded title omits it.
5. **The `⌥ <name>` title variant** (no suffix) exists under an undecoded condition; such titles fall back to the whole tab title.
6. **Localization**: the suffixes (`Figma`, `FigJam`, `Figma Slides`…) look like product names, not translated strings, but `Audience`/`Presenter` come from the string table; the skip entry tolerates translations of those.
7. **`token(22,128)`** admits `-`, `_` and `=`, which Figma keys never contain, so a hypothetical 22+-character hyphenated marketing path under `/design/`, `/board/`, `/site/`… would match. None was found.
8. **Specs, Weave, Buzz, Sites, Make** routes come from code only (S6); some are feature-gated or new products. Including them is cheap and they share the key space.
9. **Branch titles** are unverified (see above). Branch visits are not rolled up into the main file.
10. The desktop app is out of scope (not a Chrome tab).

## Validation

Both engines, with no product code changed:

```sh
cd research/blind/figma
python3 -I check/toml2json.py adapters.toml > check/doc.json
# TypeScript twin (extension/dist built with `npm ci --ignore-scripts && npm run build:extension`, Node 24.14.0)
PATH=~/.nvm/versions/node/v24.14.0/bin:$PATH node check/run-ts.mjs check/doc.json vectors.json
#   document validates
#   ts engine: 52 passed, 0 failed
# Rust host crate, via a throwaway crate that depends on ../../../../../adapter-templates (own [workspace], own target, removed afterwards)
cd check/rust && PATH=~/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH cargo run --offline -q -- ../doc.json ../../vectors.json
#   document validates
#   overlap_report: []
#   rust engine: 52 passed, 0 failed
```

52 vectors: 19 `figma.file` and 9 `figma.branch` positives (with ID, mode, extracted title) and 24 negatives. `brauser adapters eval` would also work but needs the full host binary built in this worktree, which I avoided on the shared host.

## How I would verify with a real account

1. Open one file of each product (Design, FigJam, Slides, Sites, Make, Buzz) from the file browser; record the URL and `document.title` after load, and while loading.
2. In a Design file: Copy link, Copy link to selection, switch to Dev Mode, open version history, open Present (prototype), and check each URL keeps the same key.
3. Open a multi-page prototype and a single-page one; confirm `▶ name` and `▶ name - page`.
4. In Slides: Present (audience) and Presenter view; confirm `/deck/` vs `/presenter/` and the `Audience · … - Figma Slides` title.
5. On an Organization plan, create a branch; record its URL (main key, branch key, slug) and its tab title, and whether the branch key in the URL matches the API `branches[].key`.
6. Open a legacy `/file/<key>` link to a FigJam board: confirm it redirects to `/board/<key>` with the same key.
7. Confirm whether `opaque_file_slug_in_url` is on (URLs with no slug) in the account.
8. Check a file whose name starts with an emoji and one containing ` – `.
9. Check that `/files/…`, `/files/project/<id>` and the community pages are not recorded, and that file keys seen are 22 characters.
10. Repeat titles with the UI language set to something other than English.
