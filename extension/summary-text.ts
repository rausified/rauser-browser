// One-line descriptions of a page's saved summary (DESIGN.md §5.7). The
// fields come from the agent's answer, so callers set these strings only
// through textContent.
import type { AnswerReadScope, SummaryView } from "../protocol/ts/generated.js";

/** Agent-reported reading coverage; legacy answers make no completeness claim. */
export function readScopeText(scope: AnswerReadScope | undefined): string | null {
  switch (scope) {
    case "whole":
      return "Agent read: whole artifact";
    case "partial":
      return "Agent read: part of the artifact";
    case "page_only":
      return "Agent read: browser page only";
    default:
      return null;
  }
}

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
