// Built pages and real browser behavior; only Chrome/native transport is fake.
// Every request is fulfilled locally or aborted. No server, host or harness runs.
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { AxeBuilder } from "@axe-core/playwright";
import { a11yBrowser } from "../../../scripts/a11y-browser.mjs";

export const ORIGIN = "https://brauser-a11y.invalid";
export const PAGE_URL = "https://acme.atlassian.net/browse/WORK-42";

export function task(n, state = "succeeded") {
  const at = new Date(Date.now() - n * 60_000).toISOString();
  return {
    run_id: `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`,
    kind: "task", state, harness: "codex", instruction: `Review task ${n}`,
    task_report: state === "succeeded" ? "Review complete." : null,
    record_key: { kind: "url", value: PAGE_URL }, written_record_key: null,
    url: PAGE_URL, title: "Work item", cwd: null,
    created_at: at, started_at: at, finished_at: state === "running" ? null : at,
    error: state === "failed" ? "Fixture harness failed." : null,
    error_kind: state === "failed" ? "harness_failed" : null,
    thread_id: null, session_id: null, summary_path: null, stream_bytes: 0,
    can_retry_save: false, answer: null,
  };
}

// Serialized into the browser before any product module executes.
export function installChrome({ state, runs, pageUrl, pageName, summary = null }) {
  const event = () => {
    const listeners = [];
    return { addListener: (listener) => listeners.push(listener), fire: (...args) => listeners.forEach((listener) => listener(...args)) };
  };
  const storageChanged = event();
  const area = (areaName) => {
    const data = {};
    return {
      async get(keys) {
        if (keys === null) return structuredClone(data);
        return Object.fromEntries((Array.isArray(keys) ? keys : [keys]).filter((key) => key in data).map((key) => [key, structuredClone(data[key])]));
      },
      async set(items) {
        const changes = {};
        for (const [key, value] of Object.entries(items)) {
          changes[key] = { oldValue: data[key], newValue: structuredClone(value) };
          data[key] = structuredClone(value);
        }
        storageChanged.fire(changes, areaName);
      },
      async remove(keys) { for (const key of Array.isArray(keys) ? keys : [keys]) delete data[key]; },
    };
  };
  const configured = state !== "setup";
  const fixture = window.fixture = {
    state, runs, requests: [], actions: [], copied: [], ports: [], revision: "1",
    adapterIssue: state === "adapter-repair" ? "unsupported adapter format 2" : null,
    listError: state === "error", getRunReads: 0,
    config: {
      storage: configured ? {
        root: "/fixture-home", profile: "neutral", log_dir: "logs", pages_dir: "notes",
        later_dir: "later", artifacts_dir: "artifacts", summaries_dir: "summaries", knowledge_base_dir: null,
      } : null,
      capture_enabled: configured, strip_params: [],
      agent: configured ? { harness: "codex", binary: "/fixture/codex", version: "1.0", timeout_secs: 600, probed_at: "2026-11-01T12:00:00Z" } : null,
      summary_vocabulary: { document_types: ["report"], categories: ["work"] },
      auto_summary: { enabled: true, dwell_secs: 0 },
      adapters: { format: state === "adapter-repair" ? 2 : 1, artifacts: [] },
    },
  };
  const status = {
    queued: 0, next_retry_at: null, overflow_count: 0, rejected_count: 0, last_error: null,
    retry_error: null, policy_expires_at: null, pause_pending: false, pause_token: null,
    pending_removals: [], live_activity: 0, host_error: null, delivered_batches: 0,
  };
  fixture.workerStatus = status;
  function reply(request) {
    switch (request.type) {
      case "hello": return { type: "hello_result", host_version: "fixture", configured, config_issue: null };
      case "get_config": return {
        type: "config_result", revision: fixture.revision, config: fixture.config, config_issue: null,
        agent_status: { state: configured ? "ready" : "not_set_up", harness_version: configured ? "1.0" : null, message: null },
        adapter_issue: fixture.adapterIssue, adapter_warnings: [], adopted_state: { issue: null, revision: "missing", warnings: [], full: false },
      };
      case "get_adapter_repair": return {
        type: "adapter_repair_loaded", revision: fixture.revision,
        original_toml: '[adapters]\nformat = 2\nfuture_mode = "semantic"\n',
      };
      case "replace_adapters":
        if (request.expected_revision !== fixture.revision) return { type: "error", code: "conflict", message: "Settings changed." };
        fixture.config.adapters = request.document;
        fixture.adapterIssue = null;
        fixture.revision = String(Number(fixture.revision) + 1);
        return { type: "config_updated", config: fixture.config, revision: fixture.revision };
      case "get_artifact": return { type: "artifact_result", recognized: false, artifact: null, summary };
      case "load_note": return {
        type: "note_loaded", exists: false, revision: "", title: "", body: "",
        key: { kind: "url", value: request.url }, older_notes: [],
      };
      case "save_note": return { type: "note_saved", revision: "saved", key: request.expected_key, outcome: "created", relative_path: "notes/fixture.md" };
      case "get_site_restore": return { type: "site_restore_loaded", origin: request.origin, revision: fixture.revision, adapters: { format: 1, artifacts: [] } };
      case "check_capture_site": return { type: "capture_site_checked", revision: fixture.revision };
      case "get_agent_env": return { type: "agent_env_listed", names: [], problem: null };
      case "list_artifacts": return { type: "artifact_list_result", items: [], artifact_types: [], truncated: false };
      case "worklog_choices": return {
        type: "worklog_choices_result", start_date: request.start_date, end_date: request.end_date,
        choices: (request.page?.offset ? ["Later work"] : ["Release update", "Private research"]).map((title, n) => ({
          artifact_id: `doc:${(request.page?.offset ?? 0) + n}`, local_date: request.start_date,
          text: `${title} — focused 2 min · opened 1×`, url: `https://work.example/${n}`,
        })), truncated: Boolean(fixture.worklogPages && request.page?.offset === 0), gaps: [], gaps_truncated: false,
        ...(request.page ? { page: { snapshot: "a".repeat(64), offset: request.page.offset,
          next_offset: fixture.worklogPages && request.page.offset === 0 ? 2 : null, omitted: false } } : {}),
      };
      case "worklog_focus_choices": {
        const session = { id: "10000000-0000-4000-8000-000000000001", name: "Release", revision: 1 };
        const selected = request.focus_session_id === session.id ? session : null;
        return {
          type: "worklog_focus_choices_result", start_date: request.start_date, end_date: request.end_date,
          focus_session_id: request.focus_session_id, sessions: [session], sessions_truncated: false,
          selected_session: selected, name_changed: false,
          choices: selected ? [{ artifact_id: "doc:0", local_date: request.start_date,
            text: "Release update — focused 2 min · opened 1×", url: "https://work.example/0" }] : [],
          truncated: false, gaps: [], gaps_truncated: false,
        };
      }
      case "worklog_report": return {
        type: "worklog_reported", start_date: request.start_date, end_date: request.end_date,
        text: "No recorded artifacts.", artifact_count: 0, gaps: [], gaps_truncated: false,
      };
      case "list_runs": return fixture.listError
        ? { type: "error", code: "internal", message: "Fixture task listing failed." }
        : { type: "runs_listed", runs: request.kind === "summary" ? [] : fixture.runs, more: false };
      case "get_run": {
        fixture.getRunReads++;
        const run = fixture.runs.find((run) => run.run_id === request.run_id);
        return { type: "run_fetched", run, stream_text: "", next_offset: 0, more: false };
      }
      case "save_task_note":
        return { type: "task_note_saved", run_id: request.run_id, already_saved: false };
      case "cancel_run": {
        const run = fixture.runs.find((run) => run.run_id === request.run_id);
        Object.assign(run, { state: "canceled", finished_at: new Date().toISOString() });
        return { type: "run_updated", run };
      }
      case "discover_harnesses": return { type: "harnesses_discovered", harnesses: [{ harness: "codex", binary: "/fixture/codex", version: "1.0", problem: null }] };
      case "set_up_harness":
        fixture.revision = String(Number(fixture.revision) + 1);
        return { type: "config_updated", config: fixture.config, revision: fixture.revision };
      case "choose_folder": return { type: "error", code: "cancelled", message: "Fixture picker canceled." };
      case "refresh_artifacts": return { type: "artifacts_refreshed", written: 0, unchanged: 0, conflicts: 0, conflict_paths: [] };
      case "open_path": return { type: "path_opened" };
      case "list_task_commands": return { type: "task_commands_listed", revision: fixture.revision,
        commands: [{ name: "actions", description: "Extract action items" }], issue: null };
      case "get_task_command_settings": return { type: "task_command_settings", revision: fixture.revision,
        original_toml: '[[commands]]\nname = "actions"\ndescription = "Extract actions"\nprompt = "Return a checklist."\n',
        commands: [{ name: "actions", description: "Extract actions", prompt: "Return a checklist." }], issue: null };
      case "save_task_commands":
        fixture.revision = String(Number(fixture.revision) + 1);
        return { type: "config_updated", config: fixture.config, revision: fixture.revision };
      case "start_task":
      case "start_run": return { type: "error", code: "service_unavailable", message: "Fixture harness is offline." };
      case "update_config":
        fixture.config = request.config;
        fixture.revision = String(Number(fixture.revision) + 1);
        return { type: "config_updated", config: fixture.config, revision: fixture.revision };
      default: throw new Error(`Unmocked native request: ${request.type}`);
    }
  }
  window.chrome = {
    runtime: {
      id: "fixture", onMessage: event(), lastError: undefined,
      getManifest: () => ({ options_ui: { page: "options.html" } }),
      getURL: (path) => `${location.origin}/${path}`,
      getContexts: async () => [],
      openOptionsPage: async () => { fixture.actions.push("settings"); },
      connectNative() {
        const port = { onMessage: event(), onDisconnect: event(), disconnect() {} };
        port.postMessage = (request) => {
          fixture.requests.push(structuredClone(request));
          setTimeout(() => {
            if (fixture.state === "disconnected") {
              chrome.runtime.lastError = { message: "Fixture native host unavailable." };
              port.onDisconnect.fire();
              chrome.runtime.lastError = undefined;
              return;
            }
            const response = fixture.state === "error" && pageName !== "tasks" && request.type === "get_config"
              ? { type: "error", code: "invalid_config", message: "Fixture settings need repair." }
              : reply(request);
            port.onMessage.fire(structuredClone({ ...response, request_id: request.request_id, protocol_version: request.protocol_version }));
          }, 0);
        };
        fixture.ports.push(port);
        return port;
      },
      async sendMessage(request) {
        if (request.kind === "change_focus") {
          const current = status.focus_session ?? null;
          if (JSON.stringify(request.expected) !== JSON.stringify(current)) {
            return { ok: false, value: null, error: "The focus session changed in another panel." };
          }
          status.focus_session = request.name === null ? null : {
            id: current?.id ?? "00000000-0000-4000-8000-000000000077",
            name: request.name, revision: (current?.revision ?? 0) + 1,
          };
        }
        return { ok: true, value: request.kind === "get_site_patterns" ? [] : status };
      },
    },
    storage: { local: area("local"), session: area("session"), onChanged: storageChanged },
    permissions: {
      contains: async (permission) => permission.permissions?.includes("notifications") ? fixture.taskNotificationsEnabled === true : true,
      request: async () => { fixture.taskNotificationsEnabled = true; fixture.actions.push("enable-task-notifications"); return true; },
      remove: async () => { fixture.taskNotificationsEnabled = false; fixture.actions.push("disable-task-notifications"); return true; },
      onAdded: event(), onRemoved: event(),
    },
    tabs: {
      query: async () => [{ id: 1, incognito: false, url: pageUrl, title: "Work item" }],
      create: async (properties) => { fixture.actions.push(properties.url); return { id: 2 }; },
      onActivated: event(), onUpdated: event(), onReplaced: event(),
    },
    windows: { getCurrent: async () => ({ id: 1 }) },
    sidePanel: { open: async () => { fixture.actions.push("side-panel"); } },
    scripting: { executeScript: async (injection) => {
      if (injection.func?.name === "pageHasSelection") return [{ frameId: 0, result: false }];
      if (injection.args?.[1] === true) return [{ frameId: 0, result: { current: "/browse/{key}", links: [] } }];
      if (injection.files) return [{ frameId: 0 }];
      return [{ frameId: 0, result: { title: "Work item", content: "Fixture article. ".repeat(40), selection: null, truncated: false, readable: true, href: pageUrl } }];
    } },
  };
  Object.defineProperty(navigator, "clipboard", { value: { writeText: async (text) => {
    fixture.copied.push(text);
    fixture.actions.push("copy");
  } } });
}

