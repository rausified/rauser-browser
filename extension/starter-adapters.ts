import type { AdapterObject } from "./adapter-review.js";
import { canonicalCapture, compileDocument } from "./adapter-templates.js";
import { canonicalJson } from "./capture-site-handoff.js";
import { STARTER_FAMILIES, type StarterFamilyData, type StarterTypeData } from "./starter-families.generated.js";

/** The starter family an origin belongs to: a `STARTER_FAMILIES` entry's
 * `family` id. One family can offer several starters: an Atlassian Cloud
 * site offers Jira issues and Confluence pages, GitHub offers pull requests,
 * issues and repositories. Today's families are "atlassian", "sharepoint",
 * "github" and "miro"; the type stays `string` so a new family added to
 * protocol/starters/*.json needs no signature change here. */
export type StarterKind = string;

export interface StarterOffer {
  key: string;
  kind: StarterKind;
  artifact_type: string;
  label: string;
  description: string;
  origin: string;
  needs_scope: boolean;
  adapter: AdapterObject | null;
}

const GITHUB_ORIGIN = "https://github.com";

// Derived from STARTER_FAMILIES rather than hardcoded: the GitHub family's
// container type (today, "github.repo") and the sibling types that declare a
// `within` naming it (today, github.pr and github.issue). Falls back to
// nothing if no family declares a `within`, so `linkPlan` below simply finds
// no containers to link.
const { linkableToContainer: LINKABLE_TO_REPOSITORY, containerType: GITHUB_REPOSITORY_TYPE } = (() => {
  const linkableToContainer = new Set<string>();
  let containerType: string | null = null;
  for (const family of STARTER_FAMILIES) {
    for (const type of family.types) {
      const within = (type.adapter as Record<string, unknown>).within;
      if (!within || typeof within !== "object" || Array.isArray(within)) continue;
      const to = (within as Record<string, unknown>).type;
      if (typeof to === "string") {
        linkableToContainer.add(type.type);
        containerType = to;
      }
    }
  }
  return { linkableToContainer, containerType };
})();

function secureHostname(origin: string): string | null {
  try {
    const url = new URL(origin);
    return url.origin === origin && url.protocol === "https:"
      ? url.hostname.toLowerCase()
      : null;
  } catch {
    return null;
  }
}

function familyFor(origin: string): StarterFamilyData | null {
  for (const family of STARTER_FAMILIES) {
    if (family.origin.kind === "exact" && origin === family.origin.origin) return family;
  }
  const host = secureHostname(origin);
  if (!host) return null;
  let best: StarterFamilyData | null = null;
  let bestSuffixLength = -1;
  for (const family of STARTER_FAMILIES) {
    if (family.origin.kind !== "subdomain") continue;
    const suffix = family.origin.suffix;
    if (host.endsWith(suffix) && host !== suffix.slice(1) && suffix.length > bestSuffixLength) {
      best = family;
      bestSuffixLength = suffix.length;
    }
  }
  return best;
}

export function starterKind(origin: string): StarterKind | null {
  return familyFor(origin)?.family ?? null;
}

/** The current status of a starter type, or `null` when `type` names no
 * known starter. */
export function starterStatus(type: string): StarterTypeData["status"] | null {
  for (const family of STARTER_FAMILIES) {
    for (const starterType of family.types) {
      if (starterType.type === type) return starterType.status;
    }
  }
  return null;
}

export function parseGitHubOrganizations(value: string): string[] | null {
  const entries = value.split(",").map((entry) => entry.trim().toLowerCase()).filter(Boolean);
  const unique = [...new Set(entries)];
  if (!unique.length || unique.length > 32) return null;
  return unique.every((entry) =>
    entry.length <= 39 && /^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$/.test(entry))
    ? unique
    : null;
}

/** A typed comma-separated list of shared-origin tenants (workspaces or
 * accounts): 1 to `max` distinct canonical `name|lower` values. */
export function parseTenantList(value: string, max: number): string[] | null {
  const entries = value.split(",").map((entry) => entry.trim()).filter(Boolean);
  const unique: string[] = [];
  for (const entry of entries) {
    const lower = entry.toLowerCase();
    if (canonicalCapture("name|lower", lower) !== lower) return null;
    if (!unique.includes(lower)) unique.push(lower);
  }
  return unique.length && unique.length <= max ? unique : null;
}

