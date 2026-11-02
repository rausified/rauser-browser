# Incremental SQLite Index Implementation Plan (brwr-b2e, then brwr-7vf)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:**
- **brwr-b2e.** Replace the hand-written SQLite snapshot (`build_sqlite`, `host/src/index.rs:3587`) and the whole-history JSON cache (`index-state.json`, 64 MiB `MAX_STATE_BYTES`) with one real SQLite database written through `rusqlite` 0.41 (already in `host/Cargo.toml`, `bundled`).
  - A normal delivery is one small `BEGIN IMMEDIATE` transaction. It runs no full derive, rewrites no whole file and serializes no history.
  - The daily log stays authoritative and append-only. `brauser reindex` stays the full rebuild.
  - The only persisted progress besides derived rows is a bounded watermark (per-log checkpoints, a few meta keys, the dirty set).
- **brwr-7vf.** Make that database safe when several host processes open it at once: the panel, the settings page and each worker sync (DESIGN §3.2). The tools are WAL, a busy timeout, `BEGIN IMMEDIATE` for every write, one artifact-file regenerator at a time and generation-conditional dirty clears. With them, no `SQLITE_BUSY` reaches the extension, no row is lost or duplicated, no artifact file is regenerated twice for one generation and no dirty marker is lost.

**Architecture (one paragraph):**
- `host/src/index.rs` (8822 lines) is first split mechanically into `host/src/index/`, so parallel lanes do not collide.
- `brauser_events` stores every indexed event body (`serde_json(CachedEvent)`, unchanged shape) with its ordering key, artifact, container and day. It is the cache.
- The aggregation semantics stay in one function. `derive()` is refactored into a `Fold` (`apply(event)`, then `finish()`), and every derived row is produced by that fold. Three paths use it:
  - **full rebuild:** all events;
  - **per-group re-derive:** one artifact's events, plus its children if it is a container;
  - **append fast path:** the group's stored aggregate rows are loaded into the fold, and only the new events are applied.
- The fast path runs only when every new event sorts strictly after the group's stored `latest_key`. Anything else (backdated, a position move, a conflict, a new artifact) takes the per-group re-derive.
- Reads become bounded SQL over the derived tables. Relearn keeps a log-only in-memory scan and never opens the database.

**Tech Stack:** Rust (`brauser` host crate, `rusqlite` 0.41 bundled, std `File::try_lock`, `sha2`, `cap_std`). No extension change and no protocol change.

**Sources:**
- DESIGN.md:
  - §3.2 (one host process per native-messaging connection)
  - §5.5 and §6.7 (index and artifact files)
  - the file table at lines 953-954
  - the dirty-set text at lines 1366-1367
  - §14 (line 1680, the bead list)
- AGENTS.md:
  - "Security rules": an artifact file is replaced only after finding exactly one matching `brauser.kind: artifact` and `artifact_id`; user files are preserved; the daily log is authoritative and append-only.
  - the `forbid(unsafe_code)` rule
  - line 17 (cache format)
- TESTING.md lines 1218 and 1248.
- `scripts/check_current_state.py` lines 148-152. These phrases must survive: "rebuildable sqlite \`index.db\`", "disposable sqlite artifact index", "rebuildable indexing".
- The three competing designs, two judge reports and the design challenge relayed by the workflow on 2026-11-02 (summarised below).

## Chosen design and why

The judges both picked **design B, "SQL-native delta upserts with an ord watermark"** (8/10 each) over design A ("staged, parity-proven migration with subset derive", 7 and 6) and design C ("harness-first with a lease", 6 and 7). This plan keeps B's skeleton:
- the mechanical module split first (T0), which is what makes the user's "parallelize as much as possible" achievable;
- SQL-native reads over materialized `brauser_artifact_days`;
- lazy, connection-per-operation open, so paths that must not touch the cache never create `index.db`;
- the log-checkpoint watermark with tail parsing;
- an OS `File::try_lock` regeneration lock with generation-conditional clears;
- bounds tests built on counters.

It replaces B's write engine and grafts the judges' best ideas. Each change is listed under "Grafts" with its source.

### Grafts adopted

1. **One semantics, not two (design challenge plus design A S4).** B's per-field `title_ord`/`url_ord`/`mode_ord`/`why_ord`/`first_ord` columns and `recompute_latest` are dropped. Rows are produced only by the refactored `derive()` fold: in a full rebuild, a per-group re-derive, or a seeded append fast path. See "Design challenge".
2. **A permanent full-derive oracle (design A).** Under `cfg(test)`, every write transaction in every index test ends by asserting that the derived tables equal a full `derive()` of `brauser_events`. This is in addition to a randomized replay property test (shuffled, backdated, duplicate and conflicting orders). The oracle stays after b2e ships; it is not deleted.
3. **Golden tests before behaviour changes (design A S0).** These land before any lane changes behaviour:
   - a rusqlite test reader that also runs `PRAGMA integrity_check` and replaces all 12 `python3` readers;
   - a golden relearn revision;
   - golden outputs of every read API, captured from the current code.

   This gives read parity without design A's throwaway S1 writer and S3 oracle copies.
4. **`write_txn` / `read_txn` helpers plus clippy `disallowed-methods` (design C).** No ad-hoc deferred writer can be added, and no read transaction is ever upgraded into a write. In WAL, that upgrade is the one path that returns `SQLITE_BUSY` without calling the busy handler.
5. **`render_sha256` per artifact (design A).** After a rebuild, adapter change or log reset, an artifact is marked dirty only when its new render differs from what regeneration last wrote. This replaces both `keep_unchanged_files_clean`'s double render and B's `old_*` EXCEPT diff.
6. **Legacy-file safety (design A).** Before deleting a legacy `index.db`, the plan checks header bytes 18-19 with no connection open and deletes `-wal` and `-shm` first. On Windows, if a pre-upgrade host still holds the file, it falls back to dropping the legacy tables in place.
7. **Migration that never overwrites user edits (design A).** If the v8 JSON is unusable, only artifacts whose expected file is missing are marked dirty, and `render_sha256` is stored as NULL ("unknown").
8. **Previous-winner conflict rule by join (design A).** The staged rebuild resolves the previous cache's winner by joining against the live `brauser_events`. It finishes with a streaming per-group re-derive, so memory stays bounded at year scale.
9. **A multi-process harness built early (design C T1).** It lands in wave 1 and stays `#[ignore]` until regeneration lands. A kill test proves that the OS lock hands regeneration over immediately (design C T12(c)).
10. **Unix `0o600` pre-create and a Windows drop-order rule in test helpers (design C).**
11. **M3 readiness (judge 2).** `artifacts` is an explicit rowid table, so FTS5 can use `content_rowid`. Every non-public table starts with `brauser_`. Test 4583 asserts the exact public table set plus that prefix rule.

### Grafts rejected

- **Design C's wall-clock `regen_lease` and its shared `index.lock` held for each connection's lifetime.** An OS file lock already releases on crash. The lease adds sleep and clock-jump takeover cases, and the shared lock adds a new "busy; retry" error. Both judges agreed.
- **Design A's S1 rusqlite clone of the hand-written snapshot and its S3 oracle copies of every read.** Both are throwaway work. Golden outputs (graft 3) give the same parity.
- **Design A's S7-4 move of index calls off `ConfigStore::lock_current`.** This is out of scope for 7vf; see "Open decisions" item 1. The 7vf tests prove index correctness without the config lock by driving `ArtifactIndex` from child processes that never take it.
- **Moving `regenerated.json` into meta (design C).** It is already a bounded marker. It stays a file, written only while `index-regen.lock` is held.
- **The `event_conflicts` table (design C).** Conflicts stay in memory per pass, as today (`#[serde(skip)]`).

## Design challenge

