import type { AdapterDocument, HarnessId, RunView, SiteDrafted, UncheckedReason } from "../protocol/ts/generated.js";
import { previewDocument, validateAdapterDocument } from "./adapter-review.js";
import { harnessName, serviceSetupText } from "./agent.js";
import type { WorkerStatus } from "./model.js";
import { HostError } from "./native.js";
import { RunProgress, type RunProgressDeps } from "./run-card.js";
import { NEEDS_HARNESS_MESSAGE } from "./summarize.js";
import { ConfigSession, describe, element, leaseFor, withConfigMutationLock, worker } from "./settings.js";
import {
  DROP_REASONS, captureMode, captureOrigin, checkReason, defaultChecked, draftRequest, planCaptureSite, planDraftedSite,
  recurringHint, siteConfigured, tenantPrefill,
  type CaptureMode, type CapturePlan, type DraftPlan, type DraftRequest, type DraftSelection,
} from "./capture-site.js";
import { httpOrigin, type SampledShape } from "./site-patterns.js";
import { githubOwnerFromUrl, parseTenantList, starterKind } from "./starter-adapters.js";
import { SitePatternPanel, collectPagePatterns } from "./site-pattern-panel.js";
import { draftHandoff, handoffKey, storeDraftHandoff } from "./capture-site-handoff.js";

export interface CapturePage {
  url: string;
  tabId: number | undefined;
  /** The tab title. It is shown only in the drafted review and is never
   * sent to the host's drafter. */
  title?: string;
}

interface CaptureActions {
  isBusy(): boolean;
  changed(): void;
  show(message: string, warning?: boolean): void;
  beforeSave(): Promise<void>;
  afterSave(): Promise<void>;
  openSettings(): void;
  /** Whether settings has a ready harness, so Improve with AI can run. */
  agentReady(): boolean;
}

const STARTER_SOURCE = "Development starter draft · live-tool certification is still pending.";
const DRAFT_SOURCE = "Drafted by Brauser from this site's page patterns. No agent and no page content were used; review each kind of page before saving.";
const AGENT_SOURCE = "Drafted by Brauser from this site's page patterns, then improved by AI from the same patterns. No page content was used, and Brauser checked every rule; review each kind of page before saving.";
const DRAFTED_BY = "Drafted from this site's page patterns";
const AGENT_REBUILD = "This Brauser host is older than the extension and cannot improve drafts with AI. Rebuild and reinstall the Brauser host, then open Capture Site again.";
const AGENT_FORBIDDEN = "it tried to set fields drafts may not set (titles, scope or page fields)";
const AGENT_NO_RULES = "none of its rules passed Brauser's safety checks";
const STALE = "Settings changed. Cancel and open Capture Site again to review the current settings.";
const SCOPE_INTRO = "This site holds many workspaces and accounts. Choose which ones Brauser remembers.";
const SCOPED_TEXT = "Only these workspaces or accounts";
const MAX_TENANTS = 64;

const UNCHECKED_LABELS: Record<UncheckedReason, string> = {
  link_only: "seen only in links on this page",
  token_only: "loose: its ID has no fixed shape",
  unseen: "suggested by AI: no page you visited or linked here matches it",
};

/** One capture of a path template, in words. */
function captureWords(source: string): string {
  if (source === "*" || source === "_") return "…";
  const colon = source.indexOf(":");
  const name = (colon < 0 ? source : source.slice(0, colon)).replace(/\?$/, "");
  const ctype = colon < 0 ? "token" : source.slice(colon + 1);
  const optional = source.slice(0, colon < 0 ? source.length : colon).endsWith("?");
  let words: string;
  if (ctype.startsWith("(")) {
    words = ctype.slice(1, ctype.indexOf(")")).split("|").join(" or ");
  } else {
    const type = /^[a-z_]+/.exec(ctype)?.[0] ?? "token";
    words = ctype.includes("@tail") ? "<title-ID>"
      : type === "int" ? "<number>"
      : type === "jira_key" ? "<key>"
      : type === "name" ? `<${name}>`
      : "<ID>";
  }
  return optional ? `[${words}]` : words;
}

/** A drafted rule in words, such as "/wiki/…/pages/<number>". Only the
 * rule's literals and capture classes are used. */
export function describeRule(rule: unknown): string {
  if (typeof rule !== "object" || rule === null || typeof (rule as { path?: unknown }).path !== "string") return "pages";
  const { path, query } = rule as { path: string; query?: unknown };
  let text = path.replace(/\{([^}]*)\}/g, (_, source: string) => captureWords(source));
  if (typeof query === "object" && query !== null) {
    const conditions = Object.entries(query).map(([key, value]) =>
      `${key}=${typeof value === "string" ? value.replace(/\{([^}]*)\}/g, (_, source: string) => captureWords(source)) : "…"}`);
    if (conditions.length) text += `?${conditions.join("&")}`;
  }
  return text;
}

function child<K extends keyof HTMLElementTagNameMap>(
  parent: HTMLElement, tag: K, text = "", className = "",
): HTMLElementTagNameMap[K] {
  const node = document.createElement(tag);
  if (text) node.textContent = text;
  if (className) node.className = className;
  parent.append(node);
  return node;
}

/**
 * The save-time re-check of a drafted selection (`check_site_draft`). It
 * sends `plan.checked` alone: the Tier 0 tables after unchecking and
 * renaming, all-drafted provenance and the separate scope choices. The scope,
 * `unscoped` and title overlays exist only in `plan.document` and never
 * cross. Any dropped rule, any scope verdict with a reason, a reply for
 * another revision or one that does not answer every checked rule refuses
 * with plain words. It writes nothing.
 */
