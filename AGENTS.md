# Brauser: The Rauser Browser Assistant contributor instructions

## Current state

M0 is merged. M1 development code adds a Chrome side panel and settings page,
site capture, native folder selection, and durable visit IDs. The side panel
was then reworked, before M2 (DESIGN.md §12.2 step 4): page notes are no longer
create-only with sibling review drafts. The panel follows the active tab, loads
that page's note on any HTTP(S) page, and autosaves the whole note about a
second after typing stops, on tab change, and on panel close, via
`load_note`/`save_note` protocol messages that reuse `get_config`/`update_config`'s
revision-check pattern. A save based on a stale version is refused; the host
returns the note's current content and the panel shows a copy-out of the text
it could not save. The capture housekeeping controls from M1 collapsed into a
one-line, expandable capture strip. The answer card, Summary section, and full
omnibar are not built (M2/M3).

Unacknowledged note edits now have synchronous browser-local recovery
(`extension/note-recovery.ts`, `brwr-3nq`). Every snapshot has an immutable
key, and native acknowledgements/discards only remove the exact snapshot
they saw. Reopening shows recovered copies beside the current host note with
Copy/Discard; they never enter the editor or autosave automatically. Storage
failures warn without preventing host saves. Preserve that boundary when
changing note identity, save sequencing or panel teardown.

Open access (protocol version 6, PR 1 of
`docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md`)
removed the per-site grant wall. The manifest requests `http://*/*` and
`https://*/*` as install-time `host_permissions` with `webNavigation` and
`scripting` required, and drops `activeTab`; the toolbar action only opens the
panel, and the panel reads the active tab's URL and title on any HTTP(S) page.
If the user withdraws Brauser's site access in Chrome, the panel shows one
site-access banner with a button to re-enable it. Gone with v6: the native Yes/No
confirmation (and the `macos-alert` crate), `confirm_config` and consent tokens,
the AI denylist and its confirmation, `log_incognito`, Chrome grant
reconciliation, the sync caution, and the harness setup dialog and its token.
Settings saves config with the revision check only. A site is on exactly when
it has an adapter: `ConfigSnapshot.sites` is gone, and the host derives capture
origins from the exact origins of a valid adapter document (none while
`adapter_issue` is set). Turning capture on needs only a home folder.
Path-prefix narrowing is dropped. A version 5 `config.toml` loads unchanged and
drops `sites`, `agent_denylist`, `agent_denylist_confirmed`, and `log_incognito`
on its next save. `StorageConfig.knowledge_base_dir` (null means the notes
folder) is the folder a later summary agent reads; it is chosen with the same
native picker, whose token names its `FolderPurpose` (`notes_folder` or
`knowledge_base`) so one selection cannot authorize the other, and the host
records the folder's identity as it does for the home folder.

The home folder (2026-10-27, `brwr-hme`): `storage.root` is the one folder
the user picks, and the content folders inside it are fixed
(`host/src/layout.rs`: `notes/`, `logs/`, `artifacts/`, `summaries/`,
`later/`). The wire `StorageConfig` keeps its `*_dir` fields with those
values, and `config::validate_update` refuses any others, so settings has no
per-folder fields and harness setup no summaries name. An older config.toml
is moved once by `ConfigStore::migrate_layout` (host start, `hello`,
`get_config`, and a same-folder save in `update_with_grants`): under the
config and capture locks it renames each existing older folder no-clobber,
records a `log/` rename in `visit-ids/log-relocations.json` for intent
replay (`CaptureStore::open_original_day_dir`), then writes the fixed names.
A fixed name already present, or a nested, symlinked, or case-variant older
folder, refuses with a `config_issue` and changes nothing; until the move
succeeds the older layout is never used for vault I/O.

The Brauser service skeleton (PR 2 of the open-access spec, DESIGN.md §12.2
step 10.2) is built on `feat/agent-service-pr2` in `host/src/service/`
(built on every Unix, compiled out on Windows; only the launchd glue is
macOS-only, so the service and its tests build and run on Linux, where the
support directory is the XDG config directory `~/.config/brauser` and the host
never starts the service itself: it uses one started with `brauser service`
and otherwise answers `service_unavailable`). On macOS `brauser service` is a per-user
LaunchAgent (`com.rauser.brauser.service`) that listens on a Unix socket in
Brauser's support directory with the native-messaging framing, and exits after
10 idle minutes. The short-lived native host forwards `start_run`, `get_run`,
`list_runs`, `cancel_run`, and `retry_save` (`host/src/service/client.rs`),
resolving the record key itself; it starts a missing service with
`launchctl kickstart`, checks the service's build identity with
`service_hello`, and answers `service_unavailable` when the service is not
installed or stays on another build. A host older than the binary on disk
never restarts the service; it asks for an extension reload instead. The service keeps one record directory
per run under `runs/`, runs at most two at once with one queued-or-running run
per record key, times runs out after 10 minutes, kills the process group on
cancel, marks orphaned runs failed on restart, prunes old records, and overlays
an optional `agent.env` on each run's environment. The panel polls
`get_run`; there is no host-side streaming protocol.
`node scripts/register-dev-host.mjs` now also writes and bootstraps the dev
LaunchAgent; `--no-service` skips it and `--unregister-service` removes it.

