import type { WorkerStatus } from "./model.js";
import { ConfigSession, describe, element, withConfigMutationLock, worker } from "./settings.js";
import { captureOrigin, planCaptureSite, siteConfigured, type CapturePlan } from "./capture-site.js";
import { starterKind } from "./starter-adapters.js";

export interface CapturePage {
  url: string;
  tabId: number | undefined;
}

interface CaptureActions {
  isBusy(): boolean;
  changed(): void;
  show(message: string, warning?: boolean): void;
  beforeSave(): Promise<void>;
  afterSave(): Promise<void>;
}

/** An in-panel, revision-bound review. All transient page URLs and scopes
 * remain in this panel; only the reviewed config goes to the native host. */
export class CaptureSitePanel {
  private readonly area = element<HTMLElement>("capture-site-area");
  private readonly start = element<HTMLButtonElement>("capture-site");
  private readonly hint = element<HTMLElement>("capture-site-hint");
  private readonly review = element<HTMLElement>("capture-site-review");
  private readonly heading = element<HTMLElement>("capture-site-heading");
  private readonly github = element<HTMLElement>("capture-site-github");
  private readonly prs = element<HTMLInputElement>("capture-site-prs");
  private readonly prOrgs = element<HTMLInputElement>("capture-site-pr-orgs");
  private readonly issues = element<HTMLInputElement>("capture-site-issues");
  private readonly issueOrgs = element<HTMLInputElement>("capture-site-issue-orgs");
  private readonly preview = element<HTMLElement>("capture-site-preview");
  private readonly message = element<HTMLElement>("capture-site-message");
  private readonly save = element<HTMLButtonElement>("capture-site-save");
  private readonly cancel = element<HTMLButtonElement>("capture-site-cancel");
  private page: CapturePage | null = null;
  private target: (CapturePage & { revision: string }) | null = null;
  private plan: CapturePlan | null = null;
  private previewMessage = "";
  saving = false;

  constructor(private readonly session: ConfigSession, private readonly actions: CaptureActions) {
    this.start.addEventListener("click", () => this.open());
    this.cancel.addEventListener("click", () => {
      if (this.saving) return;
      this.close();
      this.start.focus();
    });
    this.save.addEventListener("click", () => void this.commit());
    for (const input of [this.prs, this.prOrgs, this.issues, this.issueOrgs]) {
      input.addEventListener("input", () => this.updatePreview());
    }
  }

  render(page: CapturePage | null, busy: boolean): void {
    this.page = page;
    if (this.target && !this.samePage(page, this.target)) this.close();
    const origin = page ? captureOrigin(page.url) : null;
    this.area.hidden = !origin;
    if (!origin) return;
    const config = this.session.config;
    const configured = config !== null && siteConfigured(config, origin);
    this.start.hidden = configured || this.target !== null;
    this.start.disabled = configured || busy || !this.session.connected || !config?.storage ||
      !!this.session.configIssue || !!this.session.adapterIssue;
    this.hint.textContent = configured ? "This site has capture rules. Manage them in settings."
      : !config?.storage ? "Choose a notes folder in settings, then capture this site here."
      : this.session.configIssue || this.session.adapterIssue ? "Repair the configuration in settings before adding a site."
      : "Remember work on this site without configuring rules by hand.";
    if (this.target) this.updateControls(busy);
  }

  private samePage(a: CapturePage | null, b: CapturePage): boolean {
    return a?.url === b.url && a.tabId === b.tabId;
  }

  private open(): void {
    if (this.actions.isBusy() || !this.page || !this.session.revision || this.start.disabled || this.start.hidden) return;
    this.target = { ...this.page, revision: this.session.revision };
    this.review.hidden = false;
    this.start.hidden = true;
    const origin = captureOrigin(this.page.url)!;
    this.heading.textContent = `Capture work on ${new URL(origin).host}`;
    this.github.hidden = starterKind(origin) !== "github";
    this.prs.checked = true;
    this.issues.checked = true;
    this.prOrgs.value = "";
    this.issueOrgs.value = "";
    this.updatePreview();
    this.heading.focus();
  }

  private close(): void {
    this.target = null;
    this.plan = null;
    this.review.hidden = true;
    this.prOrgs.value = "";
    this.issueOrgs.value = "";
    this.preview.replaceChildren();
    this.start.hidden = false;
  }

  private updateControls(busy: boolean): void {
    const stale = this.target?.revision !== this.session.revision;
    const state = this.session.status;
    const origin = this.plan?.origin ?? (this.target ? captureOrigin(this.target.url) : null);
    const removal = origin !== null && state?.pending_removals.includes(origin);
    const problem = stale ? "Settings changed. Cancel and open Capture Site again to review the current settings."
      : this.session.configIssue || this.session.adapterIssue ? "Repair the configuration in settings first."
      : state?.pause_pending ? "A pause is still being saved. Finish it in settings before capturing a site."
      : removal ? "This site has a pending removal. Finish it in settings before adding it again."
      : null;
    this.message.textContent = problem ?? this.previewMessage;
    this.save.disabled = busy || !this.plan || !!problem || !this.session.connected;
    this.cancel.disabled = this.saving;
    for (const input of [this.prs, this.prOrgs, this.issues, this.issueOrgs]) input.disabled = this.saving || stale;
    this.prOrgs.disabled ||= !this.prs.checked;
    this.issueOrgs.disabled ||= !this.issues.checked;
    this.save.textContent = this.saving ? "Saving…" : this.plan?.startsCapture ? "Start capturing this site" : "Save site · keep capture paused";
  }

  private updatePreview(): void {
    if (!this.target || !this.session.config || this.saving) return;
    this.preview.replaceChildren();
    const result = planCaptureSite(this.session.config, this.target.url, {
      pullRequests: this.prs.checked ? this.prOrgs.value : null,
      issues: this.issues.checked ? this.issueOrgs.value : null,
    });
    this.plan = result.ok ? result.plan : null;
    this.previewMessage = result.ok
      ? result.plan.startsCapture ? "Saving enables these pages. Other sites keep their existing rules."
        : "Capture is paused for all sites. This saves the site; resume capture in settings when you are ready."
      : result.message;
    if (result.ok) {
      const plan = result.plan;
      const summary = document.createElement("p");
      summary.textContent = `Remember: ${plan.offers.map((offer) => offer.label).join(" and ")}. Recognized pages contribute titles, opens and measured focused/background time to your local worklog.`;
      this.preview.append(summary);
      for (const offer of plan.offers) {
        const line = document.createElement("p");
        line.textContent = offer.description;
        if (offer.kind === "github") {
          const scope = offer.adapter!.scope as { owner: string[] };
          line.textContent += ` Organizations for ${offer.label}: ${scope.owner.join(", ")}.`;
        }
        this.preview.append(line);
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
    const plan = this.plan;
    if (!target || !plan || this.save.disabled || this.actions.isBusy()) return;
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