// Routes that are never an organization, even though they sit where one
// would in the URL (github.com/<route>…). "orgs" with no further segment is
// covered separately below, since /orgs/<org>/… is where an org route lives.
const GITHUB_RESERVED_ROUTES = new Set([
  "settings", "notifications", "search", "orgs", "explore", "marketplace", "topics",
  "trending", "collections", "sponsors", "login", "logout", "signup", "new", "pulls",
  "issues", "codespaces", "dashboard", "features", "enterprise", "pricing", "about",
  "apps", "users", "account", "stars", "watching", "organizations", "enterprises", "copilot",
  "sessions", "join", "security", "readme", "team", "customer-stories", "solutions", "resources",
  "events", "site", "home", "mobile", "nonprofit", "education", "partners",
]);

/** The owner Brauser can read off a GitHub URL, to pre-fill (never apply
 * unseen) the Capture Site organization field. Returns `null` for anything
 * that is not confidently an organization: a non-GitHub origin, the root,
 * a reserved route, or a value `parseGitHubOrganizations` would reject. */
export function githubOwnerFromUrl(url: string): string | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.origin !== GITHUB_ORIGIN) return null;
  const segments = parsed.pathname.split("/").filter(Boolean).map((segment) => segment.toLowerCase());
  const first = segments[0];
  if (!first) return null;
  const owner = first === "orgs" ? (segments[1] ?? null) : first;
  if (!owner || GITHUB_RESERVED_ROUTES.has(owner)) return null;
  return parseGitHubOrganizations(owner) ? owner : null;
}

/** `value` with every `{"$scope": true}` marker, at any depth, replaced by
 * `organizations`; everything else deep-copied so the generated family data
 * is never mutated. */
function withScope(value: unknown, organizations: readonly string[]): unknown {
  if (Array.isArray(value)) return value.map((entry) => withScope(entry, organizations));
  if (value && typeof value === "object") {
    const entries = Object.entries(value as Record<string, unknown>);
    if (entries.length === 1 && entries[0]![0] === "$scope" && entries[0]![1] === true) {
      return [...organizations];
    }
    const copy: Record<string, unknown> = {};
    for (const [key, entry] of entries) copy[key] = withScope(entry, organizations);
    return copy;
  }
  return value;
}

/** The adapter a starter type offers at `origin`: its template with `origin`
 * inserted right after `type`, and every scope marker resolved against
 * `organizations` (or `null` when the type needs a scope and none is
 * named). */
function starterAdapter(
  type: StarterTypeData, origin: string, needsScope: boolean, organizations: readonly string[],
): AdapterObject | null {
  if (needsScope && !organizations.length) return null;
  const { type: adapterType, ...rest } = type.adapter as Record<string, unknown>;
  return { type: adapterType, origin, ...(withScope(rest, organizations) as Record<string, unknown>) };
}

/** The label for a type offered at `origin`: the first `label_variants`
 * entry whose `host_suffix` the origin contains, else the type's plain
 * `label`. */
function starterLabel(type: StarterTypeData, origin: string): string {
  return type.label_variants.find((variant) => origin.includes(variant.host_suffix))?.label ?? type.label;
}

/** The starter drafts for one site origin. A starter type whose family needs
 * a scope (today, GitHub's organizations) stays without an adapter until
 * `githubOrganizations` names at least one organization. */
export function starterOffers(origin: string, githubOrganizations: string[]): StarterOffer[] {
  const family = familyFor(origin);
  if (!family) return [];
  const needsScope = family.scope !== null;
  return family.types.map((type) => ({
    // A function replacer, not a replacement string: String.replace treats
    // `$'`, `$&`, `$1`, etc. in a replacement string as special patterns, and
    // the URL parser accepts `$` in a hostname, so a crafted origin could
    // otherwise splice part of itself or the match into the key.
    key: type.key.replace("{origin}", () => origin),
    kind: family.family,
    artifact_type: type.type,
    label: starterLabel(type, origin),
    description: type.description,
    origin,
    needs_scope: needsScope,
    adapter: starterAdapter(type, origin, needsScope, githubOrganizations),
  }));
}