export async function checkDraftedSave(
  session: ConfigSession, plan: DraftPlan, revision: string, input: DraftRequest,
): Promise<void> {
  const { checked } = plan;
  const reply = await session.checkSiteDraft(plan.origin, revision, checked, input.visited, input.links);
  if (reply.revision !== revision) throw new Error(STALE);
  if (reply.rules.length !== checked.adapters.length || reply.scope.length !== checked.adapters.length ||
      reply.rules.some((rules, a) => rules.length !== checked.provenance[a]!.length)) {
    throw new Error("Brauser's host did not check every chosen rule. Nothing was saved; open Capture Site again.");
  }
  for (const [a, table] of checked.adapters.entries()) {
    const type = String(table.type);
    const match = Array.isArray(table.match) ? table.match as unknown[] : [];
    for (const [r, verdict] of reply.rules[a]!.entries()) {
      if (verdict.verdict === "dropped") {
        throw new Error(`${type} pages like ${describeRule(match[r])} can no longer be saved: ${checkReason(verdict.reason)}. Nothing was saved; open Capture Site again.`);
      }
    }
    const scope = reply.scope[a]!;
    if (scope.reason !== null) {
      throw new Error(`The accounts chosen for ${type} were not accepted: ${checkReason(scope.reason)}. Nothing was saved; open Capture Site again.`);
    }
  }
}

interface DraftKindView {
  heading: HTMLElement;
  type: HTMLInputElement;
  rules: HTMLInputElement[];
}

interface RestorePlan {
  origin: string;
  document: AdapterDocument;
  startsCapture: boolean;
}

// The confirmation line names only what is actually checked, in this fixed
// order, so the privacy-boundary scope line is never wider than the save.
const GITHUB_RECORD_LABELS: Record<string, string> = {
  "github.repo": "repositories",
  "github.pr": "pull requests",
  "github.issue": "issues",
};
const GITHUB_RECORD_ORDER = ["github.repo", "github.pr", "github.issue"];

/** "A", "A and B", or "A, B and C" — no Oxford comma, matching this panel's
 * existing list copy (e.g. the "Remember: X and Y" summary line). */
function joinRecordedTypes(labels: string[]): string {
  if (labels.length < 2) return labels.join("");
  return `${labels.slice(0, -1).join(", ")} and ${labels[labels.length - 1]}`;
}

/** An in-panel, revision-bound review. All transient page URLs and scopes
 * remain in this panel; only the reviewed config goes to the native host. */
export class CaptureSitePanel {
  private readonly patterns = new SitePatternPanel();
  private readonly area = element<HTMLElement>("capture-site-area");
  private readonly start = element<HTMLButtonElement>("capture-site");
  private readonly recurringStart = element<HTMLButtonElement>("capture-site-recurring");
  private readonly hint = element<HTMLElement>("capture-site-hint");
  private readonly review = element<HTMLElement>("capture-site-review");
  private readonly heading = element<HTMLElement>("capture-site-heading");
  private readonly github = element<HTMLElement>("capture-site-github");
  private readonly repos = element<HTMLInputElement>("capture-site-repos");
  private readonly prs = element<HTMLInputElement>("capture-site-prs");
  private readonly issues = element<HTMLInputElement>("capture-site-issues");
  private readonly orgs = element<HTMLInputElement>("capture-site-orgs");
  private readonly preview = element<HTMLElement>("capture-site-preview");
  private readonly message = element<HTMLElement>("capture-site-message");
  private readonly save = element<HTMLButtonElement>("capture-site-save");
  private readonly cancel = element<HTMLButtonElement>("capture-site-cancel");
  private readonly source = element<HTMLElement>("capture-site-source");
  private readonly kinds = element<HTMLElement>("capture-site-draft");
  private readonly scopeArea = element<HTMLElement>("capture-site-scope");
  private readonly scopedLabel = element<HTMLElement>("capture-site-scoped-label");
  private readonly scoped = element<HTMLInputElement>("capture-site-scoped");
  private readonly scopedText = element<HTMLElement>("capture-site-scoped-text");
  private readonly scopeIntro = element<HTMLElement>("capture-site-scope-intro");
  private readonly scopeValuesLabel = element<HTMLElement>("capture-site-scope-values-label");
  private readonly scopeValues = element<HTMLInputElement>("capture-site-scope-values");
  private readonly unscoped = element<HTMLInputElement>("capture-site-unscoped");
  private readonly titleArea = element<HTMLElement>("capture-site-title");
  private readonly titleCurrent = element<HTMLElement>("capture-site-title-current");
  private readonly titleSuffix = element<HTMLInputElement>("capture-site-title-suffix");
  private readonly dropped = element<HTMLElement>("capture-site-dropped");
  private readonly advanced = element<HTMLButtonElement>("capture-site-advanced");
  private page: CapturePage | null = null;
  private busy = false;
  // The worker's private shape sample for the configured page, read from the
  // worker (never the host) to decide the recurring-pages hint. It is only
  // sent to the host after the user asks to draft rules for these pages.
  private sample: { key: string; samples: SampledShape[] | null; revision: string | null; hint: boolean } | null = null;
  private target: (CapturePage & { revision: string; mode: CaptureMode | "restore"; origin: string }) | null = null;
  private plan: CapturePlan | null = null;
  private restorePlan: RestorePlan | null = null;
  private restored: AdapterDocument | null = null;
  // Fresh rules require a successful review reporting no recorded contracts.
  // A failed review must stay blocked when starter inputs change.
  private freshRulesAllowed = false;
  private starterCheck = 0;
  // The drafted flow. The link shapes (in `draftInput`) and the draft live
  // only here and are cleared by close().
  private draft: SiteDrafted | null = null;
  private draftInput: DraftRequest | null = null;
  private draftPlan: DraftPlan | null = null;
  private draftProblem: string | null = null;
  private draftViews: DraftKindView[] = [];
  private previewMessage = "";
  // Improve with AI: the structural draft it refines (shown again by Use
  // basic draft), the watched site-draft run, and the harness that drafted
  // the shown agent draft. Cleared by close() with the other draft fields.
  private basicDraft: SiteDrafted | null = null;
  private agentProgress: RunProgress | null = null;
  // A start_site_draft or a judging draft_site is in flight.
  private agentPending = false;
  private agentHarness: HarnessId | null = null;
  private readonly drafterLine = element<HTMLElement>("capture-site-drafter");
  private readonly improveButton = element<HTMLButtonElement>("capture-site-improve");
  private readonly basicButton = element<HTMLButtonElement>("capture-site-basic");
  private readonly improveHint = element<HTMLElement>("capture-site-improve-hint");
  private readonly agentRun = element<HTMLElement>("capture-site-agent-run");
  saving = false;

