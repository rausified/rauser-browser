// Adapter template engine (adapter-templates.md, format version 1). It
// validates an adapter document, then matches URLs and tab titles against it.
// Pure: no chrome or DOM access, so the tests load this module directly. The
// host validates the same document again (§10.3); the worker must reject every
// document the host rejects, so validation follows the check order in
// protocol/adapter-templates/README.md. No regular expressions: every check is
// a character loop, so matching stays linear in the input (§12).

export type ValidationError = { code: string; adapter: number | null; rule: number | null; entry: number | null };
export type ValidationResult = { ok: true } | { error: ValidationError };
export type TypeName = "int" | "hex" | "token" | "name" | "jira_key" | "uuid";
export type RefFind = "jira_key" | "int" | "hex" | "uuid";
export type Mode = "view" | "edit" | "review";

/** A parsed ctype. `min` and `max` are the effective bounds: the project part
 * for `jira_key`, 36 for `uuid`. */
export type CType =
  | { kind: "type"; type: TypeName; min: number; max: number; caseMod: "lower" | "upper" | null; tail: boolean }
  | { kind: "alt"; alts: readonly string[] };

export type EvaluateResult =
  | { match: false; reason: NoMatchReason }
  | ({
    match: true; adapter: number; rule: number; type: string; captures: Record<string, string>;
    artifact_id: string; mode: Mode | null; within?: ContainerMatch;
  } & Partial<TitleResult>);

/** The container a match sits within (§8.3). */
export type ContainerMatch = { type: string; artifact_id: string };

export type NoMatchReason =
  | "url_too_long" | "url_invalid" | "no_adapter" | "path_rejected" | "no_rule" | "out_of_scope" | "id_too_long";

export type TitleResult = {
  title: string; title_entry: number | null; alias_id: string | null; refs: { type: string; value: string }[];
};

/** A validated document, ready for evaluate(). Opaque to callers. */
export type CompiledDocument = { readonly adapters: readonly CompiledAdapter[] };

export type CompileResult = { ok: true; doc: CompiledDocument } | { ok: false; error: ValidationError };

const FORMAT = 1;
const MAX_ADAPTERS = 64;
const MAX_RULES = 16;
const MAX_TEMPLATE_CHARS = 512;
const MAX_TEMPLATE_SEGMENTS = 32;
const MAX_ALTERNATIVES = 16;
const MAX_LITERAL_CHARS = 64;
const MAX_QUERY = 8;
const MAX_ABSENT = 8;
const MAX_TITLES = 4;
const MAX_SCOPE_VALUES = 64;
const MAX_REF_SPECS = 8;
const MAX_URL_CHARS = 2048;
const MAX_URL_SEGMENTS = 64;
const MAX_ANONYMOUS_CHARS = 256;
const MAX_ID_BYTES = 512;
const MAX_TITLE_CHARS = 512;
const MAX_TITLE_TEMPLATE_CHARS = 512;
const MAX_REFS = 20;
const UUID_CHARS = 36;

/** Host-owned shared origins (§6). */
const SHARED_ORIGINS: ReadonlySet<string> = new Set([
  "https://github.com", "https://docs.google.com", "https://www.notion.so", "https://www.figma.com",
  "https://linear.app",
]);

const TYPE_NAMES: readonly TypeName[] = ["int", "hex", "token", "name", "jira_key", "uuid"];
const DEFAULT_BOUNDS: Readonly<Record<TypeName, readonly [number, number]>> = {
  int: [1, 19], hex: [1, 64], token: [1, 128], name: [1, 100], jira_key: [1, 255], uuid: [UUID_CHARS, UUID_CHARS],
};
const REF_FINDS: readonly RefFind[] = ["jira_key", "int", "hex", "uuid"];
const MODES: readonly Mode[] = ["view", "edit", "review"];
const DOC_KEYS: ReadonlySet<string> = new Set(["format", "artifacts"]);
const ADAPTER_KEYS: ReadonlySet<string> = new Set([
  "type", "origin", "match", "id", "title", "alias", "refs", "modes", "scope", "unscoped", "path_case", "fields",
  "within",
]);
const REQUIRED_ADAPTER_KEYS = ["type", "origin", "match", "id"] as const;
const RULE_KEYS: ReadonlySet<string> = new Set(["path", "query", "absent", "mode"]);

/** `folded` holds the ASCII-lowercased literals, compared when path_case is
 * insensitive. */
type Segment =
  | { kind: "lit"; text: string; folded: string }
  | { kind: "alt"; alts: readonly string[]; folded: readonly string[] }
  | { kind: "cap"; name: string; optional: boolean; ctype: CType }
  | { kind: "anon" }
  | { kind: "rest" };

type QueryCondition =
  | { key: string; kind: "cap"; name: string; ctype: CType }
  | { key: string; kind: "lit"; text: string }
  | { key: string; kind: "present" };

type CompiledRule = {
  required: readonly Segment[]; optional: readonly Segment[]; rest: boolean;
  query: readonly QueryCondition[]; absent: readonly string[]; mode: Mode | null;
};

type IdPart = { kind: "lit"; text: string } | { kind: "cap"; name: string };

/** `fail` is the literal's KMP failure table, built when a skip searches for
 * it: over the literal in the prefix, over the reversed literal in the suffix. */
type TitleToken =
  | { kind: "lit"; chars: readonly string[]; fail?: readonly number[] }
  | { kind: "skip" }
  | { kind: "cap"; name: string; type: TypeName; bounds: readonly [number, number] | null }
  | { kind: "title" };

type TitleEntry = { prefix: readonly TitleToken[]; suffix: readonly TitleToken[] };

type CompiledAdapter = {
  type: string; origin: string; host: string; insensitive: boolean; rules: readonly CompiledRule[];
  scheme: string; idParts: readonly IdPart[]; modes: ReadonlyMap<string, Mode>;
  scope: ReadonlyMap<string, readonly string[]>; captureTypes: ReadonlyMap<string, CType>;
  titles: readonly TitleEntry[] | null; aliasCapture: string | null; refs: readonly { type: string; find: RefFind }[];
  within?: { type: string; scheme: string; idParts: readonly IdPart[] };
};

/** Validates an adapter document (§10.3). */
export function validateDocument(doc: unknown): ValidationResult {
  const compiled = compileDocument(doc);
  return compiled.ok ? { ok: true } : { error: compiled.error };
}

/** Validates an adapter document and prepares it for evaluate(). */
export function compileDocument(doc: unknown): CompileResult {
  try {
    return { ok: true, doc: { adapters: compileAdapters(doc) } };
  } catch (error) {
    if (error instanceof Invalid) {
      return { ok: false, error: { code: error.code, adapter: error.adapter, rule: error.rule, entry: error.entry } };
    }
    throw error;
  }
}

/** Matches a URL, and optionally a tab title, against a compiled document
 * (§4.3, §5, §6, §8, §9, §10.1). The first matching rule in config order wins;
 * scope and the ID length are then terminal (SQ4, SQ-V11). */
