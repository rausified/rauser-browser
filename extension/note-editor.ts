// Page note editor state for the side panel (§5.3, §5.7). This module has no
// DOM or Chrome dependencies so the tests can drive it directly.
//
// Every load and save runs on one promise chain, so at most one host request
// is in flight and the note the editor is bound to cannot change under a
// running save. Each page change bumps a refresh token; a queued or in-flight
// switch whose token is no longer current does nothing further, so only the
// latest active tab's note is loaded into the editor.

import type { OlderNote, RecordKey } from "../protocol/ts/generated.js";
import type { NoteRecovery, RecoveryText } from "./note-recovery.js";

/** Matches MAX_TITLE_BYTES in host/src/note.rs, which clamps longer titles. */
export const MAX_NOTE_TITLE_BYTES = 2_048;

/** Cuts `text` to at most `maxBytes` of UTF-8 without splitting a character. */
export function clampUtf8(text: string, maxBytes: number): string {
  let bytes = 0;
  let end = 0;
  for (const char of text) {
    const code = char.codePointAt(0)!;
    // A lone surrogate encodes as U+FFFD, which is also 3 bytes.
    const size = code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    if (bytes + size > maxBytes) break;
    bytes += size;
    end += char.length;
  }
  return text.slice(0, end);
}

/** A page whose note the panel should show, or null for no usable page. */
export type NotePage = { url: string; title: string } | null;

/** Text the host would not take, kept so the user can copy it back in. */
export interface UnsavedText {
  url: string;
  text: string;
  reason: string;
}

export interface NoteView {
  readBody(): string;
  writeBody(body: string): void;
  setEditable(editable: boolean): void;
  setStatus(message: string, warning?: boolean): void;
  showUnsaved(entries: readonly RecoveryText[]): void;
  setRecoveryWarning?(message: string): void;
  showOlderNotes?(entries: readonly OlderNote[], canUseAsArtifactNote: boolean): void;
}

export interface NoteHost {
  load(url: string): Promise<{ exists: boolean; revision: string; body: string; key: RecordKey; older_notes: OlderNote[] }>;
  save(request: { url: string; title: string; body: string; expected_revision: string; expected_key: RecordKey }): Promise<
    { type: "note_saved"; revision: string; key: RecordKey } |
    { type: "note_conflict"; exists: boolean; revision: string; body: string; key: RecordKey; reason: string }
  >;
}

const KEY_CHANGED_STATUS = "Could not save; this page's artifact identity changed.";

export class NoteEditor {
  private queue: Promise<void> = Promise.resolve();
  private pending = 0;
  private target: NotePage = null;
  private token = 0;
  private saveTimer: ReturnType<typeof setTimeout> | null = null;
  private readonly unsaved: RecoveryText[] = [];
  // The note the editor is bound to. Only queued work changes these.
  private url: string | null = null;
  private title = "";
  private revision: string | null = null;
  private key: RecordKey | null = null;
  private olderNotes: readonly OlderNote[] = [];
  private canUseOlderNote = false;
  private loaded = false;
  private loadFailed = false;
  private dirty = false;
  private saveError = "";
  private draft: RecoveryText | null = null;

  constructor(
    private readonly view: NoteView,
    private readonly host: NoteHost,
    private readonly describe: (error: unknown) => string,
    private readonly debounceMs = 1_000,
    private readonly recovery?: NoteRecovery,
  ) {
    try {
      this.unsaved.push(...(recovery?.list() ?? []));
      this.view.showUnsaved(this.unsaved);
    } catch (error) {
      this.recoveryFailed(error);
    }
  }

  /** Resolves once all load and save work queued so far has finished. */
  idle(): Promise<void> {
    return this.queue;
  }

  /** Called on every edit of the note text. */
  edited(): void {
    if (!this.loaded) return;
    this.dirty = true;
    this.rememberDraft(this.view.readBody());
    this.view.setStatus("");
    this.clearTimer();
    this.saveTimer = setTimeout(() => {
      this.saveTimer = null;
      void this.flush();
    }, this.debounceMs);
  }