/** One entry of an adapter's `match` list. */
export type AdapterRule = Record<string, unknown>;

// Config validation caps rules per adapter (adapter-templates.md §10.3).
const MAX_RULES = 16;

function ownersOf(adapter: AdapterObject): string[] {
  const scope = adapter.scope;
  if (!scope || typeof scope !== "object" || Array.isArray(scope)) return [];
  const owner = (scope as Record<string, unknown>).owner;
  return Array.isArray(owner) ? owner.filter((entry): entry is string => typeof entry === "string") : [];
}

/** The current starter `adapter` was saved from: same origin, type, and `id`
 * template string. A GitHub starter is built with the adapter's own
 * organizations, so its scope never decides the match. */
export function matchingStarter(adapter: AdapterObject): StarterOffer | null {
  if (typeof adapter.origin !== "string") return null;
  return starterOffers(adapter.origin, ownersOf(adapter)).find((offer) =>
    offer.adapter !== null && offer.adapter.type === adapter.type &&
    offer.adapter.id === adapter.id) ?? null;
}

const MODE_CAPTURE = /\{mode\??:\(([^)}]*)\)\}/g;

/** Earlier shipped versions of each starter's match rules, oldest first,
 * keyed by type, from each family type's `history` (protocol/starters/*.json).
 * Only a saved adapter holding every rule of one of these is offered an
 * update: an adapter matching no known version was written or narrowed by
 * its user, and the current starter is not a reason to widen it. */
const STARTER_HISTORY: Readonly<Record<string, readonly { match: readonly AdapterRule[] }[]>> =
  Object.fromEntries(
    STARTER_FAMILIES.flatMap((family) => family.types.map((type) => [type.type, type.history])),
  );

export interface StarterRuleUpdate {
  adapter: AdapterObject;
  /** The current starter's rules the adapter gains. */
  added: AdapterRule[];
  /** The earlier starter's rules it drops, which the current one replaced. */
  removed: AdapterRule[];
}

/**
 * `adapter` brought up to its starter's current rules, when it holds every
 * rule of a known earlier version of that starter (STARTER_HISTORY). The
 * update drops that version's rules the current starter no longer has and
 * appends the current rules it lacks. Everything else stays the user's: the
 * rules they added, scope, `within`, title and every other key, except that
 * `modes` gains the entries a new rule's `mode` capture needs and the adapter
 * lacks. `null` for an up-to-date adapter, one matching no earlier version
 * (hand-written or narrowed by the user), or when the result would pass the
 * 16-rule cap or fail to compile on its own.
 */
export function starterRuleUpdate(adapter: AdapterObject): StarterRuleUpdate | null {
  const current = matchingStarter(adapter)?.adapter;
  if (!current || !Array.isArray(adapter.match) || !Array.isArray(current.match)) return null;
  const savedRules = adapter.match as unknown[];
  const saved = new Set(savedRules.map(canonicalJson));
  const currentRules = current.match as AdapterRule[];
  const now = new Set(currentRules.map(canonicalJson));
  const added = currentRules.filter((rule) => !saved.has(canonicalJson(rule)));
  if (!added.length) return null;
  const versions = STARTER_HISTORY[String(adapter.type)] ?? [];
  // The newest earlier version the adapter fully holds.
  const version = [...versions].reverse().find((earlier) =>
    earlier.match.every((rule) => saved.has(canonicalJson(rule))));
  if (!version) return null;
  const retired = new Set(version.match.map(canonicalJson).filter((key) => !now.has(key)));
  const kept = savedRules.filter((rule) => !retired.has(canonicalJson(rule)));
  const removed = savedRules.filter((rule) => retired.has(canonicalJson(rule))) as AdapterRule[];
  if (kept.length + added.length > MAX_RULES) return null;
  return withStarterModes({ ...adapter, match: [...kept, ...added] }, current, added, removed);
}

/** `updated` with `modes` gaining the entries each `added` rule's `mode`
 * capture needs, taken from the current starter, when the adapter lacks
 * them; then the update, when the result compiles on its own. */
