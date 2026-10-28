/** Runtime shape checks for host responses. The generated types describe the
 * protocol, but a native host is a separate process and its JSON is checked
 * before the extension trusts it. */

const ERROR_CODES: ReadonlySet<string> = new Set([
  "invalid_request", "unsupported_protocol_version", "invalid_config", "not_configured",
  "unauthorized", "conflict", "message_too_large", "internal", "cancelled",
  "service_unavailable",
]);

const AGENT_STATES: ReadonlySet<string> = new Set([
  "not_set_up", "harness_problem", "ready",
]);

function stringOrNull(value: unknown): boolean {
  return value === null || typeof value === "string";
}

function isStringArray(value: unknown): boolean {
  return Array.isArray(value) && value.every((item: unknown) => typeof item === "string");
}

function isHarnessId(value: unknown): boolean {
  return value === "codex" || value === "claude";
}

function hasExactKeys(value: Record<string, unknown>, keys: readonly string[]): boolean {
  const own = Object.keys(value);
  return own.length === keys.length && keys.every((key) => Object.hasOwn(value, key));
}

const AGENT_KEYS = ["harness", "binary", "version", "timeout_secs", "probed_at"] as const;

function isAgent(value: unknown): boolean {
  if (value === null) return true;
  if (typeof value !== "object" || Array.isArray(value)) return false;
  const agent = value as Record<string, unknown>;
  return hasExactKeys(agent, AGENT_KEYS) && isHarnessId(agent.harness) &&
    typeof agent.binary === "string" && stringOrNull(agent.version) &&
    typeof agent.timeout_secs === "number" && Number.isInteger(agent.timeout_secs) &&
    typeof agent.probed_at === "string";
}

const HARNESS_FOUND_KEYS = ["harness", "binary", "version", "problem"] as const;

function isHarnessFound(value: unknown): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const found = value as Record<string, unknown>;
  return hasExactKeys(found, HARNESS_FOUND_KEYS) && isHarnessId(found.harness) &&
    stringOrNull(found.binary) && stringOrNull(found.version) && stringOrNull(found.problem);
}

function isAgentStatus(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const status = value as Record<string, unknown>;
  return typeof status.state === "string" && AGENT_STATES.has(status.state) &&
    stringOrNull(status.harness_version) && stringOrNull(status.message);
}

function isAdapterDocument(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const document = value as Record<string, unknown>;
  return typeof document.format === "number" && Number.isInteger(document.format) &&
    document.format >= 1 && Array.isArray(document.artifacts) && document.artifacts.length <= 64 &&
    document.artifacts.every((artifact: unknown) =>
      typeof artifact === "object" && artifact !== null && !Array.isArray(artifact));
}

function isAdapterWarning(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const warning = value as Record<string, unknown>;
  return ["first_adapter", "first_rule", "second_adapter", "second_rule"]
    .every((key) => typeof warning[key] === "number" && Number.isInteger(warning[key]) &&
      (warning[key] as number) >= 0);
}

function isAdoptedState(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const state = value as Record<string, unknown>;
  return Object.keys(state).length === 4 && stringOrNull(state.issue) && typeof state.full === "boolean" &&
    typeof state.revision === "string" && state.revision.length > 0 &&
    isStringArray(state.warnings) && (state.warnings as unknown[]).length <= 8;
}

function isRecordKey(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const key = value as Record<string, unknown>;
  return (key.kind === "url" || key.kind === "artifact") && typeof key.value === "string";
}

function isOlderNote(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const note = value as Record<string, unknown>;
  return isRecordKey(note.key) && typeof note.canonical_url === "string" &&
    typeof note.title === "string" && typeof note.revision === "string";
}

function isUnsignedInteger(value: unknown): boolean {
  return typeof value === "number" && Number.isInteger(value) && value >= 0;
}

const RUN_STATES: ReadonlySet<string> = new Set([
  "queued", "running", "succeeded", "failed", "canceled",
]);

const RUN_ERROR_KINDS: ReadonlySet<string> = new Set([
  "harness_failed", "timed_out", "canceled", "service_restarted", "invalid_output", "save_failed",
]);

const HARNESS_IDS: ReadonlySet<string> = new Set(["codex", "claude"]);

