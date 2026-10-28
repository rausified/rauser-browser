// Built side-panel tests: artifact card. Run after build:extension;
// the fake DOM, Chrome, host and shared helpers are in ./harness.mjs.
import assert from "node:assert/strict";
import test from "node:test";
import {
  CAPTURING, CONFIG, artifact, install, isResponseShape, noteLoaded,
  outageWithDraft, recognized, settle, statusFor, typeWhy, until,
  withStoredWhy,
} from "./harness.mjs";

// --- Artifact card: "Why I'm here" -------------------------------------------

test("artifact shape validation requires the container fields", () => {
  const result = (value) => isResponseShape(recognized(value));
  const repo = { artifact_id: "github.repo:github.com/acme/widgets", artifact_type: "github.repo",
    title: "acme/widgets", url: "https://github.com/acme/widgets" };
  const child = { artifact_id: "github.pr:github.com/acme/widgets/7", artifact_type: "github.pr",
    title: "Fix", url: "https://github.com/acme/widgets/pull/7", focused_seconds: 60 };
  assert.equal(result(artifact("doc:a", "A")), true);
  assert.equal(result({ ...artifact("doc:a", "A"), within: repo }), true);
  assert.equal(result({ ...artifact("doc:a", "A"), contained: [child] }), true);
  const { contained: _contained, ...missingContained } = artifact("doc:a", "A");
  assert.equal(result(missingContained), false);
  const { within: _within, ...missingWithin } = artifact("doc:a", "A");
  assert.equal(result(missingWithin), false);
  assert.equal(result({ ...artifact("doc:a", "A"), within: { ...repo, url: 7 } }), false);
  assert.equal(result({ ...artifact("doc:a", "A"), contained: [{ ...child, focused_seconds: -1 }] }), false);
  assert.equal(result({ ...artifact("doc:a", "A"), contained: Array(21).fill(child) }), false);
});

test("Save why waits for the opening capture replay, then records", async () => {
  let finishReplay;
  const replay = new Promise((resolve) => { finishReplay = resolve; });
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
    workerReply: async (request) => {
      if (request.kind === "sync_now") await replay;
      return undefined;
    },
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "reviewing for launch");
  // The replay is still running, so the click would be ignored: Save says so
  // by being disabled rather than doing nothing.
  assert.equal(page.el("save-why").disabled, true);

  finishReplay();
  await until(() => page.el("save-why").disabled === false, "Save to enable after the replay");
  page.el("save-why").click();
  await until(() => page.sent("record_why").length === 1, "record_why");
  const [request] = page.sent("record_why");
  assert.equal(request.event.artifact_id, "doc:a");
  assert.equal(request.event.url, "https://docs.example/a");
  assert.equal(request.event.text, "reviewing for launch");
  await settle(200);
  assert.equal(page.el("status").textContent, "Artifact context saved.");
});

test("Save why is disabled until there is text to save", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  await noteLoaded(page);
  await settle();
  assert.equal(page.el("save-why").disabled, true);
  typeWhy(page, "  ");
  assert.equal(page.el("save-why").disabled, true);
  typeWhy(page, "context");
  assert.equal(page.el("save-why").disabled, false);
  typeWhy(page, "");
  assert.equal(page.el("save-why").disabled, true);
});

test("a failed artifact lookup for a new page clears the previous page's card", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => {
      if (request.type !== "get_artifact") return undefined;
      return request.url.endsWith("/a")
        ? recognized(artifact("doc:a", "Doc A"))
        : { type: "error", code: "internal", message: "index busy" };
    },
  });
  await until(() => page.el("artifact-heading").textContent === "Doc A", "page A's card");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "context for A");

  page.followLink("https://other.example/b", "B");
  await until(() => page.sent("get_artifact").some((request) => request.url.endsWith("/b")), "B's lookup");
  await until(() => /unavailable: index busy/.test(page.el("artifact-status").textContent), "the lookup error");
  assert.equal(page.el("artifact-card").hidden, true, "page B still shows page A's card");
  assert.equal(page.el("save-why").disabled, true);
  // Nothing can attach page B's URL to page A's artifact.
  page.el("save-why").dispatch("click");
  await settle();
  assert.equal(page.sent("record_why").length, 0);
  assert.notEqual(page.el("status").textContent, "Artifact context saved.");
});

test("a failed artifact lookup for the same page keeps its card and draft", async () => {
  let fail = false;
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => {
      if (request.type !== "get_artifact") return undefined;
      return fail
        ? { type: "error", code: "internal", message: "index busy" }
        : recognized(artifact("doc:a", "Doc A", "old reason"));
    },
  });
  await until(() => page.el("artifact-why").value === "old reason", "the stored why");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "my draft");
  fail = true;
  page.titleChanged("A (1)");
  await until(() => /unavailable: index busy/.test(page.el("artifact-status").textContent), "the lookup error");
  assert.equal(page.el("artifact-card").hidden, false);
  assert.equal(page.el("artifact-why").value, "my draft");
  assert.equal(page.el("save-why").disabled, false);
});