export async function browserFixture(t, pageName, state = "configured", options = {}) {
  const browser = await a11yBrowser(t);
  if (!browser) return null;
  t.after(() => browser.close());
  const context = await browser.newContext({ colorScheme: options.colorScheme ?? "light", viewport: { width: pageName === "panel" ? 420 : 1100, height: 1000 }, serviceWorkers: "block" });
  const blocked = [];
  await context.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.origin === ORIGIN && url.pathname === "/favicon.ico") {
      await route.fulfill({ status: 204 });
      return;
    }
    if (url.origin !== ORIGIN || !/^\/[a-z0-9-]+(?:\.[a-z0-9-]+)*\.(html|js|css)$/.test(url.pathname)) {
      blocked.push(url.href);
      await route.abort();
      return;
    }
    const body = await readFile(new URL(`../../dist${url.pathname}`, import.meta.url));
    const contentType = url.pathname.endsWith(".js") ? "text/javascript" : url.pathname.endsWith(".css") ? "text/css" : "text/html";
    await route.fulfill({ body, contentType });
  });
  const page = await context.newPage();
  page.setDefaultTimeout(10_000);
  const errors = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(installChrome, { pageName, state, runs: options.runs ?? [task(1), task(2, "failed"), task(3, "canceled")], pageUrl: PAGE_URL, summary: options.summary ?? null });
  await page.goto(`${ORIGIN}/${pageName}.html`);
  const status = page.locator(pageName === "tasks" ? "#tasks-status" : "#status");
  if (state === "disconnected") await status.filter({ hasText: /unavailable|Could not list tasks/ }).waitFor();
  else if (state === "error") await status.filter({ hasText: /settings|repair|Fixture task listing failed/ }).waitFor();
  else if (pageName === "tasks") {
    await page.locator(state === "setup" ? "#tasks-status-settings" : "#tasks-layout").waitFor();
    if (state !== "setup") await page.locator("#tasks-detail .task-detail-instruction").waitFor();
  } else if (pageName === "panel") {
    await page.locator(state === "setup" ? "#setup-warning" : "#note-body:enabled").waitFor();
  } else await page.locator("#choose-folder:enabled").waitFor();
  t.after(() => {
    assert.deepEqual(errors, [], "product modules must execute without browser errors");
    assert.deepEqual(blocked, [], "all browser resources must be local fixtures");
  });
  return page;
}

