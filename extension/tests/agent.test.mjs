import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension. agent.js is pure: no chrome or DOM access.
const agent = await import("../dist/agent.js");
const {
  SERVICE_SETUP_INSTRUCTIONS, SETUP_WARNING, agentEnvText, agentStateText, harnessFoundText,
  serviceSetupText, vocabularyFromText,
} = agent;
const { isConfig, isResponseShape } = await import("../dist/protocol-shape.js");

const config = {
  storage: {
    root: "/notes", profile: "neutral", log_dir: "log", pages_dir: "pages", later_dir: "later", artifacts_dir: "artifacts",
    summaries_dir: "summaries", knowledge_base_dir: null,
  },
  capture_enabled: false, strip_params: [],
  agent: null,
  summary_vocabulary: { document_types: ["report"], categories: ["news"] },
  adapters: { format: 1, artifacts: [] },
};

test("harnessFoundText names each harness with its version, or why it cannot be used", () => {
  const codex = { harness: "codex", binary: "/bin/codex", version: "0.144.4", problem: null };
  assert.equal(harnessFoundText(codex), "Codex 0.144.4");
  assert.equal(harnessFoundText({ ...codex, version: null }), "Codex");
  assert.equal(harnessFoundText({ harness: "claude", binary: "/bin/claude", version: "2.1.310", problem: null }),
    "Claude Code 2.1.310");
  assert.equal(harnessFoundText({ harness: "claude", binary: null, version: null, problem: null }),
    "Claude Code not found on the service PATH");
  assert.equal(harnessFoundText({ ...codex, version: null, problem: "codex --version failed (exit 1)" }),
    "Codex: codex --version failed (exit 1)");
  assert.equal(harnessFoundText({ harness: "codex", binary: null, version: null, problem: "/bin is world-writable" }),
    "Codex: /bin is world-writable");
  assert.equal("harnessOfferNote" in agent, false);
  assert.equal("CODEX_DISCLOSURE" in agent, false);
});

test("agentEnvText shows names or the problem, never values", () => {
  assert.equal(agentEnvText(["AWS_PROFILE", "AWS_REGION"], null),
    "Runs also get AWS_PROFILE, AWS_REGION from agent.env.");
  assert.equal(agentEnvText([], null),
    "agent.env sets nothing, so runs get only the Brauser service's environment.");
  assert.equal(agentEnvText([], "agent.env must have mode 0600, not 0644"),
    "agent.env cannot be used: agent.env must have mode 0600, not 0644");
});

test("vocabularyFromText trims, drops blanks, and dedupes case-insensitively", () => {
  assert.deepEqual(vocabularyFromText(" report \r\n\n\tmemo\nREPORT\nMemo \n  "), ["report", "memo"]);
  assert.deepEqual(vocabularyFromText(""), []);
  assert.deepEqual(vocabularyFromText("research paper\nnews"), ["research paper", "news"]);
});

test("serviceSetupText puts the setup instructions before the host's message", () => {
  assert.match(SERVICE_SETUP_INSTRUCTIONS, /register-dev-host\.mjs/);
  assert.equal(serviceSetupText("Not installed."), `${SERVICE_SETUP_INSTRUCTIONS} Not installed.`);
});

test("the setup warning is the exact full-access text", () => {
  assert.equal(SETUP_WARNING,
    "Summaries run an AI agent with full access to your files, on your normal login. A page could contain instructions aimed at the agent. Summarize pages you trust.");
});

test("agentStateText_describes_each_state", () => {
  assert.match(agentStateText(null), /unavailable/);
  assert.match(agentStateText({ state: "not_set_up", harness_version: null, message: "x" }), /off/);
  assert.equal(agentStateText({ state: "ready", harness_version: "2.1.284", message: null }),
    "AI harness ready (version 2.1.284).");
  assert.equal(
    agentStateText({ state: "harness_problem", harness_version: "2.1.284", message: "run setup again" }),
    "AI commands are off (version 2.1.284): run setup again");
});