  constructor(private readonly session: ConfigSession, private readonly actions: CaptureActions) {
    this.start.addEventListener("click", () => this.open());
    this.recurringStart.addEventListener("click", () => this.openRecurring());
    this.cancel.addEventListener("click", () => {
      if (this.saving) return;
      this.close();
      this.start.focus();
    });
    this.save.addEventListener("click", () => void this.commit());
    for (const input of [this.repos, this.prs, this.issues, this.orgs]) {
      input.addEventListener("input", () => this.updatePreview());
    }
    this.scoped.addEventListener("input", () => {
      if (this.scoped.checked) this.unscoped.checked = false;
      this.updateDraftPreview();
    });
    this.unscoped.addEventListener("input", () => {
      if (this.unscoped.checked) this.scoped.checked = false;
      this.updateDraftPreview();
    });
    for (const input of [this.scopeValues, this.titleSuffix]) {
      input.addEventListener("input", () => this.updateDraftPreview());
    }
    this.advanced.addEventListener("click", () => void this.openAdvanced());
    this.improveButton.addEventListener("click", () => void this.improve());
    this.basicButton.addEventListener("click", () => this.useBasicDraft());
    // As for Page patterns: a reload or document replacement keeps the tab
    // ID and can keep the URL, so any new load or URL change in the reviewed
    // tab discards a drafted review and its link shapes.
    chrome.tabs.onUpdated.addListener((tabId, changeInfo) => {
      if (this.target?.mode === "draft" && tabId === this.target.tabId && !this.saving &&
        (changeInfo.status === "loading" || changeInfo.url !== undefined)) this.close();
    });
  }

  render(page: CapturePage | null, busy: boolean): void {
    this.patterns.render(page, busy);
    this.page = page;
    this.busy = busy;
    if (this.target && !this.samePage(page, this.target)) this.close();
    this.updateOffer();
  }

  /** Which Capture Site entry the page gets: the Capture Site button on a
   * starter or unknown site, the recurring-pages hint on a configured site
   * whose sampled pages recur unrecognized, or the settings pointer. */
  private updateOffer(): void {
    const page = this.page;
    const busy = this.busy;
    const config = this.session.config;
    const mode = page ? captureMode(config, page.url) : "unsupported";
    const configured = mode === "configured";
    const recurring = configured && this.recurring(page!);
    // A configured starter site still says where its rules are managed.
    const offered = this.target !== null || mode === "starter" || mode === "draft" ||
      (configured && (captureOrigin(page!.url) !== null || recurring));
    this.area.hidden = !offered;
    this.recurringStart.hidden = !offered || !recurring || this.target !== null;
    if (!offered) return;
    const blocked = busy || !this.session.connected || !config?.storage ||
      !!this.session.configIssue || !!this.session.adapterIssue;
    this.start.hidden = configured || this.target !== null;
    this.start.disabled = configured || blocked;
    this.recurringStart.disabled = !recurring || blocked;
    this.hint.textContent = configured && !recurring ? "This site has capture rules. Manage them in settings."
      : !config?.storage ? "Choose a home folder in settings, then capture this site here."
      : this.session.configIssue || this.session.adapterIssue ? "Repair the configuration in settings before adding a site."
      : recurring ? `Looks like you have recurring pages on ${new URL(page!.url).host}.`
      : "Remember work on this site without configuring rules by hand.";
    if (this.target) this.updateControls(busy);
  }

  /** Whether this configured page gets the recurring-pages hint. The
   * worker's sample is read once per page and revision and evaluated here
   * with the current policy; nothing goes to the host. */
  private recurring(page: CapturePage): boolean {
    const origin = httpOrigin(page.url);
    const config = this.session.config;
    const revision = this.session.revision;
    if (!origin || !config || !revision) return false;
    const key = `${page.tabId ?? ""}\n${page.url}\n${revision}`;
    if (this.sample?.key !== key) {
      const sample: NonNullable<CaptureSitePanel["sample"]> = { key, samples: null, revision: null, hint: false };
      this.sample = sample;
      void worker<SampledShape[]>({ kind: "get_site_patterns", origin }).catch(() => []).then((rows) => {
        if (this.sample !== sample) return;
        sample.samples = Array.isArray(rows) ? rows : [];
        this.updateOffer();
      });
      return false;
    }
    const sample = this.sample;
    if (!sample.samples) return false;
    if (sample.revision !== revision) {
      sample.hint = recurringHint(leaseFor(config, revision), sample.samples, page.url);
      sample.revision = revision;
    }
    return sample.hint;
  }

  private samePage(a: CapturePage | null, b: CapturePage): boolean {
    return a?.url === b.url && a.tabId === b.tabId;
  }

  private open(): void {
    if (this.actions.isBusy() || !this.page || !this.session.revision || this.start.disabled || this.start.hidden) return;
    const mode = captureMode(this.session.config, this.page.url);
    const origin = mode === "starter" ? captureOrigin(this.page.url) : mode === "draft" ? httpOrigin(this.page.url) : null;
    if (!origin) return;
    this.begin(mode, origin);
  }

  /** Draft rules for a configured site's recurring pages: the drafted flow,
   * with the worker's sample as the visited shapes. It only adds adapters;
   * the site's existing definitions are never replaced or edited. */
  private openRecurring(): void {
    const page = this.page;
    if (this.actions.isBusy() || !page || !this.session.revision || this.target ||
        this.recurringStart.disabled || this.recurringStart.hidden) return;
    const origin = httpOrigin(page.url);
    if (!origin || captureMode(this.session.config, page.url) !== "configured" || !this.recurring(page)) return;
    this.begin("draft", origin);
  }

  private begin(mode: CaptureMode, origin: string): void {
    const page = this.page!;
    this.target = { ...page, revision: this.session.revision!, mode, origin };
    this.review.hidden = false;
    this.start.hidden = true;
    this.recurringStart.hidden = true;
    this.heading.textContent = `Capture work on ${new URL(origin).host}`;
    this.source.textContent = mode === "draft" ? DRAFT_SOURCE : STARTER_SOURCE;
    this.github.hidden = mode !== "starter" || starterKind(origin) !== "github";
    this.repos.checked = true;
    this.prs.checked = true;
    this.issues.checked = true;
    // Shown and confirmed, never applied unseen: the user still has to save.
    this.orgs.value = mode === "starter" ? githubOwnerFromUrl(page.url) ?? "" : "";
    if (mode === "draft" && siteConfigured(this.session.config!, origin)) void this.startDraft(this.target);
    else void this.startReview(this.target);
    this.heading.focus();
  }