Summarize & log (PR 3 of the open-access spec, DESIGN.md §12.2 step 10.3)
is merged (#98). Harness setup runs in the service:
`discover_harnesses` and `set_up_harness` are forwarded to it, it detects
`codex` and `claude` on its own `PATH` plus `$HOME/.local/bin` (an owned,
non-writable install and a parseable `--version`, with no version refused),
and it commits the chosen harness to `config.toml`'s `[summarizer]` table
(wire `ConfigSnapshot.agent`) only after a probe through the real runner
passes in the knowledge base folder. A v5 `[agent]`/`[harness_record]` file
loads as not set up. The registered runners are Codex as a `codex app-server
--listen stdio://` JSON-RPC thread (`approvalPolicy: "never"`, `sandbox:
"danger-full-access"`, and the answer schema as `outputSchema`) and Claude Code
as `claude -p --output-format stream-json --verbose --include-partial-messages
--dangerously-skip-permissions` with the prompt on stdin and a resumable
session. Both run with full access in the knowledge base folder, with the
service's environment plus `agent.env`, on the user's normal login. The §6.4
prompt names the run's `page.md` by absolute path, marks it untrusted, and
lists the editable `summary_vocabulary`; `host/src/answer.rs` validates the
final JSON (one repair turn, then `invalid_output`) and drops `related_notes`
outside the knowledge base; `RunView.answer` carries the validated type,
category, and importance. `open_path` opens a Codex thread (`codex://threads/<id>`) or
reveals `agent.env`, whose names (never values) `get_agent_env` lists. The
lockdown is gone: no argv templates, `--help` or feature checks, emptied
run environment, environment allowlist, or host-owned work directories. The
settings page detects, tests and chooses a harness, sets its timeout, shows
the setup warning, `agent.env` names, and the vocabulary. Every HTTP(S) page
gets **Summarize & log** in the panel, a `summarize` command
(Command+Shift+9 on macOS) and a "Summarize with Brauser" context-menu item:
the extension injects the pinned, hash-checked Readability 0.6.0
(`dist/vendor/readability.js`) and a self-contained extractor into every
frame (`allFrames: true`; `extension/summarize.ts` `choosePage` keeps the top
frame's text unless it is short and a subframe, such as Word for the web's
editor iframe, holds more; the title and address check stay top-frame), caps
the text in encoded bytes, and sends `start_run`. A polling run
card shows state, elapsed time, the streamed text as text only, Cancel,
Retry save, Copy, Open in Codex or Copy resume command, and the
type/category/importance line, and cards return from `list_runs` when the
panel reopens. The headless smoke run covers setup and runs against fake
`codex` and `claude` scripts.

Summary outputs (PR 4, DESIGN.md §12.2 step 10.4) are built on
`feat/summary-outputs-pr4`. The service's `SummaryFinisher`
(`host/src/service/finisher.rs`) saves a validated answer under the
configuration current at finish, holding the config lock: it rechecks the
home folder and layout, re-derives the record key and follows a changed one
(`written_record_key`), writes `summaries/<note stem>.summary.md`
(`host/src/summary.rs` `render_summary`, `Vault::write_summary`), then appends
a `summary` daily-log event whose marker carries `record_key`,
`artifact_type`, and `harness` and attaches to the adapter's artifact on a
recognized page or a `web.page` artifact otherwise. The event time is the
run's `answered_at` stamp in the offset of the click's `requested_at`, so a
Retry save rebuilds the same file and log line; the supervisor re-stamps an
answer the daily log's window no longer accepts, and failures are plain
reasons that say what to do. Summaries log while capture is paused. Agent
body text goes through `markdown::neutralize_remote`, a context-free escape
(`<` as `&lt;`, `\!` before `[`, and backticks, `$`, tabs, `~` runs of three
or more and code-length space runs as numeric entities), single-line fields
through `markdown::inline` or wikilinks, and front matter through
`markdown::yaml_string` with no entities; `npm run test:markdown-neutralizer`
renders the shared corpus with markdown-it, micromark and marked, in CI too.
The index cache is format 8 (`STATE_VERSION`; older formats 4–7 retain event
checkpoints and perform one bounded gap rescan, with unknown URL provenance
for old records); summaries never count as opens or opened days, artifact
files gain a Summary line and date-ordered
Timeline summary lines, and Recent rows,
`get_artifact` (an unrecognized page included, quietly null on a failed
lookup), and the worklog carry them. `open_summary` opens only the record's
owned summary file through the native host's `SetupContext.open` (PR 3's
opener). The panel shows the page's saved summary, summarized Recent rows,
and the run card's saved-summary line (one retried lookup), each with **Open
summary** through `ConfigSession.openSummary` or the card's host call.

Worklog gap review (`brwr-9r7`, 2026-10-28) uses additive protocol-v8
`worklog_report` / `worklog_reported`, leaving the original worklog pair intact.
The same caught-up snapshot supplies measured text and up to 32 owned overflow
notices (explicit truncation). Each full count/range is shown on every day/week
its recorded local endpoint dates enclose; overlapping notices stay separate,
retry IDs deduplicate, and missing records never become measured activity.
The panel always reviews notices and offers a checked-by-default Include
capture gaps choice; Copy uses the exact preview. Cache format 8 keeps system
notices separate from artifact events, with separate checkpoints so older
caches recover previously skipped overflow in one bounded rescan. Reindex
preserves notices; SQLite artifact tables are unchanged.

