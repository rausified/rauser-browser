import assert from "node:assert/strict";
import test from "node:test";
import { AxeBuilder } from "@axe-core/playwright";
import { assertAccessible, audit, browserFixture, task } from "./fixture.mjs";

for (const pageName of ["panel", "options", "tasks"]) {
  for (const state of ["setup", "configured", "disconnected", "error"]) {
    for (const colorScheme of ["light", "dark"]) {
      test(`${pageName}: ${state}, ${colorScheme}`, async (t) => {
        const page = await browserFixture(t, pageName, state, { colorScheme });
        if (page) await audit(t, page, `${pageName}/${state}/${colorScheme}`);
      });
    }
  }
}

test("Tasks listbox keeps named groups and valid children through filtering", async (t) => {
  const today = new Date();
  today.setHours(12, 0, 0, 0);
  const yesterday = new Date(today);
  yesterday.setDate(yesterday.getDate() - 1);
  const page = await browserFixture(t, "tasks", "configured", { runs: [
    task(1, "running"),
    { ...task(2), created_at: today.toISOString() },
    { ...task(3), created_at: yesterday.toISOString() },
  ] });
  if (!page) return;
  const list = page.getByRole("listbox", { name: "Tasks", exact: true });
  for (const name of ["Running", "Today", "Yesterday"]) {
    assert.equal(await list.getByRole("group", { name, exact: true }).getByRole("option").count(), 1);
  }
  assert.equal(await list.getByRole("heading").count(), 0, "group labels must not add heading children to the listbox");
  const grouped = await audit(t, page, "tasks/named-groups");
  assert.ok(grouped.passes.some(({ id, nodes }) => id === "aria-required-children" && nodes.some(({ target }) => target.includes("#tasks-list"))), "the populated listbox's child roles must be checked and pass");
  await page.getByRole("searchbox", { name: "Search tasks" }).fill("no matching task");
  await page.getByText("No tasks match.", { exact: true }).waitFor();
  assert.equal(await list.isVisible(), false, "an empty listbox must not be exposed");
  assert.equal(await page.locator("#tasks-detail").isVisible(), true);
  assert.equal(await list.getByRole("option").count(), 0);
  await audit(t, page, "tasks/filtered-empty");
  await page.getByRole("searchbox", { name: "Search tasks" }).fill("");
  assert.equal(await list.isVisible(), true);
  assert.equal(await list.getByRole("option").count(), 3);
  await audit(t, page, "tasks/filter-cleared");
});

for (const state of ["disconnected", "error"]) {
  test(`Tasks ${state} warning stays readable when the theme changes`, async (t) => {
    const page = await browserFixture(t, "tasks", state);
    if (!page) return;
    for (const colorScheme of ["light", "dark", "light"]) {
      await page.emulateMedia({ colorScheme });
      // An implicit browser canvas is transparent to contrast checkers. The
      // product must paint an opaque, theme-aware background for its text.
      const background = await page.locator("body").evaluate((body) => getComputedStyle(body).backgroundColor);
      assert.match(background, /^rgb\(/, `expected an opaque ${colorScheme} canvas, got ${background}`);
      const results = await audit(t, page, `tasks/${state}/theme-change/${colorScheme}`);
      assert.ok(results.passes.some(({ id, nodes }) => id === "color-contrast" && nodes.some(({ target }) => target.includes("#tasks-status"))), "the warning's contrast must be measured and pass");
    }
  });
}

test("a seeded serious violation fails the same gate used for every state", async (t) => {
  const page = await browserFixture(t, "panel");
  if (!page) return;
  await page.evaluate(() => {
    const button = document.createElement("button");
    button.id = "seeded-unlabelled-button";
    document.querySelector("main").append(button);
  });
  const results = await new AxeBuilder({ page }).analyze();
  assert.ok(results.violations.some(({ id, nodes }) => id === "button-name" && nodes.some(({ target }) => target.includes("#seeded-unlabelled-button"))));
  assert.throws(() => assertAccessible(results));
});
