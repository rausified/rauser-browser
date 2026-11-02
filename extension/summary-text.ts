// One-line descriptions of a page's saved summary (DESIGN.md §5.7). The
// fields come from the agent's answer, so callers set these strings only
// through textContent.
import type { SummaryView } from "../protocol/ts/generated.js";

/** New summaries lead with their overview; older ones keep their labels. */
export function summaryLine(s: SummaryView): string {
  if (s.overview) return `${s.overview} · ${s.document_type}`;
  return `${s.document_type} · ${s.category} · ${s.importance} importance`;
}

/** A summarized Recent row's line: "Summarized · Type, Importance importance". */
export function recentSummaryText(s: SummaryView): string {
  if (s.overview) return `Summarized · ${summaryLine(s)}`;
  return `Summarized · ${s.document_type}, ${s.importance} importance`;
}
