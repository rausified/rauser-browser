// Built side-panel tests: capture site. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  CONFIG, GITHUB_URL, JIRA_URL, captureReview, hostReply, install,
  recoveryBody, recoveryRows, saveCapture, settle, until, updates,
} from "./harness.mjs";

test("Capture Site waits for the host's history check before promising a save", async () => {
  let release;
  const held = new Promise((resolve) => { release = resolve; });
  const page = await captureReview(JIRA_URL, { waitForCheck: false, intercept: (request) =>
    request.type === "check_capture_site" ? held : undefined });
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.doesNotMatch(page.el("capture-site-message").textContent, /Saving enables/);
  release({ type: "capture_site_checked", revision: "1" });
  await until(() => !page.el("capture-site-save").disabled, "the history check to pass");
  assert.match(page.el("capture-site-message").textContent, /Saving enables/);
  await saveCapture(page);
});

test("Capture Site ignores history replies for superseded selections and canceled reviews", async () => {
  const pending = [];
  const page = await captureReview(GITHUB_URL, { waitForCheck: false, intercept(request) {
    if (request.type === "check_capture_site") return new Promise((resolve) => pending.push({ request, resolve }));
  } });
  await until(() => pending.length === 1, "the first preflight");
  page.el("capture-site-orgs").value = "other-org";
  page.el("capture-site-orgs").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  await until(() => pending.length === 2, "the edited preflight");
  assert.deepEqual(pending[1].request.adapters.artifacts[0].scope, { owner: ["other-org"] });
  assert.deepEqual(Object.keys(pending[1].request).sort(),
    ["adapters", "expected_revision", "protocol_version", "request_id", "type"]);
  pending[1].resolve({ type: "error", code: "invalid_config", message: "Recorded adapter conflicts; use settings." });
  await until(() => /Recorded adapter/.test(page.el("capture-site-message").textContent), "the current refusal");
  pending[0].resolve({ type: "capture_site_checked", revision: "1" });
  await settle();
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.match(page.el("capture-site-message").textContent, /Recorded adapter/);

  page.el("capture-site-orgs").value = "third-org";
  page.el("capture-site-orgs").dispatch("input");
  await until(() => pending.length === 3, "another preflight");
  page.el("capture-site-orgs").value = "";
  page.el("capture-site-orgs").dispatch("input");
  pending[2].resolve({ type: "capture_site_checked", revision: "1" });
  await settle();
  assert.equal(page.el("capture-site-save").disabled, true, "a reply cannot enable an invalid selection");

  page.el("capture-site-orgs").value = "fourth-org";
  page.el("capture-site-orgs").dispatch("input");
  await until(() => pending.length === 4, "the canceled preflight");
  page.el("capture-site-cancel").click();
  page.el("capture-site").click();
  await until(() => pending.length === 5, "the reopened review's preflight");
  pending[3].resolve({ type: "error", code: "invalid_config", message: "Obsolete refusal" });
  await settle();
  assert.doesNotMatch(page.el("capture-site-message").textContent, /Obsolete/);
  assert.equal(page.el("capture-site-save").disabled, true);
  pending[4].resolve({ type: "capture_site_checked", revision: "1" });
  await until(() => !page.el("capture-site-save").disabled, "the new review's check");
  await saveCapture(page);
});

for (const reply of [
  { type: "error", code: "invalid_request", message: "Upgrade the native host." },
  { type: "capture_site_checked", revision: "other-revision" },
  { type: "capture_site_checked" },
]) {
  test(`Capture Site stays disabled after a refused or mismatched check: ${JSON.stringify(reply)}`, async () => {
    const page = await captureReview(JIRA_URL, { intercept: (request) =>
      request.type === "check_capture_site" ? reply : undefined });
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.doesNotMatch(page.el("capture-site-message").textContent, /Saving enables/);
    page.el("capture-site-save").click();
    assert.equal(updates(page).length, 0);
  });
}

