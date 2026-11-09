# Capture re-learning: staged implementation

Parent: `brwr-6x2`. This brings forward only the re-keying part of DESIGN.md
§12.2 step 7.2. The parent stays in progress until the panel can review and
commit a replacement policy while preserving activity, notes and summaries.

## Open decisions

- Resolved by John's 2026-11-01 approval (parent bead): continue the
  append-only re-key direction; ordinary config saves retain the ID freeze.
- Resolved by Cookie for the assigned review corrections (2026-11-02): an
  owned daily log with a recognizable but invalid visit marker refuses review.
  Unowned Markdown still cannot supply history; ordinary indexing retains its
  tolerant parsing and frozen recovery behavior.
- Resolved by Cookie: keys evidenced on another origin are occupied, including
  containers without direct visits. A shared source or occupied destination
  requires explicit conflict resolution; this stage supplies none and refuses
  approval. Other origins never become proposed migration rows.
- Resolved by existing DESIGN.md §6.2, implemented by Cookie in `brwr-c1bq`
  (2026-11-03): retain physical note ownership and creation filenames. An
  existing canonical note supplies the editor; otherwise a sole owned alias
  note can supply it. Multiple alias notes without a canonical note refuse
  selection in this stage. URL copies remain separate. Affected editors use
  an opaque revision bound to logical identity, physical ownership, content
  and replay history; unaffected editors retain their existing revisions.

## Design challenge

2026-11-02, before the assigned `brwr-eo7`/`brwr-r62` corrections: the
`/root/design_challenge` reviewer challenged a second marker grammar and
foreign-origin migration rows. Adopt its simpler alternatives: wrap the existing
event/overflow parsers with strict rejection of recognizable invalid owned
markers, retaining legacy preceding-line context; build foreign-key occupancy
separately from the reviewed site's mappings. Check occupied sources as well
as destinations, include owned files and verified implicit containers, and
bind every contract used for occupancy into the report revision. Do not parse
removed parameter values or choose a file winner. Damaged/missing ownership
headers remain outside owned log ingestion, as in ordinary indexing.

The earlier alias/journal stages remain as built below.

Durable apply design challenge (2026-11-02, reviewer
`/root/migration_design_challenge`): adopt original-file preservation from
§6.2 instead of rewriting every note and summary. A shared resolver must
project raw history before config replacement is allowed. A logical artifact
key and a physical note ownership key are distinct; ordinary note saves must
never substitute one for the other. Use the existing pairwise replay, not a
new directed-chain algorithm. Durable apply must bind exact before/after
config bytes and an authoritative home-history append, and recover or refuse
ordinary access after prepare; command-only recovery without that gate is
unsafe. Unexpected external edits refuse. Cache deletion must reproduce the
projection from home history. brwr-ruwj implements the read-only shared
resolver/index projection first; brwr-dbp retains the remaining transaction,
physical-note selection, file-stub and durable-history integration.

Note ownership design challenge (2026-11-03, reviewer
`/root/note_contract_challenge`): adopt the existing canonical preference from
§6.2 rather than invent a file winner or copy every note. Integrate the same
location-aware wrappers into ordinary native reads, saves and key-change
conflicts immediately, using empty replay until durable apply exists. Preserve
raw revisions for unaffected keys, and bind affected revisions to replay
history, including unalias. A hash of only the final map would revive an old
edit after a move away and back; hash trusted replay instead. Whole-history
invalidation is conservative but simple; component-specific generations can
follow if real editing friction justifies them. Refuse an ambiguous fallback
instead of adding an unreviewed note-selection UI.

Apply decisions, resolved by John on 2026-11-09 (`brwr-dbp`):

- Recovery: an interrupted apply recovers automatically on the next host
  start, rolling forward or back from the prepared, byte-bound journal entry.
  There is no mode that refuses ordinary access until an explicit recovery.
- Unalias: not in the first version. Durable apply supports alias and re-key
  only; the read-only replay keeps its existing unalias handling.
- Authoritative history: an append-only file inside the home folder's existing
  fixed layout, so deleting the cache rebuilds the projection from it.