/** Exactly the v6 `RunView` keys. */
const RUN_VIEW_KEYS: readonly string[] = [
  "run_id", "state", "harness", "record_key", "written_record_key", "url", "title", "cwd",
  "created_at", "started_at", "finished_at", "error", "error_kind", "thread_id", "session_id",
  "summary_path", "stream_bytes", "can_retry_save", "answer",
];

const IMPORTANCE_LEVELS: ReadonlySet<string> = new Set(["high", "medium", "low"]);

/** Exactly the v6 `RunAnswer` keys; the host bounds the strings. */
function isRunAnswer(value: unknown): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const answer = value as Record<string, unknown>;
  return Object.keys(answer).length === 4 &&
    typeof answer.document_type === "string" && typeof answer.category === "string" &&
    typeof answer.importance === "string" && IMPORTANCE_LEVELS.has(answer.importance) &&
    typeof answer.importance_reason === "string";
}

export function isRunView(value: unknown): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const run = value as Record<string, unknown>;
  const keys = Object.keys(run);
  if (keys.length !== RUN_VIEW_KEYS.length || !RUN_VIEW_KEYS.every((key) => keys.includes(key))) return false;
  return typeof run.run_id === "string" && run.run_id.length === 36 &&
    typeof run.state === "string" && RUN_STATES.has(run.state) &&
    typeof run.harness === "string" && HARNESS_IDS.has(run.harness) &&
    isRecordKey(run.record_key) &&
    (run.written_record_key === null || isRecordKey(run.written_record_key)) &&
    typeof run.url === "string" && typeof run.title === "string" && stringOrNull(run.cwd) &&
    typeof run.created_at === "string" && stringOrNull(run.started_at) &&
    stringOrNull(run.finished_at) && stringOrNull(run.error) &&
    (run.error_kind === null ||
      (typeof run.error_kind === "string" && RUN_ERROR_KINDS.has(run.error_kind))) &&
    stringOrNull(run.thread_id) && stringOrNull(run.session_id) &&
    stringOrNull(run.summary_path) && isUnsignedInteger(run.stream_bytes) &&
    typeof run.can_retry_save === "boolean" &&
    (run.answer === null || isRunAnswer(run.answer));
}

const ARTIFACT_REF_KEYS = ["artifact_id", "artifact_type", "title", "url"];

function isArtifactRef(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const ref = value as Record<string, unknown>;
  return ARTIFACT_REF_KEYS.every((key) => typeof ref[key] === "string");
}

function isContainedArtifact(value: unknown): boolean {
  return isArtifactRef(value) &&
    isUnsignedInteger((value as Record<string, unknown>).focused_seconds);
}

function isArtifact(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const artifact = value as Record<string, unknown>;
  return ["artifact_id", "artifact_type", "title", "url", "first_seen", "last_seen"]
    .every((key) => typeof artifact[key] === "string") &&
    ["focused_seconds", "background_seconds", "opens", "days_opened", "linked_count"]
      .every((key) => isUnsignedInteger(artifact[key])) &&
    (artifact.last_mode === null || ["view", "edit", "review"].includes(String(artifact.last_mode))) &&
    stringOrNull(artifact.why) &&
    (artifact.within === null || isArtifactRef(artifact.within)) &&
    Array.isArray(artifact.contained) && artifact.contained.length <= 20 &&
    artifact.contained.every(isContainedArtifact);
}

/** Exactly the v6 `SummaryView` keys; the host bounds the strings. */
const SUMMARY_VIEW_KEYS: readonly string[] = [
  "summarized_at", "document_type", "category", "importance", "summary_path", "harness",
];

export function isSummaryView(value: unknown): boolean {
  if (typeof value !== "object" || value === null || Array.isArray(value)) return false;
  const summary = value as Record<string, unknown>;
  return hasExactKeys(summary, SUMMARY_VIEW_KEYS) &&
    typeof summary.summarized_at === "string" && typeof summary.document_type === "string" &&
    typeof summary.category === "string" &&
    typeof summary.importance === "string" && IMPORTANCE_LEVELS.has(summary.importance) &&
    typeof summary.summary_path === "string" && isHarnessId(summary.harness);
}

/** `summary` is required and may be null. */
function hasSummary(value: Record<string, unknown>): boolean {
  return Object.hasOwn(value, "summary") && (value.summary === null || isSummaryView(value.summary));
}

