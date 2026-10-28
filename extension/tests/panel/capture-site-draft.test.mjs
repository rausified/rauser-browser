// Built side-panel tests: capture site draft. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  LINEAR_URL, WIKI, WIKI_URL, descendants, draftReview, draftRule, draftText,
  drafted, linearDraft, ruleBox, sentAfter, until, updates, wikiDraft,
} from "./harness.mjs";

function notionDraft(revision = "1") {
  return {
    type: "site_drafted", revision, drafter: "structural",
    adapters: [{
      adapter: {
        type: "notion.page", origin: "https://www.notion.so",
        match: [{ path: "/{workspace:name|lower}/{hex:hex(32)@tail}" }],
        id: "notion.page:{host}/{workspace}/{hex}",
      },
      scope: "pending", tenant: { segment: 0, capture: "workspace" },
      rules: [draftRule(0, "https://www.notion.so/example/0123456789abcdef0123456789abcdef")],
    }],
    dropped: [], preview: [], taken_types: [], remapped: 0,
  };
}
const typeField = (page, adapter) => descendants(page.el("capture-site-draft"))
  .find((node) => node.tagName === "INPUT" && node.dataset.type === String(adapter));

test("Capture Site on an unknown site drafts with no permission request or native dialog", async () => {
  let confirms = 0;
  const page = await draftReview();
  globalThis.window.confirm = () => { confirms += 1; return false; };
  await drafted(page);
  assert.equal(page.calls("permissions.request"), 0);
  assert.equal(page.calls("permissions.remove"), 0);
  assert.equal(confirms, 0);
  assert.deepEqual(sentAfter(page, page.from), ["get_site_restore", "draft_site"]);
  const [request] = page.sent("draft_site");
  assert.equal(request.origin, WIKI);
  assert.deepEqual(request.visited, [{ shape: "/pages/{num}", count: 1 }]);
  assert.deepEqual(request.links, [{ shape: "/archive/{num}", count: 4 }, { shape: "/space/{id}", count: 1 }]);
  // The origin has no adapter, so the worker has no sample to read.
  assert.equal(page.state.workerRequests.filter((message) => message.kind === "get_site_patterns").length, 0);
  assert.equal(page.el("capture-site-heading").textContent, "Capture work on wiki.example.com");
  assert.equal(page.el("capture-site-github").hidden, true);
  assert.equal(updates(page).length, 0);
});

test("link-only and token-only rules start unchecked with their labels", async () => {
  const page = await draftReview();
  await drafted(page);
  assert.equal(ruleBox(page, 0, 0).checked, true);
  assert.equal(ruleBox(page, 0, 1).checked, false);
  assert.equal(ruleBox(page, 1, 0).checked, false);
  const rows = descendants(page.el("capture-site-draft")).filter((node) => node.tagName === "LABEL");
  const labelOf = (adapter, rule) => rows.find((row) => row.children.includes(ruleBox(page, adapter, rule))).textContent;
  assert.match(labelOf(0, 1), /seen only in links on this page/);
  assert.match(labelOf(1, 0), /loose: its ID has no fixed shape/);
  assert.doesNotMatch(labelOf(0, 0), /seen only in links|loose/);
  assert.equal(typeField(page, 0).value, "wiki.pages");
});