export function evaluate(doc: CompiledDocument, url: string, title?: string): EvaluateResult {
  if (scalarLength(url) > MAX_URL_CHARS) return { match: false, reason: "url_too_long" };
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return { match: false, reason: "url_invalid" };
  }
  // Only an http(s) URL is ever an artifact. A `blob:` URL carries its
  // creator's origin but addresses an in-memory object, not a page.
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return { match: false, reason: "no_adapter" };
  const origin = parsed.origin;
  const candidates: number[] = [];
  doc.adapters.forEach((adapter, index) => {
    if (adapter.origin === origin) candidates.push(index);
  });
  if (candidates.length === 0) return { match: false, reason: "no_adapter" };
  const segments = urlSegments(parsed.pathname);
  if (segments === null) return { match: false, reason: "path_rejected" };
  const query = new Map<string, string[]>();
  for (const [key, value] of new URLSearchParams(parsed.search)) {
    const values = query.get(key);
    if (values) values.push(value);
    else query.set(key, [value]);
  }
  for (const adapterIndex of candidates) {
    const adapter = doc.adapters[adapterIndex];
    if (!adapter) continue;
    for (let ruleIndex = 0; ruleIndex < adapter.rules.length; ruleIndex += 1) {
      const rule = adapter.rules[ruleIndex];
      if (!rule) continue;
      const captures = matchRule(rule, adapter.insensitive, segments, query);
      if (captures === null) continue;
      for (const [name, values] of adapter.scope) {
        const value = captures.get(name);
        if (value === undefined || !values.includes(value)) return { match: false, reason: "out_of_scope" };
      }
      const artifactId = buildId(adapter.scheme, adapter.host, adapter.idParts, captures);
      if (artifactId === null) return { match: false, reason: "id_too_long" };
      // A container ID past 512 bytes drops only the container (§8.3).
      const containerId = adapter.within
        ? buildId(adapter.within.scheme, adapter.host, adapter.within.idParts, captures)
        : null;
      const modeValue = captures.get("mode");
      const mode = modeValue !== undefined ? adapter.modes.get(modeValue) ?? null : rule.mode;
      // `within` is absent, not null, when the adapter declares none.
      const result: EvaluateResult = {
        match: true, adapter: adapterIndex, rule: ruleIndex, type: adapter.type,
        captures: Object.fromEntries(captures), artifact_id: artifactId, mode,
        ...(adapter.within && containerId !== null
          ? { within: { type: adapter.within.type, artifact_id: containerId } }
          : {}),
      };
      if (title === undefined) return result;
      return { ...result, ...matchTitle(adapter, captures, artifactId, prepareTitle(title)) };
    }
  }
  return { match: false, reason: "no_rule" };
}

/** The canonical value of one decoded segment or query value for a ctype
 * written as in a path capture (`"int(3)"`, `"token|lower"`, `"(a|b)"`), or
 * null when the value is invalid or the ctype does not parse. Alternations
 * compare exactly. */
export function canonicalCapture(ctype: string, value: string): string | null {
  const parsed = parseCType(ctype);
  return isErr(parsed) ? null : canonical(parsed, value, false);
}

/** §5.1 steps 3–6 on a raw WHATWG pathname: the decoded segments, or null when
 * the path matches no rule. */
export function urlSegments(pathname: string): string[] | null {
  const raw = pathname.split("/");
  if (raw[0] === "") raw.shift();
  if (raw.length > 0 && raw[raw.length - 1] === "") raw.pop();
  if (raw.length > MAX_URL_SEGMENTS) return null;
  const segments: string[] = [];
  for (const part of raw) {
    if (part === "") return null;
    const decoded = percentDecode(part);
    if (decoded === null || decoded === "." || decoded === "..") return null;
    for (let i = 0; i < decoded.length; i += 1) {
      const code = decoded.charCodeAt(i);
      if (code === 0x2f || code < 0x20 || code === 0x7f) return null;
    }
    segments.push(decoded);
  }
  return segments;
}

/** §10.1: the distinct canonical values of `find` in `text`, first-seen, at
 * most 20. */
export function scanRefs(find: RefFind, text: string): string[] {
  const chars = Array.from(text);
  const ctype = defaultCType(find);
  const values: string[] = [];
  for (let p = 0; p < chars.length && values.length < MAX_REFS; p += 1) {
    if (p > 0 && isWordChar(chars[p - 1])) continue;
    const end = refCandidateEnd(find, chars, p);
    if (end === null || (end < chars.length && isWordChar(chars[end]))) continue;
    const value = canonical(ctype, chars.slice(p, end).join(""), false);
    if (value !== null && !values.includes(value)) values.push(value);
  }
  return values;
}

/** §9.1 title preparation: well-formed Unicode, then at most 512 scalars. */
export function prepareTitle(raw: string): string {
  let out = "";
  let count = 0;
  for (let i = 0; i < raw.length && count < MAX_TITLE_CHARS; i += 1) {
    const code = raw.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = raw.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        out += raw.slice(i, i + 2);
        i += 1;
      } else {
        out += "�";
      }
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      out += "�";
    } else {
      out += raw[i];
    }
    count += 1;
  }
  return out;
}

// ---- Validation -----------------------------------------------------------

class Invalid extends Error {
  constructor(
    readonly code: string, readonly adapter: number | null, readonly rule: number | null, readonly entry: number | null,
  ) {
    super(code);
  }
}

type Err = { err: string };

function err(code: string): Err {
  return { err: code };
}

function isErr(value: unknown): value is Err {
  return typeof value === "object" && value !== null && "err" in value;
}

