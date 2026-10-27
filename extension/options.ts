import type {
  AdapterDocument, FolderPurpose, HarnessAdapter, HarnessOffer, StorageConfig,
} from "../protocol/ts/generated.js";
import { HOME_LAYOUT, isHttpUrl, type WorkerStatus } from "./model.js";
import {
  ConfigSession,
  capturePaused,
  captureStateText,
  describe,
  element,
  setupProblem,
  siteOrigins,
  withConfigMutationLock,
  worker,
} from "./settings.js";
import { HostError, PROTOCOL_VERSION, newRequestId } from "./native.js";
import { hostBuildText } from "./build-info.js";
import {
  SETUP_WARNING,
  agentStateText,
  harnessFoundText,
  harnessOfferNote,
} from "./agent.js";
import {
  adapterLabel,
  adapterOrigin,
  exampleUrl,
  newAdapter,
  overlapText,
  parseAdapterJson,
  previewDocument,
  validateAdapterDocument,
  type AdapterObject,
} from "./adapter-review.js";
import {
  hasAdapter,
  parseGitHubOrganizations,
  starterOffers,
  type StarterOffer,
} from "./starter-adapters.js";

const status = element<HTMLDivElement>("status");
const buildId = element<HTMLSpanElement>("build-id");
const hostBuild = element<HTMLSpanElement>("host-build");

function showHostBuild(hostVersion: string): void {
  hostBuild.textContent = hostBuildText(buildId.textContent ?? "", hostVersion);
}
const folderPath = element<HTMLOutputElement>("folder-path");
const chooseFolderButton = element<HTMLButtonElement>("choose-folder");
const siteOrigin = element<HTMLInputElement>("site-origin");
const pauseButton = element<HTMLButtonElement>("pause-capture");
const resumeButton = element<HTMLButtonElement>("resume-capture");
const sitesList = element<HTMLUListElement>("sites-list");
const adapterOverlaps = element<HTMLDivElement>("adapter-overlaps");
const adapterList = element<HTMLDivElement>("adapter-list");
const newAdapterButton = element<HTMLButtonElement>("new-adapter");
const resetAdaptersButton = element<HTMLButtonElement>("reset-adapters");
const refreshArtifactsButton = element<HTMLButtonElement>("refresh-artifacts");
const starterGallery = element<HTMLDivElement>("starter-gallery");
const adapterEditor = element<HTMLDivElement>("adapter-editor");
const adapterEditorHeading = element<HTMLHeadingElement>("adapter-editor-heading");
const adapterJson = element<HTMLTextAreaElement>("adapter-json");
const adapterValidation = element<HTMLOutputElement>("adapter-validation");
const adapterPreviewUrl = element<HTMLInputElement>("adapter-preview-url");
const adapterPreviewTitle = element<HTMLInputElement>("adapter-preview-title");
const adapterPreview = element<HTMLUListElement>("adapter-preview");
const saveAdapterButton = element<HTMLButtonElement>("save-adapter");
const cancelAdapterButton = element<HTMLButtonElement>("cancel-adapter");
const agentState = element<HTMLParagraphElement>("agent-state");
const detectButton = element<HTMLButtonElement>("detect-harnesses");
const offersList = element<HTMLUListElement>("harness-offers");
const envList = element<HTMLUListElement>("harness-env");
const knowledgeBasePath = element<HTMLOutputElement>("kb-path");
const chooseKnowledgeBaseButton = element<HTMLButtonElement>("choose-kb");
const knowledgeBaseUseNotesButton = element<HTMLButtonElement>("kb-use-notes");
const setupButton = element<HTMLButtonElement>("setup-harness");
const harnessWarning = element<HTMLParagraphElement>("harness-warning");
const removeHarnessButton = element<HTMLButtonElement>("remove-harness");

let picker: { path: string; token: string } | null = null;
let busy = false;
let reloadTimer: ReturnType<typeof setTimeout> | null = null;
// What Detect harnesses last found. Setup names one by harness_id, and the
// host detects and checks it again before running anything.
let offers: HarnessOffer[] = [];
let selectedHarnessId: string | null = null;
const envChoices = new Map<string, boolean>();
type AdapterEdit =
  | { kind: "new" }
  | { kind: "existing"; identity: string; snapshot: string };

let adapterEdit: AdapterEdit | null = null;
let githubScopeText = "";

// The host binds a folder selection to the revision it was made at, so any
// save, from this page or another, makes it unusable.
const session = new ConfigSession(renderConfig, () => {
  picker = null;
});

// The header names the host process that answered. Another one, possibly
// from a rebuilt binary, starts on the next call after it exits.
session.host.onHostExit(() => { hostBuild.textContent = ""; });
session.host.onHostRestart(() => {
  void session.hello().then((hello) => showHostBuild(hello.host_version), () => undefined);
});

// A folder selection's token lives only in the host process that issued it.
// After that process exits, the next call starts a new one that would refuse
// it, so the folder must be chosen again.
session.host.onHostExit(() => {
  if (picker === null) return;
  picker = null;
  renderConfig();
  show("The native host restarted, so the folder selection was lost. Choose the folder again.", true);
});

harnessWarning.textContent = SETUP_WARNING;

function show(message: string, warning = false): void {
  status.textContent = message;
  status.classList.toggle("warning", warning);
}