function withStarterModes(
  updated: AdapterObject, current: AdapterObject, added: AdapterRule[], removed: AdapterRule[],
): StarterRuleUpdate | null {
  const own = updated.modes && typeof updated.modes === "object" && !Array.isArray(updated.modes)
    ? updated.modes as Record<string, unknown>
    : {};
  const starterModes = (current.modes ?? {}) as Record<string, unknown>;
  const missingModes: Record<string, unknown> = {};
  for (const rule of added) {
    if (typeof rule.path !== "string") continue;
    for (const [, literals] of rule.path.matchAll(MODE_CAPTURE)) {
      for (const literal of literals!.split("|")) {
        if (!Object.hasOwn(own, literal) && Object.hasOwn(starterModes, literal)) {
          missingModes[literal] = starterModes[literal];
        }
      }
    }
  }
  if (Object.keys(missingModes).length) updated.modes = { ...own, ...missingModes };
  return compileDocument({ format: 1, artifacts: [updated] }).ok ? { adapter: updated, added, removed } : null;
}

/**
 * `adapter` with one rule of its current starter appended. The rule must be
 * in the current starter's `match` (compared by canonical JSON, and the
 * starter's own copy is the one added), absent from the adapter, keep the
 * adapter within 16 rules, and compile. `modes` gains only what the rule's
 * `mode` capture needs; nothing else about the adapter changes. Unlike
 * `starterRuleUpdate` this is not gated by STARTER_HISTORY: it is offered per
 * page and per rule, and confirmed in the editor, so a hand-narrowed adapter
 * gains only the one rule that covers the page in front of the user.
 */
export function starterRuleAddition(adapter: AdapterObject, rule: AdapterRule): StarterRuleUpdate | null {
  const current = matchingStarter(adapter)?.adapter;
  if (!current || !Array.isArray(adapter.match) || !Array.isArray(current.match)) return null;
  const key = canonicalJson(rule);
  const starterRule = (current.match as AdapterRule[]).find((entry) => canonicalJson(entry) === key);
  if (!starterRule) return null;
  const savedRules = adapter.match as unknown[];
  if (savedRules.some((entry) => canonicalJson(entry) === key)) return null;
  if (savedRules.length + 1 > MAX_RULES) return null;
  return withStarterModes({ ...adapter, match: [...savedRules, starterRule] }, current, [starterRule], []);
}

export function hasAdapter(adapters: AdapterObject[], offer: StarterOffer): boolean {
  return adapters.some((adapter) =>
    adapter.type === offer.artifact_type && adapter.origin === offer.origin);
}

type LinkPlan = "link" | "skip" | null;

/** For each adapter, whether adding `repo` links it ("link"), would link it
 * but cannot ("skip"), or leaves it alone (null). Candidates are `github.pr`
 * and `github.issue` rules on the repository's origin that declare no
 * `within`. A candidate is linked only when its linked copy compiles on its
 * own, so a hand-written rule whose ID and scope do not capture owner and
 * repo is skipped rather than invalidating the whole document. */
function linkPlan(artifacts: AdapterObject[], repo: AdapterObject): LinkPlan[] {
  if (repo.type !== GITHUB_REPOSITORY_TYPE || typeof repo.id !== "string") return artifacts.map(() => null);
  const within = { type: GITHUB_REPOSITORY_TYPE, id: repo.id };
  return artifacts.map((adapter) => {
    if (adapter.origin !== repo.origin || typeof adapter.type !== "string" ||
      !LINKABLE_TO_REPOSITORY.has(adapter.type) || adapter.within !== undefined) return null;
    return compileDocument({ format: 1, artifacts: [{ ...adapter, within }] }).ok ? "link" : "skip";
  });
}

/** How many existing pull request and issue rules adding `repo` would link,
 * and how many it must leave unlinked. */
export function containerLinks(artifacts: AdapterObject[], repo: AdapterObject): { linked: number; skipped: number } {
  const plan = linkPlan(artifacts, repo);
  return {
    linked: plan.filter((entry) => entry === "link").length,
    skipped: plan.filter((entry) => entry === "skip").length,
  };
}