M1.5a is built on top of that. The adapter-template language
(`adapter-templates.md`) is built as a Rust crate (`adapter-templates/`) and a
TypeScript twin (`extension/adapter-templates.ts`) checked against each other
via the shared vectors in `protocol/adapter-templates/vectors.json`
(`npm run test:adapter-diff`). `ConfigSnapshot.adapters` round-trips as
`[adapters] format = 1` plus `[[artifacts]]`; the host validates it with the
Rust engine and reports overlap warnings or an adapter-only policy issue. A
missing table defaults to an empty format-1 document; an unsupported format or
unknown future `[adapters]` key is preserved while automatic capture is
suspended. Existing adapter IDs are frozen until alias migrations exist.
Settings lists each site with an adapter, with its adapter review workspace and
development starter drafts for Jira Cloud, SharePoint/OneDrive, and scoped
GitHub origins; the drafts still require the documented live-tool checks before
release certification.

The enabled adapter document now travels in the worker's revisioned policy
lease. The TypeScript worker validates and compiles it before buffering and
prefilters unrecognized or out-of-scope navigations; the host independently
rechecks each delivered event with the Rust engine before vault I/O. Origins a
pre-v6 build removed or saw revoked are carried into one pending-removal list,
and the worker keeps their adapters out of capture until a host-sent lease no
longer holds them. Automatic
capture records offset-qualified hourly artifact activity instead of creating
new M1 visit rows: cold opens, focused and background time, modes, refs, and
trails freeze into idempotent `activity` events, with append-only `why` events
for explicit context. Older buffered M1 visits still drain after upgrade. The
worker groups up to 64 same-local-day records under a stable batch ID and
overflow snapshot; the host validates the complete batch, then uses one
compact crash-recovery intent, one append, and one log sync. Queue pressure
coalesces compatible undispatched activity without exceeding the byte cap,
lights the toolbar badge at 80%, and leaves durable overflow evidence. The
native host now updates a rebuildable SQLite `index.db` only after durable log
delivery and lazily regenerates ownership-checked artifact Markdown when the
panel's connection closes and on the first connection of each local day
(never on worker-sync or settings connections), checkpointing long passes. `brauser reindex` recreates both from the owned log
markers; a catch-up or rebuild larger than one pass's 64 MiB read bound saves
its progress and resumes on the next pass (DESIGN.md §6.7). The panel now exposes the current artifact card and one-line context,
a cross-tool Recent list, and copyable daily or weekly worklogs. Notes resolve
to host-derived artifact keys (with guarded migration of older URL-keyed
notes), and settings can regenerate artifact files while reporting every
unowned-path conflict it leaves untouched.

After adapter/site removal, recognition narrowing, or while the adapter
document is inert, note keys come from the note-only contracts still recorded
(`[[adapter_id_history]]`, the inert document, and contracts removed from
config.toml by hand, which the host keeps in its own `adopted-contracts.json`
state file; never capture), so every route a contract matched keeps one note
(`ConfigStore::note_key_rule`). Hand-removed contracts also take part in the
frozen-ID check (`validate_adapter_transition_with_history`, via
`retired_by_hand`). An unusable state file makes identity on every page the
current policy does not recognize unavailable (`NoteKeyRule::StateUnavailable`,
whose refusal names the file), and refuses adapter additions until it is
repaired; a damaged entry does both for its origin only. `config_result`
reports it as `adopted_state` (issue, revision, and recording warnings), and
settings' **Forget recorded adapters** (`forget_adopted_contracts`,
`ConfigStore::forget_adopted_contracts`, `extension/adopted-state-settings.ts`)
backs the file up and records afresh under the config lock. Those contracts alone decide an origin they
cover, so a stripped canonical URL shared with another page (a Jira board and
its issue modal) is never identity evidence, and older-note candidates must
map to the same artifact. An origin with no recorded contract uses the URL
key with no history lookup or note scan: its recorded URLs are stripped forms
and cannot name an artifact, and its URLs never count as older-note
candidates. Ambiguous contracts fall back to the URL key, while an
uncompilable recorded contract refuses loads and saves instead of guessing an
autosave target. A save that would make `strip_params` remove a query
parameter an adapter rule reads (a `query` key or `absent` key of a current,
history, or adopted contract) is refused as `invalid_config`
(`config::validate_strip_params_readers`, brwr-f4s); a config.toml that
already has one still loads and saves, and settings derives the warning from
the snapshot and offers **Stop stripping**.

Explicit preserved-note viewing (`brwr-r88`, 2026-10-28) is separate from
note-key resolution. The panel's **Preserved notes at this address** disclosure
uses additive protocol-v8 `list_preserved_notes` / `preserved_notes_listed`
and `view_preserved_note` (answered by `note_loaded`). It uses the existing
bounded vault scan, verifies an artifact note at its owned path and exact
stored canonical address, and lists at most 24 titles/IDs. The viewer is
read-only and copyable; it never enters NoteEditor, migrates a file, restores
capture, or treats a canonical address as autosave identity. No index or
recorded contracts are needed; ordinary uncovered-page note loads still do
no scan. Collapse/navigation clear results and invalidate pending reads.

