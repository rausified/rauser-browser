import assert from "node:assert/strict";
import test from "node:test";
import { activate, audit, browserFixture, enterPanelTab, focused, selectByKeyboard, task } from "./fixture.mjs";

test("optional task notifications can be enabled and disabled by keyboard without losing focus", async (t) => {
  const page = await browserFixture(t, "options");
  if (!page) return;
  const button = page.locator("#task-notifications-toggle");
  await page.locator("#task-notifications-state").filter({ hasText: "Off." }).waitFor();
  await activate(page, button, "Space");
  await page.locator("#task-notifications-state").filter({ hasText: "On." }).waitFor();
  await focused(page, "#task-notifications-toggle");
  await activate(page, button, "Enter");
  await page.locator("#task-notifications-state").filter({ hasText: "Off." }).waitFor();
  await focused(page, "#task-notifications-toggle");
  assert.deepEqual(await page.evaluate(() => fixture.actions.filter((action) => action.endsWith("task-notifications"))),
    ["enable-task-notifications", "disable-task-notifications"]);
  await audit(t, page, "settings/task-notifications");
});

test("saved task commands can be authored, removed and saved using the keyboard", async (t) => {
  const page = await browserFixture(t, "options");
  if (!page) return;
  // Native replies arrive asynchronously. Keep this read pending long enough
  // to exercise opening the editor after the activation key has returned.
  await page.evaluate(() => {
    for (const port of fixture.ports) {
      const send = port.postMessage;
      port.postMessage = (request) => request.type === "get_task_command_settings"
        ? setTimeout(() => send(request), 100) : send(request);
    }
  });
  await activate(page, "#commands-open");
  await page.locator("#command-1-name:enabled").waitFor();
  await focused(page, "#commands-editor-heading");
  await page.keyboard.press("Tab");
  await focused(page, "#command-1-name");
  await page.keyboard.press("Tab");
  await focused(page, "#command-1-description");
  await page.keyboard.press("Tab");
  await focused(page, "#command-1-prompt");
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type("Find unresolved decisions.");
  await activate(page, "#commands-add");
  await focused(page, "#command-2-name");
  await page.keyboard.type("review");
  await page.keyboard.press("Tab");
  await page.keyboard.type("Review the page");
  await page.keyboard.press("Tab");
  await page.keyboard.type("Describe the tradeoffs.");
  await audit(t, page, "settings/command-editor");
  await activate(page, "#commands-save");
  await page.locator("#commands-status").filter({ hasText: "Commands saved." }).waitFor();
  await focused(page, "#commands-save");
  const saved = await page.evaluate(() => fixture.requests.find((r) => r.type === "save_task_commands"));
  assert.match(saved.commands_toml, /Find unresolved decisions/);
  assert.match(saved.commands_toml, /name = "review"/);
  assert.equal(await page.evaluate(() => fixture.requests.some((r) => r.type === "start_task")), false);
  await activate(page, page.locator("#commands-rows fieldset").last().getByRole("button", { name: "Remove command" }));
  await focused(page, "#command-1-name");
  await activate(page, "#commands-cancel");
  await focused(page, "#commands-open");
});

test("a task report saves to its source note by keyboard with focus and confirmation", async (t) => {
  const page = await browserFixture(t, "tasks");
  if (!page) return;
  const save = page.getByRole("button", { name: "Save to note", exact: true });
  await save.waitFor();
  assert.equal(await page.evaluate(() => fixture.requests.filter((r) => r.type === "save_task_note").length), 0);
  await activate(page, save, "Space");
  await page.getByRole("status").filter({ hasText: "Report saved to the source page's note." }).waitFor();
  assert.equal(await save.evaluate((node) => node === document.activeElement), true);
  assert.equal(await page.evaluate(() => fixture.requests.filter((r) => r.type === "save_task_note").length), 1);
  await audit(t, page, "tasks/report-saved-to-note");
});