test("isConfig requires exactly the v6 keys", () => {
  assert.equal(isConfig(config), true);
  assert.equal(isConfig({ ...config, agent_denylist: [] }), false);
  for (const key of ["agent_denylist_confirmed", "log_incognito"]) {
    assert.equal(isConfig({ ...config, [key]: false }), false, key);
  }
  assert.equal(isConfig({ ...config, sites: [] }), false, "sites");
  for (const key of ["storage", "capture_enabled", "strip_params", "agent", "summary_vocabulary", "adapters"]) {
    const without = { ...config };
    delete without[key];
    assert.equal(isConfig(without), false, key);
  }
  for (const summary_vocabulary of [
    null, { document_types: ["report"] }, { document_types: ["report"], categories: [1] },
    { document_types: "report", categories: ["news"] },
    { document_types: ["report"], categories: ["news"], tags: [] },
  ]) {
    assert.equal(isConfig({ ...config, summary_vocabulary }), false, JSON.stringify(summary_vocabulary));
  }
  const { summaries_dir: _omitted, ...m1Storage } = config.storage;
  assert.equal(isConfig({ ...config, storage: m1Storage }), false);
});

test("isResponseShape_accepts_new_responses_and_rejects_unknown", () => {
  const found = { harness: "claude", binary: "/bin/claude", version: "2.1.284", problem: null };
  const missing = { harness: "codex", binary: null, version: null, problem: null };
  assert.equal(isResponseShape({ type: "harnesses_discovered", harnesses: [missing, found] }), true);
  assert.equal(isResponseShape({ type: "harnesses_discovered", offers: [found] }), false);
  assert.equal(isResponseShape({ type: "harnesses_discovered", harnesses: [found, found, found] }), false);
  for (const key of Object.keys(found)) {
    const without = { ...found };
    delete without[key];
    assert.equal(isResponseShape({ type: "harnesses_discovered", harnesses: [without] }), false, key);
  }
  for (const bad of [{ ...found, harness: "claude_code" }, { ...found, refusal: null }, { ...found, version: 2 }]) {
    assert.equal(isResponseShape({ type: "harnesses_discovered", harnesses: [bad] }), false, JSON.stringify(bad));
  }
  assert.equal(isResponseShape({
    type: "harness_setup_confirmed", harness_token: "t", config, summary: "s",
  }), false);
  assert.equal(isResponseShape({ type: "config_updated", config, revision: "r" }), true);
  // check_agent and its answer are gone in v6.
  assert.equal(isResponseShape({
    type: "agent_checked", harness_id: "claude-code", harness_version: "2.1.284",
  }), false);
});

test("isConfig accepts the service's summary harness entry only", () => {
  const agent = {
    harness: "codex", binary: "/opt/homebrew/bin/codex", version: "0.144.4",
    timeout_secs: 600, probed_at: "2026-10-27T10:00:00Z",
  };
  assert.equal(isConfig({ ...config, agent }), true);
  assert.equal(isConfig({ ...config, agent: { ...agent, version: null } }), true);
  for (const key of Object.keys(agent)) {
    const without = { ...agent };
    delete without[key];
    assert.equal(isConfig({ ...config, agent: without }), false, key);
  }
  for (const bad of [
    { ...agent, harness: "claude-code" }, { ...agent, timeout_secs: "600" },
    { ...agent, env_allow: ["HOME"] }, { ...agent, harness_id: "codex" },
  ]) {
    assert.equal(isConfig({ ...config, agent: bad }), false, JSON.stringify(bad));
  }
});

test("artifact list responses enforce the protocol type bound", () => {
  const response = {
    type: "artifact_list_result",
    items: [],
    artifact_types: Array.from({ length: 64 }, (_, index) => `type-${index}`),
    truncated: false,
  };
  assert.equal(isResponseShape(response), true);
  assert.equal(isResponseShape({
    ...response,
    artifact_types: [...response.artifact_types, "type-64"],
  }), false);
});