Removed-site restoration (`brwr-cua`, 2026-10-28) uses additive protocol-v8
`get_site_restore` / `site_restore_loaded` before a new starter or draft, on an
explicit Capture Site click. The host returns only usable recorded recognition
contracts for that exact unconfigured origin, deduplicating identical entries
and refusing damaged or overlapping contracts (container overlaps included).
The panel names page kinds and fixed account scopes, re-reads the set before
an explicit revision-checked save, preserves global pauses, and uses the
existing note-save/recovery boundary. Title cleanup and title-derived aliases
or references were not recorded and are not reconstructed. No history is
forgotten, note migrated, page content read or capture resumed implicitly.

The panel's Capture Site flow now sets up previously unconfigured Jira Cloud,
SharePoint/OneDrive, and GitHub origins using the existing development starter
catalog. It reviews the current page, illustrative examples and fixed safety
probes inline. GitHub now offers repositories alongside pull requests and
issues, all three checked by default and sharing one organization field
(2026-10-27) instead of separate PR and issue lists; the field is pre-filled
from the current URL's owner segment (also `/orgs/<org>/…`), lowercased and
shown for the user to confirm or edit rather than applied unseen, and a page
with no usable owner (the GitHub root, a reserved top-level route, `/orgs`
with no org) gets no pre-fill and asks instead. The confirmation line names
only the types actually checked, so it is never wider than what is saved. It
then saves once through revisioned `update_config`. It preserves configured
sites and global pauses, refuses pending local pauses/removals and stale
reviews, and saves old note edits before reloading their host-derived identity.
Installing policy from a page seeds already-open recognized tabs for attention
measurement without inventing cold opens; background renewals retain their
normal navigation accounting.

GitHub repositories as container artifacts (2026-10-27): the adapter-template
language gained a general mechanism for an adapter to declare
`within = { type, id }`, naming a container artifact its own matches sit
inside (adapter-templates.md §3, §8.3), and a required path capture a `scope`
key restricts to a literal list now counts as an anchor segment too (§4.1,
§6), not only a literal or a fixed-shape capture. `github.repo` is the first
adapter to use either: a new starter, a catch-all over everything inside a
GitHub repository that no PR or issue adapter claims, scoped to the owner
like the other two, with `github.pr` and `github.issue` now declaring
`within` it. `orderContainedFirst` (`extension/starter-adapters.ts`) keeps PR
and issue adapters ahead of the repo catch-all on every save path (the panel,
settings, and the JSON editor), because adapters on one origin are tried in
config order and a PR or issue visit already rolls up into its repository.
The host's index (`host/src/index.rs`) derives each contained visit's
container by re-evaluating the logged URL, not by recording it at log time,
so existing PR and issue history rolls up into repositories retroactively; a
`within` edge records the relationship, worklog and day totals count a
rolled-up visit once, and a container whose own page has never been opened
directly still gets a title and URL derived from its ID instead of a stale or
missing one. The route-history guard in `host/src/config.rs` is amended so a
declared container may overlap the routes of a retired or removed contract it
is exactly the container of, which is what lets settings add `github.repo` on
top of already-configured PR/issue adapters (adding `within` to them in the
same save) without tripping the existing-history freeze. Protocol version 7
exposes the relationship to the panel: `ArtifactView.within` and `.contained`
on `get_artifact`/`list_artifacts`, with the card showing "In <repo>" on a
contained artifact's own card and "Worked on inside" with its rolled-up
contents on a container's card. Downgrading after adding repositories is
unsupported: older hosts reject `within` in the adapter config and in adapter
history.

Single-event recovery (`brwr-3cc`, 2026-10-28): new per-event intents use
format 2 with prepared bytes (512 KiB cap), their hash and a renderer-independent
payload hash; the full JSON journal is capped at 1 MiB. Single and batch
retries check that identity and exact torn bytes before resuming. V1 hash-only
intents retain two frozen renderer generations in `host/src/capture/legacy.rs`:
before the presentation sanitizer, and before inline math/code escaping
(`brwr-2za`). New daily-log titles and Why text use `markdown::inline`. Never
remove the frozen recovery compatibility when changing presentation.
Complete visit markers still deduplicate by ID;
summary retries require the same payload and selected folder too.

Stripped-parameter provenance (`brwr-srp`, 2026-10-28): canonicalization
records the sorted, distinct decoded names actually removed, never values.
New daily markers add `stripped=<hex JSON>` with main/mode/trail evidence;
wire event shapes and the daily-log header are unchanged. Cache format 8
keeps it beside each event and SQLite's `url_provenance` table exposes it
(JSON arrays, SQL NULL for legacy unknown). Notes and summaries record
`brauser.stripped_params`; note saves and migrations preserve creation
evidence, including a legacy missing field. Older-note discovery/view/migration
rejects evidence that removed any current or recorded contract's query/absent
key on that origin. It never chooses a primary autosave key. New single-event
intents are v3 and bind that evidence; retain the frozen v2 hash and all three
pre-provenance v1 presentation candidates for exact recovery. Provenance must
never retain removed values or an extra raw URL.

Incoming navigation (`brwr-1g2`, 2026-10-28): protocol version 8 adds required
`ArtifactView.incoming_trails`, up to five host-derived source/transition pairs
ordered by latest recorded local day, then source ID and transition. The panel
shows text-only source titles, explicit HTTP(S) links and navigation wording.
Unknown legacy transitions, back/forward, unresolved sources and non-trail
relationships never supply provenance. Empty results say no known incoming
source was recorded; log and cache formats are unchanged.