test("panel tabs expose their panels, use arrows/Home/End, and keep one tab stop", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  const tabs = page.getByRole("tab");
  assert.equal(await page.getByRole("tablist").count(), 1);
  assert.equal(await tabs.count(), 2);
  await enterPanelTab(page, "tab-page");
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
  await page.waitForFunction(() => fixture.actions.includes("copy"));
  await activate(page, "#open-settings");
  assert.ok(await page.evaluate(() => fixture.actions.includes("settings")));
});

test("worklog pagination appends unchecked choices and keeps keyboard focus on the final page", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await page.evaluate(() => { fixture.worklogPages = true; });
  await enterPanelTab(page, "tab-recent");
  await activate(page, "#worklog-curate", "Space");
  const choices = page.locator('#worklog-choices input[type="checkbox"]');
  await choices.first().waitFor();
  await activate(page, choices.first(), "Space");
  await activate(page, "#worklog-more", "Enter");
  await page.waitForFunction(() => document.querySelectorAll('#worklog-choices input').length === 3);
  assert.equal(await choices.nth(2).evaluate((input) => input === document.activeElement), true);
  assert.equal(await choices.first().isChecked(), true);
  assert.equal(await choices.nth(2).isChecked(), false);
  assert.doesNotMatch(await page.locator("#worklog-text").textContent(), /Later work/);
  await page.keyboard.press("Space");
  const preview = await page.locator("#worklog-text").textContent();
  await activate(page, "#copy-worklog", "Space");
  await page.waitForFunction(() => fixture.copied.length === 1);
  assert.equal(await page.evaluate(() => fixture.copied[0]), preview);
  await audit(t, page, "panel/selected-work-pages");
});

test("worklog curation selects and copies checked work entirely by keyboard", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await enterPanelTab(page, "tab-recent");
  await activate(page, "#worklog-curate", "Space");
  const choices = page.locator('#worklog-choices input[type="checkbox"]');
  await choices.first().waitFor();
  assert.equal(await page.locator("#copy-worklog").isDisabled(), true);
  await activate(page, choices.first(), "Space");
  assert.equal(await choices.first().evaluate((input) => input === document.activeElement), true);
  assert.match(await page.locator("#worklog-text").textContent(), /Release update/);
  assert.doesNotMatch(await page.locator("#worklog-text").textContent(), /Private research/);
  const before = await page.evaluate(() => fixture.requests.filter((r) => r.type === "worklog_choices").length);
  await page.evaluate(() => { fixture.workerStatus.delivered_batches++; });
  await page.waitForFunction((count) => fixture.requests.filter((r) => r.type === "worklog_choices").length > count, before, { timeout: 40_000 });
  await choices.first().waitFor();
  assert.equal(await choices.first().evaluate((input) => input === document.activeElement), true);
  assert.equal(await choices.first().isChecked(), true);
  await audit(t, page, "panel/selected-work");
  await activate(page, "#copy-worklog", "Space");
  await page.waitForFunction(() => fixture.actions.includes("copy"));
  await activate(page, "#worklog-clear", "Space");
  assert.equal(await page.locator("#copy-worklog").isDisabled(), true);
  await activate(page, "#worklog-select-all", "Space");
  assert.match(await page.locator("#worklog-text").textContent(), /2 artifact-day entries/);
  await audit(t, page, "panel/selected-work-all");
});

