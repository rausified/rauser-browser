import { describe, worker } from "./settings.js";
import type { SampledShape } from "./site-patterns.js";

/** Local-only controls remain available even when the native host needs
 * repair or is disconnected. Clearing patterns changes no capture rules. */
export function sitePatternControls(origin: string): HTMLElement {
  const details = document.createElement("details");
  const summary = document.createElement("summary");
  summary.textContent = "Page patterns";
  const explanation = document.createElement("p");
  explanation.textContent = "While capture is on, Brauser keeps up to 20 visited URL patterns per site for 7 days. Identifiers and query values become placeholders; short lowercase route words remain. Titles are excluded. Patterns stay in this browser and are separate from your worklog. Pause or remove the site to clear them and stop sampling.";
  const status = document.createElement("p");
  status.setAttribute("role", "status");
  const rows = document.createElement("ul");
  rows.className = "plain-list";
  const show = document.createElement("button");
  show.type = "button";
  show.className = "secondary compact";
  show.textContent = "Show patterns";
  show.dataset.patternControl = "true";
  const clear = document.createElement("button");
  clear.type = "button";
  clear.className = "secondary compact";
  clear.textContent = "Clear patterns";
  clear.dataset.patternControl = "true";
  const act = async (clearing: boolean): Promise<void> => {
    show.disabled = true;
    clear.disabled = true;
    try {
      const patterns = await worker<SampledShape[]>({ kind: clearing ? "clear_site_patterns" : "get_site_patterns", origin });
      rows.replaceChildren();
      for (const pattern of patterns) {
        const row = document.createElement("li");
        row.textContent = `${pattern.shape} · ${pattern.count} navigation${pattern.count === 1 ? "" : "s"} · last seen ${new Date(pattern.last_seen).toISOString().slice(0, 10)} UTC`;
        rows.append(row);
      }
      status.textContent = clearing ? "Patterns cleared. New navigations can add patterns while capture is on."
        : patterns.length ? `${patterns.length} pattern${patterns.length === 1 ? "" : "s"}.` : "No recent patterns for this site.";
    } catch (error) {
      status.textContent = `Could not ${clearing ? "clear" : "read"} patterns: ${describe(error)}`;
    } finally {
      show.disabled = false;
      clear.disabled = false;
    }
  };
  show.addEventListener("click", () => void act(false));
  clear.addEventListener("click", () => void act(true));
  details.append(summary, explanation, show, clear, status, rows);
  return details;
}