- Downgrade: no contract at this stage; older builds are out of scope.

Apply is authorized under these decisions. Ordinary note requests use empty replay; tests supply only
host-built replay. Neither the worker nor a journal receipt supplies aliases.

## Ordered delivery

1. **Alias rehearsal (`brwr-sq9`, complete 2026-10-28).** Added deterministic replay and
   `brauser adapters aliases-preview`, an offline JSON Lines command. Tests cover
   chains, moves back, suggestions, confirmations, pairwise unalias and
   duplicate event IDs. The tool derives endpoints through the Rust adapter
   engine and shows what would group together. This is a working preview tool, not a
   migration importer. The existing ID freeze stays enforced.
2. **Revision-bound impact planning (`brwr-hq8`, implemented 2026-10-28).** The host derives proposed mappings
   from authoritative recorded evidence and current/recorded contracts. List
   changed, unchanged, split, merged and unresolvable identities. Treat a
   simultaneous ID swap or reuse as an impact conflict, not as a sequential
   alias chain. Check stripped-query provenance before interpreting a stored
   URL. Keep both notes visible when the endpoints already have notes; never
   pick an autosave identity from a shared canonical address. Plans must bind
   both the configuration revision and the evidence they inspected.
3. **Durable migration and rebuild (`brwr-wvi` journal implemented; `brwr-dbp` integration next).** Add host-owned append-only re-key/alias
   operations and recovery before admitting any frozen-ID change. Reuse
   ownership-checked file operations, preserve conflicting notes and summaries,
   and keep browser-local note recovery separate from host migrations. Teach
   index rebuild, record-key resolution, refs, dirty artifacts and old-path
   stubs the same replay rules. Verify crash/retry, move-back and unalias
   behavior with notes on both sides; retain all frozen capture recovery.
4. **Panel review and commit.** Add Re-learn to Capture Site using the existing
   plain-language review and drafter checks. Show migration impact, refuse a
   stale plan and save pending note edits before identity changes. Only this
   host-controlled path can replace the ID freeze. Finish the parent's
   end-to-end acceptance and full gates before closing it.

## Replay decisions established by the rehearsal

Input order represents durable append order. Page clocks and the lexical
order of UUIDs cannot choose a canonical identity. The latest confirmed
operation for an unordered pair replaces its earlier decision. Confirmed
edges form connected components; the target of the latest still-active
confirmation in a component is canonical. A reverse move therefore selects
the old key again without creating a resolution loop.

Suggested aliases never group history or change a canonical key. Confirming
or rejecting a pair clears its earlier suggestion; suggestions within an
already-confirmed component are redundant. Unalias removes that pair's edge.
Other confirmed paths can still connect its endpoints, which the preview
makes visible. Exact logical duplicates (event ID, action and host-derived
endpoint identities) are idempotent; conflicting reuse refuses the rehearsal.
The durable importer must additionally bind its full stored evidence as the
capture journal does; this preview has no write authority.

## Offline command

Run `target/debug/brauser adapters aliases-preview < requests.jsonl` after a
host build. Each line supplies a format-1 `before` adapter document, an
optional `after` document and an `events` array. Without `after`, both sides
use `before`. Each operation has a UUID `event_id`, `action` (`confirm`,
`suggest` or `unalias`), and `from`/`to` objects with `url`, `artifact_id`
and optional `stripped_params`. One request uses one pair of policy versions;
separate lines are independent rehearsals, not a persistent history.

```json
{"before":{"format":1,"artifacts":[{"type":"work.item","origin":"https://work.example","match":[{"path":"/items/{id:int}"}],"id":"work:{host}/{id}"}]},"events":[{"event_id":"00000000-0000-0000-0000-000000000001","action":"confirm","from":{"url":"https://work.example/items/1","artifact_id":"work:work.example/1","stripped_params":[]},"to":{"url":"https://work.example/items/2","artifact_id":"work:work.example/2","stripped_params":[]}}]}
```