- **Challenger:** the workflow's design-challenge pass, arguing against the judges' winner.
- **Alternative:** build design A's S2/S4 core instead of B's delta engine:
  - `brauser_events` as the cache;
  - every delivery re-derives the affected group with the unchanged `derive()`;
  - keep the per-file `(bytes, mtime)` checkpoint and reread today's whole log, with no tail offset;
  - no `ord`-keyed per-field columns;
  - add a delta path only if measurement shows it is needed.

  The objections behind it:
  - B and C fork every order-dependent rule into a second copy: the `Untitled` fallback, type and site from the first event, a `None` title not overriding, container renaming, summaries adding no opens or days, legacy adding no focus.
  - Index writes are already serialized by the config lock, so 7vf is about removing a dependency on a coarse lock, not about fixing data loss.
  - Tail parsing with a 4 KiB hash guard is a new correctness surface.
  - The v4/v5 importers are dead weight.
- **Outcome: adopted in part.**
  - **Adopted: one aggregation semantics.** The fold is shared by rebuild, re-derive and the fast path, which answers the strongest objection. The fast path is not a second implementation. It seeds the same `Fold` with the group's stored aggregate rows and calls the same `apply()` on the new events, so the only new code is the seed and write-back. It is legal only when every new event sorts strictly after the group's `latest_key`. In exactly that case, ordered replay of the full history and seeded replay of the suffix are the same computation. This is checked by the permanent oracle and the randomized replay test.
  - **Adopted: per-group re-derive as the exact fallback** for backdated events, same-body position moves, conflicts, new artifacts, adapter changes and late-resolving mentions (`rederive_group` on the affected set, never a global rederive).
  - **Adopted: 7vf leaves the config lock alone,** and C's lease and connection-lifetime lock are dropped.
  - **Adopted: the v4/v5/v1 migrators are deleted** (see "Open decisions" item 3).
  - **Kept from B, against the challenge: an always-on fast path from the start**, not "only if measured". The b2e acceptance requires bounded per-delivery work on a year-scale fixture. Per-group re-derive is O(artifact history) for a long-lived artifact and O(container) for every child delivery into a 20k-event container, on every normal in-order delivery. The fast path is small once the fold exists, and it can be switched off with one constant (`FAST_PATH`) without any schema change.
  - **Kept from B, against the challenge: tail parsing.** Logs are daily files, but a heavy day can reach `MAX_LOG_BYTES` (32 MiB). Rereading today's whole log on every delivery is O(day) per delivery, quadratic across the day, and the "heavy-use day" step 6.6 test exists for that day. The guard is deliberately conservative. Any doubt (same size with a new mtime, the byte before the offset not being `\n`, a tail-hash mismatch) causes a full reread of that one log, which is idempotent, never a silent skip. A shrunk or removed log triggers the staged rebuild. An in-place edit outside the hashed window is documented as needing `brauser reindex`, matching the append-only contract.

## Open decisions (need John)

Each has a default the plan proceeds with.

1. **Remove the config lock around index calls?** Default: **no**, filed as a follow-up bead. Today about 26 `lock_current` sites, the finisher (`service/finisher.rs:6-7, 123`) and `reindex` (`main.rs:103`) serialize index work across processes. 7vf makes the index correct without that lock (tested), but taking the lock away changes real panel latency and finisher behaviour and deserves its own revertible change.
2. **Migration fallback when `index-state.json` is unusable.** Default: **mark dirty only artifacts whose file is missing** and store `render_sha256 = NULL`. Existing files and user edits are kept, and a stale file refreshes on its next real change. Today's missing-cache behaviour (everything dirty, so owned files are rewritten) is the alternative.
3. **Delete the v1/v4/v5 cache migrators?** Default: **yes**. A pre-v8 cache takes the fallback in item 2. John's installs are on v8. Keep `migrate_v5_state` only if some other install is known to still be on v5.
4. **The per-delivery budget for a backdated child event in a 20k-event container.** That event takes the container re-derive, O(container). Default: accept it, record its cost in the bounds test with a looser ceiling, and add a container-additive fast path only if T17 shows it above 250 ms in a release build.

## Global constraints

- Work only in `/Users/jrauser/ENDURANCE/work/rauserBrowser-index` on `feat/sqlite-index` (based on `48d80d6`). Lanes run in their own child worktrees off this branch.
  - Never edit, check out or build in `/Users/jrauser/ENDURANCE/work/rauserBrowser`, and never run `scripts/dev/deploy-live`.
  - Read-only git calls use `GIT_OPTIONAL_LOCKS=0`. Never use bare `git stash`.
  - Implementer tasks never push, open PRs, spawn agents or touch `.beads` through `bd`. The coordinator does those.
- No new dependency. `rusqlite` 0.41 bundled is already declared. No extension, permission or protocol change.
- First-party crates keep `#![forbid(unsafe_code)]`; rusqlite's own unsafe code is accepted.
- Index code is cross-platform, and Windows CI must stay green.
- Tests never touch the real HOME, never mutate the process environment (children get env through `Command::env`), and never need `python3` after T1.
- Commits end with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Commands (cargo is not on PATH):
  - **Env:** `export PATH=$HOME/.rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH`. Build into the worktree's own `target/`, and never set `CARGO_TARGET_DIR` to the live checkout.
  - **Host:** `cargo test --locked -p brauser <filter>`
  - **Workspace:** `cargo test --locked --workspace`
  - **Lint:** `cargo fmt --all -- --check && cargo clippy --locked --workspace --all-targets -- -D warnings`
  - **Windows clippy:** `cargo clippy --locked --workspace --all-targets --target x86_64-pc-windows-gnu -- -D warnings`
  - **Extension (regression only):** `npm run build:extension && npm run typecheck && npm run lint && npm run test:extension`. Rebuild before testing. `node_modules` is a symlink, so never run `npm install`.
  - **Smoke:** only `npm run smoke:macos:headless`. Never run `scripts/smoke-macos.mjs` directly and never import it. It checks that the reindex stdout starts with `Rebuilt index.db;` (`smoke-macos.mjs:2910`).
  - **Docs:** `python3 scripts/check_current_state.py`

## Verified starting point (`48d80d6`)

- `host/src/index.rs`, 8822 lines:
  - constants (lines 61-71): `STATE_VERSION = 8`, `REGENERATION_CHECKPOINT` 500 ms, `MAX_STATE_BYTES` 64 MiB, `MAX_LOG_BYTES` 32 MiB, `MAX_SCAN_BYTES` 64 MiB, `MAX_SCAN_FILES` 100 000, `SQLITE_PAGE_SIZE` 65 536;
  - `IndexState` at line 118: events, gaps, gap_logs, positions, sequence, next_sequence, adapters, conflicts (serde skip), dirty, logs, containers, container_files;
  - `LogCheckpoint { bytes, modified }` at line 300;
  - `ArtifactIndex::derive` at 569, `record_events` at 611, `reconcile_from_logs` at 643, `keep_unchanged_files_clean` at 655, `adapter_dependent_files` at 688;
  - `reindex` at 735, `sweep_stale_temporaries` at 787, `regenerate_state` at 815 and `regenerate_state_with` at 823;
  - `publish_state_and_database` at 1422 and `relearn_evidence` at 1523;
  - `ordered_events` at 2056: the sort key is `(event_time, position.is_none(), position, sequence or u64::MAX, id)`;
  - free `derive` at 2079, `container_for` at 2397, `roll_up` at 2537, `add_edge` at 2605;
  - `render_artifact` at 3232 and `timeline_rollups` at 3366;
  - the writer block at about 3445-3837, with `build_sqlite` at 3587.
- `derive()` facts that bind the fold:
  - Latest-wins uses `at >= row.latest_at` in ordered replay. Activity sets `title` only when it is `Some` and sets `url` and `last_mode`. Legacy and Summary set `title` and `url`.
  - Type and site come from the event that creates the row.
  - Legacy adds 1 open and no time. A summary adds no opens and no days.
  - `why` is the newest by instant (ties go to the later ordered event), applied in `finish` only to artifacts that have a row.
  - A container row comes from `roll_up` (`rolled_up = true`, a `within` edge from the child), and `finish` renames container-type rows.
