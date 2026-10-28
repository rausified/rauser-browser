// Built side-panel tests: capture site agent. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  JIRA_URL, LINEAR_URL, NOTES_URL, READY, RUN_VIEW, SET_UP, WIKI, WIKI_URL,
  buttonIn, captureReview, checks, descendants, draftReview, draftRule,
  draftText, drafted, draftedSave, indexOf, isResponseShape, linearDraft,
  recurringReview, ruleBox, sampleRow, saveCapture, sentAfter, settle,
  siteChecked, siteDrafted, until, updates, wikiDraft,
} from "./harness.mjs";

test("site_drafted accepts the agent drafter and the unseen reason", () => {
  const draft = siteDrafted();
  draft.drafter = "agent";
  draft.adapters[0].rules[0].unchecked_reason = "unseen";
  assert.equal(isResponseShape(draft), true);
});

// Improve with AI (DESIGN §12.2 step 7.7): the structural review first, then
// an agent draft the host judged, on request.
const NEEDS_HARNESS = "Set up a summary harness in settings first.";

const SITE_RUN_ID = "00000000-0000-4000-8000-000000000077";

const SITE_RUN = {
  ...RUN_VIEW, run_id: SITE_RUN_ID, state: "running", harness: "codex",
  record_key: { kind: "url", value: WIKI }, url: WIKI, title: "Capture Site: wiki.example.com",
  started_at: "2026-10-27T09:00:01-07:00", kind: "site_draft", site_draft: null,
};

const AGENT_TABLE = {
  type: "wiki.pages", origin: WIKI,
  match: [{ path: "/pages/{num:int}" }, { path: "/docs/{num:int}" }],
  id: "wiki.pages:{host}/{num}",
};

const siteRun = (fields = {}) => ({ ...SITE_RUN, ...fields });

const succeededRun = (fields = {}) => siteRun({
  state: "succeeded", finished_at: "2026-10-27T09:00:41-07:00", site_draft: { adapters: [AGENT_TABLE] }, ...fields,
});

function agentDraft(revision = "1") {
  return {
    type: "site_drafted", revision, drafter: "agent",
    adapters: [{
      adapter: AGENT_TABLE, scope: "not_shared", tenant: null,
      rules: [
        draftRule(0, `${WIKI}/pages/123`, { visited: 3, links: 2 }),
        draftRule(1, `${WIKI}/docs/123`, { checked: false, unchecked_reason: "unseen", visited: 0, links: 0 }),
      ],
    }],
    dropped: [{ shape: null, adapter: 0, rule: 2, reason: "draft_search_capture" }],
    preview: [
      { source: "visited", shape: "/pages/{num}", url: `${WIKI}/pages/123`, adapter: 0, rule: 0, artifact_id: "wiki.pages:wiki.example.com/123" },
      { source: "link", shape: "/archive/{num}", url: `${WIKI}/archive/123`, adapter: null, rule: null, artifact_id: null },
    ],
    taken_types: [], remapped: 0,
  };
}

const rejectedDraft = (reason) => ({
  type: "site_drafted", revision: "1", drafter: "agent", adapters: [],
  dropped: [{ shape: null, adapter: null, rule: null, reason }], preview: [], taken_types: [], remapped: 0,
});