/** `artifacts` with `within` naming `repo` added to each same-origin
 * `github.pr` and `github.issue` adapter that lacks one and can take it.
 * Nothing else about those adapters changes (their scope in particular);
 * adapters that already declare a container, or cannot be linked, are
 * returned as they are. */
export function withContainerLinks(artifacts: AdapterObject[], repo: AdapterObject): AdapterObject[] {
  const plan = linkPlan(artifacts, repo);
  return artifacts.map((adapter, at) => plan[at] === "link"
    ? { ...adapter, within: { type: GITHUB_REPOSITORY_TYPE, id: repo.id as string } }
    : adapter);
}

/** The union of `scope.owner` across `https://github.com`
 * adapters, in adapter order, to pre-fill the organization field. */
export function configuredGitHubOrganizations(artifacts: AdapterObject[]): string[] {
  const owners: string[] = [];
  for (const adapter of artifacts) {
    if (adapter.origin !== GITHUB_ORIGIN) continue;
    const scope = adapter.scope;
    if (!scope || typeof scope !== "object" || Array.isArray(scope)) continue;
    const owner = (scope as Record<string, unknown>).owner;
    if (!Array.isArray(owner)) continue;
    for (const entry of owner) if (typeof entry === "string") owners.push(entry.toLowerCase());
  }
  return [...new Set(owners)];
}

function containerType(adapter: AdapterObject): string | null {
  const within = adapter.within;
  if (!within || typeof within !== "object" || Array.isArray(within)) return null;
  const type = (within as Record<string, unknown>).type;
  return typeof type === "string" ? type : null;
}

/** `additions` with `within` dropped from each adapter whose container type
 * has no adapter on its origin among `existing` and `additions`. A starter
 * links to its container only when the user keeps that container, so
 * choosing pull requests without repositories records no repositories.
 * Adapters in `existing` are never changed: an established `within` is
 * frozen (DESIGN.md §6.2). */
export function linkedToPresentContainers<T extends AdapterObject>(additions: T[], existing: AdapterObject[]): T[] {
  const present = new Set([...existing, ...additions].map((adapter) => `${String(adapter.origin)} ${String(adapter.type)}`));
  return additions.map((adapter) => {
    const container = containerType(adapter);
    if (container === null || present.has(`${String(adapter.origin)} ${container}`)) return adapter;
    const unlinked: AdapterObject = { ...adapter };
    delete unlinked.within;
    return unlinked as T;
  });
}

/** A stable reorder so that, on each origin, every adapter declaring
 * `within.type` T comes before every adapter of type T. First match wins
 * (adapter-templates.md §3), so a container catch-all placed first would
 * swallow the pages of the artifacts inside it. Adapters keep their order,
 * except that a contained adapter found after its container moves up to just
 * ahead of it (recursively, for nested containers). Adapters on other
 * origins, and an already-ordered list, are untouched. Members of a cycle of
 * `within` declarations, which cannot all be satisfied, keep their order. */
export function orderContainedFirst<T extends AdapterObject>(artifacts: T[]): T[] {
  const containers = artifacts.map(containerType);
  // inside[i]: the adapters that sit directly within adapter i on its origin.
  const inside = artifacts.map((adapter) => artifacts.flatMap((other, at) =>
    containers[at] === adapter.type && other.origin === adapter.origin ? [at] : []));
  const reaches = (from: number, to: number): boolean => {
    const seen = new Set<number>();
    const stack = [from];
    while (stack.length) {
      const at = stack.pop()!;
      if (at === to) return true;
      if (seen.has(at)) continue;
      seen.add(at);
      stack.push(...inside[at]!);
    }
    return false;
  };
  const placed = artifacts.map(() => false);
  const ordered: T[] = [];
  const place = (index: number): void => {
    if (placed[index]) return;
    placed[index] = true;
    // A contained adapter that also (transitively) contains this one is in a
    // cycle; it is not pulled ahead, so the cycle keeps its order.
    for (const at of inside[index]!) if (!reaches(at, index)) place(at);
    ordered.push(artifacts[index]!);
  };
  artifacts.forEach((_adapter, index) => place(index));
  return ordered;
}