- Edge kinds partition by owner, so a group re-derive can own its edges:
  - trail edges carry a valid transition from `capture::trail_transition_is_valid` (241), never `mentions` or `within`, and are owned by the event's artifact (the target);
  - `mentions` edges are owned by the source event's artifact;
  - `within` edges are owned by the child.
- `resolve_refs` depends only on the adapter Document.
- Test anchors in `index.rs`:
  - 4421 `hostile_presentation_is_safe_in_logs_and_rebuilt_artifacts_without_changing_payloads`
  - 4543 `artifact_files_and_worklogs_neutralize_unicode_formatting`
  - 4583 `index_db_has_queryable_tables_and_capture_does_not_touch_artifact_files`
  - 5038 `case_collisions_external_edits_and_unchanged_files_are_handled`
  - 5090 `a_heavy_day_only_marks_the_artifact_dirty_until_regeneration`
  - 5165, 5197, 5221 (checkpoint publishing)
  - 5461 `a_year_of_daily_opens_has_a_bounded_timeline`
  - 5614, 5694, 5754, 5833, 5864, 5914 (catch-up and rebuild bounds)
  - 5982 `reindex_rebuilds_database_and_files_from_log_markers`
  - 6302 `reconciling_a_duplicate_keeps_the_dirty_set_and_external_edits`
  - 7030, 7988, 8436, 8498, 8767 (v1/v4/v5 migrations)
  - 8637 `an_oversize_cached_row_is_left_out_of_index_db_without_stopping_publication`
  - 8696 `index_db_rows_up_to_the_local_payload_limit_are_readable_and_larger_ones_are_left_out`
- `host/src/native.rs`:
  - 4097 `discovery_does_not_open_unavailable_history_or_need_note_contracts` (a garbage `index.db` stays byte-identical, line 4101);
  - 7222 and 7422: no `index.db` after refused deliveries;
  - 7536, 7661 and 7770: fault injection through `index-state.json` and the bytes of `index.db`;
  - `python3` readers at 7121, 7721 and 8364.
- `index.rs` plus `native.rs` hold 12 `python3` references in total.
- `host/src/relearn.rs` tests 728 (the cache dir is never created), 741, 941, 1009 and 1170.
- `AGENTS.md:17` says "rebuildable SQLite `index.db` (cache format 8)".

## Schema (cache format 9)

File `<cache>/index.db`, plus SQLite's `index.db-wal` and `index.db-shm`. The lock file `<cache>/index-regen.lock` is never read or written.

**Identity:** `PRAGMA application_id = 0x62727772` ('brwr') and `PRAGMA user_version = 9`. `STATE_VERSION` 8 becomes 9.

**Per-connection pragmas**, set by `db::open`:
- `journal_mode=WAL`. If WAL is refused, fall back to `DELETE` and log a warning.
- `synchronous=NORMAL` (safe because the log is authoritative, and log checkpoints commit atomically with the rows)
- `busy_timeout=10000`
- `foreign_keys=OFF`
- `temp_store=MEMORY`
- `journal_size_limit=8388608`

`page_size` stays at the default 4096. On Unix, the file is pre-created with mode `0o600` before the first SQLite open.

**Ordering key.** `order_key` is a BLOB built in Rust (`index/ord.rs`) that sorts under memcmp exactly like `ordered_events`, concatenated in this order:
1. instant as i128 nanoseconds, big-endian with the sign bit flipped;
2. one byte: `0` = has a position, `1` = none;
3. the position's relative log path components, each with `0x00` escaped as `0x00 0xFF`, separated by `0x00 0x01` and terminated by `0x00 0x00` (empty when there is no position);
4. the line as u64 BE;
5. the sequence as u64 BE (`u64::MAX` when absent);
6. the event_id bytes.

### Public tables

These are the consumer contract and the M3 FTS5 base. Legacy column names and order come first.

```sql
CREATE TABLE artifacts(            -- rowid table on purpose (FTS5 content_rowid)
  artifact_id TEXT NOT NULL UNIQUE, artifact_type TEXT NOT NULL, site TEXT NOT NULL,
  title TEXT NOT NULL, url TEXT NOT NULL, first_seen TEXT, last_seen TEXT,
  focused_secs INTEGER, background_secs INTEGER, opens INTEGER, days_opened INTEGER,
  last_mode TEXT, why TEXT);
CREATE INDEX artifacts_by_type ON artifacts(artifact_type, artifact_id);

CREATE TABLE visits(               -- own visits only; no contained copies (as today)
  event_id TEXT PRIMARY KEY, artifact_id TEXT NOT NULL, occurred_at TEXT,
  local_date TEXT NOT NULL, focused_secs INTEGER, background_secs INTEGER,
  opens INTEGER, mode TEXT, title TEXT, url TEXT);
CREATE INDEX visits_by_artifact ON visits(artifact_id, occurred_at);
CREATE INDEX visits_by_date ON visits(local_date, artifact_id);

CREATE TABLE edges(
  source_artifact_id TEXT NOT NULL, target_artifact_id TEXT NOT NULL, kind TEXT NOT NULL,
  weight INTEGER NOT NULL, last_seen TEXT NOT NULL,
  PRIMARY KEY(source_artifact_id, target_artifact_id, kind)) WITHOUT ROWID;
CREATE INDEX edges_by_target ON edges(target_artifact_id, kind);

CREATE TABLE url_provenance(
  event_id TEXT NOT NULL, artifact_id TEXT NOT NULL, role TEXT NOT NULL,
  url TEXT NOT NULL, stripped_params TEXT,
  PRIMARY KEY(event_id, role)) WITHOUT ROWID;
CREATE INDEX url_provenance_by_artifact ON url_provenance(artifact_id, role);

CREATE TABLE dirty_artifacts(
  artifact_id TEXT PRIMARY KEY, generation INTEGER NOT NULL) WITHOUT ROWID;
```

### Internal tables

Every name starts with `brauser_`, and they are private to external SQL consumers.

```sql
CREATE TABLE brauser_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
-- keys: root, root_identity, log_dir, artifacts_dir, adapters (document fingerprint),
--       next_sequence, next_generation, rebuild ('none'|'staging'), rebuild_id,
--       legacy_import ('done')

CREATE TABLE brauser_events(
  event_id TEXT PRIMARY KEY, artifact_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK(kind IN ('activity','why','legacy','summary')),
  order_key BLOB NOT NULL UNIQUE, local_date TEXT NOT NULL,
  log_path TEXT, log_line INTEGER, sequence INTEGER,
  container_id TEXT,          -- container_for(url, id) under meta.adapters; NULL if none
  has_refs INTEGER NOT NULL DEFAULT 0,
  body TEXT NOT NULL,         -- serde_json(CachedEvent), byte-identical to today's shape
  body_sha256 BLOB NOT NULL) WITHOUT ROWID;
CREATE INDEX brauser_events_by_artifact ON brauser_events(artifact_id, order_key);
CREATE INDEX brauser_events_by_container ON brauser_events(container_id, order_key)
  WHERE container_id IS NOT NULL;
CREATE INDEX brauser_events_by_day ON brauser_events(kind, local_date, order_key);

CREATE TABLE brauser_group_state(   -- one row per artifact that has a derived row
  artifact_id TEXT PRIMARY KEY,
  latest_key BLOB NOT NULL,         -- max order_key over the group's events (own + children)
  latest_at TEXT NOT NULL,          -- ArtifactRow.latest_at (RFC 3339 with offset)
  why_at TEXT,                      -- instant of the current why, to seed the fold
  rolled_up INTEGER NOT NULL,
  file_path TEXT,                   -- where regeneration last wrote it (replaces container_files)
  render_sha256 BLOB) WITHOUT ROWID;  -- NULL = unknown (forces a write on next change)

CREATE TABLE brauser_artifact_days(
  artifact_id TEXT NOT NULL, local_date TEXT NOT NULL,
  focused_secs INTEGER NOT NULL, background_secs INTEGER NOT NULL, opens INTEGER NOT NULL,
  own_focused_secs INTEGER NOT NULL, own_background_secs INTEGER NOT NULL,
  own_opens INTEGER NOT NULL, summaries INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY(artifact_id, local_date)) WITHOUT ROWID;
CREATE INDEX brauser_days_by_date ON brauser_artifact_days(local_date, focused_secs DESC, artifact_id);

CREATE TABLE brauser_logs(
  path TEXT NOT NULL, purpose TEXT NOT NULL CHECK(purpose IN ('events','gaps')),
  bytes INTEGER NOT NULL, modified_ns INTEGER NOT NULL,
  parsed_bytes INTEGER NOT NULL, tail_sha256 BLOB NOT NULL,
  PRIMARY KEY(path, purpose)) WITHOUT ROWID;   -- path: log-dir-relative, '/' separators

CREATE TABLE brauser_gaps(
  gap_id TEXT PRIMARY KEY, marker TEXT NOT NULL, log_path TEXT, log_line INTEGER,
  start_date TEXT, end_date TEXT) WITHOUT ROWID;
CREATE INDEX brauser_gaps_by_date ON brauser_gaps(start_date, end_date);

-- Present only while meta.rebuild = 'staging':
CREATE TABLE brauser_stage_events(  -- same columns as brauser_events, keyed (event_id, body_sha256),
  ...                               -- at most 8 bodies per event_id
  PRIMARY KEY(event_id, body_sha256)) WITHOUT ROWID;
CREATE TABLE brauser_stage_logs(... same columns as brauser_logs ...) WITHOUT ROWID;
CREATE TABLE brauser_stage_gaps(... same columns as brauser_gaps ...) WITHOUT ROWID;
```