test("Capture Site previews Jira and installs the host-returned policy in one save", async () => {
  const page = await captureReview();
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured/);
  assert.match(page.el("capture-site-preview").textContent, /Illustrative example/);
  assert.match(page.el("capture-site-preview").textContent, /27.*examples skipped/);
  assert.match(page.el("capture-site-preview").textContent, /Skipped: https:\/\/acme.atlassian.net\/share/);
  assert.doesNotMatch(page.el("capture-site-preview").textContent, /"match"|"id"/);
  assert.equal(page.el("capture-site-heading").focused, true);
  await saveCapture(page);
  assert.equal(updates(page).length, 1);
  const update = updates(page)[0];
  assert.equal(update.expected_revision, "1");
  assert.equal(update.config.capture_enabled, true);
  assert.equal(update.config.adapters.artifacts[0].type, "jira.issue");
  assert.equal(update.picker_token, null);
  assert.equal(update.knowledge_base_token, null);
  const installed = page.state.workerRequests.filter((request) => request.kind === "install_policy").at(-1);
  assert.equal(installed.lease.revision, "2");
  assert.deepEqual(installed.lease.adapters, update.config.adapters);
  assert.equal(page.calls("permissions.request"), 0);
  assert.equal(page.calls("permissions.remove"), 0);
  // The panel's opening list_runs restores run cards; it is not part of the save.
  assert.deepEqual([...new Set(page.state.requests.map((r) => r.type).filter((type) => type !== "list_runs"))].sort(),
    ["check_capture_site", "get_artifact", "get_config", "get_site_restore", "hello", "load_note", "update_config"]);
});

test("Capture Site recognizes OneDrive document routes without scope or JSON", async () => {
  const url = "https://acme-my.sharepoint.com/personal/alex/_layouts/15/Doc.aspx?sourcedoc=%7B123e4567-e89b-12d3-a456-426614174000%7D";
  const page = await captureReview(url);
  assert.equal(page.el("capture-site-github").hidden, true);
  assert.match(page.el("capture-site-preview").textContent, /OneDrive for work documents/);
  assert.match(page.el("capture-site-preview").textContent, /will be captured/);
  await saveCapture(page);
  assert.equal(updates(page)[0].config.adapters.artifacts[0].id, "sharepoint:{host}/{doc}");
});

test("Capture Site pre-fills the organization from the current GitHub page", async () => {
  const page = await captureReview(GITHUB_URL);
  assert.equal(page.el("capture-site-github").hidden, false);
  assert.equal(page.el("capture-site-orgs").value, "acme-corp");
  assert.equal(page.el("capture-site-repos").checked, true);
  assert.equal(page.el("capture-site-prs").checked, true);
  assert.equal(page.el("capture-site-issues").checked, true);
  assert.equal(page.el("capture-site-save").disabled, false);
  assert.match(page.el("capture-site-preview").textContent, /Records repositories, pull requests and issues owned by: acme-corp\./);
  await saveCapture(page);
  assert.equal(updates(page)[0].config.adapters.artifacts.length, 3);
  for (const artifact of updates(page)[0].config.adapters.artifacts) {
    assert.deepEqual(artifact.scope, { owner: ["acme-corp"] });
  }
});

test("editing the pre-filled organization keeps the user's edit, not the page's owner", async () => {
  const page = await captureReview(GITHUB_URL);
  page.el("capture-site-orgs").value = "other-org";
  page.el("capture-site-orgs").dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /owned by: other-org\./);
  await saveCapture(page);
  assert.deepEqual(updates(page)[0].config.adapters.artifacts[0].scope, { owner: ["other-org"] });
});