test("the review lists what will and will not be captured with the real current page", async () => {
  const page = await draftReview();
  await drafted(page);
  const text = () => draftText(page);
  assert.match(text(), /Brauser will remember: wiki\.pages — pages like \/pages\/<number>/);
  assert.match(text(), /Illustrative example: https:\/\/wiki\.example\.com\/pages\/123/);
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured — https:\/\/wiki\.example\.com\/pages\/42/);
  assert.match(page.el("capture-site-preview").textContent, /Matches 1 of 2 visited patterns and 0 of 3 link patterns/);
  assert.match(page.el("capture-site-preview").textContent, /Illustrative example \(captured\): https:\/\/wiki\.example\.com\/pages\/123/);
  assert.doesNotMatch(text(), /"match"|"id"|\{num:int\}/);
  assert.equal(page.el("capture-site-save").disabled, false);
  // Opting into the link-only rule counts its link pattern.
  ruleBox(page, 0, 1).checked = true;
  ruleBox(page, 0, 1).dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /Matches 1 of 2 visited patterns and 1 of 3 link patterns/);
  // With this page's rule dropped, the review says so.
  ruleBox(page, 0, 0).checked = false;
  ruleBox(page, 0, 0).dispatch("input");
  assert.match(page.el("capture-site-preview").textContent, /This page: will not be captured/);
  // Nothing checked leaves nothing to save.
  ruleBox(page, 0, 1).checked = false;
  ruleBox(page, 0, 1).dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.match(page.el("capture-site-message").textContent, /Choose at least one/);
  // A rename that collides with the other kind is refused in words.
  ruleBox(page, 0, 0).checked = true;
  ruleBox(page, 0, 0).dispatch("input");
  ruleBox(page, 1, 0).checked = true;
  ruleBox(page, 1, 0).dispatch("input");
  typeField(page, 0).value = "wiki.space";
  typeField(page, 0).dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.match(page.el("capture-site-message").textContent, /both named "wiki\.space"/);
  typeField(page, 0).value = "wiki.doc";
  typeField(page, 0).dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, false);
  assert.match(text(), /Brauser will remember: wiki\.doc/);
  assert.equal(updates(page).length, 0);
});