**Derivation of tables from the fold:**
- `artifacts`, `visits` (rows with `contained = false`), `edges` and `url_provenance` are written exactly as the old writer wrote them. `days_opened` is `days.len()`.
- `brauser_artifact_days` is the per-(artifact, day) sum of all of the fold's visit rows, own plus contained. The `own_*` columns count only `contained = false` rows, and `summaries` counts summary rows.

## Data flow

### Delivery

This covers `record_events`, used by `record_activity`, `record_activities`, `record_why` and `record_summary`. Steps 1 to 6 run in one `write_txn`:
1. Check `brauser_meta`'s location and adapter fingerprint.
   - A location mismatch starts a staged rebuild.
   - A fingerprint mismatch runs the adapter change (below) once.
2. Incremental catch-up (below), which usually ingests this delivery's own log append, with its position.
3. For each caller-supplied event, `INSERT ... ON CONFLICT(event_id) DO NOTHING` into `brauser_events`, with `container_id` and `has_refs` computed at insert.
   - **Same body:** a no-op. If the log copy now gives an earlier position or replaces a sequence, update `log_path`, `log_line` and `order_key`, and add the group to the re-derive set.
   - **Different body:** a conflict. The first-indexed copy wins, and the conflict is returned after commit, as today.
4. Affected groups = the new events' artifacts ∪ their `container_id`s. For each group:
   - **Fast path** (`FAST_PATH = true`): taken when the group row exists, when every new event's `order_key` is greater than `brauser_group_state.latest_key` (for a child, that check must also hold against its container's row), and when none of the new events needs a re-derive. Load the group's `artifacts` row, its `brauser_artifact_days` dates (needed for `days`) and its `why` and `why_at` into a seeded `Fold`. Load the existing `edges` rows only for the keys the new events touch. Call the same `Fold::apply` on the new events in order, then `finish()`. Upsert the changed rows, insert the new `visits`, `url_provenance` and day deltas, and set `latest_key`.
   - **Otherwise:** `rederive_group`. Load the group's own events plus, for a container, its children (`brauser_events_by_container`). Run `Fold` over them in `ordered_events` order. DELETE and INSERT only the group's owned rows: its artifact row, its visits and url_provenance, its edges (owner rules above), its days and its state row.
5. Upsert `dirty_artifacts(id, generation = next_generation++)` for each affected group.
6. COMMIT.

The ack path in `native.rs` is unchanged: index errors are only logged, and capture stays durable.

### Incremental catch-up

Used by delivery, `reconcile_from_logs` and read freshness.
- Stat the log dir (`collect_markdown`) and compare each file with `brauser_logs`. Skip unchanged files.
- **Grown file:** if the byte at `parsed_bytes-1` is `\n` and the up-to-4-KiB window before `parsed_bytes` hashes to `tail_sha256`, parse from `parsed_bytes` up to the last complete marker. A trailing partial marker stays unparsed (test 5614).
- **Same size with a new mtime, or a guard mismatch:** reread that one log in full, bounded by `MAX_LOG_BYTES`. Ingest is idempotent.
- **Shrunk or removed log:** set `rebuild='staging'`.
- Overflow markers go into `brauser_gaps`.
- `LOG_BYTES_READ` keeps counting.

`reconcile_from_logs` becomes this catch-up, plus a body comparison for each id the caller named. `keep_unchanged_files_clean` is deleted, because only newly ingested events mark dirty (tests 6302 and 5038 hold by construction).

### Reads

These are `artifact`, `artifact_with_summary`, `summary_for`, `latest_why`, `artifact_url_evidence`, `recent`, `worklog_report` and `worklog_gaps`. Each runs one `read_txn` after a stat-only freshness check against `brauser_logs`.
- If the logs are ahead, a separate `write_txn` catch-up runs first. It re-checks the watermark inside the transaction, so two hosts never ingest the same tail twice.
- If that write lock stays busy past the timeout, the read serves the committed snapshot instead of an error, because the lock holder is ingesting those same appends.
- While `rebuild='staging'`, reads return the existing `INCOMPLETE_CATCH_UP` text, "artifact history is unavailable".

The queries:
- **Card:** the `artifacts` row, plus:
  - `within`: the `within` edge from this id with the highest weight, ties to the lowest target;
  - `contained`: `within` edges to this id joined to `artifacts`, `ORDER BY focused_secs DESC, artifact_id`, `LIMIT 20`;
  - `incoming_trails`: valid non-`forward_back` transitions from http(s) sources, `ORDER BY last_seen DESC`, `LIMIT 5`;
  - `linked_count`: a `COUNT(DISTINCT ...)` over both directions;
  - the summary: the newest `kind='summary'` event by `order_key` that `summary_view` renders.
- **Recent:** `brauser_artifact_days JOIN artifacts`, using the existing type filter, the Rust-lowercase title match (a registered deterministic `brauser_fold(text)` function, so non-ASCII folding matches today), the existing ORDER BY (focus saturated at `u32::MAX`), `LIMIT n+1` to set `truncated`, and the per-day summary from `brauser_events_by_day`. `types` is `SELECT DISTINCT artifact_type ... LIMIT 64`.
- **Worklog:** `brauser_artifact_days` within the range of at most 7 days, with the own/contained splits, plus summary events in range and `brauser_gaps` intersecting the range. `MAX_WORKLOG_BYTES` and the text formatter are unchanged.
- **URL evidence:** `url_provenance WHERE artifact_id=? AND role NOT LIKE 'trail:%'`.

All output structs and text are byte-identical to the T2 golden outputs.

### Regeneration

`regenerate_dirty`, the daily pass, the settings refresh and the reindex tail:
1. Take `index-regen.lock`, then snapshot `SELECT artifact_id, generation FROM dirty_artifacts` in a `read_txn`.
   - In b2e the lock is taken with `lock()`, because the config lock already serializes callers.
   - In 7vf the connection-close pass and the daily pass use `try_lock` and skip when it is held. The explicit refresh and reindex wait.
2. For each id, run `rederive_group` in a `read_txn` to get the full `ArtifactRow`. Its activities and summaries are bounded per artifact. Then call the unchanged `render_artifact`, and write through the unchanged `vault::replace_owned_file` / `owned_artifact` check, or `remove_artifact` for a group with no row but a `file_path`.
3. **Written or Unchanged:** a short `write_txn` runs `DELETE FROM dirty_artifacts WHERE artifact_id=? AND generation=?` and stores `render_sha256` and `file_path`.
4. **Conflict or Err:** the row stays dirty, with up to 64 paths reported.