// A drafted review with a ready harness whose fake host also runs site
// drafts. `start` answers start_site_draft, `fetch` get_run and `judge` the
// draft_site that carries a run_id; each is a function of the request and
// state. Cancel answers a canceled run, and later fetches see it canceled.
async function agentReview(url = WIKI_URL, options = {}) {
  const {
    start = () => ({ type: "run_started", run: siteRun(), existing: false }),
    fetch = () => ({ type: "run_fetched", run: succeededRun(), stream_text: "", next_offset: 0, more: false }),
    judge = () => agentDraft(), draft = () => wikiDraft(), scopes, intercept, ...rest
  } = options;
  const page = await draftReview(url, {
    agentStatus: READY, config: SET_UP, ...rest,
    draft: (request, state) => request.run_id === undefined ? draft(request, state) : judge(request, state),
    intercept: (request, state) => {
      if (request.type === "start_site_draft") return start(request, state);
      if (request.type === "get_run") {
        if (state.canceledRun === request.run_id) {
          return { type: "run_fetched", run: siteRun({ run_id: request.run_id, state: "canceled", finished_at: "2026-10-27T09:00:09-07:00", error_kind: "canceled" }), stream_text: "", next_offset: request.since_offset, more: false };
        }
        return fetch(request, state);
      }
      if (request.type === "cancel_run") {
        state.canceledRun = request.run_id;
        return { type: "run_updated", run: siteRun({ run_id: request.run_id, state: "canceled", finished_at: "2026-10-27T09:00:09-07:00", error_kind: "canceled" }) };
      }
      if (request.type === "check_site_draft") return siteChecked(request, state, scopes);
      return intercept?.(request, state);
    },
  });
  await drafted(page);
  return page;
}

const agentRunText = (page) => page.el("capture-site-agent-run").textContent;

const judged = (page) => page.sent("draft_site").filter((request) => request.run_id !== undefined);

const improved = (page) => until(() => /Improved with AI/.test(page.el("capture-site-drafter").textContent), "the AI draft");

const notUsed = (page) => until(() => /The AI draft was not used/.test(agentRunText(page)), "the not-used line");

test("Improve with AI is disabled with the Summarize setup hint when no harness is ready", async () => {
  const page = await draftReview();
  await drafted(page);
  const improve = page.el("capture-site-improve");
  assert.equal(improve.hidden, false);
  assert.equal(improve.textContent, "Improve with AI");
  assert.equal(improve.disabled, true);
  assert.equal(improve.title, NEEDS_HARNESS);
  assert.equal(page.el("capture-site-improve-hint").hidden, false);
  assert.equal(page.el("capture-site-improve-hint").textContent, NEEDS_HARNESS);
  assert.equal(page.el("capture-site-basic").hidden, true);
  improve.disabled = false;
  improve.click();
  await settle(20);
  assert.equal(page.sent("start_site_draft").length, 0);
  // With a harness ready, the hint goes.
  const ready = await agentReview(WIKI_URL, { start: () => new Promise(() => {}) });
  assert.equal(ready.el("capture-site-improve").disabled, false);
  assert.equal(ready.el("capture-site-improve").title, "");
  assert.equal(ready.el("capture-site-improve-hint").hidden, true);
  assert.equal(ready.el("capture-site-drafter").textContent, "Drafted from this site's page patterns");
  // A starter review has no AI draft to improve.
  const starter = await captureReview(JIRA_URL, { agentStatus: READY, config: SET_UP });
  await until(() => !starter.el("capture-site-save").disabled, "the starter preview");
  assert.equal(starter.el("capture-site-improve").hidden, true);
  assert.equal(starter.el("capture-site-drafter").hidden, true);
});

test("Capture Site still drafts and saves with no harness and no service", async () => {
  const unavailable = { type: "error", code: "service_unavailable", message: "The Brauser service is not running." };
  const page = await draftedSave(WIKI_URL, {
    intercept: (request) => ["start_site_draft", "get_run", "list_runs", "cancel_run"].includes(request.type) ? unavailable : null,
  });
  assert.equal(page.el("capture-site-improve").disabled, true);
  await saveCapture(page);
  assert.equal(checks(page).length, 1);
  assert.equal(updates(page).length, 1);
  for (const type of ["start_site_draft", "get_run", "cancel_run"]) assert.equal(page.sent(type).length, 0, type);
});