test("sharing drafts survive reload and support explicit keyboard copy, comparison and deletion", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await enterPanelTab(page, "tab-recent");
  await activate(page, "#worklog-curate", "Space");
  const choice = page.locator('#worklog-choices input[type="checkbox"]').first();
  await choice.waitFor();
  await activate(page, choice, "Space");
  const original = await page.locator("#worklog-text").textContent();
  if (!await page.locator("#sharing-drafts").evaluate((node) => node.open)) await activate(page, "#sharing-drafts > summary");
  await activate(page, "#sharing-draft-name", null);
  await page.keyboard.type("Release handoff");
  await activate(page, "#sharing-draft-save");
  await page.locator("#sharing-draft-status").filter({ hasText: "Draft saved" }).waitFor();
  await focused(page, "#sharing-draft-save");
  assert.equal(await page.locator("#sharing-draft-original").textContent(), original);
  await page.reload();
  await page.locator("#note-body:enabled").waitFor();
  await enterPanelTab(page, "tab-recent");
  if (!await page.locator("#sharing-drafts").evaluate((node) => node.open)) await activate(page, "#sharing-drafts > summary");
  await activate(page, "#sharing-draft-list", null);
  await page.keyboard.press("End");
  await page.keyboard.press("Enter");
  await page.locator("#sharing-draft-view:visible").waitFor();
  await focused(page, "#sharing-draft-list");
  assert.equal(await page.locator("#sharing-draft-original").textContent(), original);
  assert.deepEqual(await page.evaluate(() => fixture.copied), []);
  await activate(page, "#sharing-draft-copy", "Space");
  await page.waitForFunction(() => fixture.copied.length === 1);
  assert.equal(await page.evaluate(() => fixture.copied[0]), original);
  await activate(page, "#sharing-draft-refresh");
  await page.locator("#sharing-draft-review:visible").waitFor();
  await focused(page, "#sharing-draft-refresh");
  const rows = page.locator('#sharing-draft-rows input[type="checkbox"]');
  assert.equal(await rows.count(), 1);
  assert.equal(await rows.first().isChecked(), true);
  await audit(t, page, "panel/sharing-draft-review");
  await activate(page, "#sharing-draft-delete", "Space");
  await page.locator("#sharing-draft-status").filter({ hasText: "Draft deleted" }).waitFor();
  await focused(page, "#sharing-draft-list");
  assert.equal(await page.evaluate(() => fixture.requests.some((r) => r.type === "start_task" || r.type === "save_note")), false);
});

test("More sessions preserves checked work and returns keyboard focus to the catalog when exhausted", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await page.evaluate(() => { fixture.focusPages = true; });
  await enterPanelTab(page, "tab-recent");
  await activate(page, "#worklog-curate", "Space");
  await page.locator('#worklog-choices input[type="checkbox"]').first().waitFor();
  await page.locator("#worklog-share-scope").focus();
  await selectByKeyboard(page, "#worklog-share-scope", "last");
  await page.locator("#worklog-focus-session:enabled").waitFor();
  await page.locator("#worklog-focus-session").focus();
  await selectByKeyboard(page, "#worklog-focus-session", "last");
  const choice = page.locator('#worklog-choices input[type="checkbox"]').first();
  await choice.waitFor();
  await activate(page, choice, "Space");
  const preview = await page.locator("#worklog-text").textContent();
  await activate(page, "#worklog-focus-more", null);
  const before = await page.evaluate(() => fixture.requests.filter((r) => r.type === "worklog_focus_choices").length);
  await page.evaluate(() => { fixture.workerStatus.delivered_batches++; });
  await page.waitForFunction((count) => fixture.requests.filter((r) => r.type === "worklog_focus_choices").length > count, before, { timeout: 40_000 });
  await page.locator("#worklog-focus-more:visible").waitFor();
  await focused(page, "#worklog-focus-more");
  await activate(page, "#worklog-focus-more", "Enter");
  await page.waitForFunction(() => document.querySelectorAll('#worklog-focus-session option').length === 3);
  await focused(page, "#worklog-focus-session");
  assert.equal(await choice.isChecked(), true);
  assert.equal(await page.locator("#worklog-text").textContent(), preview);
  assert.equal(await page.locator("#worklog-focus-session").inputValue(), "10000000-0000-4000-8000-000000000001");
  await audit(t, page, "panel/share-session-pages");
});

