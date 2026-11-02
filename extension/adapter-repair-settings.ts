import type { AdapterDocument } from "../protocol/ts/generated.js";
import { matchText, previewDocument, validateAdapterDocument } from "./adapter-review.js";
import { HostError, newRequestId, PROTOCOL_VERSION } from "./native.js";
import { ConfigSession, describe, element, siteOrigins, withConfigMutationLock } from "./settings.js";

/** An inert document is never interpreted or downgraded into a new policy.
 * The preserved values and the explicitly authored replacement stay separate. */
export class AdapterRepairControls {
  private readonly open = element<HTMLButtonElement>("repair-adapters");
  private readonly workspace = element<HTMLDivElement>("adapter-repair");
  private readonly heading = element<HTMLHeadingElement>("adapter-repair-heading");
  private readonly original = element<HTMLTextAreaElement>("adapter-repair-original");
  private readonly copy = element<HTMLButtonElement>("adapter-repair-copy");
  private readonly replacement = element<HTMLTextAreaElement>("adapter-repair-json");
  private readonly validation = element<HTMLOutputElement>("adapter-repair-validation");
  private readonly url = element<HTMLInputElement>("adapter-repair-url");
  private readonly title = element<HTMLInputElement>("adapter-repair-title");
  private readonly preview = element<HTMLUListElement>("adapter-repair-preview");
  private readonly save = element<HTMLButtonElement>("adapter-repair-save");
  private readonly cancel = element<HTMLButtonElement>("adapter-repair-cancel");
  private revision: string | null = null;

  constructor(
    private readonly session: ConfigSession,
    private readonly show: (message: string, warning?: boolean) => void,
    private readonly busy: () => boolean,
    private readonly setBusy: (value: boolean) => void,
  ) {
    this.open.addEventListener("click", () => void this.begin());
    this.copy.addEventListener("click", () => void this.copyOriginal());
    for (const control of [this.replacement, this.url, this.title]) {
      control.addEventListener("input", () => this.render());
    }
    this.save.addEventListener("click", () => void this.submit());
    this.cancel.addEventListener("click", () => this.close());
    this.workspace.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && !this.busy()) {
        event.preventDefault();
        this.close();
      }
    });
  }

  private candidate(): { document: AdapterDocument; message: string } | { document: null; message: string } {
    if (this.replacement.value.length > 65_536) {
      return { document: null, message: "The replacement cannot exceed 65,536 characters." };
    }
    try {
      const value: unknown = JSON.parse(this.replacement.value);
      if (typeof value !== "object" || value === null || Array.isArray(value)) {
        return { document: null, message: "Enter a complete format-1 JSON document with format and artifacts." };
      }
      const document = value as AdapterDocument;
      const issue = validateAdapterDocument(document);
      return issue ? { document: null, message: issue } : {
        document, message: `Valid here: ${document.artifacts.length} adapter(s). The host rechecks the full replacement and recorded identities before saving.`,
      };
    } catch (error) {
      return { document: null, message: `The replacement is not valid JSON: ${describe(error)}` };
    }
  }

  render(): void {
    const unavailable = !this.session.connected || this.busy() || this.session.configIssue !== null;
    const repairable = this.session.adapterIssue !== null;
    this.open.hidden = !repairable;
    this.open.disabled = unavailable || !repairable;
    this.open.textContent = this.revision === null ? "Repair adapter document…" : "Start a new repair…";
    this.copy.disabled = this.busy() || this.revision === null;
    this.cancel.disabled = this.busy();
    this.replacement.disabled = this.busy();
    this.url.disabled = this.busy();
    this.title.disabled = this.busy();
    if (this.workspace.hidden) return;
    const candidate = this.candidate();
    const stale = this.revision !== this.session.revision || !repairable;
    this.save.disabled = unavailable || stale || candidate.document === null;
    this.validation.textContent = stale
      ? "Settings changed. Your replacement is kept here for copying; start a new repair to review the current document before saving."
      : candidate.message;
    this.validation.classList.toggle("warning", stale || candidate.document === null);
    this.preview.replaceChildren();
    if (!candidate.document) return;
    // Every represented site gets the fixed non-artifact probes, including a
    // multi-site replacement with no example entered by the user.
    const origins = siteOrigins(candidate.document);
    const urls = new Set<string>();
    for (const origin of origins.length ? origins : [undefined]) {
      const result = previewDocument(candidate.document, this.url.value, this.title.value, true, origin);
      if (!result.ok) continue;
      for (const row of result.rows) {
        const key = `${row.probe ? "probe" : "example"}:${row.url}`;
        if (urls.has(key)) continue;
        urls.add(key);
        const item = document.createElement("li");
        item.textContent = `${row.probe ? "Probe" : "Example"}: ${row.url} — ${matchText(row.result)}`;
        if (row.probe && row.result.match) item.className = "probe-match";
        this.preview.append(item);
      }
    }
  }

  private async begin(): Promise<void> {
    if (this.open.disabled || !this.session.revision) return;
    this.setBusy(true);
    try {
      const expected = this.session.revision;
      const result = await this.session.host.call({
        type: "get_adapter_repair", protocol_version: PROTOCOL_VERSION,
        request_id: newRequestId(), expected_revision: expected,
      }, "adapter_repair_loaded");
      if (result.revision !== expected) throw new Error("Settings changed; reopen adapter repair.");
      this.revision = result.revision;
      this.original.value = result.original_toml;
      this.replacement.value = JSON.stringify({ format: 1, artifacts: [] }, null, 2);
      this.url.value = "";
      this.title.value = "";
      this.workspace.hidden = false;
      this.workspace.scrollIntoView({ block: "start", behavior: "smooth" });
      this.heading.focus({ preventScroll: true });
    } catch (error) {
      this.show(`Could not open adapter repair: ${describe(error)}`, true);
    } finally {
      this.setBusy(false);
      this.render();
    }
  }

  private async copyOriginal(): Promise<void> {
    if (this.copy.disabled) return;
    try {
      await navigator.clipboard.writeText(this.original.value);
      this.show("Preserved adapter values copied. The on-disk original is backed up when a replacement is saved.");
    } catch (error) {
      this.show(`Could not copy preserved values: ${describe(error)}. Select and copy the text above.`, true);
    }
  }

  private close(): void {
    if (this.busy()) return;
    this.workspace.hidden = true;
    this.revision = null;
    this.render();
    if (this.open.hidden) element<HTMLButtonElement>("new-adapter").focus();
    else this.open.focus();
  }

  private async submit(): Promise<void> {
    this.render();
    const candidate = this.candidate();
    const expected = this.revision;
    if (this.save.disabled || !candidate.document || !expected) return;
    this.setBusy(true);
    let saved = false;
    try {
      const verified = await withConfigMutationLock(() => this.session.replaceAdapters(candidate.document!, expected));
      if (!verified) {
        throw new Error("The host could not verify the saved policy. Capture stays suspended; reload settings to check it.");
      }
      this.workspace.hidden = true;
      this.revision = null;
      saved = true;
      this.show("Adapter document replaced; the original config is backed up beside config.toml. Capture keeps its previous on/off setting.");
    } catch (error) {
      this.show(`Could not replace artifact adapters: ${describe(error)}`, true);
      if (error instanceof HostError && error.code === "conflict") {
        await this.session.reload().catch(() => undefined);
      }
    } finally {
      this.setBusy(false);
      this.render();
      if (saved) element<HTMLButtonElement>("new-adapter").focus();
    }
  }
}