function showConfigState(): void {
  const config = session.config;
  const policyIssue = session.configIssue ?? session.adapterIssue;
  const problem = setupProblem(config, policyIssue);
  if (policyIssue) show(problem!, true);
  else if (problem) show(`${problem} Choose a folder, then add an adapter for each site you want logged.`);
  else if (!config) return;
  else if (!siteOrigins(config.adapters).length) {
    show("No site has an adapter yet. Enter a site address, then save a starter or add an adapter to start capturing.");
  }
  else if (session.adapterWarnings.length) {
    const count = session.adapterWarnings.length;
    show(
      `${captureStateText(config)} ${count} adapter rule overlap${count === 1 ? " needs" : "s need"} review. The earlier configured rule wins.`,
      true,
    );
  }
  else if (capturePaused(config, null)) show("Capture is paused. Click Resume capture to turn it back on.");
  else show(captureStateText(config));
}

/** The origin the Site address box names, or null until it parses as an
 * HTTP(S) address without credentials. A pasted page URL names its origin. */
function typedOrigin(): string | null {
  const value = siteOrigin.value.trim();
  if (!value) return null;
  try {
    const url = new URL(value);
    return isHttpUrl(url) && !url.username && !url.password ? url.origin : null;
  } catch {
    return null;
  }
}

function updateControls(): void {
  const connected = session.connected;
  // A base-config issue needs folder repair. An adapter-only issue keeps
  // unrelated controls live, but freezes individual adapter edits until the
  // user removes the invalid document as one revisioned update.
  const repairing = session.configIssue !== null;
  const repairingAdapters = session.adapterIssue !== null;
  chooseFolderButton.disabled = !connected || busy;
  pauseButton.disabled = !connected || busy || repairing || session.config?.capture_enabled !== true;
  // The host refuses capture without a home folder, and capture has nothing
  // to record until some site has an adapter.
  resumeButton.disabled = !connected || busy || repairing || repairingAdapters ||
    session.config?.capture_enabled !== false || !session.config.storage ||
    !siteOrigins(session.config.adapters).length;
  // Resume is offered only while a finished setup is paused.
  resumeButton.hidden = !capturePaused(session.config, session.configIssue ?? session.adapterIssue);
  for (const button of sitesList.querySelectorAll("button")) {
    button.disabled = !connected || busy || repairing || repairingAdapters;
  }
  newAdapterButton.disabled = !connected || busy || repairing || repairingAdapters;
  resetAdaptersButton.disabled = !connected || busy || repairing || !repairingAdapters;
  refreshArtifactsButton.disabled = !connected || busy || repairing || !session.config?.storage;
  for (const button of adapterList.querySelectorAll("button")) {
    button.disabled = !connected || busy || repairing || repairingAdapters || button.dataset.boundary === "true";
  }
  for (const button of starterGallery.querySelectorAll<HTMLButtonElement>("button")) {
    button.disabled = !connected || busy || repairing || repairingAdapters || button.dataset.available !== "true";
  }
  saveAdapterButton.disabled = !connected || busy || repairing || repairingAdapters ||
    adapterCandidate() === null;
  cancelAdapterButton.disabled = busy;
  const idle = connected && !busy;
  detectButton.disabled = !idle || session.configIssue !== null;
  setupButton.disabled = !idle || !session.config?.storage || selectedOffer() === null;
  removeHarnessButton.disabled = !idle || !session.config?.agent;
  // The knowledge base is stored with the home folder, so it needs one.
  const folderReady = idle && !repairing && !!session.config?.storage;
  chooseKnowledgeBaseButton.disabled = !folderReady;
  knowledgeBaseUseNotesButton.disabled = !folderReady || !session.config?.storage?.knowledge_base_dir;
  for (const input of offersList.querySelectorAll<HTMLInputElement>("input")) {
    input.disabled = !idle || input.dataset.ready !== "true";
  }
  for (const input of envList.querySelectorAll<HTMLInputElement>("input")) {
    input.disabled = !idle || input.dataset.required === "true";
  }
}

function harnessName(adapter: HarnessAdapter): string {
  return adapter === "claude_code" ? "Claude Code" : "Codex";
}

function clearOffers(): void {
  offers = [];
  selectedHarnessId = null;
  envChoices.clear();
}

function selectedOffer(): HarnessOffer | null {
  return offers.find((offer) => offer.refusal === null && offer.harness_id === selectedHarnessId) ?? null;
}

function selectOffer(harnessId: string | null): void {
  selectedHarnessId = harnessId;
  envChoices.clear();
  // Credentials the host has are proposed; the user can untick any of them.
  for (const env of selectedOffer()?.env_optional ?? []) envChoices.set(env.name, env.present);
}