function isTable(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function compileAdapters(doc: unknown): CompiledAdapter[] {
  if (!isTable(doc)) throw new Invalid("wrong_type", null, null, null);
  const artifacts = doc.artifacts;
  if (!("format" in doc)) {
    if (Array.isArray(artifacts) && artifacts.length > 0) throw new Invalid("missing_format", null, null, null);
  } else {
    const format = doc.format;
    if (typeof format !== "number" || !Number.isInteger(format) || format < 1) {
      throw new Invalid("invalid_format", null, null, null);
    }
    if (format !== FORMAT) throw new Invalid("unsupported_format", null, null, null);
  }
  for (const key of Object.keys(doc)) if (!DOC_KEYS.has(key)) throw new Invalid("unknown_key", null, null, null);
  if (artifacts === undefined) return [];
  if (!Array.isArray(artifacts)) throw new Invalid("wrong_type", null, null, null);
  if (artifacts.length > MAX_ADAPTERS) throw new Invalid("too_many_adapters", null, null, null);
  return artifacts.map((adapter: unknown, index) => compileAdapter(adapter, index));
}

function compileAdapter(raw: unknown, a: number): CompiledAdapter {
  const fail = (code: string, rule: number | null = null, entry: number | null = null): never => {
    throw new Invalid(code, a, rule, entry);
  };

  // 1. Keys.
  if (!isTable(raw)) return fail("wrong_type");
  for (const key of Object.keys(raw)) if (!ADAPTER_KEYS.has(key)) fail("unknown_key");
  for (const key of REQUIRED_ADAPTER_KEYS) if (!(key in raw)) fail("missing_key");
  const { type, origin, match, id } = raw;
  if (typeof type !== "string" || typeof origin !== "string" || !Array.isArray(match) || typeof id !== "string") {
    return fail("wrong_type");
  }
  // Keys whose own steps list no wrong_type are typed here, down to their
  // elements; title, alias, refs, and fields are typed in steps 8 to 11.
  const isStringList = (v: unknown) => Array.isArray(v) && v.every((item) => typeof item === "string");
  const optionalTypes: [string, (value: unknown) => boolean][] = [
    ["path_case", (v) => typeof v === "string"], ["unscoped", (v) => typeof v === "string"],
    ["modes", (v) => isTable(v) && Object.values(v).every((m) => typeof m === "string")],
    ["scope", (v) => isTable(v) && Object.values(v).every(isStringList)],
    ["within", (v) => isTable(v) && Object.keys(v).sort().join(",") === "id,type"
      && typeof v.id === "string" && typeof v.type === "string"],
  ];
  for (const [key, check] of optionalTypes) if (key in raw && !check(raw[key])) fail("wrong_type");

  // 2. type, origin, path_case.
  if (!isGrammarName(type, "._")) fail("bad_adapter_type");
  if (type === "web.page") fail("reserved_adapter_type");
  let host = "";
  try {
    const url = new URL(origin);
    if ((url.protocol !== "http:" && url.protocol !== "https:") || url.origin !== origin) fail("bad_origin");
    // SQ15: an IP address or an ASCII hostname. WHATWG also accepts hosts
    // such as `*.example.com`, which no real page is served from.
    if (!url.hostname.startsWith("[") && ![...url.hostname].every(isIdLiteralChar)) {
      fail("bad_origin");
    }
    host = url.host;
  } catch (error) {
    if (error instanceof Invalid) throw error;
    fail("bad_origin");
  }
  const pathCase = raw.path_case ?? "sensitive";
  if (pathCase !== "sensitive" && pathCase !== "insensitive") fail("bad_path_case");

  // 3. match.
  if (match.length === 0) fail("no_rules");
  if (match.length > MAX_RULES) fail("too_many_rules");
  const rules: CompiledRule[] = [];
  const ruleCaptures: Map<string, { ctype: CType; optional: boolean }>[] = [];
  match.forEach((rule: unknown, r) => {
    const compiled = compileRule(rule, (code) => fail(code, r));
    rules.push(compiled.rule);
    ruleCaptures.push(compiled.captures);
  });

  // 4. capture_type_conflict.
  const captureTypes = new Map<string, CType>();
  ruleCaptures.forEach((captures, r) => {
    for (const [name, { ctype }] of captures) {
      if (name === "mode") continue;
      const seen = captureTypes.get(name);
      if (seen === undefined) captureTypes.set(name, ctype);
      else if (ctypeKey(seen) !== ctypeKey(ctype)) fail("capture_type_conflict", r);
    }
  });
  for (const captures of ruleCaptures) {
    const mode = captures.get("mode");
    if (mode && !captureTypes.has("mode")) captureTypes.set("mode", mode.ctype);
  }

  // 5. modes.
  const modesTable = isTable(raw.modes) ? raw.modes : null;
  ruleCaptures.forEach((captures, r) => {
    const mode = captures.get("mode");
    if (mode && mode.ctype.kind !== "alt") fail("mode_not_alternation", r);
  });
  ruleCaptures.forEach((captures, r) => {
    const mode = captures.get("mode");
    if (!mode || mode.ctype.kind !== "alt") return;
    for (const literal of mode.ctype.alts) {
      if (modesTable === null || !Object.hasOwn(modesTable, literal)) fail("mode_missing_mapping", r);
    }
  });
  const modes = new Map<string, Mode>();
  if (modesTable !== null) {
    for (const [literal, value] of Object.entries(modesTable)) {
      if (typeof value !== "string" || !isMode(value)) return fail("bad_modes_value");
      modes.set(literal, value);
    }
  }

  // 6. id.
  const parsedId = parseIdTemplate(id);
  if (parsedId === null) return fail("id_syntax");
  if (parsedId.scheme === "web") fail("id_reserved_scheme");
  const idNames = parsedId.parts.flatMap((part) => (part.kind === "cap" ? [part.name] : []));
  if (idNames.includes("mode")) fail("mode_in_id");
  // A second {host} parses as an ordinary {name} part (SQ-V19). No rule can
  // bind a reserved name, so the error names no rule.
  if (idNames.includes("host") || idNames.includes("title")) fail("id_unknown_capture");
  ruleCaptures.forEach((captures, r) => {
    for (const name of idNames) {
      const capture = captures.get(name);
      if (capture === undefined) fail("id_unknown_capture", r);
      else if (capture.optional) fail("id_optional_capture", r);
    }
  });
  const scopeTable = isTable(raw.scope) ? raw.scope : null;
  const scopeNames = new Set(scopeTable ? Object.keys(scopeTable) : []);
  ruleCaptures.forEach((captures, r) => {
    for (const name of captures.keys()) {
      if (name !== "mode" && !scopeNames.has(name) && !idNames.includes(name)) fail("extra_capture", r);
    }
  });

  // 7. scope / unscoped.
  const shared = SHARED_ORIGINS.has(origin);
  if (scopeTable !== null && "unscoped" in raw) fail("scope_and_unscoped");
  if ("unscoped" in raw) {
    if (raw.unscoped !== "acknowledged") fail("bad_unscoped");
    if (!shared) fail("unscoped_not_shared");
  }
  const scope = new Map<string, string[]>();
  if (scopeTable !== null) {
    const entries = Object.entries(scopeTable);
    if (entries.length === 0) fail("scope_empty_list");
    for (const [name, values] of entries) {
      // Lists of strings, by step 1.
      if ((values as string[]).length === 0) fail("scope_empty_list");
      scope.set(name, values as string[]);
    }
    for (const values of scope.values()) if (values.length > MAX_SCOPE_VALUES) fail("too_many_scope_values");
    ruleCaptures.forEach((captures, r) => {
      for (const name of scope.keys()) {
        const capture = captures.get(name);
        if (capture === undefined || capture.optional) fail("scope_capture_not_bound", r);
      }
    });
    // A value must be one some URL could produce (SQ-V22): canonical for at
    // least one rule's binding. Only `mode` may differ between rules.
    for (const [name, values] of scope) {
      for (const value of values) {
        const producible = ruleCaptures.some((captures) => {
          const capture = captures.get(name);
          return capture !== undefined && canonical(capture.ctype, value, false) === value;
        });
        if (!producible) fail("scope_bad_value");
      }
    }
  }
  if (shared && scopeTable === null && !("unscoped" in raw)) fail("shared_origin_unscoped");
  // 4.7.9: each rule needs an anchor among its required path segments. A
  // required path capture named by a scope key anchors too: the scope list
  // pins it the way a literal would. A query binding never anchors.
  rules.forEach((rule, r) => {
    const anchored = rule.required.some((segment) =>
      isAnchorSegment(segment) || (segment.kind === "cap" && !segment.optional && scope.has(segment.name)));
    if (!anchored) fail("no_literal_segment", r);
  });

  // 8. title.
  let titles: TitleEntry[] | null = null;
  if ("title" in raw) {
    if (typeof raw.title !== "string" && !Array.isArray(raw.title)) fail("wrong_type");
    const list: unknown[] = typeof raw.title === "string" ? [raw.title] : (raw.title as unknown[]);
    if (list.length === 0) fail("empty_title_list");
    if (list.length > MAX_TITLES) fail("too_many_titles");
    titles = list.map((entry, e) => compileTitleEntry(entry, captureTypes, (code) => fail(code, null, e)));
  }

  // 9. alias.
  let aliasCapture: string | null = null;
  if ("alias" in raw) {
    if (!isTable(raw.alias)) fail("wrong_type");
    const alias = raw.alias as Record<string, unknown>;
    const keys = Object.keys(alias).sort().join(",");
    let name: string | null = null;
    let metaType: CType | null = null;
    if (alias.from === "tab_title" && keys === "capture,from" && typeof alias.capture === "string") {
      name = alias.capture;
    } else if (
      alias.from === "meta" && keys === "from,name,value" && typeof alias.name === "string" && alias.name !== ""
      && typeof alias.value === "string"
    ) {
      const value = alias.value;
      const capture = value.startsWith("{") && value.endsWith("}") ? parseCapture(value.slice(1, -1)) : err("");
      if (isErr(capture) || capture.optional) return fail("bad_alias");
      name = capture.name;
      metaType = capture.ctype;
    } else {
      return fail("bad_alias");
    }
    if (!idNames.includes(name)) fail("alias_capture_not_id");
    const idType = captureTypes.get(name);
    if (metaType !== null) {
      if (idType === undefined || ctypeKey(idType) !== ctypeKey(metaType)) fail("alias_type_mismatch");
    } else {
      const inTitle = (titles ?? []).some((entry) =>
        [...entry.prefix, ...entry.suffix].some((token) => token.kind === "cap" && token.name === name));
      if (!inTitle) fail("alias_capture_not_in_title");
      aliasCapture = name;
    }
  }

  // 10. refs.
  const refs: { type: string; find: RefFind }[] = [];
  if ("refs" in raw) {
    if (!Array.isArray(raw.refs)) fail("wrong_type");
    if ((raw.refs as unknown[]).length > MAX_REF_SPECS) fail("too_many_refs");
    (raw.refs as unknown[]).forEach((ref, e) => {
      if (
        !isTable(ref) || Object.keys(ref).sort().join(",") !== "find,type" || typeof ref.type !== "string"
        || typeof ref.find !== "string" || !isGrammarName(ref.type, "._")
      ) {
        return fail("bad_ref", null, e);
      }
      const find = REF_FINDS.find((candidate) => candidate === ref.find);
      if (find === undefined) return fail("bad_ref_find", null, e);
      refs.push({ type: ref.type, find });
    });
  }

  // 11. fields.
  if ("fields" in raw) {
    if (!isTable(raw.fields)) fail("wrong_type");
    for (const value of Object.values(raw.fields as Record<string, unknown>)) {
      if (typeof value !== "string") fail("wrong_type");
    }
  }

  // 12. within (§8.3). Typed in step 1. Its captures are the adapter ID's
  // and scope's, so step 6's extra_capture is unchanged.
  let within: CompiledAdapter["within"];
  if (isTable(raw.within)) {
    const { type: containerType, id: containerId } = raw.within as { type: string; id: string };
    if (!isGrammarName(containerType, "._") || containerType === "web.page") fail("within_bad_type");
    if (containerType === type) fail("within_self");
    const parsedContainer = parseIdTemplate(containerId);
    if (parsedContainer === null) return fail("within_id_syntax");
    if (parsedContainer.scheme === "web") fail("within_id_reserved_scheme");
    const containerNames = parsedContainer.parts.flatMap((part) => (part.kind === "cap" ? [part.name] : []));
    ruleCaptures.forEach((captures, r) => {
      for (const name of containerNames) {
        const known = name !== "mode" && captures.has(name) && (idNames.includes(name) || scopeNames.has(name));
        if (!known) fail("within_unknown_capture", r);
      }
    });
    within = { type: containerType, scheme: parsedContainer.scheme, idParts: parsedContainer.parts };
  }

  return {
    type, origin, host, insensitive: pathCase === "insensitive", rules, scheme: parsedId.scheme,
    idParts: parsedId.parts, modes, scope, captureTypes, titles, aliasCapture, refs,
    ...(within ? { within } : {}),
  };
}

/** Orders strings by Unicode code point, which UTF-16 `<` does not do for
 * characters past U+FFFF. */
function compareCodePoints(a: string, b: string): number {
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    const x = a.codePointAt(i) ?? 0;
    const y = b.codePointAt(j) ?? 0;
    if (x !== y) return x - y;
    i += x > 0xffff ? 2 : 1;
    j += y > 0xffff ? 2 : 1;
  }
  return (a.length - i) - (b.length - j);
}

