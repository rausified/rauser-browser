import type { SiteConfig } from "../protocol/ts/generated.js";
import type { AdapterObject } from "./adapter-review.js";

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
  };
}

export function starterOffers(site: SiteConfig, githubOrganizations: string[] | null): StarterOffer[] {
  // Starters cover several top-level routes. A narrower site policy is a
  // deliberate custom boundary and must not be silently widened or presented
  // as if the whole starter would work inside it.
  if (site.path_prefix !== "/") return [];
  const kind = starterKind(site.origin);
  if (kind === "jira") {
    return [{
      key: `jira:${site.origin}`,
      kind,
      artifact_type: "jira.issue",
      label: "Jira issues",
      description: "Issue pages and board modals become one artifact per Jira key.",
      origin: site.origin,
      needs_scope: false,
      adapter: jira(site.origin),
    }];
  }
  if (kind === "sharepoint") {
    return [{
      key: `sharepoint:${site.origin}`,
      kind,
      artifact_type: "sharepoint.doc",
      label: site.origin.includes("-my.sharepoint.com")
        ? "OneDrive for work documents"
        : "SharePoint documents",
      description: "Office document links resolve to the stable sourcedoc GUID, across common sharing routes.",
      origin: site.origin,
      needs_scope: false,
      adapter: sharepoint(site.origin),
    }];
  }
  if (kind === "github") {
    const scope = githubOrganizations;
    return [
      {
        key: "github.pr",
        kind,
        artifact_type: "github.pr",
        label: "GitHub pull requests",
        description: "Pull request routes share one artifact; files use review mode, while commits and checks use view mode.",
        origin: site.origin,
        needs_scope: true,
        adapter: scope ? githubPullRequest(scope) : null,
      },
      {
        key: "github.issue",
        kind,
        artifact_type: "github.issue",
        label: "GitHub issues",
        description: "Issue pages are captured only for the organizations you name.",
        origin: site.origin,
        needs_scope: true,
        adapter: scope ? githubIssue(scope) : null,
      },
    ];
  }
  return [];
}

export function hasAdapter(adapters: AdapterObject[], offer: StarterOffer): boolean {
  return adapters.some((adapter) =>
    adapter.type === offer.artifact_type && adapter.origin === offer.origin);
}
