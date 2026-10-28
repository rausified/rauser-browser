// Summary harness settings helpers. Pure: no chrome or DOM access, so the
// tests load this module directly. The host and the service validate every
// value again; these only let the settings page explain a problem first.
import type { AgentStatus, HarnessFound, HarnessId } from "../protocol/ts/generated.js";
import { APP_NAME } from "./brand.js";

/** Shown above the harness list, and once before the first agent run of
 * either kind, a summary or a task (spec §6.1). Both run with full access. */
export const SETUP_WARNING =
  "Summaries and tasks run an AI agent with full access to your files, on your normal login. A page could contain instructions aimed at the agent. Use them on pages you trust.";

/** What to do when the native host cannot reach the service. The host's own
 * message, which says why, follows it (serviceSetupText). */
export const SERVICE_SETUP_INSTRUCTIONS =
  `Summaries need the ${APP_NAME} service, which runs the agent as a per-user LaunchAgent on macOS. For a development build, run scripts/register-dev-host.mjs again with your extension ID and the rebuilt binary, then reload this page.`;

export function harnessName(harness: HarnessId): string {
  return harness === "claude" ? "Claude Code" : "Codex";
}

/** One harness as the service found it on its own PATH. */
export function harnessFoundText(found: HarnessFound): string {
  const name = harnessName(found.harness);
  if (found.problem !== null) return `${name}: ${found.problem}`;
  if (found.binary === null) return `${name} not found on the service PATH`;
  return found.version ? `${name} ${found.version}` : name;
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

/** The names agent.env adds to every run. The service never sends values. */
export function agentEnvText(names: string[], problem: string | null): string {
  if (problem !== null) return `agent.env cannot be used: ${problem}`;
  if (!names.length) return `agent.env sets nothing, so runs get only the ${APP_NAME} service's environment.`;
  return `Runs also get ${names.join(", ")} from agent.env.`;
}

/** Setup instructions for a `service_unavailable` answer, then its message. */
export function serviceSetupText(message: string): string {
  return `${SERVICE_SETUP_INSTRUCTIONS} ${message}`;
}

/** One vocabulary entry per line: trimmed, blanks dropped, and later
 * case-insensitive duplicates removed. The host checks the limits. */
export function vocabularyFromText(text: string): string[] {
  const seen = new Set<string>();
  const entries: string[] = [];
  for (const line of text.split("\n")) {
    const entry = line.trim();
    const key = entry.toLowerCase();
    if (!entry || seen.has(key)) continue;
    seen.add(key);
    entries.push(entry);
  }
  return entries;
}