The separate **Page patterns** panel review now inspects a URL and up to 500
same-page anchors on an explicit click, in the top frame's isolated world.
Its self-contained shaper (`extension/site-patterns.ts`) replaces identifier
and query values with fixed placeholders, keeps only short lowercase route
words, generalizes link-only singleton literals and returns at most 50 link
shapes. Link shapes stay only in the review and are discarded on navigation,
close or collapse. The worker independently samples at most 20 visited shapes
per actively enabled origin for seven days in trusted-only local storage;
settings can show or clear them without the host. Private tabs are excluded,
pauses/removals purge samples, and samples never enter delivery or the vault.
Those shapes are also the only page-derived input to unknown-site capture.

Capture Site for unknown sites (`brwr-t40`, 2026-10-27, on
`feat/capture-site-unknown`) adds two host-only request/response pairs
within protocol v6, `draft_site`/`site_drafted` and
`check_site_draft`/`site_draft_checked`; no existing message changed shape.
Only the exact origin and private shapes cross (the current page's shape plus
the worker sample, and at most 50 click-time link shapes); no raw URL, link
`href`, or tab title reaches the host. The host's deterministic structural
drafter (`adapter-templates/src/structural.rs`) runs Filter, Group, Keep, Map,
Name and Rank, and every drafted rule then passes the per-rule checks
`check_draft_rules` (`adapter-templates/src/draft.rs`): a failing drafted rule
is dropped with its reason and the rest kept, a user-written rule only warns,
and a shared origin with no agreeing scoped adapter gets pending scope rather
than a refusal. Before a drafted rule's first identifier the anchor is a
literal or the scoped shared-origin tenant (#126's scoped-capture anchor;
`scope_anchored` rules cannot be saved unscoped). The review pre-fills the
tenant from the tab's URL as "Capture for <tenant>", already chosen, so Save
confirms it in one click (John, 2026-10-27), but only when a kept drafted rule
scoped to that value records the tab's page (`tenantPrefill`). The probe paths, search and paging keys, blocked route words,
mode words, fill values, starter types and shared-origin tenant/account-index
positions are one host-owned catalog, `protocol/capture-site/catalog.json`,
that the extension's tests pin. The panel's review is plain language with the
real current page evaluated locally; scope, `unscoped` and a title entry are
panel-owned overlays applied after `check_site_draft`, and scope travels as a
separate field so it never trips `draft_forbidden_key`. The save re-checks the
selection against the reviewed revision and then saves with the usual
revision-checked `update_config`. On a configured origin a recurring-pages
hint (an unrecognized sampled shape seen three times) drafts additions only;
existing adapters are never edited. **Open advanced settings** hands the draft
to the adapter editor through a 30-minute, read-once `storage.session` entry,
where an edited rule becomes user-written. Drafted rules never re-map an
already-recognized artifact: a shape the current policy or a recorded
contract (retired or hand-removed) recognizes is listed as already captured,
a rule that may overlap one is dropped, and `draft_site` refuses a draft
whose `remapped` is not 0. Secret and capability query keys (catalog
`secret_query_keys`) never become IDs, blocked route words are matched by
their hyphen/underscore parts and plurals and at the shared-origin tenant
position too, and a link shape never renames or reranks a visited kind. Known
limits: a Notion page with no workspace segment is not drafted (workspace
pages draft behind the scoped workspace, their anchor; `brwr-gf1` as
revisited, DESIGN.md §5.1), and on a shared origin a short
lowercase repository or namespace name stays a literal, so its rule covers
that one name. Starter live-tool certification remains human work, and
re-learning and re-keying (`brwr-6x2`) are not built.

Capture Site Improve with AI (`brwr-f45`, 2026-10-28, on
`feat/capture-site-agent`, DESIGN.md §5.1 and §12.2 step 7.7) refines a shown
structural draft, including the recurring-pages hint's, with the default
harness through the service; the structural draft always comes first and
stays savable, and **Use basic draft** returns to it. Additive within
protocol v7: the extension sends `start_site_draft` with the origin and
shapes only; the host re-runs the structural drafter, builds the agent's
untrusted-data block itself, and forwards a `start_run` with
`kind: "site_draft"` (a `kind` from the extension is refused). The service
validates only the answer's form (`host/src/site_draft_answer.rs`, one
repair turn, then `invalid_output`) and writes nothing; the panel then sends
`draft_site` with the run's `run_id`, and the host judges the tables with
`structural::check_agent_draft`: a forbidden key or a foreign origin rejects
the whole agent draft before any preview, and every other rule passes
`check_draft_rules` as `Drafted`. The answer is `site_drafted` with
`drafter: "agent"`; a rule no seen shape matches starts unchecked as
`unseen`, and the save is the unchanged `check_site_draft` plus
`update_config`. Without a harness the button is disabled with Summarize's
setup hint; on Windows the run messages answer `service_unavailable`.

The service worker delivers buffered events and renews its policy lease
itself, from `alarms` and at browser startup, so capture does not depend on
opening the panel (DESIGN.md §3.1); the panel's Send pending visits asks the
worker to sync now. The rest of the agent integration, search index, signed
installers, Chrome Web Store listing, and public release do not exist yet. M1
still needs interactive Chrome checks on macOS, the primary platform. Windows
support was reinstated on 2026-10-16 (DESIGN.md §13, §14): Windows code must
keep compiling and passing CI, and its interactive checks are separate
[human] work.

