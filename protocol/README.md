# Brauser native messaging protocol

`schema.json` is the source of truth for native messages. Regenerate the Rust and
TypeScript types after changing it:

```sh
python3 protocol/generate.py
python3 protocol/generate.py --check
```

The generated files are `protocol/src/generated.rs` and
`protocol/ts/generated.ts`. Both are committed so ordinary builds do not need
Python or a code generation package. The generator uses only Python's standard
library and rejects schema features it cannot translate.

Every request has `protocol_version`, a nonempty `request_id` of at most 128
characters, and a `type` tag. The host checks the version before deserializing
the typed request. A mismatch returns an `error` response with code
`unsupported_protocol_version` and the host's protocol version. If malformed
input has no recoverable request ID, an `error` response may use an empty ID.

`hello_result.host_version` is the host's package version with its build
commit as semver build metadata, for example `0.1.0+c7c6be06d378` or
`0.1.0+c7c6be06d378-dirty` when the Rust sources built into it had uncommitted
changes (`unknown` when built outside a Git checkout). The
panel and settings page show that commit beside the extension's own and flag a
different commit; nothing else depends on its format.

Protocol version 6 covers host discovery, configuration, native folder
selection, visit and artifact-activity capture, record-keyed whole-file page notes, artifact index
queries, the AI-harness readiness gate, and format-versioned artifact-adapter
configuration. `get_config` returns a
`revision` that the extension must send as `expected_revision` with its full
`update_config` snapshot. A successful update returns a new `revision`; a stale
revision returns a `conflict` error. The revision is `missing` when no config
file exists, or `sha256:` followed by 64 lowercase hex digits for an existing
file. The host compares it with the current file before replacement. An invalid
or newer TOML file yields an inert empty snapshot, its raw-file revision, and a
`config_issue` in `hello_result` and `config_result`. A revision-checked repair
backs up those exact bytes before replacing the file.

`ConfigSnapshot.storage` must be present: use `null` only to explicitly clear
the selected notes folder. `strip_params` is
also required. The generated Rust deserializer rejects omitted nullable fields,
matching the JSON Schema requirement. Existing M0 TOML defaults to an empty
site list, `utm_*`/`fbclid`/`gclid` stripping, and a 300-second repeat window.
An M0 file with a notes root but no native-picker provenance is returned as an
inert repair state. The user must reselect the folder; the host backs up the
old file before saving the M1 config. M1 accepts the neutral profile only.

Version 4 added `StorageConfig.summaries_dir` (required, `null` until a
summaries folder is chosen) and four required `ConfigSnapshot` keys:
`agent_denylist` (normalized hostnames), `agent_denylist_confirmed`,
`log_incognito` (always `false`), and `agent` (the harness entry or `null`).
`config_result` also reports `agent_status`, the host's view of whether AI
commands may run. `update_config` requires `harness_token`; only native
harness setup mints one, and it is the only way to set
`agent_denylist_confirmed` or a harness entry. The extension may echo both
unchanged, or remove the harness, which keeps the confirmation. Adding a
denylist entry needs no confirmation; removing or replacing one needs a
`confirm_config` token. `check_agent` runs the readiness gate every AI command
uses and, for a non-null `url`, reports whether the denylist allows that
page; `agent_checked` is its only success response. Every refusal is an
`error`, `not_configured` naming settings. An M1 TOML file loads with
`summaries_dir` null and the denylist unconfirmed. Version 6 removes the denylist, `log_incognito`, and the
harness token (below).

Version 5 adds the required `ConfigSnapshot.adapters` document: `format` is
the adapter-template language version and `artifacts` contains at most 64
adapter tables. The JSON Schema bounds their outer shape; the host validates
their complete grammar, origins, scope, and IDs with the Rust
`adapter-templates` engine. In version 5 every adapter origin also had to
appear in the separately confirmed `sites` allowlist; version 6 removes
`sites`.
`config_result.adapter_warnings` identifies bounded pairs of rules that may
overlap (the earlier configured rule wins), while `adapter_issue` suspends
automatic capture for a malformed or unsupported on-disk adapter document.
The host still returns that document unchanged so a newer format is preserved
and repairable; notes and AI settings remain available. Existing TOML without
an `[adapters]` table loads as format 1 with no artifacts. Until alias
migrations arrive, an existing adapter's `id` template is immutable. In
version 5, adapter additions or changes required a native confirmation through
`confirm_config`, and an unscoped adapter on a shared origin added an explicit
every-account-in-this-Chrome-profile warning to it; version 6 removes that
confirmation, and an unscoped adapter still needs its `unscoped` acknowledgment.

Version 6 is the current protocol. The host refuses version 5 and earlier
requests with `unsupported_protocol_version`. It goes with standing HTTP(S)
access: the extension requests `http://*/*` and `https://*/*` at install, so
nothing in the protocol depends on a per-site Chrome grant.

Removed in version 6:

- The native config confirmation: `confirm_config`, `config_confirmed`, and
  `update_config.consent_token`. Every config change is saved with the
  revision check alone.
- The AI denylist and incognito flag: `ConfigSnapshot.agent_denylist`,
  `agent_denylist_confirmed`, and `log_incognito`, which the host now refuses
  as unknown keys. `AgentState` drops `denylist_unconfirmed`, so a harness
  committed by setup is enough for `ready`. `check_agent` carries no `url`, and
  `agent_checked` no `url_allowed`.
- The harness setup dialog and its token: `confirm_harness_setup`,
  `harness_setup_confirmed`, `update_config.harness_token`,
  `HarnessOffer.offer_id`, and `discover_harnesses.expected_revision`.
