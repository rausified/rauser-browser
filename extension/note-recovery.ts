import { storageKey } from "./brand.js";
import type { UnsavedText } from "./note-editor.js";

const PREFIX = `${storageKey("note_recovery_v1")}:`;

export interface RecoveryText extends UnsavedText {
  id: string;
}

/** Synchronous writes happen in the input handler, before a panel can close.
 * Every snapshot gets a new key, so a save/discard can never remove a newer
 * edit between a storage read and remove in another panel. No shared list. */
export class NoteRecovery {
  constructor(private readonly storage: () => Storage) {}

  list(): RecoveryText[] {
    const entries: RecoveryText[] = [];
    const storage = this.storage();
    for (let index = 0; index < storage.length; index += 1) {
      const key = storage.key(index);
      if (!key?.startsWith(PREFIX)) continue;
      const raw = storage.getItem(key);
      let entry: unknown;
      try { entry = JSON.parse(raw ?? "null"); } catch { continue; }
      if (typeof entry !== "object" || entry === null) continue;
      const value = entry as Record<string, unknown>;
      if (typeof value.id !== "string" || key !== PREFIX + value.id ||
          typeof value.url !== "string" || typeof value.text !== "string" ||
          typeof value.reason !== "string") continue;
      // Malformed entries stay untouched; nothing from storage becomes HTML,
      // a navigation, or an automatic save request.
      entries.push(value as unknown as RecoveryText);
    }
    return entries;
  }

  write(text: UnsavedText): RecoveryText {
    const entry: RecoveryText = { ...text, id: crypto.randomUUID() };
    this.storage().setItem(PREFIX + entry.id, JSON.stringify(entry));
    return entry;
  }

  remove(entry: RecoveryText): void {
    const key = PREFIX + entry.id;
    const storage = this.storage();
    if (storage.getItem(key) === JSON.stringify(entry)) storage.removeItem(key);
  }
}
