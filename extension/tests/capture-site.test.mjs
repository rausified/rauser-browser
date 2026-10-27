import assert from "node:assert/strict";
import test from "node:test";

const { captureOrigin, planCaptureSite } = await import("../dist/capture-site.js");
const { compileDocument, evaluate } = await import("../dist/adapter-templates.js");
const { starterOffers } = await import("../dist/starter-adapters.js");
const CONFIG = {
  storage: { root: "/vault" }, capture_enabled: false,
  adapters: { format: 1, artifacts: [] },
};
const BOTH = { pullRequests: "Acme-Corp", issues: "other-org" };
const plan = (url, config = CONFIG, choice = BOTH) => {
  const result = planCaptureSite(config, url, choice);
  assert.equal(result.ok, true, result.message);
  return result.plan;
};
const match = (plan, url) => {
  const result = compileDocument(plan.document);
  assert.equal(result.ok, true);
  return evaluate(result.doc, url);
};

test("Jira and OneDrive start from the current URL and retain starter identities", () => {
  for (const [url, id] of [
    ["https://acme.atlassian.net/browse/PROJ-42", "jira:acme.atlassian.net/PROJ-42"],
    ["https://acme.atlassian.net/jira/software/projects/PROJ/boards/7?selectedIssue=PROJ-42", "jira:acme.atlassian.net/PROJ-42"],
    ["https://acme.sharepoint.com/sites/work/_layouts/15/Doc.aspx?sourcedoc=%7B123e4567-e89b-12d3-a456-426614174000%7D", "sharepoint:acme.sharepoint.com/123e4567-e89b-12d3-a456-426614174000"],
    ["https://acme-my.sharepoint.com/personal/person/_layouts/15/Doc.aspx?sourcedoc=123e4567-e89b-12d3-a456-426614174000", "sharepoint:acme-my.sharepoint.com/123e4567-e89b-12d3-a456-426614174000"],
  ]) {
    const result = plan(url);
    assert.equal(result.current.result.artifact_id, id);
    assert.equal(result.startsCapture, true);
    assert.equal(result.probes.length, 27);
    assert.ok(result.probes.every((row) => row.probe && !row.result.match));
    assert.ok(result.examples.every((row) => !row.probe && row.result.match));
    assert.deepEqual(result.document.artifacts, starterOffers(new URL(url).origin, []).map((offer) => offer.adapter));
  }
});

test("GitHub choices keep PR and issue scopes separate and show current-page misses", () => {
  const result = plan("https://github.com/acme-corp/widget/pull/12/files");
  assert.equal(result.current.result.mode, "review");
  assert.deepEqual(result.document.artifacts[0].scope, { owner: ["acme-corp"] });
  assert.deepEqual(result.document.artifacts[1].scope, { owner: ["other-org"] });
  assert.equal(match(result, "https://github.com/other-org/widget/pull/12").match, false);
  assert.equal(match(result, "https://github.com/acme-corp/widget/issues/12").match, false);
  assert.equal(match(result, "https://github.com/other-org/widget/issues/12").match, true);
  assert.equal(match(result, "https://github.com/personal/widget/issues/12").match, false);
  assert.ok(result.examples.every((row) => row.result.match));
  const issueOnly = plan("https://github.com/acme-corp/widget/pull/12", CONFIG, { pullRequests: null, issues: "other-org" });
  assert.equal(issueOnly.document.artifacts.length, 1);
  assert.equal(issueOnly.current.result.match, false);
});

test("empty, invalid, or unselected GitHub scope cannot produce a save", () => {
  for (const choice of [
    { pullRequests: "", issues: "acme" }, { pullRequests: "acme", issues: "" },
    { pullRequests: null, issues: null }, { pullRequests: "bad/org", issues: null },
    { pullRequests: "*", issues: null },
  ]) assert.equal(planCaptureSite(CONFIG, "https://github.com/acme/repo/pull/1", choice).ok, false);
});

test("unknown, insecure, lookalike and credential-bearing sites stay outside starter setup", () => {
  for (const url of [
    "https://work.example/items/3", "http://acme.atlassian.net/browse/X-1",
    "https://github.com.evil.example/acme/repo/pull/1", "chrome://newtab", "not a URL",
    "https://user:secret@acme.atlassian.net/browse/ABC-1",
  ]) {
    assert.equal(captureOrigin(url), null);
    assert.equal(planCaptureSite(CONFIG, url, BOTH).ok, false);
  }
});

