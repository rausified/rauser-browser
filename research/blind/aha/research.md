# Aha! blind starter: research notes

Date: 2026-11-03. This is a blind draft. It uses only public, logged-out sources and this repository. No Aha! account was used, and no browser history was read. Files in this folder:

- `adapters.toml`: the adapter document (7 adapters, format 1), with a comment per rule
- `vectors.json`: 90 synthetic cases (60 positive, 30 negative)
- `validate.py`, `rs/`: throwaway harnesses (see Validation)

## Sources consulted

| # | Source | What it showed |
|---|---|---|
| S1 | https://www.aha.io/api (Introduction, URLs) | The API "must be accessed using the account specific domain `<yourcompany>.aha.io`". Records are addressed by numeric ID (e.g. `5938362174479841842`) or by reference number (`APP-1`). |
| S2 | https://www.aha.io/api/resources/features/get_a_specific_feature, …/epics/get_a_specific_epic, …/ideas/get_a_specific_idea, …/releases/get_a_specific_release, …/requirements/get_a_specific_requirement, …/initiatives/get_a_specific_initiative | Example IDs: feature `PRJ1-1`, epic `PRJ1-E-1`, idea `PRJ1-I-1`, release `PRJ1-R-1`, requirement `PRJ1-1-1`. Initiatives used a numeric ID. "Numeric ID or key of the feature." Response bodies are loaded client-side and were not visible. |
| S3 | github.com/aha-app/aha-services, `spec/fixtures/*.json` (public webhook/API fixtures) | `"url": "http://watersco.aha.io/features/PROD-2"`, `.../requirements/PROD-2-1`, `.../releases/PROD-R-1`, plus `peterklipfel-trial.aha.io/features/INTSANDBOX-4` and `/releases/INTSANDBOX-R-3`. |
| S4 | github.com/aha-develop/aha-mcp (`README.md`, `src/types.ts`) | Note URL `https://company.aha.io/pages/ABC-N-123`. Aha!'s own ref regexes: feature `^[A-Z][A-Z0-9]*-\d+$`, requirement `^[A-Z][A-Z0-9]*-\d+-\d+$`, note `^[A-Z][A-Z0-9]*-N-\d+$`. |
| S5 | github.com/aha-develop/github, `src/lib/extractReferenceFromName.ts` | Prefix is 1–10 characters (`[a-z][a-z0-9]{0,9}`). Shapes: requirement `X-n-n`, epic `X-E-n`, feature `X-n`. |
| S6 | support.aha.io sitemap: about 1,150 of 2,741 articles fetched and searched as text | Aha!'s own in-article links into its account `big.aha.io`: `/products/AKB/pages/AKB-N-1950` (thousands), `/pages/AKB-N-2655`, `/pages/7400459715869298098` (numeric), `/features/AI-2056`, `/requirements/KNB-6438-2?full_screen=…#…`, `/whiteboards/7431998640652670909?width=540&height=340` (an embed), `/bookmarks/custom_pivots/<id>/<id>`. The GitHub-extension article gives the autolink target `https://account.aha.io/records/<num>`. |
| S7 | support.aha.io/aha-roadmaps/support-articles/features/move-records-to-another-workspace | "If you want your features to take on the workspace prefix of their new workspace, consider cloning the release": moved records keep their old prefix and reference number. |
| S8 | support.aha.io/aha-roadmaps/support-articles/workspaces/error-prefix-cannot-be-one-used-previously-… | A prefix can be edited. A prefix cannot be reused while any "features, requirements, ideas, notes or releases" still use it as part of their ID. Existing records keep their reference numbers after a prefix edit. |
| S9 | support.aha.io/aha-ideas/support-articles/ideas-portals/ideas-portal-settings and …/create-custom-portal-domain | Portal URL "follows this format: name.ideas.aha.io" (example `https://big.ideas.aha.io`). An optional CNAME such as `ideas.yourcompany.com`. A duplicated portal gets "a new random URL". |
| S10 | support.aha.io …/portal-notification-emails | `$IDEA_REF` "(e.g. DEMO-I-38)". |
| S11 | https://big.ideas.aha.io (public portal, logged out) | Idea links `/ideas/A-I-24044`; `<link rel="canonical" href="https://big.ideas.aha.io/ideas/A-I-24044">`; tab title `Allow hiding unused record types on the new features \| Aha! Big Ideas`; `/ideas/new` is titled `New idea \| Aha! Big Ideas`. Trailing slash and `?foo=1` serve the same idea. A lowercase ref and a numeric idea ID both redirect to `/portal_session/new` (not found or private). Unknown words under `/ideas/` also redirect to login; `/ideas/search` redirects to `/ideas`. |
| S12 | Logged-out HTTP probes of `https://big.aha.io/<path>` (Aha!'s own account, about 70 requests, no sign-in) | A 302 to `/session/new` means the route exists; a 404 means it does not. **302:** `/features/x`, `/epics/x`, `/epics/x/features`, `/ideas/ideas/x`, `/ideas/x`, `/releases/x`, `/requirements/x`, `/initiatives/x`, `/strategic_imperatives/x`, `/key_results/x`, `/pages/x`, `/notes/x`, `/records/x`, `/whiteboards/x`, `/products/P/pages/x`, `/products/P/features[/x]`, `/products/P/releases[/x]`, `/products/P/ideas`, `/products/P/initiatives/x`, `/develop/features[/x]`, `/develop/epics/x`, `/develop/requirements/x`, `/develop/pages/x`, `/develop/ideas/ideas/x`. **404:** `/features/x/edit`, `/features/x/y`, `/epics/x/y`, `/releases/x/y`, `/requirements/x/y`, `/pages/x/y`, `/whiteboards/x/y`, `/ideas/ideas/x/y`, `/goals/x`, `/master_features/x`, `/products/P/epics[/x]`, `/products/P/requirements/x`, `/products/P/ideas/ideas/x`, `/develop/epics`, `/FEATURES/x` (paths are case-sensitive). The login page title is `Log in to your workspace \| Aha!`. |

Route probes only show that a route exists. They do not show what the page displays, and they never show a record's tab title.

## Origin model

**Two tenant subdomain families, not a shared origin.** Neither needs a §6 scope.

1. **Application: `https://<account>.aha.io`** (S1, S3). Aha! Roadmaps, Ideas (in-app), Develop (`/develop/…`), Knowledge/Notes (`/pages/…`) and Whiteboards all use this one origin. The host name is the account, so a starter for the user's account sees only that account. The family is like `*.atlassian.net`. A suggested `starterKind` check is `host.endsWith(".aha.io")`, with these exclusions:
   - `aha.io` itself;
   - `www`, `secure` (login), `support`, `status`, and asset or infrastructure hosts seen in S6 (`iad-prod1.aha.io` for `expiring_url`, `cdn`);
   - anything ending in `.ideas.aha.io` (portals, see 2) or `.identity.aha.io` (portal SSO).

   Better still, offer the family only for an origin the user has enabled. No regional or EU host name turned up in public sources (open question).
2. **Ideas portals: `https://<portal>.ideas.aha.io`** (S9, S11), plus customer CNAMEs (`ideas.example.com`). No suffix rule can detect a CNAME portal. The portal adapter works on one unchanged if the user enables that origin by hand, because the paths are identical. One account can have many portals, each with its own subdomain. A portal's host differs from the app's, so the same idea gets two IDs (`aha.idea:acme.aha.io/PRJ-I-45` in-app and `aha.portalidea:acme.ideas.aha.io/PRJ-I-45` on the portal). That is honest: these are two different views for two audiences.

`acme` in `adapters.toml` is a placeholder. A starter offer substitutes the enabled origin, as the Jira starter does.

## Stable ID: reference number

- Every chosen record type opens at `/<route>/<reference number>` (S3, S4, S6). The reference number is what users paste and what titles and integrations use.
- It survives renames (it contains no name), moves between workspaces (S7: moved records keep their old prefix) and workspace-prefix edits (S8: old prefixes stay attached to existing records). **Confidence: medium-high.** S7 and S8 imply this but do not state it outright.
- The numeric internal ID (S1, a 19-digit number) also exists and some URLs use it, notably `/pages/<numeric>` for notes (S6). It is not the ID here because the reference-number form is what record URLs normally use (S3).
- Capture types: only the feature ref (`PRJ-123`) has the `jira_key` shape (S4 regex matches §7.1). The others contain a second dash (`PRJ-E-12`, `PRJ-123-1`), and the template language has no type for that, so they use `name(5,100)|upper` behind a literal route word:
  - the minimum length 5 (`A-E-1`) excludes `new`;
  - `|upper` matches Aha!'s uppercase-only refs (S4) and folds a hand-typed lowercase link into the same ID. A lowercase ref does not resolve on the portal (S11), so folding never merges two distinct records.

## Artifact types

### aha.feature: confidence HIGH (URL), LOW (title)

- URLs: `/features/PRJ-123` (S3, S6). `/develop/features/PRJ-123` (S12, route exists; undocumented, medium-low).
- ID: `aha.feature:{host}/{ref}`. `jira_key` rejects route words such as `/features/board`.
- No rest segment: `/features/x/edit` and `/features/x/y` are 404s (S12), so no deeper view is lost.
- Not matched:
  - `/records/<x>`: a generic redirector (S6, S12). The tab ends on the redirect target, which these rules then match.
  - `/api/v1/…`.
  - `/products/<P>/features/<x>`: the route exists for any `<x>`, so it is probably a workspace list view named by a word such as `list`. Unverified and excluded.

### aha.requirement: confidence HIGH (URL), LOW (title)

- URLs: `/requirements/PRJ-123-1` (S3, S6, including `?full_screen=<id>#anchor` noise). `/develop/requirements/PRJ-123-1` (S12).
- Its own artifact, not merged into the parent feature: one segment `PRJ-123-1` cannot be split into the parent ref with the template language, so there is no `within`.

### aha.epic: confidence MEDIUM

- Ref format `PRJ1-E-1` comes from S2 and S5. The route `/epics/<x>` exists (S12), but no public page links to an epic URL.
- Also `/develop/epics/<ref>` (S12).
- Not matched: `/epics/<ref>/features` (a sub-list) and `/master_features/<x>` (404 today).

### aha.release: confidence HIGH (URL), LOW (title)

- `/releases/PRJ-R-3` (S3: two separate fixtures).
- Not matched: `/products/<P>/releases/<x>`, which exists for any `<x>`, like the features case.

### aha.idea (in-app): confidence MEDIUM

- `/ideas/ideas/PRJ-I-45` and `/develop/ideas/ideas/PRJ-I-45` (S12, route exists). Ref format from S2 and S10.
- The single-level `/ideas/<x>` answers for any word (the Ideas module's own pages, such as `/ideas/overview` and `/ideas/portals`), so it is excluded.

### aha.note (Notes / Knowledge documents): confidence HIGH (URL), LOW (title)

- `/products/<P>/pages/PRJ-N-67` and `/pages/PRJ-N-67`: thousands of links in Aha!'s own support site (S6), plus the aha-mcp README (S4).
- `/develop/pages/<ref>`: S12 only, low.
- The workspace segment is `{_}`, so a note moved between workspaces keeps its ID.
- **Known limit:** `/pages/<19-digit id>` also appears (S6). It matches and records a second artifact for the same note, because no rule can map the numeric ID to the ref.

### aha.portalidea: confidence HIGH (URL and title)

- `/ideas/<ref>`, observed directly (S11).
- Title `<idea name> | <portal title>`, matched with `{title} | {~}`. Trade-off: a portal title that contains ` | ` leaves its first part in the extracted title. An idea name that contains ` | ` is kept whole.
- Not matched: `/ideas/new`, `/ideas`, `?page=`, `/ideas/<ref>/idea_comments/…`, `/ideas/<ref>/idea_endorsements/…`, `/portal_session/new`, `/?category=…`.

### Title format for app records: LOW confidence for every app type

No public source shows the tab title of an in-app record page. The login page reads `Log in to your workspace | Aha!` (S12), which suggests a ` | Aha!` suffix. Each app adapter tries two entries:

1. `{ref} {title} | Aha!{~}`: used only when the leading word equals the path's ref (the engines compare them).
2. `{title} | Aha!{~}`.

The `{~}` after `| Aha!` also accepts suffixes such as `| Aha! Roadmaps` or `| Aha! Develop`. If neither entry matches, Brauser records the whole tab title, so a wrong guess only costs title cleanliness, never a wrong ID.

`refs = aha.feature/jira_key` on the app adapters turns `PRJ-77` in a title into a mention of an Aha! feature. With Jira also enabled, the same key shape could mean a Jira issue. A mention resolves only once the target ID has been recorded (§10.1), and that limits the confusion.

## Deliberately excluded

| Excluded | Why |
|---|---|
| Initiatives (`/initiatives/<x>`), goals (`/strategic_imperatives/<x>`), key results | The routes exist (S12), but the reference format is unverified: S2's initiative example used a numeric ID. Lower browsing value. Easy to add once one real URL is seen. |
| Whiteboards (`/whiteboards/<numeric>`) | The only evidence is an embed URL (`?width=&height=`). Unknown whether a full-tab whiteboard uses this path or `/pages/<ref>`. |
| `/records/<x>` | A polymorphic redirector. The landing URL is recorded instead. |
| Drawer views on boards, lists, reports and whiteboards | Support articles say "open its drawer view" many times, but no public source shows whether the drawer changes the URL (query, fragment or nothing). Fragments are out of scope anyway (§2, §13). |
| `/products/<P>/features/<x>`, `/products/<P>/releases/<x>` | The routes accept any `<x>`, so they are probably named list views. A guess would be a confident wrong rule. |
| Roadmaps, reports, pivots (`/bookmarks/…`), settings, `/session/new`, `/api/…`, search, home | Not artifacts, or listed in the spec as skip-worthy. |
| Shared-as-webpage notes and published knowledge bases | Separate share or custom-domain URLs that the public docs do not show. Knowledge bases can live on custom domains (sitemap: create-a-custom-knowledge-base-domain). A later starter. |
| Aha! Discovery and Builder records | Out of the requested scope. Formats not researched. |

## Known limits and open questions

1. App tab titles are guessed (low confidence). They need one live check per type.
2. `name(5,100)` captures accept any 5–100 character word after the route literal. A route word of 5 or more letters directly under `/requirements/`, `/epics/`, `/releases/`, `/ideas/ideas/`, `/pages/` or `/products/<P>/pages/` would be recorded as an artifact. No such word turned up: S12 shows those routes take one ID segment and have 404 sub-paths. On the portal, unknown words redirect to login, and `/ideas/search` redirects to `/ideas`. A dash-required type, or a "must contain `-`" bound, would remove this risk. That would be an engine change (Brauser improvement suggestion).
3. Numeric note IDs record a second artifact (see aha.note).
4. Does the app redirect `/pages/<numeric>` to `/pages/<ref>`? If it does, limit 3 disappears through server-redirect aliases.
5. Are `/develop/...` paths (S12) the real Aha! Develop record URLs, or legacy routes? Medium-low confidence.
6. Converting a feature to an epic, or promoting an idea to a feature, creates a new reference number, so it is a new artifact by design. Not verified whether Aha! redirects the old URL.
7. Custom-CNAME portals and any regional application hosts cannot be auto-suggested by a suffix rule.
8. Not verified whether the in-app idea page and the portal page for the same idea should be linked (the `within` or alias story).

## How I would verify this with a real account

1. Open one feature, requirement, epic, release, in-app idea and note in Chrome. For each, record the address-bar URL after redirects and the exact tab title (`document.title`), including the one shown while the page is still loading.
2. Open each type from a board, list report and roadmap drawer. Note whether the URL changes (query parameter, fragment, or path) and what the tab title becomes.
3. Copy the "share" or "copy link" URL for each type and compare it with the address-bar form.
4. Move a feature and a note to another workspace, and rename both. Confirm that the URL and ref are unchanged and that the old link still opens.
5. Open `/records/<ref>`, `/pages/<numeric id>`, `/products/<P>/features/<ref>` and `/products/<P>/releases/<ref>`. Record where each lands and what it shows.
6. Check `/develop/...` URLs from an Aha! Develop team view.
7. Open an initiative, goal and full-tab whiteboard. Record URL and ref shapes to decide whether they deserve starters.
8. On a private portal and on a CNAME portal, confirm that the idea URL, the title format and the effect of the portal title on `{title} | {~}` match the public `big.ideas.aha.io`.
9. Add every real sample, made synthetic, to `vectors.json`. Re-run both engines.

## Validation

Both repository engines were run over `adapters.toml` and every case in `vectors.json`. Neither run edited product code.

- **TypeScript twin** (extension/dist build, via the repo's own harness `scripts/adapter-templates-eval.mjs`):
  ```
  export PATH=~/.nvm/versions/node/v24.14.0/bin:$PATH
  cd research/blind/aha && python3 validate.py node ../../../scripts/adapter-templates-eval.mjs
  → 90 cases (60 positive, 30 negative): 90 pass, 0 fail
  ```
- **Rust crate** (`adapter-templates`, the host's authoritative engine). The repo's `brauser adapters eval` needs the full host build, so a throwaway standalone crate in `rs/` with its own `[workspace]` path-depends on `adapter-templates` and prints the same result shape:
  ```
  export PATH=~/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH
  cd research/blind/aha/rs && cargo build --offline && cd .. && python3 validate.py ./rs/target/debug/aha-blind-eval
  → 90 cases (60 positive, 30 negative): 90 pass, 0 fail
  ./rs/target/debug/aha-blind-eval --overlap  (TOML converted to JSON on stdin)
  → adapters=7 overlapping_rule_pairs=0
  ```
  `rs/target` was deleted after the run to save disk space. Rebuild it to repeat the run.
- Validation found one real issue, which is now fixed: `aha.portal_idea` failed with `id_syntax`, because ID schemes (§8.1) do not allow `_`. The type is now `aha.portalidea`.
