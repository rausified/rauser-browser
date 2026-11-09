import { describe, element } from "./settings.js";
import {
  SharingDrafts, choiceKey, compareDraft, selectionPreview,
  type DraftComparison, type DraftSelection, type SharingDraft,
} from "./sharing-drafts.js";
import type { WorklogChoicesResult, WorklogFocusChoicesResult } from "../protocol/ts/generated.js";

export interface SharingContext { scope: string; revision: string }
interface Options {
  context: () => SharingContext | null;
  selection: () => DraftSelection | null;
  load: (draft: SharingDraft, revision: string, offset: number, snapshot?: string) =>
    Promise<WorklogChoicesResult | WorklogFocusChoicesResult>;
}

/** Saved text and refresh review deliberately never enter the live worklog controls. */
export class SharingDraftPanel {
  private readonly store = new SharingDrafts(() => window.localStorage);
  private readonly name = element<HTMLInputElement>("sharing-draft-name");
  private readonly save = element<HTMLButtonElement>("sharing-draft-save");
  private readonly catalog = element<HTMLSelectElement>("sharing-draft-list");
  private readonly count = element<HTMLElement>("sharing-draft-count");
  private readonly status = element<HTMLElement>("sharing-draft-status");
  private readonly viewer = element<HTMLElement>("sharing-draft-view");
  private readonly original = element<HTMLElement>("sharing-draft-original");
  private readonly copy = element<HTMLButtonElement>("sharing-draft-copy");
  private readonly remove = element<HTMLButtonElement>("sharing-draft-delete");
  private readonly refresh = element<HTMLButtonElement>("sharing-draft-refresh");
  private readonly review = element<HTMLElement>("sharing-draft-review");
  private readonly rows = element<HTMLElement>("sharing-draft-rows");
  private readonly contextArea = element<HTMLElement>("sharing-draft-context");
  private readonly contextText = element<HTMLElement>("sharing-draft-context-text");
  private readonly approveContext = element<HTMLInputElement>("sharing-draft-context-approve");
  private readonly preview = element<HTMLElement>("sharing-draft-preview");
  private readonly copyReview = element<HTMLButtonElement>("sharing-draft-copy-review");
  private readonly saveReview = element<HTMLButtonElement>("sharing-draft-save-review");
  private readonly reviewStatus = element<HTMLElement>("sharing-draft-review-status");
  private drafts: SharingDraft[] = [];
  private selected: SharingDraft | null = null;
  private comparison: DraftComparison | null = null;
  private readonly checked = new Set<string>();
  private signature = "";
  private sequence = 0;
  private busy = false;
  private readError = false;

  constructor(private readonly options: Options) {
    this.name.addEventListener("input", () => this.renderButtons());
    this.save.addEventListener("click", () => this.saveDraft(false));
    this.saveReview.addEventListener("click", () => this.saveDraft(true));
    this.catalog.addEventListener("change", () => this.open());
    this.copy.addEventListener("click", () => void this.copyText(false));
    this.copyReview.addEventListener("click", () => void this.copyText(true));
    this.refresh.addEventListener("click", () => void this.compare());
    this.approveContext.addEventListener("change", () => this.renderPreview());
    this.remove.addEventListener("click", () => {
      if (!this.selected || !this.options.context()) return;
      try {
        this.store.remove(this.selected);
        this.clear();
        this.render();
        this.status.textContent = "Draft deleted. Other versions remain.";
        this.catalog.focus();
      } catch (error) { this.status.textContent = `Could not delete draft: ${describe(error)}`; }
    });
  }

  /** Immediately cancels stale work, including the policy-reload debounce window. */
  invalidate(): void {
    ++this.sequence;
    this.comparison = null;
    this.checked.clear();
    this.busy = false;
    this.review.hidden = true;
    this.preview.textContent = "";
    this.rows.replaceChildren();
    this.contextText.textContent = "";
    this.reviewStatus.textContent = "";
    this.renderButtons();
  }

