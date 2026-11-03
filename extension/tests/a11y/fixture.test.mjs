// Transport fixtures still get checked when the browser is sandbox-blocked:
// wire-invalid replies must not turn a configured page into an error-state test.
import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { isResponseShape } from "../../dist/protocol-shape.js";
import { assertAccessible, installChrome, PAGE_URL, task } from "./fixture.mjs";

test("the offline host fixture emits valid wire replies for every exercised request", async () => {
  const scope = vm.createContext({ structuredClone, setTimeout, navigator: {}, location: { origin: "https://brauser-a11y.invalid" } });
  scope.window = scope;
  vm.runInContext(`(${installChrome.toString()})(${JSON.stringify({ state: "configured", runs: [task(1)], pageUrl: PAGE_URL })})`, scope);
  const port = scope.chrome.runtime.connectNative();
  const requests = [
    { type: "hello" }, { type: "get_config" }, { type: "get_artifact" },
    { type: "load_note", url: PAGE_URL }, { type: "save_note", expected_key: { kind: "url", value: PAGE_URL } },
    { type: "get_site_restore", origin: "https://acme.atlassian.net" }, { type: "check_capture_site" },
    { type: "get_agent_env" }, { type: "list_artifacts" },
    { type: "get_adapter_repair", expected_revision: "1" },
    { type: "replace_adapters", expected_revision: "1", document: { format: 1, artifacts: [] } },
    { type: "worklog_report", start_date: "2026-11-02", end_date: "2026-11-02" },
    { type: "list_runs" }, { type: "get_run", run_id: task(1).run_id },
    { type: "cancel_run", run_id: task(1).run_id }, { type: "discover_harnesses" },
    { type: "set_up_harness" },
    { type: "choose_folder" }, { type: "refresh_artifacts" }, { type: "open_path" },
    { type: "start_task" }, { type: "start_run" },
    { type: "update_config", config: scope.fixture.config },
  ];
  const responses = new Map();
  port.onMessage.addListener((response) => responses.get(response.request_id)(response));
  for (const [index, request] of requests.entries()) {
    const request_id = `fixture-${index}`;
    const response = await new Promise((resolve) => {
      responses.set(request_id, resolve);
      port.postMessage({ ...request, request_id, protocol_version: 8 });
    });
    assert.equal(isResponseShape(response), true, `${request.type}: ${JSON.stringify(response)}`);
  }
});

test("the axe gate refuses serious/critical violations without treating incomplete checks as passes", () => {
  for (const impact of ["critical", "serious"]) {
    assert.throws(() => assertAccessible({ violations: [{ id: "seed", impact, nodes: [{ target: ["#seed"] }] }] }));
  }
  assert.doesNotThrow(() => assertAccessible({ violations: [], incomplete: [{ id: "manual-review" }] }));
});
