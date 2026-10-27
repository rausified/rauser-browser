import assert from "node:assert/strict";
import test from "node:test";

// Run after build:extension. agent.js is pure: no chrome or DOM access.
const {
  CODEX_DISCLOSURE, agentStateText, harnessFoundText, harnessOfferNote,
} = await import("../dist/agent.js");
const { isConfig, isResponseShape } = await import("../dist/protocol-shape.js");

const config = {
  storage: {
    root: "/notes", profile: "neutral", log_dir: "log", pages_dir: "pages", later_dir: "later", artifacts_dir: "artifacts",
    summaries_dir: "summaries",
  },
  capture_enabled: false, sites: [], strip_params: [],
  agent: null,
  adapters: { format: 1, artifacts: [] },
};

test("harness_found_text_asks_for_a_notes_folder_before_setup", () => {
  const withFolder = harnessFoundText("Claude Code", "2.1.284", true);
  assert.match(withFolder, /^Found Claude Code 2\.1\.284\./);
  assert.match(withFolder, /choose Set up harness/);
  const withoutFolder = harnessFoundText("Claude Code", "2.1.284", false);
  assert.match(withoutFolder, /^Found Claude Code 2\.1\.284\./);
  assert.match(withoutFolder, /Choose a notes folder first/);
  assert.doesNotMatch(withoutFolder, /then choose Set up harness\.$/);
  assert.match(harnessFoundText("Codex", null, true), /^Found Codex\. /);
});

test("codex_offer_note_discloses_shared_codex_home", () => {
  const offer = {
    offer_id: "offer-1", adapter: "codex", harness_id: "codex", binary: "/bin/codex",
    real_path: "/bin/codex", version: "0.144.4", args: [], env_required: ["HOME", "PATH"],
    env_optional: [], refusal: null,
  };
  assert.match(CODEX_DISCLOSURE, /instructions \(AGENTS\.md\) and skills can shape/);
  assert.equal(harnessOfferNote(offer), `Ready to set up. ${CODEX_DISCLOSURE}`);
  assert.equal(harnessOfferNote({ ...offer, offer_id: null, refusal: "Codex 0.1.0 has not been reviewed" }),
    `Cannot be set up: Codex 0.1.0 has not been reviewed. ${CODEX_DISCLOSURE}`);
  const claude = { ...offer, adapter: "claude_code", harness_id: "claude-code" };
  assert.equal(harnessOfferNote(claude), "Ready to set up.");
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
  for (const key of ["storage", "capture_enabled", "sites", "strip_params", "agent", "adapters"]) {
    const without = { ...config };
    delete without[key];
    assert.equal(isConfig(without), false, key);
  }
  const { summaries_dir: _omitted, ...m1Storage } = config.storage;
  assert.equal(isConfig({ ...config, storage: m1Storage }), false);
});

test("isResponseShape_accepts_new_responses_and_rejects_unknown", () => {
  const offer = {
    offer_id: "offer-1", adapter: "claude_code", harness_id: "claude-code",
    binary: "/bin/claude", real_path: "/opt/claude", version: "2.1.284", args: ["-p", "{prompt}"],
    env_required: ["HOME", "PATH"], env_optional: [{ name: "ANTHROPIC_API_KEY", present: false }],
    refusal: null,
  };
  assert.equal(isResponseShape({ type: "harnesses_discovered", offers: [offer] }), true);
  assert.equal(isResponseShape({
    type: "harness_setup_confirmed", harness_token: "t", config, summary: "s",
  }), true);
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
