import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension. agent.js is pure: no chrome or DOM access.
const agent = await import("../dist/agent.js");
const { SETUP_WARNING, agentStateText, harnessFoundText, harnessOfferNote } = agent;
const { isConfig, isResponseShape } = await import("../dist/protocol-shape.js");

const config = {
  storage: {
    root: "/notes", profile: "neutral", log_dir: "logs", pages_dir: "notes", later_dir: "later", artifacts_dir: "artifacts",
    summaries_dir: "summaries", knowledge_base_dir: null,
  },
  capture_enabled: false, strip_params: [],
  agent: null,
  adapters: { format: 1, artifacts: [] },
};

test("harness_found_text_asks_for_a_notes_folder_before_setup", () => {
  const withFolder = harnessFoundText("Claude Code", "2.1.284", true);
  assert.equal(withFolder, "Found Claude Code 2.1.284. Choose Set up harness to run a test prompt.");
  assert.doesNotMatch(withFolder, /Review it below/);
  const withoutFolder = harnessFoundText("Claude Code", "2.1.284", false);
  assert.match(withoutFolder, /^Found Claude Code 2\.1\.284\./);
  assert.match(withoutFolder, /Choose a home folder first/);
  assert.doesNotMatch(withoutFolder, /then choose Set up harness\.$/);
  assert.match(harnessFoundText("Codex", null, true), /^Found Codex\. /);
});

test("harness offer notes carry no offer id and no Codex disclosure", () => {
  const offer = {
    adapter: "codex", harness_id: "codex", binary: "/bin/codex",
    real_path: "/bin/codex", version: "0.144.4", args: [], env_required: ["HOME", "PATH"],
    env_optional: [], refusal: null,
  };
  assert.equal("CODEX_DISCLOSURE" in agent, false);
  assert.equal(harnessOfferNote(offer), "Ready to set up.");
  assert.equal(harnessOfferNote({ ...offer, refusal: "Codex 0.1.0 has not been reviewed" }),
    "Cannot be set up: Codex 0.1.0 has not been reviewed.");
  const claude = { ...offer, adapter: "claude_code", harness_id: "claude-code" };
  assert.equal(harnessOfferNote(claude), "Ready to set up.");
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
  for (const key of ["storage", "capture_enabled", "strip_params", "agent", "adapters"]) {
    const without = { ...config };
    delete without[key];
    assert.equal(isConfig(without), false, key);
  }
  const { summaries_dir: _omitted, ...m1Storage } = config.storage;
  assert.equal(isConfig({ ...config, storage: m1Storage }), false);
});

test("isResponseShape_accepts_new_responses_and_rejects_unknown", () => {
  const offer = {
    adapter: "claude_code", harness_id: "claude-code",
    binary: "/bin/claude", real_path: "/opt/claude", version: "2.1.284", args: ["-p", "{prompt}"],
    env_required: ["HOME", "PATH"], env_optional: [{ name: "ANTHROPIC_API_KEY", present: false }],
    refusal: null,
  };
  assert.equal(isResponseShape({ type: "harnesses_discovered", offers: [offer] }), true);
  assert.equal(isResponseShape({
    type: "harness_setup_confirmed", harness_token: "t", config, summary: "s",
  }), false);
  assert.equal(isResponseShape({ type: "config_updated", config, revision: "r" }), true);
  assert.equal(isResponseShape({
    type: "agent_checked", harness_id: "claude-code", harness_version: "2.1.284",
  }), true);
  assert.equal(isResponseShape({ type: "harness_ready", offers: [offer] }), false);
  assert.equal(isResponseShape({ type: "agent_checked", harness_id: "claude-code" }), false);
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
    stream_bytes: 0, can_retry_save: false,
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