test("existing site policy is never replaced or supplemented, including custom definitions", () => {
  const artifacts = [{ type: "custom.work", origin: "https://github.com", match: [{ path: "/items/{id:int}" }], id: "custom:{host}/{id}", unscoped: true }];
  const config = { ...CONFIG, adapters: { format: 1, artifacts } };
  assert.match(planCaptureSite(config, "https://github.com/acme/repo/issues/1", BOTH).message, /already has capture rules/);
  assert.deepEqual(config.adapters.artifacts, artifacts);
});

test("adding a site preserves other definitions and an intentional global pause", () => {
  const config = structuredClone(CONFIG);
  config.adapters.artifacts = starterOffers("https://first.atlassian.net", []).map((offer) => offer.adapter);
  const before = structuredClone(config);
  const result = plan("https://second.atlassian.net/browse/ABC-1", config);
  assert.equal(result.startsCapture, false);
  assert.deepEqual(result.document.artifacts[0], config.adapters.artifacts[0]);
  assert.equal(plan("https://second.atlassian.net/browse/ABC-1", { ...config, capture_enabled: true }).startsCapture, true);
  assert.deepEqual(config, before);
  assert.equal(planCaptureSite({ ...CONFIG, storage: null }, "https://second.atlassian.net", BOTH).ok, false);
});

test("a dashboard is explicitly skipped without preventing useful site setup", () => {
  const result = plan("https://acme.atlassian.net/dashboard");
  assert.equal(result.current.result.match, false);
  assert.ok(result.examples[0].result.match);
});

test("invalid future documents and the whole-document adapter limit refuse a plan", () => {
  const future = { ...CONFIG, adapters: { format: 999, artifacts: [] } };
  assert.equal(planCaptureSite(future, "https://acme.atlassian.net", BOTH).ok, false);
  const artifacts = Array.from({ length: 64 }, (_, n) => starterOffers(`https://work${n}.atlassian.net`, [])[0].adapter);
  assert.equal(planCaptureSite({ ...CONFIG, adapters: { format: 1, artifacts } }, "https://acme.atlassian.net", BOTH).ok, false);
});

// --- Drafted Capture Site (unknown sites) -----------------------------------

const {
  captureMode, draftRequest, planDraftedSite, tenantPrefill,
} = await import("../dist/capture-site.js");

const WIKI = "https://wiki.example.com";
const LINEAR = "https://linear.app";
const DOCS = "https://docs.google.com";
const GITHUB = "https://github.com";
const PROBE = { source: "probe", shape: "/login", url: `${WIKI}/login`, adapter: null, rule: null, artifact_id: null };