function renderAgent(): void {
  const config = session.config;
  const state = [agentStateText(session.agentStatus)];
  if (config?.agent) state.push(`Harness: ${config.agent.binary}.`);
  if (config?.storage?.summaries_dir) state.push(`Summaries folder: ${config.storage.summaries_dir}.`);
  agentState.textContent = state.join(" ");
  const storage = config?.storage ?? null;
  knowledgeBasePath.textContent = storage?.knowledge_base_dir ??
    (storage ? `Same as the home folder (${storage.root})` : "Same as the home folder");

  offersList.replaceChildren();
  if (!offers.length) {
    const item = document.createElement("li");
    item.textContent = "Not detected yet.";
    offersList.append(item);
  }
  for (const [index, offer] of offers.entries()) {
    const ready = offer.refusal === null;
    const item = document.createElement("li");
    item.dataset.adapter = offer.adapter;
    item.dataset.ready = String(ready);
    if (offer.real_path !== null) item.dataset.realPath = offer.real_path;
    const label = document.createElement("label");
    const radio = document.createElement("input");
    radio.type = "radio";
    radio.name = "harness-offer";
    radio.id = `harness-offer-${index}`;
    radio.value = offer.harness_id;
    radio.dataset.ready = String(ready);
    radio.checked = ready && offer.harness_id === selectedHarnessId;
    radio.addEventListener("change", () => {
      selectOffer(offer.harness_id);
      renderConfig();
    });
    const version = offer.version ? ` ${offer.version}` : "";
    const runs = offer.real_path && offer.real_path !== offer.binary ? ` (runs ${offer.real_path})` : "";
    const text = document.createElement("span");
    text.textContent = `${harnessName(offer.adapter)}${version}: ${offer.binary}${runs}`;
    label.append(radio, text);
    item.append(label);
    const note = document.createElement("small");
    note.textContent = ` ${harnessOfferNote(offer)}`;
    item.append(note);
    offersList.append(item);
  }

  envList.replaceChildren();
  const offer = selectedOffer();
  if (!offer) {
    const item = document.createElement("li");
    item.textContent = "Detect a harness first.";
    envList.append(item);
  } else {
    const names = [
      ...offer.env_required.map((name) => ({ name, required: true, present: true })),
      ...offer.env_optional.map((env) => ({ name: env.name, required: false, present: env.present })),
    ];
    for (const env of names) {
      const item = document.createElement("li");
      const label = document.createElement("label");
      const box = document.createElement("input");
      box.type = "checkbox";
      box.dataset.env = env.name;
      box.dataset.required = String(env.required);
      box.checked = env.required || envChoices.get(env.name) === true;
      box.addEventListener("change", () => { envChoices.set(env.name, box.checked); });
      const text = document.createElement("span");
      text.textContent = env.required
        ? `${env.name} (always passed)`
        : `${env.name} (${env.present ? "set" : "not set"} in the native host)`;
      label.append(box, text);
      item.append(label);
      envList.append(item);
    }
  }
}

function objectValue(value: unknown): AdapterObject | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? value as AdapterObject
    : null;
}

function textValue(value: unknown, fallback = "Not configured"): string {
  return typeof value === "string" && value.length > 0 ? value : fallback;
}

function ruleDescription(raw: unknown, index: number): string {
  const rule = objectValue(raw);
  if (!rule) return `Rule ${index + 1}: invalid rule`;
  const parts = [`Rule ${index + 1}: ${textValue(rule.path, "missing path")}`];
  const query = objectValue(rule.query);
  if (query && Object.keys(query).length) {
    parts.push(`query ${Object.entries(query).map(([key, value]) => `${key}=${String(value)}`).join(", ")}`);
  }
  if (Array.isArray(rule.absent) && rule.absent.length) parts.push(`without ${rule.absent.join(", ")}`);
  if (typeof rule.mode === "string") parts.push(`mode ${rule.mode}`);
  return parts.join(" · ");
}

function scopeDescription(adapter: AdapterObject): string {
  const scope = objectValue(adapter.scope);
  if (scope) {
    const entries = Object.entries(scope).map(([name, values]) =>
      `${name}: ${Array.isArray(values) ? values.join(", ") : String(values)}`);
    return `Scope: ${entries.join("; ") || "empty"}.`;
  }
  if (adapter.unscoped === "acknowledged") {
    return "Scope: every matching account in this Chrome profile (explicitly acknowledged).";
  }
  return "Scope: this origin is not classified as a shared-account origin.";
}

function titleDescription(adapter: AdapterObject): string {
  if (Array.isArray(adapter.title)) return `Titles: ${adapter.title.map(String).join("; ")}`;
  if (typeof adapter.title === "string") return `Title: ${adapter.title}`;
  return "Title: uses the whole tab title.";
}

function fieldsDescription(adapter: AdapterObject): string {
  const fields = objectValue(adapter.fields);
  if (!fields || Object.keys(fields).length === 0) return "Tier 1 fields: none.";
  return `Tier 1 fields (M1.5b): ${Object.entries(fields)
    .map(([name, selector]) => `${name} ← ${String(selector)}`).join("; ")}.`;
}

function addText(parent: HTMLElement, tag: string, text: string, className?: string): HTMLElement {
  const child = document.createElement(tag);
  child.textContent = text;
  if (className) child.className = className;
  parent.append(child);
  return child;
}

function offerConfigured(offer: StarterOffer): boolean {
  return hasAdapter(session.config?.adapters.artifacts ?? [], offer);
}

function githubScope(): string[] {
  return parseGitHubOrganizations(githubScopeText) ?? [];
}

function currentStarterOffer(origin: string, key: string): StarterOffer | null {
  return starterOffers(origin, githubScope()).find((offer) => offer.key === key) ?? null;
}

