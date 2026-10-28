// Built side-panel tests: capture site draft save. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  CONFIG, LINEAR_URL, WIKI, WIKI_URL, checks, draftRule, draftedSave,
  hostReply, indexOf, install, linearDraft, ruleBox, saveCapture, siteChecked,
  until, updates,
} from "./harness.mjs";

test("saving a draft unchecked down to one rule saves exactly that rule", async () => {
  const page = await draftedSave();
  assert.equal(ruleBox(page, 0, 1).checked, false);
  assert.equal(ruleBox(page, 1, 0).checked, false);
  await saveCapture(page);
  assert.equal(checks(page).length, 1);
  const [check] = checks(page);
  const [draftRequest] = page.sent("draft_site");
  assert.equal(check.origin, WIKI);
  assert.equal(check.expected_revision, "1");
  assert.deepEqual(check.adapters, [{
    type: "wiki.pages", origin: WIKI, match: [{ path: "/pages/{num:int}" }], id: "wiki.pages:{host}/{num}",
  }]);
  assert.deepEqual(check.provenance, [["drafted"]]);
  assert.deepEqual(check.scope, [null]);
  assert.deepEqual(check.visited, draftRequest.visited);
  assert.deepEqual(check.links, draftRequest.links);
  assert.ok(indexOf(page, "check_site_draft") < indexOf(page, "update_config"));
  assert.equal(updates(page).length, 1);
  const [update] = updates(page);
  assert.equal(update.expected_revision, "1");
  assert.deepEqual(update.config.adapters.artifacts, check.adapters);
  assert.equal(update.config.capture_enabled, true);
  assert.equal(page.calls("permissions.request"), 0);
  assert.match(page.el("status").textContent, /Saved capture rules for wiki\.example\.com\. Capture is on\./);
});

test("a revision change between draft and save refuses without rebasing", async () => {
  // Another settings page saved after the draft was reviewed.
  const page = await draftedSave();
  page.state.config.strip_params = ["tracking"];
  page.state.revision = "2";
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Settings changed"), "stale config refusal");
  await until(() => page.el("capture-site-message").textContent.includes("Settings changed"), "stale review marker");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(checks(page).length, 0);
  assert.equal(updates(page).length, 0);
  assert.deepEqual(page.state.config.strip_params, ["tracking"]);

  // Another save lands while the selection is being re-checked: the check
  // passed for revision 1, and update_config is refused, never rebased.
  const raced = await draftedSave(WIKI_URL, { check(request, state) {
    const reply = siteChecked(request, state);
    state.config = { ...state.config, strip_params: ["raced"] };
    state.revision = "2";
    return reply;
  } });
  raced.el("capture-site-save").click();
  await until(() => raced.el("status").textContent.includes("Could not capture site"), "conflict at commit");
  assert.equal(updates(raced).length, 1);
  assert.deepEqual(raced.state.config.strip_params, ["raced"]);
  assert.equal(raced.state.config.adapters.artifacts.length, 0);

  // A check answered for another revision is refused before any save.
  const other = await draftedSave(WIKI_URL, { check: (request, state) => ({ ...siteChecked(request, state), revision: "9" }) });
  other.el("capture-site-save").click();
  await until(() => other.el("status").textContent.includes("Settings changed"), "stale check refusal");
  assert.equal(updates(other).length, 0);
});

