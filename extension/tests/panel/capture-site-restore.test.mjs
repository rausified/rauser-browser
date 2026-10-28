// Built side-panel tests: capture site restore. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  CONFIG, GITHUB_URL, JIRA_URL, captureReview, isResponseShape, saveCapture,
  settle, until, updates,
} from "./harness.mjs";

const JIRA_OTHER_ADAPTER = { type: "work.item", origin: "https://work.example", match: [{ path: "/items/{id:int}" }], id: "work:{host}/{id}" };
const RECORDED_JIRA = {
  format: 1, artifacts: [{
    type: "custom.issue", origin: "https://acme.atlassian.net",
    match: [{ path: "/browse/{key:jira_key}" }, { path: "/jira/{*}", query: { selectedIssue: "{key:jira_key}" } }],
    id: "custom:{host}/issues/{key}",
  }],
};
const restoreReply = (request, adapters = RECORDED_JIRA) => ({
  type: "site_restore_loaded", revision: request.expected_revision, origin: request.origin, adapters,
});

for (const paused of [false, true]) {
  test(`restore review keeps a custom Jira identity and ${paused ? "preserves pause" : "saves explicitly"}`, async () => {
    const key = { kind: "artifact", value: "custom:acme.atlassian.net/issues/PROJ-42" };
    const config = structuredClone(CONFIG);
    config.capture_enabled = !paused;
    // Another configured origin is preserved exactly.
    config.adapters.artifacts = [JIRA_OTHER_ADAPTER];
    const page = await captureReview(JIRA_URL, { config, intercept(request) {
      if (request.type === "get_site_restore") return restoreReply(request);
      if (request.type === "load_note") return {
        type: "note_loaded", exists: true, revision: "same-note", key,
        title: "My note", body: "  Unchanged text\n", older_notes: [],
      };
    } });
    assert.equal(page.el("capture-site-heading").textContent, "Restore capture on acme.atlassian.net");
    assert.match(page.el("capture-site-preview").textContent, /Recorded page kind: custom issue/);
    assert.match(page.el("capture-site-preview").textContent, /This page: will be captured/);
    assert.doesNotMatch(page.el("capture-site-preview").textContent, /custom:\{host\}|"match"/);
    assert.equal(page.el("capture-site-github").hidden, true);
    assert.equal(page.el("capture-site-save").textContent, paused ? "Restore rules · keep capture paused" : "Restore capture rules");
    assert.equal(updates(page).length, 0);
    assert.equal(page.sent("save_note").length, 0);
    assert.equal(page.state.scripts.length, 0);
    assert.equal(page.sent("draft_site").length, 0);
    assert.equal(page.sent("get_site_restore").length, 1);
    assert.deepEqual(Object.keys(page.sent("get_site_restore")[0]).sort(),
      ["expected_revision", "origin", "protocol_version", "request_id", "type"]);
    await saveCapture(page);
    assert.equal(page.sent("get_site_restore").length, 2, "record re-read before save");
    const [update] = updates(page);
    assert.equal(update.expected_revision, "1");
    assert.equal(update.config.capture_enabled, !paused);
    assert.deepEqual(update.config.adapters.artifacts, [JIRA_OTHER_ADAPTER, ...RECORDED_JIRA.artifacts]);
    assert.equal(page.el("note-body").value, "  Unchanged text\n");
    assert.equal(page.sent("save_note").length, 0);
    assert.equal(page.sent("use_artifact_note").length, 0);
    assert.equal(page.sent("forget_adopted_contracts").length, 0);
  });
}

test("restore review shows fixed shared-origin account scope with text-only page kinds", async () => {
  const adapters = { format: 1, artifacts: [{
    type: "github.pr", origin: "https://github.com",
    match: [{ path: "/{owner:name}/{repo:name}/pull/{n:int}/{*}" }],
    id: "github:{host}/{owner}/{repo}/pull/{n}", scope: { owner: ["acme-corp", "partner"] },
  }] };
  const page = await captureReview(GITHUB_URL, { intercept: (request) =>
    request.type === "get_site_restore" ? restoreReply(request, adapters) : undefined });
  assert.match(page.el("capture-site-preview").textContent, /only for owner: acme-corp, partner/);
  assert.equal(page.el("capture-site-github").hidden, true);
  await saveCapture(page);
  assert.deepEqual(updates(page)[0].config.adapters.artifacts, adapters.artifacts);
});

test("restore review explicitly discloses recorded every-account capture", async () => {
  const adapters = { format: 1, artifacts: [{
    type: "github.pr", origin: "https://github.com",
    match: [{ path: "/{owner:name}/{repo:name}/pull/{n:int}/{*}" }],
    id: "github:{host}/{owner}/{repo}/pull/{n}", unscoped: "acknowledged",
  }] };
  const page = await captureReview(GITHUB_URL, { intercept: (request) =>
    request.type === "get_site_restore" ? restoreReply(request, adapters) : undefined });
  assert.match(page.el("capture-site-preview").textContent, /pull requests in every account on this site in this Chrome profile/);
  await saveCapture(page);
  assert.deepEqual(updates(page)[0].config.adapters.artifacts, adapters.artifacts);
});

test("unknown-site restoration never reads page patterns or launches a drafter", async () => {
  const adapters = { format: 1, artifacts: [{
    type: "work.item", origin: "https://tools.example", match: [{ path: "/items/{n:int}" }], id: "work:{host}/{n}",
  }] };
  const page = await captureReview("https://tools.example/items/7", { intercept: (request) =>
    request.type === "get_site_restore" ? restoreReply(request, adapters) : undefined });
  assert.match(page.el("capture-site-heading").textContent, /Restore capture/);
  assert.equal(page.state.scripts.length, 0);
  await saveCapture(page);
  assert.equal(page.sent("draft_site").length, 0);
});

