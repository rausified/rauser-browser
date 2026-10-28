// One-line descriptions of a page's saved summary (DESIGN.md §5.7). The
// fields come from the agent's answer, so callers set these strings only
// through textContent.
import type { SummaryView } from "../protocol/ts/generated.js";

/** The page's summary line: "Type · Category · Importance importance". */
export function summaryLine(s: SummaryView): string {
  return `${s.document_type} · ${s.category} · ${s.importance} importance`;
}

/** A summarized Recent row's line: "Summarized · Type, Importance importance". */
export function recentSummaryText(s: SummaryView): string {
  return `Summarized · ${s.document_type}, ${s.importance} importance`;
}