function setStarterButtonState(button: HTMLButtonElement, origin: string, key: string): void {
  const offer = currentStarterOffer(origin, key);
  const configured = offer ? offerConfigured(offer) : false;
  button.dataset.available = String(Boolean(offer?.adapter) && !configured);
  button.textContent = configured ? "Already configured" : "Review starter…";
  const repairing = session.configIssue !== null || session.adapterIssue !== null;
  button.disabled = !session.connected || busy || repairing || button.dataset.available !== "true";
}

function renderStarterGallery(): void {
  starterGallery.replaceChildren();
  // The typed site address first, then the sites that already have adapters.
  const typed = typedOrigin();
  const origins = [...new Set([
    ...(typed ? [typed] : []),
    ...(session.config ? siteOrigins(session.config.adapters) : []),
  ])];
  const compatible = origins.flatMap((origin) => starterOffers(origin, githubScope())
    .map((offer) => ({ origin, offer })));
  if (!compatible.length) {
    addText(starterGallery, "p", "Enter a Jira Cloud, SharePoint/OneDrive, or GitHub site address above to see starter drafts.");
    return;
  }
  let githubScopeRendered = false;
  for (const { origin, offer } of compatible) {
    const card = document.createElement("article");
    card.className = "starter-card";
    addText(card, "h3", offer.label);
    addText(card, "p", offer.description);
    addText(card, "small", offer.origin);
    if (offer.kind === "github" && !githubScopeRendered) {
      githubScopeRendered = true;
      const label = document.createElement("label");
      label.textContent = "Work organizations (comma-separated)";
      const input = document.createElement("input");
      input.type = "text";
      input.value = githubScopeText;
      input.placeholder = "acme-corp, partner-org";
      input.autocomplete = "off";
      input.spellcheck = false;
      label.append(input);
      card.append(label);
      const guidance = addText(card, "small", "Required: GitHub is shared across accounts, so the starter never captures every organization.");
      guidance.className = parseGitHubOrganizations(githubScopeText) ? "" : "warning";
      input.addEventListener("input", () => {
        githubScopeText = input.value;
        const valid = parseGitHubOrganizations(githubScopeText) !== null;
        guidance.className = valid ? "" : "warning";
        for (const scoped of starterGallery.querySelectorAll<HTMLButtonElement>("button")) {
          if (scoped.dataset.siteOrigin === origin) {
            setStarterButtonState(scoped, origin, scoped.dataset.starterKey ?? "");
          }
        }
      });
    }
    const review = document.createElement("button");
    review.type = "button";
    review.dataset.siteOrigin = origin;
    review.dataset.starterKey = offer.key;
    setStarterButtonState(review, origin, offer.key);
    review.addEventListener("click", () => {
      const selected = currentStarterOffer(origin, offer.key);
      if (!selected?.adapter || offerConfigured(selected)) return;
      beginAdapterDraft(selected.adapter, selected.label);
    });
    card.append(review);
    starterGallery.append(card);
  }
}

function renderAdapters(): void {
  const adapters = session.config?.adapters.artifacts ?? [];
  adapterOverlaps.replaceChildren();
  for (const warning of session.adapterWarnings) {
    addText(adapterOverlaps, "p", overlapText(warning, adapters));
  }

  adapterList.replaceChildren();
  if (!adapters.length) {
    addText(adapterList, "p", "No adapters configured. Enter a site address above, then save a starter or add an adapter.");
  }
  adapters.forEach((adapter, index) => {
    const card = document.createElement("article");
    card.className = "adapter-card";
    addText(card, "h3", adapterLabel(adapter, index));
    addText(card, "p", `${textValue(adapter.origin, "Invalid origin")} · ID ${textValue(adapter.id, "missing")}`);
    const rules = document.createElement("ul");
    rules.className = "plain-list";
    if (Array.isArray(adapter.match)) {
      adapter.match.forEach((rule, ruleIndex) => addText(rules, "li", ruleDescription(rule, ruleIndex)));
    } else {
      addText(rules, "li", "No readable match rules.");
    }
    card.append(rules);
    addText(card, "p", scopeDescription(adapter));
    addText(card, "p", titleDescription(adapter));
    addText(card, "p", fieldsDescription(adapter));

    const actions = document.createElement("div");
    actions.className = "adapter-actions";
    const edit = document.createElement("button");
    edit.type = "button";
    edit.textContent = "Edit and preview";
    edit.addEventListener("click", () => beginAdapterEdit(index));
    actions.append(edit);
    for (const [label, offset] of [["Move up", -1], ["Move down", 1]] as const) {
      const move = document.createElement("button");
      move.type = "button";
      move.className = "secondary";
      move.textContent = label;
      move.dataset.boundary = String(index + offset < 0 || index + offset >= adapters.length);
      move.addEventListener("click", () => void moveAdapter(index, offset));
      actions.append(move);
    }
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "secondary";
    remove.textContent = "Remove";
    remove.setAttribute("aria-label", `Remove ${adapterLabel(adapter, index)}`);
    remove.addEventListener("click", () => void removeAdapter(index));
    actions.append(remove);
    card.append(actions);
    adapterList.append(card);
  });
}

type CandidateResult =
  | { ok: true; adapter: AdapterObject; document: AdapterDocument }
  | { ok: false; message: string };

function adapterIdentity(adapter: AdapterObject): string | null {
  const origin = adapterOrigin(adapter);
  return origin && typeof adapter.type === "string" && typeof adapter.id === "string"
    ? JSON.stringify([origin, adapter.type, adapter.id])
    : null;
}

