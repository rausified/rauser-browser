// What an origin's capture rules cover, in words, for the side panel's
// "Not an artifact" explanation (DESIGN.md §5.7). Pure: no DOM or Chrome
// access. It reads only the adapters the policy lease holds and the URL, never
// page content, and it never changes the host's verdict. The host never
// consumes it, so it has no Rust twin.

import type { AdapterObject } from "./adapter-review.js";
import { matchingStarter } from "./starter-adapters.js";

// The confirmation line names only what is actually checked, in this fixed
// order, so the privacy-boundary scope line is never wider than the save.
export const GITHUB_RECORD_LABELS: Readonly<Record<string, string>> = {
  "github.repo": "repositories",
  "github.pr": "pull requests",
  "github.issue": "issues",
};
export const GITHUB_RECORD_ORDER: readonly string[] = ["github.repo", "github.pr", "github.issue"];

/** "A", "A and B", or "A, B and C" — no Oxford comma, matching the panel's
 * existing list copy (e.g. the "Remember: X and Y" summary line). */
export function joinRecordedTypes(labels: readonly string[]): string {
  if (labels.length < 2) return labels.join("");
  return `${labels.slice(0, -1).join(", ")} and ${labels[labels.length - 1]}`;
}

export interface Coverage {
  /** The page's host, as `new URL(url).host`. */
  host: string;
  /** Plain-language nouns for what the origin captures, deduplicated, in display order. */
  types: string[];
  /** Scope values shown after "for", at most three. */
  accounts: string[];
  /** Further scope values beyond those three. */
  moreAccounts: number;
}

const MAX_ACCOUNTS = 3;

function pageOrigin(url: string): URL | null {
  try {
    return new URL(url);
  } catch {
    return null;
  }
}

/** The adapters on `url`'s origin, in config order, with their indices. */
export function originAdapters(artifacts: readonly AdapterObject[], url: string): { adapter: AdapterObject; index: number }[] {
  const parsed = pageOrigin(url);
  if (!parsed) return [];
  return artifacts.flatMap((adapter, index) => adapter.origin === parsed.origin ? [{ adapter, index }] : []);
}

/** The noun for one adapter: a GitHub record label, a starter's label, or the
 * adapter's own type. `null` for an adapter with no type. */
export function typeNoun(adapter: AdapterObject): string | null {
  if (typeof adapter.type !== "string") return null;
  const github = GITHUB_RECORD_LABELS[adapter.type];
  if (github !== undefined) return github;
  return matchingStarter(adapter)?.label ?? adapter.type;
}

function scopeTable(adapter: AdapterObject): Record<string, unknown> | null {
  const scope = adapter.scope;
  return scope && typeof scope === "object" && !Array.isArray(scope) ? scope as Record<string, unknown> : null;
}

/** What `url`'s origin captures, or `null` when no adapter there has a type. */
export function siteCoverage(artifacts: readonly AdapterObject[], url: string): Coverage | null {
  const parsed = pageOrigin(url);
  const adapters = originAdapters(artifacts, url).map((entry) => entry.adapter);
  if (!parsed || !adapters.length) return null;
  // GitHub's record types read as one group, in record order, where the
  // first of them sits in config order.
  const github = GITHUB_RECORD_ORDER.filter((type) => adapters.some((adapter) => adapter.type === type));
  const types: string[] = [];
  for (const adapter of adapters) {
    const nouns = typeof adapter.type === "string" && GITHUB_RECORD_LABELS[adapter.type] !== undefined
      ? github.map((type) => GITHUB_RECORD_LABELS[type]!)
      : [typeNoun(adapter)];
    for (const noun of nouns) if (noun !== null && !types.includes(noun)) types.push(noun);
  }
  if (!types.length) return null;
  // Naming accounts that some of the origin's rules ignore would mislead, so
  // they are listed only when every adapter there is scoped.
  const scopes = adapters.map(scopeTable);
  const values: string[] = [];
  if (scopes.every((scope) => scope !== null)) {
    for (const scope of scopes) {
      for (const entry of Object.values(scope!)) {
        if (!Array.isArray(entry)) continue;
        for (const value of entry) {
          if (typeof value !== "string") continue;
          const lower = value.toLowerCase();
          if (!values.includes(lower)) values.push(lower);
        }
      }
    }
  }
  return {
    host: parsed.host,
    types,
    accounts: values.slice(0, MAX_ACCOUNTS),
    moreAccounts: Math.max(0, values.length - MAX_ACCOUNTS),
  };
}

/** The exact coverage sentence for a page the rules did not match, or `null`
 * for an out-of-scope page on an origin with no accounts to name. */
export function coverageSentence(coverage: Coverage, reason: "no_rule" | "out_of_scope"): string | null {
  const accounts = coverage.moreAccounts
    ? [...coverage.accounts, `${coverage.moreAccounts} more`]
    : coverage.accounts;
  const forAccounts = accounts.length ? ` for ${joinRecordedTypes(accounts)}` : "";
  const captures = `${coverage.host} captures ${joinRecordedTypes(coverage.types)}${forAccounts}`;
  if (reason === "no_rule") return `${captures}; this page isn't one of them.`;
  return accounts.length ? `${captures}; this page belongs to another account.` : null;
}
