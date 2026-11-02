// What an origin's capture rules cover, in words, for the side panel's
// "Not an artifact" explanation (DESIGN.md §5.7). Pure: no DOM or Chrome
// access. It reads only the adapters the policy lease holds and the URL, never
// page content, and it never changes the host's verdict. The host never
// consumes it, so it has no Rust twin.

import type { AdapterObject } from "./adapter-review.js";
import { canonicalCapture, compileDocument, evaluate } from "./adapter-templates.js";
import { canonicalJson } from "./capture-site-handoff.js";
import { matchingStarter, starterRuleAddition, type AdapterRule } from "./starter-adapters.js";

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

/** Why a page the rules missed is close to an artifact: the adapter's
 * current starter has a rule that would match it (offered for review), or
 * the page carries a rule's fixed-shape query identifier on a route no rule
 * covers (explained only). `adapter` indexes the lease's adapters. */
export type NearMiss =
  | { kind: "starter_rule"; adapter: number; label: string; rule: AdapterRule }
  | { kind: "identifier"; adapter: number; label: string; key: string };

const QUERY_CAPTURE = /^\{[A-Za-z_][A-Za-z0-9_]*:([^{}]+)\}$/;
const FIXED_HEX = /^hex\((\d+)(?:,(\d+))?\)$/;

/** The ctype of a query value that is exactly one required capture of a
 * fixed shape, the anchor definition of adapter-templates.md §4.1: `uuid`, or
 * `hex` whose bounds fix its length at 32 or more. */
function fixedShapeCapture(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const ctype = QUERY_CAPTURE.exec(value)?.[1];
  if (ctype === undefined) return null;
  const base = ctype.split("|")[0]!;
  if (base === "uuid") return ctype;
  const hex = FIXED_HEX.exec(base);
  if (!hex) return null;
  const min = Number(hex[1]);
  const max = hex[2] === undefined ? min : Number(hex[2]);
  return min === max && min >= 32 ? ctype : null;
}

function matchesAlone(adapter: AdapterObject, url: string): boolean {
  const compiled = compileDocument({ format: 1, artifacts: [adapter] });
  return compiled.ok && evaluate(compiled.doc, url).match;
}

/** The first near-miss for `url`, which the caller has already found matches
 * no rule of the full document. No site list is hard-coded: starter rules
 * come from the starter catalog, and identifiers from the adapters' own
 * query captures. Path-only identity is never guessed. */
export function nearMiss(artifacts: readonly AdapterObject[], url: string): NearMiss | null {
  const adapters = originAdapters(artifacts, url);
  // 1. A rule of the adapter's current starter that it lacks, which alone
  // would match the page under the adapter's own scope. The full document
  // matched no rule, so it would be the page's first match once saved.
  for (const { adapter, index } of adapters) {
    const starter = matchingStarter(adapter);
    const current = starter?.adapter;
    if (!starter || !current || !Array.isArray(current.match) || !Array.isArray(adapter.match)) continue;
    const saved = new Set((adapter.match as unknown[]).map(canonicalJson));
    for (const rule of current.match as AdapterRule[]) {
      if (saved.has(canonicalJson(rule))) continue;
      const addition = starterRuleAddition(adapter, rule);
      if (addition && matchesAlone(addition.adapter, url)) {
        return { kind: "starter_rule", adapter: index, label: starter.label, rule };
      }
    }
  }
  // 2. A rule's fixed-shape query identifier, present exactly once and valid.
  let query: URLSearchParams;
  try {
    query = new URL(url).searchParams;
  } catch {
    return null;
  }
  for (const { adapter, index } of adapters) {
    if (!Array.isArray(adapter.match)) continue;
    for (const rule of adapter.match as unknown[]) {
      const table = rule && typeof rule === "object" && !Array.isArray(rule)
        ? (rule as Record<string, unknown>).query
        : undefined;
      if (!table || typeof table !== "object" || Array.isArray(table)) continue;
      for (const [key, value] of Object.entries(table as Record<string, unknown>)) {
        const ctype = fixedShapeCapture(value);
        const values = query.getAll(key);
        if (ctype !== null && values.length === 1 && canonicalCapture(ctype, values[0]!) !== null) {
          return { kind: "identifier", adapter: index, label: typeNoun(adapter) ?? String(adapter.type), key };
        }
      }
    }
  }
  return null;
}
