import assert from "node:assert/strict";
import test from "node:test";
import { activate, audit, browserFixture, focused, task } from "./fixture.mjs";

test("panel tabs expose their panels, use arrows/Home/End, and keep one tab stop", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  const tabs = page.getByRole("tab");
  assert.equal(await page.getByRole("tablist").count(), 1);
  assert.equal(await tabs.count(), 2);
  await activate(page, "#tab-page");
  for (const [key, id, panel] of [["ArrowRight", "tab-recent", "recent-view"], ["Home", "tab-page", "page-view"], ["End", "tab-recent", "recent-view"], ["ArrowLeft", "tab-page", "page-view"]]) {
    await page.keyboard.press(key);
    await focused(page, `#${id}`);
    assert.equal(await page.locator(`#${id}`).getAttribute("aria-selected"), "true");
    assert.equal(await page.locator('[role="tab"]:visible').count(), 2);
    assert.equal(await page.locator('[role="tab"][tabindex="0"]').count(), 1);
    assert.equal(await page.locator(`#${id}`).getAttribute("aria-controls"), panel);
    assert.equal(await page.getByRole("tabpanel").count(), 1);
  }
  await page.keyboard.press("ArrowRight");
  await page.locator("#copy-worklog:enabled").waitFor();
  await audit(t, page, "panel/recent");
  await activate(page, "#copy-worklog", "Space");
  assert.ok(await page.evaluate(() => fixture.actions.includes("copy")));
  await activate(page, "#open-settings");
  assert.ok(await page.evaluate(() => fixture.actions.includes("settings")));
});

test("Capture Site and Page patterns open by keyboard, escape and return focus", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await activate(page, "#capture-site");
  await focused(page, "#capture-site-heading");
  await page.locator("#capture-site-save:enabled").waitFor();
  await audit(t, page, "panel/capture-site-review");
  await page.keyboard.press("Escape");
  assert.equal(await page.locator("#capture-site-review").isVisible(), false);
  await focused(page, "#capture-site");
  await activate(page, page.locator("#site-pattern-area > summary"));
  await activate(page, "#site-pattern-review");
  await focused(page, "#site-pattern-close");
  await page.locator("#site-pattern-message").filter({ hasText: "Review only" }).waitFor();
  await audit(t, page, "panel/page-patterns");
  await page.keyboard.press("Escape");
  await focused(page, "#site-pattern-review");
  assert.equal(await page.locator("#site-pattern-result").isVisible(), false);
  await activate(page, "#capture-site");
  await page.locator("#capture-site-save:enabled").waitFor();
  await activate(page, "#capture-site-save");
  await page.waitForFunction(() => fixture.config.adapters.artifacts.length > 0 && document.activeElement.id === "tab-page");
});

test("task Run, Tasks and Summarize activate without a mouse and expose failure text", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await activate(page, "#note-body", null);
  await page.keyboard.type("Fixture page note");
  await page.waitForFunction(() => fixture.requests.some((request) => request.type === "save_note"));
  await activate(page, "#task-input", null);
  await page.keyboard.type("Review this work item");
  await activate(page, "#task-run");
  await page.waitForFunction(() => fixture.requests.some((request) => request.type === "start_task"));
  await page.locator("#task-run:enabled").waitFor();
  await page.locator("#task-run-area").filter({ hasText: "Fixture harness is offline." }).waitFor();
  await focused(page, "#task-input");
  await activate(page, "#task-open");
  await page.waitForFunction(() => fixture.actions.some((action) => action.includes("/tasks.html")));
  await activate(page, "#summarize");
  await page.waitForFunction(() => fixture.requests.some((request) => request.type === "start_run"));
  await page.locator("#summarize:enabled").waitFor();
  await page.locator("#run-area").filter({ hasText: "Fixture harness is offline." }).waitFor();
  await focused(page, "#summarize");
  await audit(t, page, "panel/agent-start-error");
});