export function seriousViolations(results) {
  return results.violations.filter((violation) => ["critical", "serious"].includes(violation.impact));
}

export function assertAccessible(results) {
  assert.deepEqual(seriousViolations(results).map(({ id, impact, nodes }) => ({ id, impact, nodes: nodes.map(({ target, failureSummary }) => ({ target, failureSummary })) })), []);
}

export async function audit(t, page, label) {
  const results = await new AxeBuilder({ page }).analyze();
  // Incomplete checks are visible even when no serious violation is found.
  // These need human review and must never be reported as automated passes.
  t.diagnostic(JSON.stringify({ state: label, violations: results.violations.map(({ id, impact }) => ({ id, impact })), manual_review: results.incomplete.map(({ id, help, helpUrl, nodes }) => ({ id, help, helpUrl, targets: nodes.map((node) => node.target) })) }));
  assertAccessible(results);
  return results;
}

/**
 * Reach by Tab, assert visible keyboard focus, and activate by Enter.
 * For roving-tabindex composites, Tab reaches the selected child only;
 * use the composite's arrow keys to reach its other tabs/options.
 */
export async function activate(page, target, key = "Enter") {
  const locator = typeof target === "string" ? page.locator(target) : target;
  for (let n = 0; n < 150; n++) {
    if (await locator.evaluate((element) => element === document.activeElement)) {
      const focus = await locator.evaluate((element) => {
        const style = getComputedStyle(element);
        return { visible: element.matches(":focus-visible"), outline: style.outlineStyle, width: parseFloat(style.outlineWidth) };
      });
      assert.equal(focus.visible, true);
      assert.notEqual(focus.outline, "none");
      assert.ok(focus.width >= 2);
      if (key !== null) await page.keyboard.press(key);
      return;
    }
    await page.keyboard.press("Tab");
  }
  assert.fail(`Could not reach ${target} using Tab`);
}