test("Improve with AI sends start_site_draft with the held shapes and reads nothing new from the page", async () => {
  const page = await agentReview(WIKI_URL, { start: () => new Promise(() => {}) });
  const scripts = page.state.scripts.length;
  const samples = page.state.workerRequests.filter((message) => message.kind === "get_site_patterns").length;
  const from = page.state.requests.length;
  page.el("capture-site-improve").click();
  await until(() => page.sent("start_site_draft").length === 1, "start_site_draft");
  const [structural] = page.sent("draft_site");
  const [request] = page.sent("start_site_draft");
  assert.deepEqual(Object.keys(request).sort(),
    ["harness", "links", "origin", "protocol_version", "request_id", "requested_at", "type", "visited"]);
  assert.equal(request.origin, WIKI);
  assert.equal(request.harness, null);
  assert.deepEqual(request.visited, structural.visited);
  assert.deepEqual(request.links, structural.links);
  assert.doesNotMatch(JSON.stringify({ ...request, origin: null, request_id: null, requested_at: null }), /http|Work page|42/);
  assert.equal(page.state.scripts.length, scripts, "no page is read again");
  assert.equal(page.state.workerRequests.filter((message) => message.kind === "get_site_patterns").length, samples);
  assert.deepEqual(sentAfter(page, from), ["start_site_draft"]);
  // While it starts, the button waits.
  assert.equal(page.el("capture-site-improve").disabled, true);
});

test("the run progress shows state, streamed text as text, and Cancel", async () => {
  const page = await agentReview(WIKI_URL, {
    fetch: (request) => ({ type: "run_fetched", run: siteRun(), stream_text: request.since_offset ? "" : "<b>Reading the shapes</b>", next_offset: 26, more: false }),
  });
  page.el("capture-site-improve").click();
  await until(() => agentRunText(page).includes("Reading the shapes"), "the streamed text");
  assert.match(agentRunText(page), /Running/);
  assert.match(agentRunText(page), /<b>Reading the shapes<\/b>/, "agent text is shown as text");
  assert.equal(page.el("capture-site-improve").disabled, true, "one AI draft at a time");
  const cancel = buttonIn(page.el("capture-site-agent-run"), "Cancel");
  assert.ok(cancel);
  cancel.click();
  await until(() => page.sent("cancel_run").length === 1, "cancel_run");
  assert.equal(page.sent("cancel_run")[0].run_id, SITE_RUN_ID);
  await notUsed(page);
  assert.match(agentRunText(page), /The AI draft was not used: it was canceled/);
  assert.equal(page.el("capture-site-review").hidden, false, "the basic review stays");
  assert.equal(page.el("capture-site-improve").disabled, false, "it can be tried again");
});

test("a succeeded run re-renders the review from draft_site with run_id and marks it Improved with AI", async () => {
  const page = await agentReview();
  page.el("capture-site-improve").click();
  await improved(page);
  const [structural] = page.sent("draft_site");
  const [request] = judged(page);
  assert.equal(request.run_id, SITE_RUN_ID);
  assert.equal(request.origin, WIKI);
  assert.deepEqual(request.visited, structural.visited);
  assert.deepEqual(request.links, structural.links);
  assert.equal(page.el("capture-site-drafter").textContent, "Improved with AI (Codex), checked by Brauser");
  assert.match(draftText(page), /Pages like \/docs\/<number>/);
  assert.doesNotMatch(draftText(page), /wiki\.space/, "the agent draft replaced the basic one");
  assert.match(page.el("capture-site-dropped").textContent, /A drafted rule: /);
  assert.equal(page.el("capture-site-basic").hidden, false);
  assert.equal(page.el("capture-site-improve").hidden, true);
  assert.equal(page.el("capture-site-agent-run").children.length, 0, "the finished progress goes");
  assert.equal(page.el("capture-site-save").disabled, false);
  assert.equal(updates(page).length, 0);
  // A Claude Code run names its harness.
  const claude = await agentReview(WIKI_URL, {
    fetch: () => ({ type: "run_fetched", run: succeededRun({ harness: "claude" }), stream_text: "", next_offset: 0, more: false }),
  });
  claude.el("capture-site-improve").click();
  await improved(claude);
  assert.equal(claude.el("capture-site-drafter").textContent, "Improved with AI (Claude Code), checked by Brauser");
});