test("settings primary actions and adapter editor work from the keyboard", async (t) => {
  const page = await browserFixture(t, "options");
  if (!page) return;
  await activate(page, "#choose-folder");
  await page.locator("#choose-folder:enabled").waitFor();
  await page.waitForFunction(() => fixture.requests.some((request) => request.type === "choose_folder"));
  await activate(page, "#detect-harnesses");
  await page.locator("#detect-harnesses:enabled").waitFor();
  await page.waitForFunction(() => fixture.requests.some((request) => request.type === "discover_harnesses"));
  await activate(page, "#harness-0", "Space");
  await activate(page, "#test-harness");
  await page.waitForFunction(() => fixture.requests.some((request) => request.type === "set_up_harness"));
  await page.locator("#test-harness:enabled").waitFor();
  await activate(page, "#new-adapter");
  await focused(page, "#adapter-editor-heading");
  await audit(t, page, "options/adapter-editor");
  await page.keyboard.press("Escape");
  assert.equal(await page.locator("#adapter-editor").isVisible(), false);
  await focused(page, "#new-adapter");
  await activate(page, "#new-adapter");
  await activate(page, "#save-adapter");
  await page.waitForFunction(() => fixture.config.adapters.artifacts.length === 1 && document.activeElement.id === "new-adapter");
  const saved = await page.evaluate(() => fixture.requests.filter((request) => request.type === "update_config").length);
  await activate(page, "#save-vocabulary");
  await page.waitForFunction((before) => fixture.requests.filter((request) => request.type === "update_config").length > before, saved);
  await activate(page, "#copy-diagnostics");
  assert.ok(await page.evaluate(() => fixture.actions.includes("copy")));
});

test("task list selection follows arrows/Home/End and detail actions survive refresh and dismissal", async (t) => {
  const page = await browserFixture(t, "tasks");
  if (!page) return;
  const row = (n) => `#task-row-${task(n).run_id}`;
  await activate(page, row(1));
  for (const [key, n] of [["ArrowDown", 2], ["End", 3], ["Home", 1], ["ArrowDown", 2], ["ArrowUp", 1]]) {
    await page.keyboard.press(key);
    await focused(page, row(n));
    assert.equal(await page.locator(row(n)).getAttribute("aria-selected"), "true");
    assert.equal(await page.locator('[role="option"][tabindex="0"]').count(), 1);
  }
  await page.locator("#tasks-detail .task-detail-report").waitFor();
  await activate(page, page.getByRole("button", { name: "Copy report", exact: true }));
  assert.ok(await page.evaluate(() => fixture.actions.includes("copy")));
  await activate(page, page.getByRole("button", { name: "Reuse", exact: true }));
  assert.ok(await page.evaluate(() => fixture.actions.includes("side-panel")));
  await activate(page, page.getByRole("button", { name: "Open page", exact: true }));
  await activate(page, page.getByRole("button", { name: "Dismiss", exact: true }));
  await page.locator(row(1)).waitFor({ state: "detached" });
  await focused(page, row(2));
  // Search/filter controls stay in normal tab order outside the listbox.
  await activate(page, "#tasks-search", null);
  await page.keyboard.type("no matching task");
  await page.getByText("No tasks match.", { exact: true }).waitFor();
  await focused(page, "#tasks-search");
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.press("Backspace");
  await page.locator(row(2)).waitFor();
  await audit(t, page, "tasks/after-dismiss");
});

test("polling retains row and detail focus; Cancel and dismissing the last task leave a usable focus target", async (t) => {
  const page = await browserFixture(t, "tasks", "configured", { runs: [task(1, "running")] });
  if (!page) return;
  await activate(page, '[role="option"]');
  const reads = await page.evaluate(() => fixture.getRunReads);
  await page.waitForFunction((before) => fixture.getRunReads > before, reads);
  await focused(page, '[role="option"]');
  // Reach Cancel by Tab without activating it yet, then wait for another real poll.
  await page.keyboard.press("Tab");
  await focused(page, '#tasks-detail button:first-child');
  const next = await page.evaluate(() => fixture.getRunReads);
  await page.waitForFunction((before) => fixture.getRunReads > before, next);
  await focused(page, '#tasks-detail button:first-child');
  await activate(page, page.getByRole("button", { name: "Cancel", exact: true }));
  await page.waitForFunction(() => fixture.runs[0].state === "canceled");
  await page.getByRole("button", { name: "Dismiss", exact: true }).waitFor();
  await activate(page, page.getByRole("button", { name: "Dismiss", exact: true }));
  await page.locator('[role="option"]').waitFor({ state: "detached" });
  await focused(page, "#tasks-search");
});

for (const name of ["panel", "tasks"]) {
  test(`${name} setup offers keyboard access to settings`, async (t) => {
    const page = await browserFixture(t, name, "setup");
    if (!page) return;
    await activate(page, name === "panel" ? "#setup-open-settings" : "#tasks-status-settings");
    assert.ok(await page.evaluate(() => fixture.actions.includes("settings")));
  });
}