function isArtifactListItem(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const item = value as Record<string, unknown>;
  return ["local_date", "artifact_id", "artifact_type", "title", "url"]
    .every((key) => typeof item[key] === "string") &&
    ["focused_seconds", "background_seconds", "opens"]
      .every((key) => isUnsignedInteger(item[key])) && stringOrNull(item.why) && hasSummary(item);
}

/** Exactly the v6 `ConfigSnapshot` keys: a retired key such as
 * `agent_denylist` or `sites` means the host speaks another shape. */
const CONFIG_KEYS: readonly string[] = [
  "storage", "capture_enabled", "strip_params", "agent", "summary_vocabulary", "adapters",
];

/** The host validates entry lengths and duplicates; this checks the shape. */
function isSummaryVocabulary(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const vocabulary = value as Record<string, unknown>;
  return Object.keys(vocabulary).length === 2 &&
    isStringArray(vocabulary.document_types) && isStringArray(vocabulary.categories);
}

export function isConfig(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const config = value as Record<string, unknown>;
  const keys = Object.keys(config);
  if (keys.length !== CONFIG_KEYS.length || !CONFIG_KEYS.every((key) => keys.includes(key))) return false;
  const storage = config.storage;
  if (storage !== null) {
    if (typeof storage !== "object" || storage === null) return false;
    const record = storage as Record<string, unknown>;
    if (!["root", "profile", "log_dir", "pages_dir", "later_dir", "artifacts_dir"]
      .every((key) => typeof record[key] === "string")) return false;
    if (!stringOrNull(record.summaries_dir) || !stringOrNull(record.knowledge_base_dir)) return false;
  }
  return typeof config.capture_enabled === "boolean" &&
    isStringArray(config.strip_params) &&
    isAgent(config.agent) && isSummaryVocabulary(config.summary_vocabulary) &&
    isAdapterDocument(config.adapters);
}

// --- Capture Site drafting (additive v6) ---------------------------------
// Every object has exactly its schema keys and every list its schema bound;
// the panel renders this text, so a host speaking another shape is refused.

const MESSAGE_KEYS = ["type", "protocol_version", "request_id"] as const;
const DRAFT_SCOPE_STATES: ReadonlySet<string> = new Set([
  "not_shared", "copied", "pending", "unscoped_only",
]);
const RESOLVED_SCOPES: ReadonlySet<string> = new Set(["not_shared", "scoped", "unscoped"]);
const UNCHECKED_REASONS: ReadonlySet<string> = new Set(["link_only", "token_only"]);
const PREVIEW_SOURCES: ReadonlySet<string> = new Set(["visited", "link", "probe"]);
const RULE_VERDICTS: ReadonlySet<string> = new Set(["kept", "dropped", "warned"]);
/** The structural `DropReason` values, then the `DraftErrorCode` values. */
const DRAFT_DROP_REASONS: ReadonlySet<string> = new Set([
  "root", "probe", "search_or_paging", "secret_query", "blocked_route", "param_name", "already_recognized",
  "no_route_word", "no_identifier", "limit",
  "draft_forbidden_key", "draft_no_scope_source", "draft_scope_conflict",
  "draft_no_leading_literal", "draft_rest_second", "draft_id_needs_typed_capture",
  "draft_search_capture", "draft_matches_probe", "draft_overlap", "draft_blocked_route",
  "draft_paging_capture", "draft_secret_capture", "draft_invalid_rule", "draft_scope_pending", "draft_scope_choice",
]);

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isBoundedList(value: unknown, max: number, item: (entry: unknown) => boolean): boolean {
  return Array.isArray(value) && value.length <= max && value.every(item);
}

function isBoundedString(value: unknown, max: number, min = 0): boolean {
  return typeof value === "string" && value.length >= min && value.length <= max;
}

function isOneOf(value: unknown, values: ReadonlySet<string>): boolean {
  return typeof value === "string" && values.has(value);
}

function isIndexOrNull(value: unknown): boolean {
  return value === null || isUnsignedInteger(value);
}

function isReasonOrNull(value: unknown): boolean {
  return value === null || isBoundedString(value, 64);
}