DESIGN.md's principle 9 (standalone product) governs this repo: no file, build
step, test, or default here may assume AMOS/ENDURANCE, Backstaff, or Astrolabe
exist, even though development happens inside the ENDURANCE vault. No AMOS
inventory entry or feature briefing exists yet for Brauser (checked via
`grep -ril "brauser\|rauserBrowser" AMOS/inventory AMOS_docs` from the ENDURANCE
root); creating one is a separate follow-up, not covered here. Use
[DESIGN.md](DESIGN.md) for intended behavior and update it when implementation
changes a contract.

## Repository map

- `protocol/schema.json` defines native messages. Run `python3 protocol/generate.py` after changing it and commit the generated Rust and TypeScript files together.
- `adapter-templates/` is the Rust crate implementing the adapter-template grammar specified in `adapter-templates.md`; `extension/adapter-templates.ts` is its TypeScript twin. Keep both in sync with the shared vectors in `protocol/adapter-templates/vectors.json` — run `npm run test:adapter-diff` (`scripts/adapter-templates-diff.mjs`) after changing either engine; CI also checks `adapter-templates/fuzz` compiles.
- `adapter-templates/src/shape.rs` parses private URL shapes (the Rust twin of `isUrlShape`), `adapter-templates/src/structural.rs` is the Capture Site structural drafter, and `adapter-templates/src/policy.rs` holds `DraftPolicy`, the plain-struct form of the host catalog; the per-rule checks are `check_draft_rules` in `adapter-templates/src/draft.rs`. `host/src/capture_site.rs` parses the catalog and handles `draft_site` (structural, or an agent draft's judgment with `run_id`), `start_site_draft` and `check_site_draft`; `structural::check_agent_draft` judges an agent's tables with the same per-rule checks.
- `protocol/capture-site/` holds `catalog.json` (the host-owned drafting catalog) and `shapes.json` (real URL forms and their expected shapes, shared by the Rust parser and the in-page shaper's tests).
- `protocol/strip-params/vectors.json` pins the `strip_params` matching rule shared by `matches_param` (host/src/capture.rs) and `stripsParam` (extension/adapter-review.ts); both test suites read it.
- `host/` contains the native host. Keep host policy checks on the privileged side of the messaging boundary. `host/src/consent.rs` holds the purpose-bound folder-picker grants and the `update_config` field checks; `host/src/dialog.rs` runs the native folder picker, the only native dialog. There is no `privacy.rs`, `extension/grants.ts`, or `macos-alert/` crate any more (removed with protocol v6).
- Every first-party crate is `#![forbid(unsafe_code)]`.
- `host/src/answer.rs` validates the structured summary answer and `host/src/site_draft_answer.rs` the form of a Capture Site agent's answer (`parse_site_draft_answer`); `host/src/open.rs` is the platform opener (`/usr/bin/open` on macOS, a log file in test builds). `host/src/service/` holds the service: `child.rs` (line-streaming child processes), `codex.rs` and `claude.rs` (the runners), `prompt.rs` (the §6.4 `summary_prompt`, plus `site_draft_prompt` and the host-built site-draft input block with its marker escaping), `setup.rs` (detect and probe), `open.rs` (`open_path`, `get_agent_env`), and `finisher.rs` (PR 4's `SummaryFinisher`, and `SiteDraftFinisher`, which writes nothing), beside PR 2's server, supervisor, queue, records, and client. There is no `harness_setup.rs` any more. `host/src/markdown.rs` holds the shared Markdown safety helpers (`yaml_string`, `inline`, `wikilink`, `neutralize_remote`, `relative_link`) and `host/src/markdown_corpus.rs` checks them against the shared corpus; `host/src/summary.rs` names, renders, and recognizes summary files and validates summary events.
- `extension/` contains the MV3 Chrome development build. `npm run build:extension` emits `extension/dist/` for unpacked loading, including the pinned `vendor/readability.js` it copies from `node_modules` after checking its hash. `extension/extract.ts` is the self-contained page extractor, `extension/summarize.ts` starts a run from a tab, and `extension/run-card.ts` is the panel's polling run card; `extension/summary-text.ts` words a saved summary for the panel, Recent, and the run card.
- Product naming (app name, native host ID, storage key, marker, and temp-file prefixes) lives in `extension/brand.ts` (also used by the build and dev scripts) and `host/src/brand.rs`. Page and manifest text use `{{APP_NAME}}`/`{{FULL_NAME}}`, filled in by `extension/build.mjs`. Crate, package, and binary names in `Cargo.toml` and `package.json` still change separately.
- `scripts/register-dev-host.mjs` registers a per-user development native host for one unpacked extension ID; installers own release registration.
- `.github/workflows/ci.yml` runs the build and quality checks from the reusable `.github/workflows/build-os.yml` on `macos-15` and `windows-latest`, with the headless smoke run (`npm run smoke:macos:headless`) and fuzz-target compile check on macOS only, and `BRAUSER_REQUIRE_IPC_TESTS=1` so socket tests never skip. When GitHub cancels a starved `macos-15` job (cancelled with zero steps run, checked through the Actions API), the same steps rerun on `macos-15-intel` (x64; a separate runner pool from the arm64 images); a real failure, timeout or interruption of `macos-15` does not fall back. The `Build and check` gate job, the check branch protection requires, passes only when Windows passes and macOS passes on `macos-15` or, after that cancellation, on the `macos-15-intel` fallback.
- `TESTING.md` is this repo's own testing strategy (headless end-to-end recipe, dialog seams, what headless cannot prove). It is self-contained; nothing in this repo should point outside it for testing guidance.

## Local checks

Run the commands in [README.md](README.md#development) before proposing a code change. The Rust version is pinned in `rust-toolchain.toml`. `deny.toml` is the Rust dependency policy (license allowlist; no wildcard versions, unknown registries, or unknown git sources) that CI's `cargo-deny` job enforces — run `cargo deny check --all-features --locked` before adding or upgrading a Rust dependency. After changing either adapter-template engine, run `npm run test:adapter-diff` (differential testing against the shared vectors, `adapter-templates.md` §12).

For end-to-end checks, use `npm run smoke:macos:headless`; it never shows a window or takes focus. Only when the user asks for it, run `npm run smoke:macos -- --auto` instead (real dialogs, UI scripting; warn them before it takes focus) — `--auto` and `--headless` are mutually exclusive flags of the same script, not combinable. See [TESTING.md](TESTING.md) for what each check (`smoke:macos:headless`, `probe:m15a`) actually does, how to run the real-dialog variants, and what headless cannot prove — it is the single, self-contained source for testing guidance; update it with anything you learn here.

## Review and merge cadence

These rules exist because repeated review passes, main moving under open PRs, and strictly serial review chains cost more time than building (2026-10-27).

- **One Codex pass.** Run `codex exec review --base origin/main` once, after the full gate. Fix every P1 test-first. File each P2 and P3 as a bead (with the finding text and file) instead of fixing it in the PR, unless the fix is a few lines and needs no new test seam. Run a second pass only when the P1 fixes changed behaviour substantially; never a third.
- **Run the Codex review alongside CI**, not after it. Both start on the same pushed head.
- **Announce the final stretch of a big PR.** Before merging main into a PR that touches more than about ten files or a shared seam (protocol schema, `adapter-templates`, `service/`, `config.rs`), tell the other active sessions so they hold merges that touch the same files until it lands. Whoever merges second resolves the conflicts.
- **Focused checks per task, the full gate once.** In multi-task builds, each task runs only the tests for what it touched (the named tests, the affected crate or extension test file, plus fmt and clippy). The full gate in README.md runs once, before the PR review, and again only after merges or fixes that touch other areas.
- **Overlap per-task review with the next task** in multi-task plans: review task N while task N+1 is being built, unless N+1 depends on code the review may change. Batch small, low-risk tasks (docs, wiring) into one review.
- **Check scope before a large build.** For a plan of more than about five tasks, list the user-facing decisions it assumes and confirm any that are open before building.

## Security rules

- Treat all extension messages and page-derived content as untrusted.
- Keep vault paths rooted in a user-selected directory; never accept a raw file path from a page or extension request.
- Preserve existing user files. Page notes, generated artifact files, and summary files are the three implemented file kinds Brauser replaces whole. A page note is replaced only after checking the caller's version and the file's recorded canonical URL and Brauser ownership (`host/src/note.rs`); an artifact is replaced only after finding exactly one matching `brauser.kind: artifact` and `artifact_id` in its frontmatter (`host/src/index.rs`); a summary is replaced only when the `brauser:` block in its leading frontmatter is `kind: summary` with this record's `url_id` or `artifact_id` and not the other key (`summary::owned_summary`, through the shared `replace_owned_file` in host/src/vault.rs). An unrelated or unowned file at any of these paths is a conflict, never adopted or overwritten; summary files are never adopted. Every other content location (log, later) stays create-only.
- Keep capture off until a home folder is chosen and the host has a valid adapter document; automatic capture runs only on the exact origins of that document's adapters (`ConfigStore::capture_origins` in host/src/config.rs, `is_authorized` in host/src/capture.rs). Any stored config whose home folder was never confirmed through the native picker, or whose folder no longer matches its recorded identity (unmounted or remounted drive), must block capture and require reselection through the native picker before it can be used again (host/src/config.rs, `DiskState` load path). The knowledge base folder is set only through its own purpose-bound picker token; a picker token is valid only for the `FolderPurpose` it was minted for (host/src/consent.rs). Its identity is checked against the folder when it is chosen and recorded at save time (`update_with_grants` in host/src/config.rs); later saves that leave it unchanged carry the recorded identity without touching the folder, so a missing knowledge base never blocks capture or unrelated settings. The service re-checks that identity before every run and refuses a missing or replaced knowledge base folder (`not_configured`, host/src/service/server.rs), and checks it again under the config lock when a queued or resumed run leaves the queue, failing that run before its harness starts (`cwd_check`, `ServiceOptions::run_cwd`).
- The host re-checks every delivered event against its own adapters with the Rust engine before any vault I/O; a worker can only narrow what is recorded, never widen it. Never let a Chrome permission, a worker claim, or extension state substitute for that check. That includes every activity trail source: the host rejects the event rather than drop a trail, so the worker strips trails whose source left the lease before dispatch (`trailsUnder` in extension/worker.ts) and never edits a dispatched event.
- Carry pending site removals across upgrades. Origins an earlier build removed or saw revoked are migrated into the worker's one pending-removal list (`migratePendingRemovals` in extension/worker.ts), their adapters stay out of every capture lease, and the list clears only when a lease the host actually sent holds no adapter on those origins. An unreadable list pauses capture (fail closed).
- Only harness setup (`set_up_harness`, host/src/service/setup.rs) may set the harness entry; it names a `HarnessId`, never a path or arguments, and the service detects that harness again and probes it through the real runner before committing. `update_config` may only echo the harness entry unchanged or remove it (host/src/consent.rs `check_agent_fields`).
- Before offering a harness, detection verifies the resolved binary and its parent directory are owned by the current user or root and are not group/world-writable, and refuses it otherwise (host/src/harness.rs `detect`, `check_install`).
- Summary runs have full access by design (Codex `danger-full-access` with `approvalPolicy: "never"`, Claude Code `--dangerously-skip-permissions`), run in the knowledge base folder with the service's environment plus `agent.env`, and are bounded by the per-run timeout and a process-group kill on cancel or timeout. Never put page text, or anything derived from the page, on a command line: the prompt names `page.md` by path, and Claude Code gets the prompt on stdin (host/src/service/prompt.rs, host/src/service/claude.rs).
- The prompt must mark the page content as untrusted data that may carry instructions aimed at the agent, and tell the agent to ignore them (host/src/service/prompt.rs `summary_prompt`). The agent's answer is untrusted too: the service validates it (host/src/answer.rs `parse_answer`) and drops any `related_notes` entry outside the knowledge base.
- Site-draft runs (Capture Site's Improve with AI) run in their own run directory under the service's support folder, never the knowledge base; their input is built by the native host from its own structural draft and shapes (the extension sends only the origin and shapes and may not set a run `kind`); the service validates only the answer's form and writes nothing to the home folder, the knowledge base or `config.toml` (`SiteDraftFinisher`); and the answer is judged only by the host's `structural::check_agent_draft` with `Drafted` provenance, never by the extension, which renders only the host's `site_drafted` (host/src/capture_site.rs, host/src/service/prompt.rs `site_draft_prompt`).
- Render streamed agent text and answers as text only (`textContent`), never as HTML or Markdown, and never navigate from them (extension/run-card.ts). The saved-summary lines in the panel and the run card follow the same rule (extension/summary-text.ts).
- Only the service writes summary files and `summary` log events, and only from a run it finished itself, with the answer it validated (`SummaryFinisher` in host/src/service/finisher.rs); no message lets the extension supply summary text, a summary path, or a record key for one.
- Agent text in a summary file is never written raw: front matter values go through `summary.rs` `yaml_text` (`presentation::plain_line`, which turns control and invisible format characters into spaces, then `markdown::yaml_string`'s JSON quoting; no Markdown entities), identity values (the `brauser:` block's canonical URL and record key) go through `yaml_string` unchanged so the ownership check reads them back exactly, single-line fields are escaped with `markdown::inline` or written as wikilinks only when the wikilink carries the name unchanged, and the description and assessment go through `markdown::neutralize_remote`, each starting a line after a host newline (host/src/summary.rs `render_summary`, DESIGN.md §5.2 step 6). Change the neutralizer only with its corpus (`host/tests/fixtures/markdown-neutralize-corpus.json`) and `npm run test:markdown-neutralizer` green.
- `open_summary` opens only the summary file the record's own key names and only when `summary::owned_summary` accepts it; it never opens a path the extension supplies or any other file at that name, and it re-checks, under the config lock and just before the opener runs, that the path still names the checked inode and no symlink (`vault::same_file`) (host/src/native.rs `open_summary`).
- Tests never run a real harness, `/usr/bin/open`, or `launchctl`: runners are tested with fake `codex`/`claude` scripts on an injected search path, the opener in test and `scripted-dialogs` builds only appends to the file `BRAUSER_OPEN_LOG` names (host/src/open.rs), and the smoke run's fakes sit on a pinned `PATH`.
- The Brauser service (`host/src/service/`) runs as the user, never root, as a per-user LaunchAgent started on demand. Its support directory must be a real directory owned by the user with mode 0700 (refuse a symlink or another owner, `ServicePaths::ensure_private`), its socket is mode 0600, and the service refuses a support path too long for a Unix socket rather than moving the socket elsewhere. Only the native host talks to it; the host accepts no `service_hello` from the extension and always chooses the run's record key itself.
- `agent.env` is read at the start of every run and must be a regular, non-symlink file of mode 0600, or the run fails before it starts (host/src/service/agent_env.rs). Never put a value from it in an error, log line, protocol message, or settings view; settings may show its names only.
- Before forwarding a run message, the host compares the service's `service_hello` `build_id` with its own and restarts a stale service once; a persistent mismatch is `service_unavailable`, never a request sent to old code (host/src/service/client.rs).
- A test that binds or connects a Unix socket (or reads a Node child's piped output) starts with the IPC guard, `ipc_test_guard::ipc_denied()` in Rust or `ipcDenied(t, kind)` from `scripts/tests/ipc-guard.mjs`, so it skips with a printed reason in a sandbox that denies local IPC and fails there when `BRAUSER_REQUIRE_IPC_TESTS=1` (CI). Never weaken a socket test's assertions instead (TESTING.md "Local IPC in sandboxes").
- Tests never call `launchctl` or touch the real `~/Library`: they use temp support directories, an injected launch function, and the test-only `BRAUSER_SERVICE_SPAWN` seam (scripted-dialogs builds only) that spawns the service directly. `scripts/register-dev-host.mjs` takes an injectable `run` for the same reason.
- Do not claim an installer, browser extension, or agent mode works until it has been built and exercised.