/** §4.1: a required literal, a required alternation (captured or not), or a
 * required fixed-shape capture (`uuid`, or `hex` with bounds that fix its
 * length at exactly 32 or more) anchors a template. An optional segment, and
 * any other capture type, never does. A required path capture named by a
 * scope key also anchors; compileAdapter checks that after scope (4.7.9). */
function isAnchorSegment(segment: Segment): boolean {
  if (segment.kind === "lit" || segment.kind === "alt") return true;
  if (segment.kind !== "cap" || segment.optional) return false;
  const ctype = segment.ctype;
  if (ctype.kind === "alt") return true;
  if (ctype.type === "uuid") return true;
  return ctype.type === "hex" && ctype.min === ctype.max && ctype.min >= 32;
}

function compileRule(
  raw: unknown, fail: (code: string) => never,
): { rule: CompiledRule; captures: Map<string, { ctype: CType; optional: boolean }> } {
  // 1. Rule keys.
  if (!isTable(raw)) return fail("wrong_type");
  for (const key of Object.keys(raw)) if (!RULE_KEYS.has(key)) fail("unknown_key");
  if (!("path" in raw)) fail("missing_key");
  if (typeof raw.path !== "string") return fail("wrong_type");
  if ("query" in raw && !isTable(raw.query)) fail("wrong_type");
  const queryTable = isTable(raw.query) ? raw.query : {};
  if (Object.values(queryTable).some((value) => typeof value !== "string")) fail("wrong_type");
  if ("absent" in raw && !Array.isArray(raw.absent)) fail("wrong_type");
  const absentList: unknown[] = Array.isArray(raw.absent) ? raw.absent : [];
  if (absentList.some((key) => typeof key !== "string")) fail("wrong_type");
  if ("mode" in raw && typeof raw.mode !== "string") fail("wrong_type");

  // 2 and 3. Path parse, then structure.
  const path = raw.path;
  const segments = parsePathTemplate(path);
  if (isErr(segments)) return fail(segments.err);
  if (scalarLength(path) > MAX_TEMPLATE_CHARS) fail("template_too_long");
  if (segments.length > MAX_TEMPLATE_SEGMENTS) fail("too_many_segments");
  segments.forEach((segment, i) => {
    if (segment.kind === "rest" && i !== segments.length - 1) fail("rest_not_last");
  });
  let sawOptional = false;
  for (const segment of segments) {
    const optional = segment.kind === "cap" && segment.optional;
    if (sawOptional && !optional && segment.kind !== "rest") fail("optional_misplaced");
    if (optional) sawOptional = true;
  }

  // 4. query.
  // Conditions are checked in ascending key order by code point (SQ-V21):
  // `Object.keys` puts integer-like keys first, and a parsed object has no
  // document order to recover.
  const query: QueryCondition[] = [];
  const queryKeys = Object.keys(queryTable).sort(compareCodePoints);
  if (queryKeys.length > MAX_QUERY) fail("too_many_query");
  for (const key of queryKeys) {
    if (key === "") fail("bad_query_key");
    const value = queryTable[key] as string;
    if (value === "{_}") {
      query.push({ key, kind: "present" });
    } else if (value.startsWith("{") && value.endsWith("}") && value.length >= 2) {
      const content = value.slice(1, -1);
      if (content === "*") fail("bad_query_value");
      const capture = parseCapture(content);
      if (isErr(capture)) fail(capture.err === "path_syntax" ? "bad_query_value" : capture.err);
      else if (capture.optional) fail("optional_in_query");
      else query.push({ key, kind: "cap", name: capture.name, ctype: capture.ctype });
    } else {
      for (let i = 0; i < value.length; i += 1) {
        const code = value.charCodeAt(i);
        if (code === 0x7b || code === 0x7d || code < 0x20 || code === 0x7f) fail("bad_query_value");
      }
      if (value === "") fail("bad_query_value");
      query.push({ key, kind: "lit", text: value });
    }
  }

  // 5. absent.
  const absent: string[] = [];
  if (absentList.length > MAX_ABSENT) fail("too_many_absent");
  for (const key of absentList as string[]) {
    if (key === "") fail("bad_query_key");
    if (Object.hasOwn(queryTable, key) || absent.includes(key)) fail("query_key_conflict");
    absent.push(key);
  }

  // 6. Rule mode.
  let mode: Mode | null = null;
  if ("mode" in raw) {
    const value = raw.mode as string;
    if (!isMode(value)) return fail("bad_mode");
    mode = value;
  }

  // 7. Captures.
  const captures = new Map<string, { ctype: CType; optional: boolean }>();
  for (const segment of segments) {
    if (segment.kind !== "cap") continue;
    if (captures.has(segment.name)) fail("duplicate_capture");
    captures.set(segment.name, { ctype: segment.ctype, optional: segment.optional });
  }
  for (const condition of query) {
    if (condition.kind !== "cap") continue;
    if (captures.has(condition.name)) fail("duplicate_capture");
    captures.set(condition.name, { ctype: condition.ctype, optional: false });
  }

  const firstOptional = segments.findIndex((segment) => segment.kind === "cap" && segment.optional);
  const rest = segments.length > 0 && segments[segments.length - 1]?.kind === "rest";
  const end = rest ? segments.length - 1 : segments.length;
  const split = firstOptional < 0 ? end : firstOptional;
  return {
    rule: { required: segments.slice(0, split), optional: segments.slice(split, end), rest, query, absent, mode },
    captures,
  };
}