function drafted(adapters, preview = [], taken = []) {
  return {
    type: "site_drafted", protocol_version: 6, request_id: "draft-1", revision: "7",
    drafter: "structural", adapters, dropped: [], preview, taken_types: taken, remapped: 0,
  };
}
const rule = (index, example, extra = {}) => ({
  rule: index, checked: true, unchecked_reason: null, visited: 1, links: 0, example, ...extra,
});
const wikiDraft = () => drafted([
  {
    adapter: {
      type: "wiki.pages", origin: WIKI,
      match: [{ path: "/pages/{num:int}" }, { path: "/archive/{num:int}" }],
      id: "wiki.pages:{host}/{num}",
    },
    scope: "not_shared", tenant: null,
    rules: [
      rule(0, `${WIKI}/pages/123`, { visited: 3, links: 2 }),
      rule(1, `${WIKI}/archive/123`, { checked: false, unchecked_reason: "link_only", visited: 0, links: 4 }),
    ],
  },
  {
    adapter: { type: "wiki.space", origin: WIKI, match: [{ path: "/space/{id:token}" }], id: "wiki.space:{host}/{id}" },
    scope: "not_shared", tenant: null,
    rules: [rule(0, `${WIKI}/space/a1b2`, { checked: false, unchecked_reason: "token_only" })],
  },
], [
  { source: "visited", shape: "/pages/{num}", url: `${WIKI}/pages/123`, adapter: 0, rule: 0, artifact_id: "wiki.pages:wiki.example.com/123" },
  { source: "visited", shape: "/settings", url: `${WIKI}/settings`, adapter: null, rule: null, artifact_id: null },
  { source: "link", shape: "/archive/{num}", url: `${WIKI}/archive/123`, adapter: 0, rule: 1, artifact_id: "wiki.pages:wiki.example.com/123" },
  { source: "link", shape: "/space/{id}", url: `${WIKI}/space/a1b2`, adapter: 1, rule: 0, artifact_id: "wiki.space:wiki.example.com/a1b2" },
  { source: "link", shape: "/search?q={slug}", url: `${WIKI}/search?q=example`, adapter: null, rule: null, artifact_id: null },
  PROBE,
], ["wiki.taken"]);
const linearDraft = () => drafted([{
  adapter: {
    type: "linear.issue", origin: LINEAR,
    match: [{ path: "/{workspace:name|lower}/issue/{key:jira_key}" }],
    id: "linear.issue:{host}/{workspace}/{key}",
  },
  // The host's illustrative example names a stand-in tenant, never the user's.
  scope: "pending", tenant: { segment: 0, capture: "workspace" },
  rules: [rule(0, `${LINEAR}/example/issue/ABC-123`)],
}]);
const docsDraft = () => drafted([{
  adapter: { type: "docs.document", origin: DOCS, match: [{ path: "/document/d/{doc:token}/edit" }], id: "docs.document:{host}/{doc}" },
  scope: "unscoped_only", tenant: null,
  rules: [rule(0, `${DOCS}/document/d/a1b2/edit`)],
}]);
const githubDraft = () => drafted([{
  adapter: {
    type: "github.issues", origin: GITHUB,
    match: [{ path: "/{owner:name|lower}/issues/{num:int}" }], id: "github.issues:{host}/{owner}/{num}",
  },
  scope: "copied", tenant: { segment: 0, capture: "owner" },
  rules: [rule(0, `${GITHUB}/example/issues/123`)],
}]);
const select = (draft, overrides = {}) => ({
  checked: draft.adapters.map((adapter) => adapter.rules.map((entry) => entry.checked)),
  types: draft.adapters.map(() => null),
  scope: draft.adapters.map(() => null),
  titleSuffix: "",
  ...overrides,
});
const drafting = (draft, selection, url, title = "", config = CONFIG) => planDraftedSite(config, draft, selection, url, title);
const drafts = (...args) => {
  const result = drafting(...args);
  assert.equal(result.ok, true, result.message);
  return result.plan;
};
const added = (plan, config = CONFIG) => plan.document.artifacts.slice(config.adapters.artifacts.length);

test("unknown https origin offers drafted capture", () => {
  for (const url of ["https://wiki.example.com/pages/42", "http://intranet.local:8080/doc/1", "https://linear.app/acme/issue/ABC-1"]) {
    assert.equal(captureMode(CONFIG, url), "draft", url);
    assert.equal(captureOrigin(url), null, url);
  }
  for (const url of ["chrome://newtab", "file:///tmp/x.html", "not a URL", "https://user:secret@wiki.example.com/pages/1"]) {
    assert.equal(captureMode(CONFIG, url), "unsupported", url);
  }
});

test("starter origins keep the starter flow", () => {
  for (const url of [
    "https://acme.atlassian.net/browse/ABC-1", "https://github.com/acme/repo/pull/1",
    "https://acme.sharepoint.com/sites/work/_layouts/15/Doc.aspx?sourcedoc=%7B123e4567-e89b-12d3-a456-426614174000%7D",
  ]) {
    assert.equal(captureMode(CONFIG, url), "starter", url);
    assert.equal(captureOrigin(url), new URL(url).origin);
  }
  // A lookalike or insecure starter host is just an unknown site.
  assert.equal(captureMode(CONFIG, "http://acme.atlassian.net/browse/ABC-1"), "draft");
  assert.equal(captureMode(CONFIG, "https://github.com.evil.example/acme"), "draft");
});

