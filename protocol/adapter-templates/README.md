# Adapter-template test vectors

`vectors.json` is the shared oracle for the two adapter-template engines: the Rust crate in the host and the TypeScript module in the extension. See `adapter-templates.md` §12. The expected results were derived from the specification (`adapter-templates.md`, accepted 2026-09-28, format version 1), not from either engine. When a vector and an engine disagree, one of three things is wrong: the engine, the vector, or the specification. Settle which by reading the spec section named in the vector's `why`.

Where the specification leaves something open, this file defines it. That covers error granularity, the order of checks, error locations, and the readings listed under "Spec questions". Both engines must follow those definitions.

## File layout

```json
{
  "vectors_format": 1,
  "spec": "adapter-templates.md, accepted 2026-09-28, format version 1",
  "fixtures": { "<name>": <adapter document>, ... },
  "cases": [ <case>, ... ]
}
```

An **adapter document** is the JSON form of the `[adapters]` TOML table:

```json
{"format": 1, "artifacts": [{"type": "...", "origin": "...", "match": [{"path": "...", "query": {}, "absent": [], "mode": "..."}], "id": "...", ...}]}
```

Adapter keys are `type`, `origin`, `match`, `id`, `title`, `alias`, `refs`, `modes`, `scope`, `unscoped`, `path_case`, `fields`, and `within`. Rule keys are `path`, `query`, `absent`, and `mode`. Some validate vectors deliberately break this shape. A harness must pass `doc` to the engine unchanged and must not repair it first.

A case's `doc` field (called `existing` for `draft` and `draft_rules` cases) is either an inline adapter document or `{"$fixture": "<name>"}`, which refers to an entry in `fixtures`. `$fixture` is never a real document key.

### String builder

Any string-valued field in a case may instead be a builder object:

```json
{"$build": ["prefix", {"repeat": "x", "times": 505}, {"utf16": [55357]}, " - Mail"]}
```

Concatenate the parts in order. The part types are:

- A plain string.
- `repeat`: the string repeated `times` times.
- `utf16`: a sequence of raw UTF-16 code units. It may contain lone surrogates.

Each harness passes the result to its engine in its own way:

- The TypeScript harness builds a JS string with `String.fromCharCode`. Lone surrogates stay in the string, and the engine must apply `toWellFormed()` itself (§9.1).
- The Rust harness decodes each `utf16` part with `String::from_utf16_lossy`. That is exactly the host's well-formedness step.

Two `utf16` parts are never adjacent, so a surrogate pair is never split across parts. Builders appear only in `url`, `title`, and expected `title` values.

JSON strings in the file escape invisible and combining characters (` `, `́`, `﻿`) so that they can be reviewed.

## Common case fields

| Field | Meaning |
|---|---|
| `id` | Unique, dotted, stable. Prefix is the kind. |
| `kind` | See below. |
| `why` | One line starting with the spec section (`§5.1: ...`, or `DESIGN 6.2: ...`). |
| `impls` | `["rust", "ts"]` or `["rust"]`. The worker does not validate everything, overlap-check, or judge drafts, so host-only kinds are `["rust"]`. |
| `sq` | Optional. Spec-question tags whose reading this case depends on. See "Spec questions". |
| `pending` | Optional. A spec-question tag. The harness skips the case, but must still parse it, until that question is settled. |
| `budget_ms` | Optional. Bounds cases (§12): the call must complete within this many milliseconds on CI hardware. |
| `expect` | Kind-specific expected result. Compare it by deep equality. Object key order is not significant. |

## Kinds

Case counts are at the end of this file.

### `validate` (rust, ts)

- **Input:** `doc`.
- **Expected result:** either `{"ok": true}` or `{"error": {"code", "adapter", "rule", "entry"}}`.

The `error` fields are:

- `adapter`: the index into `artifacts`, or `null` for document-level errors.
- `rule`: the index into `match`, or `null`.
- `entry`: the index into a title list or into `refs`, or `null`.

§10.3 also asks for a character position. Vectors do not pin positions, so engines may report one and harnesses ignore it. The Rust host reports a `field` (the path, a query value, an absent entry, the ID, a title entry, or the alias) and a character offset into it; the TS engine reports neither. Each document carries exactly one defect unless a case says otherwise. When a document has several defects, the first error in the check order below is the one reported.

The TS engine validates only what the worker needs (§12). The TS harness must still produce the same code for every validate vector, because the worker must reject a config the host rejects. If that becomes impractical, change a case's `impls` to `["rust"]`. Do not weaken the expected code.

### `evaluate` (rust, ts)

- **Input:** `doc`, `url`, and an optional `title`.
- **Expected result, no match:** `{"match": false, "reason": R}`.
- **Expected result, match:** `{"match": true, "adapter", "rule", "type", "captures", "artifact_id", "mode"}`.
  - `captures` maps every bound capture to its canonical value. That includes `mode` and scoped captures. An unbound optional capture is omitted.
  - `mode` is `"view"`, `"edit"`, `"review"`, or `null`.