  /** Prefer recorded identity to a new starter or draft. Only the exact
   * origin crosses this read boundary; page patterns are read only if the
   * host reports no recorded rules. */
  private async startReview(target: NonNullable<CaptureSitePanel["target"]>): Promise<void> {
    this.freshRulesAllowed = false;
    this.previewMessage = "Checking saved site identities…";
    this.updateControls(this.actions.isBusy());
    try {
      const recorded = await this.session.siteRestore(target.origin, target.revision);
      if (this.target !== target || !this.samePage(this.page, target) || this.session.revision !== target.revision) return;
      if (!recorded.artifacts.length) {
        this.freshRulesAllowed = true;
        if (target.mode === "draft") await this.startDraft(target);
        else this.updatePreview();
        return;
      }
      if (recorded.artifacts.some((a) => a.origin !== target.origin)) throw new Error("The host returned rules for another site.");
      const document: AdapterDocument = {
        format: 1, artifacts: [...this.session.config!.adapters.artifacts, ...recorded.artifacts],
      };
      const problem = validateAdapterDocument(document);
      if (problem) throw new Error(problem);
      target.mode = "restore";
      this.restored = recorded;
      this.restorePlan = { origin: target.origin, document, startsCapture: this.session.config!.capture_enabled };
      this.heading.textContent = `Restore capture on ${new URL(target.origin).host}`;
      this.source.textContent = "Recorded capture rules · existing notes keep their identity. Title cleanup and reference extraction can be adjusted in settings.";
      this.github.hidden = true;
      this.preview.replaceChildren();
      for (const adapter of recorded.artifacts) {
        const name = GITHUB_RECORD_LABELS[String(adapter.type)] ?? String(adapter.type).replace(/[._]/g, " ");
        const rules = Array.isArray(adapter.match) ? adapter.match : [];
        child(this.preview, "p", `Recorded page kind: ${name} — pages like ${rules.map(describeRule).join(" or ")}.`);
        const scope = adapter.scope;
        if (typeof scope === "object" && scope !== null && !Array.isArray(scope)) {
          for (const [key, values] of Object.entries(scope)) {
            child(this.preview, "p", `Records ${name} only for ${key}: ${(values as string[]).join(", ")}.`);
          }
        } else {
          child(this.preview, "p", adapter.unscoped === "acknowledged"
            ? `Records ${name} in every account on this site in this Chrome profile.`
            : `Records matching ${name} pages on this site.`);
        }
      }
      const preview = previewDocument(recorded, target.url, "", false);
      if (preview.ok) child(this.preview, "p", `This page: ${preview.rows[0]?.result.match ? "will be captured" : "will not be captured"}.`);
      this.previewMessage = this.restorePlan.startsCapture
        ? "Restore these recorded page kinds and accounts to capture. Other sites keep their existing rules."
        : "Restore these rules while keeping capture paused. Resume capture in settings when you are ready.";
    } catch (error) {
      if (this.target !== target) return;
      this.restorePlan = null;
      this.previewMessage = `Could not review recorded rules: ${describe(error)}`;
    }
    if (this.target === target) this.updateControls(this.actions.isBusy());
  }

  private close(): void {
    this.starterCheck += 1;
    // An unfinished AI run is canceled: the shapes it drafts from are
    // discarded with this review.
    const progress = this.agentProgress;
    this.agentProgress = null;
    if (progress) void progress.close();
    this.agentPending = false;
    this.agentHarness = null;
    this.basicDraft = null;
    this.agentRun.replaceChildren();
    this.drafterLine.textContent = "";
    this.drafterLine.hidden = true;
    this.target = null;
    this.plan = null;
    this.restorePlan = null;
    this.restored = null;
    this.freshRulesAllowed = false;
    this.draft = null;
    this.draftInput = null;
    this.draftPlan = null;
    this.draftProblem = null;
    this.draftViews = [];
    this.previewMessage = "";
    this.review.hidden = true;
    this.orgs.value = "";
    this.preview.replaceChildren();
    this.kinds.replaceChildren();
    this.dropped.replaceChildren();
    this.scopeArea.hidden = true;
    this.scoped.checked = false;
    this.unscoped.checked = false;
    this.scopeValues.value = "";
    this.scopedText.textContent = SCOPED_TEXT;
    this.scopeIntro.textContent = SCOPE_INTRO;
    this.titleArea.hidden = true;
    this.titleCurrent.textContent = "";
    this.titleSuffix.value = "";
    this.advanced.hidden = true;
    this.message.textContent = "";
    this.updateOffer();
  }

  /** Read this page's shapes on the click, then have the host draft from
   * the origin and shapes alone. Raw URLs, link hrefs and the tab title
   * never leave the panel. */
  private async startDraft(target: NonNullable<CaptureSitePanel["target"]>): Promise<void> {
    const isCurrent = (): boolean => this.target === target && this.samePage(this.page, target);
    this.previewMessage = "Reading this page's patterns…";
    this.updateControls(this.actions.isBusy());
    try {
      const patterns = await collectPagePatterns(target, isCurrent);
      if (!patterns) {
        // The page navigated: its link shapes are not this page's.
        if (this.target === target) this.close();
        return;
      }
      // The worker samples only origins that already have capture rules.
      let samples: SampledShape[] = [];
      const config = this.session.config;
      if (config && siteConfigured(config, target.origin)) {
        samples = await worker<SampledShape[]>({ kind: "get_site_patterns", origin: target.origin }).catch(() => []);
        if (!isCurrent()) return;
      }
      const input = draftRequest(target.origin, patterns, samples);
      this.previewMessage = "Drafting capture rules from this site's page patterns…";
      this.updateControls(this.actions.isBusy());
      const draft = await this.session.draftSite(input.origin, input.visited, input.links);
      if (!isCurrent()) return;
      if (draft.revision !== target.revision) {
        this.draftProblem = STALE;
        this.updateControls(this.actions.isBusy());
        return;
      }
      this.draft = draft;
      this.draftInput = input;
      this.renderDraft(draft, target);
    } catch (error) {
      if (this.target !== target) return;
      this.previewMessage = error instanceof HostError && error.code === "invalid_request" &&
          /unsupported request/.test(error.message)
        ? "This Brauser host is older than the extension and cannot draft capture rules. Rebuild and reinstall the Brauser host, then open Capture Site again."
        : `Could not draft capture rules: ${describe(error)}`;
      this.updateControls(this.actions.isBusy());
    }
  }

