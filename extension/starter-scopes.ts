// The scope-kind registry: for each `StarterScopeData.kind`, the copy,
// parsing and URL prefill Capture Site and the settings gallery need to ask
// for and validate a starter's scope. A new family using an existing kind
// needs no change here; a new kind is one addition to `scopeInput`.
import { githubOwnerFromUrl, parseGitHubOrganizations, parseTenantList } from "./starter-adapters.js";
import type { StarterFamilyData } from "./starter-families.generated.js";

export interface ScopeInput {
  label: string;
  placeholder: string;
  maxLength: number;
  galleryLabel: string;
  galleryPlaceholder: string;
  guidance: string;
  invalid: string;
  parse(value: string): string[] | null;
  /** The panel's prefill reading of a URL, or `null` for a kind with none
   * (never applied unseen: the caller still checks the result matches the
   * current page before using it). */
  fromUrl: ((url: string) => string | null) | null;
  records(nouns: string, values: string): string;
}

/** `family`'s scope input, or `null` when it has no scope (one type, no
 * scope kind) or is `unscoped` (no value to ask for). */
export function scopeInput(family: StarterFamilyData): ScopeInput | null {
  const scope = family.scope;
  if (!scope) return null;
  if (scope.kind === "github_org") {
    return {
      label: "Work organizations",
      placeholder: "acme-corp, partner",
      maxLength: 1312,
      galleryLabel: "Work organizations (comma-separated)",
      galleryPlaceholder: "acme-corp, partner-org",
      guidance: "Required: GitHub is shared across accounts, so the starter never captures every organization.",
      invalid: "Enter 1–32 comma-separated GitHub organizations, or uncheck every GitHub page type.",
      parse: parseGitHubOrganizations,
      fromUrl: githubOwnerFromUrl,
      records: (nouns, values) => `Records ${nouns} owned by: ${values}.`,
    };
  }
  if (scope.kind === "tenant_name") {
    const label = scope.label;
    const display = family.display_name;
    return {
      label,
      placeholder: scope.placeholder,
      maxLength: scope.max * 41,
      galleryLabel: `${label} (comma-separated)`,
      galleryPlaceholder: scope.placeholder,
      guidance: `Required: ${display} is shared across accounts, so the starter records only the ${label.toLowerCase()} you name.`,
      invalid: `Enter 1–${scope.max} comma-separated ${label.toLowerCase()}, or uncheck every ${display} page type.`,
      parse: (value) => parseTenantList(value, scope.max),
      fromUrl: null,
      records: (nouns, values) => `Records ${nouns} only for: ${values}.`,
    };
  }
  return null; // "unscoped"
}

/** `family`'s every-account notice, or `null` for a scoped (or scope-less)
 * family. */
export function unscopedNotice(family: StarterFamilyData): string | null {
  return family.scope?.kind === "unscoped" ? family.scope.notice : null;
}