The result has `review_only: true`, sorted `groups` (canonical identity and
members), and outstanding `suggestions`. It describes only the supplied
operations. A claimed ID must equal host evaluation, endpoints must have the
same artifact type, and out-of-scope or unrecognized URLs refuse the request.
Query and `absent` readers on the endpoint's origin require explicit evidence
that none of their keys were stripped. Missing provenance is insufficient for
this rehearsal when a query reader exists; path-only identities can use legacy
unknown evidence. This is stricter than legacy note discovery and changes no
existing note behavior. Evidence lists contain names only, never values.

The command reads stdin and writes stdout/stderr only. It opens no config,
vault, cache, log or harness and accepts no apply flag. Each request is capped
at 4 MiB and 4,096 events. A failure identifies its request/event number, with
no JSON result for that request. Earlier successful lines may already have
produced results. The tests include the real binary in an empty temporary
support directory and assert it creates no files.

Validation: 12 replay/boundary tests and three real CLI isolation tests pass;
complete required and README gates pass (987 reported Rust outcomes including
guarded IPC skips, 15 extension files, both Clippy configurations and 10,696
inert Markdown renderings). The JSON example above ran through the built
binary. Linux testing does not establish macOS browser acceptance.

## Host-derived impact plan

`brauser adapters relearn-plan [--config /absolute/path/to/config.toml]`
reads one JSON object from stdin and prints one JSON report. Without
`--config` it reads the normal Brauser config. The selected home folder must
already be confirmed with the fixed layout; the command performs no layout
migration. Request shape:

```json
{"origin":"https://work.example","expected_revision":"<config revision>","proposed":{"format":1,"artifacts":[{"type":"work.item","origin":"https://work.example","match":[{"path":"/items/{id:int}"}],"id":"corrected:{host}/{id}"}]}}
```

Use the revision returned by `get_config` (`sha256:` followed by the
SHA-256 of config.toml's bytes). `proposed` replaces only this exact origin's rules; other origins
are refused. An empty list reviews removal. Callers cannot supply historical
IDs, URLs or evidence. The host holds the config lock, reads a fresh config
without recording adopted contracts, takes the capture lock, and rescans the
owned log markers directly. The disposable index cannot invent evidence or
hide a conflicting event ID. It also scans the fixed notes/summaries folders,
verifying ownership and expected filenames before accepting their recorded
keys and addresses. Neither kind's body is returned in the report.

Each `identities` row has a `source` record key, host-derived `targets`, a set
of `classifications` (`unchanged`, `changed`, `split`, `merged`,
`unresolvable`), `issues`, `evidence_count`, and source/destination file lists.
A split can also merge into another identity. File entries contain a path,
kind, ownership result and content revision. `unowned_files` explicitly lists
inspected Markdown that could not supply identity; an occupied unowned target
is an unresolved conflict. Two notes or summaries remain two files, with no
primary selected. A URL-keyed note at an artifact's address is a separate row
with `url_note_requires_explicit_choice`, never an inferred autosave key.
That choice also protects URL-keyed summaries.
Container mappings are derived from a recorded contract whose primary ID
matches the known source, including containers with no direct open.

The occupancy check includes other origins' log and owned note/summary keys,
plus implicit containers verified through all recorded contracts. Only the
reviewed origin supplies migration rows. A source also evidenced on another
origin receives `other_origin_source`; a foreign-occupied target receives
`other_origin_destination` and `merged`. Both refuse approval. This includes
HTTP/HTTPS rules sharing `{host}` and keys shared before the proposed edit.
The contract revision binds all origins' contracts used for this check.

A proposed target that is also an old source moving elsewhere (or one with
unresolved evidence) produces `simultaneous_key_reuse`. Thus A→B/B→A and
A→B/B→C are not flattened into a sequential alias component. Missing proposed
recognition, unknown source contracts, type changes and insufficient query
provenance remain unresolved. Any current, retired, hand-removed or proposed
query/absence reader requires explicit names-only provenance proving its key
was not removed. Legacy unknown works only for path-only recognition. Agent
body text never supplies frontmatter provenance.

`config_revision`, `contracts_revision`, `evidence_revision` and
`proposed_revision` bind the inputs; `plan_revision` hashes the report with
its own revision field empty. Evidence includes the inspected log event
payloads and hashes of every inspected Markdown file, so another delivery or
body edit changes the plan even when IDs are unchanged. The same inputs
produce the same report. A future apply must recompute it, compare revisions
and recheck file ownership under the locks; this token grants no write
permission. External editors do not participate in Brauser's locks.

Limits: 4 MiB request; the index's existing one-pass log bounds (including
64 MiB aggregate reads); 4,096 Markdown files and 16 MiB total across both
folders, with 4 MiB per file; 32,768 evidence entries including derived
container entries. Oversized, unreadable, non-regular or symlinked Markdown,
incomplete history and conflicting event IDs refuse the review instead of
presenting a partial plan. Coordination lock files may be created. Config,
adopted state, cache, logs, notes, summaries and browser-local recovery are
unchanged. There is no apply flag or native/worker migration message.