  /** The plain-language review: what Brauser will and will not remember,
   * with the controls that shape the selection. */
  private renderDraft(draft: SiteDrafted, target: NonNullable<CaptureSitePanel["target"]>): void {
    const agent = draft.drafter === "agent";
    this.source.textContent = agent ? AGENT_SOURCE : DRAFT_SOURCE;
    this.drafterLine.hidden = false;
    this.drafterLine.textContent = agent
      ? `Improved with AI${this.agentHarness ? ` (${harnessName(this.agentHarness)})` : ""}, checked by Brauser`
      : DRAFTED_BY;
    this.kinds.replaceChildren();
    this.draftViews = [];
    for (const [a, view] of draft.adapters.entries()) {
      const card = child(this.kinds, "div", "", "adapter-card");
      const heading = child(card, "p");
      const field = child(card, "div", "", "field");
      const label = child(field, "label", "Kind name");
      const type = child(field, "input");
      type.type = "text";
      type.id = `capture-site-type-${a}`;
      label.htmlFor = type.id;
      type.maxLength = 32;
      type.autocomplete = "off";
      type.spellcheck = false;
      type.value = String(view.adapter.type);
      type.dataset.type = String(a);
      type.addEventListener("input", () => this.updateDraftPreview());
      const match = Array.isArray(view.adapter.match) ? view.adapter.match : [];
      const rules: HTMLInputElement[] = [];
      for (const entry of view.rules) {
        const row = child(card, "label", "", "check-label");
        const box = child(row, "input");
        box.type = "checkbox";
        box.checked = defaultChecked(entry);
        box.dataset.rule = `${a}/${entry.rule}`;
        box.addEventListener("input", () => this.updateDraftPreview());
        const reason = entry.unchecked_reason ? ` · ${UNCHECKED_LABELS[entry.unchecked_reason]}` : "";
        child(row, "span", `Pages like ${describeRule(match[entry.rule])}${reason}`);
        child(card, "p", `Illustrative example: ${entry.example}`, "page-url");
        rules.push(box);
      }
      this.draftViews.push({ heading, type, rules });
    }
    const pending = draft.adapters.find((view) => view.scope === "pending");
    const unscopedOnly = draft.adapters.some((view) => view.scope === "unscoped_only");
    this.scopeArea.hidden = !pending && !unscopedOnly;
    for (const node of [this.scopedLabel, this.scoped, this.scopeValuesLabel, this.scopeValues]) node.hidden = !pending;
    // The tenant is read from this tab's own URL at the host's position,
    // only when a kept drafted rule records this page with it, and
    // pre-filled as "Capture for <tenant>", already chosen: Save is the
    // one-click confirmation, and the list stays editable. With no tenant in
    // the URL the user still chooses (John, 2026-10-27; DESIGN §5.1).
    const prefill = pending ? tenantPrefill(target.url, pending) : "";
    this.scopeValues.value = prefill;
    this.scoped.checked = prefill !== "";
    this.unscoped.checked = false;
    this.scopeIntro.textContent = prefill
      ? `This site holds many workspaces and accounts. Brauser filled in ${prefill} from this page; confirm it with Save, or change the list.`
      : SCOPE_INTRO;
    this.titleArea.hidden = draft.adapters.length === 0;
    this.titleCurrent.textContent = `This tab's title: ${target.title ?? ""}`;
    this.titleSuffix.value = "";
    this.renderDropped(draft);
    // The advanced editor is offered for every draft, and is the only way
    // forward for an empty one.
    this.advanced.hidden = false;
    if (!draft.adapters.length) {
      const noRouteWord = draft.dropped.some((row) => row.reason === "no_route_word" || row.reason === "draft_no_leading_literal");
      this.previewMessage = `Brauser could not draft capture rules for this site from what this page shows.${noRouteWord
        ? " This site's pages have no fixed route word to draft from safely." : ""} You can write rules by hand in advanced settings.`;
      this.updateControls(this.actions.isBusy());
      return;
    }
    this.updateDraftPreview();
  }

  private renderDropped(draft: SiteDrafted): void {
    this.dropped.replaceChildren();
    if (draft.dropped.length) {
      child(this.dropped, "p", "Brauser won't capture:");
      const list = child(this.dropped, "ul", "", "plain-list");
      for (const row of draft.dropped) {
        // An agent drop's `adapter` indexes the agent's answer, not
        // `adapters`, so only a structural row may name its kind.
        const type = row.adapter === null || draft.drafter === "agent"
          ? undefined
          : draft.adapters[row.adapter]?.adapter.type;
        const subject = row.shape ?? (typeof type === "string" ? `A drafted ${type} rule` : "A drafted rule");
        child(list, "li", `${subject}: ${DROP_REASONS[row.reason]}`);
      }
    }
    const probes = draft.preview.filter((row) => row.source === "probe");
    if (probes.length) {
      const details = child(this.dropped, "details");
      child(details, "summary", `Safety checks: ${probes.length} sign-in, search, dashboard, sharing and other pages skipped`);
      const list = child(details, "ul", "", "plain-list");
      for (const row of probes) {
        child(list, "li", `${row.adapter === null ? "Skipped" : "Would be captured"}: ${row.url}`, row.adapter === null ? "" : "warning");
      }
    }
  }