test("configured origins stay managed in settings", () => {
  const artifacts = [
    { type: "wiki.pages", origin: WIKI, match: [{ path: "/pages/{num:int}" }], id: "wiki.pages:{host}/{num}" },
    ...starterOffers("https://acme.atlassian.net", []).map((offer) => offer.adapter),
  ];
  const config = { ...CONFIG, adapters: { format: 1, artifacts } };
  assert.equal(captureMode(config, `${WIKI}/other/1`), "configured");
  assert.equal(captureMode(config, "https://acme.atlassian.net/browse/ABC-1"), "configured");
  assert.equal(captureMode(config, "https://other.example.com/pages/1"), "draft");
});

test("unchecked rules are omitted and an adapter with none is omitted", () => {
  const draft = wikiDraft();
  const plan = drafts(draft, select(draft), `${WIKI}/pages/42`);
  const adapters = added(plan);
  assert.equal(adapters.length, 1);
  assert.deepEqual(adapters[0].match, [{ path: "/pages/{num:int}" }]);
  assert.deepEqual(plan.checked.adapters.map((adapter) => adapter.match), [[{ path: "/pages/{num:int}" }]]);
  assert.deepEqual(plan.checked.provenance, [["drafted"]]);
  assert.deepEqual(plan.kinds.map((kind) => [kind.adapter, kind.type, kind.rules]), [[0, "wiki.pages", [0]]]);
  // Opting into the link-only rule and the loose kind keeps both.
  const all = drafts(draft, select(draft, { checked: [[true, true], [true]] }), `${WIKI}/pages/42`);
  assert.deepEqual(added(all).map((adapter) => adapter.match.length), [2, 1]);
  assert.deepEqual(all.checked.provenance, [["drafted", "drafted"], ["drafted"]]);
  // Nothing checked leaves nothing to save.
  assert.equal(drafting(draft, select(draft, { checked: [[false, false], [false]] }), `${WIKI}/pages/42`).ok, false);
  // A selection that does not line up with the draft is refused.
  assert.equal(drafting(draft, select(draft, { checked: [[true]] }), `${WIKI}/pages/42`).ok, false);
  // The draft is never changed in place.
  assert.deepEqual(draft, wikiDraft());
});

test("pending scope blocks the plan until scope or unscoped is chosen", () => {
  const draft = linearDraft();
  const url = `${LINEAR}/acme/issue/ABC-12`;
  for (const scope of [null, { values: "" }, { values: " , " }, { values: "a/b" }, { values: "a b" }]) {
    const result = drafting(draft, select(draft, { scope: [scope] }), url);
    assert.equal(result.ok, false, JSON.stringify(scope));
    assert.match(result.message, /workspace|account/i);
  }
  const scoped = drafts(draft, select(draft, { scope: [{ values: "Acme, acme-labs, acme" }] }), url);
  assert.deepEqual(added(scoped)[0].scope, { workspace: ["acme", "acme-labs"] });
  assert.equal("unscoped" in added(scoped)[0], false);
  assert.deepEqual(scoped.checked.scope, [{ values: ["acme", "acme-labs"] }]);
  assert.equal(scoped.current.result.match, true);
  const tooMany = Array.from({ length: 65 }, (_, n) => `org${n}`).join(",");
  assert.equal(drafting(draft, select(draft, { scope: [{ values: tooMany }] }), url).ok, false);
  const unscoped = drafts(draft, select(draft, { scope: ["unscoped"] }), url);
  assert.equal(added(unscoped)[0].unscoped, "acknowledged");
  assert.equal("scope" in added(unscoped)[0], false);
  assert.deepEqual(unscoped.checked.scope, ["unscoped"]);
});

test("unscoped-only origins offer only the unscoped statement", () => {
  const draft = docsDraft();
  const url = `${DOCS}/document/d/a1b2c3/edit`;
  assert.equal(drafting(draft, select(draft), url).ok, false);
  assert.equal(drafting(draft, select(draft, { scope: [{ values: "acme" }] }), url).ok, false);
  const plan = drafts(draft, select(draft, { scope: ["unscoped"] }), url);
  assert.equal(added(plan)[0].unscoped, "acknowledged");
  assert.deepEqual(plan.checked.scope, ["unscoped"]);
  // A not-shared origin takes no scope choice at all.
  const wiki = wikiDraft();
  assert.equal(drafting(wiki, select(wiki, { scope: ["unscoped", null] }), `${WIKI}/pages/1`).ok, false);
  assert.equal(drafting(wiki, select(wiki, { scope: [{ values: "acme" }, null] }), `${WIKI}/pages/1`).ok, false);
});

