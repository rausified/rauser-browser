// The known-site slice of Capture Site. These are the existing starter
// definitions, not structural drafts: unknown-site drafting stays host-owned.
import type { AdapterDocument, ConfigSnapshot } from "../protocol/ts/generated.js";
import { exampleUrl, previewDocument, validateAdapterDocument, type PreviewRow } from "./adapter-review.js";
import {
  linkedToPresentContainers, orderContainedFirst, parseGitHubOrganizations, starterKind, starterOffers, type StarterOffer,
} from "./starter-adapters.js";

export interface CaptureChoice {
  /** One shared, comma-separated organization list, used for every checked GitHub type. */
  organizations: string;
  repositories: boolean;
  pullRequests: boolean;
  issues: boolean;
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

// A repository example is its home page.
const GITHUB_EXAMPLE_ROUTES: Record<string, string> = { "github.pr": "/pull/123", "github.issue": "/issues/123" };

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
  if (!config.storage) return { ok: false, message: "Choose a home folder in settings first, then return here." };
  // Re-learning a configured site requires explicit aliases/re-keying. Never
  // replace a custom definition or silently add overlapping starter rules.
  if (siteConfigured(config, origin)) return { ok: false, message: "This site already has capture rules. Manage them in settings." };
  const offers: StarterOffer[] = [];
  if (starterKind(origin) === "github") {
    const checkedTypes = [
      ["github.pr", choice.pullRequests],
      ["github.issue", choice.issues],
      ["github.repo", choice.repositories],
    ] as const;
    if (!checkedTypes.some(([, checked]) => checked)) {
      return { ok: false, message: "Choose repositories, pull requests, issues, or some combination." };
    }
    const organizations = parseGitHubOrganizations(choice.organizations);
    if (!organizations) return { ok: false, message: "Enter 1–32 comma-separated GitHub organizations, or uncheck every GitHub page type." };
    const allOffers = starterOffers(origin, organizations);
    for (const [type, checked] of checkedTypes) {
      if (!checked) continue;
      offers.push(allOffers.find((offer) => offer.artifact_type === type)!);
    }
  } else {
    offers.push(...starterOffers(origin, []));
  }
  const artifacts = linkedToPresentContainers(offers.map((offer) => offer.adapter!), config.adapters.artifacts);
  const additions: AdapterDocument = { format: 1, artifacts: orderContainedFirst(artifacts) };
  const document: AdapterDocument = {
    ...config.adapters,
    artifacts: orderContainedFirst([...config.adapters.artifacts, ...artifacts]),
  };
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
      const route = GITHUB_EXAMPLE_ROUTES[offer.artifact_type] ?? "";
      example = `${origin}/${scope.owner[0]}/example-repository${route}`;
    }
    const result = previewDocument(additions, example, "", false);
    if (result.ok) examples.push(...result.rows);
  }
  return { ok: true, plan: {
    origin, document, offers, current: preview.rows[0]!, examples, probes,
    startsCapture: config.capture_enabled || config.adapters.artifacts.length === 0,
  } };
}