test("an agent drop row never names a kind by its answer index", async () => {
  // The agent's table 0 (/search/{n}) was refused whole and table 1 kept:
  // the host's adapters hold only table 1, and the drop names table 0.
  const page = await agentReview(WIKI_URL, {
    judge: () => ({
      ...agentDraft(),
      dropped: [{ shape: null, adapter: 0, rule: null, reason: "draft_search_capture" }],
    }),
  });
  page.el("capture-site-improve").click();
  await improved(page);
  const dropped = page.el("capture-site-dropped").textContent;
  assert.doesNotMatch(dropped, /wiki\.pages/, "the surviving kind is not blamed");
  assert.match(dropped, /A drafted rule: /);
});

test("the review renders only the host's site_drafted, never the run's raw answer", async () => {
  const raw = { ...AGENT_TABLE, match: [{ path: "/leakyroute/{num:int}" }], title: "{title} RAWTITLE", scope: { space: ["RAWSCOPE"] } };
  const page = await agentReview(WIKI_URL, {
    fetch: () => ({ type: "run_fetched", run: succeededRun({ site_draft: { adapters: [raw] } }), stream_text: "", next_offset: 0, more: false }),
    judge: () => rejectedDraft("draft_forbidden_key"),
  });
  page.el("capture-site-improve").click();
  await notUsed(page);
  assert.match(agentRunText(page), /The AI draft was not used: it tried to set fields drafts may not set \(titles, scope or page fields\)/);
  const shown = [draftText(page), agentRunText(page), page.el("capture-site-drafter").textContent].join("\n");
  assert.doesNotMatch(shown, /leakyroute|RAWTITLE|RAWSCOPE/);
  assert.match(draftText(page), /Brauser will remember: wiki\.pages — pages like \/pages\/<number>/);
  assert.equal(page.el("capture-site-drafter").textContent, "Drafted from this site's page patterns");
  assert.equal(page.el("capture-site-basic").hidden, true);
});

test("unseen AI rules start unchecked with their label", async () => {
  const page = await agentReview();
  page.el("capture-site-improve").click();
  await improved(page);
  assert.equal(ruleBox(page, 0, 0).checked, true);
  assert.equal(ruleBox(page, 0, 1).checked, false);
  const rows = descendants(page.el("capture-site-draft")).filter((node) => node.tagName === "LABEL");
  const labelOf = (rule) => rows.find((row) => row.children.includes(ruleBox(page, 0, rule))).textContent;
  assert.match(labelOf(1), /suggested by AI: no page you visited or linked here matches it/);
  assert.doesNotMatch(labelOf(0), /suggested by AI/);
});

test("Use basic draft returns to the structural draft", async () => {
  const page = await agentReview();
  page.el("capture-site-improve").click();
  await improved(page);
  page.el("capture-site-basic").click();
  assert.equal(page.el("capture-site-drafter").textContent, "Drafted from this site's page patterns");
  assert.match(draftText(page), /wiki\.space/);
  assert.doesNotMatch(draftText(page), /\/docs\/<number>/);
  assert.equal(page.el("capture-site-basic").hidden, true);
  assert.equal(page.el("capture-site-improve").hidden, false);
  assert.equal(ruleBox(page, 0, 1).checked, false, "the basic draft's defaults return");
  await saveCapture(page);
  assert.deepEqual(checks(page)[0].adapters, [{ ...wikiDraft().adapters[0].adapter, match: [{ path: "/pages/{num:int}" }] }]);
});