- **Additional fields when `title` is present** (and the URL matched):
  - `title`: the extracted title, or the whole prepared title for the fallback.
  - `title_entry`: the index of the matching entry, or `null` for the fallback.
  - `alias_id`: the suggested tab-title alias ID, or `null`.
  - `refs`: `[{"type", "value"}]`, in scan order.
- **Additional field when the adapter declares `within`** (§8.3): `within: {"type", "artifact_id"}`, the container's type and built ID. The key is absent when the adapter declares no `within`, and also when the container ID would exceed 512 bytes; the match stands either way.

Reasons, in the order they are decided:

| Reason | When |
|---|---|
| `url_too_long` | More than 2048 scalar values, checked before parsing (§5.1 step 1) |
| `url_invalid` | WHATWG parse failure |
| `no_adapter` | The URL is not `http(s)`, or no adapter's `origin` equals the URL's origin. Opaque origins (`about:`, `data:`, `chrome:`) never match, and neither does a `blob:` URL, whose origin is its creator's (SQ-V24). |
| `path_rejected` | §5.1 steps 3–6 failed. This is decided once per URL, before any rule. |
| `no_rule` | No rule of any same-origin adapter matched |
| `out_of_scope` | The first matching rule's adapter failed scope. This result is terminal: later adapters are not tried (SQ4). |
| `id_too_long` | The built ID exceeds 512 bytes (§8.1) |

The harness also checks these invariants on every match:

- Every canonical value is ASCII and contains no `/`, `:`, `{`, `}`, `%`, whitespace, or controls.
- `artifact_id` is at most 512 bytes.
- `artifact_id` splits at its first `:` and at each `/` into the scheme, `{host}`, and the parts.

### `capture` (rust, ts)

- **Input:** `ctype` (the ctype text of a path capture, for example `"int(3)"`, `"token|lower"`, `"hex(32)@tail"`, or `"(edit|view)"`) and `value` (one decoded segment).
- **Expected result:** `{"canonical": v}`, where `v` is `null` for an invalid value.
- Alternations compare exactly here, as they do in queries.

### `segments` (rust, ts)

- **Input:** `path`, a raw pathname as WHATWG serializes it (it may be synthetic).
- **Expected result:** `{"segments": [..]}` after §5.1 steps 3–6, or `{"segments": null}` for "matches no rule".

### `refs` (rust, ts)

- **Input:** `find` (`jira_key`, `int`, `hex`, or `uuid`) and `text`.
- **Expected result:** `{"values": [..]}`, the distinct canonical values in first-seen order, at most 20.

The scan (§10.1) works like this:

1. For each position `p` where `p` is 0 or the character before `p` is a boundary, take the single candidate that starts at `p`. A boundary is any character that is not ASCII `[A-Za-z0-9_]`.
   - `jira_key`: an uppercase letter, then the maximal run of `[A-Z0-9_]`, then `-`, then the maximal digit run.
   - `int`: the maximal digit run.
   - `hex`: the maximal hex run.
   - `uuid`: the next 36 characters.
2. Accept the candidate when the character after it is a boundary or the end of the text, and the candidate is a valid value of the type with its default bounds.
3. Continue at `p + 1`.

### `refs_resolve` (rust)

- **Input:** `doc`, `refs` (`[{type, value}]`), and an optional `self_id`.
- **Expected result:** `{"resolved": [{type, value, artifact_id}]}`.

A ref resolves when exactly one adapter has the target type and that adapter's ID template has exactly one placeholder besides `{host}`. The value is canonicalized with that placeholder's capture type, and a value that is invalid for the type stays unresolved (`artifact_id: null`). A ref whose resolved ID equals `self_id` is dropped.

### `meta_alias` (rust)