test("dropped rules and safety probes are listed in plain words", async () => {
  const page = await draftReview();
  await drafted(page);
  const dropped = page.el("capture-site-dropped").textContent;
  assert.match(dropped, /Brauser won't capture/);
  assert.match(dropped, /\/search\?q=\{slug\}: search or paging pages/);
  assert.match(dropped, /\/share\/\{id\}: sign-in, sharing or account pages/);
  assert.match(dropped, /\/dashboard: .*dashboard/);
  assert.match(dropped, /no fixed route word/);
  assert.match(dropped, /already captured/);
  assert.match(dropped, /Safety checks: 2 sign-in, search, dashboard, sharing and other pages skipped/);
  assert.match(dropped, /Skipped: https:\/\/wiki\.example\.com\/login/);
  assert.doesNotMatch(dropped, /draft_|search_or_paging|blocked_route|no_route_word/);
});

test("canceling, closing or switching tabs discards the draft and its link shapes and saves nothing", async () => {
  const cleared = (page) => {
    assert.equal(page.el("capture-site-review").hidden, true);
    assert.equal(page.el("capture-site-draft").children.length, 0);
    assert.equal(page.el("capture-site-dropped").children.length, 0);
    assert.equal(page.el("capture-site-preview").children.length, 0);
    assert.doesNotMatch(draftText(page), /archive|space/);
  };
  const page = await draftReview();
  await drafted(page);
  page.el("capture-site-cancel").click();
  cleared(page);
  page.el("capture-site-save").click();
  // Opening again drafts afresh.
  page.el("capture-site").click();
  await drafted(page);
  assert.equal(page.sent("draft_site").length, 2);
  // A reload of the reviewed tab (same URL) discards the review.
  page.tabUpdated({ status: "loading" });
  cleared(page);
  page.el("capture-site").click();
  await drafted(page);
  page.navigate("https://other.example/pages/1", 2);
  await until(() => page.el("capture-site-review").hidden, "a tab switch to discard the draft");
  cleared(page);
  assert.equal(updates(page).length, 0);

  // A draft that answers after Cancel is dropped.
  let finish;
  const held = new Promise((resolve) => { finish = resolve; });
  const second = await draftReview(WIKI_URL, { draft: () => held.then(() => wikiDraft()) });
  await until(() => second.sent("draft_site").length === 1, "the held draft");
  second.el("capture-site-cancel").click();
  finish();
  await new Promise((resolve) => setTimeout(resolve, 10));
  cleared(second);
  assert.equal(updates(second).length, 0);
});

test("a page that navigated before links were read cancels the draft without calling draft_site", async () => {
  // The tab moved on while the link shapes were being read.
  const moved = await draftReview(WIKI_URL, { script: async (_injection, state) => {
    state.tab.url = `${WIKI}/pages/43`;
    return { current: "/pages/{num}", links: [{ shape: "/stale/{num}", count: 1 }] };
  } });
  await until(() => moved.el("capture-site-review").hidden, "the draft to cancel");
  assert.equal(moved.sent("draft_site").length, 0);
  // A reload while the read is held.
  let finish;
  const held = new Promise((resolve) => { finish = resolve; });
  const reloaded = await draftReview(WIKI_URL, { script: () => held });
  await until(() => reloaded.state.scripts.length === 1, "the held link read");
  reloaded.tabUpdated({ status: "loading" });
  finish({ current: "/pages/{num}", links: [{ shape: "/stale/{num}", count: 1 }] });
  await new Promise((resolve) => setTimeout(resolve, 10));
  assert.equal(reloaded.el("capture-site-review").hidden, true);
  assert.equal(reloaded.sent("draft_site").length, 0);
  // The inspector itself found another document there.
  const stale = await draftReview(WIKI_URL, { script: async () => ({ current: null, links: [] }) });
  await until(() => stale.el("capture-site-message").textContent.startsWith("Could not draft"), "the honest error");
  assert.equal(stale.sent("draft_site").length, 0);
  assert.equal(stale.el("capture-site-save").disabled, true);
});

// Step 7.6's shared-origin flow, as John decided on 2026-10-27: the tenant
// is pre-filled from this tab's URL and shown as "Capture for <tenant>", so
// one click on Save confirms it; it stays editable, and the every-account
// statement stays the alternative. This replaced "scope pending, Enable
// disabled until the user chooses" (DESIGN §5.1, §12.2 step 7.6).
test("a shared origin pre-fills this tab's tenant as Capture for <tenant> and saves with one click", async () => {
  const page = await draftReview(LINEAR_URL, { draft: () => linearDraft(),
    script: async () => ({ current: "/{slug}/issue/{key}", links: [] }) });
  await drafted(page);
  assert.equal(page.el("capture-site-scope").hidden, false);
  assert.equal(page.el("capture-site-scope-values").value, "acme", "prefilled from this tab's URL");
  assert.equal(page.el("capture-site-scoped").checked, true, "the pre-filled tenant is already chosen");
  assert.equal(page.el("capture-site-scoped-text").textContent, "Capture for acme");
  assert.match(page.el("capture-site-scope-intro").textContent, /filled in acme from this page/);
  assert.equal(page.el("capture-site-unscoped").checked, false);
  assert.equal(page.el("capture-site-scope-values").disabled, false, "the tenant stays editable");
  assert.equal(page.el("capture-site-save").disabled, false, "Save is the one-click confirmation");
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured/);
  assert.match(page.el("capture-site-preview").textContent, /Records pages only for: acme\./);
  assert.match(page.el("capture-site-scope").textContent, /every account in this Chrome profile/);
  assert.match(page.el("capture-site-scope").textContent, /separate Chrome profile for work/);
  // The prefill never came from the host: the draft's example names a stand-in.
  assert.doesNotMatch(JSON.stringify(page.sent("draft_site")), /acme/);
  // Editing the list changes what is confirmed.
  page.el("capture-site-scope-values").value = "acme, partner";
  page.el("capture-site-scope-values").dispatch("input");
  assert.equal(page.el("capture-site-scoped-text").textContent, "Capture for acme and partner");
  assert.equal(page.el("capture-site-save").disabled, false);
  page.el("capture-site-scope-values").value = "";
  page.el("capture-site-scope-values").dispatch("input");
  assert.equal(page.el("capture-site-scoped-text").textContent, "Only these workspaces or accounts");
  assert.equal(page.el("capture-site-save").disabled, true);
  page.el("capture-site-unscoped").checked = true;
  page.el("capture-site-unscoped").dispatch("input");
  assert.equal(page.el("capture-site-scoped").checked, false, "the two choices exclude each other");
  assert.equal(page.el("capture-site-scope-values").disabled, true);
  assert.equal(page.el("capture-site-save").disabled, false);
  page.el("capture-site-unscoped").checked = false;
  page.el("capture-site-unscoped").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(updates(page).length, 0, "nothing is saved before Save");

  // A tab whose URL names no tenant at that position pre-fills nothing, so
  // the user still chooses before Save.
  const invite = await draftReview("https://linear.app/invite/abc123", { draft: () => linearDraft(),
    script: async () => ({ current: "/invite/{id}", links: [] }) });
  await drafted(invite);
  assert.equal(invite.el("capture-site-scope-values").value, "");
  assert.equal(invite.el("capture-site-scoped").checked, false);
  assert.equal(invite.el("capture-site-scoped-text").textContent, "Only these workspaces or accounts");
  assert.equal(invite.el("capture-site-scope-intro").textContent,
    "This site holds many workspaces and accounts. Choose which ones Brauser remembers.");
  assert.equal(invite.el("capture-site-save").disabled, true);
  assert.match(invite.el("capture-site-message").textContent, /workspaces or accounts/);

  // An app route at the tenant position is no tenant either: no drafted
  // rule records linear.app/settings/account, so nothing is pre-filled.
  const settings = await draftReview("https://linear.app/settings/account", { draft: () => linearDraft(),
    script: async () => ({ current: "/{slug}/{slug}", links: [] }) });
  await drafted(settings);
  assert.equal(settings.el("capture-site-scope-values").value, "");
  assert.equal(settings.el("capture-site-scoped").checked, false);
  assert.doesNotMatch(settings.el("capture-site-scope-intro").textContent, /filled in/);
  assert.equal(settings.el("capture-site-save").disabled, true);

  // A workspace-less Notion page: its title slug is no workspace.
  const hex = "0123456789abcdef0123456789abcdef";
  const notion = await draftReview(`https://www.notion.so/Meeting-Notes-${hex}`, { draft: () => notionDraft(),
    script: async () => ({ current: "/{tailhex32}", links: [] }) });
  await drafted(notion);
  assert.equal(notion.el("capture-site-scope-values").value, "");
  assert.equal(notion.el("capture-site-scoped").checked, false);
  assert.doesNotMatch(notion.el("capture-site-scope-intro").textContent, /filled in/);
  assert.equal(notion.el("capture-site-save").disabled, true);

  // A not-shared origin shows no scope controls.
  const wiki = await draftReview();
  await drafted(wiki);
  assert.equal(wiki.el("capture-site-scope").hidden, true);
  // An origin with no tenant position offers only the unscoped statement.
  const docsUrl = "https://docs.google.com/document/d/a1b2c3d4e5f6a7b8c9d0e1/edit";
  const docs = await draftReview(docsUrl, { draft: () => ({
    ...linearDraft(),
    adapters: [{
      adapter: { type: "docs.document", origin: "https://docs.google.com", match: [{ path: "/document/d/{doc:token}/edit" }], id: "docs.document:{host}/{doc}" },
      scope: "unscoped_only", tenant: null,
      rules: [draftRule(0, "https://docs.google.com/document/d/a1b2/edit")],
    }],
  }), script: async () => ({ current: "/document/d/{id}/edit", links: [] }) });
  await drafted(docs);
  assert.equal(docs.el("capture-site-scope").hidden, false);
  assert.equal(docs.el("capture-site-scope-values").hidden, true);
  assert.equal(docs.el("capture-site-scoped").hidden, true);
  assert.equal(docs.el("capture-site-save").disabled, true);
  docs.el("capture-site-unscoped").checked = true;
  docs.el("capture-site-unscoped").dispatch("input");
  assert.equal(docs.el("capture-site-save").disabled, false);
});