function isMessage(value: Record<string, unknown>, keys: readonly string[]): boolean {
  return hasExactKeys(value, [...MESSAGE_KEYS, ...keys]) && isBoundedString(value.revision, 128, 1);
}

function isDraftTenant(value: unknown): boolean {
  return value === null || (isPlainObject(value) && hasExactKeys(value, ["segment", "capture"]) &&
    isUnsignedInteger(value.segment) && isBoundedString(value.capture, 64, 1));
}

function isDraftedRule(value: unknown): boolean {
  return isPlainObject(value) &&
    hasExactKeys(value, ["rule", "checked", "unchecked_reason", "visited", "links", "example"]) &&
    isUnsignedInteger(value.rule) && typeof value.checked === "boolean" &&
    (value.unchecked_reason === null || isOneOf(value.unchecked_reason, UNCHECKED_REASONS)) &&
    isUnsignedInteger(value.visited) && isUnsignedInteger(value.links) &&
    isBoundedString(value.example, 8192);
}

function isDraftedAdapter(value: unknown): boolean {
  return isPlainObject(value) && hasExactKeys(value, ["adapter", "scope", "tenant", "rules"]) &&
    isPlainObject(value.adapter) && isOneOf(value.scope, DRAFT_SCOPE_STATES) &&
    isDraftTenant(value.tenant) && isBoundedList(value.rules, 16, isDraftedRule);
}

function isDropped(value: unknown): boolean {
  return isPlainObject(value) && hasExactKeys(value, ["shape", "adapter", "rule", "reason"]) &&
    (value.shape === null || isBoundedString(value.shape, 2048)) &&
    isIndexOrNull(value.adapter) && isIndexOrNull(value.rule) &&
    isOneOf(value.reason, DRAFT_DROP_REASONS);
}

function isPreviewRow(value: unknown): boolean {
  return isPlainObject(value) &&
    hasExactKeys(value, ["source", "shape", "url", "adapter", "rule", "artifact_id"]) &&
    isOneOf(value.source, PREVIEW_SOURCES) && isBoundedString(value.shape, 2048) &&
    isBoundedString(value.url, 8192) && isIndexOrNull(value.adapter) &&
    isIndexOrNull(value.rule) &&
    (value.artifact_id === null || isBoundedString(value.artifact_id, 2048));
}

function isRuleVerdict(value: unknown): boolean {
  return isPlainObject(value) && hasExactKeys(value, ["verdict", "reason"]) &&
    isOneOf(value.verdict, RULE_VERDICTS) && isReasonOrNull(value.reason);
}

function isScopeVerdict(value: unknown): boolean {
  return isPlainObject(value) && hasExactKeys(value, ["state", "resolved", "reason"]) &&
    isOneOf(value.state, DRAFT_SCOPE_STATES) &&
    (value.resolved === null || isOneOf(value.resolved, RESOLVED_SCOPES)) &&
    isReasonOrNull(value.reason);
}

function isSiteDrafted(value: Record<string, unknown>): boolean {
  return isMessage(value, [
    "revision", "drafter", "adapters", "dropped", "preview", "taken_types", "remapped",
  ]) && value.drafter === "structural" && isBoundedList(value.adapters, 8, isDraftedAdapter) &&
    isBoundedList(value.dropped, 128, isDropped) && isBoundedList(value.preview, 128, isPreviewRow) &&
    isBoundedList(value.taken_types, 160, (type) => isBoundedString(type, 64, 1)) &&
    isUnsignedInteger(value.remapped);
}

function isSiteDraftChecked(value: Record<string, unknown>): boolean {
  return isMessage(value, ["revision", "rules", "scope", "preview"]) &&
    isBoundedList(value.rules, 8, (rules) => isBoundedList(rules, 16, isRuleVerdict)) &&
    isBoundedList(value.scope, 8, isScopeVerdict) && isBoundedList(value.preview, 128, isPreviewRow);
}

