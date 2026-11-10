// Built panel: selection capture receipts are plain text, independent of runs.
import assert from "node:assert/strict";
import test from "node:test";
import { descendants, isResponseShape, summarizePanel, until } from "./harness.mjs";

for (const failed of [false, true]) {
  test(`evidence ${failed ? "failure" : "success"} appears without a harness warning or agent run`, async () => {
    const page = await summarizePanel();
    await until(() => page.el("page-url").textContent !== "", "the current page");
    const message = failed ? "Could not keep the passage: <img src=x>. Repeat the same capture to retry safely."
      : `Passage saved: notes/evidence/${"a".repeat(64)}.md`;
    page.notify({ kind: "evidence_result", message, failed, tab_id: 1, at: Date.now() });
    await until(() => page.el("run-area").textContent.includes(message), "the evidence receipt");
    const line = descendants(page.el("run-area")).find((el) => el.className.includes("run-line"));
    assert.equal(line.textContent, message);
    assert.equal(line.className.includes("warning"), failed);
    assert.equal(descendants(page.el("run-area")).filter((el) => el.tagName === "IMG").length, 0);
    assert.doesNotMatch(page.el("run-area").textContent, /full access|first summary/i);
    assert.equal(page.sent("start_run").length, 0);
    assert.equal(page.sent("start_task").length, 0);
    page.notify({ kind: "evidence_result", message: "other tab", failed, tab_id: 2, at: Date.now() });
    assert.ok(!page.el("run-area").textContent.includes("other tab"));
  });
}

test("evidence receipts reject malformed paths, digests, warnings and extra content", () => {
  const saved = { type: "evidence_saved", protocol_version: 8, request_id: "r",
    relative_path: `notes/evidence/${"a".repeat(64)}.md`, key: { kind: "url", value: "key" },
    canonical_url: "https://work.example/page", captured_at: "2026-11-03T00:00:00Z",
    selection_sha256: "b".repeat(64), already_saved: false, warnings: [] };
  assert.equal(isResponseShape(saved), true);
  for (const change of [
    { relative_path: "../../private" }, { selection_sha256: "forged" },
    { already_saved: "yes" }, { warnings: ["a".repeat(257)] },
    { content: "must not echo" }, { key: { kind: "wrong", value: "key" } },
  ]) assert.equal(isResponseShape({ ...saved, ...change }), false);
});
