// Capture Site. Starter origins use the existing starter definitions; any
// other HTTP(S) origin is drafted by the host's structural drafter from
// private URL shapes, and this module turns the review's selection into the
// checked tables and the document to save.
import type {
  AdapterDocument, ConfigSnapshot, DraftDropReason, DraftScopeState, DraftTenantView, RuleProvenance,
  ScopeChoiceView, ShapeCount, SiteDrafted,
} from "../protocol/ts/generated.js";
import { canonicalCapture, compileDocument, evaluate, urlSegments } from "./adapter-templates.js";
import type { PolicyLease } from "./model.js";
import {
  exampleUrl, previewDocument, validateAdapterDocument, type AdapterObject, type PreviewRow,
} from "./adapter-review.js";
import {
  MAX_SHAPE_COUNT, MAX_SITE_SHAPES, httpOrigin, isUrlShape, type SampledShape, type SitePatterns,
} from "./site-patterns.js";
import {
  linkedToPresentContainers, orderContainedFirst, parseGitHubOrganizations, parseTenantList, starterKind,
  starterOffers, type StarterOffer,
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

// --- Drafted sites -----------------------------------------------------------

const SAFETY_PAGES = "a sign-in, search, dashboard or sharing page";
const NO_ROUTE_WORD = "no fixed route word to draft from safely";
const FAILED_CHECKS = "it did not pass Brauser's safety checks";
/** Plain words for each drafter and checker reason. */
export const DROP_REASONS: Record<DraftDropReason, string> = {
  root: "the site's home page",
  probe: SAFETY_PAGES,
  search_or_paging: "search or paging pages",
  secret_query: "its ID would come from a sign-in or sharing token",
  blocked_route: "sign-in, sharing or account pages",
  param_name: "its ID would come from a search, paging or tracking setting",
  already_recognized: "already captured by this site's rules",
  no_route_word: NO_ROUTE_WORD,
  no_identifier: "no page ID in the address",
  limit: "more kinds of page than Brauser drafts at once",
  draft_forbidden_key: FAILED_CHECKS,
  draft_no_scope_source: FAILED_CHECKS,
  draft_scope_conflict: FAILED_CHECKS,
  draft_no_leading_literal: NO_ROUTE_WORD,
  draft_rest_second: "it would match too many pages",
  draft_id_needs_typed_capture: "its ID has no fixed shape",
  draft_search_capture: "its ID would come from search or paging",
  draft_matches_probe: `it would also capture ${SAFETY_PAGES}`,
  draft_overlap: "already captured by this site's rules",
  draft_blocked_route: "sign-in, sharing or account pages",
  draft_paging_capture: "its ID would come from search or paging",
  draft_secret_capture: "its ID would come from a sign-in or sharing token",
  draft_invalid_rule: FAILED_CHECKS,
  draft_scope_pending: FAILED_CHECKS,
  draft_scope_choice: FAILED_CHECKS,
};

/** Plain words for a host check refusal code. */
export function checkReason(reason: string | null): string {
  return (reason !== null && Object.hasOwn(DROP_REASONS, reason) ? DROP_REASONS[reason as DraftDropReason] : null) ??
    FAILED_CHECKS;
}

export type CaptureMode = "starter" | "draft" | "configured" | "unsupported";

/** Which Capture Site flow the page gets. A configured origin stays managed
 * in settings; a starter origin keeps the starter flow; any other
 * credential-free HTTP(S) origin is drafted. */
export function captureMode(config: ConfigSnapshot | null, url: string): CaptureMode {
  const origin = httpOrigin(url);
  if (!origin) return "unsupported";
  if (config && siteConfigured(config, origin)) return "configured";
  return captureOrigin(url) ? "starter" : "draft";
}

/** What `draft_site` and `check_site_draft` carry: the origin and private
 * URL shapes, never a raw URL, link href, or tab title. */
export interface DraftRequest {
  origin: string;
  visited: ShapeCount[];
  links: ShapeCount[];
}

const MAX_LINK_SHAPES = 50;

function shapeCount(shape: unknown, count: unknown, max: number): ShapeCount | null {
  return isUrlShape(shape) && Number.isInteger(count) && (count as number) >= 1 && (count as number) <= max
    ? { shape, count: count as number }
    : null;
}

/** The current page's shape (count 1, or its sampled count when the worker
 * already sampled it), then the worker's sample, then the click-only link
 * shapes. Invalid rows are dropped; only shapes and counts are kept. */
export function draftRequest(origin: string, patterns: SitePatterns, samples: readonly SampledShape[]): DraftRequest {
  const sampled = new Map<string, number>();
  for (const row of samples.slice(0, MAX_SITE_SHAPES)) {
    const entry = shapeCount(row?.shape, row?.count, MAX_SHAPE_COUNT);
    if (entry && !sampled.has(entry.shape)) sampled.set(entry.shape, entry.count);
  }
  const visited: ShapeCount[] = [];
  if (isUrlShape(patterns.current)) {
    visited.push({ shape: patterns.current, count: sampled.get(patterns.current) ?? 1 });
    sampled.delete(patterns.current);
  }
  for (const [shape, count] of sampled) visited.push({ shape, count });
  const links: ShapeCount[] = [];
  const seen = new Set<string>();
  for (const row of patterns.links.slice(0, MAX_LINK_SHAPES)) {
    const entry = shapeCount(row?.shape, row?.count, MAX_SHAPE_COUNT);
    if (entry && !seen.has(entry.shape)) {
      seen.add(entry.shape);
      links.push(entry);
    }
  }
  return { origin, visited, links };
}

/** The tenant at the drafted tenant position of the panel's own current
 * URL, as a `name|lower` value, or "" when there is none. The host supplies
 * only the position; the value never comes from the host. */
export function tenantPrefill(url: string, tenant: DraftTenantView | null): string {
  if (!tenant) return "";
  let segments: string[] | null;
  try {
    segments = urlSegments(new URL(url).pathname);
  } catch {
    return "";
  }
  const value = segments?.[tenant.segment];
  const canonical = value === undefined ? null : canonicalCapture("name|lower", value);
  // A sign-in, invite or sharing route at the tenant position is no tenant.
  return canonical === null || isBlockedRouteWord(canonical) ? "" : canonical;
}

/** The review's choices, one entry per drafted adapter (and per rule in
 * `checked`). `scope.values` is the typed comma-separated list. */
export interface DraftSelection {
  checked: boolean[][];
  types: (string | null)[];
  scope: ({ values: string } | "unscoped" | null)[];
  titleSuffix: string;
}

/** Exactly what `check_site_draft` receives: the Tier 0 tables after
 * unchecking and renaming, never the scope, unscoped, or title overlays. */
export interface DraftChecked {
  adapters: AdapterObject[];
  provenance: RuleProvenance[][];
  scope: (ScopeChoiceView | null)[];
}

export interface DraftKind {
  /** The index in `SiteDrafted.adapters`. */
  adapter: number;
  type: string;
  /** The drafted `match` indices that are kept, in order. */
  rules: number[];
  scope: DraftScopeState;
}

export interface DraftCounts {
  visited: number;
  visitedTotal: number;
  links: number;
  linksTotal: number;
}

export interface DraftPlan extends Pick<CapturePlan, "origin" | "document" | "current" | "examples" | "probes" | "startsCapture"> {
  checked: DraftChecked;
  kinds: DraftKind[];
  counts: DraftCounts;
}

const MAX_TENANT_VALUES = 64;
// The type is also the ID scheme, so it follows the stricter scheme grammar.
const DRAFT_TYPE = /^[a-z][a-z0-9.]{0,31}$/;
function hasControl(text: string): boolean {
  for (let i = 0; i < text.length; i += 1) {
    const code = text.charCodeAt(i);
    if (code < 0x20 || (code >= 0x7f && code <= 0x9f)) return true;
  }
  return false;
}

function isObject(value: unknown): value is AdapterObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function copiedScope(config: ConfigSnapshot, origin: string): Record<string, string[]> | null {
  const scoped = config.adapters.artifacts.find((adapter) => adapter.origin === origin && isObject(adapter.scope));
  return scoped ? structuredClone(scoped.scope as Record<string, string[]>) : null;
}

type Failure = { ok: false; message: string };
const fail = (message: string): Failure => ({ ok: false, message });

/** The drafted adapters with the review's selection applied: unchecked
 * rules and empty adapters omitted, renames applied to the type and the ID
 * scheme, then the scope, unscoped and title overlays, which only the saved
 * document carries. The real current URL is evaluated here, locally. */
export function planDraftedSite(
  config: ConfigSnapshot,
  draft: SiteDrafted,
  selection: DraftSelection,
  url: string,
  title: string,
): { ok: true; plan: DraftPlan } | Failure {
  const origin = httpOrigin(url);
  if (!origin) return fail("Capture Site works only on web pages.");
  if (!config.storage) return fail("Choose a home folder in settings first, then return here.");
  const count = draft.adapters.length;
  if (selection.checked.length !== count || selection.types.length !== count || selection.scope.length !== count ||
      selection.checked.some((rules, a) => rules.length !== draft.adapters[a]!.rules.length)) {
    return fail("The review is out of date. Capture this site again.");
  }
  const suffix = selection.titleSuffix;
  if (suffix.trim() !== "") {
    if (/[{}]/.test(suffix) || hasControl(suffix)) return fail("A title ending cannot contain braces or control characters.");
    if (!title.endsWith(suffix) || title.length === suffix.length) {
      return fail("The current tab title does not end with that title ending.");
    }
  }
  const taken = new Set([...draft.taken_types, ...config.adapters.artifacts.map((adapter) => String(adapter.type))]);
  const used = new Set<string>();
  const checked: DraftChecked = { adapters: [], provenance: [], scope: [] };
  const saved: AdapterObject[] = [];
  const kinds: DraftKind[] = [];
  const exampleUrls: string[] = [];
  for (const [a, view] of draft.adapters.entries()) {
    const table = structuredClone(view.adapter);
    const drafted = table.type;
    if (table.origin !== origin || typeof drafted !== "string" || typeof table.id !== "string" ||
        !Array.isArray(table.match)) {
      return fail("The drafted capture rules are not for this page's site. Capture this site again.");
    }
    const rules = view.rules.filter((_, r) => selection.checked[a]![r]);
    if (!rules.length) continue;
    const match = table.match as unknown[];
    if (rules.some((entry) => entry.rule >= match.length)) return fail("The drafted capture rules are invalid. Capture this site again.");
    table.match = rules.map((entry) => match[entry.rule]);
    // Rename: the type and the ID scheme together.
    const renamed = selection.types[a]?.trim() ?? drafted;
    if (renamed !== drafted) {
      if (!DRAFT_TYPE.test(renamed) || renamed === "web" || renamed.startsWith("web.")) {
        return fail(`"${renamed}" is not a valid kind name. Use a lowercase letter, then lowercase letters, digits, or dots.`);
      }
      if (taken.has(renamed)) return fail(`The kind name "${renamed}" is already used. Choose another.`);
      if (!(table.id as string).startsWith(`${drafted}:`)) return fail("The drafted capture rules are invalid. Capture this site again.");
      table.type = renamed;
      table.id = renamed + (table.id as string).slice(drafted.length);
    }
    if (used.has(renamed)) return fail(`Two kinds of page are both named "${renamed}". Choose different names.`);
    used.add(renamed);
    // Scope: chosen in the review, copied from the origin, or none.
    const choice = selection.scope[a] ?? null;
    let sent: ScopeChoiceView | null = null;
    const overlay: AdapterObject = {};
    let tenantValue: string | null = null;
    if (view.scope === "pending") {
      if (choice === "unscoped") {
        sent = "unscoped";
        overlay.unscoped = "acknowledged";
      } else {
        const values = choice === null ? null : parseTenantList(choice.values, MAX_TENANT_VALUES);
        if (!values || !view.tenant) {
          return fail(`Enter 1–${MAX_TENANT_VALUES} comma-separated workspaces or accounts for ${renamed}, or record every account.`);
        }
        const route = values.find(isBlockedRouteWord);
        if (route !== undefined) {
          return fail(`"${route}" is a sign-in, sharing or account page, not a workspace or account. Enter the workspaces or accounts for ${renamed}.`);
        }
        sent = { values };
        overlay.scope = { [view.tenant.capture]: values };
        tenantValue = values[0]!;
      }
    } else if (view.scope === "unscoped_only") {
      if (choice !== "unscoped") return fail(`This site does not name an account in its addresses. Confirm recording every account for ${renamed}.`);
      sent = "unscoped";
      overlay.unscoped = "acknowledged";
    } else if (choice !== null) {
      return fail(`The scope of ${renamed} is fixed for this site.`);
    } else if (view.scope === "copied") {
      const scope = copiedScope(config, origin);
      if (!scope) return fail("The site's existing scope changed. Capture this site again.");
      overlay.scope = scope;
      const capture = view.tenant?.capture;
      tenantValue = capture ? scope[capture]?.[0] ?? null : null;
    }
    if (suffix.trim() !== "") overlay.title = `{title}${suffix}`;
    checked.adapters.push(table);
    checked.provenance.push(rules.map(() => "drafted"));
    checked.scope.push(sent);
    saved.push({ ...structuredClone(table), ...overlay });
    kinds.push({ adapter: a, type: renamed, rules: rules.map((entry) => entry.rule), scope: view.scope });
    for (const entry of rules) {
      let example = entry.example;
      if (view.tenant && tenantValue !== null) {
        try {
          const parsed = new URL(example);
          const parts = parsed.pathname.split("/");
          if (parts.length > view.tenant.segment + 1) {
            parts[view.tenant.segment + 1] = tenantValue;
            parsed.pathname = parts.join("/");
            example = parsed.href;
          }
        } catch {
          // An unparsable example is simply not shown as a match.
        }
      }
      exampleUrls.push(example);
    }
  }
  if (!saved.length) return fail("Choose at least one kind of page to capture.");
  const additions: AdapterDocument = { format: 1, artifacts: saved };
  const document: AdapterDocument = { ...config.adapters, artifacts: [...config.adapters.artifacts, ...saved] };
  const validation = validateAdapterDocument(document);
  if (validation) return fail(validation);
  const preview = previewDocument(additions, url, title, true, origin);
  if (!preview.ok) return preview;
  const probes = preview.rows.filter((row) => row.probe);
  if (probes.some((row) => row.result.match)) return fail("The proposed capture rules match a safety-check page. Use settings to review them.");
  const examples: PreviewRow[] = [];
  for (const example of exampleUrls) {
    const result = previewDocument(additions, example, "", false);
    if (result.ok) examples.push(...result.rows.filter((row) => row.result.match));
  }
  const kept = new Set(kinds.flatMap((kind) => kind.rules.map((r) => `${kind.adapter}/${r}`)));
  const counts: DraftCounts = { visited: 0, visitedTotal: 0, links: 0, linksTotal: 0 };
  for (const row of draft.preview) {
    if (row.source === "probe") continue;
    const matched = row.adapter !== null && row.rule !== null && kept.has(`${row.adapter}/${row.rule}`);
    if (row.source === "visited") {
      counts.visitedTotal += 1;
      if (matched) counts.visited += 1;
    } else {
      counts.linksTotal += 1;
      if (matched) counts.links += 1;
    }
  }
  return { ok: true, plan: {
    origin, document, current: preview.rows[0]!, examples, probes,
    startsCapture: config.capture_enabled || config.adapters.artifacts.length === 0,
    checked, kinds, counts,
  } };
}


// --- Recurring pages on configured origins ----------------------------------

/** The host catalog's example value for each shape class
 * (protocol/capture-site/catalog.json `fill`; a test pins the two). */
const SHAPE_FILL: Readonly<Record<string, string>> = {
  num: "123456",
  key: "ABC-123",
  slug: "example-page",
  uuid: "123e4567-e89b-12d3-a456-426614174000",
  hex32: "0123456789abcdef0123456789abcdef",
  hex40: "0123456789abcdef0123456789abcdef01234567",
  hex64: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
  tailhex32: "example-page-0123456789abcdef0123456789abcdef",
  id: "example123",
};

/** The host catalog's `blocked_route_words`: never a route word to draft
 * from (a test pins the two). */
export const BLOCKED_ROUTE_WORDS: ReadonlySet<string> = new Set([
  "search", "query", "filter", "login", "home", "dashboard", "invite", "reset", "password", "verify",
  "confirm", "share", "magic", "token", "oauth", "auth", "signin", "signup", "logout", "unsubscribe",
  "invitation", "callback", "authorize", "sso", "saml", "join", "activate", "activation", "verification",
]);

/** Whether a route literal is a blocked route word, as the host's
 * `DraftPolicy::is_blocked_route_word` judges it: ASCII case folded, split
 * on `-`, `_` and `.`, and blocked when any part, or the parts joined, is a
 * listed word once a trailing number and then a plural `s` are folded. */
export function isBlockedRouteWord(literal: string): boolean {
  const parts = literal.replace(/[A-Z]/g, (c) => c.toLowerCase()).split(/[-_.]/).filter((part) => part !== "");
  const listed = (word: string): boolean => {
    const bare = word.replace(/[0-9]+$/, "");
    const singular = bare.endsWith("s") && bare.length >= 4 ? bare.slice(0, -1) : null;
    return BLOCKED_ROUTE_WORDS.has(bare) || (singular !== null && BLOCKED_ROUTE_WORDS.has(singular));
  };
  return parts.some(listed) || listed(parts.join(""));
}

/** How many sampled visits make a page shape recurring. */
export const RECURRING_VISITS = 3;

const fillClass = (part: string): string => SHAPE_FILL[part.slice(1, -1)] ?? part;
const isPlaceholder = (part: string): boolean => part.startsWith("{");

/** An example URL for a private shape, filled with the host catalog's class
 * examples as the host's `UrlShape::fill` does, or null for a non-shape. */
export function fillShape(origin: string, shape: string): string | null {
  if (!isUrlShape(shape)) return null;
  const [path, query] = shape.split("?");
  let url = origin + path!.split("/").map((part) => isPlaceholder(part) ? fillClass(part) : part).join("/");
  if (query !== undefined) {
    url += "?" + query.split("&").map((pair) => {
      const [key, value] = pair.split("=");
      return `${key === "{param}" ? "param" : key}=${fillClass(value!)}`;
    }).join("&");
  }
  return url;
}

/**
 * Whether the panel offers to draft rules for recurring pages on a
 * configured origin: the current page is not recognized, and the worker's
 * sample holds a shape that is not recognized either, was visited at least
 * three times, and has a path route word. It is computed here by filling each
 * shape and evaluating it with the lease's compiled document, and sends
 * nothing anywhere.
 */
export function recurringHint(lease: PolicyLease | null, samples: readonly SampledShape[], url: string): boolean {
  const origin = httpOrigin(url);
  if (!lease || !origin || !lease.adapters.artifacts.some((adapter) => adapter.origin === origin)) return false;
  const compiled = compileDocument(lease.adapters);
  if (!compiled.ok || evaluate(compiled.doc, url).match) return false;
  return samples.slice(0, MAX_SITE_SHAPES).some((row) => {
    const entry = shapeCount(row?.shape, row?.count, MAX_SHAPE_COUNT);
    if (!entry || entry.count < RECURRING_VISITS) return false;
    const words = entry.shape.split("?")[0]!.split("/").filter((part) => part !== "" && !isPlaceholder(part));
    if (!words.length || words.some(isBlockedRouteWord)) return false;
    const example = fillShape(origin, entry.shape);
    return example !== null && !evaluate(compiled.doc, example).match;
  });
}