test("Reload sessions keeps keyboard focus after clearing an invalidated session selection", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await page.evaluate(() => { fixture.focusPages = true; fixture.focusPageConflict = true; });
  await enterPanelTab(page, "tab-recent");
  await activate(page, "#worklog-curate", "Space");
  await page.locator('#worklog-choices input[type="checkbox"]').first().waitFor();
  await page.locator("#worklog-share-scope").focus();
  await selectByKeyboard(page, "#worklog-share-scope", "last");
  await page.locator("#worklog-focus-session:enabled").waitFor();
  await page.locator("#worklog-focus-session").focus();
  await selectByKeyboard(page, "#worklog-focus-session", "last");
  await page.locator('#worklog-choices input[type="checkbox"]').first().waitFor();
  await activate(page, "#worklog-focus-more");
  await page.locator("#worklog-focus-more").filter({ hasText: "Reload sessions" }).waitFor();
  await focused(page, "#worklog-focus-more");
  await page.evaluate(() => { fixture.focusPageConflict = false; });
  await activate(page, "#worklog-focus-more");
  await page.locator("#worklog-focus-more").filter({ hasText: "More sessions" }).waitFor();
  await focused(page, "#worklog-focus-more");
  assert.equal(await page.locator("#worklog-focus-session").inputValue(), "");
});

test("sharing a recorded focus session is named, keyboard operable and copies the exact preview", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await enterPanelTab(page, "tab-recent");
  await activate(page, "#worklog-curate", "Space");
  await page.locator('#worklog-choices input[type="checkbox"]').first().waitFor();
  await page.locator("#worklog-share-scope").focus();
  await selectByKeyboard(page, "#worklog-share-scope", "last");
  // Use the native select's keyboard, and wait for the recorded catalog.
  await page.locator("#worklog-focus-session:enabled").waitFor();
  await page.getByLabel("Recorded focus session", { exact: true }).focus();
  await selectByKeyboard(page, "#worklog-focus-session", "last");
  const choice = page.locator('#worklog-choices input[type="checkbox"]').first();
  await choice.waitFor();
  await focused(page, "#worklog-focus-session");
  // A second keyboard choice must work without focusing the select again.
  await selectByKeyboard(page, "#worklog-focus-session", "first");
  await page.getByText("Choose a session, then check entries to include.", { exact: true }).first().waitFor();
  await focused(page, "#worklog-focus-session");
  await selectByKeyboard(page, "#worklog-focus-session", "last");
  await choice.waitFor();
  await focused(page, "#worklog-focus-session");
  await activate(page, choice, "Space");
  const preview = await page.locator("#worklog-text").textContent();
  assert.match(preview, /Focus session: Release/);
  assert.doesNotMatch(preview, /Private research/);
  await audit(t, page, "panel/share-focus-session");
  await activate(page, "#copy-worklog", "Space");
  await page.waitForFunction(() => fixture.actions.includes("copy"));
  assert.equal(await page.evaluate(() => fixture.copied.at(-1)), preview);
});

test("the worklog period segments are one tab stop that arrows move, with a visible ring", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await enterPanelTab(page, "tab-recent");
  await page.locator("#copy-worklog:enabled").waitFor();
  for (let n = 0; n < 40 && !(await page.locator("#worklog-today").evaluate((input) => input === document.activeElement)); n++) {
    await page.keyboard.press("Tab");
  }
  await focused(page, "#worklog-today");
  assert.equal(await page.getByRole("radio").count(), 3);
  assert.equal(await page.getByRole("group", { name: "Worklog period" }).count(), 1);
  const ring = await page.locator('label[for="worklog-today"]').evaluate((label) => getComputedStyle(label).outlineStyle);
  assert.equal(ring, "solid", "the focused segment's label carries the focus ring");
  const before = await page.evaluate(() => fixture.requests.filter((r) => r.type === "worklog_report").length);
  await page.keyboard.press("ArrowRight");
  await focused(page, "#worklog-yesterday");
  assert.equal(await page.locator("#worklog-yesterday").isChecked(), true);
  assert.equal(await page.locator("#worklog-today").isChecked(), false);
  await page.waitForFunction((count) => fixture.requests.filter((r) => r.type === "worklog_report").length > count, before);
  const range = await page.evaluate(() => fixture.requests.filter((r) => r.type === "worklog_report").at(-1));
  assert.equal(range.start_date, range.end_date, "Yesterday is one day");
  // Tab leaves the group: the other segments are not tab stops.
  await page.keyboard.press("Tab");
  assert.equal(await page.evaluate(() => document.activeElement?.getAttribute("name")), null);
  await audit(t, page, "panel/worklog-period");
});