function adapterSnapshot(value: unknown): string {
  if (Array.isArray(value)) return `[${value.map(adapterSnapshot).join(",")}]`;
  if (value !== null && typeof value === "object") {
    const record = value as Record<string, unknown>;
    return `{${Object.keys(record)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${adapterSnapshot(record[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value) ?? "undefined";
}

function readAdapterCandidate(): CandidateResult {
  if (adapterEdit === null || !session.config) return { ok: false, message: "No adapter is being edited." };
  if (session.config.adapters.format !== 1) {
    return { ok: false, message: `Adapter format ${session.config.adapters.format} cannot be edited by this build.` };
  }
  const parsed = parseAdapterJson(adapterJson.value);
  if (!parsed.ok) return parsed;
  const artifacts = [...session.config.adapters.artifacts];
  if (adapterEdit.kind === "new") artifacts.push(parsed.adapter);
  else {
    const target = adapterEdit;
    const matches = artifacts
      .map((adapter, index) => adapterIdentity(adapter) === target.identity ? index : -1)
      .filter((index) => index >= 0);
    if (matches.length !== 1 || adapterSnapshot(artifacts[matches[0]!]!) !== target.snapshot) {
      return { ok: false, message: "This adapter changed on another settings page. Close this editor and review the current version before editing it again." };
    }
    artifacts[matches[0]!] = parsed.adapter;
  }
  const document = { format: 1, artifacts };
  const problem = validateAdapterDocument(document);
  return problem ? { ok: false, message: problem } : { ok: true, adapter: parsed.adapter, document };
}

function adapterCandidate(): CandidateResult | null {
  const result = readAdapterCandidate();
  return result.ok ? result : null;
}

function matchText(result: { match: false; reason: string } | {
  match: true; type: string; artifact_id: string; rule: number; title?: string; mode: string | null;
}): string {
  if (!result.match) return `No match (${result.reason})`;
  const details = [`${result.type} rule ${result.rule + 1}`, result.artifact_id];
  if (result.title) details.push(`title “${result.title}”`);
  if (result.mode) details.push(`mode ${result.mode}`);
  return `Matches ${details.join(" · ")}`;
}

function refreshAdapterEditor(): void {
  if (adapterEdit === null) return;
  const candidate = readAdapterCandidate();
  adapterValidation.classList.toggle("warning", !candidate.ok);
  adapterValidation.textContent = candidate.ok
    ? "Valid here. The native host will validate the complete configuration again before saving."
    : candidate.message;
  adapterPreview.replaceChildren();
  if (!candidate.ok) {
    addText(adapterPreview, "li", "Fix the definition to see its match preview.");
    updateControls();
    return;
  }
  const preview = previewDocument(
    candidate.document,
    adapterPreviewUrl.value,
    adapterPreviewTitle.value,
    true,
    adapterOrigin(candidate.adapter) ?? undefined,
  );
  if (!preview.ok) {
    addText(adapterPreview, "li", preview.message);
    updateControls();
    return;
  }
  const probeMatches = preview.rows.filter((row) => row.probe && row.result.match).length;
  addText(adapterPreview, "li", probeMatches === 0
    ? "All fixed non-artifact probes stay unmatched."
    : `${probeMatches} fixed non-artifact probe${probeMatches === 1 ? " matches" : "s match"}; review before saving.`,
  probeMatches ? "probe-match" : undefined);
  for (const row of preview.rows) {
    const item = addText(adapterPreview, "li", `${row.probe ? "Probe" : "Example"}: ${row.url} — ${matchText(row.result)}`);
    if (row.probe && row.result.match) item.className = "probe-match";
  }
  updateControls();
}

function openAdapterEditor(edit: AdapterEdit, adapter: AdapterObject, heading: string): void {
  adapterEdit = edit;
  adapterEditor.hidden = false;
  adapterEditorHeading.textContent = heading;
  adapterJson.value = JSON.stringify(adapter, null, 2);
  adapterPreviewUrl.value = exampleUrl(adapter);
  adapterPreviewTitle.value = "";
  refreshAdapterEditor();
}

function beginAdapterDraft(adapter: AdapterObject, label: string): void {
  if (!session.config || busy) return;
  openAdapterEditor({ kind: "new" }, adapter, `Review ${label} starter`);
}

function beginAdapterEdit(index: number | "new"): void {
  if (!session.config || busy) return;
  const adapter = index === "new"
    ? newAdapter(typedOrigin() ?? "https://example.com")
    : session.config.adapters.artifacts[index];
  if (!adapter) return;
  const edit: AdapterEdit = index === "new"
    ? { kind: "new" }
    : { kind: "existing", identity: adapterIdentity(adapter)!, snapshot: adapterSnapshot(adapter) };
  openAdapterEditor(edit, adapter, index === "new"
    ? "Add and review adapter"
    : `Edit and review ${adapterLabel(adapter, index)}`);
}

function closeAdapterEditor(): void {
  adapterEdit = null;
  adapterEditor.hidden = true;
  adapterJson.value = "";
  adapterValidation.textContent = "";
  adapterPreview.replaceChildren();
  updateControls();
}

/**
 * Save a new adapter document. A site is on while it has an adapter, so the
 * host derives the sites from the document, and the first adapter saved with a home folder
 * turns capture on unless a pause is still pending. A folder selection the
 * host has not saved yet is saved with it.
 */
async function saveAdapterDocument(document: AdapterDocument, success: string): Promise<void> {
  if (!session.config || busy) return;
  busy = true;
  updateControls();
  const selection = picker;
  try {
    await withConfigMutationLock(async () => {
      const config = session.config!;
      const storage = selection ? storageFor(selection.path, config.storage) : config.storage;
      const hasSites = siteOrigins(document).length > 0;
      const state = await worker<WorkerStatus>({ kind: "get_status" });
      const first = config.adapters.artifacts.length === 0 && document.artifacts.length > 0 &&
        storage !== null && !state.pause_pending;
      await session.saveConfig({
        ...config,
        storage,
        adapters: document,
        capture_enabled: hasSites && (first || config.capture_enabled),
      }, selection?.token ?? null);
    });
    if (selection && picker === selection) picker = null;
    closeAdapterEditor();
    show(success);
  } catch (error) {
    show(`Could not save artifact adapters: ${describe(error)}`, true);
    if (error instanceof HostError && error.code === "conflict") await reloadConfig().catch(() => undefined);
  } finally {
    busy = false;
    renderConfig();
  }
}

/**
 * Remove an adapter document that needs repair. The host drops unsupported
 * `[adapters]` keys only on a real adapter change or, for a document already
 * empty on the wire, an exact no-op update. So the repair changes nothing
 * else; capture turns off in a second update, and success is reported only
 * once the host no longer reports the issue.
 */
async function resetAdapters(): Promise<void> {
  if (!session.config || busy) return;
  busy = true;
  updateControls();
  try {
    await withConfigMutationLock(async () => {
      await session.saveConfig({ ...session.config!, adapters: { format: 1, artifacts: [] } }, null);
      if (session.adapterIssue !== null) throw new Error(session.adapterIssue);
      if (session.config!.capture_enabled) {
        await session.saveConfig({ ...session.config!, capture_enabled: false }, null);
      }
    });
    closeAdapterEditor();
    show("Invalid adapter rules removed; automatic capture remains off until new rules are added.");
  } catch (error) {
    show(`Could not remove the artifact adapters: ${describe(error)}`, true);
    if (error instanceof HostError && error.code === "conflict") await reloadConfig().catch(() => undefined);
  } finally {
    busy = false;
    renderConfig();
  }
}

async function saveAdapter(): Promise<void> {
  const candidate = readAdapterCandidate();
  if (!candidate.ok) {
    refreshAdapterEditor();
    return;
  }
  const origin = adapterOrigin(candidate.adapter);
  if (origin && session.status?.pending_removals.includes(origin)) {
    show("Finish removing this site from the host before adding it again. Reload this page to retry.", true);
    return;
  }
  const action = adapterEdit?.kind === "new" ? "added" : "updated";
  await saveAdapterDocument(candidate.document, `${adapterLabel(candidate.adapter, 0)} adapter ${action}.`);
}

async function removeAdapter(index: number): Promise<void> {
  const config = session.config;
  if (!config || busy || !config.adapters.artifacts[index]) return;
  const label = adapterLabel(config.adapters.artifacts[index]!, index);
  const artifacts = config.adapters.artifacts.filter((_adapter, adapterIndex) => adapterIndex !== index);
  await saveAdapterDocument({ format: 1, artifacts }, `${label} adapter removed; its pages are no longer captured.`);
}

async function moveAdapter(index: number, offset: -1 | 1): Promise<void> {
  const config = session.config;
  const target = index + offset;
  if (!config || busy || target < 0 || target >= config.adapters.artifacts.length) return;
  const artifacts = [...config.adapters.artifacts];
  const current = artifacts[index]!;
  artifacts[index] = artifacts[target]!;
  artifacts[target] = current;
  await saveAdapterDocument({ format: 1, artifacts },
    `${adapterLabel(current, index)} moved ${offset < 0 ? "earlier" : "later"} in match precedence.`);
}

function renderConfig(): void {
  const config = session.config;
  resetAdaptersButton.hidden = session.adapterIssue === null;
  folderPath.textContent = picker?.path ?? config?.storage?.root ?? "None selected";
  sitesList.replaceChildren();
  const origins = config ? siteOrigins(config.adapters) : [];
  if (!origins.length) {
    const item = document.createElement("li");
    item.textContent = "None";
    sitesList.append(item);
  }
  for (const origin of origins) {
    const item = document.createElement("li");
    const label = document.createElement("span");
    label.textContent = origin;
    item.append(label);
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "secondary compact";
    remove.textContent = "Remove site";
    remove.setAttribute("aria-label", `Remove site ${origin}`);
    remove.addEventListener("click", () => void removeSite(origin));
    item.append(remove);
    sitesList.append(item);
  }
  renderStarterGallery();
  renderAdapters();
  renderAgent();
  if (adapterEdit === null) updateControls();
  else refreshAdapterEditor();
}

async function reloadConfig(): Promise<void> {
  await session.reload();
  showConfigState();
  if (session.status?.pause_pending && session.config?.capture_enabled && !session.configIssue) {
    show("Capture is paused locally. Use Pause capture to finish saving this setting in the host.", true);
  }
}

// Another page saved a different revision. Wait for this page's own change
// to finish, then read the host again.
function scheduleReload(): void {
  if (reloadTimer !== null) return;
  reloadTimer = setTimeout(() => {
    reloadTimer = null;
    if (busy) {
      scheduleReload();
      return;
    }
    busy = true;
    updateControls();
    void reloadConfig()
      .catch((error: unknown) => show(`Could not refresh settings: ${describe(error)}`, true))
      .finally(() => {
        busy = false;
        updateControls();
      });
  }, 250);
}

/** The storage settings for a home folder chosen through the native picker.
 * Its folders always have the fixed names; only the knowledge base carries over. */
function storageFor(root: string, current: StorageConfig | null): StorageConfig {
  return {
    root,
    profile: "neutral",
    ...HOME_LAYOUT,
    knowledge_base_dir: current?.knowledge_base_dir ?? null,
  };
}

/** Ask the host to show its native folder picker for one setting. */
function pickFolder(purpose: FolderPurpose) {
  return session.host.call({
    type: "choose_folder", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(), purpose,
  }, "folder_chosen", 5 * 60_000);
}

async function chooseFolder(): Promise<void> {
  busy = true;
  updateControls();
  try {
    const response = await pickFolder("notes_folder");
    picker = { path: response.path, token: response.picker_token };
    folderPath.textContent = response.path;
    updateControls();
    // Save the folder now, so setup is not lost with this page and the side
    // panel stops asking for one. Sites and the capture switch stay as they are.
    await withConfigMutationLock(() => session.saveConfig({
      ...session.config!, storage: storageFor(response.path, session.config!.storage),
    }, response.picker_token));
    picker = null;
    showConfigState();
  } catch (error) {
    // A canceled picker changed nothing. A failed save keeps the selection
    // usable: the next adapter saved on this page saves it too.
    show(describe(error), true);
    if (error instanceof HostError && error.code === "conflict") {
      picker = null;
      await reloadConfig().catch(() => undefined);
    }
  } finally {
    busy = false;
    renderConfig();
  }
}

/**
 * Choose the folder the summary agent reads, with its own native picker and
 * token, and save it at once. Only the knowledge base setting changes.
 */
async function chooseKnowledgeBase(): Promise<void> {
  if (busy || !session.config?.storage || session.configIssue) return;
  busy = true;
  updateControls();
  try {
    const response = await pickFolder("knowledge_base");
    await withConfigMutationLock(() => session.saveConfig({
      ...session.config!,
      storage: { ...session.config!.storage!, knowledge_base_dir: response.path },
    }, null, response.picker_token));
    show(`Knowledge base folder saved. Summary agents will read ${response.path}.`);
  } catch (error) {
    show(`Could not set the knowledge base folder: ${describe(error)}`, true);
    if (error instanceof HostError && error.code === "conflict") await reloadConfig().catch(() => undefined);
  } finally {
    busy = false;
    renderConfig();
  }
}

/** Let the knowledge base follow the home folder again. */
async function useNotesFolderAsKnowledgeBase(): Promise<void> {
  if (busy || !session.config?.storage?.knowledge_base_dir || session.configIssue) return;
  busy = true;
  updateControls();
  try {
    await withConfigMutationLock(() => session.saveConfig({
      ...session.config!,
      storage: { ...session.config!.storage!, knowledge_base_dir: null },
    }, null));
    show("Summary agents will read the home folder as the knowledge base.");
  } catch (error) {
    show(`Could not change the knowledge base folder: ${describe(error)}`, true);
    if (error instanceof HostError && error.code === "conflict") await reloadConfig().catch(() => undefined);
  } finally {
    busy = false;
    renderConfig();
  }
}

async function resumeCapture(): Promise<void> {
  if (busy || !session.config || session.configIssue) return;
  busy = true;
  updateControls();
  try {
    await withConfigMutationLock(async () => {
      // An explicit resume also clears a local pause, unless another page
      // paused again after this status was read.
      const state = await worker<WorkerStatus>({ kind: "get_status" });
      await session.saveConfig({ ...session.config!, capture_enabled: true }, null, null, undefined, true, state.pause_token);
    });
    showConfigState();
  } catch (error) {
    show(`Could not resume capture: ${describe(error)}`, true);
    if (error instanceof HostError && error.code === "conflict") await reloadConfig().catch(() => undefined);
  } finally {
    busy = false;
    await session.refreshStatus().catch((error: unknown) => show(describe(error), true));
    renderConfig();
  }
}

async function pauseCapture(): Promise<void> {
  if (!session.config || session.configIssue) return;
  busy = true;
  updateControls();
  try {
    await session.pauseCapture();
    show("Capture paused. Pending capture records cleared.");
  } catch (error) {
    show(`Could not pause capture: ${describe(error)}`, true);
  } finally {
    busy = false;
    await session.refreshStatus().catch((error: unknown) => show(describe(error), true));
    updateControls();
  }
}

async function refreshArtifactFiles(): Promise<void> {
  if (busy || !session.config?.storage) return;
  busy = true;
  updateControls();
  try {
    const response = await session.host.call({
      type: "refresh_artifacts",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
    }, "artifacts_refreshed");
    const conflict = response.conflicts
      ? ` ${response.conflicts} unowned path conflict${response.conflicts === 1 ? " remains" : "s remain"}; those files were not overwritten: ${response.conflict_paths.join(", ")}.`
      : "";
    show(`Artifact files updated: ${response.written} written, ${response.unchanged} already current.${conflict}`, response.conflicts > 0);
  } catch (error) {
    show(`Could not update artifact files: ${describe(error)}`, true);
  } finally {
    busy = false;
    updateControls();
  }
}

/** Turn a site off by removing every adapter on its origin, in one update. */
async function removeSite(origin: string): Promise<void> {
  const config = session.config;
  if (busy || !config || session.configIssue || session.adapterIssue) return;
  const artifacts = config.adapters.artifacts.filter((adapter) => adapter.origin !== origin);
  const removed = config.adapters.artifacts.length - artifacts.length;
  if (!removed) return;
  await saveAdapterDocument({ format: 1, artifacts },
    `Removed ${origin} and its ${removed} adapter${removed === 1 ? "" : "s"}; its pages are no longer captured.`);
}

async function detectHarnesses(): Promise<void> {
  if (busy || !session.connected) return;
  busy = true;
  updateControls();
  try {
    offers = await session.discoverHarnesses();
    const ready = offers.find((offer) => offer.refusal === null);
    selectOffer(ready?.harness_id ?? null);
    if (ready) {
      show(harnessFoundText(harnessName(ready.adapter), ready.version, !!session.config?.storage));
    } else {
      show(offers.length
        ? "No harness found here can be set up yet; see the reasons below."
        : "No supported harness was found on the native host's PATH.", true);
    }
  } catch (error) {
    clearOffers();
    show(`Could not detect harnesses: ${describe(error)}`, true);
  } finally {
    busy = false;
    renderConfig();
  }
}

async function setupHarness(): Promise<void> {
  const offer = selectedOffer();
  if (busy || !offer) return;
  if (!session.config?.storage) {
    show("Choose a home folder first.", true);
    return;
  }
  const envNames = offer.env_optional.filter((env) => envChoices.get(env.name)).map((env) => env.name);
  busy = true;
  updateControls();
  try {
    await withConfigMutationLock(() =>
      session.setupHarness(offer.harness_id, envNames, null));
    show(`${harnessName(offer.adapter)} is set up. ${agentStateText(session.agentStatus)}`);
  } catch (error) {
    show(`Harness setup did not finish: ${describe(error)}`, true);
    if (error instanceof HostError && error.code === "conflict") await reloadConfig().catch(() => undefined);
  } finally {
    busy = false;
    renderConfig();
  }
}

async function removeHarness(): Promise<void> {
  if (busy || !session.config?.agent) return;
  busy = true;
  updateControls();
  try {
    await withConfigMutationLock(() => session.saveConfig({ ...session.config!, agent: null }, null));
    show("Harness removed. AI commands are off.");
  } catch (error) {
    show(`Could not remove the harness: ${describe(error)}`, true);
    if (error instanceof HostError && error.code === "conflict") await reloadConfig().catch(() => undefined);
  } finally {
    busy = false;
    renderConfig();
  }
}

detectButton.addEventListener("click", () => void detectHarnesses());
newAdapterButton.addEventListener("click", () => beginAdapterEdit("new"));
resetAdaptersButton.addEventListener("click", () => void resetAdapters());
refreshArtifactsButton.addEventListener("click", () => void refreshArtifactFiles());
saveAdapterButton.addEventListener("click", () => void saveAdapter());
cancelAdapterButton.addEventListener("click", closeAdapterEditor);
adapterJson.addEventListener("input", refreshAdapterEditor);
adapterPreviewUrl.addEventListener("input", refreshAdapterEditor);
adapterPreviewTitle.addEventListener("input", refreshAdapterEditor);
setupButton.addEventListener("click", () => void setupHarness());
removeHarnessButton.addEventListener("click", () => void removeHarness());
chooseFolderButton.addEventListener("click", () => void chooseFolder());
chooseKnowledgeBaseButton.addEventListener("click", () => void chooseKnowledgeBase());
knowledgeBaseUseNotesButton.addEventListener("click", () => void useNotesFolderAsKnowledgeBase());
pauseButton.addEventListener("click", () => void pauseCapture());
resumeButton.addEventListener("click", () => void resumeCapture());
siteOrigin.addEventListener("input", () => {
  renderStarterGallery();
  updateControls();
});
session.watchPolicy(scheduleReload);
window.addEventListener("pagehide", () => {
  if (reloadTimer !== null) clearTimeout(reloadTimer);
  session.host.disconnect();
});

void (async () => {
  busy = true;
  updateControls();
  try {
    let hello: Awaited<ReturnType<ConfigSession["hello"]>>;
    try {
      hello = await session.hello();
    } catch (error) {
      show(`Native host unavailable: ${describe(error)}. Install or register the development host, then reload this page.`, true);
      return;
    }
    showHostBuild(hello.host_version);
    try {
      await reloadConfig();
      if (hello.config_issue) show(hello.config_issue, true);
      if (session.config && session.revision && !session.configIssue &&
          await session.applyPendingRemovals(session.config, session.revision)) {
        renderConfig();
        show("Finished removing sites that an earlier version turned off.");
      }
    } catch (error) {
      // The host answered, so this is a settings problem, not a missing host.
      if (session.configIssue) showConfigState();
      else show(`Could not load settings: ${describe(error)}. Reload this page to try again.`, true);
    }
  } finally {
    busy = false;
    await session.refreshStatus().catch(() => undefined);
    updateControls();
  }
})();