test("scope prefill comes from the current tab's tenant position, never from the host", () => {
  const { tenant, rules } = linearDraft().adapters[0];
  assert.equal(tenantPrefill(`${LINEAR}/Acme-Labs/issue/ABC-12`, tenant), "acme-labs");
  assert.notEqual(tenantPrefill(rules[0].example, tenant), tenantPrefill(`${LINEAR}/acme/issue/ABC-12`, tenant));
  assert.equal(tenantPrefill(`${LINEAR}/acme/issue/ABC-12`, { segment: 4, capture: "workspace" }), "");
  assert.equal(tenantPrefill(`${LINEAR}/acme/issue/ABC-12`, null), "");
  assert.equal(tenantPrefill(`${LINEAR}/a%20b/issue/ABC-12`, tenant), "");
  assert.equal(tenantPrefill("not a URL", tenant), "");
});

test("rename must keep types unique and valid", () => {
  const draft = wikiDraft();
  const both = { checked: [[true, false], [true]] };
  const url = `${WIKI}/pages/42`;
  const config = { ...CONFIG, adapters: { format: 1, artifacts: [
    { type: "other.item", origin: "https://other.example", match: [{ path: "/item/{n:int}" }], id: "other.item:{host}/{n}" },
  ] } };
  for (const bad of ["", "Wiki.Docs", "wiki_docs", "9wiki", "wiki docs", "web.page", "web", "web.docs", "wiki.taken", "other.item", "wiki.space", "a".repeat(33)]) {
    const result = drafting(draft, select(draft, { ...both, types: [bad, null] }), url, "", config);
    assert.equal(result.ok, false, bad);
  }
  // Two kinds renamed to the same type collide too.
  assert.equal(drafting(draft, select(draft, { ...both, types: ["wiki.docs", "wiki.docs"] }), url).ok, false);
  const plan = drafts(draft, select(draft, { ...both, types: [" wiki.docs ", "wiki.pages"] }), url, "", config);
  const adapters = added(plan, config);
  assert.deepEqual(adapters.map((adapter) => [adapter.type, adapter.id]), [
    ["wiki.docs", "wiki.docs:{host}/{num}"], ["wiki.pages", "wiki.pages:{host}/{id}"],
  ]);
  assert.deepEqual(plan.checked.adapters.map((adapter) => adapter.type), ["wiki.docs", "wiki.pages"]);
  assert.equal(plan.current.result.artifact_id, "wiki.docs:wiki.example.com/42");
  // Renaming an omitted kind's type away frees nothing it did not hold.
  assert.equal(drafting(draft, select(draft, { types: ["wiki.space", "wiki.other"] }), url).ok, true);
});

test("title entry builds a valid title template and never reaches draft_site", () => {
  const draft = wikiDraft();
  const url = `${WIKI}/pages/42`;
  const title = "Release plan - Acme Wiki";
  const plan = drafts(draft, select(draft, { titleSuffix: " - Acme Wiki" }), url, title);
  assert.equal(added(plan)[0].title, "{title} - Acme Wiki");
  assert.equal(plan.current.result.title, "Release plan");
  assert.ok(!JSON.stringify(plan.checked).includes("Acme Wiki"));
  assert.ok(!JSON.stringify(plan.checked).includes("Release plan"));
  const request = draftRequest(WIKI, { current: "/pages/{num}", links: [] }, []);
  assert.ok(!JSON.stringify(request).includes("Release"));
  for (const suffix of [" - {Wiki}", " - Wiki}", " - \u0007Wiki", "​ - Wiki\n"]) {
    assert.equal(drafting(draft, select(draft, { titleSuffix: suffix }), url, `Release plan${suffix}`).ok, false, JSON.stringify(suffix));
  }
  // The tab title must actually end with the ending the user typed.
  const missing = drafting(draft, select(draft, { titleSuffix: " - Other" }), url, title);
  assert.equal(missing.ok, false);
  assert.match(missing.message, /title/i);
  assert.equal("title" in added(drafts(draft, select(draft), url, title))[0], false);
});