Each artifact commits on its own, so `REGENERATION_CHECKPOINT` and `checkpoint_interval` go. `record_pass` takes its offset from the `latest_at` of the newest group. `regenerated.json` is unchanged and written under the lock.

### Adapter change

This runs once per document change, never per delivery, in one `write_txn`:
1. Recompute `container_id` for events whose URL any adapter could match.
2. Recompute the resolved mentions for `has_refs=1` events.
3. Affected set = the old ∪ new containers (`brauser_group_state.rolled_up`, plus the new `container_id`s) ∪ the artifacts with `has_refs=1`. Run `rederive_group` for each, and mark it dirty only where the new render's sha256 differs from `render_sha256`.
4. A container left with no row keeps `file_path`, so regeneration removes its file.

This replaces `adapter_dependent_files` and `dirty_containers`. If T17 shows it takes more than 2 s at year scale, chunk it as staged passes.

### Relearn

`relearn_evidence` uses `index/scan.rs::LogScan`, which is `IndexState` minus persistence (events, positions, gaps). It runs in strict mode with symlink refusal and the incomplete-pass and conflict refusals. The revision stays `sha256(serde_json(BTreeMap<event_id, CachedEvent>))`. It never opens or creates `index.db`.

## Migration for existing users (format 8 to 9)

This runs on the first write-capable open by a format-9 host, and only when there is something to index. Read-only and discovery paths never create the file (native test 4101 and relearn test 728).

1. **Detect.** With no connection open, read the first 100 bytes of `index.db` if it exists. A file that is not SQLite, has `application_id != 'brwr'` or has `user_version != 9` is legacy or foreign. The hand-written writer's file has application_id 0.
2. **Clear it safely.** Delete `index.db-wal` and `index.db-shm` first, so stale frames can never be replayed into a foreign file, then delete `index.db`, and do all of this only after header bytes 18-19 show the file is not WAL-mode.
   - A directory at `index.db` means "artifact history is unavailable", with capture durable and the path untouched (native test 7770).
   - If the delete fails on Windows because a pre-upgrade host holds the file, open it in rollback mode and DROP the legacy tables in place. Until that succeeds, reads stay "unavailable", and the next open retries.
3. **Create.** In one `write_txn`, create the schema with `IF NOT EXISTS` and re-check `user_version` inside the transaction, so two hosts opening for the first time converge.
4. **Import the v8 JSON once.** If `index-state.json` parses as `STATE_VERSION` 8 within the existing 64 MiB `read_bounded_json` bound (this one read is the only remaining use of `MAX_STATE_BYTES`):
   - its events go into `brauser_events` as the "previous winners" for the conflict rule;
   - its `dirty` set goes into `dirty_artifacts`;
   - `containers` and `container_files` go into `brauser_group_state.rolled_up` and `file_path`;
   - for every other artifact, `render_sha256` is seeded with the hash of the render the v8 state derives, which is the clean or dirty split `keep_unchanged_files_clean` used to compute, done once.
5. **Fallback.** If the JSON is absent, any other version, oversized or invalid, nothing is imported. After the rebuild, only artifacts whose expected file is missing are marked dirty, and `render_sha256 = NULL` (open decision 2). User edits are never overwritten.
6. **Rebuild.** Run the staged rebuild (below) from the logs. While it spans passes, reads return `INCOMPLETE_CATCH_UP` and each call advances one pass, as today.
7. **Clean up.** After the swap commits, delete `index-state.json` and `index-catch-up.json` and set `legacy_import='done'`. A crash before this point simply repeats the import.

**Downgrade and mixed versions:**
- A pre-format-9 host finds no `index-state.json`, rebuilds from the logs and renames its snapshot over `index.db`.
- The next format-9 open sees application_id 0 and takes steps 1-2. Writes made in that window landed in the orphaned inode and are recovered from the authoritative logs.

### Staged rebuild and `brauser reindex`

The same mechanism serves reindex, a shrunk or removed log, a location change and migration. No database file is renamed or deleted on this path, so it is Windows-safe.
1. In a `write_txn`, set `rebuild='staging'` and `rebuild_id`, then create and clear the `brauser_stage_*` tables. The previous winners stay readable in `brauser_events`.
2. **Passes.** Each `write_txn` reads at most `MAX_SCAN_BYTES` and `MAX_SCAN_FILES` of logs into the stage tables and commits. Progress resumes across calls and processes. This replaces `index-catch-up.json` and its cap.
3. **Finalize,** in one `write_txn`:
   - choose winners: a body whose sha matches the live `brauser_events` row wins (the previous cache winner, by join); otherwise the earliest `order_key` wins;
   - replace `brauser_events`, `brauser_logs` and `brauser_gaps`;
   - stream `rederive_group` over every group (each container with its children, then each uncontained artifact), so memory is bounded per group;
   - re-adopt vanished container files from `brauser_group_state.file_path` ∪ `owned_rollup_files`;
   - drop the stage tables and set `rebuild='none'`.
4. **After the rebuild:**
   - **reindex:** mark every artifact dirty, then regenerate all files (forced, which intentionally replaces user edits to owned files, as today). Print a line that starts `Rebuilt index.db; wrote N, kept N, and left N conflicting...`.
   - **a reset rebuild (not reindex):** mark dirty only where the render sha differs.
   - Run `PRAGMA wal_checkpoint(TRUNCATE)` after reindex.

"Reindex reproduces the same tables" means a canonical dump of the public tables and `brauser_artifact_days`, each `ORDER BY` primary key, with `generation`, `sequence` and the sequence bytes of `order_key` excluded.

## Concurrency

The base comes from b2e and is extended by 7vf. It holds when no caller takes `.config.lock`.

**Every mutation** is a `write_txn` (`TransactionBehavior::Immediate`) with `busy_timeout` 10 s and up to 3 retries with jitter at BEGIN for `SQLITE_BUSY`/`SQLITE_BUSY_SNAPSHOT`. A busy error that survives the retries maps to the existing "artifact history is unavailable" text. `database is locked` or `busy` wording never reaches a native response, and a cfg(test) counter `BUSY_SURFACED` counts it.

**No lost or duplicated rows.** A writer re-reads its watermark after taking the lock. `event_id` is the primary key, and `visits` and `url_provenance` are keyed by `event_id`.

**Readers** take WAL snapshots, never block writers and are never upgraded into writers. `clippy.toml` `disallowed-methods` forbids `rusqlite::Connection::transaction`, `execute_batch` and `Transaction::new` outside `index/db.rs`.

**One regenerator.** Only the holder of `<cache>/index-regen.lock` (std `File::try_lock`, exclusive; never the database file, because Windows `LockFileEx` is mandatory) runs `regenerate_dirty`, `record_pass`, adapter-change file removals or the reindex tail. A crash releases it.

**No lost dirty marker.** A mark gets a fresh `next_generation` inside the delivery transaction, and a clear is conditional on the generation that was snapshotted.

**Lock order:** `.config.lock`, then the capture lock (relearn only), then `index-regen.lock`, then the SQLite write lock. No code takes a file lock while it holds a SQLite transaction.

**Connections** are per operation (open, set pragmas, run the transaction, drop), and none is opened in `ArtifactIndex::open_in`. This keeps Windows handles short-lived, keeps the open lazy, and lets the last closer checkpoint the WAL.

**The sweep.** `sweep_stale_temporaries` keeps its exact `.brauser-index-` and `.brauser-artifact-` prefixes. A test asserts it never removes `index.db`, `-wal`, `-shm`, `-journal` or `index-regen.lock`.

## Module layout (after T0)

The index code moves into `host/src/index/`, every file under `forbid(unsafe_code)` through the crate root:

| File | Holds |
| --- | --- |
| `mod.rs` | the public `ArtifactIndex` API |
| `db.rs` | open, pragmas, DDL, `write_txn`/`read_txn`, legacy detection, busy mapping |
| `ord.rs` | the ordering-key encoding |
| `fold.rs` | `derive`, `Fold`, `roll_up`, `add_edge`, `container_for` |
| `store.rs` | ingest, `rederive_group`, the fast path, owned-row writes, the oracle hook |
| `catchup.rs` | log checkpoints, tail parse, gaps |
| `rebuild.rs` | staging, finalize, adapter change |
| `query.rs` | the card, Recent, worklog, why, summary, URL evidence |
| `render.rs` | `render_artifact`, `timeline_rollups`, the YAML helpers |
| `regen.rs` | regeneration, the regen lock, the pass marker |
| `scan.rs` | `LogScan`, log parsing |
| `legacy.rs` | detection and import of v8 |
| `tests/` | the existing tests, split by area |

## Tasks

Parallel waves, each lane in its own child worktree off `feat/sqlite-index`. A lane edits only its named files plus the listed `mod.rs` methods. The coordinator merges each wave and runs **Host** and **Lint** before the next wave starts.

| Wave | Tasks (parallel within a wave) | Needs |
| --- | --- | --- |
| 0 | T0 | none |
| 1 | T1, T2, T3, T4, T5, T6, T7 | T0 |
| 2 | T8, T9 | T8: T3, T4, T5. T9: T3, T6 |
| 3 | T10, T11, T12, T13, T18 (docs draft) | T10: T8, T9. T11: T8, T9. T12: T8. T13: T8 |
| 4 | T14, T16, then T15 | T14: T8, T10. T16: T10, T12. T15: T10 to T14 and T16 |
| 5 (7vf) | T17 (b2e bounds), T19 | T17: T15, T16. T19: T15 |
| 6 (7vf) | T20, then T21 (gate) | T20: T7, T17, T19. T21: all |

### Task 0: Split `index.rs` into modules (wave 0, alone)

**Files:** `host/src/index.rs` becomes the `host/src/index/` layout above, including `tests/`.
- [ ] Move code with no behaviour change. Keep every item's visibility and keep the tests under `#[cfg(test)] mod tests` per area.
- [ ] **Host**, **Workspace**, **Lint**, **Windows clippy** and **Extension** must all be green, with the test count unchanged (record it in the commit message).
- [ ] Commit `refactor(index): split index.rs into modules`.

### Task 1: rusqlite test reader (wave 1)

**Files:** new `host/src/index/tests/sql.rs` (`sql_rows(db, query) -> Vec<Vec<Value>>`, `integrity_ok(db)`), the index tests that use `python3`, and the `native.rs` tests at 7121, 7721 and 8364.
- [ ] Replace all 12 `python3` uses. The old `sqlite_check` becomes `PRAGMA integrity_check` plus `PRAGMA quick_check`.
- [ ] Open the reader with `SQLITE_OPEN_READ_ONLY`, and drop it before tempdir cleanup (the Windows drop-order rule).
- [ ] Keep tests: everything that used `python3`, with its assertions unchanged.
- [ ] Commit `test(index): read index.db through rusqlite, not python3`.

### Task 2: Golden fixtures (wave 1)

**Files:** new `host/tests/fixtures/index/`, containing log trees (hostile title, case-colliding IDs, containers and roll-ups, legacy M1, summaries, conflicts, backdated, equal instants across logs, a year of daily opens) and the golden outputs; new `host/src/index/tests/golden.rs`.
- [ ] Capture with today's code: the relearn revision, every read API's serialized output (card, Recent with filters and truncation, worklog, why, summary, URL evidence), and a canonical dump of the five public tables per fixture.
- [ ] Add tests: `relearn_revision_is_golden` and `read_apis_match_golden`. Later tasks keep both green unchanged.
- [ ] Commit `test(index): golden fixtures for reads, tables and the relearn revision`.

### Task 3: Connection factory and schema (wave 1)

**Files:** new `host/src/index/db.rs` and `clippy.toml` (`disallowed-methods`).
- [ ] `open(cache_dir, Mode::{ReadWrite, IfExists})`: the pragmas, the WAL fallback, Unix `0o600` pre-create, and the exact DDL above as one const.
- [ ] `write_txn` (Immediate plus BEGIN retry), `read_txn`, busy-error mapping, and `detect_legacy(path)` (header read with no connection; WAL-byte check; sidecars deleted first; the Windows DROP fallback).
- [ ] Add tests:
  - the pragmas are applied;
  - `IfExists` never creates a file and leaves a garbage file byte-identical;
  - a directory gives unavailable;
  - the Unix mode is 0600;
  - two threads in `BEGIN IMMEDIATE` never surface BUSY;
  - legacy detection deletes the sidecars before the main file;
  - a foreign WAL-mode file is left alone and reported unavailable.
- [ ] Commit `feat(index): rusqlite connection factory and format 9 schema`.

### Task 4: Ordering key (wave 1)

**Files:** new `host/src/index/ord.rs`.
- [ ] `order_key(instant, position, sequence, id) -> Vec<u8>` per the encoding above.
- [ ] Add a property test: sorting by key equals the `ordered_events` tuple sort on random instants, paths (including `-`, `.`, `/`, NUL and multi-component paths), lines, sequences and ids. Add a unit test for negative instants.
- [ ] Commit `feat(index): order-preserving event key`.

### Task 5: Fold refactor of `derive` (wave 1)

**Files:** `host/src/index/fold.rs`.
- [ ] Turn the body of the `derive()` loop into `Fold::apply(&CachedEvent)`, and the post-loop sorts, `why` assignment and container renaming into `Fold::finish() -> Derived`. `derive()` becomes `Fold::new`, then `apply` over `ordered_events`, then `finish`.
- [ ] Add `Fold::seeded(rows, days, why, edges)` for the fast path.
- [ ] No behaviour change: all tests and the T2 goldens must stay green.
- [ ] Add a test `seeded_suffix_equals_full_replay`: for random histories split at any point where the suffix is strictly newest, a seeded replay of the suffix gives the same derived rows as a full replay.
- [ ] Commit `refactor(index): derive as a reusable fold`.

### Task 6: `LogScan` for relearn (wave 1)

**Files:** new `host/src/index/scan.rs` and `relearn_evidence` in `mod.rs`.
- [ ] Extract the log parsing and in-memory catch-up (strict mode, symlink refusal, incomplete-pass and conflict refusals, earliest-position winner) from `IndexState`.
- [ ] `relearn_evidence` uses it and never opens the cache dir.
- [ ] Keep tests: relearn 728, 741, 941, 1009 and 1170, `host/tests/relearn_plan.rs`, `relearn_journal.rs`, and T2's golden revision.
- [ ] Commit `refactor(index): log-only scan for relearn evidence`.

### Task 7: Multi-process harness skeleton (wave 1)

**Files:** new `host/src/index/tests/mp_harness.rs`.
- [ ] `spawn_children(role, n, dir, seed)` re-execs `current_exe()` with `--exact <child test> --nocapture --test-threads=1` and `BRAUSER_INDEX_MP_CHILD=<role>:<dir>:<seed>`, set through `Command::env`. The child test is a no-op without that env var.
- [ ] Roles: deliver, read, regenerate and reindex. Children start together on a barrier file. Each writes a JSON result (errors, plus a regeneration trace of pid, artifact, generation, start_ns and end_ns from a cfg(test) hook in `replace_artifact`).
- [ ] Children never take `.config.lock`.
- [ ] Land it `#[ignore]` with the deliver and read roles running against today's code.
- [ ] Commit `test(index): multi-process harness`.

### Task 8: SQL store, ingest and group re-derive (wave 2)