test("an empty draft explains why and offers advanced settings", async () => {
  const page = await draftReview(WIKI_URL, { draft: () => ({
    ...wikiDraft(), adapters: [], preview: [],
    dropped: [{ shape: "/{slug}/{tailhex32}", adapter: null, rule: null, reason: "draft_no_leading_literal" }],
  }) });
  await drafted(page);
  assert.equal(page.el("capture-site-advanced").hidden, false);
  assert.match(page.el("capture-site-message").textContent, /could not draft/i);
  assert.match(page.el("capture-site-dropped").textContent, /no fixed route word/);
  assert.equal(page.el("capture-site-save").disabled, true);
  page.el("capture-site-advanced").click();
  await until(() => page.state.created.length === 1, "advanced settings to open");
  const [key] = [...page.state.session.keys()].filter((name) => name.startsWith("captureSiteDraft:"));
  assert.equal(page.state.created[0].url, `chrome-extension://test-id/options.html#draft=${key.slice("captureSiteDraft:".length)}`);
  assert.deepEqual(page.state.session.get(key).adapters, []);
  assert.equal(page.el("capture-site-review").hidden, true, "the handed-off draft leaves the panel");
  assert.equal(page.el("capture-site-advanced").hidden, true);
  assert.equal(updates(page).length, 0);
});