  /** Saves pending edits now, after any load or save already running. */
  flush(): Promise<void> {
    this.clearTimer();
    // Taken now, not when the save runs, so a page change queued behind this
    // flush is seen as newer.
    const token = this.token;
    return this.enqueue(async () => {
      if (!(await this.saveNow())) return;
      // The host now maps this page to another key (§6.2). Reload it the way
      // reload() does, in this same queued step so no edit or autosave can
      // reach the stale key first. A page change queued meanwhile loads the
      // newest page itself, so it needs no reload here.
      if (token !== this.token) return;
      const reloadToken = ++this.token;
      await this.switchTo(this.target, reloadToken);
      if (reloadToken === this.token && this.loaded) this.view.setStatus(KEY_CHANGED_STATUS, true);
    });
  }

  /** Follows the active tab. Saves the note being left before loading the new
   * one, and retries a failed load when asked for the same page again. */
  showPage(page: NotePage): Promise<void> {
    if (page !== null) page = { url: page.url, title: clampUtf8(page.title, MAX_NOTE_TITLE_BYTES) };
    const samePage = page?.url === this.target?.url;
    this.target = page;
    if (samePage) {
      if (page !== null && this.url === page.url) this.title = page.title;
      const retry = page !== null && this.url === page.url && this.loadFailed;
      if (!retry) return this.queue;
    }
    const token = ++this.token;
    // Stop edits to the old page's text while its header is gone; what was
    // already typed is saved by the switch below.
    this.view.setEditable(false);
    this.clearTimer();
    return this.enqueue(() => this.switchTo(page, token));
  }

  /** Loads the current page's note again if the last load failed, for
   * example after the host configuration changed. */
  retry(): Promise<void> {
    return this.showPage(this.target);
  }

  /** Force a fresh host load after a note migration or config revision change. */
  reload(): Promise<void> {
    const page = this.target;
    this.target = null;
    return this.showPage(page);
  }

  /** Explicit discard affects only the selected copy, never live edits in
   * this panel or a newer snapshot written by another panel. */
  discardUnsaved(id: string): void {
    const index = this.unsaved.findIndex((entry) => entry.id === id);
    if (index === -1) return;
    try {
      this.recovery?.remove(this.unsaved[index]!);
      this.unsaved.splice(index, 1);
      this.view.showUnsaved(this.unsaved);
    } catch (error) {
      this.recoveryFailed(error);
    }
  }

  private recoveryFailed(error: unknown): void {
    this.view.setRecoveryWarning?.(`Could not keep or clear a local recovery copy: ${this.describe(error)}. Keep this panel open until your note is saved.`);
  }

  private rememberDraft(text: string, reason = "This edit has no confirmed save. Check the current note before copying it back."): void {
    if (!this.recovery || this.url === null) return;
    try {
      const previous = this.draft;
      this.draft = this.recovery.write({ url: this.url, text, reason });
      // Publish the new copy before retiring the previous one. Distinct keys
      // avoid cross-panel read/remove races; a failed write leaves old bytes.
      if (previous) this.recovery.remove(previous);
      this.view.setRecoveryWarning?.("");
    } catch (error) {
      this.recoveryFailed(error);
    }
  }

  private enqueue(work: () => Promise<void>): Promise<void> {
    // Start at once when nothing is running, so a flush from `pagehide` sends
    // its request before the panel disconnects from the host.
    this.pending += 1;
    const next = this.pending === 1 ? work() : this.queue.then(work);
    // Work handles its own errors; keep the chain alive regardless.
    this.queue = next.catch(() => undefined).finally(() => {
      this.pending -= 1;
    });
    return this.queue;
  }

  private clearTimer(): void {
    if (this.saveTimer !== null) {
      clearTimeout(this.saveTimer);
      this.saveTimer = null;
    }
  }

  private keepUnsaved(url: string, text: string, reason: string): void {
    this.rememberDraft(text, reason);
    this.unsaved.push(this.draft?.text === text && this.draft.url === url && this.draft.reason === reason
      ? this.draft : { id: crypto.randomUUID(), url, text, reason });
    this.draft = null; // Later edits must not replace this copy-out.
    this.view.showUnsaved(this.unsaved);
  }