test("checked tables carry no title, scope or unscoped and the chosen scope travels separately", () => {
  const draft = linearDraft();
  const url = `${LINEAR}/acme/issue/ABC-12`;
  const title = "Fix login - Linear";
  for (const scope of [{ values: "acme" }, "unscoped"]) {
    const plan = drafts(draft, select(draft, { scope: [scope], titleSuffix: " - Linear" }), url, title);
    for (const table of plan.checked.adapters) {
      for (const key of ["title", "scope", "unscoped"]) assert.equal(key in table, false, key);
      assert.deepEqual(Object.keys(table).sort(), ["id", "match", "origin", "type"]);
    }
    assert.deepEqual(Object.keys(plan.checked).sort(), ["adapters", "provenance", "scope"]);
    assert.deepEqual(plan.checked.scope, [scope === "unscoped" ? "unscoped" : { values: ["acme"] }]);
    const saved = added(plan)[0];
    assert.equal(saved.title, "{title} - Linear");
    assert.ok(saved.scope || saved.unscoped);
    // The document's adapter is the checked table plus only the overlays.
    const { title: _t, scope: _s, unscoped: _u, ...tier0 } = saved;
    assert.deepEqual(tier0, plan.checked.adapters[0]);
  }
});

test("a copied scope is applied to the document but sent as null", () => {
  const config = { ...CONFIG, adapters: { format: 1, artifacts: starterOffers(GITHUB, ["acme-corp"]).map((offer) => offer.adapter) } };
  const draft = githubDraft();
  const url = `${GITHUB}/acme-corp/issues/7`;
  const plan = drafts(draft, select(draft), url, "", config);
  assert.deepEqual(added(plan, config)[0].scope, { owner: ["acme-corp"] });
  assert.deepEqual(plan.checked.scope, [null]);
  assert.equal("scope" in plan.checked.adapters[0], false);
  assert.deepEqual(plan.document.artifacts.slice(0, 2), config.adapters.artifacts);
  assert.equal(plan.current.result.match, true);
  // The copied scope is fixed: the review cannot choose another.
  for (const scope of ["unscoped", { values: "other-org" }]) {
    assert.equal(drafting(draft, select(draft, { scope: [scope] }), url, "", config).ok, false);
  }
  // Without an agreeing scoped adapter to copy from, nothing is saved.
  assert.equal(drafting(draft, select(draft), url).ok, false);
});

test("the draft request carries shapes and origin only", () => {
  const patterns = {
    current: "/pages/{num}",
    links: [
      { shape: "/archive/{num}", count: 4 }, { shape: "/search?q={slug}", count: 1 },
      { shape: "https://wiki.example.com/archive/9", count: 1 }, { shape: "/pages/{num}", count: 0 },
    ],
  };
  const samples = [
    { shape: "/space/{id}", count: 2, last_seen: 1_700_000_000_000 },
    { shape: "/pages/{num}", count: 5, last_seen: 1_700_000_000_001 },
    { shape: "/raw/42?token=abc", count: 1, last_seen: 1_700_000_000_002 },
  ];
  const request = draftRequest(WIKI, patterns, samples);
  assert.deepEqual(Object.keys(request).sort(), ["links", "origin", "visited"]);
  assert.equal(request.origin, WIKI);
  assert.deepEqual(request.visited, [{ shape: "/pages/{num}", count: 5 }, { shape: "/space/{id}", count: 2 }]);
  assert.deepEqual(request.links, [{ shape: "/archive/{num}", count: 4 }, { shape: "/search?q={slug}", count: 1 }]);
  const shapes = JSON.stringify({ visited: request.visited, links: request.links });
  assert.ok(!shapes.includes("http"));
  assert.ok(!shapes.includes("last_seen"));
  // A current page the worker never sampled counts once.
  assert.deepEqual(draftRequest(WIKI, { current: "/pages/{num}", links: [] }, []).visited, [{ shape: "/pages/{num}", count: 1 }]);
  assert.deepEqual(draftRequest(WIKI, { current: null, links: [] }, []).visited, []);
});