In an owned daily log, a recognizable `<!-- brauser:visit` marker that the
existing event/overflow parsers cannot validate refuses review with the log
path and line number. Truncated and joined markers cannot become an empty,
approvable report. Valid legacy visits retain preceding-line context, and both
overflow forms remain valid. Ordinary indexing still skips damaged markers
so frozen capture recovery can complete them later.

Next: the journal below records reviewed decisions; `brwr-dbp` implements file/index
integration, then panel review follows. Keep `brwr-6x2` in progress and
the existing ID freeze enforced until all stages satisfy their acceptance.


Impact-stage validation (2026-10-28): 19 library cases and two real CLI
boundary tests added; the documented JSON request was also exercised through
the built binary against an isolated config and owned note. Exact file bytes
survived, with no cache or adopted-state write. The full required/README gate
passed 1,018 workspace Rust outcomes (43 explicit IPC skips), all 15 extension
files, both Clippy builds, two script files and 10,696 inert renderings. The
scripted-dialog suite passed 880 outcomes (53 IPC skips). This closes brwr-hq8
only; Windows/macOS evidence and durable migration acceptance stay separate.

## Durable decision journal

`brauser adapters relearn-record [--config /absolute/path/to/config.toml]`
reads one JSON object. Obtain the `review` and its `plan_revision` from
`relearn-plan` first. Example (replace both revision placeholders):

```json
{"operation_id":"00000000-0000-0000-0000-000000000001","decision":"approve","expected_plan_revision":"<plan_revision>","review":{"origin":"https://work.example","expected_revision":"<config revision>","proposed":{"format":1,"artifacts":[{"type":"work.item","origin":"https://work.example","match":[{"path":"/items/{id:int}"}],"id":"corrected:{host}/{id}"}]}}}
```

The host takes the config lock followed by the capture lock, recomputes the
complete impact plan and compares its revision before publishing any new
decision. Config, contracts, log evidence, inspected file bytes and proposal
are therefore bound to the review. The selected home root and its recorded
folder identity are retained with the decision. This is a durable record of
review, not an apply token. The receipt always says `applied: false`.

`decision` is `approve` or `reject`. Approval requires every evidenced row to
have one artifact target and no issues, splits or merges. Unresolved evidence,
URL copies, type changes and simultaneous key reuse require a revised proposal
or rejection. A rejection retains the complete simultaneous mapping and all
issues; it never becomes sequential alias edges. Both-side files remain in
the report and untouched; even an approval selects no winning note or summary.
No decision replaces a prior entry. A new review uses a new canonical lowercase
UUID. Reusing a UUID with a different decision, proposal or review revision
refuses. An exact retry returns the original receipt without a second append,
including after evidence/config changed: it acknowledges the historical
decision and grants no current write authority.

`brauser adapters relearn-journal [--config /absolute/path/to/config.toml]`
takes no request body. It completes any prepared append and returns
`{"applied":false,"entries":[...]}` with full reports and proposals. It can
recover a decision even if its old home folder is missing or its config is now
unusable. The config directory must still be accessible; inspection uses the
same locks. With no journal, only lock files may be created.