test("late recorded-rule reviews cannot survive cancel, navigation or a newer review", async () => {
  for (const navigation of [false, true]) {
    const pending = [];
    const page = await captureReview(JIRA_URL, { waitForCheck: false, intercept(request) {
      if (request.type === "get_site_restore") return new Promise((resolve) => pending.push({ request, resolve }));
    } });
    await until(() => pending.length === 1, "held restore read");
    assert.equal(page.el("capture-site-save").disabled, true);
    if (navigation) {
      page.navigate("https://tools.example/items/7", 2);
      await until(() => page.el("page-url").textContent.includes("tools.example"), "new page");
    } else page.el("capture-site-cancel").click();
    page.el("capture-site").click();
    await until(() => pending.length === 2, "new restore review");
    pending[0].resolve(restoreReply(pending[0].request));
    await settle();
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.doesNotMatch(page.el("capture-site-heading").textContent, /Restore capture/);
    pending[1].resolve({ type: "error", code: "invalid_config", message: "Recorded rules overlap. Review them in settings." });
    await until(() => /overlap/.test(page.el("capture-site-message").textContent), "current refusal");
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.equal(updates(page).length, 0);
    assert.equal(page.state.scripts.length, 0);
  }
});

for (const problem of ["revision", "record", "pause", "removal"]) {
  test(`restore refuses a save after ${problem} changes`, async () => {
    let reads = 0;
    const page = await captureReview(JIRA_URL, { intercept(request) {
      if (request.type !== "get_site_restore") return undefined;
      reads += 1;
      if (problem === "record" && reads === 2) return restoreReply(request, { format: 1, artifacts: [] });
      return restoreReply(request);
    } });
    if (problem === "revision") page.state.revision = "external";
    if (problem === "pause") page.state.workerStatus.pause_pending = true;
    if (problem === "removal") page.state.workerStatus.pending_removals = ["https://acme.atlassian.net"];
    page.el("capture-site-save").click();
    await until(() => /Could not capture site/.test(page.el("status").textContent), "save refusal");
    assert.equal(updates(page).length, 0);
    assert.equal(page.sent("save_note").length, 0);
  });
}

test("restore response validation and host refusals never fall back to fresh rules", async () => {
  const good = restoreReply({ expected_revision: "1", origin: "https://acme.atlassian.net" });
  assert.equal(isResponseShape({ ...good, request_id: "r", protocol_version: 8 }), true);
  for (const reply of [
    { ...good, adapters: { format: 1, artifacts: Array(65).fill(RECORDED_JIRA.artifacts[0]) } },
    { ...good, origin: 1 }, { ...good, adapters: { format: 0, artifacts: [] } },
  ]) assert.equal(isResponseShape({ ...reply, request_id: "r", protocol_version: 8 }), false);
  for (const reply of [
    { type: "error", code: "invalid_config", message: "Fix adopted-contracts.json or choose Forget recorded adapters in settings." },
    { ...good, revision: "another" },
    { ...good, adapters: { format: 1, artifacts: [{ ...RECORDED_JIRA.artifacts[0], origin: "https://other.example" }] } },
  ]) {
    const page = await captureReview(JIRA_URL, { intercept: (request) => request.type === "get_site_restore" ? reply : undefined });
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.match(page.el("capture-site-message").textContent, /Could not review recorded rules/);
    assert.equal(page.sent("check_capture_site").length, 0);
    assert.equal(page.sent("draft_site").length, 0);
  }
});

test("GitHub starter edits cannot bypass a failed recorded-rule review", async () => {
  let refuse = true;
  const page = await captureReview(GITHUB_URL, { intercept(request) {
    if (request.type === "get_site_restore" && refuse) return {
      type: "error", code: "invalid_config", message: "Recorded rules overlap. Review them in settings.",
    };
  } });
  const refusal = page.el("capture-site-message").textContent;
  assert.match(refusal, /Could not review recorded rules/);
  page.el("capture-site-orgs").value = "other-org";
  page.el("capture-site-orgs").dispatch("input");
  page.el("capture-site-repos").checked = false;
  page.el("capture-site-repos").dispatch("input");
  await settle();
  assert.equal(page.el("capture-site-message").textContent, refusal);
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(page.sent("check_capture_site").length, 0);
  assert.equal(updates(page).length, 0);

  // A new review may offer starters once the host confirms no recorded rules.
  refuse = false;
  page.el("capture-site-cancel").click();
  page.el("capture-site").click();
  await until(() => !page.el("capture-site-save").disabled, "fresh review after repair");
  assert.equal(page.sent("get_site_restore").length, 2);
  assert.equal(page.sent("check_capture_site").length, 1);
});

for (const history of ["retired", "hand-removed"]) {
  test(`Capture Site refuses a starter conflicting with a ${history} contract before save`, async () => {
    const page = await captureReview(JIRA_URL, { intercept(request) {
      if (request.type === "check_capture_site") return {
        type: "error", code: "invalid_config",
        message: 'Recorded adapter "custom.issue" on https://acme.atlassian.net uses "jira:{host}/issues/{key}". Review its rules in settings.',
      };
    } });
    await settle();
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.match(page.el("capture-site-message").textContent, /custom.issue.*jira:\{host\}\/issues\/\{key\}/);
    assert.doesNotMatch(page.el("capture-site-message").textContent, /Saving enables/);
    page.el("capture-site-save").click();
    assert.equal(updates(page).length, 0);
  });
}