test("the advanced handoff stores only the drafted adapters and shapes for 30 minutes", async () => {
  const { DRAFT_HANDOFF_TTL_MS, canonicalJson } = await import("../../dist/capture-site-handoff.js");
  const title = "Secret Launch Plan · Acme Wiki";
  const page = await draftReview(WIKI_URL, { title });
  await drafted(page);
  assert.equal(page.el("capture-site-advanced").hidden, false, "a drafted review also offers the advanced editor");
  page.el("capture-site-title-suffix").value = " · Acme Wiki";
  page.el("capture-site-title-suffix").dispatch("input");
  const before = Date.now();
  page.el("capture-site-advanced").click();
  await until(() => page.state.created.length === 1, "advanced settings to open");
  const keys = [...page.state.session.keys()].filter((name) => name.startsWith("captureSiteDraft:"));
  assert.equal(keys.length, 1);
  const entry = page.state.session.get(keys[0]);
  assert.equal(`captureSiteDraft:${entry.id}`, keys[0]);
  assert.equal(page.state.created[0].url, `chrome-extension://test-id/options.html#draft=${entry.id}`);
  assert.equal(entry.origin, WIKI);
  assert.ok(entry.created >= before && entry.created <= Date.now());
  assert.ok(DRAFT_HANDOFF_TTL_MS === 30 * 60_000);
  // The reviewed selection (rule 1 starts unchecked) without the title overlay.
  const table = wikiDraft().adapters[0].adapter;
  const kept = { ...table, match: [table.match[0]] };
  assert.deepEqual(entry.adapters, [kept]);
  assert.deepEqual(entry.baseline_rules, [kept.match.map(canonicalJson)]);
  assert.deepEqual(entry.visited, [{ shape: "/pages/{num}", count: 1 }]);
  assert.deepEqual(entry.links, [{ shape: "/archive/{num}", count: 4 }, { shape: "/space/{id}", count: 1 }]);
  const text = JSON.stringify(entry);
  assert.equal(text.includes(WIKI_URL), false, "no raw URL");
  assert.equal(text.includes("Secret Launch Plan"), false, "no tab title");
  assert.equal(text.includes("Acme Wiki"), false, "no title ending");
  assert.equal(page.el("capture-site-review").hidden, true);
  assert.equal(updates(page).length, 0);
  assert.deepEqual(sentAfter(page, page.from), ["get_site_restore", "draft_site"]);
});

test("the advanced handoff with no valid plan keeps only checked rules", async () => {
  const { canonicalJson } = await import("../../dist/capture-site-handoff.js");
  // The pre-filled workspace cleared, so the review has no plan; one rule
  // is link-only and one adapter is token-only, both unchecked by the host.
  const linear = linearDraft();
  linear.adapters[0].adapter.match.push({ path: "/{workspace:name|lower}/project/{key:jira_key}" });
  linear.adapters[0].rules.push(draftRule(1, "https://linear.app/example/project/ABC-123",
    { checked: false, unchecked_reason: "link_only", visited: 0, links: 3 }));
  linear.adapters.push({
    adapter: { type: "linear.doc", origin: "https://linear.app", match: [{ path: "/{workspace:name|lower}/document/{id:token}" }], id: "linear.doc:{host}/{workspace}/{id}" },
    scope: "pending", tenant: { segment: 0, capture: "workspace" },
    rules: [draftRule(0, "https://linear.app/example/document/a1b2", { checked: false, unchecked_reason: "token_only" })],
  });
  const page = await draftReview(LINEAR_URL, { draft: () => linear,
    script: async () => ({ current: "/{slug}/issue/{key}", links: [] }) });
  await drafted(page);
  page.el("capture-site-scope-values").value = "";
  page.el("capture-site-scope-values").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true, "no workspace is chosen");
  page.el("capture-site-advanced").click();
  await until(() => page.state.created.length === 1, "advanced settings to open");
  const [key] = [...page.state.session.keys()].filter((name) => name.startsWith("captureSiteDraft:"));
  const entry = page.state.session.get(key);
  const issue = { ...linear.adapters[0].adapter, match: [linear.adapters[0].adapter.match[0]] };
  assert.deepEqual(entry.adapters, [issue]);
  assert.deepEqual(entry.baseline_rules, [issue.match.map(canonicalJson)]);
  assert.doesNotMatch(JSON.stringify(entry), /project|document/);
});

