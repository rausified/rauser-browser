// AI harness settings helpers. Pure: no chrome or DOM access, so the tests
// load this module directly. The host validates every value again (§7.3);
// these only let the settings page explain a problem before sending.
import type { AgentStatus, HarnessFound } from "../protocol/ts/generated.js";
import { APP_NAME } from "./brand.js";

/** Shown above Set up harness (spec §6). Summaries run with full access. */
export const SETUP_WARNING =
  "Summaries run an AI agent with full access to your files, on your normal login. A page could contain instructions aimed at the agent. Summarize pages you trust.";

/** The note under a detected harness: whether it can be set up. */
export function harnessOfferNote(found: HarnessFound): string {
  if (found.binary === null) return `Not found on the ${APP_NAME} service's PATH.`;
  return found.problem === null ? "Ready to set up." : `Cannot be set up: ${found.problem}.`;
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