test("unchecking repositories omits the repo adapter", async () => {
  const page = await captureReview(GITHUB_URL);
  page.el("capture-site-repos").checked = false;
  page.el("capture-site-repos").dispatch("input");
  await until(() => !page.el("capture-site-save").disabled, "the edited selection preflight");
  await saveCapture(page);
  assert.deepEqual(updates(page)[0].config.adapters.artifacts.map((artifact) => artifact.type),
    ["github.pr", "github.issue"]);
});

test("the confirmation line names only the checked types, truthfully and in a fixed order", async () => {
  const page = await captureReview(GITHUB_URL);
  page.el("capture-site-repos").checked = false;
  page.el("capture-site-repos").dispatch("input");
  page.el("capture-site-prs").checked = false;
  page.el("capture-site-prs").dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /Records issues owned by: acme-corp\./);

  page.el("capture-site-repos").checked = true;
  page.el("capture-site-repos").dispatch("input");
  page.el("capture-site-prs").checked = true;
  page.el("capture-site-prs").dispatch("input");
  page.el("capture-site-issues").checked = false;
  page.el("capture-site-issues").dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /Records repositories and pull requests owned by: acme-corp\./);

  page.el("capture-site-issues").checked = true;
  page.el("capture-site-issues").dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /Records repositories, pull requests and issues owned by: acme-corp\./);
});

test("unchecking every GitHub kind disables save, and clearing the organization blocks it", async () => {
  const page = await captureReview(GITHUB_URL);
  for (const id of ["capture-site-repos", "capture-site-prs", "capture-site-issues"]) {
    page.el(id).checked = false;
    page.el(id).dispatch("input");
  }
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(page.el("capture-site-orgs").disabled, true);
  page.el("capture-site-repos").checked = true;
  page.el("capture-site-repos").dispatch("input");
  page.el("capture-site-orgs").value = "";
  page.el("capture-site-orgs").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
});

test("canceling and switching tabs clear a review without changing capture", async () => {
  const page = await captureReview(GITHUB_URL);
  page.el("capture-site-orgs").value = "sensitive-org";
  page.el("capture-site-cancel").click();
  assert.equal(page.el("capture-site-review").hidden, true);
  assert.equal(page.el("capture-site-orgs").value, "");
  assert.equal(updates(page).length, 0);
  page.el("capture-site").click();
  page.navigate(JIRA_URL, 2);
  await until(() => page.el("capture-site-review").hidden, "tab change to dismiss the review");
  page.el("capture-site-save").click();
  assert.equal(updates(page).length, 0);
  page.navigate("chrome://extensions", 3);
  await until(() => page.el("capture-site-area").hidden, "non-web tab to hide capture setup");
});

test("a same-URL tab switch and an SPA route change dismiss the review", async () => {
  const page = await captureReview();
  page.navigate(JIRA_URL, 2);
  await until(() => page.el("capture-site-review").hidden, "same-URL tab switch to dismiss review");
  page.el("capture-site").click();
  assert.equal(page.el("capture-site-review").hidden, false);
  page.navigate("https://acme.atlassian.net/browse/PROJ-43", 2);
  await until(() => page.el("capture-site-review").hidden, "SPA navigation to dismiss review");
  assert.equal(updates(page).length, 0);
});

test("a concurrent configuration save refuses the old review rather than rebasing it", async () => {
  const page = await captureReview();
  page.state.config.strip_params = ["tracking"];
  page.state.revision = "2";
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Settings changed"), "stale config refusal");
  await until(() => page.el("capture-site-message").textContent.includes("Settings changed"), "stale review marker");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(updates(page).length, 0);
  assert.deepEqual(page.state.config.strip_params, ["tracking"]);
});

test("host revision conflicts keep the reviewed site unsaved", async () => {
  const page = await captureReview(JIRA_URL, { intercept(request) {
    if (request.type === "update_config") return { type: "error", code: "conflict", message: "Settings changed at commit" };
  } });
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Settings changed at commit"), "revision conflict to surface");
  await until(() => !page.el("note-body").disabled, "old note to return after failed commit");
  assert.equal(page.state.config.adapters.artifacts.length, 0);
  assert.equal(updates(page).length, 1);
  assert.equal(page.el("capture-site-review").hidden, false);
});

