import type { AdapterDocument, AdapterWarning } from "../protocol/ts/generated.js";
import {
  compileDocument,
  evaluate,
  type EvaluateResult,
  type ValidationError,
} from "./adapter-templates.js";

export type AdapterObject = Record<string, unknown>;

export type ParsedAdapter =
  | { ok: true; adapter: AdapterObject }
  | { ok: false; message: string };

export type PreviewRow = {
  url: string;
  probe: boolean;
  result: EvaluateResult;
};

const ERROR_MESSAGES: Readonly<Record<string, string>> = {
  wrong_type: "A value has the wrong JSON type.",
  missing_format: "The adapter document has no format number.",
  invalid_format: "The adapter format must be a positive whole number.",
  unsupported_format: "This extension does not support that adapter format.",
  unknown_key: "The adapter contains an unknown setting.",
  missing_key: "The adapter is missing a required setting.",
  too_many_adapters: "The adapter limit is 64.",
  bad_adapter_type: "Type must start with a lowercase letter and use only lowercase letters, digits, dots, or underscores.",
  reserved_adapter_type: "The web.page type is reserved by Brauser.",
  bad_origin: "Origin must be one exact HTTP(S) origin, without a path.",
  bad_path_case: "path_case must be sensitive or insensitive.",
  no_rules: "Add at least one match rule.",
  too_many_rules: "An adapter can contain at most 16 match rules.",
  path_syntax: "A match path has invalid adapter-template syntax.",
  bad_capture_name: "A capture name is invalid.",
  unknown_capture_type: "A capture uses an unknown type.",
  id_syntax: "The ID template has invalid syntax.",
  id_reserved_scheme: "The web ID scheme is reserved by Brauser.",
  id_unknown_capture: "The ID uses a capture that is not present in every rule.",
  extra_capture: "A rule captures a value that is not used by the ID, mode, or scope.",
  no_literal_segment: "Each rule needs a fixed route segment, a fixed-shape identifier, or a scoped tenant.",
  bad_unscoped: "unscoped must be acknowledged, and only when the origin requires it.",
  shared_origin_unscoped: "This shared origin needs a tenant scope or an explicit unscoped acknowledgment.",
  scope_capture_not_bound: "Every rule must bind each capture used for scope.",
  title_syntax: "A title template has invalid syntax.",
  within_bad_type: "The within type must be a valid adapter type other than web.page.",
  within_self: "An adapter cannot sit within its own type or its own ID.",
  within_id_syntax: "The within ID template has invalid syntax.",
  within_id_reserved_scheme: "The web ID scheme is reserved by Brauser, including in within.",
  within_unknown_capture: "The within ID may use only captures from the adapter's ID or scope, and never mode.",
};

function isObject(value: unknown): value is AdapterObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

export function adapterLabel(adapter: AdapterObject, index: number): string {
  return typeof adapter.type === "string" && adapter.type.length > 0
    ? adapter.type
    : `Adapter ${index + 1}`;
}

export function adapterOrigin(adapter: AdapterObject): string | null {
  return typeof adapter.origin === "string" ? adapter.origin : null;
}

export function formatValidationError(error: ValidationError): string {
  const location = [
    error.adapter === null ? null : `adapter ${error.adapter + 1}`,
    error.rule === null ? null : `rule ${error.rule + 1}`,
    error.entry === null ? null : `entry ${error.entry + 1}`,
  ].filter((part): part is string => part !== null).join(", ");
  const message = ERROR_MESSAGES[error.code] ?? `Adapter validation failed (${error.code}).`;
  return location ? `${message} Check ${location}.` : message;
}

export function parseAdapterJson(text: string): ParsedAdapter {
  if (text.length > 65_536) {
    return { ok: false, message: "One adapter definition cannot exceed 65,536 characters." };
  }
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch (error) {
    const detail = error instanceof Error ? error.message : "invalid JSON";
    return { ok: false, message: `The adapter is not valid JSON: ${detail}` };
  }
  if (!isObject(value)) return { ok: false, message: "The adapter must be one JSON object." };
  const compiled = compileDocument({ format: 1, artifacts: [value] });
  if (!compiled.ok) return { ok: false, message: formatValidationError(compiled.error) };
  return { ok: true, adapter: value };
}