function parsePathTemplate(path: string): Segment[] | Err {
  if (!path.startsWith("/")) return err("path_syntax");
  if (path === "/") return [];
  const segments: Segment[] = [];
  for (const text of path.slice(1).split("/")) {
    if (text === "") return err("path_syntax");
    if (text.startsWith("{")) {
      if (text.length < 2 || !text.endsWith("}")) return err("path_syntax");
      const content = text.slice(1, -1);
      if (content === "_") {
        segments.push({ kind: "anon" });
      } else if (content === "*") {
        segments.push({ kind: "rest" });
      } else {
        const capture = parseCapture(content);
        if (isErr(capture)) return capture;
        segments.push({ kind: "cap", ...capture });
      }
    } else if (text.startsWith("(")) {
      const alternation = parseAlternation(text);
      if (isErr(alternation)) return alternation;
      if (alternation.rest !== "") return err("path_syntax");
      segments.push({ kind: "alt", alts: alternation.alts, folded: alternation.alts.map(asciiLower) });
    } else {
      for (let i = 0; i < text.length; i += 1) if (!isLitChar(text.charCodeAt(i))) return err("path_syntax");
      if (text.length > MAX_LITERAL_CHARS) return err("literal_too_long");
      segments.push({ kind: "lit", text, folded: asciiLower(text) });
    }
  }
  return segments;
}

/** A capture's content between the braces: `name[?]:ctype`. */
function parseCapture(content: string): { name: string; optional: boolean; ctype: CType } | Err {
  const colon = content.indexOf(":");
  if (colon < 0) return err("path_syntax");
  let name = content.slice(0, colon);
  const optional = name.endsWith("?");
  if (optional) name = name.slice(0, -1);
  if (!isGrammarName(name, "_")) return err("bad_capture_name");
  if (name === "host" || name === "title") return err("reserved_capture_name");
  const ctype = parseCType(content.slice(colon + 1));
  if (isErr(ctype)) return ctype;
  if (ctype.kind === "alt") {
    for (const literal of ctype.alts) {
      if (literal === "." || literal === ".." || !allChars(literal, isIdLiteralChar)) return err("bad_capture_literal");
    }
  }
  return { name, optional, ctype };
}

/** ctype = alternation | type_name [bounds] [case_mod] ["@tail"], checked
 * left to right; the first error wins (SQ-V12). */
function parseCType(text: string): CType | Err {
  if (text.startsWith("(")) {
    const alternation = parseAlternation(text);
    if (isErr(alternation)) return alternation;
    if (alternation.rest !== "") return err("path_syntax");
    return { kind: "alt", alts: alternation.alts };
  }
  let i = 0;
  while (i < text.length && isWordChar(text[i])) i += 1;
  if (i === 0) return err("path_syntax");
  const type = TYPE_NAMES.find((candidate) => candidate === text.slice(0, i));
  if (type === undefined) return err("unknown_capture_type");
  let rest = text.slice(i);
  let [min, max] = DEFAULT_BOUNDS[type];
  if (rest.startsWith("(")) {
    const bounds = parseBounds(rest);
    if (isErr(bounds)) return bounds;
    if (type === "uuid") return err("bad_bounds");
    const [low, high] = bounds.bounds;
    if (low === 0 || high === 0 || low > high || low < min || high > max) return err("bad_bounds");
    [min, max] = [low, high];
    rest = bounds.rest;
  }
  let caseMod: "lower" | "upper" | null = null;
  if (rest.startsWith("|")) {
    if (rest.startsWith("|lower")) caseMod = "lower";
    else if (rest.startsWith("|upper")) caseMod = "upper";
    else return err("path_syntax");
    if (type !== "token" && type !== "name") return err("case_mod_not_allowed");
    rest = rest.slice("|lower".length);
  }
  let tail = false;
  if (rest.startsWith("@")) {
    if (!rest.startsWith("@tail")) return err("path_syntax");
    if (type !== "int" && type !== "hex") return err("tail_not_allowed");
    tail = true;
    rest = rest.slice("@tail".length);
  }
  if (rest !== "") return err("path_syntax");
  return { kind: "type", type, min, max, caseMod, tail };
}

/** `(n[,m])` at the start of `text`: the numbers and what follows. Numbers use
 * the int form, without a leading zero (SQ-V1). */
function parseBounds(text: string): { bounds: [number, number]; rest: string } | Err {
  const close = text.indexOf(")");
  if (close < 0) return err("path_syntax");
  const parts = text.slice(1, close).split(",");
  if (parts.length > 2) return err("path_syntax");
  const numbers: number[] = [];
  for (const part of parts) {
    if (part === "" || !allChars(part, isDigit) || (part.length > 1 && part.startsWith("0"))) return err("path_syntax");
    numbers.push(part.length > 9 ? Number.MAX_SAFE_INTEGER : Number(part));
  }
  const low = numbers[0] ?? 0;
  return { bounds: [low, numbers[1] ?? low], rest: text.slice(close + 1) };
}

/** `(a|b|...)` at the start of `text`: the literals and what follows. */
function parseAlternation(text: string): { alts: string[]; rest: string } | Err {
  const close = text.indexOf(")");
  if (close < 0) return err("path_syntax");
  const inner = text.slice(1, close);
  if (inner.includes("(")) return err("path_syntax");
  const alts = inner.split("|");
  for (const literal of alts) {
    if (literal === "") return err("path_syntax");
    for (let i = 0; i < literal.length; i += 1) if (!isLitChar(literal.charCodeAt(i))) return err("path_syntax");
    if (literal.length > MAX_LITERAL_CHARS) return err("literal_too_long");
  }
  if (alts.length > MAX_ALTERNATIVES) return err("too_many_alternatives");
  return { alts, rest: text.slice(close + 1) };
}

/** id_template = scheme ":" "{host}" { "/" id_part } (§8.1), or null. */
function parseIdTemplate(id: string): { scheme: string; parts: IdPart[] } | null {
  const colon = id.indexOf(":");
  if (colon < 0) return null;
  const scheme = id.slice(0, colon);
  if (!isGrammarName(scheme, ".")) return null;
  const rest = id.slice(colon + 1);
  if (!rest.startsWith("{host}")) return null;
  const tail = rest.slice("{host}".length);
  if (tail === "") return { scheme, parts: [] };
  if (!tail.startsWith("/")) return null;
  const parts: IdPart[] = [];
  for (const text of tail.slice(1).split("/")) {
    if (text.startsWith("{") && text.endsWith("}") && text.length >= 2) {
      const name = text.slice(1, -1);
      if (!isGrammarName(name, "_")) return null;
      parts.push({ kind: "cap", name });
    } else if (text !== "" && allChars(text, isIdLiteralChar)) {
      parts.push({ kind: "lit", text });
    } else {
      return null;
    }
  }
  return { scheme, parts };
}