export function isResponseShape(value: Record<string, unknown>): boolean {
  switch (value.type) {
    case "error":
      return typeof value.code === "string" && ERROR_CODES.has(value.code) &&
        typeof value.message === "string";
    case "hello_result":
      return typeof value.host_version === "string" &&
        typeof value.configured === "boolean" && stringOrNull(value.config_issue);
    case "config_result":
      return typeof value.revision === "string" && isConfig(value.config) &&
        stringOrNull(value.config_issue) && isAgentStatus(value.agent_status) &&
        stringOrNull(value.adapter_issue) && Array.isArray(value.adapter_warnings) &&
        value.adapter_warnings.length <= 256 && value.adapter_warnings.every(isAdapterWarning) &&
        isAdoptedState(value.adopted_state);
    case "config_updated":
      return typeof value.revision === "string" && isConfig(value.config);
    case "folder_chosen":
      return typeof value.path === "string" && typeof value.picker_token === "string";
    case "visit_recorded":
    case "activity_recorded":
    case "why_recorded":
      return typeof value.event_id === "string" &&
        ["persisted", "suppressed", "rejected", "retryable"].includes(String(value.outcome)) &&
        stringOrNull(value.reason) && stringOrNull(value.relative_path);
    case "batch_recorded":
      return typeof value.batch_id === "string" && typeof value.overflow_recorded === "boolean" &&
        Array.isArray(value.results) && value.results.length <= 64 &&
        value.results.every((result) => typeof result === "object" && result !== null &&
          typeof (result as Record<string, unknown>).event_id === "string" &&
          ["persisted", "suppressed", "rejected", "retryable"]
            .includes(String((result as Record<string, unknown>).outcome)) &&
          stringOrNull((result as Record<string, unknown>).reason) &&
          stringOrNull((result as Record<string, unknown>).relative_path));
    case "note_loaded":
      return typeof value.exists === "boolean" && typeof value.revision === "string" &&
        typeof value.title === "string" && typeof value.body === "string" &&
        isRecordKey(value.key) && Array.isArray(value.older_notes) &&
        value.older_notes.length <= 24 && value.older_notes.every(isOlderNote);
    case "note_saved":
      return ["created", "replaced", "created_with_warning", "replaced_with_warning"]
        .includes(String(value.outcome)) &&
        typeof value.revision === "string" && typeof value.relative_path === "string" &&
        isRecordKey(value.key);
    case "note_conflict":
      return typeof value.exists === "boolean" && typeof value.revision === "string" &&
        typeof value.title === "string" && typeof value.body === "string" &&
        isRecordKey(value.key) && ["stale", "key_changed"].includes(String(value.reason));
    case "artifact_result":
      return typeof value.recognized === "boolean" &&
        (value.artifact === null || isArtifact(value.artifact)) && hasSummary(value);
    case "artifact_list_result":
      return Array.isArray(value.items) && value.items.length <= 32 &&
        value.items.every(isArtifactListItem) && Array.isArray(value.artifact_types) &&
        value.artifact_types.length <= 64 && isStringArray(value.artifact_types) &&
        typeof value.truncated === "boolean";
    case "worklog_result":
      return typeof value.start_date === "string" && typeof value.end_date === "string" &&
        typeof value.text === "string" && isUnsignedInteger(value.artifact_count);
    case "artifacts_refreshed":
      return isUnsignedInteger(value.written) && isUnsignedInteger(value.unchanged) &&
        isUnsignedInteger(value.conflicts) && Array.isArray(value.conflict_paths) &&
        value.conflict_paths.length <= 64 && isStringArray(value.conflict_paths);
    case "harnesses_discovered":
      return Array.isArray(value.harnesses) && value.harnesses.length <= 2 &&
        value.harnesses.every(isHarnessFound);
    case "run_started":
      return isRunView(value.run) && typeof value.existing === "boolean";
    case "run_fetched":
      return isRunView(value.run) && typeof value.stream_text === "string" &&
        isUnsignedInteger(value.next_offset) && typeof value.more === "boolean";
    case "runs_listed":
      return Array.isArray(value.runs) && value.runs.length <= 20 && value.runs.every(isRunView);
    case "run_updated":
      return isRunView(value.run);
    case "path_opened":
      return true;
    case "summary_opened":
      return typeof value.relative_path === "string";
    case "agent_env_listed":
      return Array.isArray(value.names) && value.names.length <= 64 &&
        isStringArray(value.names) && stringOrNull(value.problem);
    case "site_drafted":
      return isSiteDrafted(value);
    case "site_draft_checked":
      return isSiteDraftChecked(value);
    default:
      return false;
  }
}