  private async switchTo(page: NotePage, token: number): Promise<void> {
    if (token !== this.token) return; // A newer page change will run next.
    await this.saveNow();
    if (this.dirty && this.url !== null) {
      // The save failed. Never drop the text: keep it for the user to copy.
      this.keepUnsaved(this.url, this.view.readBody(), this.saveError);
    }
    this.url = page?.url ?? null;
    // The tab's title may have changed while this switch waited its turn.
    this.title = page !== null && this.target?.url === page.url ? this.target.title : page?.title ?? "";
    this.revision = null;
    this.key = null;
    this.olderNotes = [];
    this.canUseOlderNote = false;
    this.loaded = false;
    this.loadFailed = false;
    this.dirty = false;
    this.view.setEditable(false);
    this.view.writeBody("");
    this.view.setStatus("");
    this.view.showOlderNotes?.([], false);
    if (page === null || token !== this.token) return;
    try {
      const response = await this.host.load(page.url);
      if (token !== this.token) return;
      this.revision = response.revision;
      this.key = response.key;
      this.olderNotes = response.older_notes;
      this.canUseOlderNote = response.key.kind === "artifact" && !response.exists;
      this.view.showOlderNotes?.(this.olderNotes, this.canUseOlderNote);
      this.view.writeBody(response.body);
      this.loaded = true;
      this.view.setEditable(true);
      this.view.setStatus(response.exists ? "Saved" : "");
    } catch (error) {
      if (token !== this.token) return;
      this.loadFailed = true;
      this.view.setStatus(`Could not load this page's note: ${this.describe(error)}`, true);
    }
  }

  /** Saves pending edits. Returns true when the host refused the save because
   * the page's key changed; the editor is then closed until it reloads. */
  private async saveNow(): Promise<boolean> {
    this.clearTimer();
    if (!this.dirty || !this.loaded || this.url === null || this.revision === null || this.key === null) return false;
    const url = this.url;
    const body = this.view.readBody();
    // Retry a failed local write before dispatch; asynchronous teardown must
    // never be the first attempt to keep the user's text.
    this.rememberDraft(body);
    const draft = this.draft;
    this.dirty = false;
    this.view.setStatus("Saving…");
    let response: Awaited<ReturnType<NoteHost["save"]>>;
    try {
      response = await this.host.save({
        url, title: this.title, body, expected_revision: this.revision, expected_key: this.key,
      });
    } catch (error) {
      this.dirty = true; // Retry on the next edit, tab change, or close.
      this.saveError = `Could not save: ${this.describe(error)}`;
      this.view.setStatus(this.saveError, true);
      return false;
    }
    this.revision = response.revision;
    this.key = response.key ?? this.key;
    // Creating the shared note (here or in another panel) makes migration
    // unavailable immediately. A stale save may also report a deletion.
    const canUseOlderNote = this.key.kind === "artifact" && response.type === "note_conflict" &&
      response.reason !== "key_changed" && !response.exists;
    if (canUseOlderNote !== this.canUseOlderNote) {
      this.canUseOlderNote = canUseOlderNote;
      this.view.showOlderNotes?.(this.olderNotes, canUseOlderNote);
    }
    if (response.type === "note_saved") {
      if (draft) {
        try {
          this.recovery?.remove(draft);
          if (this.draft === draft) this.draft = null;
        } catch (error) {
          this.recoveryFailed(error);
        }
      }
      // Edits typed during the save are still pending; their timer saves them.
      if (!this.dirty) this.view.setStatus("Saved");
      return false;
    }
    // The note changed elsewhere since this panel last loaded it. Nothing was
    // written; show the newer note and keep everything typed, including edits
    // made while this save was in flight (§4.4, §5.3).
    this.clearTimer();
    this.dirty = false;
    const keyChanged = response.reason === "key_changed";
    const reason = keyChanged
      ? "This page's artifact identity changed, so your edit was not saved."
      : "This note changed elsewhere, so your edit was not saved.";
    this.keepUnsaved(url, this.view.readBody(), reason);
    if (keyChanged) {
      // The note under the old key is gone for this page: stop edits and
      // autosaves until the caller reloads the note under its new key. The
      // copy-out box keeps the text; a reload never clears it.
      this.loaded = false;
      this.view.setEditable(false);
    }
    this.view.writeBody(response.body);
    this.view.setStatus(keyChanged ? KEY_CHANGED_STATUS : "Could not save; this note changed elsewhere.", true);
    return keyChanged;
  }
}