test("a check_site_draft refusal at save time saves nothing", async () => {
  // A rule that now overlaps a rule saved meanwhile is dropped by the host.
  const page = await draftedSave(WIKI_URL, { check: (request, state) => ({
    ...siteChecked(request, state),
    rules: [[{ verdict: "dropped", reason: "draft_overlap" }]],
  }) });
  page.el("capture-site-save").click();
  await until(() => page.el("status").textContent.includes("Could not capture site"), "check refusal");
  assert.match(page.el("status").textContent, /already captured by this site's rules/);
  assert.doesNotMatch(page.el("status").textContent, /draft_overlap/);
  assert.equal(checks(page).length, 1);
  assert.equal(updates(page).length, 0);
  assert.equal(page.state.requests.filter((request) => request.type === "save_note").length, 0);
  assert.equal(page.el("capture-site-review").hidden, false);

  // A refused scope choice.
  const scoped = await draftedSave(LINEAR_URL, { draft: () => linearDraft(),
    script: async () => ({ current: "/{slug}/issue/{key}", links: [] }),
    check: (request, state) => ({
      ...siteChecked(request, state, ["pending"]),
      scope: [{ state: "pending", resolved: null, reason: "draft_scope_choice" }],
    }) });
  scoped.el("capture-site-scoped").checked = true;
  scoped.el("capture-site-scoped").dispatch("input");
  scoped.el("capture-site-save").click();
  await until(() => scoped.el("status").textContent.includes("Could not capture site"), "scope refusal");
  assert.equal(updates(scoped).length, 0);

  // A host error, or a reply that does not answer every checked rule.
  for (const check of [
    () => ({ type: "error", code: "invalid_request", message: "checked adapters are invalid or over their bounds" }),
    (request, state) => ({ ...siteChecked(request, state), rules: [[]] }),
  ]) {
    const refused = await draftedSave(WIKI_URL, { check });
    refused.el("capture-site-save").click();
    await until(() => refused.el("status").textContent.includes("Could not capture site"), "refusal");
    assert.equal(updates(refused).length, 0);
  }
});

test("saving a pending-scope shared-origin draft with chosen scope succeeds", async () => {
  const page = await draftedSave(LINEAR_URL, { draft: () => linearDraft(), scopes: ["pending"],
    script: async () => ({ current: "/{slug}/issue/{key}", links: [] }) });
  // One click: the tab's tenant is already chosen.
  assert.equal(page.el("capture-site-scoped").checked, true);
  assert.equal(page.el("capture-site-scope-values").value, "acme");
  await saveCapture(page);
  const [check] = checks(page);
  assert.deepEqual(check.adapters, [linearDraft().adapters[0].adapter]);
  assert.equal("scope" in check.adapters[0], false);
  assert.deepEqual(check.scope, [{ values: ["acme"] }]);
  const saved = updates(page)[0].config.adapters.artifacts;
  assert.equal(saved.length, 1);
  assert.deepEqual(saved[0].scope, { workspace: ["acme"] });
  assert.equal(saved[0].unscoped, undefined);
});

test("saving an unscoped-only origin draft with the unscoped statement succeeds", async () => {
  const docsUrl = "https://docs.google.com/document/d/a1b2c3d4e5f6a7b8c9d0e1/edit";
  const table = { type: "docs.document", origin: "https://docs.google.com", match: [{ path: "/document/d/{doc:token}/edit" }], id: "docs.document:{host}/{doc}" };
  const page = await draftedSave(docsUrl, { scopes: ["unscoped_only"], draft: () => ({
    ...linearDraft(),
    adapters: [{ adapter: table, scope: "unscoped_only", tenant: null, rules: [draftRule(0, "https://docs.google.com/document/d/a1b2/edit")] }],
    preview: [],
  }), script: async () => ({ current: "/document/d/{id}/edit", links: [] }) });
  page.el("capture-site-unscoped").checked = true;
  page.el("capture-site-unscoped").dispatch("input");
  await saveCapture(page);
  const [check] = checks(page);
  assert.deepEqual(check.adapters, [table]);
  assert.deepEqual(check.scope, ["unscoped"]);
  const saved = updates(page)[0].config.adapters.artifacts;
  assert.deepEqual(saved, [{ ...table, unscoped: "acknowledged" }]);
});

test("a copied scope is not echoed into check_site_draft and still saves", async () => {
  // The panel offers drafting on configured origins only from Task 13, so
  // this drives the same check-then-save sequence the commit runs.
  const { starterOffers } = await import("../../dist/starter-adapters.js");
  const { planDraftedSite } = await import("../../dist/capture-site.js");
  const { checkDraftedSave } = await import("../../dist/capture-site-panel.js");
  const { ConfigSession } = await import("../../dist/settings.js");
  const GITHUB = "https://github.com";
  // Pull request and issue rules only: a repository rule's catch-all would
  // also cover the drafted route, which the host drops as an overlap.
  const existing = starterOffers(GITHUB, ["acme-corp"])
    .filter((offer) => offer.artifact_type !== "github.repo")
    .map(({ adapter: { within: _within, ...adapter } }) => adapter);
  const config = { ...CONFIG, adapters: { format: 1, artifacts: existing } };
  const table = { type: "github.issues", origin: GITHUB, match: [{ path: "/{owner:name|lower}/issues/{num:int}" }], id: "github.issues:{host}/{owner}/{num}" };
  const draft = {
    ...linearDraft(),
    adapters: [{ adapter: table, scope: "copied", tenant: { segment: 0, capture: "owner" }, rules: [draftRule(0, `${GITHUB}/example/issues/123`)] }],
    preview: [],
  };
  const url = `${GITHUB}/acme-corp/issues/7`;
  const page = await install({ tab: { id: 1, url }, config,
    intercept: (request, state) => request.type === "check_site_draft" ? siteChecked(request, state, ["copied"]) : null });
  await until(() => !page.el("note-body").disabled, "the note to load");
  const result = planDraftedSite(config, draft, { checked: [[true]], types: [null], scope: [null], titleSuffix: "" }, url, "");
  assert.equal(result.ok, true, result.message);
  const plan = result.plan;
  const session = new ConfigSession(() => undefined);
  session.config = structuredClone(config);
  session.revision = "1";
  await checkDraftedSave(session, plan, "1", { origin: GITHUB, visited: [{ shape: "/{slug}/issues/{num}", count: 1 }], links: [] });
  const [check] = page.sent("check_site_draft");
  assert.deepEqual(check.scope, [null]);
  assert.deepEqual(check.adapters, [table]);
  assert.equal(JSON.stringify(check).includes("acme-corp"), false);
  assert.equal(await session.saveConfig({ ...config, adapters: plan.document, capture_enabled: plan.startsCapture }, null), true);
  const saved = updates(page)[0].config.adapters.artifacts;
  assert.deepEqual(saved.slice(0, 2), existing);
  assert.deepEqual(saved[2], { ...table, scope: { owner: ["acme-corp"] } });
});

test("a title entry does not trip draft_forbidden_key at save", async () => {
  const title = "Secret Launch Plan · Acme Wiki";
  const page = await draftedSave(WIKI_URL, { title });
  page.el("capture-site-title-suffix").value = " · Acme Wiki";
  page.el("capture-site-title-suffix").dispatch("input");
  await saveCapture(page);
  const [check] = checks(page);
  assert.equal("title" in check.adapters[0], false);
  for (const secret of ["Secret", "Acme Wiki", "Launch"]) assert.doesNotMatch(JSON.stringify(check), new RegExp(secret));
  assert.equal(updates(page)[0].config.adapters.artifacts[0].title, "{title} · Acme Wiki");
});

test("a saved drafted site starts capture and reloads the note under its host-derived key", async () => {
  const key = { kind: "artifact", value: "wiki.pages:wiki.example.com/42" };
  const page = await draftedSave(WIKI_URL, { intercept(request, state) {
    if (request.type === "load_note" && state.revision === "2") return { ...hostReply(request), key, body: "artifact note" };
  } });
  page.el("note-body").value = "Keep this text";
  page.el("note-body").dispatch("input");
  await saveCapture(page);
  const requests = page.state.requests;
  const checked = indexOf(page, "check_site_draft");
  const saved = indexOf(page, "save_note");
  const updated = indexOf(page, "update_config");
  // The selection is re-checked, then old note edits finish, then the save.
  assert.ok(checked >= 0 && checked < saved && saved < updated);
  assert.equal(requests[saved].body, "Keep this text");
  assert.equal(updates(page)[0].config.capture_enabled, true);
  const installed = page.state.workerRequests.filter((request) => request.kind === "install_policy").at(-1);
  assert.equal(installed.lease.revision, "2");
  assert.deepEqual(installed.lease.adapters, updates(page)[0].config.adapters);
  assert.ok(requests.slice(updated + 1).some((request) => request.type === "load_note"));
  await until(() => page.el("note-body").value === "artifact note", "the artifact note");
});

test("a pending pause or removal blocks the drafted save", async () => {
  for (const workerStatus of [{ pause_pending: true, pause_token: "pause" }, { pending_removals: [WIKI] }]) {
    const page = await draftedSave(WIKI_URL, { workerStatus });
    assert.equal(page.el("capture-site-save").disabled, true);
    assert.match(page.el("capture-site-message").textContent, /pause|removal/i);
  }
  for (const change of [
    (status) => { status.pause_pending = true; },
    (status) => { status.pending_removals = [WIKI]; },
  ]) {
    const page = await draftedSave();
    change(page.state.workerStatus);
    page.el("capture-site-save").click();
    await until(() => page.el("status").textContent.includes("pending"), "the pending refusal");
    assert.equal(checks(page).length, 0);
    assert.equal(updates(page).length, 0);
  }
});