function compileTitleEntry(
  raw: unknown, captureTypes: ReadonlyMap<string, CType>, fail: (code: string) => never,
): TitleEntry {
  if (typeof raw !== "string") return fail("wrong_type");
  if (scalarLength(raw) > MAX_TITLE_TEMPLATE_CHARS) return fail("title_too_long");
  const tokens = parseTitleTemplate(raw);
  if (tokens === null) return fail("title_syntax");
  for (let i = 1; i < tokens.length; i += 1) {
    if (tokens[i]?.kind !== "lit" && tokens[i - 1]?.kind !== "lit") fail("title_unbounded_capture");
  }
  for (const token of tokens) {
    if (token.kind === "cap" && !captureTypes.has(token.name)) fail("title_unknown_capture");
  }
  for (const token of tokens) {
    if (token.kind !== "cap") continue;
    const ctype = captureTypes.get(token.name);
    if (ctype === undefined || ctype.kind !== "type" || ctype.type !== token.type) return fail("title_type_mismatch");
    // A uuid capture takes no bounds in a title (§9.1), even ones equal to its
    // fixed length.
    if (token.bounds !== null && (token.type === "uuid" || token.bounds[0] !== ctype.min || token.bounds[1] !== ctype.max)) {
      fail("title_type_mismatch");
    }
  }
  const at = tokens.findIndex((token) => token.kind === "title");
  const prefix = tokens.slice(0, at).map((token, i, list): TitleToken =>
    token.kind === "lit" && list[i - 1]?.kind === "skip"
      ? { ...token, fail: failureTable(token.chars.length, (k) => token.chars[k]) } : token);
  const suffix = tokens.slice(at + 1).map((token, i, list): TitleToken => {
    if (token.kind !== "lit" || list[i + 1]?.kind !== "skip") return token;
    const n = token.chars.length;
    return { ...token, fail: failureTable(n, (k) => token.chars[n - 1 - k]) };
  });
  prefix.forEach((token, i) => {
    const next = prefix[i + 1];
    if (token.kind === "cap" && next?.kind === "lit" && inTitleAlphabet(token.type, next.chars[0])) {
      fail("title_capture_swallows");
    }
  });
  suffix.forEach((token, i) => {
    const previous = suffix[i - 1];
    if (token.kind === "cap" && previous?.kind === "lit"
      && inTitleAlphabet(token.type, previous.chars[previous.chars.length - 1])) {
      fail("title_capture_swallows");
    }
  });
  return { prefix, suffix };
}

/** title_template tokens (§9.1), or null for title_syntax. */
function parseTitleTemplate(text: string): TitleToken[] | null {
  const tokens: TitleToken[] = [];
  let literal: string[] = [];
  let titles = 0;
  const chars = Array.from(text);
  for (let i = 0; i < chars.length; i += 1) {
    const char = chars[i] ?? "";
    if (char === "}") return null;
    if (char !== "{") {
      literal.push(char);
      continue;
    }
    const close = chars.indexOf("}", i + 1);
    if (close < 0) return null;
    const content = chars.slice(i + 1, close).join("");
    if (content.includes("{")) return null;
    if (literal.length > 0) tokens.push({ kind: "lit", chars: literal });
    literal = [];
    i = close;
    if (content === "title") {
      titles += 1;
      tokens.push({ kind: "title" });
      continue;
    }
    if (content === "~") {
      tokens.push({ kind: "skip" });
      continue;
    }
    const colon = content.indexOf(":");
    if (colon < 0) return null;
    const name = content.slice(0, colon);
    if (!isGrammarName(name, "_")) return null;
    const ctypeText = content.slice(colon + 1);
    let end = 0;
    while (end < ctypeText.length && isWordChar(ctypeText[end])) end += 1;
    const type = TYPE_NAMES.find((candidate) => candidate === ctypeText.slice(0, end));
    if (type === undefined) return null;
    let bounds: [number, number] | null = null;
    let rest = ctypeText.slice(end);
    if (rest.startsWith("(")) {
      const parsed = parseBounds(rest);
      if (isErr(parsed)) return null;
      bounds = parsed.bounds;
      rest = parsed.rest;
    }
    if (rest !== "") return null;
    tokens.push({ kind: "cap", name, type, bounds });
  }
  if (literal.length > 0) tokens.push({ kind: "lit", chars: literal });
  return titles === 1 ? tokens : null;
}

function ctypeKey(ctype: CType): string {
  if (ctype.kind === "alt") return `(${ctype.alts.join("|")})`;
  return `${ctype.type}(${ctype.min},${ctype.max})|${ctype.caseMod ?? ""}@${ctype.tail ? "tail" : ""}`;
}

// ---- Matching -------------------------------------------------------------

function matchRule(
  rule: CompiledRule, insensitive: boolean, segments: readonly string[], query: ReadonlyMap<string, string[]>,
): Map<string, string> | null {
  const n = segments.length;
  const { required, optional } = rule;
  if (n < required.length) return null;
  if (!rule.rest && n > required.length + optional.length) return null;
  const captures = new Map<string, string>();
  for (let i = 0; i < required.length; i += 1) {
    if (!matchSegment(required[i], segments[i] ?? "", insensitive, captures)) return null;
  }
  for (let j = 0; j < optional.length && required.length + j < n; j += 1) {
    if (matchSegment(optional[j], segments[required.length + j] ?? "", insensitive, captures)) continue;
    if (rule.rest) break;
    return null;
  }
  for (const condition of rule.query) {
    const values = query.get(condition.key);
    if (values === undefined || values.length !== 1) return null;
    const value = values[0] ?? "";
    if (condition.kind === "present") {
      if (value === "") return null;
    } else if (condition.kind === "lit") {
      if (value !== condition.text) return null;
    } else {
      const canonicalValue = canonical(condition.ctype, value, false);
      if (canonicalValue === null) return null;
      captures.set(condition.name, canonicalValue);
    }
  }
  for (const key of rule.absent) if (query.has(key)) return null;
  return captures;
}

function matchSegment(
  segment: Segment | undefined, value: string, insensitive: boolean, captures: Map<string, string>,
): boolean {
  if (segment === undefined) return false;
  switch (segment.kind) {
    case "lit":
      return insensitive ? foldedEquals(segment.folded, value) : segment.text === value;
    case "alt":
      return insensitive
        ? segment.folded.some((literal) => foldedEquals(literal, value))
        : segment.alts.includes(value);
    case "anon":
      return value !== "" && scalarLength(value) <= MAX_ANONYMOUS_CHARS;
    case "rest":
      return false;
    case "cap": {
      const canonicalValue = canonical(segment.ctype, value, insensitive);
      if (canonicalValue === null) return false;
      captures.set(segment.name, canonicalValue);
      return true;
    }
  }
}

/** Whether `value`, ASCII-lowercased, equals the already-lowercased `folded`.
 * Folding keeps the length and never touches a surrogate, so this compares
 * code units without allocating and stops at the first difference. */
function foldedEquals(folded: string, value: string): boolean {
  if (folded.length !== value.length) return false;
  for (let i = 0; i < value.length; i += 1) {
    const unit = value.charCodeAt(i);
    if ((unit >= 0x41 && unit <= 0x5a ? unit + 32 : unit) !== folded.charCodeAt(i)) return false;
  }
  return true;
}

/** The canonical value for a ctype (§7), or null. `fold` applies path_case to
 * captured alternations, which then bind the template literal. */
