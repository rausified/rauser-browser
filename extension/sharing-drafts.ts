import { storageKey } from "./brand.js";
import type {
  StorageConfig, WorklogChoice, WorklogChoicesResult, WorklogFocusChoicesResult, WorklogReported,
} from "../protocol/ts/generated.js";

const PREFIX = `${storageKey("sharing_draft_v1")}:`;
const MAX_BYTES = 256 * 1024;
const MAX_DRAFTS = 20;
const MAX_ROWS = 2000;
const MAX_PAGES = 50;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
type Choices = WorklogChoicesResult | WorklogFocusChoicesResult;
type Gaps = Pick<WorklogReported, "gaps" | "gaps_truncated">;

export interface DraftSelection {
  start_date: string;
  end_date: string;
  focus_session_id: string | null;
  heading: string;
  footer: string;
  include_gaps: boolean;
  choices: WorklogChoice[];
}

export interface SharingDraft extends DraftSelection {
  format: 1;
  id: string;
  scope: string;
  name: string;
  created_at: string;
  preview: string;
}

export const choiceKey = (row: WorklogChoice): string => JSON.stringify([row.artifact_id, row.local_date]);
export const draftScope = (storage: StorageConfig): string => JSON.stringify([storage.root, storage.profile]);

export function gapLines(report: Gaps): string[] {
  const lines = report.gaps.map((gap) =>
    `${gap.count} completed activity-hour record${gap.count === 1 ? "" : "s"} missing from ${gap.first_occurred_at} through ${gap.last_occurred_at}.`);
  if (report.gaps_truncated) lines.push("More capture gaps are recorded; only the first 32 notices are shown.");
  return lines;
}

export function gapFooter(report: Gaps, include: boolean, curated = true): string {
  const lines = gapLines(report);
  return include && lines.length
    ? `\n\nCapture gaps: ${curated ? "this period has missing activity" : "this worklog is incomplete"}. Counts cover full reported ranges, may overlap, and are not added to activity totals.\n${lines.join("\n")}`
    : "";
}

export function focusHeading(report: Choices): string {
  return report.type === "worklog_focus_choices_result" && report.selected_session
    ? `Focus session: ${report.selected_session.name}${report.name_changed ? " (name changed in this period)" : ""}\n` : "";
}

export function selectionPreview(selection: DraftSelection): string {
  const rows = selection.choices;
  return `${selection.heading}Selected work: ${selection.start_date} through ${selection.end_date}\n${rows.length} artifact-day entr${rows.length === 1 ? "y" : "ies"}\n\n${rows.map((row) => `${row.local_date} · ${row.text}\n${row.url}`).join("\n\n")}${selection.footer}`;
}

function text(value: unknown, max: number): value is string {
  return typeof value === "string" && value.length <= max;
}

function date(value: unknown): value is string {
  return typeof value === "string" && /^\d{4}-\d{2}-\d{2}$/.test(value) &&
    Number.isFinite(Date.parse(value)) && new Date(value).toISOString().slice(0, 10) === value;
}

function validDraft(value: unknown): value is SharingDraft {
  if (typeof value !== "object" || value === null) return false;
  const v = value as Record<string, unknown>;
  if (v.format !== 1 || !text(v.id, 36) || !UUID.test(v.id) ||
      !text(v.scope, 16384) || !text(v.name, 80) || !v.name.trim() ||
      [...v.name].some((char) => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127) ||
      !text(v.created_at, 24) || !Number.isFinite(Date.parse(v.created_at)) || new Date(v.created_at).toISOString() !== v.created_at ||
      !date(v.start_date) || !date(v.end_date) || v.start_date > v.end_date ||
      Date.parse(v.end_date) - Date.parse(v.start_date) > 6 * 86400000 ||
      !(v.focus_session_id === null || (text(v.focus_session_id, 36) && UUID.test(v.focus_session_id))) ||
      !text(v.heading, 4096) || !text(v.footer, 32768) || typeof v.include_gaps !== "boolean" ||
      !text(v.preview, 131072) || !Array.isArray(v.choices) || !v.choices.length || v.choices.length > MAX_ROWS) return false;
  const keys = new Set<string>();
  for (const row of v.choices) {
    if (typeof row !== "object" || row === null || !text(row.artifact_id, 4096) || !row.artifact_id ||
        !date(row.local_date) || row.local_date < v.start_date || row.local_date > v.end_date ||
        !text(row.text, 32768) || !text(row.url, 16384)) return false;
    const key = choiceKey(row as WorklogChoice);
    if (keys.has(key)) return false;
    keys.add(key);
  }
  return v.preview === selectionPreview(value as SharingDraft);
}

/** Independent immutable keys keep concurrent panels from replacing a shared list. */
export class SharingDrafts {
  constructor(private readonly storage: () => Storage) {}