- **Input:** `doc`, `url`, and `content` (the `<meta>` element's `content`).
- **Expected result:** `{"alias_id": id | null}`.

The whole `content` must be a valid value for the capture. There is no alias when the value is invalid, when it equals the primary ID, or when the ID would exceed 512 bytes.

### `overlap` (rust)

- **Input:** `doc`, `a: [adapter, rule]`, and `b: [adapter, rule]`.
- **Expected result:** `{"overlap": "may_overlap" | "disjoint"}`.

This is the conservative §10.3 comparison:

- Segment count ranges are compared first, then position by position:
  - literal against literal compares with ASCII folding if either adapter is insensitive;
  - literal against capture tests membership;
  - capture against capture intersects alphabets and length ranges;
  - `{_}` accepts anything;
  - an optional position accepts its set, or anything once a rest follows;
  - `@tail` is treated as any segment, even against a literal (SQ-V7).
- For queries (SQ-V6), the pair is disjoint only when a key is required in one rule and in `absent` in the other, or when both rules condition the same key and those conditions cannot share a value: two different literals, or disjoint type length ranges or alphabets. A condition in only one rule never proves disjointness.

### `overlap_report` (rust)

- **Input:** `doc`.
- **Expected result:** `{"pairs": [{"first": [a, r], "second": [a, r]}]}`.
  - Each pair has rules from different adapters on the same origin that may overlap.
  - `first` is the earlier rule in config order, the one that wins.
  - Pairs are ordered by `first`, then by `second`.
  - A declared fall-through is not a pair: when the earlier adapter declares `within` whose type is the later adapter's type on the same origin (§8.3), none of their rule pairs are reported. The reverse order, and the same rules without `within`, are reported.

### `draft` (rust)

- **Input:** `existing` (a document), `draft` (one adapter table), `probes` (URLs), and `search_keys`.
- **Expected result:** one of:
  - `{"ok": true, "scope": <copied scope or null>}`;
  - `{"draft_error": code}`;
  - `{"error": <validation error>}`. The draft is validated as a one-adapter document, so `adapter` is 0 (SQ-V10).

The host checks a draft in this order (§10.4):

1. `draft_forbidden_key`
2. On a shared origin, the scope source: `draft_no_scope_source` or `draft_scope_conflict`. Scopes compare as maps of sets (SQ-V5). The copied scope is the first scoped adapter's scope.
3. Validation of the draft with the copied scope. Failures are reported as `error`.
4. Per rule: `draft_no_leading_literal`, then `draft_rest_second`.
5. `draft_id_needs_typed_capture`
6. `draft_search_capture`: a capture used in the ID is bound from a query key in `search_keys`. This check comes from DESIGN §5.1 and is kept by §10.4.
7. `draft_matches_probe`: probes are the origin root, which is always probed, plus `probes`. Rules are matched without applying scope (SQ-V3).
8. `draft_overlap`: any rule of any existing adapter on the origin may overlap.

### `draft_rules` (rust)

The per-rule checks both Capture Site drafters use (DESIGN §5.1). A failing drafted rule is dropped and the rest of the draft is kept. A failing user-written rule only warns and is kept.

- **Input:**
  - `existing` (a document) and `draft` (one adapter table). The table is the checked table: only the Tier 0 keys, before the review's scope, unscoped and title overlays.
  - `provenance`: optional, one of `"drafted"` or `"user_written"` per rule. When absent, every rule is drafted. A rule without an entry counts as drafted.
  - `accepted`: optional, the earlier-ranked drafted adapter documents.
  - The policy is the host catalog, `protocol/capture-site/catalog.json`. `extra_probe_paths` (optional) appends probe paths to its `probe_paths`.
  - `choice`: optional, the review's scope choice: `null`, `"unscoped"`, or `{"values": [..]}`.
- **Expected result:** one of:
  - `{"scope": S, "rules": [V, ..]}`, plus `"resolved": R` when `choice` is present;
  - `{"draft_error": code}`, for a refusal of the whole draft;
  - `{"error": <validation error>}`, for an adapter-level validation failure (`rule` is `null`).
- `S` is `"not_shared"`, `"unscoped_only"`, `{"pending": capture}`, or `{"copied": scope}`.
- `V` is `"kept"`, `{"dropped": code}`, or `{"warned": code}`.
- `R` is `"not_shared"`, `"unscoped"`, `{"scoped": scope}`, or `{"scope_error": code}`.

The host checks the rules in this order. The first failing check decides each verdict.

1. `draft_forbidden_key` refuses the whole draft when any rule is drafted. When every rule is user-written, every rule is warned with it instead.
2. The scope state, on a shared origin:
   - `unscoped_only` where the catalog has no tenant position, whatever the existing adapters are scoped by;
   - otherwise `copied` when every scoped adapter on the origin agrees (SQ-V5, SQ-V17);
   - otherwise `pending`, naming the catalog's tenant capture: there is no scoped adapter, or they disagree.
3. Each rule is validated as a one-rule adapter with the copied scope. While scope is pending, a stand-in scope of the tenant capture is used, and where only unscoped is possible, `unscoped`. A rule that fails is dropped as `draft_invalid_rule`, whoever wrote it, because it can never be saved. A failure with no rule index refuses the whole draft as `error`.
4. Per rule: `draft_no_leading_literal`, `draft_rest_second`, `draft_blocked_route` (a path literal or alternative is a catalog `blocked_route_words` entry, compared with ASCII folding, by its `-`/`_`/`.` parts and their join, with a trailing number and a plural `s` folded), `draft_search_capture`, `draft_paging_capture` and `draft_secret_capture` (an ID capture bound from a `search_keys`, `paging_keys` or secret query key: a `secret_query_keys` entry, a key containing a `secret_query_key_parts` fragment or ending in a `secret_query_key_suffixes` entry, compared with case folded and `-`, `_`, `.` removed), `draft_matches_probe` (the origin root and each catalog probe path on the draft's origin, matched without scope, SQ-V3), and `draft_overlap` (with `existing`, then with `accepted`). The survivors are then validated together in rule order, and a rule that conflicts with an earlier survivor is dropped as `draft_invalid_rule`.
5. `draft_id_needs_typed_capture`, judged on the survivors. It refuses the whole draft when any rule is drafted, and warns every surviving rule otherwise.

`resolved` judges `choice` against the scope state after the checks, and never changes a verdict:

- Pending scope accepts `"unscoped"` or non-empty `values` that are canonical `name|lower` values, at most 64 after duplicates are removed. No choice, or an empty list, is `draft_scope_pending`; a bad value is `draft_scope_choice`.
- Unscoped-only accepts only `"unscoped"`.
- Copied and not-shared scope accept only `null`; copied scope resolves to the host's copy.
- Any other choice is `draft_scope_choice`.

## Error codes (`validate`)

These are the complete format-1 enum. Every code appears in at least one vector.

| Area | Codes |
|---|---|
| Document | `missing_format`, `invalid_format`, `unsupported_format`, `too_many_adapters` |
| Generic | `unknown_key`, `missing_key`, `wrong_type` |
| Adapter | `bad_adapter_type`, `reserved_adapter_type`, `bad_origin`, `bad_path_case`, `no_rules`, `too_many_rules` |
| Path | `path_syntax`, `bad_capture_name`, `reserved_capture_name`, `unknown_capture_type`, `bad_bounds`, `case_mod_not_allowed`, `tail_not_allowed`, `bad_capture_literal`, `rest_not_last`, `optional_misplaced`, `no_literal_segment`, `duplicate_capture`, `too_many_segments`, `too_many_alternatives`, `literal_too_long`, `template_too_long` |
| Query | `too_many_query`, `too_many_absent`, `bad_query_key`, `bad_query_value`, `optional_in_query`, `query_key_conflict` |
| Rule mode | `bad_mode` |
| Across rules | `capture_type_conflict`, `extra_capture` |
| Modes | `bad_modes_value`, `mode_missing_mapping`, `mode_not_alternation`, `mode_in_id` |
| ID | `id_syntax`, `id_reserved_scheme`, `id_unknown_capture`, `id_optional_capture` |
| Scope | `shared_origin_unscoped`, `scope_and_unscoped`, `unscoped_not_shared`, `bad_unscoped`, `scope_empty_list`, `too_many_scope_values`, `scope_capture_not_bound`, `scope_bad_value` |
| Title | `title_too_long`, `title_syntax`, `title_unbounded_capture`, `title_capture_swallows`, `title_unknown_capture`, `title_type_mismatch`, `too_many_titles`, `empty_title_list` |
| Alias | `bad_alias`, `alias_capture_not_id`, `alias_capture_not_in_title`, `alias_type_mismatch` |
| Refs | `too_many_refs`, `bad_ref`, `bad_ref_find` |
| Within | `within_bad_type`, `within_self`, `within_id_syntax`, `within_id_reserved_scheme`, `within_unknown_capture` |

The draft codes are `draft_forbidden_key`, `draft_no_scope_source`, `draft_scope_conflict`, `draft_no_leading_literal`, `draft_rest_second`, `draft_id_needs_typed_capture`, `draft_search_capture`, `draft_matches_probe`, and `draft_overlap`. The `draft_rules` kind adds `draft_blocked_route`, `draft_paging_capture`, `draft_secret_capture`, `draft_invalid_rule`, `draft_scope_pending`, and `draft_scope_choice`.

### Check order

The first failing check is reported.

1. **`format`.**
   - `format` absent while `artifacts` is non-empty gives `missing_format`. With no adapters, a missing `format` is valid.
   - A `format` that is not a positive integer by value gives `invalid_format`. `1.0` is 1, and `2.0`, `1e300`, and integers past 2^63 − 1 are positive integers (SQ-V20).
   - A positive integer other than 1 gives `unsupported_format`. That is "newer than host", decided before anything else is read (SQ14).
2. **Unknown document keys:** `unknown_key` (SQ16).
3. **`artifacts`:** a non-array gives `wrong_type`. More than 64 entries gives `too_many_adapters`.
4. **Each adapter, in index order:**
   1. **Keys.**
      - An adapter that is not a table gives `wrong_type`.
      - Unknown keys give `unknown_key`.
      - A missing `type`, `origin`, `match`, or `id` gives `missing_key`.
      - A mistyped value gives `wrong_type`. `within` must be a table with exactly the keys `type` and `id`, both strings.
   2. **`type`, `origin`, `path_case`.**
      - `type`: `bad_adapter_type`, or `reserved_adapter_type` for `web.page`.
      - `origin`: `bad_origin`. The origin must equal its own WHATWG origin serialization and use `http` or `https`, and its host must be an IP address or an ASCII hostname (SQ15).
      - `path_case`: `bad_path_case`.
   3. **`match`.** An empty `match` gives `no_rules`. More than 16 rules gives `too_many_rules`. Then each rule in order:
      1. **Rule keys:** `unknown_key`, `missing_key` (for `path`), `wrong_type`. Every `query` value and every `absent` entry must be a string here, before the path is parsed.
      2. **Path parse, left to right.** A segment that starts with `{` must end with `}`.
         - Content `_` is anonymous and `*` is a rest.
         - Otherwise, if the content has no `:`, the error is `path_syntax`.
         - Otherwise the name is the text before the first `:`, with a trailing `?` marking an optional segment. A name that breaks the name grammar gives `bad_capture_name`. The names `host` and `title` give `reserved_capture_name`.
         - The type name is the maximal run of `[A-Za-z0-9_]`. An empty run that is not an alternation gives `path_syntax`. A non-empty run that is not one of the six types gives `unknown_capture_type`.
         - Then come optional bounds, `|lower`/`|upper`, and `@tail`, in that order, and then the end of the segment. Anything else is `path_syntax`. The parts are checked left to right and the first error wins, so `token@tail|lower` gives `tail_not_allowed`, `int@tail|lower` gives `path_syntax`, and `int(0)@head` gives `bad_bounds` (SQ-V12).
         - Bound numbers with a leading zero are `path_syntax` (SQ-V1). Bounds that widen, reverse, or are 0 give `bad_bounds`, and bounds on `uuid` give `bad_bounds`.
         - Modifiers on the wrong type give `case_mod_not_allowed` or `tail_not_allowed`. Bounds or modifiers after an alternation give `path_syntax`.
         - A captured-alternation literal outside `[a-z0-9._-]`, or equal to `.` or `..`, gives `bad_capture_literal`.
         - Parsing also enforces the size limits: `literal_too_long` and `too_many_alternatives`.
      3. **Path structure:**
         1. `template_too_long`
         2. `too_many_segments`
         3. `rest_not_last`
         4. `optional_misplaced`
      4. **`query`.**
         - `too_many_query`
         - Then each condition in ascending key order by Unicode code point, not document order (SQ-V21):
           - An empty key gives `bad_query_key`.
           - A value is checked this way:
             - `{_}` is presence.
             - A value that starts with `{` and ends with `}` is parsed as a capture. It is rejected with `optional_in_query` if optional. Parse errors that would be `path_syntax` become `bad_query_value`. The other capture errors keep their codes. `{*}` is not a query value (`bad_query_value`), `{_:int}` gives `bad_capture_name`, and `{host:…}` or `{title:…}` gives `reserved_capture_name`.
             - Otherwise the value is a literal. A literal that is empty or contains `{`, `}`, or a control character gives `bad_query_value`.
      5. **`absent`:** `too_many_absent`, an empty key gives `bad_query_key`, and a key that is also in `query` or listed twice gives `query_key_conflict`.
      6. **Rule `mode`:** `bad_mode`.
      7. **Captures:** `duplicate_capture`, across path and query.
   4. **`capture_type_conflict`.** Same-named captures in different rules must have an identical ctype: type, bounds, and modifiers. The error names the later rule. `mode` is exempt: each rule's mode alternation may differ, and §5.4 checks each one against `modes` (SQ-V13).
   5. **`modes`:**
      1. `mode_not_alternation` (per rule)
      2. `mode_missing_mapping` (per rule)
      3. `bad_modes_value`
   6. **`id`:**
      1. `id_syntax`
      2. `id_reserved_scheme`
      3. `mode_in_id`
      4. per rule, `id_unknown_capture` and `id_optional_capture`
      5. per rule, `extra_capture`. The exempt captures are `mode` and scoped captures.
   7. **`scope` / `unscoped`:**
      1. `scope_and_unscoped`
      2. `bad_unscoped`
      3. `unscoped_not_shared`
      4. `scope_empty_list` (also for an empty `scope` table)
      5. `too_many_scope_values`: more than 64 values in any one list (SQ-V25)
      6. `scope_capture_not_bound`, per rule. A scoped capture that is optional counts as not bound.
      7. `scope_bad_value`: each listed value must be the canonical value of the capture as at least one rule binds it. Only `mode` can differ between rules (SQ-V22).
      8. `shared_origin_unscoped`
      9. `no_literal_segment`, per rule, unless some required path segment is an anchor (§4.1) or is a required path capture whose name is a `scope` key. A capture bound from a query value never anchors, and an `unscoped` adapter gets no scope anchor. This check follows scope so that scope keys are in view.
   8. **`title`.**
      - For the list: a value that is neither a string nor a list gives `wrong_type`. Then `empty_title_list` and `too_many_titles`.
      - For each entry, which sets `entry`:
        1. `wrong_type`
        2. `title_too_long`: more than 512 scalar values in the entry (SQ-V25)
        3. `title_syntax`
        4. `title_unbounded_capture`
        5. `title_unknown_capture`
        6. `title_type_mismatch`: the type name must match. Title bounds are optional, but if present they must equal the path capture's effective bounds (SQ26). A `uuid` title capture takes no bounds at all. A capture named `mode` refers to an alternation, which no title type matches (SQ-V23).
        7. `title_capture_swallows`
      - Title captures take no modifiers, alternations, or `@tail`. Any of those gives `title_syntax`.
   9. **`alias`:**
      1. `wrong_type`
      2. `bad_alias`: the shape is exactly `{from="tab_title", capture}` or `{from="meta", name (non-empty), value}`, and `value` is a single non-optional capture with nothing around it.
      3. `alias_capture_not_id`
      4. `alias_type_mismatch` (meta)
      5. `alias_capture_not_in_title` (tab_title: at least one entry must capture it)
   10. **`refs`:**
       1. `wrong_type`
       2. `too_many_refs`: more than 8 entries, before any entry is checked (SQ-V25)
       3. `bad_ref`: the shape is exactly `{type, find}`, and `type` must follow the adapter-type grammar.
       4. `bad_ref_find`: only `jira_key`, `int`, `hex`, and `uuid` are allowed, with no bounds (SQ21).
   11. **`fields`:** `wrong_type` unless it is a table of strings.
   12. **`within`** (§8.3), typed in step 1:
       1. `within_bad_type`: `type` breaks the adapter-type grammar or is `web.page`.
       2. `within_self`: `type` equals the adapter's own `type`.
       3. `within_id_syntax`: `id` breaks the §8.1 grammar.
       4. `within_id_reserved_scheme`: the scheme is `web`.
       5. `within_self`: `id` is the adapter's own ID template, scheme and part by part, which would give the artifact and its container one record key.
       6. `within_unknown_capture`, per rule: a `{name}` that is not a capture the rule binds and the adapter's `id` uses or `scope` lists, or that is `mode`. A second `{host}` is such a name (SQ-V19). Because the names come from the ID and scope, step 6's `extra_capture` is unchanged.

## Fixtures

| Name | Contents |
|---|---|
| `starters` | 0 `jira.issue` and 1 `confluence.page` on `acme.atlassian.net` (§11), 2 `gdoc` (§11), 3 `github.pr` scoped to `acme-corp` (§11), 4 `github.issue` (synthetic, same scope, DESIGN 6.8), each `within` its `github.repo`, 5 the `github.repo` catch-all `/{owner}/{repo}/{*}` (same scope, no title, after the PR and issue adapters it contains, §8.3), 6 `sharepoint.doc` on `acme.sharepoint.com`, and 7 `sharepoint.doc` on `acme-my.sharepoint.com` (OneDrive for work, SQ-V2) |
| `github_without_repo` | The `starters` PR and issue adapters (with `within`, without titles or refs) and no repository catch-all, for GitHub drafts that the catch-all would overlap (§10.4) |
| `notion_starter` | The §11 Notion adapter verbatim, anchored by its fixed-shape `hex(32)` capture (§4.1). |
| `mail` | A prefix title list: `["({~}) {title} - Mail", "{title} - Mail"]` |
| `tracker` | Title captures that must agree with path captures, with a tab-title alias on `num` |
| `long_jira` | An ID that repeats its capture, used to reach the 512-byte ID cap |
| `query_lab`, `mode_lab`, `ci_lab`, `tail_lab`, `order_lab`, `port_lab` | Query conditions, mode sources, `path_case`, `@tail`, adapter and rule order, and ports |
| `scope_google`, `scope_notion`, `scope_figma`, `scope_linear`, `scope_multi` | Scope on each shared origin (GitHub is covered by `starters`) |
| `sq4_lab` | A scoped and an unscoped GitHub adapter, for the terminal scope result |
| `bounds_64x16` | 64 adapters × 16 rules |
| `overlap_lab` | One rule per adapter, for the pairwise overlap cases |
| `scope_lab` | Scope from a query capture, scope across two rules of one adapter, a scoped adapter whose ID exceeds 512 bytes ahead of an unscoped one, and a scoped captured alternation |
| `ci_capture_lab` | Captures and query conditions on `path_case = "insensitive"` adapters |
| `query_canon_lab` | Query captures with `@tail`, `hex`, and `name\|lower` |
| `url_lab` | Raw characters the parser encodes, IPv4, IPv6, and IDN origins, rule precedence, and a required path mode capture |
| `title_lab` | A leading skip before a capture, a title list with the alias in the second entry, and a `uuid` title capture |
| `bounds_ci_alts` | 64 `path_case = "insensitive"` adapters × 16 rules, each an alternation segment and a 16-literal `mode` alternation, for the folding cost of a long segment |
| `fold_lab` | Insensitive literal, alternation, and captured-alternation segments containing `k` and `s`, for ASCII-only folding (U+212A KELVIN SIGN, U+017F LONG S) |
| `refs_lab` | Two ref specs whose hits exceed the 20-ref cap, two types that find the same values, and 8 specs that share one `find` |
| `long_once` | An ID that uses each of four `token` captures once, for the 512-byte ID cap without repetition |

## Spec questions

Tags in `sq` mean that a case depends on the reading given here. If the specification settles a question differently, update the tagged cases.

| Tag | Section | Reading used |
|---|---|---|
| SQ2 | §5.1 | Dot segments are resolved by the WHATWG parser before step 3, including `%2e`. Step 5 therefore fires only on synthetic `segments` input. |
| SQ4 | §3, §6 | Scope is checked on the first matching rule, and a failure there is terminal (`out_of_scope`). |
| SQ5 | §6, §8.1 | `scope` is allowed on an origin that is not shared. `unscoped` is an error there, and so is having both keys. An empty list or table is an error. An optional capture cannot be scoped. Differing modifiers are a `capture_type_conflict`. |
| SQ6 | §4.1, §7.1 | `jira_key` bounds apply to the project part. `uuid` takes no bounds. |
| SQ7 | §7.1, §9.1, §10.1 | `uuid` accepts exactly one pair of braces, or none. In refs, braces act as boundaries. For title runs and bounding, the alphabet is hex digits and `-` (SQ-V16). |
| SQ8 | §4.2, §5.1 | Under `path_case = "insensitive"`, a captured alternation compares folded and binds the template literal. |
| SQ9 | §8.1, §9.1 | `host` and `title` are reserved capture names. |
| SQ11 | §5.3 | `path_case` never affects query keys or values. |
| SQ12 | §5.3 | The query value grammar is as given in the check order. `@tail` and alternations are allowed in query captures. Optional captures are not. |
| SQ13 | §5.3 | A key in both `query` and `absent`, or listed twice in `absent`, gives `query_key_conflict`. |
| SQ14 | §3 | A non-positive-integer `format` gives `invalid_format`. A different positive integer gives `unsupported_format`, before any other check. |
| SQ15 | §3, §5.1 | `origin` must be `http(s)` and must equal its own WHATWG serialization. Uppercase letters, a default port, a path, credentials, or Unicode hosts are errors. The host must also be an IP address or a hostname of only `a-z`, `0-9`, `-`, `.`, and `_`: WHATWG accepts hosts such as `*.example.com`, which look like wildcards but would match only that literal host, so they are rejected rather than accepted as an adapter that matches nothing. |
| SQ16 | §3 | Unknown keys at every level fail closed. |
| SQ17 | §5.1 | The 2048 cap counts scalar values of the raw input, before parsing. |
| SQ18 | §5.4 | `modes` keys that no rule uses are ignored. |
| SQ21 | §10.1 | Only `jira_key`, `int`, `hex`, and `uuid` can be ref types. The fallback title is scanned too, prefix included. Self-mentions are reported by the scan and dropped at resolution. The 20-value cap applies to distinct values. |
| SQ22 | §8.1 | An ID may repeat a capture, or have no parts after `{host}`. |
| SQ23 | §8.1 | Only the exact scheme `web` and the exact type `web.page` are reserved. |
| SQ24 | §9.1 | "Whitespace" means Unicode White_Space. U+FEFF is not trimmed. |
| SQ25 | §9.1 | The fallback records the whole prepared title (well-formed and capped), untrimmed. |
| SQ26 | §9.1 | Title bounds are optional. When present, they must equal the path capture's effective bounds. |
| SQ27 | §9.3 | `alias` is one table. A tab-title alias capture must appear in some title entry. A meta capture must have the ID capture's type. |
| SQ28 | §10.4 | Neither `{_}` nor a plain alternation is a capture. A plain alternation does not satisfy "a literal segment before its first capture". |
| SQ29 | §11 | The `<your-site>` placeholders are not valid origins. The vectors use `acme`. |
| SQ-V1 | §4.1 | Bound numbers use the same no-leading-zero form as `int`, so `int(03)` gives `path_syntax`. |
| SQ-V2 | §11 | `/personal/` URLs are served from `<tenant>-my.sharepoint.com`, which is a different origin from `<tenant>.sharepoint.com`. The SharePoint starter therefore needs a second adapter for OneDrive for work. Its ID uses its own `{host}`. |
| SQ-V3 | §10.4 | Probe URLs are matched by rule without scope, so a draft that would match a probe from another org is still rejected. |
| SQ-V4 | §10.4 | The origin root can never match, because a template needs a literal segment and `/` has none. The root probe is kept for DESIGN §5.1 parity. |
| SQ-V5 | §10.4 | "Scopes differ" compares scopes as maps of sets, so order does not matter. |
| SQ-V6 | §10.3 | For queries, a key required in one rule and absent in the other, or two different literals on one key, prove disjointness. A condition in only one rule never does. |
| SQ-V7 | §10.3 | `@tail` is compared as "any segment", even against a literal. |
| SQ-V8 | §10.3 | The spec does not fix error granularity or check order. This README defines them. |
| SQ-V9 | §10.3 | Character positions are not pinned by the vectors. |
| SQ-V10 | §10.4 | A draft is validated as a one-adapter document, so the adapter index is 0. |
| SQ-V11 | §6, §8.1 | Scope is decided before the ID is built, and `id_too_long` is terminal like `out_of_scope`: later adapters are not tried. |
| SQ-V12 | §4.1 | Ctype parts are checked left to right and the first error wins. |
| SQ-V13 | §5.4, §8.1 | The identical-ctype rule does not apply to `mode`. Mode alternations may differ per rule, and §5.4 checks each against `modes`. Other same-named captured alternations must still be identical. |
| SQ-V14 | §8.1 | `.` and `..` are valid ID literal parts, and schemes such as `a.` and `a..b` are valid, because the grammar allows them. IDs are opaque strings, not paths. |
| SQ-V15 | §9.3 | A meta value capture must have exactly the ID capture's ctype (type, bounds, and modifiers). The meta value is canonicalized with it before it is compared with the primary ID. |
| SQ-V16 | §7.1, §9.1 | The `uuid` title alphabet is hex digits and `-`, following SQ7: braces are not in it. A braced UUID in a title ends the capture's run at the brace, so that entry does not match. Braces are still accepted in path and query values (§7.1). |
| SQ-V17 | §10.4 | Only scoped adapters are scope sources. Unscoped adapters on the origin are ignored when the scope is copied. |
| SQ-V18 | §10.4 | Rules are checked in order. Within a rule, the leading-literal check comes before the rest-position check. |
| SQ-V19 | §8.1 | The ID grammar has no rule against a second `{host}`: it parses as an ordinary `{name}` part. No rule can bind the reserved name `host`, so the error is `id_unknown_capture`. |
| SQ-V20 | §3 | "Positive integer" is judged by value, not by how the number is written: `1.0` is format 1, and `2.0`, `1e300`, and `9223372036854775808` are `unsupported_format`. A JSON-parsing worker cannot tell `1.0` from `1`, so this is the only reading both engines can share. |
| SQ-V21 | §10.3 | Query conditions are checked in ascending key order by code point. A worker's parsed object puts integer-like keys first and loses document order, so document order cannot be shared. |
| SQ-V22 | §6 | "A value no URL could produce" is read literally: a scope value is valid when at least one rule's capture of that name can produce it. This matters only for `mode`, whose alternation may differ per rule (SQ-V13). |
| SQ-V23 | §9.1 | A title capture named `mode` names a real capture, so it is `title_type_mismatch` rather than `title_unknown_capture`. |
| SQ-V24 | §5.1 | Only `http(s)` URLs are evaluated. A `blob:` URL has a tuple origin, its creator's, but is `no_adapter`. |
| SQ-V25 | §9.1, §10.3 | Three limits bound per-event work: 8 `refs` entries (`too_many_refs`), 64 values per `scope` list (`too_many_scope_values`), and 512 scalar values per title entry (`title_too_long`). Each is checked as soon as its list or string is read, ahead of the checks on its contents. |

## Case counts

| Kind | Cases | Impls |
|---|---|---|
| validate | 603 | rust, ts |
| evaluate | 392 | rust, ts |
| capture | 119 | rust, ts |
| segments | 50 | rust, ts |
| refs | 37 | rust, ts |
| overlap | 35 | rust |
| draft | 38 | rust |
| draft_rules | 35 | rust |
| meta_alias | 9 | rust |
| refs_resolve | 8 | rust |
| overlap_report | 8 | rust |
| **Total** | **1334** | |

## Maintenance

The file is generated by an author-side script. The expected values were written by hand from the spec, so the generator only assembles cases. Before this file was committed:

- An independent throwaway checker re-derived every non-pending `capture`, `segments`, `refs`, and `evaluate` result (545 cases), and all agreed.
- Every evaluate URL was parsed by both Node's `URL` and the Rust `url` 2.5.8 crate, and they agreed on origin, host, path, and query pairs.

The eight `eval.bounds` cases added later (`longest_uuid`, `4_title_entries_*`, `title_512_*`, and `url_2048_*`) and the budgets on `eval.long_jira.exactly_512` and `eval.long_jira.514` were derived by hand, then checked against both engines.

The limit, folding, trimming, refs-cap, and `long_once` cases added with SQ-V25 were derived by hand, then checked against both engines. Each was also run against a mutated engine (20-ref cap off by one, Unicode case folding, U+0085 dropped from the trim set, ID cap at 513 bytes) to confirm it catches that mutation. The `bounds_ci_alts` budgets fail when an insensitive comparison lowercases the whole segment per literal.

The `github.repo` starter changed `starters`: the `eval.github.repo_*` cases and `draft.github_inside_repo_overlaps` were added, and five GitHub URLs the PR and issue rules miss (`repo_root`, `pulls_list`, `issue_new`, `pr_bad_number`, `pr_leading_zero`) now fall through to the repository. Their expectations, the `within` on every PR and issue match, and the `overlap_report.starters` and `overlap_report.container_*` results were derived by hand, then checked against both engines. `draft.github_scope_copied` and `draft.github_first_capture` moved to `github_without_repo`, because the catch-all overlaps every GitHub draft path with two or more segments, `/orgs/{owner}/projects/{number}` included (overlap ignores scope).

`npm run test:adapter-diff` (`scripts/adapter-templates-diff.mjs`) complements these vectors. It generates seeded random documents, URLs, and titles, including near misses, and runs them through `brauser adapters eval` and the TypeScript engine. It fails on any difference. Reproduce a failure with `--seed S --only N`.

Edit cases by hand. Keep ids stable, keep one defect per validate case, and cite the spec section in `why`.
