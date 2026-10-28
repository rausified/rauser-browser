// Built side-panel tests: capture site recurring. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  NOTES_URL, PROTOCOL_VERSION, WIKI, checks, configuredPage, draftRule,
  draftText, drafted, isResponseShape, notesDraft, previewRow, recurringReview,
  sampleRow, saveCapture, sentAfter, siteChecked, siteDrafted,
  starterOffersFor, until, updates,
} from "./harness.mjs";

test("the recurring-pages hint shows only after three unrecognized visits and sends nothing to the host", async () => {
  const two = await configuredPage(NOTES_URL, { patterns: [sampleRow("/notes/{num}", 2), sampleRow("/pages/{num}", 9)] });
  assert.equal(two.el("capture-site-area").hidden, true);
  assert.equal(two.el("capture-site-recurring").hidden, true);
  const recognized = await configuredPage(`${WIKI}/pages/42`, { current: "/pages/{num}", patterns: [sampleRow("/notes/{num}", 9)] });
  assert.equal(recognized.el("capture-site-area").hidden, true);
  const page = await configuredPage(NOTES_URL, { patterns: [sampleRow("/notes/{num}", 3)] });
  await until(() => !page.el("capture-site-recurring").hidden, "the recurring hint");
  assert.equal(page.el("capture-site-area").hidden, false);
  assert.equal(page.el("capture-site").hidden, true);
  assert.equal(page.el("capture-site-hint").textContent, "Looks like you have recurring pages on wiki.example.com.");
  assert.equal(page.el("capture-site-recurring").textContent, "Draft rules for these pages");
  for (const shown of [two, recognized, page]) {
    assert.deepEqual(shown.sent("draft_site"), []);
    assert.deepEqual(shown.sent("check_site_draft"), []);
    assert.equal(shown.state.scripts.length, 0, "no page links are read before the click");
  }
});

test("the sample reaches the host only after the user asks", async () => {
  const patterns = [sampleRow("/notes/{num}", 3), sampleRow("/pages/{num}", 5)];
  const page = await configuredPage(NOTES_URL, { patterns });
  await until(() => !page.el("capture-site-recurring").hidden, "the recurring hint");
  assert.deepEqual(page.sent("draft_site"), []);
  const from = page.state.requests.length;
  page.el("capture-site-recurring").click();
  await drafted(page);
  assert.deepEqual(sentAfter(page, from), ["draft_site"]);
  const [request] = page.sent("draft_site");
  assert.equal(request.origin, WIKI);
  // The current page's shape carries its sampled count; the sample follows.
  assert.deepEqual(request.visited, [{ shape: "/notes/{num}", count: 3 }, { shape: "/pages/{num}", count: 5 }]);
  assert.deepEqual(request.links, [{ shape: "/notes/{num}", count: 2 }]);
  assert.doesNotMatch(JSON.stringify({ ...request, origin: null, request_id: null }), /http|Work page|last_seen|42/);
  assert.equal(page.el("capture-site-heading").textContent, "Capture work on wiki.example.com");
  assert.match(draftText(page), /Brauser will remember: wiki\.notes — pages like \/notes\/<number>/);
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured/);
  // Canceling returns to the hint and saves nothing.
  page.el("capture-site-cancel").click();
  assert.equal(page.el("capture-site-review").hidden, true);
  assert.equal(page.el("capture-site-recurring").hidden, false);
  assert.equal(page.el("capture-site").hidden, true);
  assert.equal(updates(page).length, 0);
});

test("drafting on a configured origin adds adapters and never changes existing ones", async () => {
  const custom = { id: "wiki.pages:{host}/{num}", match: [{ path: "/pages/{num:int}" }], origin: WIKI, type: "wiki.pages", title: "{title} · Wiki" };
  const artifacts = [custom, { type: "tools.item", origin: "https://tools.example.com", match: [{ path: "/items/{id:int}" }], id: "tools.item:{host}/{id}" }];
  const page = await recurringReview(NOTES_URL, {
    artifacts, patterns: [sampleRow("/notes/{num}", 4)],
    intercept: (request, state) => request.type === "check_site_draft" ? siteChecked(request, state) : null,
  });
  assert.equal(page.el("capture-site-save").textContent, "Start capturing this site");
  await saveCapture(page);
  const [check] = checks(page);
  assert.deepEqual(check.adapters, [notesDraft().adapters[0].adapter]);
  const [update] = updates(page);
  assert.equal(update.config.adapters.artifacts.length, 3);
  assert.equal(JSON.stringify(update.config.adapters.artifacts.slice(0, 2)), JSON.stringify(artifacts));
  assert.deepEqual(update.config.adapters.artifacts[2], notesDraft().adapters[0].adapter);
  assert.equal(update.config.capture_enabled, true);
});

