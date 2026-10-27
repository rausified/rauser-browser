// The known-site slice of Capture Site. These are the existing starter
// definitions, not structural drafts: unknown-site drafting stays host-owned.
import type { AdapterDocument, ConfigSnapshot } from "../protocol/ts/generated.js";
import { exampleUrl, previewDocument, validateAdapterDocument, type PreviewRow } from "./adapter-review.js";
import { parseGitHubOrganizations, starterKind, starterOffers, type StarterOffer } from "./starter-adapters.js";

export interface CaptureChoice {
  pullRequests: string | null;
  issues: string | null;
}

export interface CapturePlan {
  origin: string;
  document: AdapterDocument;
  offers: StarterOffer[];
  current: PreviewRow;
  examples: PreviewRow[];
  probes: PreviewRow[];
  startsCapture: boolean;
}

export function captureOrigin(url: string): string | null {
  try {
    const parsed = new URL(url);
    return !parsed.username && !parsed.password && starterKind(parsed.origin) ? parsed.origin : null;
  } catch {
    return null;
  }
}

export function siteConfigured(config: ConfigSnapshot, origin: string): boolean {
  return config.adapters.artifacts.some((adapter) => adapter.origin === origin);
}

export function planCaptureSite(
  config: ConfigSnapshot,
  url: string,
  choice: CaptureChoice,
): { ok: true; plan: CapturePlan } | { ok: false; message: string } {
  const origin = captureOrigin(url);
  if (!origin) return { ok: false, message: "Automatic setup for this site is still in development. Use settings to configure it." };
  if (!config.storage) return { ok: false, message: "Choose a notes folder in settings first, then return here." };
  // Re-learning a configured site requires explicit aliases/re-keying. Never
  // replace a custom definition or silently add overlapping starter rules.
  if (siteConfigured(config, origin)) return { ok: false, message: "This site already has capture rules. Manage them in settings." };
  const offers: StarterOffer[] = [];
  if (starterKind(origin) === "github") {
    for (const [type, value, label] of [
      ["github.pr", choice.pullRequests, "pull requests"],
      ["github.issue", choice.issues, "issues"],
    ] as const) {
      if (value === null) continue;
      const organizations = parseGitHubOrganizations(value);
      if (!organizations) return { ok: false, message: `Enter 1–32 comma-separated GitHub organizations for ${label}, or uncheck that kind of page.` };
      offers.push(starterOffers(origin, organizations).find((offer) => offer.artifact_type === type)!);
    }
  } else {
    offers.push(...starterOffers(origin, []));
  }
  if (!offers.length) return { ok: false, message: "Choose pull requests, issues, or both." };
  const artifacts = offers.map((offer) => offer.adapter!);
  const additions: AdapterDocument = { format: 1, artifacts };
  const document: AdapterDocument = { ...config.adapters, artifacts: [...config.adapters.artifacts, ...artifacts] };
  const validation = validateAdapterDocument(document);
  if (validation) return { ok: false, message: validation };
  const preview = previewDocument(additions, url, "", true, origin);
  if (!preview.ok) return preview;
  const probes = preview.rows.filter((row) => row.probe);
  if (probes.some((row) => row.result.match)) return { ok: false, message: "The proposed capture rules match a safety-check page. Use settings to review them." };
  const examples: PreviewRow[] = [];
  for (const offer of offers) {
    let example = exampleUrl(offer.adapter!);
    if (offer.kind === "github") {
      const scope = offer.adapter!.scope as { owner: string[] };
      example = `${origin}/${scope.owner[0]}/example-repository/${offer.artifact_type === "github.pr" ? "pull" : "issues"}/123`;
    }
    const result = previewDocument(additions, example, "", false);
    if (result.ok) examples.push(...result.rows);
  }
  return { ok: true, plan: {
    origin, document, offers, current: preview.rows[0]!, examples, probes,
    startsCapture: config.capture_enabled || config.adapters.artifacts.length === 0,
  } };
}
