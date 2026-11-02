// Built side-panel tests: run cards. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  ARTICLE_URL, RUN_VIEW, WARNING_SEEN_KEY, descendants, isResponseShape, summarizePanel,
  until,
} from "./harness.mjs";

test("run cards follow the page: another tab's card hides and comes back", async () => {
  const page = await summarizePanel({ local: new Map([[WARNING_SEEN_KEY, true]]) });
  await until(() => page.el("summarize").disabled === false, "the button to enable");
  page.el("summarize").click();
  const card = () => page.el("run-area").children.find((child) => child.className.split(" ").includes("run-card"));
  await until(() => card() && /Readable title/.test(card().textContent), "the run card");
  assert.equal(card().hidden, false);

  page.navigate("https://other.example/b", 2);
  await until(() => page.el("page-url").textContent === "https://other.example/b", "tab 2 to show");
  assert.equal(card().hidden, true);

  page.navigate(ARTICLE_URL, 1);
  await until(() => page.el("page-url").textContent === ARTICLE_URL, "tab 1 to show");
  assert.equal(card().hidden, false);
});

for (const [scope, label] of [
  ["whole", "Agent read: whole artifact"],
  ["partial", "Agent read: part of the artifact"],
  ["page_only", "Agent read: browser page only"],
  [undefined, null],
]) {
  test(`a built summary card discloses read scope: ${scope ?? "legacy"}`, async () => {
    const overview = '<img src="https://remote.example/pixel"> Release decision.';
    const answer = {
      overview, document_type: "report", category: "news", importance: "high",
      importance_reason: "Affects the release.", ...(scope ? { read_scope: scope } : {}),
    };
    const run = {
      ...RUN_VIEW, state: "succeeded", answer,
      created_at: new Date().toISOString(), finished_at: new Date().toISOString(),
    };
    const page = await summarizePanel({ intercept: (request) => {
      if (request.type === "list_runs") return { type: "runs_listed", runs: [run], more: false };
      if (request.type === "get_run") return { type: "run_fetched", run, stream_text: "", next_offset: 0, more: false };
      return null;
    } });
    await until(() => page.el("run-area").textContent.includes(overview), "the summary answer");
    const area = page.el("run-area");
    if (label) assert.ok(area.textContent.includes(`${overview}\n${label}`), area.textContent);
    else assert.doesNotMatch(area.textContent, /Agent read:/);
    assert.ok(area.textContent.includes("Type: report"));
    assert.equal(descendants(area).filter((el) => el.tagName === "IMG").length, 0);
    assert.equal(page.sent("start_run").length, 0);
  });
}

test("a finished summary restores after more than a list window of newer tasks or site drafts", async () => {
  for (const kind of ["task", "site_draft"]) {
    const summary = {
      ...RUN_VIEW, state: "succeeded", kind: "summary",
      created_at: new Date(Date.now() - 60_000).toISOString(),
      finished_at: new Date(Date.now() - 30_000).toISOString(),
    };
    const newer = Array.from({ length: 21 }, (_, i) => ({
      ...summary, run_id: `00000000-0000-4000-8000-${String(i + 100).padStart(12, "0")}`,
      kind, title: "Newer run", created_at: new Date().toISOString(),
      ...(kind === "task" ? { instruction: "Newer task" } : {}),
    }));
    const mixed = [...newer, summary];
    const page = await summarizePanel({ intercept: (request) => {
      if (request.type === "list_runs") {
        const runs = (request.kind ? mixed.filter((run) => run.kind === request.kind) : mixed).slice(0, 20);
        return { type: "runs_listed", runs, more: false };
      }
      if (request.type === "get_run") {
        return { type: "run_fetched", run: mixed.find((run) => run.run_id === request.run_id), stream_text: "", next_offset: 0, more: false };
      }
      return null;
    } });
    await until(() => /Readable title/.test(page.el("run-area").textContent), "the older summary card");
    const cards = page.el("run-area").children.filter((child) => child.className.split(" ").includes("run-card"));
    assert.equal(cards.length, 1);
    assert.equal(cards[0].hidden, false);
    assert.equal(page.sent("list_runs").filter((request) => request.kind === "summary").length, 1);
    assert.equal(page.sent("start_run").length, 0);
    assert.doesNotMatch(page.el("run-area").textContent, /Newer run|Newer task/);
  }
});

test("run views with kind and site_draft pass the response shape check, and unknown kinds fail", () => {
  const table = { type: "work.doc", origin: "https://work.example", match: [{ path: "/docs/{num:int}" }], id: "work.doc:{host}/{num}" };
  const fetched = (run) => ({ type: "run_fetched", run, stream_text: "", next_offset: 0, more: false });
  // An older service's view has neither key.
  assert.equal(isResponseShape(fetched(RUN_VIEW)), true);
  for (const kind of ["summary", "site_draft"]) {
    assert.equal(isResponseShape(fetched({ ...RUN_VIEW, kind })), true, kind);
  }
  assert.equal(isResponseShape(fetched({ ...RUN_VIEW, kind: "site_draft", site_draft: null })), true);
  assert.equal(isResponseShape(fetched({ ...RUN_VIEW, site_draft: null })), true);
  for (const count of [1, 8]) {
    const site_draft = { adapters: Array.from({ length: count }, () => table) };
    assert.equal(isResponseShape(fetched({ ...RUN_VIEW, kind: "site_draft", site_draft })), true, String(count));
  }
  assert.equal(isResponseShape({ type: "runs_listed", runs: [{ ...RUN_VIEW, kind: "site_draft", site_draft: null }] }), true);
  const bad = [
    { ...RUN_VIEW, kind: "chat" },
    { ...RUN_VIEW, kind: null },
    { ...RUN_VIEW, kind: 1 },
    { ...RUN_VIEW, site_draft: { adapters: [] } },
    { ...RUN_VIEW, site_draft: { adapters: Array.from({ length: 9 }, () => table) } },
    { ...RUN_VIEW, site_draft: { adapters: [[]] } },
    { ...RUN_VIEW, site_draft: { adapters: ["/docs/{num:int}"] } },
    { ...RUN_VIEW, site_draft: { adapters: [table], answer: "raw" } },
    { ...RUN_VIEW, site_draft: [] },
    { ...RUN_VIEW, site_draft: "raw" },
    { ...RUN_VIEW, extra: 1 },
  ];
  for (const run of bad) {
    assert.equal(isResponseShape(fetched(run)), false, JSON.stringify(run));
  }
});