for (const outcome of ["append", "conflict"]) {
  test(`Recent Load more preserves keyboard focus after ${outcome}`, async (t) => {
    const page = await browserFixture(t, "panel");
    if (!page) return;
    await page.evaluate((conflict) => { fixture.recentPages = true; fixture.recentPageConflict = conflict; }, outcome === "conflict");
    await enterPanelTab(page, "tab-recent");
    await activate(page, "#recent-more", "Enter");
    if (outcome === "append") {
      await page.getByRole("link", { name: /Later report/ }).waitFor();
      await focused(page, '.recent-link[href="https://work.example/1"]');
      assert.equal(await page.locator("#recent-list h3").count(), 1);
    } else {
      await page.getByRole("button", { name: "Reload results", exact: true }).waitFor();
      await focused(page, "#recent-reload");
    }
    assert.equal(await page.locator("#recent-more").isVisible(), false);
    await audit(t, page, `panel/recent-pages-${outcome}`);
  });
}

test("Recent's saved-summary reading filter is named and keyboard operable", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await enterPanelTab(page, "tab-recent");
  const selector = page.getByRole("combobox", { name: "Saved summary reading", exact: true });
  await activate(page, selector, null);
  await selectByKeyboard(page, selector, "last");
  await page.waitForFunction(() => fixture.requests.some((request) => request.type === "list_artifacts" && request.summary_reading === "unknown"));
  await focused(page, "#recent-reading");
  await audit(t, page, "panel/summary-reading");
  await selectByKeyboard(page, selector, "first");
  await page.waitForFunction(() => {
    const latest = fixture.requests.filter((request) => request.type === "list_artifacts").at(-1);
    return latest && !Object.hasOwn(latest, "summary_reading");
  });
  await focused(page, "#recent-reading");
});

test("a summarized page's re-summarize icon sits in the Summary card, named and keyboard-operable", async (t) => {
  for (const colorScheme of ["light", "dark"]) {
    const page = await browserFixture(t, "panel", "configured", { colorScheme, summary: {
      summarized_at: "2026-11-02T10:00:00-07:00", document_type: "report", category: "work", importance: "high",
      summary_path: "summaries/2026-11-02 Work item.md", harness: "codex", overview: "A fixture overview.", read_scope: "page_only",
    } });
    if (!page) return;
    await page.locator("#page-summary:visible").waitFor();
    await page.locator("#page-summary-chips").getByText("Agent read: browser page only", { exact: true }).waitFor();
    assert.equal(await page.locator("#page-summary-head #summarize").count(), 1);
    assert.equal(await page.locator("#page-actions #summarize").count(), 0);
    await page.getByRole("button", { name: "Summarize again" }).waitFor();
    // Only the icon shows: the label is clipped to the visually hidden 1px box.
    const box = await page.locator("#summarize-label").boundingBox();
    assert.ok(box && box.width <= 1 && box.height <= 1, `label box ${JSON.stringify(box)}`);
    assert.ok((await page.locator("#summarize .summarize-icon").boundingBox())?.width > 1, "the icon shows");
    await activate(page, "#summarize");
    await page.waitForFunction(() => fixture.requests.some((request) => request.type === "start_run"));
    await page.locator("#summary-activity-label").waitFor();
    await activate(page, "#summary-activity-label", "Enter");
    await page.locator("#run-area").filter({ hasText: "Fixture harness is offline." }).waitFor();
    await audit(t, page, `panel/summary-card-${colorScheme}`);
  }
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
  await activate(page, page.locator("#site-pattern-area > summary"), null);
  // A clipping ancestor would hide an outward ring, so it must draw inward.
  const ring = await page.locator("#site-pattern-area > summary").evaluate((element) => {
    let clipped = false;
    for (let node = element.parentElement; node; node = node.parentElement) {
      if (getComputedStyle(node).overflow !== "visible") clipped = true;
      if (node === document.body) break;
    }
    const style = getComputedStyle(element);
    return { clipped, offset: parseFloat(style.outlineOffset), width: parseFloat(style.outlineWidth) };
  });
  assert.ok(!ring.clipped || ring.offset <= -ring.width, `focus ring is clipped: ${JSON.stringify(ring)}`);
  await page.keyboard.press("Enter");
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
  await page.locator("#task-activity-label").waitFor();
  await focused(page, "#task-input");
  await activate(page, "#task-activity-label", "Enter");
  await page.locator("#task-run-area").filter({ hasText: "Fixture harness is offline." }).waitFor();
  await activate(page, "#task-open");
  await page.waitForFunction(() => fixture.actions.some((action) => action.includes("/tasks.html")));
  await activate(page, "#summarize");
  await page.waitForFunction(() => fixture.requests.some((request) => request.type === "start_run"));
  await page.locator("#summarize:enabled").waitFor();
  await page.locator("#summary-activity-label").waitFor();
  await focused(page, "#summarize");
  await activate(page, "#summary-activity-label", "Enter");
  await page.locator("#run-area").filter({ hasText: "Fixture harness is offline." }).waitFor();
  await audit(t, page, "panel/agent-start-error");
});