  /**
   * Hand the draft to the settings page's adapter editor. The entry holds
   * the reviewed selection's checked tables with its chosen scope (or, when
   * the review is not yet complete, the drafted tables with only their
   * checked rules, so a link-only or loose rule the user never checked is
   * never handed off as drafted), and the private shapes. It never holds a
   * raw URL, a link href, the tab title or the title ending typed from it.
   * The review then closes.
   */
  private async openAdvanced(): Promise<void> {
    const target = this.target;
    const draft = this.draft;
    const input = this.draftInput;
    if (!target || target.mode !== "draft" || !draft || !input || this.saving) {
      this.actions.openSettings();
      return;
    }
    const plan = this.draftPlan;
    const adapters = plan
      ? plan.document.artifacts.slice(-plan.checked.adapters.length).map((adapter) => {
        const table = { ...adapter };
        delete table.title;
        return table;
      })
      : this.checkedDraftTables(draft);
    const entry = draftHandoff(crypto.randomUUID(), target.origin, adapters, input.visited, input.links, Date.now());
    try {
      await storeDraftHandoff(chrome.storage.session, entry);
      await chrome.tabs.create({ url: chrome.runtime.getURL(`options.html#draft=${entry.id}`) });
      if (this.target === target) this.close();
    } catch (error) {
      await chrome.storage.session.remove?.(handoffKey(entry.id)).catch(() => undefined);
      this.actions.show(`Could not open advanced settings: ${describe(error)}`, true);
    }
  }

  /** The drafted tables as they came, with only the rules whose boxes are
   * checked, and no adapter whose rules are all unchecked. */
  private checkedDraftTables(draft: SiteDrafted): Record<string, unknown>[] {
    const checked = this.draftViews.map((view) => view.rules.map((box) => box.checked));
    const tables: Record<string, unknown>[] = [];
    for (const [a, view] of draft.adapters.entries()) {
      const match = Array.isArray(view.adapter.match) ? view.adapter.match as unknown[] : [];
      const kept = view.rules.filter((entry, r) => checked[a]?.[r] === true && entry.rule < match.length);
      if (!kept.length) continue;
      tables.push({ ...structuredClone(view.adapter), match: kept.map((entry) => structuredClone(match[entry.rule])) });
    }
    return tables;
  }

  private selection(): DraftSelection {
    const draft = this.draft!;
    return {
      checked: this.draftViews.map((view) => view.rules.map((box) => box.checked)),
      types: this.draftViews.map((view) => view.type.value),
      scope: draft.adapters.map((view) =>
        view.scope === "pending"
          ? this.unscoped.checked ? "unscoped" : this.scoped.checked ? { values: this.scopeValues.value } : null
          : view.scope === "unscoped_only" && this.unscoped.checked ? "unscoped" : null),
      titleSuffix: this.titleSuffix.value,
    };
  }

  private updateDraftPreview(): void {
    const target = this.target;
    const draft = this.draft;
    const config = this.session.config;
    if (!target || target.mode !== "draft" || !draft || !config || this.saving) return;
    const selection = this.selection();
    for (const [a, view] of this.draftViews.entries()) {
      const match = Array.isArray(draft.adapters[a]!.adapter.match) ? draft.adapters[a]!.adapter.match as unknown[] : [];
      const kept = draft.adapters[a]!.rules.filter((_, r) => selection.checked[a]![r]);
      const shown = (kept.length ? kept : draft.adapters[a]!.rules).map((entry) => describeRule(match[entry.rule]));
      const name = view.type.value.trim() || String(draft.adapters[a]!.adapter.type);
      view.heading.textContent = `${kept.length ? "Brauser will remember" : "Not remembered"}: ${name} — pages like ${shown.join(" or ")}`;
    }
    const tenants = parseTenantList(this.scopeValues.value, MAX_TENANTS);
    this.scopedText.textContent = tenants ? `Capture for ${joinRecordedTypes(tenants)}` : SCOPED_TEXT;
    this.preview.replaceChildren();
    const result = planDraftedSite(config, draft, selection, target.url, target.title ?? "");
    this.draftPlan = result.ok ? result.plan : null;
    this.previewMessage = result.ok
      ? result.plan.startsCapture ? "Saving enables these pages. Other sites keep their existing rules."
        : "Capture is paused for all sites. This saves the site; resume capture in settings when you are ready."
      : result.message;
    if (result.ok) {
      const plan = result.plan;
      const { visited, visitedTotal, links, linksTotal } = plan.counts;
      child(this.preview, "p", "Recognized pages contribute titles, opens and measured focused/background time to your local worklog.");
      const scoped = selection.scope.some((choice) => choice !== null && choice !== "unscoped");
      if (scoped && tenants) child(this.preview, "p", `Records pages only for: ${tenants.join(", ")}.`);
      child(this.preview, "p", `Matches ${visited} of ${visitedTotal} visited patterns and ${links} of ${linksTotal} link patterns.`);
      child(this.preview, "p", `This page: ${plan.current.result.match ? "will be captured" : "will not be captured"} — ${plan.current.url}`);
      const examples = child(this.preview, "ul", "", "plain-list");
      for (const example of plan.examples) {
        child(examples, "li", `Illustrative example (${example.result.match ? "captured" : "skipped"}): ${example.url}`);
      }
    }
    this.updateControls(this.actions.isBusy());
  }

  private updateControls(busy: boolean): void {
    const stale = this.target?.revision !== this.session.revision;
    const state = this.session.status;
    const origin = this.target?.origin ?? null;
    const removal = origin !== null && state?.pending_removals.includes(origin);
    const problem = stale || this.draftProblem ? STALE
      : this.session.configIssue || this.session.adapterIssue ? "Repair the configuration in settings first."
      : state?.pause_pending ? "A pause is still being saved. Finish it in settings before capturing a site."
      : removal ? "This site has a pending removal. Finish it in settings before adding it again."
      : null;
    const plan = this.target?.mode === "draft" ? this.draftPlan : this.target?.mode === "restore" ? this.restorePlan : this.plan;
    this.message.textContent = problem ?? this.previewMessage;
    this.save.disabled = busy || !plan || !!problem || !this.session.connected;
    this.cancel.disabled = this.saving;
    for (const input of [this.repos, this.prs, this.issues, this.orgs]) input.disabled = this.saving || stale;
    this.orgs.disabled ||= !this.repos.checked && !this.prs.checked && !this.issues.checked;
    const draftInputs = [
      ...this.draftViews.flatMap((view) => [view.type, ...view.rules]), this.scoped, this.unscoped, this.titleSuffix,
    ];
    for (const input of draftInputs) input.disabled = this.saving || stale;
    this.scopeValues.disabled = this.saving || stale || !this.scoped.checked;
    this.save.textContent = this.saving ? "Saving…" : this.target?.mode === "restore"
      ? plan?.startsCapture ? "Restore capture rules" : "Restore rules · keep capture paused"
      : plan?.startsCapture ? "Start capturing this site" : "Save site · keep capture paused";
    // Improve with AI refines a shown structural draft; Use basic draft
    // returns to it from an agent draft.
    const drafted = this.target?.mode === "draft" ? this.draft : null;
    const ready = this.actions.agentReady();
    this.improveButton.hidden = drafted?.drafter !== "structural";
    this.improveButton.disabled = !ready || busy || this.saving || !!problem || !this.session.connected ||
      this.agentPending || this.agentProgress !== null;
    this.improveButton.title = ready ? "" : NEEDS_HARNESS_MESSAGE;
    this.improveHint.textContent = ready ? "" : NEEDS_HARNESS_MESSAGE;
    this.improveHint.hidden = this.improveButton.hidden || ready;
    this.basicButton.hidden = drafted?.drafter !== "agent" || this.basicDraft === null;
    this.basicButton.disabled = this.saving;
  }

