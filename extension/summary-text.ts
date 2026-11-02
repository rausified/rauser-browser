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

/** Why a summary run saved nothing: the agent met a sign-in, no-access or
 * not-found wall instead of the artifact. `null` for an answer it read. */
export function notSavedText(access: string | undefined): string | null {
  switch (access) {
    case "sign_in":
      return "Not saved: the agent saw a sign-in page. Sign in, then summarize again.";
    case "denied":
      return "Not saved: you don't have access to this page.";
    case "not_found":
      return "Not saved: the page was not found.";
    default:
      return null;
  }
}
