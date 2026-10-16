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
  no_literal_segment: "Each rule needs a fixed route segment or fixed-shape identifier.",
  bad_unscoped: "unscoped must be acknowledged, and only when the origin requires it.",
  shared_origin_unscoped: "This shared origin needs a tenant scope or an explicit unscoped acknowledgment.",
  scope_capture_not_bound: "Every rule must bind each capture used for scope.",
  title_syntax: "A title template has invalid syntax.",
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
  let type = colon < 0 ? "token" : source.slice(colon + 1);
  if (colon > 0 && source[colon - 1] === "?") type = source.slice(colon + 1);
  if (type.startsWith("(")) {
    const separator = type.indexOf("|");
    const close = type.indexOf(")");
    return type.slice(1, separator >= 0 ? separator : close);
  }
  if (type.startsWith("int")) return "123456";
  if (type.startsWith("jira_key")) return "ABC-123";
  if (type.startsWith("uuid")) return "123e4567-e89b-12d3-a456-426614174000";
  if (type.startsWith("hex")) {
    const open = type.indexOf("(");
    const close = type.indexOf(")");
    const length = open >= 0 && close > open ? Number.parseInt(type.slice(open + 1, close), 10) : 32;
    const hex = "a".repeat(Number.isFinite(length) ? Math.min(Math.max(length, 1), 64) : 32);
    return type.includes("@tail") ? `example-${hex}` : hex;
  }
  return "example-123";
}

export function newAdapter(origin: string): AdapterObject {
  return {
    type: "work.item",
    origin,
    match: [{ path: "/items/{id:int}" }],
    id: "work.item:{host}/{id}",
  };
}
