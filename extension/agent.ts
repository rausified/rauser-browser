// AI harness settings helpers. Pure: no chrome or DOM access, so the tests
// load this module directly. The host validates every value again (§7.3);
// these only let the settings page explain a problem before sending.
import type { AgentStatus, HarnessOffer } from "../protocol/ts/generated.js";

/** Shown above Set up harness (spec §6). Summaries run with full access. */
export const SETUP_WARNING =
  "Summaries run an AI agent with full access to your files, on your normal login. A page could contain instructions aimed at the agent. Summarize pages you trust.";

/** The note under a detected harness: whether it can be set up. */
export function harnessOfferNote(offer: HarnessOffer): string {
  return offer.refusal === null ? "Ready to set up." : `Cannot be set up: ${offer.refusal}.`;
}

/** What Detect harnesses reports for a ready offer. Set up harness needs a
 * notes folder, so without one this says to choose it first. */
export function harnessFoundText(name: string, version: string | null, hasNotesFolder: boolean): string {
  const found = `Found ${name}${version ? ` ${version}` : ""}.`;
  return hasNotesFolder
    ? `${found} Choose Set up harness to run a test prompt.`
    : `${found} Choose a notes folder first, then set up the harness.`;
}

/** The settings page's one-line AI state. Capture setup is described
 * separately (settings.ts setupProblem); this never blocks capture. */
export function agentStateText(status: AgentStatus | null): string {
  if (!status) return "AI harness status is unavailable.";
  const version = status.harness_version ? ` (version ${status.harness_version})` : "";
  switch (status.state) {
    case "ready":
      return `AI harness ready${version}.`;
    case "not_set_up":
      return "No AI harness is set up. AI commands are off.";
    case "harness_problem":
      return `AI commands are off${version}: ${status.message ?? "the harness needs setup again"}`;
  }
}