  private runDeps(): RunProgressDeps {
    return {
      host: this.session.host,
      now: () => Date.now(),
      copy: (text) => navigator.clipboard.writeText(text),
      visible: () => document.visibilityState !== "hidden",
    };
  }

  /** Improve with AI: start (or rejoin) the site-draft run from the shapes
   * already held for the structural draft. Nothing is read from the page
   * again, and the host builds the agent's input itself. */
  private async improve(): Promise<void> {
    const target = this.target;
    const input = this.draftInput;
    if (!target || target.mode !== "draft" || !input || this.draft?.drafter !== "structural" ||
        this.improveButton.disabled || this.improveButton.hidden || this.actions.isBusy() || !this.actions.agentReady()) return;
    this.agentPending = true;
    this.agentRun.replaceChildren();
    this.updateControls(this.actions.isBusy());
    let started;
    try {
      started = await this.session.startSiteDraft(input.origin, input.visited, input.links);
    } catch (error) {
      if (this.target !== target) return;
      this.agentPending = false;
      this.agentNotUsed(this.agentErrorText(error));
      return;
    }
    if (this.target !== target) {
      // The review closed while the run started; its shapes are discarded.
      if (!started.existing) void new RunProgress(started.run, this.runDeps(), () => undefined).close();
      return;
    }
    this.agentPending = false;
    // A run already under way for this origin (from a review whose panel
    // closed) is rejoined rather than started again.
    const progress: RunProgress = new RunProgress(started.run, this.runDeps(),
      (run) => void this.agentFinished(run, target, input, progress));
    this.agentProgress = progress;
    this.agentRun.replaceChildren(progress.element);
    this.updateControls(this.actions.isBusy());
    progress.start();
  }

  /** A finished site-draft run. Only the host's judgment of it is shown:
   * draft_site with the run id re-checks its tables as the structural ones
   * are checked. Anything else keeps the basic draft and says why. */
  private async agentFinished(
    run: RunView, target: NonNullable<CaptureSitePanel["target"]>, input: DraftRequest, progress: RunProgress,
  ): Promise<void> {
    if (this.agentProgress !== progress || this.target !== target) return;
    this.agentProgress = null;
    progress.dispose();
    this.agentRun.replaceChildren();
    if (run.state !== "succeeded") {
      this.agentNotUsed(`The AI draft was not used: ${run.state === "canceled" ? "it was canceled" : run.error ?? "the run did not finish"}`);
      return;
    }
    this.agentPending = true;
    this.updateControls(this.actions.isBusy());
    let draft: SiteDrafted;
    try {
      draft = await this.session.draftSite(input.origin, input.visited, input.links, run.run_id);
    } catch (error) {
      if (this.target !== target) return;
      this.agentPending = false;
      this.agentNotUsed(this.agentErrorText(error));
      return;
    }
    if (this.target !== target) return;
    this.agentPending = false;
    if (draft.revision !== target.revision) {
      this.draftProblem = STALE;
      this.updateControls(this.actions.isBusy());
      return;
    }
    if (!draft.adapters.length) {
      const forbidden = draft.dropped.some((row) => row.reason === "draft_forbidden_key");
      this.agentNotUsed(`The AI draft was not used: ${forbidden ? AGENT_FORBIDDEN : AGENT_NO_RULES}`);
      return;
    }
    if (this.draft?.drafter === "structural") this.basicDraft = this.draft;
    this.draft = draft;
    this.agentHarness = run.harness;
    this.renderDraft(draft, target);
  }

  /** Back to the structural draft, with its own default choices. */
  private useBasicDraft(): void {
    const target = this.target;
    const basic = this.basicDraft;
    if (!target || target.mode !== "draft" || !basic || this.saving) return;
    this.basicDraft = null;
    this.agentHarness = null;
    this.draft = basic;
    this.agentRun.replaceChildren();
    this.renderDraft(basic, target);
  }

  private agentErrorText(error: unknown): string {
    if (error instanceof HostError && error.code === "invalid_request" && /unsupported request/.test(error.message)) {
      return AGENT_REBUILD;
    }
    if (error instanceof HostError && error.code === "service_unavailable") {
      return `The AI draft was not used: ${serviceSetupText(error.message)}`;
    }
    return `The AI draft was not used: ${describe(error)}`;
  }

  /** The basic draft stays on screen, with one line saying why. */
  private agentNotUsed(text: string): void {
    const line = document.createElement("p");
    line.className = "warning";
    line.textContent = text;
    this.agentRun.replaceChildren(line);
    this.updateControls(this.actions.isBusy());
  }