  render(): void {
    const context = this.options.context();
    const signature = JSON.stringify(context);
    if (signature !== this.signature) {
      this.clear(); this.signature = signature; this.name.value = ""; this.status.textContent = "";
    }
    this.catalog.disabled = !context;
    try {
      this.drafts = context ? this.store.list(context.scope) : [];
      const selected = this.selected?.id ?? "";
      // Keep focus on the native select; never replace the select itself.
      const option = (title: string, value: string): HTMLOptionElement => {
        const item = document.createElement("option"); item.textContent = title; item.value = value; return item;
      };
      this.catalog.replaceChildren(option("Choose a saved draft…", ""));
      for (const draft of this.drafts.slice(0, 20)) {
        this.catalog.add(option(`${draft.name} · ${draft.created_at}`, draft.id));
      }
      this.catalog.value = selected;
      this.count.textContent = `${this.drafts.length} saved draft${this.drafts.length === 1 ? "" : "s"} for this home/profile.` +
        (this.drafts.length > 20 ? " First 20 shown; delete a displayed draft to reach older drafts." : "");
      const latest = this.drafts.find((draft) => draft.id === selected);
      if (selected && (!latest || JSON.stringify(latest) !== JSON.stringify(this.selected))) {
        this.clear();
        this.catalog.value = "";
      }
      if (this.readError) this.status.textContent = "Sharing drafts are available again.";
      this.readError = false;
    } catch (error) {
      this.readError = true;
      this.clear();
      this.drafts = [];
      this.catalog.replaceChildren();
      this.catalog.disabled = true;
      this.count.textContent = "";
      this.status.textContent = `Could not read sharing drafts: ${describe(error)}`;
    }
    this.renderButtons();
  }

  private clear(): void {
    ++this.sequence;
    this.selected = null;
    this.comparison = null;
    this.checked.clear();
    this.busy = false;
    this.viewer.hidden = true;
    this.review.hidden = true;
    this.original.textContent = "";
    this.preview.textContent = "";
    this.rows.replaceChildren();
    this.contextText.textContent = "";
    this.reviewStatus.textContent = "";
    this.renderButtons();
  }

  private renderButtons(): void {
    const context = this.options.context();
    const named = !!this.name.value.trim();
    this.save.disabled = !context || !named || !this.options.selection();
    this.copy.disabled = !context || !this.selected;
    this.remove.disabled = !context || !this.selected || this.busy;
    this.refresh.disabled = !context || !this.selected;
    this.refresh.setAttribute("aria-disabled", String(!context || !this.selected || this.busy));
    const changedContext = this.comparison && this.selected &&
      (this.comparison.heading !== this.selected.heading || this.comparison.footer !== this.selected.footer);
    const reviewed = !!context && !!this.comparison && this.checked.size > 0 &&
      (!changedContext || this.approveContext.checked) && !this.busy;
    this.copyReview.disabled = !reviewed;
    this.saveReview.disabled = !reviewed || !named;
  }

  private open(): void {
    const id = this.catalog.value;
    this.clear();
    this.selected = this.drafts.find((draft) => draft.id === id) ?? null;
    if (this.selected && this.options.context()?.scope === this.selected.scope) {
      this.viewer.hidden = false;
      this.original.textContent = this.selected.preview;
      this.name.value = this.selected.name;
      this.status.textContent = `Saved ${this.selected.created_at}. Frozen preview; nothing copied or refreshed.`;
    } else this.selected = null;
    this.renderButtons();
  }

  private reviewedSelection(): DraftSelection | null {
    if (!this.selected || !this.comparison) return null;
    return { ...this.selected, heading: this.comparison.heading, footer: this.comparison.footer,
      choices: this.comparison.rows.flatMap((row) => row.current && this.checked.has(choiceKey(row.saved)) ? [row.current] : []) };
  }