  list(scope: string): SharingDraft[] {
    const storage = this.storage();
    const drafts: SharingDraft[] = [];
    for (let index = 0; index < storage.length; index += 1) {
      const key = storage.key(index);
      if (!key?.startsWith(PREFIX)) continue;
      const raw = storage.getItem(key);
      if (!raw || raw.length > MAX_BYTES) continue;
      let value: unknown;
      try { value = JSON.parse(raw); } catch { continue; }
      if (validDraft(value) && key === PREFIX + value.id && value.scope === scope &&
          new TextEncoder().encode(raw).length <= MAX_BYTES) drafts.push(value);
    }
    return drafts.sort((a, b) => b.created_at.localeCompare(a.created_at) || a.id.localeCompare(b.id));
  }

  save(scope: string, name: string, selection: DraftSelection): SharingDraft {
    const draft: SharingDraft = { ...structuredClone(selection), format: 1, id: crypto.randomUUID(),
      scope, name: name.trim(), created_at: new Date().toISOString(), preview: selectionPreview(selection) };
    const raw = JSON.stringify(draft);
    if (!validDraft(draft) || new TextEncoder().encode(raw).length > MAX_BYTES) {
      throw new Error("Use a name of 1–80 characters and a smaller selection (at most 2,000 entries and 256 KiB).");
    }
    if (this.list(scope).length >= MAX_DRAFTS) throw new Error("This home already has 20 drafts. Delete a draft before saving another.");
    this.storage().setItem(PREFIX + draft.id, raw);
    return draft;
  }

  remove(draft: SharingDraft): void {
    const storage = this.storage();
    const key = PREFIX + draft.id;
    if (storage.getItem(key) !== JSON.stringify(draft)) throw new Error("This draft changed elsewhere. Reopen it before deleting.");
    storage.removeItem(key);
  }
}

export interface ComparedChoice {
  saved: WorklogChoice;
  current: WorklogChoice | null;
  state: "unchanged" | "changed" | "missing" | "unreached";
}
export interface DraftComparison {
  rows: ComparedChoice[];
  heading: string;
  footer: string;
  incomplete: boolean;
}

/** Compare only saved identities. Never infer deletion from a bounded/omitted scan. */
export async function compareDraft(
  draft: SharingDraft,
  load: (offset: number, snapshot?: string) => Promise<Choices>,
  current: () => boolean,
): Promise<DraftComparison> {
  const found = new Map<string, WorklogChoice>();
  const wanted = new Set(draft.choices.map(choiceKey));
  const seen = new Set<string>();
  let offset = 0;
  let snapshot: string | undefined;
  let complete = false;
  let omitted = false;
  let first: Choices | null = null;
  const check = (): void => { if (!current()) throw new Error("Settings or the selected draft changed. Refresh again."); };
  for (let count = 0; count < MAX_PAGES; count += 1) {
    check();
    const report = await load(offset, snapshot);
    check();
    if (report.start_date !== draft.start_date || report.end_date !== draft.end_date ||
        (draft.focus_session_id === null) !== (report.type === "worklog_choices_result") ||
        (report.type === "worklog_focus_choices_result" && report.focus_session_id !== draft.focus_session_id)) {
      throw new Error("The host returned different sharing dates or session. Refresh again.");
    }
    first ??= report;
    for (const row of report.choices) {
      const key = choiceKey(row);
      if (seen.has(key) || row.local_date < draft.start_date || row.local_date > draft.end_date) {
        throw new Error("The host returned duplicate or out-of-period work. Refresh again.");
      }
      seen.add(key);
      if (wanted.has(key)) found.set(key, row);
    }
    if (report.type === "worklog_focus_choices_result" || !report.page) {
      if (snapshot !== undefined) throw new Error("Work changed during the comparison. Refresh again.");
      complete = !report.truncated && (report.type !== "worklog_focus_choices_result" || report.selected_session !== null);
      break;
    }
    const page = report.page;
    if (!/^[a-f0-9]{64}$/.test(page.snapshot) || page.offset !== offset ||
        (snapshot !== undefined && snapshot !== page.snapshot) ||
        (page.next_offset !== null && (page.next_offset <= offset || !report.choices.length)) ||
        report.truncated !== (page.next_offset !== null || page.omitted)) {
      throw new Error("Work changed during the comparison. Refresh again.");
    }
    snapshot = page.snapshot;
    omitted ||= page.omitted;
    if (page.next_offset === null) { complete = !omitted; break; }
    if (found.size === wanted.size) break;
    offset = page.next_offset;
  }
  check();
  if (!first) throw new Error("No current work was returned.");
  return {
    heading: focusHeading(first), footer: gapFooter(first, draft.include_gaps),
    incomplete: !complete && found.size < wanted.size,
    rows: draft.choices.map((saved) => {
      const row = found.get(choiceKey(saved)) ?? null;
      return { saved, current: row, state: row
        ? row.text === saved.text && row.url === saved.url ? "unchanged" : "changed"
        : complete ? "missing" : "unreached" };
    }),
  };
}
