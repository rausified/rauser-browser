# Capture re-learning: staged implementation

Parent: `brwr-6x2`. This brings forward only the re-keying part of DESIGN.md
§12.2 step 7.2. The parent stays in progress until the panel can review and
commit a replacement policy while preserving activity, notes and summaries.

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
3. **Durable migration and rebuild (`brwr-wvi` starts with the journal).** Add host-owned append-only re-key/alias
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

Next: `brwr-wvi` implements the durable journal and recovery. File/index
integration and panel review still follow it; keep `brwr-6x2` in progress and
the existing ID freeze enforced until all stages satisfy their acceptance.


Impact-stage validation (2026-10-28): 19 library cases and two real CLI
boundary tests added; the documented JSON request was also exercised through
the built binary against an isolated config and owned note. Exact file bytes
survived, with no cache or adopted-state write. The full required/README gate
passed 1,018 workspace Rust outcomes (43 explicit IPC skips), all 15 extension
files, both Clippy builds, two script files and 10,696 inert renderings. The
scripted-dialog suite passed 880 outcomes (53 IPC skips). This closes brwr-hq8
only; Windows/macOS evidence and durable migration acceptance stay separate.