export function validateAdapterDocument(document: AdapterDocument): string | null {
  const result = compileDocument(document);
  return result.ok ? null : formatValidationError(result.error);
}

export function overlapText(
  warning: AdapterWarning,
  adapters: readonly AdapterObject[],
): string {
  const first = adapters[warning.first_adapter];
  const second = adapters[warning.second_adapter];
  const firstName = first ? adapterLabel(first, warning.first_adapter) : `Adapter ${warning.first_adapter + 1}`;
  const secondName = second ? adapterLabel(second, warning.second_adapter) : `Adapter ${warning.second_adapter + 1}`;
  return `${firstName} rule ${warning.first_rule + 1} may overlap ${secondName} rule ${warning.second_rule + 1}; ${firstName} wins.`;
}

/** A strip_params entry that removes query parameters adapter rules read. */
export type StripConflict = {
  entry: string;
  params: string[];
  adapters: string[];
};

function asciiLower(value: string): string {
  return value.replace(/[A-Z]/g, (letter) => letter.toLowerCase());
}

/** Whether a strip_params entry removes the query key `key`: ASCII
 * case-insensitively, by prefix for a trailing `*`, as the host strips.
 * The host's `matches_param` (host/src/capture.rs) must agree; both are
 * tested against protocol/strip-params/vectors.json. */
export function stripsParam(entry: string, key: string): boolean {
  const pattern = asciiLower(entry);
  const name = asciiLower(key);
  return pattern.endsWith("*") ? name.startsWith(pattern.slice(0, -1)) : name === pattern;
}

/** The query keys an adapter's rules read: each rule's `query` keys and
 * `absent` list. Nothing else in a template reads the query. */
function queryKeysRead(adapter: AdapterObject): string[] {
  if (!Array.isArray(adapter.match)) return [];
  return adapter.match.flatMap((rule: unknown) => {
    if (!isObject(rule)) return [];
    const keys = isObject(rule.query) ? Object.keys(rule.query) : [];
    const absent = Array.isArray(rule.absent)
      ? rule.absent.filter((key): key is string => typeof key === "string")
      : [];
    return [...keys, ...absent];
  });
}

/**
 * The strip_params entries that remove a parameter some current adapter
 * rule reads, in strip_params order. Stripping it gives the pages that rule
 * tells apart one stored address. The host refuses a save that adds such an
 * entry or rule (brwr-f4s); one already in config.toml is shown here.
 */
export function stripParamConflicts(
  stripParams: readonly string[],
  document: AdapterDocument,
): StripConflict[] {
  const conflicts: StripConflict[] = [];
  for (const entry of new Set(stripParams)) {
    const params = new Set<string>();
    const adapters = new Set<string>();
    document.artifacts.forEach((adapter, index) => {
      const read = queryKeysRead(adapter).filter((key) => stripsParam(entry, key));
      if (!read.length) return;
      for (const key of read) params.add(key);
      adapters.add(`${adapterLabel(adapter, index)} on ${adapterOrigin(adapter) ?? "an invalid origin"}`);
    });
    if (params.size) conflicts.push({ entry, params: [...params], adapters: [...adapters] });
  }
  return conflicts;
}

/** Fixed non-artifact cases from DESIGN.md §5.1. The preview is advisory;
 * the native host remains the config authority. */
export function probeUrls(origin: string): string[] {
  const pathProbes = [
    "/", "/search?q=x", "/?q=x", "/query", "/filter", "/login", "/home", "/dashboard",
    "/invite", "/reset", "/password", "/verify", "/confirm", "/share", "/magic", "/token",
    "/oauth", "/auth", "/signin", "/signup", "/logout", "/unsubscribe",
  ];
  const queryProbes = ["search", "query", "q", "jql", "filter"].map((key) => `/?${key}=x`);
  return [...pathProbes, ...queryProbes].map((path) => `${origin}${path}`);
}