test("rules overlapping the origin's existing adapters are listed as already captured", async () => {
  // GitHub issues for one organization only. The sampled issues shape is
  // recognized by that adapter only without its owner scope, so the hint
  // offers it and the host lists it as already captured.
  const issues = starterOffersFor("https://github.com", ["acme-corp"]).find((offer) => offer.artifact_type === "github.issue").adapter;
  const github = "https://github.com";
  const page = await recurringReview(`${github}/acme-corp/widget/discussions/5`, {
    artifacts: [issues], current: "/{slug}/{slug}/discussions/{num}",
    patterns: [sampleRow("/{slug}/{slug}/discussions/{num}", 3), sampleRow("/{slug}/{slug}/issues/{num}", 4)],
    draft: (request) => ({
      type: "site_drafted", revision: "1", drafter: "structural",
      adapters: [{
        adapter: {
          type: "github.discussions", origin: github,
          match: [{ path: "/{owner:name|lower}/{repo:name|lower}/discussions/{num:int}" }],
          id: "github.discussions:{host}/{owner}/{repo}/{num}",
        },
        scope: "copied", tenant: { segment: 0, capture: "owner" },
        rules: [draftRule(0, `${github}/example/repo/discussions/123`, { visited: 3 })],
      }],
      dropped: [
        { shape: request.visited[1].shape, adapter: null, rule: null, reason: "already_recognized" },
        { shape: null, adapter: 0, rule: 1, reason: "draft_overlap" },
      ],
      preview: [], taken_types: ["github.issue"], remapped: 0,
    }),
  });
  const [request] = page.sent("draft_site");
  assert.deepEqual(request.visited.map((row) => row.shape), ["/{slug}/{slug}/discussions/{num}", "/{slug}/{slug}/issues/{num}"]);
  const dropped = page.el("capture-site-dropped").textContent;
  assert.match(dropped, /\/\{slug\}\/\{slug\}\/issues\/\{num\}: already captured by this site's rules/);
  assert.match(dropped, /A drafted github\.discussions rule: already captured by this site's rules/);
  // The copied scope keeps the new kind to the same organization.
  assert.match(page.el("capture-site-preview").textContent, /This page: will be captured/);
  assert.equal(page.el("capture-site-scope").hidden, true);
});

function siteDraftChecked() {
  return {
    type: "site_draft_checked", protocol_version: PROTOCOL_VERSION, request_id: "r2", revision: "sha256:00",
    rules: [[{ verdict: "kept", reason: null }], [{ verdict: "warned", reason: "draft_overlap" }]],
    scope: [
      { state: "pending", resolved: "scoped", reason: null },
      { state: "unscoped_only", resolved: null, reason: "draft_scope_choice" },
    ],
    preview: [previewRow()],
  };
}

test("site_drafted with an extra key or an unbounded list fails the response shape check", () => {
  assert.equal(isResponseShape(siteDrafted()), true);
  assert.equal(isResponseShape(siteDraftChecked()), true);
  const mutations = [
    (draft) => { draft.extra = 1; },
    (draft) => { draft.adapters[0].extra = 1; },
    (draft) => { draft.adapters[0].tenant.extra = 1; },
    (draft) => { draft.adapters[0].rules[0].extra = 1; },
    (draft) => { draft.dropped[0].extra = 1; },
    (draft) => { draft.preview[0].extra = 1; },
    (draft) => { delete draft.remapped; },
    (draft) => { draft.drafter = "llm"; },
    (draft) => { draft.adapters[0].scope = "scoped"; },
    (draft) => { draft.adapters[0].rules[0].unchecked_reason = "planted"; },
    (draft) => { draft.dropped[0].reason = "because"; },
    (draft) => { draft.preview[0].source = "tab"; },
    (draft) => { draft.adapters[0].adapter = []; },
    (draft) => { draft.adapters = Array.from({ length: 9 }, () => siteDrafted().adapters[0]); },
    (draft) => { draft.adapters[0].rules = Array.from({ length: 17 }, () => siteDrafted().adapters[0].rules[0]); },
    (draft) => { draft.dropped = Array.from({ length: 129 }, () => siteDrafted().dropped[0]); },
    (draft) => { draft.preview = Array.from({ length: 129 }, () => previewRow()); },
    (draft) => { draft.taken_types = Array.from({ length: 161 }, (_, n) => `t${n}`); },
    (draft) => { draft.remapped = -1; },
  ];
  for (const mutate of mutations) {
    const draft = siteDrafted();
    mutate(draft);
    assert.equal(isResponseShape(draft), false, mutate.toString());
  }
  const checkedMutations = [
    (checked) => { checked.extra = 1; },
    (checked) => { checked.rules[0][0].extra = 1; },
    (checked) => { checked.rules[0][0].verdict = "maybe"; },
    (checked) => { checked.scope[0].extra = 1; },
    (checked) => { checked.scope[0].state = "copied_maybe"; },
    (checked) => { checked.scope[0].resolved = "pending"; },
    (checked) => { checked.rules = Array.from({ length: 9 }, () => [{ verdict: "kept", reason: null }]); },
    (checked) => { checked.rules[0] = Array.from({ length: 17 }, () => ({ verdict: "kept", reason: null })); },
    (checked) => { checked.scope = Array.from({ length: 9 }, () => siteDraftChecked().scope[0]); },
    (checked) => { checked.preview = Array.from({ length: 129 }, () => previewRow()); },
    (checked) => { delete checked.preview; },
  ];
  for (const mutate of checkedMutations) {
    const checked = siteDraftChecked();
    mutate(checked);
    assert.equal(isResponseShape(checked), false, mutate.toString());
  }
});