test("activity dock stays at the bottom and caps expanded output", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await page.evaluate(() => {
    const hidden = document.createElement("div");
    hidden.className = "run-card";
    hidden.hidden = true;
    const staleTitle = document.createElement("p");
    staleTitle.className = "run-card-title";
    staleTitle.textContent = "Previous page";
    hidden.append(staleTitle);
    document.querySelector("#run-area").append(hidden);
    const card = document.createElement("div");
    card.className = "run-card";
    const title = document.createElement("p");
    title.className = "run-card-title";
    title.textContent = "Astryx";
    const state = document.createElement("p");
    state.className = "run-card-state";
    state.textContent = "Running";
    const stream = document.createElement("pre");
    stream.className = "run-card-stream";
    stream.textContent = "Long agent output\n".repeat(300);
    card.append(title, state, stream);
    document.querySelector("#run-area").append(card);
  });
  await page.locator("#summary-activity-label").filter({ hasText: "Astryx · Running" }).waitFor();
  assert.equal(await page.locator("#run-area").isVisible(), false);
  await page.locator("#summary-activity-label").focus();
  const before = await page.locator("#page-area").boundingBox();
  await page.keyboard.press("Enter");
  assert.equal(await page.locator("#run-area").isVisible(), true);
  const layout = await page.evaluate(() => {
    const dock = document.querySelector("#activity-dock").getBoundingClientRect();
    const strip = document.querySelector(".capture-bar").getBoundingClientRect();
    const main = document.querySelector("main").getBoundingClientRect();
    return { dockHeight: dock.height, stripTop: strip.top, dockBottom: dock.bottom,
      mainBottom: main.bottom, viewport: window.innerHeight };
  });
  assert.ok(layout.dockHeight <= layout.viewport * .38 + 2, JSON.stringify(layout));
  assert.ok(layout.dockBottom <= layout.stripTop + 1, JSON.stringify(layout));
  assert.ok(layout.stripTop < layout.mainBottom, JSON.stringify(layout));
  const after = await page.locator("#page-area").boundingBox();
  assert.ok(before && after && Math.abs(before.y - after.y) <= 1, "page content does not jump");
  await page.evaluate(() => { document.querySelector("#run-area .run-card:not([hidden])").hidden = true; });
  await page.locator("#summary-activity-label").waitFor({ state: "hidden" });
  await page.locator(".activity-empty:visible").waitFor();
  await page.setViewportSize({ width: 400, height: 320 });
  await activate(page, "#capture-strip-summary", "Enter");
  const strip = await page.evaluate(() => {
    const bar = document.querySelector(".capture-bar");
    bar.scrollTop = bar.scrollHeight;
    const last = document.querySelector("#discard-pending").getBoundingClientRect();
    return { scrollHeight: bar.scrollHeight, clientHeight: bar.clientHeight,
      lastBottom: last.bottom, barBottom: bar.getBoundingClientRect().bottom };
  });
  assert.ok(strip.scrollHeight > strip.clientHeight, JSON.stringify(strip));
  assert.ok(strip.lastBottom <= strip.barBottom + 1, JSON.stringify(strip));
});