test("an unsaved Why draft survives a host outage", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING, intercept: withStoredWhy,
  });
  await outageWithDraft(page);
  assert.equal(page.el("artifact-card").hidden, false);
  assert.equal(page.el("artifact-why").value, "my unsaved draft");

  // The host is back; the user follows a link to another URL of the same artifact.
  page.state.hostDead = false;
  page.followLink("https://docs.example/a?view=1", "A");
  await until(() => page.el("artifact-status").textContent === "● Artifact recognized", "the lookup to succeed");
  await settle();
  assert.equal(page.el("artifact-card").hidden, false);
  assert.equal(page.el("artifact-why").value, "my unsaved draft",
    "the unsaved Why draft was replaced after the host outage");
});

test("Save why reports a host outage and records once the host is back", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" }, config: CAPTURING, intercept: withStoredWhy,
  });
  await outageWithDraft(page);
  // The click itself restarts the host, so Save stays available.
  assert.equal(page.el("save-why").disabled, false);
  page.el("save-why").click();
  await until(() => /Could not save artifact context/.test(page.el("status").textContent), "the save failure");
  assert.equal(page.el("artifact-why").value, "my unsaved draft");

  page.state.hostDead = false;
  await until(() => page.el("save-why").disabled === false, "Save to enable again");
  page.el("save-why").click();
  await until(() => page.el("status").textContent === "Artifact context saved.", "the saved status");
  const recorded = page.sent("record_why");
  assert.equal(recorded.at(-1).event.text, "my unsaved draft");
  assert.equal(recorded.at(-1).event.url, "https://docs.example/a");
});

test("Save why is disabled while capture is paused, keeps the draft, and points to Resume capture", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: { ...CONFIG, capture_enabled: false },
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "a draft while paused");
  assert.equal(page.el("save-why").disabled, true);
  assert.equal(page.el("artifact-why").value, "a draft while paused");
  const hint = page.el("why-paused-hint");
  assert.equal(hint.hidden, false);
  assert.match(hint.textContent, /Use Resume capture in settings to turn it back on\./);
  assert.equal(page.sent("record_why").length, 0);
});

test("Save why stays disabled while a local pause waits for the host", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
    // The worker paused, but the host still has capture on.
    workerReply: () => ({ ok: true, value: { ...statusFor(), pause_pending: true } }),
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  await noteLoaded(page);
  await settle();
  typeWhy(page, "a draft while paused");
  assert.equal(page.el("save-why").disabled, true);
  assert.equal(page.el("why-paused-hint").hidden, false);
  // Resume capture is not offered until the host confirms the pause.
  assert.match(page.el("why-paused-hint").textContent, /Use Pause capture to finish saving this setting in the host\./);
  page.el("save-why").dispatch("click");
  await settle();
  assert.equal(page.sent("record_why").length, 0);
  assert.match(page.el("status").textContent, /Use Pause capture to finish/);
});

// --- Artifact card: container and contents -----------------------------------

const INCOMING_SOURCE = {
  artifact_id: "doc:source", artifact_type: "doc", title: "<img src=x onerror=alert(1)> Source",
  url: "http://localhost:8123/docs/source",
};
const INCOMING_TRAIL = { source: INCOMING_SOURCE, transition: "link", last_seen: "2026-10-27" };

test("incoming trail response shapes bound the list and reject unsafe links and unknown transitions", () => {
  const valid = artifact("doc:target", "Target");
  const result = (trails) => isResponseShape(recognized({ ...valid, incoming_trails: trails }));
  assert.equal(result([INCOMING_TRAIL]), true);
  assert.equal(result(Array(5).fill(INCOMING_TRAIL)), true);
  assert.equal(result(Array(6).fill(INCOMING_TRAIL)), false);
  const { incoming_trails: _trails, ...missing } = valid;
  assert.equal(isResponseShape(recognized(missing)), false);
  for (const transition of ["mentions", "co-focused", "within", "forward_back", "invented"]) {
    assert.equal(result([{ ...INCOMING_TRAIL, transition }]), false, transition);
  }
  for (const url of ["javascript:alert(1)", "data:text/html,hi", "file:///tmp/note", "/relative", "not a URL"]) {
    assert.equal(result([{ ...INCOMING_TRAIL, source: { ...INCOMING_SOURCE, url } }]), false, url);
  }
  assert.equal(result([{ ...INCOMING_TRAIL, source: null }]), false);
  assert.equal(result([{ ...INCOMING_TRAIL, last_seen: "<script>" }]), false);
});