  private updatePreview(): void {
    if (!this.target || this.target.mode !== "starter" || !this.session.config || this.saving || !this.freshRulesAllowed) return;
    this.preview.replaceChildren();
    const result = planCaptureSite(this.session.config, this.target.url, {
      organizations: this.orgs.value,
      repositories: this.repos.checked,
      pullRequests: this.prs.checked,
      issues: this.issues.checked,
    });
    const target = this.target;
    const check = ++this.starterCheck;
    this.plan = null;
    this.previewMessage = result.ok ? "Checking saved site identities…" : result.message;
    if (result.ok) {
      const plan = result.plan;
      void this.session.checkCaptureSite(plan.document, target.revision).then(() => {
        if (this.target !== target || this.starterCheck !== check || this.session.revision !== target.revision) return;
        this.plan = plan;
        this.previewMessage = plan.startsCapture
          ? "Saving enables these pages. Other sites keep their existing rules."
          : "Capture is paused for all sites. This saves the site; resume capture in settings when you are ready.";
        this.updateControls(this.actions.isBusy());
      }).catch((error: unknown) => {
        if (this.target !== target || this.starterCheck !== check) return;
        this.previewMessage = `These rules cannot be saved: ${describe(error)}`;
        this.updateControls(this.actions.isBusy());
      });
    }
    if (result.ok) {
      const plan = result.plan;
      const summary = document.createElement("p");
      summary.textContent = `Remember: ${plan.offers.map((offer) => offer.label).join(" and ")}. Recognized pages contribute titles, opens and measured focused/background time to your local worklog.`;
      this.preview.append(summary);
      for (const offer of plan.offers) {
        const line = document.createElement("p");
        line.textContent = offer.description;
        this.preview.append(line);
      }
      const githubOffer = plan.offers.find((offer) => offer.kind === "github");
      if (githubOffer) {
        const scope = githubOffer.adapter!.scope as { owner: string[] };
        const checkedTypes = new Set(plan.offers.map((offer) => offer.artifact_type));
        const labels = GITHUB_RECORD_ORDER.filter((type) => checkedTypes.has(type)).map((type) => GITHUB_RECORD_LABELS[type]!);
        const orgs = document.createElement("p");
        orgs.textContent = `Records ${joinRecordedTypes(labels)} owned by: ${scope.owner.join(", ")}.`;
        this.preview.append(orgs);
      }
      const current = document.createElement("p");
      current.textContent = `This page: ${plan.current.result.match ? "will be captured" : "will not be captured"} — ${plan.current.url}`;
      this.preview.append(current);
      const examples = document.createElement("ul");
      examples.className = "plain-list";
      for (const example of plan.examples) {
        const row = document.createElement("li");
        row.textContent = `Illustrative example (${example.result.match ? "captured" : "skipped"}): ${example.url}`;
        examples.append(row);
      }
      this.preview.append(examples);
      const details = document.createElement("details");
      const title = document.createElement("summary");
      title.textContent = `Safety checks: ${plan.probes.length} root, search, dashboard, login, sharing and auth examples skipped`;
      const probes = document.createElement("ul");
      probes.className = "plain-list";
      for (const probe of plan.probes) {
        const row = document.createElement("li");
        row.textContent = `Skipped: ${probe.url}`;
        probes.append(row);
      }
      details.append(title, probes);
      this.preview.append(details);
    }
    this.updateControls(this.actions.isBusy());
  }

  private async commit(): Promise<void> {
    const target = this.target;
    // A drafted site is saved only after the host re-checks the selection.
    const drafted = target?.mode === "draft" ? this.draftPlan : null;
    const input = this.draftInput;
    const plan: CapturePlan | DraftPlan | RestorePlan | null = target?.mode === "draft" ? drafted : target?.mode === "restore" ? this.restorePlan : this.plan;
    if (!target || !plan || (target.mode === "draft" && !input) || this.save.disabled || this.actions.isBusy()) return;
    this.saving = true;
    this.actions.changed();
    let committed = false;
    try {
      await withConfigMutationLock(async () => {
        const latest = await this.session.readHostConfig();
        if (latest.revision !== target.revision || this.session.revision !== target.revision) {
          throw new Error("Settings changed. Cancel and open Capture Site again to review the current settings.");
        }
        if (latest.config_issue || latest.adapter_issue) throw new Error("Repair the configuration in settings first.");
        const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
        if (!tab || tab.incognito || tab.id !== target.tabId || tab.url !== target.url || this.target !== target) {
          throw new Error("The active page changed. Open Capture Site on the page you want to capture.");
        }
        const state = await worker<WorkerStatus>({ kind: "get_status" });
        if (state.pause_pending || state.pending_removals.includes(plan.origin)) {
          throw new Error("A pause or site removal is pending. Finish it in settings first.");
        }
        // The host re-checks the reviewed selection against the config it
        // holds now, for this revision. A dropped rule or refused scope saves
        // nothing; only plan.document carries the overlays.
        if (drafted && input) {
          await checkDraftedSave(this.session, drafted, target.revision, input);
          if (this.target !== target || !this.samePage(this.page, target)) throw new Error("The active page changed. Open Capture Site again.");
        }
        if (target.mode === "restore") {
          const recorded = await this.session.siteRestore(plan.origin, target.revision);
          if (JSON.stringify(recorded) !== JSON.stringify(this.restored)) throw new Error("Recorded rules changed. Open Capture Site again to review them.");
          await this.session.checkCaptureSite(plan.document, target.revision);
        }
        // Finish old URL-note edits before enabling artifact identity. The
        // editor keeps a copy-out if saving fails, then reloads under the new
        // host-derived key. No autosave can race this config update.
        await this.actions.beforeSave();
        if (this.target !== target || !this.samePage(this.page, target)) throw new Error("The active page changed. Open Capture Site again.");
        const verified = await this.session.saveConfig({
          ...latest.config, adapters: plan.document, capture_enabled: plan.startsCapture,
        }, null, null, () => { committed = true; });
        if (!verified) throw new Error("The host could not verify the saved capture policy.");
      });
      this.close();
      this.actions.show(`Saved capture rules for ${new URL(plan.origin).host}. ${this.session.status?.pause_pending
        ? "A local pause is active; finish it in settings."
        : plan.startsCapture ? "Capture is on." : "Capture stays paused; resume it in settings."}`);
    } catch (error) {
      if (committed) this.close();
      this.actions.show(committed
        ? `Site saved, but capture status could not be refreshed: ${describe(error)}. Reopen the panel to check it.`
        : `Could not capture site: ${describe(error)}`, true);
      // Do not retry/rebase a reviewed policy against a different revision.
      await this.session.reload().catch(() => undefined);
    } finally {
      try {
        await this.actions.afterSave();
      } finally {
        this.saving = false;
        this.actions.changed();
      }
    }
  }
}
