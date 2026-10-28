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
2. **Revision-bound impact planning (`brwr-hq8`, next).** Have the host derive proposed mappings
   from authoritative recorded evidence and current/recorded contracts. List
   changed, unchanged, split, merged and unresolvable identities. Treat a
   simultaneous ID swap or reuse as an impact conflict, not as a sequential
   alias chain. Check stripped-query provenance before interpreting a stored
   URL. Keep both notes visible when the endpoints already have notes; never
   pick an autosave identity from a shared canonical address. Plans must bind
   both the configuration revision and the evidence they inspected.
3. **Durable migration and rebuild.** Add host-owned append-only re-key/alias
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
