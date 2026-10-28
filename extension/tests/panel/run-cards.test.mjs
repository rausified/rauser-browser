// Built side-panel tests: run cards. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  ARTICLE_URL, RUN_VIEW, WARNING_SEEN_KEY, isResponseShape, summarizePanel,
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