test("ConfigSession sends draft_site and check_site_draft with only their schema fields", async () => {
  const posted = [];
  const previous = globalThis.chrome;
  globalThis.chrome = {
    runtime: {
      connectNative() {
        const listeners = [];
        return {
          onMessage: { addListener(listener) { listeners.push(listener); } },
          onDisconnect: { addListener() {} },
          postMessage(request) {
            posted.push(structuredClone(request));
            const reply = request.type === "draft_site"
              ? { ...wikiDraft(), request_id: request.request_id }
              : { type: "site_draft_checked", protocol_version: 6, request_id: request.request_id, revision: "7", rules: [[{ verdict: "kept", reason: null }]], scope: [{ state: "not_shared", resolved: "not_shared", reason: null }], preview: [] };
            setTimeout(() => { for (const listener of listeners) listener(reply); }, 0);
          },
          disconnect() {},
        };
      },
    },
  };
  try {
    const { ConfigSession } = await import("../dist/settings.js");
    const session = new ConfigSession(() => undefined);
    const request = draftRequest(WIKI, { current: "/pages/{num}", links: [{ shape: "/archive/{num}", count: 4 }] }, []);
    const draft = await session.draftSite(request.origin, request.visited, request.links);
    assert.equal(draft.type, "site_drafted");
    const plan = drafts(draft, select(draft), `${WIKI}/pages/42?secret=1`, "Secret title");
    const checked = await session.checkSiteDraft(WIKI, draft.revision, plan.checked, request.visited, request.links);
    assert.equal(checked.type, "site_draft_checked");
    assert.deepEqual(posted.map((message) => Object.keys(message).sort()), [
      ["links", "origin", "protocol_version", "request_id", "type", "visited"],
      ["adapters", "expected_revision", "links", "origin", "protocol_version", "provenance", "request_id", "scope", "type", "visited"],
    ]);
    assert.equal(posted[1].expected_revision, "7");
    assert.deepEqual(posted[1].adapters, plan.checked.adapters);
    const wire = JSON.stringify(posted);
    for (const secret of ["secret", "Secret title", "/pages/42"]) assert.ok(!wire.includes(secret), secret);
    session.host.disconnect();
  } finally {
    globalThis.chrome = previous;
  }
});

test("the real current URL is evaluated locally", () => {
  const draft = wikiDraft();
  const url = `${WIKI}/pages/42?ref=mail#top`;
  const plan = drafts(draft, select(draft), url, "Release plan");
  assert.equal(plan.origin, WIKI);
  assert.equal(plan.current.url, url);
  assert.equal(plan.current.probe, false);
  assert.equal(plan.current.result.artifact_id, "wiki.pages:wiki.example.com/42");
  const miss = drafts(draft, select(draft), `${WIKI}/settings`);
  assert.equal(miss.current.result.match, false);
  assert.equal(plan.probes.length, 27);
  assert.ok(plan.probes.every((row) => row.probe && !row.result.match));
  assert.ok(plan.examples.length >= 1 && plan.examples.every((row) => !row.probe && row.result.match));
  assert.equal(plan.startsCapture, true);
  // Counts come from the host's preview of shapes, scoped to the selection.
  assert.deepEqual(plan.counts, { visited: 1, visitedTotal: 2, links: 0, linksTotal: 3 });
  const all = drafts(draft, select(draft, { checked: [[true, true], [true]] }), url);
  assert.deepEqual(all.counts, { visited: 1, visitedTotal: 2, links: 2, linksTotal: 3 });
  // Shared-origin examples show the chosen tenant, so they match the saved scope.
  const linear = linearDraft();
  const scoped = drafts(linear, select(linear, { scope: [{ values: "acme" }] }), `${LINEAR}/acme/issue/ABC-12`);
  assert.ok(scoped.examples.length === 1 && scoped.examples[0].result.match);
  assert.equal(scoped.examples[0].url, `${LINEAR}/acme/issue/ABC-123`);
  // No home folder, or a draft for another origin, cannot produce a save.
  assert.equal(drafting(draft, select(draft), url, "", { ...CONFIG, storage: null }).ok, false);
  assert.equal(drafting(draft, select(draft), "https://other.example.com/pages/42").ok, false);
});