test("incoming source links open the local source fixture and clear when the page changes", async () => {
  const target = { ...artifact("doc:target", "Target"), incoming_trails: [INCOMING_TRAIL,
    { ...INCOMING_TRAIL, transition: "form_submit" }] };
  const page = await install({
    tab: { id: 1, url: "http://localhost:8123/docs/target", title: "Target" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact"
      ? recognized(request.url === INCOMING_SOURCE.url ? artifact("doc:source", "Source") : target)
      : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the target card");
  const rows = page.el("artifact-incoming-list").children;
  assert.equal(rows.length, 2);
  assert.equal(rows[0].textContent, `Link navigation from ${INCOMING_SOURCE.title} · last recorded 2026-10-27`);
  assert.equal(rows[1].textContent, `Form submission navigation from ${INCOMING_SOURCE.title} · last recorded 2026-10-27`);
  assert.equal(page.el("artifact-incoming-empty").hidden, true);
  const link = rows[0].querySelector("a");
  assert.equal(link.href, INCOMING_SOURCE.url);
  assert.equal(link.target, "_blank");
  assert.equal(link.rel, "noopener noreferrer");
  // The fake DOM has no default anchor action. Follow its explicit URL as
  // Chrome does when the newly opened source tab becomes active.
  page.navigate(link.href, 2);
  await until(() => page.el("artifact-heading").textContent === "Source", "the source fixture card");
  assert.equal(page.el("artifact-incoming-list").children.length, 0);
  assert.equal(page.el("artifact-incoming-list").hidden, true);
  assert.equal(page.el("artifact-incoming-empty").hidden, false);
  assert.equal(page.calls("permissions.request"), 0);
  page.navigate("chrome://extensions", 3);
  await until(() => page.el("artifact-card").hidden, "no card on a Chrome page");
  assert.equal(page.el("artifact-incoming-list").children.length, 0);
});

const REPO = {
  artifact_id: "github.repo:github.com/acme-corp/widgets",
  artifact_type: "github.repo",
  title: "acme-corp/widgets",
  url: "https://github.com/acme-corp/widgets",
};

test("artifact card links a PR to its repository", async () => {
  const pr = { ...artifact("github.pr:github.com/acme-corp/widgets/7", "Fix the thing"), within: REPO };
  const page = await install({
    tab: { id: 1, url: "https://github.com/acme-corp/widgets/pull/7", title: "Fix the thing" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(pr) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  assert.equal(page.el("artifact-within").hidden, false);
  assert.equal(page.el("artifact-within").textContent, "In acme-corp/widgets");
  const link = page.el("artifact-within").querySelector("a");
  assert.equal(link.href, REPO.url);
});

test("repository card lists contained work", async () => {
  const pr = { artifact_id: "github.pr:github.com/acme-corp/widgets/7", artifact_type: "github.pr",
    title: "Fix the thing", url: "https://github.com/acme-corp/widgets/pull/7", focused_seconds: 125 };
  const issue = { artifact_id: "github.issue:github.com/acme-corp/widgets/3", artifact_type: "github.issue",
    title: "Flaky test", url: "https://github.com/acme-corp/widgets/issues/3", focused_seconds: 40 };
  const repo = { ...artifact("github.repo:github.com/acme-corp/widgets", "acme-corp/widgets"), contained: [pr, issue] };
  const page = await install({
    tab: { id: 1, url: REPO.url, title: "acme-corp/widgets" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(repo) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  assert.equal(page.el("artifact-contained").hidden, false);
  assert.equal(page.el("artifact-contained-heading").textContent, "Worked on inside");
  const rows = page.el("artifact-contained-list").children;
  assert.equal(rows.length, 2, "one row per contained artifact");
  const firstLink = rows[0].querySelector("a");
  assert.equal(firstLink.textContent, "Fix the thing · 2 min");
  assert.equal(firstLink.href, pr.url);
  const secondLink = rows[1].querySelector("a");
  assert.equal(secondLink.textContent, "Flaky test · 40 sec");
  assert.equal(secondLink.href, issue.url);
});

test("card hides container rows when absent", async () => {
  const page = await install({
    tab: { id: 1, url: "https://docs.example/a", title: "A" },
    config: CAPTURING,
    intercept: (request) => request.type === "get_artifact" ? recognized(artifact("doc:a", "Doc A")) : undefined,
  });
  await until(() => page.el("artifact-card").hidden === false, "the artifact card");
  assert.equal(page.el("artifact-within").hidden, true);
  assert.equal(page.el("artifact-within").textContent, "");
  assert.equal(page.el("artifact-contained").hidden, true);
  assert.equal(page.el("artifact-contained-list").children.length, 0);
});
