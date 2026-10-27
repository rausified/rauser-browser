// AI harness settings helpers. Pure: no chrome or DOM access, so the tests
// load this module directly. The host validates every value again (§7.3);
// these only let the settings page explain a problem before sending.
import type { AgentStatus, HarnessOffer } from "../protocol/ts/generated.js";

/** DESIGN §14: Codex runs with the user's own HOME and CODEX_HOME. The host's
 * setup confirmation says the same (host/src/consent.rs CODEX_DISCLOSURE). */
export const CODEX_DISCLOSURE =
  "Codex runs with your normal Codex home and login, so your own Codex instructions (AGENTS.md) and skills can shape its answers. Claude Code is the recommended harness.";

/** The note under a detected harness: whether it can be set up and, for
 * Codex, what sharing the user's Codex home means. */
export function harnessOfferNote(offer: HarnessOffer): string {
  const ready = offer.offer_id !== null && offer.refusal === null;
  const state = ready ? "Ready to set up." : `Cannot be set up: ${offer.refusal ?? "detect again"}.`;
  return offer.adapter === "codex" ? `${state} ${CODEX_DISCLOSURE}` : state;
}

/** What Detect harnesses reports for a ready offer. Set up harness needs a
 * notes folder, so without one this says to choose it first. */
export function harnessFoundText(name: string, version: string | null, hasNotesFolder: boolean): string {
  const found = `Found ${name}${version ? ` ${version}` : ""}.`;
  return hasNotesFolder
    ? `${found} Review it below, then choose Set up harness.`
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