function canonical(ctype: CType, value: string, fold: boolean): string | null {
  // Captured alternation literals are lowercase (bad_capture_literal).
  if (ctype.kind === "alt") {
    return ctype.alts.find((literal) => (fold ? foldedEquals(literal, value) : literal === value)) ?? null;
  }
  let text = value;
  if (ctype.tail) {
    const dash = text.lastIndexOf("-");
    if (dash >= 0) text = text.slice(dash + 1);
  }
  const { min, max } = ctype;
  const inRange = (length: number): boolean => length >= min && length <= max;
  switch (ctype.type) {
    case "int":
      return isIntText(text) && inRange(text.length) ? text : null;
    case "hex":
      return allChars(text, isHexDigit) && inRange(text.length) ? asciiLower(text) : null;
    case "token":
      return allChars(text, isTokenChar) && inRange(text.length) ? applyCase(text, ctype.caseMod) : null;
    case "name":
      if (text === "." || text === ".." || !allChars(text, isNameChar) || !inRange(text.length)) return null;
      return applyCase(text, ctype.caseMod);
    case "jira_key": {
      const dash = text.indexOf("-");
      if (dash < 0) return null;
      const project = text.slice(0, dash);
      if (!isAsciiLetter(project[0]) || !allChars(project, isWordChar) || !inRange(project.length)) return null;
      const number = text.slice(dash + 1);
      return isIntText(number) && number.length <= DEFAULT_BOUNDS.int[1] ? asciiUpper(text) : null;
    }
    case "uuid": {
      if (text.length === UUID_CHARS + 2 && text.startsWith("{") && text.endsWith("}")) text = text.slice(1, -1);
      if (text.length !== UUID_CHARS) return null;
      for (let i = 0; i < text.length; i += 1) {
        const dash = i === 8 || i === 13 || i === 18 || i === 23;
        if (dash ? text[i] !== "-" : !isHexDigit(text[i])) return null;
      }
      return asciiLower(text);
    }
  }
}

function defaultCType(type: TypeName): CType {
  const [min, max] = DEFAULT_BOUNDS[type];
  return { kind: "type", type, min, max, caseMod: null, tail: false };
}

/** The artifact ID, or null once it passes 512 bytes (§8.1). The template has
 * no length cap, so stopping there keeps the work per visit bounded by the ID
 * limit rather than by the config (§10.2). */
function buildId(
  scheme: string, host: string, idParts: readonly IdPart[], captures: ReadonlyMap<string, string>,
): string | null {
  let id = `${scheme}:${host}`;
  let bytes = utf8Length(id);
  for (const part of idParts) {
    if (bytes > MAX_ID_BYTES) return null;
    const text = part.kind === "lit" ? part.text : captures.get(part.name) ?? "";
    id += `/${text}`;
    bytes += 1 + utf8Length(text);
  }
  return bytes > MAX_ID_BYTES ? null : id;
}

// ---- Titles and refs ------------------------------------------------------

function matchTitle(
  adapter: CompiledAdapter, captures: ReadonlyMap<string, string>, artifactId: string, prepared: string,
): TitleResult {
  const chars = Array.from(prepared);
  const entries = adapter.titles ?? [];
  for (let e = 0; e < entries.length; e += 1) {
    const entry = entries[e];
    if (!entry) continue;
    const found = matchTitleEntry(entry, chars, adapter, captures);
    if (found === null) continue;
    let aliasId: string | null = null;
    // An out-of-scope URL is not an artifact (§6), so an alias whose scoped
    // capture leaves the scope is not suggested either.
    const scoped = adapter.aliasCapture === null ? undefined : adapter.scope.get(adapter.aliasCapture);
    if (found.alias !== null && adapter.aliasCapture !== null && (scoped === undefined || scoped.includes(found.alias))) {
      const aliased = new Map(captures);
      aliased.set(adapter.aliasCapture, found.alias);
      const candidate = buildId(adapter.scheme, adapter.host, adapter.idParts, aliased);
      if (candidate !== null && candidate !== artifactId) aliasId = candidate;
    }
    return { title: found.title, title_entry: e, alias_id: aliasId, refs: findRefs(adapter, found.title) };
  }
  return { title: prepared, title_entry: null, alias_id: null, refs: findRefs(adapter, prepared) };
}

/** Matches P forward and Q backward (§9.1). Returns the trimmed title and the
 * alias capture's title value when it differs from the URL's, or null. */
function matchTitleEntry(
  entry: TitleEntry, chars: readonly string[], adapter: CompiledAdapter, captures: ReadonlyMap<string, string>,
): { title: string; alias: string | null } | null {
  const found: [string, string][] = [];
  let start = 0;
  const { prefix, suffix } = entry;
  for (let k = 0; k < prefix.length; k += 1) {
    const token = prefix[k];
    if (token?.kind === "lit") {
      if (!literalAt(chars, token.chars, start)) return null;
      start += token.chars.length;
    } else if (token?.kind === "skip") {
      const next = prefix[k + 1];
      if (next?.kind !== "lit") return null;
      const at = indexOfChars(chars, next.chars, start, next.fail);
      if (at < 0) return null;
      start = at + next.chars.length;
      k += 1;
    } else if (token?.kind === "cap") {
      let end = start;
      while (end < chars.length && inTitleAlphabet(token.type, chars[end])) end += 1;
      found.push([token.name, chars.slice(start, end).join("")]);
      start = end;
    }
  }
  let end = chars.length;
  for (let k = suffix.length - 1; k >= 0; k -= 1) {
    const token = suffix[k];
    if (token?.kind === "lit") {
      const at = end - token.chars.length;
      if (at < 0 || !literalAt(chars, token.chars, at)) return null;
      end = at;
    } else if (token?.kind === "skip") {
      const previous = suffix[k - 1];
      if (previous?.kind !== "lit") return null;
      const at = lastIndexOfChars(chars, previous.chars, end, previous.fail);
      if (at < 0) return null;
      end = at;
      k -= 1;
    } else if (token?.kind === "cap") {
      let at = end;
      while (at > 0 && inTitleAlphabet(token.type, chars[at - 1])) at -= 1;
      found.push([token.name, chars.slice(at, end).join("")]);
      end = at;
    }
  }
  if (start > end) return null;
  const title = trimWhiteSpace(chars.slice(start, end).join(""));
  if (title === "") return null;
  let alias: string | null = null;
  for (const [name, raw] of found) {
    const ctype = adapter.captureTypes.get(name);
    const value = ctype === undefined ? null : canonical(ctype, raw, false);
    if (value === null) return null;
    const bound = captures.get(name);
    if (value === bound) continue;
    if (name !== adapter.aliasCapture || bound === undefined) return null;
    alias = value;
  }
  return { title, alias };
}

/** At most 20 distinct (type, value) refs per event, across every ref in
 * `refs` order (§10.1). */
function findRefs(adapter: CompiledAdapter, text: string): { type: string; value: string }[] {
  const out: { type: string; value: string }[] = [];
  // Refs that share a `find` share one scan of the text.
  const scans = new Map<RefFind, string[]>();
  for (const ref of adapter.refs) {
    let values = scans.get(ref.find);
    if (values === undefined) {
      values = scanRefs(ref.find, text);
      scans.set(ref.find, values);
    }
    for (const value of values) {
      if (out.some((seen) => seen.type === ref.type && seen.value === value)) continue;
      if (out.length === MAX_REFS) return out;
      out.push({ type: ref.type, value });
    }
  }
  return out;
}

/** The end of the single ref candidate that starts at `p`, or null. */
function refCandidateEnd(find: RefFind, chars: readonly string[], p: number): number | null {
  let q = p;
  switch (find) {
    case "jira_key": {
      if (!isAsciiUpper(chars[q])) return null;
      q += 1;
      while (q < chars.length && (isAsciiUpper(chars[q]) || isDigit(chars[q]) || chars[q] === "_")) q += 1;
      if (chars[q] !== "-") return null;
      q += 1;
      const digits = q;
      while (q < chars.length && isDigit(chars[q])) q += 1;
      return q > digits ? q : null;
    }
    case "int":
      while (q < chars.length && isDigit(chars[q])) q += 1;
      return q > p ? q : null;
    case "hex":
      while (q < chars.length && isHexDigit(chars[q])) q += 1;
      return q > p ? q : null;
    case "uuid":
      return p + UUID_CHARS <= chars.length ? p + UUID_CHARS : null;
  }
}

function literalAt(chars: readonly string[], literal: readonly string[], at: number): boolean {
  if (at < 0 || at + literal.length > chars.length) return false;
  for (let i = 0; i < literal.length; i += 1) if (chars[at + i] !== literal[i]) return false;
  return true;
}

