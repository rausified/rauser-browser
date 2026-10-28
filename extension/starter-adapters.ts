import type { AdapterObject } from "./adapter-review.js";
import { compileDocument } from "./adapter-templates.js";

export type StarterKind = "jira" | "sharepoint" | "github";

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
const GITHUB_REPOSITORY = "github.repo";
// Frozen with the PR and issue IDs (adapter-templates.md §8.2, §8.3).
const GITHUB_REPOSITORY_WITHIN = { type: GITHUB_REPOSITORY, id: "github.repo:{host}/{owner}/{repo}" };

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

export function starterKind(origin: string): StarterKind | null {
  const host = secureHostname(origin);
  if (!host) return null;
  if (host.endsWith(".atlassian.net") && host !== "atlassian.net") return "jira";
  if (host.endsWith(".sharepoint.com") && host !== "sharepoint.com") return "sharepoint";
  if (origin === GITHUB_ORIGIN) return "github";
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

function jira(origin: string): AdapterObject {
  return {
    type: "jira.issue",
    origin,
    match: [
      { path: "/browse/{key:jira_key}" },
      { path: "/jira/{*}", query: { selectedIssue: "{key:jira_key}" } },
    ],
    id: "jira:{host}/{key}",
    title: "[{key:jira_key}] {title} - Jira",
    alias: { from: "tab_title", capture: "key" },
    refs: [{ type: "jira.issue", find: "jira_key" }],
  };
}

function sharepoint(origin: string): AdapterObject {
  return {
    type: "sharepoint.doc",
    origin,
    path_case: "insensitive",
    match: [
      { path: "/sites/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } },
      { path: "/:w:/r/sites/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } },
      { path: "/personal/{_}/_layouts/15/Doc.aspx", query: { sourcedoc: "{doc:uuid}" } },
    ],
    id: "sharepoint:{host}/{doc}",
  };
}

function githubPullRequest(organizations: string[]): AdapterObject {
  return {
    type: "github.pr",
    origin: GITHUB_ORIGIN,
    match: [{
      path: "/{owner:name|lower}/{repo:name|lower}/pull/{number:int}/{mode?:(files|commits|checks)}/{*}",
    }],
    scope: { owner: organizations },
    modes: { files: "review", commits: "view", checks: "view" },
    id: "github.pr:{host}/{owner}/{repo}/{number}",
    title: "{title} by {~} · Pull Request #{number:int} · {owner:name}/{repo:name} · GitHub",
    refs: [{ type: "jira.issue", find: "jira_key" }],
    within: { ...GITHUB_REPOSITORY_WITHIN },
  };
}

function githubIssue(organizations: string[]): AdapterObject {
  return {
    type: "github.issue",
    origin: GITHUB_ORIGIN,
    match: [{ path: "/{owner:name|lower}/{repo:name|lower}/issues/{number:int}" }],
    scope: { owner: organizations },
    id: "github.issue:{host}/{owner}/{repo}/{number}",
    title: "{title} · Issue #{number:int} · {owner:name}/{repo:name} · GitHub",
    refs: [{ type: "jira.issue", find: "jira_key" }],
    within: { ...GITHUB_REPOSITORY_WITHIN },
  };
}

/** Every page inside a repository. It has no title templates: the host
 * derives a container's title from its ID. It must follow the PR and issue
 * adapters on the origin (first match wins), which `orderContainedFirst`
 * guarantees. */
function githubRepository(organizations: string[]): AdapterObject {
  return {
    type: GITHUB_REPOSITORY,
    origin: GITHUB_ORIGIN,
    match: [{ path: "/{owner:name|lower}/{repo:name|lower}/{*}" }],
    scope: { owner: organizations },
    id: GITHUB_REPOSITORY_WITHIN.id,
  };
}

/** The starter drafts for one site origin. GitHub starters stay without an
 * adapter until `githubOrganizations` names at least one organization. */
export function starterOffers(origin: string, githubOrganizations: string[]): StarterOffer[] {
  const kind = starterKind(origin);
  if (kind === "jira") {
    return [{
      key: `jira:${origin}`,
      kind,
      artifact_type: "jira.issue",
      label: "Jira issues",
      description: "Issue pages and board modals become one artifact per Jira key.",
      origin,
      needs_scope: false,
      adapter: jira(origin),
    }];
  }
  if (kind === "sharepoint") {
    return [{
      key: `sharepoint:${origin}`,
      kind,
      artifact_type: "sharepoint.doc",
      label: origin.includes("-my.sharepoint.com")
        ? "OneDrive for work documents"
        : "SharePoint documents",
      description: "Office document links resolve to the stable sourcedoc GUID, across common sharing routes.",
      origin,
      needs_scope: false,
      adapter: sharepoint(origin),
    }];
  }
  if (kind === "github") {
    const scope = githubOrganizations.length ? githubOrganizations : null;
    return [
      {
        key: "github.pr",
        kind,
        artifact_type: "github.pr",
        label: "GitHub pull requests",
        description: "Pull request routes share one artifact; files use review mode, while commits and checks use view mode.",
        origin,
        needs_scope: true,
        adapter: scope ? githubPullRequest(scope) : null,
      },
      {
        key: "github.issue",
        kind,
        artifact_type: "github.issue",
        label: "GitHub issues",
        description: "Issue pages are captured only for the organizations you name.",
        origin,
        needs_scope: true,
        adapter: scope ? githubIssue(scope) : null,
      },
      {
        key: GITHUB_REPOSITORY,
        kind,
        artifact_type: GITHUB_REPOSITORY,
        label: "GitHub repositories",
        description: "Every page inside a repository counts toward it; pull requests and issues count toward both.",
        origin,
        needs_scope: true,
        adapter: scope ? githubRepository(scope) : null,
      },
    ];
  }
  return [];
}

export function hasAdapter(adapters: AdapterObject[], offer: StarterOffer): boolean {
  return adapters.some((adapter) =>
    adapter.type === offer.artifact_type && adapter.origin === offer.origin);
}

const LINKABLE_TO_REPOSITORY = new Set(["github.pr", "github.issue"]);

type LinkPlan = "link" | "skip" | null;

/** For each adapter, whether adding `repo` links it ("link"), would link it
 * but cannot ("skip"), or leaves it alone (null). Candidates are `github.pr`
 * and `github.issue` rules on the repository's origin that declare no
 * `within`. A candidate is linked only when its linked copy compiles on its
 * own, so a hand-written rule whose ID and scope do not capture owner and
 * repo is skipped rather than invalidating the whole document. */
function linkPlan(artifacts: AdapterObject[], repo: AdapterObject): LinkPlan[] {
  if (repo.type !== GITHUB_REPOSITORY || typeof repo.id !== "string") return artifacts.map(() => null);
  const within = { type: GITHUB_REPOSITORY, id: repo.id };
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
    ? { ...adapter, within: { type: GITHUB_REPOSITORY, id: repo.id as string } }
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