test("a pending local pause or removal blocks capture both at review and at save", async () => {
  for (const workerStatus of [
    { pause_pending: true, pause_token: "pause" },
    { pending_removals: ["https://acme.atlassian.net"] },
  ]) {
    const page = await captureReview(JIRA_URL, { workerStatus });
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.match(page.el("capture-site-message").textContent, /pause|removal/i);
    assert.equal(updates(page).length, 0);
  }
  const page = await captureReview();
  page.state.workerStatus.pause_pending = true;
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("pending"), "new local pause to refuse save");
  assert.equal(updates(page).length, 0);
});

test("adding another site leaves global capture paused", async () => {
  const config = structuredClone(CONFIG);
  const { starterOffers } = await import("../../dist/starter-adapters.js");
  config.adapters.artifacts = starterOffers("https://first.atlassian.net", []).map((offer) => offer.adapter);
  const page = await captureReview(JIRA_URL, { config });
  assert.match(page.el("capture-site-save").textContent, /keep capture paused/);
  await saveCapture(page);
  assert.equal(updates(page)[0].config.capture_enabled, false);
  assert.equal(updates(page)[0].config.adapters.artifacts.length, 2);
  assert.deepEqual(updates(page)[0].config.adapters.artifacts[0], config.adapters.artifacts[0]);
});

test("configured sites, missing folders, and invalid policies cannot use Capture Site", async () => {
  const { starterOffers } = await import("../../dist/starter-adapters.js");
  for (const options of [
    { config: { ...CONFIG, storage: null } },
    { adapterIssue: "Future policy format" },
    { configIssue: "Folder moved" },
    { config: { ...CONFIG, adapters: { format: 1, artifacts: starterOffers("https://acme.atlassian.net", []).map((offer) => offer.adapter) } } },
  ]) {
    const page = await install({ tab: { id: 1, url: JIRA_URL }, ...options });
    await until(() => !page.el("capture-site-area").hidden, "capture area to settle");
    assert.ok(page.el("capture-site").disabled || page.el("capture-site").hidden);
    page.el("capture-site").click();
    assert.equal(updates(page).length, 0);
    assert.equal(page.el("capture-site-review").hidden, true);
  }
});

test("pending note edits save before capture changes identity, then load the new identity", async () => {
  const page = await captureReview();
  page.el("note-body").value = "Keep this text";
  page.el("note-body").dispatch("input");
  await saveCapture(page);
  const requests = page.state.requests;
  const saved = requests.findIndex((request) => request.type === "save_note");
  const updated = requests.findIndex((request) => request.type === "update_config");
  assert.ok(saved >= 0 && saved < updated);
  assert.equal(requests[saved].body, "Keep this text");
  assert.ok(requests.slice(updated + 1).some((request) => request.type === "load_note"));
});

test("the save rechecks the active tab even before Chrome delivers a tab event", async () => {
  const page = await captureReview();
  page.state.tab.url = "https://other.atlassian.net/browse/OTHER-1";
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("active page changed"), "fresh-tab refusal");
  assert.equal(updates(page).length, 0);
});

test("note loads stay suspended through title updates during a capture save", async () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  const page = await captureReview(JIRA_URL, { async intercept(request) {
    if (request.type === "update_config") await gate;
  } });
  page.el("capture-site-save").click();
  page.el("capture-site-save").click();
  await until(() => updates(page).length === 1, "in-flight config save");
  const initialLoads = page.state.requests.filter((request) => request.type === "load_note").length;
  const initialQueries = page.calls("tabs.query");
  page.navigate(JIRA_URL);
  await until(() => page.calls("tabs.query") > initialQueries, "title/tab refresh during save");
  await new Promise((resolve) => setTimeout(resolve, 5));
  assert.equal(page.el("note-body").disabled, true);
  assert.equal(page.state.requests.filter((request) => request.type === "load_note").length, initialLoads);
  release();
  await until(() => page.el("capture-site-review").hidden, "capture save to complete");
  await until(() => !page.el("note-body").disabled, "new note identity to load");
  assert.equal(updates(page).length, 1);
});