Format 1 lives beside config.toml in `rekey/decisions.jsonl`, under a private
directory (0700 on Unix; files 0600). Existing state must be owned by the user
and private on Unix. Directory/file symlinks, nonregular files and unsupported
formats refuse. Entries carry sequence numbers, previous-entry checksums,
request hashes and full-payload checksums. Before appending, the host writes
and syncs `pending.json` through a temporary and a no-clobber hard link, then
syncs the directory. It stores the exact append bytes, their hash, the old
offset and the complete prefix hash. Recovery verifies the whole resulting
journal, then appends only missing prepared bytes when the existing tail is
an exact prefix (including a tear inside UTF-8). A different tail or changed
prefix refuses without truncation or guessing. The log and directory are
synced before pending is removed and the directory synced again. A retry after
any crash point either prepares the as-yet-unpublished decision afresh or
returns its single durable receipt. Unpublished temporary files are ignored,
never promoted to decisions; retain them for diagnosis. Windows retains the
existing shared helper's limitation: directory-entry power-loss durability
still needs platform validation.

Limits: 4 MiB input and per journal entry, 64 MiB total journal, 4,096 operations,
and 8 MiB plus 4 KiB for the escaped prepare record. Full/oversized or damaged
state refuses; there is no automatic pruning or archive procedure yet. Preserve
format-1 decoding, exact prepared bytes and checksums in later versions.

`brwr-wvi` adds only this host developer path. It adds no native message,
worker alias authority, config ID-freeze exception, cache replay or content
migration. Ordinary host/panel startup does not inspect the journal; recovery
runs on explicit journal inspection/recording. `brwr-dbp` must integrate
crash-safe config/content changes and authoritative reindex, with fresh input
and ownership checks at apply, preserving URL copies and both-side notes and
summaries. Panel Re-learn follows that integration, and the parent remains open.

Journal-stage validation (2026-10-28): 16 library regressions and two real
binary boundary tests added. The full required/README gate passed with 1,037
workspace Rust outcomes (43 explicit sandbox IPC skips), 899 scripted-build
outcomes (53 IPC skips), all 15 extension files, both Clippy builds, two script
files and 10,696 inert Markdown renderings. Fresh review fixed parent-directory
syncing when retrying an already-created journal directory. This completes
brwr-wvi only; brwr-dbp and panel review remain before parent acceptance.


## Review prerequisite corrections — 2026-11-02

Completed assigned brwr-eo7 and brwr-r62 before brwr-dbp can rely on reviewed
approvals. Seven regressions cover damaged markers through planning/recording,
real CLI refusal, valid legacy/overflow compatibility, foreign log/note/summary
and implicit-container occupancy, shared sources and unrelated-origin scope.
The damaged-marker and collision tests reproduced the old defects before the
fix. Full required/README checks pass with 1,314 Rust-reported outcomes (92
explicit IPC skips), 40 extension files, both Clippy configurations, eight
script files, 221 Python tests (92 platform skips) and 10,696 inert Markdown
renderings. The accessibility run passes two fixture checks and explicitly
skips 36 browser cases blocked by this sandbox. Fresh complete-diff review and
one local Codex reviewer pass report no findings. The driver still owns its
independent review, external test gate and publication.

Next: brwr-dbp's separate migration design challenge and implementation,
then panel review. Recompute all inputs at apply even for an old journal
approval; exact journal retries only acknowledge historical decisions. The
parent stays in progress and ordinary config saves retain the ID freeze.
Plain-language conflict evidence is proposed as brwr-pgxz; it is unassigned
and must not displace the remaining assigned work.


## Projected history preview

`brauser adapters relearn-preview [--config /absolute/path/to/config.toml]`
accepts exactly the same single JSON request as `relearn-plan`. It returns
`{"review_only":true,"plan":{...},"projection":{...}}`. The full host-derived
plan retains original ownership/file revisions. Projection contains:

- `artifacts`: the proposed policy's artifact views and corrected measures.
- `summaries`: original `SummaryView` entries grouped by projected artifact ID,
  oldest first, retaining checked physical summary paths.