test("run responses are checked against the RunView shape", () => {
  const view = {
    run_id: "00000000-0000-4000-8000-000000000001", state: "running", harness: "codex",
    record_key: { kind: "url", value: "https://a.example/x" }, written_record_key: null,
    url: "https://a.example/x", title: "T", cwd: null,
    created_at: "2026-10-26T14:00:00-07:00", started_at: null, finished_at: null,
    error: null, error_kind: null, thread_id: null, session_id: null, summary_path: null,
    stream_bytes: 0, can_retry_save: false, answer: null,
  };
  const fetched = { type: "run_fetched", run: view, stream_text: "x", next_offset: 1, more: false };
  assert.equal(isResponseShape(fetched), true);
  assert.equal(isResponseShape({ ...fetched, run: { ...view, state: "paused" } }), false);
  assert.equal(isResponseShape({ ...fetched, run: { ...view, harness: "claude_code" } }), false);
  assert.equal(isResponseShape({ ...fetched, run: { ...view, error_kind: "exploded" } }), false);
  assert.equal(isResponseShape({
    ...fetched, run: { ...view, state: "failed", error: "e", error_kind: "save_failed", can_retry_save: true },
  }), true);
  const { summary_path: _omitted, ...missing } = view;
  assert.equal(isResponseShape({ ...fetched, run: missing }), false);
  assert.equal(isResponseShape({ ...fetched, next_offset: -1 }), false);
  assert.equal(isResponseShape({ type: "run_started", run: view, existing: true }), true);
  assert.equal(isResponseShape({ type: "run_started", run: view }), false);
  assert.equal(isResponseShape({ type: "run_updated", run: view }), true);
  assert.equal(isResponseShape({ type: "runs_listed", runs: [view] }), true);
  assert.equal(isResponseShape({ type: "runs_listed", runs: Array.from({ length: 21 }, () => view) }), false);
  assert.equal(isResponseShape({ type: "service_ready", build_id: "b" }), false);
  assert.equal(isResponseShape({ type: "error", code: "service_unavailable", message: "m" }), true);
});

test("isRunView requires answer and rejects an unknown importance", () => {
  const view = {
    run_id: "00000000-0000-4000-8000-000000000001", state: "succeeded", harness: "claude",
    record_key: { kind: "url", value: "https://a.example/x" }, written_record_key: null,
    url: "https://a.example/x", title: "T", cwd: "/kb",
    created_at: "2026-10-26T14:00:00-07:00", started_at: null, finished_at: null,
    error: null, error_kind: null, thread_id: null, session_id: null, summary_path: null,
    stream_bytes: 0, can_retry_save: false,
    answer: { document_type: "report", category: "news", importance: "high", importance_reason: "New." },
  };
  const updated = (run) => isResponseShape({ type: "run_updated", run });
  assert.equal(updated(view), true);
  assert.equal(updated({ ...view, answer: null }), true);
  const { answer: _omitted, ...missing } = view;
  assert.equal(updated(missing), false);
  assert.equal(updated({ ...view, answer: { ...view.answer, importance: "critical" } }), false);
  assert.equal(updated({ ...view, answer: { ...view.answer, importance: "High" } }), false);
  const { importance_reason: _reason, ...partial } = view.answer;
  assert.equal(updated({ ...view, answer: partial }), false);
  assert.equal(updated({ ...view, answer: { ...view.answer, extra: 1 } }), false);
  assert.equal(updated({ ...view, answer: { ...view.answer, document_type: 7 } }), false);
  assert.equal(updated({ ...view, answer: "high" }), false);
});

test("path_opened and agent_env_listed responses are checked", () => {
  assert.equal(isResponseShape({ type: "path_opened" }), true);
  assert.equal(isResponseShape({ type: "agent_env_listed", names: ["FOO"], problem: null }), true);
  assert.equal(isResponseShape({ type: "agent_env_listed", names: [], problem: "agent.env must have mode 0600, not 0644" }), true);
  assert.equal(isResponseShape({ type: "agent_env_listed", names: [7], problem: null }), false);
  assert.equal(isResponseShape({ type: "agent_env_listed", names: ["FOO"] }), false);
  assert.equal(isResponseShape({
    type: "agent_env_listed", names: Array.from({ length: 65 }, (_, n) => `K${n}`), problem: null,
  }), false);
});