**Files:** new `host/src/index/store.rs`; in `mod.rs`: `record_events` and the load path.
- [ ] Ingest into `brauser_events` with `container_id`, `has_refs`, `local_date`, `order_key` and the conflict rules.
- [ ] `rederive_group(tx, ids)` with the owned DELETE/INSERT of every table, including `brauser_artifact_days` and `brauser_group_state`.
- [ ] Dirty upsert with generations.
- [ ] Delivery uses the re-derive path for every group (`FAST_PATH = false` until T14).
- [ ] Add the cfg(test) oracle hook: at the end of every `write_txn` in tests, the canonical dump of the derived tables must equal a full `derive()` of `brauser_events` written through the same writer.
- [ ] Add the replay property test: random streams in shuffled, backdated, duplicate and conflicting orders, with summaries, legacy, why-before-activity, containers and mentions, compared against a full derive.
- [ ] Keep tests: 4421, 4543, 5038, 5090 (one dirty row read with `sql_rows`, no artifacts dir, `FULL_DERIVES == 0`), 6302, and every T2 golden table dump.
- [ ] Commit `feat(index): events table as the cache with per-group re-derive`.

### Task 9: Incremental catch-up (wave 2)

**Files:** new `host/src/index/catchup.rs`; `reconcile_from_logs` in `mod.rs`.
- [ ] The `brauser_logs` watermark, the stat-only freshness check, the tail parse with its guards, the single-log reread, shrink or remove leading to `rebuild='staging'`, and `brauser_gaps`.
- [ ] Delete `keep_unchanged_files_clean`.
- [ ] Port the checkpoint tests: 5614 (unchanged logs never reread; a trailing marker is recovered), 5694, 5833, plus new `an_append_reads_only_the_tail` and `a_same_size_rewrite_rereads_only_that_log`.
- [ ] Commit `feat(index): log watermark catch-up`.

### Task 10: Staged rebuild, reindex and adapter change (wave 3)

**Files:** new `host/src/index/rebuild.rs`; `reindex` in `mod.rs`; `host/src/main.rs` (stdout unchanged).
- [ ] Stage tables and passes, finalize with the previous-winner join, a streaming per-group re-derive, adoption of vanished container files, and `wal_checkpoint(TRUNCATE)`.
- [ ] The adapter change runs per the data flow, with render-sha dirtying.
- [ ] Port: 5754, 5864, 5914, 5982, and the `reindex_after_losing_the_only_roll_up*` tests (7211-7284). Rewrite 8498 as `rebuild_progress_resumes_after_a_crash_mid_pass`.
- [ ] Add `incremental_equals_reindex` (canonical dump equality, including a rebuild split over passes and resumed across `ArtifactIndex` instances) and `adapter_change_dirties_only_changed_renders`.
- [ ] Commit `feat(index): staged in-place rebuild`.

### Task 11: Reads to SQL (wave 3)

**Files:** new `host/src/index/query.rs`; the read methods in `mod.rs`. Callers in `native.rs` keep their signatures.
- [ ] Every read per the data flow, inside one `read_txn` after the freshness check, serving the snapshot when busy. Register `brauser_fold`.
- [ ] Keep tests: T2 `read_apis_match_golden`, the Recent, worklog and card tests, and 7341 (stale-cache catch-up, also when another process wrote the logs).
- [ ] Commit `feat(index): bounded SQL reads`.

### Task 12: Regeneration (wave 3)

**Files:** new `host/src/index/regen.rs`, `host/src/index/render.rs` (unchanged rendering, moved).
- [ ] Per-artifact `rederive_group`, render, `replace_owned_file`, then the conditional clear plus `render_sha256` and `file_path`.
- [ ] `index-regen.lock` taken with `lock()`.
- [ ] `record_pass` and `daily_pass_due` from `brauser_group_state`.
- [ ] Remove `REGENERATION_CHECKPOINT` and `checkpoint_interval`.
- [ ] Port 5165, 5197 and 5221 as `each_cleared_artifact_commits_alone` and `a_crash_between_write_and_clear_leaves_it_dirty_and_the_rewrite_is_unchanged`.
- [ ] Add `a_redirty_between_render_and_clear_survives`.
- [ ] Keep 5461 (365 visits, Timeline of at most 48 lines; the page-size line dropped) and 5038.
- [ ] Commit `feat(index): per-artifact regeneration with generation clears`.

### Task 13: Lazy-open contracts and native fault tests (wave 3)

**Files:** `host/src/native.rs` tests; the open sites in `mod.rs`.
- [ ] Open the database only after validation and only when committing ingested data.
- [ ] Rewrite:
  - 7536: plant a directory at `index.db` before first open; capture stays durable and history reads "unavailable";
  - 7661: delete the last delivery's `brauser_events` and `brauser_logs` rows through SQL, then assert catch-up without replay;
  - 7770: a directory gives unavailable with no side effect.
- [ ] Keep 4101 (byte-identical), 7222 and 7422 (no `index.db`), and relearn 728.
- [ ] Commit `test(native): index fault tests against index.db`.

### Task 14: Append fast path (wave 4)

**Files:** `host/src/index/store.rs`.
- [ ] Turn on `FAST_PATH`: the seeded `Fold`, the guard on `latest_key` (child and container), and the write-back of changed rows and deltas. Every other case falls back to `rederive_group`.
- [ ] The T8 oracle and replay test now run with the fast path on. Add `in_order_delivery_takes_the_fast_path` (counter `FAST_PATHS`) and `backdated_delivery_falls_back`.
- [ ] Commit `perf(index): append fast path through the shared fold`.

### Task 15: Delete the writer and the JSON state (wave 4)

**Files:** `host/src/index/*`.
- [ ] Delete:
  - `SqlValue`, `SqlTable`, `sqlite_snapshot`, `build_sqlite`, `SqliteImage` and their helpers (the old lines 3445-3837), and `SQLITE_PAGE_SIZE`;
  - `publish_state_and_database`, `atomic_write` for the database, `save_pending`/`read_pending`, `PendingCatchUp` and `state_path`;
  - `IndexState` persistence;
  - `migrate_state`, `migrate_v4_state` and `migrate_v5_state`;
  - `PUBLICATIONS` (replaced by `FULL_DERIVES` and `TRANSACTIONS`);
  - `adapter_dependent_files` and `dirty_containers`.
- [ ] Set `STATE_VERSION = 9`.
- [ ] Delete 8696. Flip 8637 to `an_oversize_row_is_stored_and_rendered_bounded` (the title is still truncated at 2048 at ingest). Rewrite 4583 to the exact public table set plus the `brauser_` prefix rule. Re-express the `dirty_ids` helper and every remaining `index-state.json` test against the tables.
- [ ] Add a gate test `no_legacy_writer_remains`: `include_str!` over the module files, asserting no `build_sqlite`, `SqliteImage` or `index-state.json` write.
- [ ] Commit `refactor(index): remove the hand-written writer and JSON cache`.

### Task 16: Migration from format 8 (wave 4)

**Files:** new `host/src/index/legacy.rs`; fixtures `host/tests/fixtures/index/v8/` (a legacy `index.db` and `index-state.json` generated by the pre-change build at `48d80d6` and committed as bytes).
- [ ] Implement migration steps 1-7.
- [ ] Add tests:
  - `v8_import_keeps_dirty_set_container_files_and_user_edits`;
  - `v8_tables_equal_a_fresh_reindex`;
  - `unusable_json_dirties_only_missing_files`;
  - `stray_sidecars_are_removed_before_the_legacy_file`;
  - `legacy_json_files_are_removed_after_commit`;
  - `an_old_host_rename_over_is_recovered`;
  - `two_first_opens_converge`.
- [ ] These replace 7030, 7988, 8436 and 8767. The m1 files of 8767 still gain their opens through the rebuild.
- [ ] Commit `feat(index): migrate format 8 caches`.

### Task 17: Year-scale bounds, the b2e acceptance (wave 5)