test("task working folder can be selected by keyboard without starting a task", async (t) => {
  const page = await browserFixture(t, "panel", "configured", {
    taskFolders: [{ id: "project-id", path: "/work/project" }],
  });
  if (!page) return;
  await page.locator("#task-folder option[value='project-id']").waitFor({ state: "attached" });
  await activate(page, "#task-input", null);
  await page.keyboard.type("Inspect this project");
  await activate(page, "#task-folder", null);
  await selectByKeyboard(page, "#task-folder", 1);
  assert.equal(await page.locator("#task-folder").inputValue(), "project-id");
  assert.equal(await page.locator("#task-input").inputValue(), "Inspect this project");
  assert.equal(await page.evaluate(() => fixture.requests.some((r) => r.type === "start_task")), false);
  await audit(t, page, "panel/task-project-folder");
  await activate(page, "#task-run");
  await page.waitForFunction(() => fixture.requests.some((r) => r.type === "start_task"));
  assert.equal(await page.evaluate(() => fixture.requests.find((r) => r.type === "start_task").task_folder_id), "project-id");
});

test("task page scope can be changed by keyboard without submitting or losing a draft", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await page.locator("#task-run:enabled").waitFor();
  await activate(page, "#task-input", null);
  await page.keyboard.type("Organize my notes");
  await activate(page, "#task-use-page", "Space");
  assert.equal(await page.locator("#task-use-page").isChecked(), false);
  assert.equal(await page.locator("#task-input").inputValue(), "Organize my notes");
  assert.equal(await page.locator("#task-context").textContent(), "No page · Runs in your knowledge base");
  assert.equal(await page.evaluate(() => fixture.requests.some((request) => request.type === "start_task")), false);
  await audit(t, page, "panel/task-without-page");
  await activate(page, "#task-run");
  await page.waitForFunction(() => fixture.requests.some((request) => request.type === "start_task"));
  const request = await page.evaluate(() => fixture.requests.find((request) => request.type === "start_task"));
  assert.equal(request.no_page, true);
  for (const key of ["url", "title", "content", "capture_truncated"]) assert.equal(key in request, false, key);
});

test("task slash focus and history arrows preserve drafts and never submit implicitly", async (t) => {
  const full = "Review the complete instruction " + "x".repeat(350);
  const page = await browserFixture(t, "panel", "configured", { runs: [
    { ...task(1), instruction: full }, { ...task(2), instruction: "Earlier request" },
  ] });
  if (!page) return;
  await page.locator("#task-run:enabled").waitFor();
  await enterPanelTab(page, "tab-recent");
  await page.keyboard.press("/");
  await focused(page, "#task-input");
  assert.equal(await page.locator("#task-input").inputValue(), "");
  await page.keyboard.type("My unsent draft");
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.press("ArrowLeft");
  await page.keyboard.press("ArrowUp");
  await page.waitForFunction((text) => document.querySelector("#task-input").value === text, full);
  // The recalled value keeps its caret at the start for repeated Up presses.
  await page.keyboard.press("ArrowUp");
  await page.waitForFunction(() => document.querySelector("#task-input").value === "Earlier request");
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.press("ArrowRight");
  await page.keyboard.press("ArrowDown");
  await page.waitForFunction((text) => document.querySelector("#task-input").value === text, full);
  await page.keyboard.press("ArrowDown");
  await page.waitForFunction(() => document.querySelector("#task-input").value === "My unsent draft");
  await page.keyboard.type("/");
  assert.equal(await page.locator("#task-input").inputValue(), "My unsent draft/");
  await enterPanelTab(page, "tab-page");
  await page.evaluate(() => chrome.runtime.onMessage.fire(
    { kind: "focus_task", tab_id: 1, at: Date.now() }, { id: chrome.runtime.id },
  ));
  await focused(page, "#task-input");
  assert.equal(await page.locator("#task-input").inputValue(), "My unsent draft/");
  await activate(page, "#note-body", null);
  await page.keyboard.type("A note/");
  assert.equal(await page.locator("#note-body").inputValue(), "A note/");
  assert.equal(await page.evaluate(() => fixture.requests.filter((request) => request.type === "start_task").length), 0);
  await audit(t, page, "panel/task-keyboard-recall");
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
  await page.waitForFunction(() => fixture.actions.includes("copy"));
});

