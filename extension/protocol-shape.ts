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

function isAgent(value: unknown): boolean {
  if (value === null) return true;
  if (typeof value !== "object") return false;
  const agent = value as Record<string, unknown>;
  return typeof agent.harness_id === "string" &&
    (agent.adapter === "claude_code" || agent.adapter === "codex") &&
    typeof agent.binary === "string" &&
    isStringArray(agent.args) && isStringArray(agent.env_allow) &&
    typeof agent.timeout_secs === "number" && Number.isInteger(agent.timeout_secs);
}

function isEnvName(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const env = value as Record<string, unknown>;
  return typeof env.name === "string" && typeof env.present === "boolean";
}

function isHarnessOffer(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const offer = value as Record<string, unknown>;
  return (offer.adapter === "claude_code" || offer.adapter === "codex") &&
    typeof offer.harness_id === "string" && typeof offer.binary === "string" &&
    stringOrNull(offer.real_path) && stringOrNull(offer.version) &&
    isStringArray(offer.args) && isStringArray(offer.env_required) &&
    Array.isArray(offer.env_optional) && offer.env_optional.every(isEnvName) &&
    stringOrNull(offer.refusal);
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
  "summary_path", "stream_bytes", "can_retry_save",
];

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
    typeof run.can_retry_save === "boolean";
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

function isArtifactListItem(value: unknown): boolean {
  if (typeof value !== "object" || value === null) return false;
  const item = value as Record<string, unknown>;
  return ["local_date", "artifact_id", "artifact_type", "title", "url"]
    .every((key) => typeof item[key] === "string") &&
    ["focused_seconds", "background_seconds", "opens"]
      .every((key) => isUnsignedInteger(item[key])) && stringOrNull(item.why);
}

/** Exactly the v6 `ConfigSnapshot` keys: a retired key such as
 * `agent_denylist` or `sites` means the host speaks another shape. */
const CONFIG_KEYS: readonly string[] = [
  "storage", "capture_enabled", "strip_params", "agent", "adapters",
];

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
    isAgent(config.agent) && isAdapterDocument(config.adapters);
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
        value.adapter_warnings.length <= 256 && value.adapter_warnings.every(isAdapterWarning);
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
        (value.artifact === null || isArtifact(value.artifact));
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
    case "agent_checked":
      return typeof value.harness_id === "string" && typeof value.harness_version === "string";
    case "harnesses_discovered":
      return Array.isArray(value.offers) && value.offers.every(isHarnessOffer);
    case "run_started":
      return isRunView(value.run) && typeof value.existing === "boolean";
    case "run_fetched":
      return isRunView(value.run) && typeof value.stream_text === "string" &&
        isUnsignedInteger(value.next_offset) && typeof value.more === "boolean";
    case "runs_listed":
      return Array.isArray(value.runs) && value.runs.length <= 20 && value.runs.every(isRunView);
    case "run_updated":
      return isRunView(value.run);
    default:
      return false;
  }
}