export async function focused(page, selector) {
  assert.equal(await page.locator(selector).evaluate((node) => node === document.activeElement), true, `${selector} should hold focus`);
}

/**
 * Change a focused, closed <select>'s value the way a keyboard user would
 * with Home/End/ArrowDown/ArrowUp. `which` is "first", "last", or a signed
 * step from the option index selected now (matching the number of
 * ArrowDown/ArrowUp presses a user would need).
 *
 * On Linux Chrome (CI's Accessibility job) those keys cycle a closed
 * <select>'s value and fire `change` in-page, with no popup, so real key
 * presses there keep exercising actual keyboard-selection coverage.  macOS
 * Chrome instead opens a native, OS-drawn popup on the same keys — confirmed
 * on both headless and a visible window: a synthetic Home/End/ArrowDown CDP
 * key event leaves the value unchanged — so Chrome DevTools Protocol's
 * synthetic key events never reach it and no `change` fires. There,
 * selectOption() fires the same `input`/`change` events a real selection
 * would and leaves the control focused, exercising the app's change
 * handling without depending on a popup CDP cannot drive.
 */
export async function selectByKeyboard(page, target, which) {
  const locator = typeof target === "string" ? page.locator(target) : target;
  if (process.platform !== "darwin") {
    const key = which === "first" ? "Home" : which === "last" ? "End" : which >= 0 ? "ArrowDown" : "ArrowUp";
    for (let n = 0; n < (typeof which === "number" ? Math.abs(which) : 1); n++) await page.keyboard.press(key);
    await page.keyboard.press("Enter");
    return;
  }
  const { count, current } = await locator.evaluate((element) => ({ count: element.options.length, current: element.selectedIndex }));
  const index = which === "first" ? 0 : which === "last" ? count - 1 : Math.max(0, Math.min(count - 1, current + which));
  await locator.selectOption({ index });
}

/** Composite entry: only the selected panel tab participates in Tab order. */
export async function enterPanelTab(page, id) {
  assert.ok(["tab-page", "tab-recent"].includes(id));
  await activate(page, page.locator('[role="tab"][aria-selected="true"]'), null);
  await page.keyboard.press(id === "tab-page" ? "Home" : "End");
  await focused(page, `#${id}`);
  assert.equal(await page.locator(`#${id}`).getAttribute("aria-selected"), "true");
  assert.equal(await page.locator(`#${id}`).getAttribute("tabindex"), "0");
}