- `edges`: resolved source/target keys, kind, weight and last-seen date.
- `recent`: up to 64 projected daily rows, with `recent_truncated`.
- `worklog`: the observed visit date range, text, artifact count and capture-gap
  notices; null when no indexed visit exists. Notes alone remain in the plan.

The same index derivation and Recent/worklog renderers serve ordinary reads
with an empty resolution and this preview with the reviewed resolution.
Historical capture/summary payloads, summary ownership keys and paths, event
positions and names-only provenance stay raw. Container recognition compares
resolved identities and counts each visit once in worklog totals. Ref targets
and trail sources resolve through the same table; collapsed self-links are
omitted. Existing `aliases-preview` uses the same extracted replay semantics,
including chains, latest-target move-back and pairwise unalias. A raw key
cannot name two artifact types. A simultaneous plan refuses split, merge,
key reuse, foreign occupancy and unresolved URL-copy choices before replay;
it is never flattened into a sequential chain.

The host takes config then capture locks. It computes a fresh impact plan,
reads strict owned log evidence for projection and compares a shared hash of event payloads,
durable ordering positions and capture gaps with the reviewed snapshot, then recomputes the plan to refuse changes by
external editors. The reopened config must also retain the bound revision.
Each scan retains the existing one-pass 64 MiB bound and inventory limits. No caller supplies aliases or history.
No cache/checkpoint is read, resumed or published; no notes, summaries,
artifacts, journal, adopted state or configuration is changed. Coordination
lock files may be created. Malformed requests and `--apply` refuse. This
stage deliberately adds no persistent applied-history format or write path.

brwr-ruwj is the shared resolver/index projection prerequisite of assigned
brwr-dbp. The latter remains in progress; the ownership slice below follows
the preview. Panel review stays on brwr-6x2; do not close either assigned item
based on prerequisite tests.

## Physical note ownership contract

`brwr-c1bq` integrates `note::NoteLocation` into ordinary native load/save and
key-change conflict loading. Public response keys remain logical. Vault
reads, ownership parsing, rendering, revision checks and replacements use the
physical key. A sole alias note therefore saves in place with its original
artifact ID, creation URL, stripped-parameter evidence and title. An existing
canonical note takes precedence while other files stay untouched; a malformed
or symlinked selected path refuses. Broken optional aliases do not hide an
owned canonical primary. URL copies never select an artifact editor.

The native request takes the config lock and recomputes the location for each
load/save. `location-v1:<hash>` binds affected revisions to both keys, the raw
content revision and the complete trusted replay history, including operations
that later unalias a key. This detects changed physical selection while the
logical key stays fixed and prevents a move away/back from reviving an old
edit. An unrelated operation conservatively invalidates affected editors;
keys never touched by replay keep existing raw revisions, including `missing`.
The low-level writer repeats its physical ownership/version check. Key changes
still produce `key_changed`; changed selection/history produces `stale`.
Browser-local recovered copies remain copy-out only.

This slice publishes no applied history and adds no native message or CLI.
Ordinary native calls pass empty replay and retain existing behavior. Tests
exercise the same native request handlers with host-built replay. Durable apply
must load and bind authoritative replay under the locks before using this
parameter; a historical approval cannot supply it. Remaining `brwr-dbp` work:
resolve the prepare-recovery/downgrade contract, publish exact config plus
authoritative home history, integrate replay into all readers and summary/run
identity, expose preserved alias-note viewing, generate old-path artifact
stubs and prove crash/reindex recovery. Panel review follows separately.


Ownership-stage validation (2026-11-03): 12 new regressions pass, including
native request handling with host-built replay. The complete required and
README gates pass with 1,421 Rust-reported outcomes (94 explicit IPC skips),
44 extension test files, both Clippy configurations, nine script test files,
221 Python cases (92 platform skips) and 10,696 inert Markdown renderings.
Accessibility passes two fixtures and explicitly skips 42 sandbox-blocked
browser cases. This completes brwr-c1bq only; driver/platform validation,
durable application and panel acceptance remain separate.