**Files:** new `host/src/index/tests/bounds.rs`.
- [ ] Fixture written as real daily logs: 365 days with about 200 events a day, about 2k artifacts, one container with more than 20k child events, and bodies sized so that the old serialized state exceeds 64 MiB.
- [ ] Build it with `reindex` (which proves there is no ceiling). Then deliver: to a fresh artifact, to a long-lived artifact, in order to a child of the big container, and backdated to that child.
- [ ] Assert:
  - for in-order deliveries: `FULL_DERIVES == 0`, `FAST_PATHS == 1` per group, `EVENTS_LOADED` at most the batch size, a bounded `ROWS_WRITTEN`;
  - `LOG_BYTES_READ <= appended bytes`;
  - the `index.db` file identity (inode or Windows file index) is unchanged;
  - no file under `artifacts/` is touched;
  - Recent, worklog and card all answer;
  - a loose debug wall-clock ceiling (500 ms in order; the backdated container case is reported against its own ceiling, open decision 4).
- [ ] Add a 100k-event release variant, `#[ignore]`, for one CI OS.
- [ ] Commit `test(index): year-scale per-delivery bounds`.

### Task 18: Docs (draft in wave 3, final after T16)

**Files:**
- `AGENTS.md:17`: cache format 9, keeping "rebuildable SQLite `index.db`".
- `DESIGN.md`:
  - the file table at 953-954: drop `index-state.json` and `index-catch-up.json`; add `-wal`, `-shm` and `index-regen.lock`;
  - 1366-1367: the dirty set with generations, the log watermark, no 64 MiB cap;
  - §3.2: the index is self-locking;
  - 1680: b2e and 7vf shipped.
- `TESTING.md` 1218 and 1248: no python3; the multi-process harness.
- The finisher module comment at `service/finisher.rs:6-7`.
- The AMOS inventory entry, if this capability is listed there.
- [ ] Run **Docs**.
- [ ] Commit `docs: SQLite index format 9`.

### Task 19: Busy hardening and the regen lock with `try_lock`, 7vf (wave 5)

**Files:** `db.rs`, `regen.rs`, the native error mapping.
- [ ] Add the `BUSY_SURFACED` counter.
- [ ] The connection-close and daily passes use `try_lock` and skip. The explicit refresh and reindex wait.
- [ ] Map busy and locked errors to "artifact history is unavailable".
- [ ] Add retries for the WAL pragma and schema creation.
- [ ] Add tests: `a_held_regen_lock_skips_the_close_pass`, `busy_text_never_reaches_a_response`, and `sweep_keeps_wal_shm_journal_and_lock_files`.
- [ ] Commit `feat(index): single regenerator and busy hardening`.

### Task 20: Multi-process acceptance, 7vf (wave 6)

**Files:** `mp_harness.rs` (remove `#[ignore]`) and new `host/tests/index_concurrency.rs`.
- [ ] **(a) Index level.** 4 deliverers, 3 readers and 2 regenerators for 200 batches with overlapping and retried ids against one cache and root, with no config lock. Assert:
  - zero errors, and no busy or locked text;
  - the `brauser_events` count equals the unique ids in the logs, and `visits` and `url_provenance` have no duplicates;
  - artifact totals equal the logged sums;
  - no two regeneration intervals overlap for one artifact, and no (artifact, generation) pair is written twice;
  - a delivery during a pass keeps its marker;
  - one final pass empties the dirty set;
  - `reindex` then gives an equal canonical dump;
  - `-wal` stays under a bound.
- [ ] **(b) Host level.** 3 real `CARGO_BIN_EXE_brauser` host processes over native-messaging framing (4-byte little-endian length plus JSON), sharing an injected config and cache. They interleave `record_batch`, `list_artifacts` and `get_artifact`. Every response is ok, and `list_artifacts` eventually shows every artifact.
- [ ] **(c) Kill.** `Child::kill` the regenerator holding `index-regen.lock`. Another process takes the lock at once, and no dirty marker is lost.
- [ ] Runs on macOS, Linux and Windows CI.
- [ ] Commit `test(index): multi-process concurrency acceptance`.

### Task 21: Gate (coordinator)

- [ ] Rebase on `origin/main`.
- [ ] Run **Workspace**, **Lint**, **Windows clippy**, **Extension**, **Smoke** and **Docs**.
- [ ] Get the CI matrix green, including Windows.
- [ ] Ask for a review, focused on the test-diff mapping: every retired test names its replacement.
- [ ] The coordinator pushes, opens the PR, and closes brwr-b2e, then brwr-7vf, in `.beads`.

## Risks

- **The fast path drifts from a full derive.** Mitigations:
  - the fast path calls the same `Fold::apply`, and only on strict suffixes;
  - the permanent oracle runs after every write in about 110 tests;
  - the replay property test covers the orderings;
  - `FAST_PATH = false` disables it without a schema change.
- **Group re-derive is not group-local.** That would happen if `derive` had a hidden cross-group dependency. The known ones are handled: mentions are resolved per source event from the Document, container rows come only from children, and edge ownership is partitioned by kind. The oracle catches any unknown one.
- **`order_key` diverges from `PathBuf` component order,** including Windows `OsStr`. Mitigations: positions use the portable relative path, plus the T4 property test.
- **The tail-hash guard misses an in-place edit before the window.** Mitigation: the append-only contract, any size or mtime oddity forces a single-log reread, and `reindex` is the documented recovery.
- **A backdated child event in a huge container costs O(container)** (open decision 4). Its cost is measured in T17.
- **O(history) transactions hold the write lock for seconds.** These are finalize, reindex and the adapter change, never deliveries. Mitigations: the 10 s timeout with retries, reads serve the snapshot, delivery errors are only logged, and the adapter change is chunked if T17 shows more than 2 s.
- **Mixed-version deploys.** An old host can rename a snapshot over the database. Mitigations: the application_id check rebuilds from the logs, and sidecars are deleted first.
- **Windows.** An open handle can block a legacy delete; the DROP fallback covers it. WAL is unavailable on redirected folders; the DELETE fallback is correct but blocks readers. Tempdir cleanup can fail while handles are open; the drop-order rule covers it.
- **Lazy open regresses.** That would break 4101, 7222, 7422 and relearn 728. Those tests plus T13 cover it.
- **Test rewrites weaken invariants.** About 20 tests change mechanics. Every retired test is mapped to a named replacement in its task, and review checks that map.
- **The relearn revision changes.** Mitigations: `CachedEvent` serde stays frozen, and T2 pins the golden revision.

## Acceptance checks

**brwr-b2e:**
1. `index.db` is written only through rusqlite. `no_legacy_writer_remains` passes. `build_sqlite`, `SqliteImage`, the JSON state and `index-catch-up.json` are gone. The only state outside the derived rows is the bounded watermark (`brauser_logs`, `brauser_meta`, `dirty_artifacts`).
2. A normal delivery runs no full derive and no file rewrite: `FULL_DERIVES == 0`, the file identity is unchanged, and the counters are bounded (T8, T14, T17).
3. The year-scale fixture exceeds the old 64 MiB state with no error, and every per-delivery bound holds (T17).
4. `brauser reindex` reproduces the same canonical dump (`incremental_equals_reindex`, and T20(a)'s final reindex).
5. The step 6.6 tests are green with their behavioural assertions unchanged: 4421, 4543, 4583 (updated deliberately), 5038, 5090, 5461, 5982 and 6302. Relearn 728, 741, 941, 1009 and 1170 and the golden revision are green. The T2 read goldens are green.
6. The full suites are green: **Workspace**, **Lint**, **Windows clippy**, **Extension**, **Smoke** and **Docs**, and Windows CI.

**brwr-7vf:**
1. T20(a) and (b): two or more host processes deliver batches and read `list_artifacts` against one `index.db` at once. No `SQLITE_BUSY` is surfaced (`BUSY_SURFACED == 0`), no row is lost or duplicated, and no artifact file is regenerated twice for one generation.
2. A dirty marker set during a pass survives it (T12, T20(a)).
3. Killing the regenerator hands the lock over immediately (T20(c)).
4. `brauser reindex` afterwards gives the same dump.
5. The full suites are green on macOS, Linux and Windows.