test("a failed old-note save keeps a copy-out through capture setup", async () => {
  const page = await captureReview(JIRA_URL, { intercept(request) {
    if (request.type === "save_note") return { type: "error", code: "internal", message: "Notes volume unavailable" };
  } });
  page.el("note-body").value = "Never lose my edit";
  page.el("note-body").dispatch("input");
  await saveCapture(page);
  assert.equal(page.el("note-conflict").hidden, false);
  assert.equal(recoveryBody(recoveryRows(page)[0]).value, "Never lose my edit");
  assert.equal(updates(page).length, 1);
});

test("switching pages during the post-capture note reload follows the new note", async () => {
  let release;
  const gate = new Promise((resolve) => { release = resolve; });
  let reloading = false;
  const nextUrl = "https://acme.atlassian.net/browse/PROJ-43";
  const page = await captureReview(JIRA_URL, { async intercept(request, state) {
    if (request.type === "load_note" && state.revision === "2") {
      if (request.url === JIRA_URL) {
        reloading = true;
        await gate;
      }
      return { ...hostReply(request), body: request.url };
    }
  } });
  page.el("capture-site-save").click();
  await until(() => reloading, "post-save note reload");
  try {
    page.navigate(nextUrl, 2);
    await until(() => page.el("page-url").textContent === nextUrl, "new page header");
  } finally {
    release();
  }
  await until(() => !page.el("note-body").disabled && page.el("note-body").value === nextUrl,
    "the new page's note after capture setup");
  page.el("note-body").value = "Edit for the new page";
  page.el("note-body").dispatch("input");
  // A tab change flushes the edit without waiting for the autosave timer.
  page.navigate("https://other.example/", 3);
  await until(() => page.state.requests.some((request) => request.type === "save_note"), "note save");
  const saved = page.state.requests.find((request) => request.type === "save_note");
  assert.equal(saved.url, nextUrl);
  assert.equal(saved.body, "Edit for the new page");
});

test("a verification failure after commit says the site was saved and never resubmits", async () => {
  const page = await captureReview(JIRA_URL, { intercept(request, state) {
    if (request.type === "get_config" && state.revision === "2") {
      return { type: "error", code: "internal", message: "Status temporarily unavailable" };
    }
  } });
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Site saved, but"), "post-commit failure message");
  assert.equal(updates(page).length, 1);
  assert.equal(page.state.config.adapters.artifacts[0].type, "jira.issue");
  assert.ok(page.state.workerRequests.some((request) => request.kind === "suspend_policy"));
  assert.equal(page.el("capture-site-review").hidden, true);
});

test("a policy update makes an open review stale before it can be saved", async () => {
  const page = await captureReview();
  page.state.revision = "2";
  page.policyChanged();
  await until(() => page.el("capture-site-message").textContent.includes("Settings changed"), "policy revision refresh");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(updates(page).length, 0);
});

test("private tabs do not offer Capture Site and unknown origins never get the starter flow", async () => {
  const page = await install({ tab: { id: 1, url: JIRA_URL, incognito: true } });
  await until(() => page.calls("tabs.query") > 0, "tab query");
  assert.equal(page.el("capture-site-area").hidden, true);
  const unknown = await install({ tab: { id: 1, url: "https://unknown.example/items/1" } });
  await until(() => !unknown.el("capture-site-area").hidden, "the drafted flow to be offered");
  assert.equal(unknown.el("capture-site-github").hidden, true);
  assert.equal(unknown.el("capture-site-review").hidden, true);
});