/** The first occurrence of `literal` at or after `from`, or -1. Linear in the
 * text scanned, given the literal's failure table `fail`; without one the
 * table is built here, adding the literal's length (§10.2). */
export function indexOfChars(
  chars: readonly string[], literal: readonly string[], from: number, fail?: readonly number[],
): number {
  const start = Math.max(from, 0);
  const table = fail ?? failureTable(literal.length, (i) => literal[i]);
  const at = kmp(chars.length - start, literal.length, (i) => chars[start + i], (i) => literal[i], table);
  return at < 0 ? -1 : start + at;
}

/** The start of the last occurrence of `literal` that ends at or before `end`,
 * or -1: the first occurrence of the reversed literal in the reversed text.
 * `fail` is the reversed literal's failure table. */
export function lastIndexOfChars(
  chars: readonly string[], literal: readonly string[], end: number, fail?: readonly number[],
): number {
  const h = Math.min(end, chars.length);
  const n = literal.length;
  const table = fail ?? failureTable(n, (i) => literal[n - 1 - i]);
  const at = kmp(h, n, (i) => chars[h - 1 - i], (i) => literal[n - 1 - i], table);
  return at < 0 ? -1 : h - at - n;
}

/** The KMP failure table of an indexed sequence of length `n`: `fail[i]` is
 * the length of the longest proper border of needle[0..i]. */
function failureTable(n: number, needle: (i: number) => string | undefined): number[] {
  const fail = new Array<number>(n).fill(0);
  for (let i = 1, k = 0; i < n; i += 1) {
    while (k > 0 && needle(i) !== needle(k)) k = fail[k - 1] ?? 0;
    if (needle(i) === needle(k)) k += 1;
    fail[i] = k;
  }
  return fail;
}

/** Knuth-Morris-Pratt search over indexed sequences of lengths `h` and `n`:
 * the start of the first occurrence, or -1, in O(h) comparisons given the
 * needle's failure table. A needle longer than the text cannot occur. */
function kmp(
  h: number, n: number, hay: (i: number) => string | undefined, needle: (i: number) => string | undefined,
  fail: readonly number[],
): number {
  if (n === 0) return h >= 0 ? 0 : -1;
  if (n > h) return -1;
  for (let i = 0, k = 0; i < h; i += 1) {
    while (k > 0 && hay(i) !== needle(k)) k = fail[k - 1] ?? 0;
    if (hay(i) === needle(k)) k += 1;
    if (k === n) return i + 1 - n;
  }
  return -1;
}

/** The Unicode White_Space property. U+FEFF is not in it. */
function isWhiteSpace(char: string): boolean {
  const code = char.charCodeAt(0);
  return (code >= 0x09 && code <= 0x0d) || code === 0x20 || code === 0x85 || code === 0xa0 || code === 0x1680
    || (code >= 0x2000 && code <= 0x200a) || code === 0x2028 || code === 0x2029 || code === 0x202f
    || code === 0x205f || code === 0x3000;
}

function trimWhiteSpace(text: string): string {
  let start = 0;
  let end = text.length;
  while (start < end && isWhiteSpace(text[start] ?? "")) start += 1;
  while (end > start && isWhiteSpace(text[end - 1] ?? "")) end -= 1;
  return text.slice(start, end);
}

/** The character run a title capture takes (§9.1). A uuid run stops at a
 * brace, so a braced UUID in a title does not match (SQ7). */
function inTitleAlphabet(type: TypeName, char: string | undefined): boolean {
  switch (type) {
    case "int":
      return isDigit(char);
    case "hex":
      return isHexDigit(char);
    case "token":
      return isTokenChar(char);
    case "name":
      return isNameChar(char);
    case "jira_key":
      return isWordChar(char) || char === "-";
    case "uuid":
      return isHexDigit(char) || char === "-";
  }
}

// ---- Characters -----------------------------------------------------------

/** Strict percent-decoding as UTF-8, or null (§5.1 step 4). */
function percentDecode(text: string): string | null {
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] !== "%") continue;
    if (!isHexDigit(text[i + 1]) || !isHexDigit(text[i + 2])) return null;
    i += 2;
  }
  try {
    return decodeURIComponent(text);
  } catch {
    return null;
  }
}

function scalarLength(text: string): number {
  let count = 0;
  for (let i = 0; i < text.length; i += 1) {
    const code = text.charCodeAt(i);
    if (code >= 0xd800 && code <= 0xdbff) {
      const next = text.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) i += 1;
    }
    count += 1;
  }
  return count;
}

function utf8Length(text: string): number {
  let bytes = 0;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
  }
  return bytes;
}

/** `[a-z]` then at most 31 of `[a-z0-9]` or `extra`. */
function isGrammarName(text: string, extra: string): boolean {
  if (text.length === 0 || text.length > 32 || !isAsciiLower(text[0])) return false;
  for (let i = 1; i < text.length; i += 1) {
    const char = text[i];
    if (!isAsciiLower(char) && !isDigit(char) && !extra.includes(char ?? "")) return false;
  }
  return true;
}

function isIntText(text: string): boolean {
  return text !== "" && allChars(text, isDigit) && (text === "0" || !text.startsWith("0"));
}

function isMode(value: string): value is Mode {
  return MODES.some((mode) => mode === value);
}

function applyCase(text: string, caseMod: "lower" | "upper" | null): string {
  return caseMod === "lower" ? asciiLower(text) : caseMod === "upper" ? asciiUpper(text) : text;
}

function asciiLower(text: string): string {
  let out = "";
  for (const char of text) out += isAsciiUpper(char) ? String.fromCharCode(char.charCodeAt(0) + 32) : char;
  return out;
}

function asciiUpper(text: string): string {
  let out = "";
  for (const char of text) out += isAsciiLower(char) ? String.fromCharCode(char.charCodeAt(0) - 32) : char;
  return out;
}

function allChars(text: string, check: (char: string | undefined) => boolean): boolean {
  for (let i = 0; i < text.length; i += 1) if (!check(text[i])) return false;
  return true;
}

/** lit_char: ASCII printable except / { } ( ) | ? # % and space (§4.1). */
function isLitChar(code: number): boolean {
  return code > 0x20 && code < 0x7f && !"/{}()|?#%".includes(String.fromCharCode(code));
}

function isDigit(char: string | undefined): boolean {
  return char !== undefined && char.length === 1 && char >= "0" && char <= "9";
}

function isAsciiLower(char: string | undefined): boolean {
  return char !== undefined && char.length === 1 && char >= "a" && char <= "z";
}

function isAsciiUpper(char: string | undefined): boolean {
  return char !== undefined && char.length === 1 && char >= "A" && char <= "Z";
}

function isAsciiLetter(char: string | undefined): boolean {
  return isAsciiLower(char) || isAsciiUpper(char);
}

function isHexDigit(char: string | undefined): boolean {
  if (isDigit(char) || char === undefined || char.length !== 1) return isDigit(char);
  return (char >= "a" && char <= "f") || (char >= "A" && char <= "F");
}

/** ASCII `[A-Za-z0-9_]`; anything else is a ref boundary (§10.1). */
function isWordChar(char: string | undefined): boolean {
  return isAsciiLetter(char) || isDigit(char) || char === "_";
}

function isTokenChar(char: string | undefined): boolean {
  return isWordChar(char) || char === "-";
}

function isNameChar(char: string | undefined): boolean {
  return isTokenChar(char) || char === ".";
}

/** ID-literal characters: `a-z`, `0-9`, `.`, `_`, `-` (§8.1). */
function isIdLiteralChar(char: string | undefined): boolean {
  return isAsciiLower(char) || isDigit(char) || char === "." || char === "_" || char === "-";
}