- The site allowlist: `ConfigSnapshot.sites` and `SiteConfig`, refused as an
  unknown key. Path-prefix narrowing is dropped with it; an adapter's own
  match rules are its path rule.

Added in version 6:

- `set_up_harness` names a detected `harness_id` with `expected_revision`,
  `env_names`, and `summaries_dir`. The host detects that harness again, runs
  one test prompt, and commits it, answering `config_updated`. A failed test
  run is `invalid_config` and changes nothing; `update_config` still cannot
  set a harness entry.
- `StorageConfig.knowledge_base_dir`, required and nullable: the absolute
  folder a summary agent reads as its knowledge base, with `null` meaning the
  notes folder. `choose_folder` now names a `purpose` (`notes_folder` or
  `knowledge_base`), and `update_config` has a required-nullable
  `knowledge_base_token`. A new or changed `knowledge_base_dir` needs a picker
  token minted for the `knowledge_base` purpose, and a notes-folder token
  cannot set it (or the reverse).

A site is on exactly when it has an adapter. The host records automatic
capture only on the exact origins of a valid adapter document, and none while
`adapter_issue` is set. Turning capture on needs only a notes folder.

A version 5 `config.toml` loads unchanged. A file that still has `sites`,
`agent_denylist`, `agent_denylist_confirmed`, or `log_incognito` keeps working,
those keys are ignored, and the host drops them on its next save. An M1 or
version 5 file has no knowledge base folder and loads with
`knowledge_base_dir` null.

`load_note` returns the host-derived `RecordKey` and any bounded older
URL-keyed notes that now resolve to the same artifact. `save_note` must echo
both that key and the note revision; if adapter policy changes the key before
save, the host returns `note_conflict` with reason `key_changed` and current
content. `use_artifact_note` moves one verified older note only when no
artifact-keyed note exists, publishing without clobber before removing the
source. `get_artifact`, `list_artifacts`, and `worklog` expose bounded derived
index views; `refresh_artifacts` runs the owned-file regeneration pass and
returns bounded conflict paths for unrelated occupants it did not replace.

Harness setup is two messages and has no native dialog. `discover_harnesses`
returns `harnesses_discovered` with at most eight offers. Each offer has its
`harness_id`, the binary the host found, its resolved `real_path`, `version`,
argument template, the environment names it requires, and optional names with
a `present` flag; values are never sent. A refused offer (an unsafe path, for
example) carries a `refusal`. `set_up_harness` names one `harness_id`, the
optional environment names to pass through, and `summaries_dir` (`null` keeps
the current folder or uses `summaries`). The host rediscovers that harness,
re-checks the binary's identity, runs one short test prompt, and only when it
passes commits the harness entry at `expected_revision`. The extension never
supplies a binary path.

`choose_folder` opens the native OS directory picker for one `purpose`. A
successful `folder_chosen` returns a canonical path and a five-minute,
single-use `picker_token` bound to the current revision, the path, and that
purpose. `update_config` requires a `notes_folder` token in `picker_token` when
`storage.root` changes, and a `knowledge_base` token in `knowledge_base_token`
when `storage.knowledge_base_dir` is set to a new folder. Both are
required-nullable fields: send `null` when one is unnecessary. The extension
cannot authorize a raw path. Picker cancellation leaves config unchanged.
Other config changes, including adding or removing adapters, turning capture
on or off, and changing `strip_params`, need only the revision check.

`record_batch` carries 1 to 64 immutable visit or activity events from one
local calendar day, a stable UUID batch ID, and an optional bounded overflow
marker. `batch_recorded` returns one outcome per event plus whether the
overflow marker is durable. The extension persists the batch ID and exact
overflow snapshot before sending so a lost acknowledgement replays the same
request. The host appends every surviving entry under one compact intent and
one log sync. `record_visit` remains as the upgrade path for older queues and
carries one immutable `VisitEvent` with a UUID event ID,
original URL, nullable title, timestamp, and incognito flag. The timestamp
is RFC 3339 with seconds precision and either `Z` or the local `+HH:MM`/`-HH:MM`
offset; the host files the visit under that offset's calendar date. The host
checks the original URL against its adapters and returns a
`visit_recorded` outcome of `persisted`, `suppressed`, `rejected`, or
`retryable`; only the first three are terminal for the extension queue.

Page notes work on any HTTP(S) page, whether or not an adapter covers it.
`load_note` carries the page's current URL; the host normalizes it the same
way as a logged visit and returns `note_loaded` with `exists`, a `revision`
(`missing` when no note exists yet, otherwise `sha256:` followed by 64
lowercase hex digits of the file's exact bytes), the note's `title`, and its
`body`. `save_note` carries the URL, a `title` (used only when the note does
not exist yet; the host preserves the existing title on every later save,
since the panel only edits the body), the new `body`, and `expected_revision`
from the panel's last load or save. A matching revision replaces the note
whole and returns `note_saved` with a new `revision`, `relative_path`, and an
`outcome` of `created`, `replaced`, `created_with_warning`, or
`replaced_with_warning` (the file is in place but a post-publication step,
such as a directory sync, warned; not a failure and not retried). A stale
`expected_revision` refuses the write and returns `note_conflict` with the
note's current `exists`, `revision`, `title`, and `body` instead, so the
panel can reload the newer note and show the text it could not save. Before
replacing, the host checks the existing file's recorded canonical URL and
`brauser:` ownership (absent or `page` `kind`, never `summary`); a mismatch
is a `conflict` error, and the file is never adopted or overwritten.

The host remains responsible for runtime validation of string lengths,
absolute paths, adapter origins, storage confinement, picker-token authority,
and whether a requested config change is authorized.