export function previewDocument(
  document: AdapterDocument,
  url: string,
  title: string,
  includeProbes = true,
  probeOrigin?: string,
): { ok: true; rows: PreviewRow[] } | { ok: false; message: string } {
  const compiled = compileDocument(document);
  if (!compiled.ok) return { ok: false, message: formatValidationError(compiled.error) };
  const rows: PreviewRow[] = [];
  const trimmed = url.trim();
  if (trimmed) rows.push({ url: trimmed, probe: false, result: evaluate(compiled.doc, trimmed, title) });
  if (includeProbes) {
    const origin = probeOrigin ?? previewOrigin(trimmed, document.artifacts);
    if (origin) {
      for (const probe of probeUrls(origin)) {
        rows.push({ url: probe, probe: true, result: evaluate(compiled.doc, probe, title) });
      }
    }
  }
  return { ok: true, rows };
}

function previewOrigin(url: string, artifacts: readonly AdapterObject[]): string | null {
  if (url) {
    try {
      const parsed = new URL(url);
      if (parsed.protocol === "http:" || parsed.protocol === "https:") return parsed.origin;
    } catch {
      // Fall through to the edited adapter's origin.
    }
  }
  return artifacts.length === 1 ? adapterOrigin(artifacts[0]!) : null;
}

export function exampleUrl(adapter: AdapterObject): string {
  const origin = adapterOrigin(adapter) ?? "https://example.com";
  const rules = adapter.match;
  if (!Array.isArray(rules) || !isObject(rules[0]) || typeof rules[0].path !== "string") return origin;
  const rule = rules[0];
  const path = rule.path as string;
  let url = origin + fillTemplate(path);
  if (isObject(rule.query)) {
    const query = new URLSearchParams();
    for (const [key, value] of Object.entries(rule.query)) {
      if (typeof value !== "string") continue;
      query.set(key, value === "{_}" ? "present" : fillTemplate(value));
    }
    const suffix = query.toString();
    if (suffix) url += `?${suffix}`;
  }
  return url;
}

function fillTemplate(template: string): string {
  let result = "";
  for (let index = 0; index < template.length;) {
    if (template[index] !== "{") {
      result += template[index];
      index += 1;
      continue;
    }
    const end = template.indexOf("}", index + 1);
    if (end < 0) return template;
    result += exampleCapture(template.slice(index + 1, end));
    index = end + 1;
  }
  return result;
}

function exampleCapture(source: string): string {
  if (source === "*") return "more";
  if (source === "_") return "example";
  const colon = source.indexOf(":");
  const ctype = colon < 0 ? "token" : source.slice(colon + 1);
  if (ctype.startsWith("(")) {
    const separator = ctype.indexOf("|");
    const close = ctype.indexOf(")");
    return ctype.slice(1, separator >= 0 ? separator : close);
  }

  // parseAdapterJson has already validated this ctype. Mirror only the small
  // part of its grammar needed to choose an in-range example; otherwise a
  // valid bounded capture such as token(20,64) previews as a rule miss.
  const parsed = /^(int|hex|token|name|jira_key|uuid)(?:\((\d+)(?:,(\d+))?\))?/.exec(ctype);
  if (!parsed) return "example-123";
  const defaults: Readonly<Record<string, readonly [number, number]>> = {
    int: [1, 19], hex: [1, 64], token: [1, 128], name: [1, 100], jira_key: [1, 255], uuid: [36, 36],
  };
  const [defaultMin, defaultMax] = defaults[parsed[1]!]!;
  const min = parsed[2] === undefined ? defaultMin : Number(parsed[2]);
  const max = parsed[2] === undefined ? defaultMax : Number(parsed[3] ?? parsed[2]);
  const fit = (seed: string, fill: string): string => {
    const length = Math.min(Math.max(seed.length, min), max);
    return seed.length >= length ? seed.slice(0, length) : seed + fill.repeat(length - seed.length);
  };

  let value: string;
  switch (parsed[1]) {
    case "int": value = fit("123456", "1"); break;
    case "hex": value = fit("abcdef", "a"); break;
    case "token": value = fit("example-123", "x"); break;
    case "name": value = fit("example.name", "x"); break;
    case "jira_key": value = `${fit("ABC", "A")}-123`; break;
    case "uuid": value = "123e4567-e89b-12d3-a456-426614174000"; break;
    default: value = "example-123";
  }
  return ctype.includes("@tail") ? `example-${value}` : value;
}

export function newAdapter(origin: string): AdapterObject {
  return {
    type: "work.item",
    origin,
    match: [{ path: "/items/{id:int}" }],
    id: "work.item:{host}/{id}",
  };
}