test("a failed, canceled or rejected AI draft keeps the basic draft and says why", async () => {
  const cases = [
    [{ fetch: () => ({ type: "run_fetched", run: siteRun({ state: "failed", finished_at: "2026-10-27T09:00:41-07:00", error_kind: "invalid_output", error: "the agent's answer was not a usable draft" }), stream_text: "", next_offset: 0, more: false }) },
      /The AI draft was not used: the agent's answer was not a usable draft/],
    [{ fetch: () => ({ type: "run_fetched", run: siteRun({ state: "canceled", finished_at: "2026-10-27T09:00:41-07:00", error_kind: "canceled" }), stream_text: "", next_offset: 0, more: false }) },
      /The AI draft was not used: it was canceled/],
    [{ judge: () => ({ ...rejectedDraft("draft_search_capture"), dropped: [{ shape: null, adapter: null, rule: null, reason: "draft_search_capture" }] }) },
      /The AI draft was not used: none of its rules passed Brauser's safety checks/],
    [{ judge: () => rejectedDraft("draft_invalid_rule") },
      /The AI draft was not used: none of its rules passed Brauser's safety checks/],
    [{ judge: () => ({ type: "error", code: "invalid_request", message: "run is not a finished site draft" }) },
      /The AI draft was not used: run is not a finished site draft/],
    [{ start: () => ({ type: "error", code: "service_unavailable", message: "The Brauser service is not running." }) },
      /The AI draft was not used: .*The Brauser service is not running\./],
  ];
  for (const [options, reason] of cases) {
    const page = await agentReview(WIKI_URL, options);
    page.el("capture-site-improve").click();
    await notUsed(page);
    assert.match(agentRunText(page), reason);
    assert.equal(page.el("capture-site-drafter").textContent, "Drafted from this site's page patterns");
    assert.match(draftText(page), /Brauser will remember: wiki\.pages — pages like \/pages\/<number>/);
    assert.match(draftText(page), /wiki\.space/);
    assert.equal(page.el("capture-site-basic").hidden, true);
    assert.equal(page.el("capture-site-save").disabled, false, "the basic draft stays savable");
    assert.doesNotMatch(agentRunText(page), /draft_|invalid_output/);
  }
});

test("a stale revision after the AI run refuses as the structural flow does", async () => {
  const page = await agentReview(WIKI_URL, { judge: () => agentDraft("2") });
  page.el("capture-site-improve").click();
  await until(() => page.el("capture-site-message").textContent.includes("Settings changed"), "the stale refusal");
  assert.equal(page.el("capture-site-save").disabled, true);
  assert.doesNotMatch(draftText(page), /\/docs\/<number>/);
  assert.equal(page.el("capture-site-improve").disabled, true);
});

test("closing the review cancels an unfinished AI run and discards its shapes", async () => {
  const running = { fetch: (request) => ({ type: "run_fetched", run: siteRun({ run_id: request.run_id }), stream_text: "", next_offset: 0, more: false }) };
  for (const close of [
    (page) => page.el("capture-site-cancel").click(),
    (page) => page.navigate("https://other.example/pages/1", 2),
    (page) => page.tabUpdated({ status: "loading" }),
  ]) {
    const page = await agentReview(WIKI_URL, running);
    page.el("capture-site-improve").click();
    await until(() => /Running/.test(agentRunText(page)), "the running AI draft");
    close(page);
    await until(() => page.sent("cancel_run").length === 1, "cancel_run");
    assert.equal(page.sent("cancel_run")[0].run_id, SITE_RUN_ID);
    await until(() => page.el("capture-site-review").hidden, "the review to close");
    assert.equal(page.el("capture-site-agent-run").children.length, 0);
    assert.doesNotMatch(draftText(page), /archive|space/);
    await settle(20);
    assert.equal(judged(page).length, 0);
  }
  // A run that starts after the review closed is canceled when it answers.
  let answer;
  const held = new Promise((resolve) => { answer = resolve; });
  const late = await agentReview(WIKI_URL, { ...running, start: () => held });
  late.el("capture-site-improve").click();
  await until(() => late.sent("start_site_draft").length === 1, "start_site_draft");
  late.el("capture-site-cancel").click();
  answer({ type: "run_started", run: siteRun(), existing: false });
  await until(() => late.sent("cancel_run").length === 1, "the late cancel");
  assert.equal(late.el("capture-site-agent-run").children.length, 0);
  // A finished run is not canceled by closing the review.
  const done = await agentReview(WIKI_URL, { judge: () => new Promise(() => {}) });
  done.el("capture-site-improve").click();
  await until(() => judged(done).length === 1, "the judged draft");
  done.el("capture-site-cancel").click();
  await settle(20);
  assert.equal(done.sent("cancel_run").length, 0);
});

test("reopening Capture Site on the same origin reattaches to the running AI draft", async () => {
  const existingId = "00000000-0000-4000-8000-000000000078";
  const page = await agentReview(WIKI_URL, {
    start: () => ({ type: "run_started", run: siteRun({ run_id: existingId, stream_bytes: 40 }), existing: true }),
    fetch: (request) => ({ type: "run_fetched", run: succeededRun({ run_id: request.run_id }), stream_text: "", next_offset: 0, more: false }),
  });
  page.el("capture-site-improve").click();
  await improved(page);
  assert.equal(page.sent("start_site_draft").length, 1);
  assert.equal(page.sent("get_run")[0].run_id, existingId);
  assert.equal(judged(page)[0].run_id, existingId);
});

test("saving an AI draft goes through check_site_draft and update_config exactly like a basic one", async () => {
  const page = await agentReview();
  page.el("capture-site-improve").click();
  await improved(page);
  await saveCapture(page);
  assert.equal(checks(page).length, 1);
  const [check] = checks(page);
  const [structural] = page.sent("draft_site");
  assert.equal("drafter" in check, false);
  assert.equal("run_id" in check, false);
  assert.deepEqual(check.adapters, [{ ...AGENT_TABLE, match: [{ path: "/pages/{num:int}" }] }]);
  assert.deepEqual(check.provenance, [["drafted"]]);
  assert.deepEqual(check.visited, structural.visited);
  assert.deepEqual(check.links, structural.links);
  assert.ok(indexOf(page, "check_site_draft") < indexOf(page, "update_config"));
  const [update] = updates(page);
  assert.equal(update.expected_revision, "1");
  assert.deepEqual(update.config.adapters.artifacts, check.adapters);
  assert.match(page.el("status").textContent, /Saved capture rules for wiki\.example\.com/);
});

test("a shared origin AI draft pre-fills this tab's tenant and saves with one click", async () => {
  const agentLinear = () => {
    const draft = linearDraft();
    draft.drafter = "agent";
    draft.adapters[0].adapter = { ...draft.adapters[0].adapter, match: [...draft.adapters[0].adapter.match, { path: "/{workspace:name|lower}/project/{num:int}" }] };
    draft.adapters[0].rules.push(draftRule(1, "https://linear.app/example/project/123", { checked: false, unchecked_reason: "unseen", visited: 0 }));
    return draft;
  };
  const page = await agentReview(LINEAR_URL, {
    draft: () => linearDraft(), judge: agentLinear, scopes: ["pending"],
    script: async () => ({ current: "/{slug}/issue/{key}", links: [] }),
  });
  page.el("capture-site-improve").click();
  await improved(page);
  assert.equal(page.el("capture-site-scope-values").value, "acme");
  assert.equal(page.el("capture-site-scoped").checked, true);
  assert.equal(page.el("capture-site-save").disabled, false);
  await saveCapture(page);
  const [check] = checks(page);
  assert.deepEqual(check.scope, [{ values: ["acme"] }]);
  const saved = updates(page)[0].config.adapters.artifacts;
  assert.deepEqual(saved[0].scope, { workspace: ["acme"] });
  assert.deepEqual(saved[0].match, [{ path: "/{workspace:name|lower}/issue/{key:jira_key}" }]);
});

test("an older host without start_site_draft asks for a host rebuild", async () => {
  const unsupported = () => ({ type: "error", code: "invalid_request", message: "invalid or unsupported request JSON" });
  for (const options of [{ start: unsupported }, { judge: unsupported }]) {
    const page = await agentReview(WIKI_URL, options);
    page.el("capture-site-improve").click();
    await until(() => /rebuild/i.test(agentRunText(page)), "the rebuild message");
    assert.match(agentRunText(page), /Rebuild and reinstall the Brauser host/);
    assert.match(draftText(page), /wiki\.space/, "the basic draft stays");
    assert.equal(page.el("capture-site-save").disabled, false);
  }
});

test("the recurring-pages review offers Improve with AI too", async () => {
  const page = await recurringReview(NOTES_URL, { patterns: [sampleRow("/notes/{num}", 3)], agentStatus: READY });
  assert.equal(page.el("capture-site-improve").hidden, false);
  assert.equal(page.el("capture-site-improve").disabled, false);
  assert.equal(page.el("capture-site-drafter").textContent, "Drafted from this site's page patterns");
});