  private saveDraft(reviewed: boolean): void {
    const context = this.options.context();
    const selection = reviewed ? this.reviewedSelection() : this.options.selection();
    if (!context || !selection || (reviewed ? this.saveReview.disabled : this.save.disabled)) return;
    try {
      const saved = this.store.save(context.scope, this.name.value, selection);
      this.clear();
      this.render();
      if (this.readError) {
        this.status.textContent = "Draft saved, but the catalog could not be reloaded. Reopen the panel to retry.";
        return;
      }
      this.catalog.value = saved.id;
      this.open();
      this.status.textContent = "Draft saved in this browser. The exact preview is below; other versions remain.";
    } catch (error) { this.status.textContent = `Draft not saved: ${describe(error)}`; }
  }

  private async copyText(reviewed: boolean): Promise<void> {
    if (reviewed ? this.copyReview.disabled : this.copy.disabled) return;
    const text = reviewed ? this.preview.textContent : this.selected?.preview;
    if (!text) return;
    const seq = this.sequence;
    try {
      await navigator.clipboard.writeText(text);
      if (seq === this.sequence) this.status.textContent = reviewed ? "Reviewed preview copied." : "Frozen preview copied.";
    } catch (error) {
      if (seq === this.sequence) this.status.textContent = `Could not copy draft: ${describe(error)}`;
    }
  }

  private async compare(): Promise<void> {
    const draft = this.selected;
    const context = this.options.context();
    if (!draft || !context || this.busy) return;
    const seq = ++this.sequence;
    const valid = (): boolean => seq === this.sequence && this.selected === draft &&
      JSON.stringify(this.options.context()) === JSON.stringify(context);
    this.busy = true;
    this.comparison = null;
    this.checked.clear();
    this.review.hidden = true;
    this.status.textContent = "Comparing the saved entries with current evidence…";
    this.renderButtons();
    try {
      const comparison = await compareDraft(draft,
        (offset, snapshot) => this.options.load(draft, context.revision, offset, snapshot), valid);
      if (!valid()) return;
      this.comparison = comparison;
      this.review.hidden = false;
      this.rows.replaceChildren();
      for (const row of comparison.rows) {
        const label = document.createElement("label");
        label.classList.add("check-label");
        const input = document.createElement("input");
        input.type = "checkbox";
        input.disabled = row.current === null;
        input.checked = row.state === "unchanged";
        const key = choiceKey(row.saved);
        if (input.checked) this.checked.add(key);
        input.addEventListener("change", () => {
          if (input.checked && row.current) this.checked.add(key); else this.checked.delete(key);
          this.renderPreview();
        });
        const text = document.createElement("span");
        text.textContent = `${row.state}: ${row.saved.local_date} · ${row.saved.text}\n${row.saved.url}` +
          (row.state === "changed" ? `\nCurrent: ${row.current?.text}\n${row.current?.url}` : "");
        label.append(input, text);
        this.rows.append(label);
      }
      const changed = comparison.heading !== draft.heading || comparison.footer !== draft.footer;
      this.contextArea.hidden = !changed;
      this.approveContext.checked = !changed;
      this.contextText.textContent = `Saved session/gap context:\n${draft.heading}${draft.footer || "No gap notices included."}\n\nCurrent session/gap context:\n${comparison.heading}${comparison.footer || "No gap notices included."}`;
      const counts = comparison.rows.reduce<Record<string, number>>((all, row) => {
        all[row.state] = (all[row.state] ?? 0) + 1; return all;
      }, {});
      this.reviewStatus.textContent = Object.entries(counts).map(([state, count]) => `${count} ${state}`).join(" · ") +
        (comparison.incomplete ? ". Unreached entries may be outside the scan limits; they are not proven missing." : ".") +
        " New activity is excluded. Changed entries start unchecked; missing and unreached entries cannot enter the refreshed preview.";
      this.status.textContent = `Compared at ${new Date().toISOString()}. The saved preview above is unchanged.`;
    } catch (error) {
      if (valid()) this.status.textContent = `Could not compare draft: ${describe(error)}`;
    } finally {
      if (valid()) { this.busy = false; this.renderPreview(); }
    }
  }

  private renderPreview(): void {
    const selection = this.reviewedSelection();
    this.preview.textContent = selection?.choices.length ? selectionPreview(selection) : "Choose current entries to build a refreshed preview.";
    this.renderButtons();
  }
}