test("with every rule unchecked the advanced handoff holds no drafted rule", async () => {
  const page = await draftReview();
  await drafted(page);
  ruleBox(page, 0, 0).checked = false;
  ruleBox(page, 0, 0).dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  page.el("capture-site-advanced").click();
  await until(() => page.state.created.length === 1, "advanced settings to open");
  const [key] = [...page.state.session.keys()].filter((name) => name.startsWith("captureSiteDraft:"));
  const entry = page.state.session.get(key);
  assert.deepEqual(entry.adapters, []);
  assert.deepEqual(entry.baseline_rules, []);
});

test("the tab title is shown only in the review and never sent to the drafter", async () => {
  const title = "Secret Launch Plan · Acme Wiki";
  const page = await draftReview(WIKI_URL, { title });
  await drafted(page);
  assert.equal(page.el("capture-site-title").hidden, false);
  assert.match(page.el("capture-site-title-current").textContent, /Secret Launch Plan · Acme Wiki/);
  page.el("capture-site-title-suffix").value = " · Acme Wiki";
  page.el("capture-site-title-suffix").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, false);
  page.el("capture-site-title-suffix").value = " · Other";
  page.el("capture-site-title-suffix").dispatch("input");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.match(page.el("capture-site-message").textContent, /does not end with/);
  // Only the origin and shapes cross: no raw URL, link href or tab title.
  const [request] = page.sent("draft_site");
  assert.deepEqual(Object.keys(request).sort(), ["links", "origin", "protocol_version", "request_id", "type", "visited"]);
  assert.equal(request.origin, WIKI);
  const shapes = JSON.stringify([request.visited, request.links]);
  for (const secret of ["Secret", "Acme Wiki", "http", "42", "wiki.example"]) assert.doesNotMatch(shapes, new RegExp(secret));
  assert.equal(page.sent("draft_site").length, 1);
  assert.deepEqual(sentAfter(page, page.from), ["get_site_restore", "draft_site"]);
});

test("an older host without draft_site asks for a host rebuild", async () => {
  const page = await draftReview(WIKI_URL, { draft: () => ({
    type: "error", code: "invalid_request", message: "invalid or unsupported request JSON",
  }) });
  await until(() => /rebuild/i.test(page.el("capture-site-message").textContent), "the rebuild message");
  assert.match(page.el("capture-site-message").textContent, /Rebuild and reinstall the Brauser host/);
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.equal(page.el("capture-site-draft").children.length, 0);
  // Any other refusal is shown as it is.
  const other = await draftReview(WIKI_URL, { draft: () => ({
    type: "error", code: "invalid_config", message: "adapter policy is unavailable",
  }) });
  await until(() => other.el("capture-site-message").textContent.includes("adapter policy is unavailable"), "the refusal");
  assert.doesNotMatch(other.el("capture-site-message").textContent, /rebuild/i);
});

test("a draft from another revision is stale before it can be saved", async () => {
  const page = await draftReview(WIKI_URL, { draft: () => wikiDraft("2") });
  await until(() => page.el("capture-site-message").textContent.includes("Settings changed"), "the stale draft");
  assert.equal(page.el("capture-site-save").disabled, true);
});

test("Page patterns review still works through the shared collector", async () => {
  const page = await draftReview();
  await drafted(page);
  page.el("site-pattern-review").click();
  await until(() => page.el("site-pattern-rows").textContent.includes("Link pattern"), "pattern results");
  assert.equal(page.state.scripts.length, 2);
  for (const injection of page.state.scripts) {
    assert.deepEqual(injection.target, { tabId: 1, frameIds: [0] });
    assert.equal(injection.world, "ISOLATED");
    assert.deepEqual(injection.args, [WIKI_URL, true]);
  }
  assert.equal(page.state.scripts[0].func, page.state.scripts[1].func);
  assert.match(page.el("site-pattern-rows").textContent, /Current page: \/pages\/\{num\}/);
  // Each review keeps its own state: closing one leaves the other.
  page.el("site-pattern-close").click();
  assert.equal(page.el("capture-site-review").hidden, false);
  assert.ok(page.el("capture-site-draft").children.length > 0);
});