test("site review selects Google kinds without JSON and saves by keyboard", async (t) => {
  const page = await browserFixture(t, "options");
  if (!page) return;
  await page.locator("#site-origin").fill("https://docs.google.com");
  const review = page.locator('[data-site-bundle="true"]');
  await review.waitFor();
  await activate(page, review);
  await focused(page, "#adapter-editor-heading");
  assert.equal(await page.locator("#adapter-technical-review").isVisible(), false);
  assert.equal(await page.locator("#site-bundle-review input[type=checkbox]").count(), 5);
  await audit(t, page, "options/site-review");
  await page.locator('[data-site-kind="google.formresponse"]').focus();
  await page.keyboard.press("Space");
  await activate(page, "#save-adapter");
  await page.waitForFunction(() => fixture.config.adapters.artifacts.length === 4);
  assert.deepEqual(await page.evaluate(() => fixture.config.adapters.artifacts.map((adapter) => adapter.type)),
    ["google.doc", "google.sheet", "google.slides", "google.form"]);
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
  await page.waitForFunction(() => fixture.actions.includes("copy"));
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


test("focus sessions start, rename and end by keyboard with text-only names", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await activate(page, "#focus-heading");
  await activate(page, "#focus-name", null);
  await page.keyboard.type("Release planning");
  await page.locator("#focus-save:enabled").waitFor();
  await activate(page, "#focus-save");
  await page.waitForFunction(() => document.querySelector("#focus-current").textContent === "Release planning");
  await audit(t, page, "panel/focus-session");
  await activate(page, "#focus-name", null);
  await page.keyboard.press("ControlOrMeta+A");
  await page.keyboard.type("<img src=x onerror=attack()>");
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => document.querySelector("#focus-current").textContent === "<img src=x onerror=attack()>");
  assert.equal(await page.locator("#focus-current img").count(), 0);
  await activate(page, "#focus-end");
  await page.waitForFunction(() => document.querySelector("#focus-current").textContent === "None");
  assert.equal(await page.locator("#focus-end").isHidden(), true);
  await audit(t, page, "panel/focus-ended");
});

test("saved task commands are chosen by keyboard and require an explicit Run", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await page.locator("#task-run:enabled").waitFor();
  await activate(page, "#task-input", null);
  await page.keyboard.type("Focus on today");
  await activate(page, "#task-commands summary", "Enter");
  await page.locator("#task-command:enabled").waitFor();
  await activate(page, "#task-command", null);
  await selectByKeyboard(page, "#task-command", 1);
  assert.equal(await page.locator("#task-input").inputValue(), "Focus on today");
  assert.equal(await page.evaluate(() => fixture.requests.some((r) => r.type === "start_task")), false);
  await activate(page, "#task-command-insert", "Space");
  await page.waitForFunction(() => document.querySelector("#task-input").value === "/actions Focus on today");
  await focused(page, "#task-input");
  assert.equal(await page.evaluate(() => fixture.requests.some((r) => r.type === "start_task")), false);
  await audit(t, page, "panel/saved-commands");
  await page.keyboard.press("Enter");
  await page.waitForFunction(() => fixture.requests.some((r) => r.type === "start_task"));
  const request = await page.evaluate(() => fixture.requests.find((r) => r.type === "start_task"));
  assert.equal(request.command_revision, "1");
  assert.equal(request.instruction, "/actions Focus on today");
  assert.equal("prompt" in request, false);
});
