#!/usr/bin/env node

// Differential property tests for the two adapter-template engines
// (adapter-templates.md §12). A seeded generator writes random adapter
// documents, most of them valid, and URLs and tab titles derived from their
// rules, including near misses: one character changed, a segment added or
// removed, a case change, and encoded characters. `brauser adapters eval`
// (the Rust engine) and scripts/adapter-templates-eval.mjs (the TypeScript
// engine) evaluate every case, and any difference fails. A small share of
// documents is mutated so validation is compared too.
//
// Build both engines first:
//   cargo build --locked -p brauser && npm run build:extension
// Then run, for example:
//   npm run test:adapter-diff -- --cases 20000 --seed 7
//   npm run test:adapter-diff -- --seed 7 --only 1234   (one case, printed)
//
// Every case is generated from (seed, index) alone, so a failure reported as
// "seed 7 case 1234" reproduces with --seed 7 --only 1234.

import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual, parseArgs } from "node:util";
import { BINARY_NAME } from "../extension/brand.ts";

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const EXE = process.platform === "win32" ? ".exe" : "";
const DEFAULT_HOST = path.join(REPO, "target", "debug", `${BINARY_NAME}${EXE}`);
const NODE_HARNESS = path.join(REPO, "scripts", "adapter-templates-eval.mjs");
const BUILD = `cargo build --locked -p ${BINARY_NAME} && npm run build:extension`;
// Cases per engine process; keeps each run's stdin and stdout modest.
const BATCH = 2000;
const SHOWN_FAILURES = 5;

// ---- Seeded randomness ------------------------------------------------------

/** mulberry32: a small, fast, well-mixed 32-bit generator. */
function generator(seed) {
  let state = seed >>> 0;
  const next = () => {
    state = (state + 0x6d2b79f5) >>> 0;
    let t = state;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const r = {
    next,
    int: (n) => Math.floor(next() * n),
    range: (lo, hi) => lo + Math.floor(next() * (hi - lo + 1)),
    chance: (p) => next() < p,
    pick: (items) => items[Math.floor(next() * items.length)],
    shuffle: (items) => {
      const out = [...items];
      for (let i = out.length - 1; i > 0; i -= 1) {
        const j = Math.floor(next() * (i + 1));
        [out[i], out[j]] = [out[j], out[i]];
      }
      return out;
    },
    /** A length in [lo, hi], usually short, sometimes at either end. */
    length: (lo, hi) => {
      const roll = next();
      if (roll < 0.1) return lo;
      if (roll < 0.2) return hi;
      return r.range(lo, Math.min(hi, lo + 11));
    },
    string: (alphabet, n) => Array.from({ length: n }, () => alphabet[Math.floor(next() * alphabet.length)]).join(""),
  };
  return r;
}

function caseSeed(seed, index) {
  let h = Math.imul(seed ^ 0x9e3779b9, 0x85ebca6b) ^ Math.imul(index + 1, 0xc2b2ae35);
  h ^= h >>> 16;
  h = Math.imul(h, 0x7feb352d);
  return (h ^ (h >>> 15)) >>> 0;
}

// ---- Capture types ----------------------------------------------------------

const DIGITS = "0123456789";
const LOWER = "abcdefghijklmnopqrstuvwxyz";
const UPPER = LOWER.toUpperCase();
const HEX = "0123456789abcdefABCDEF";
const TOKEN = `${LOWER}${UPPER}${DIGITS}_-`;
const NAME = `${TOKEN}.`;
const DEFAULT_BOUNDS = { int: [1, 19], hex: [1, 64], token: [1, 128], name: [1, 100], jira_key: [1, 255] };
const ID_LITERAL = `${LOWER}${DIGITS}._-`;

/** A random ctype: {base, min, max, text} or {alts, text}. */
function genCtype(r, { allowAlt = true } = {}) {
  if (allowAlt && r.chance(0.12)) {
    const alts = [...new Set(Array.from({ length: r.range(1, 4) }, () => r.pick(["a", "b", "edit", "v1", "x-y", "k.z", "_"])))];
    return { alts, text: `(${alts.join("|")})` };
  }
  const base = r.pick(["int", "int", "hex", "token", "name", "jira_key", "uuid"]);
  if (base === "uuid") return { base, min: 36, max: 36, text: "uuid" };
  let [min, max] = DEFAULT_BOUNDS[base];
  let text = base;
  if (r.chance(0.3)) {
    const lo = r.range(min, Math.min(max, min + 6));
    const hi = r.range(lo, Math.min(max, lo + 12));
    [min, max] = [lo, hi];
    text += lo === hi && r.chance(0.5) ? `(${lo})` : `(${lo},${hi})`;
  }
  const ctype = { base, min, max, caseMod: null, tail: false, text };
  if ((base === "token" || base === "name") && r.chance(0.3)) {
    ctype.caseMod = r.pick(["lower", "upper"]);
    ctype.text += `|${ctype.caseMod}`;
  }
  if ((base === "int" || base === "hex") && r.chance(0.2)) {
    ctype.tail = true;
    ctype.text += "@tail";
  }
  return ctype;
}

/** A value the ctype accepts (before any @tail slug). A `long` value takes the
 * type's maximum length, so IDs can reach the 512-byte cap. */
function genValue(r, ctype, long = false) {
  if (ctype.alts) return r.pick(ctype.alts);
  const n = long ? ctype.max : r.length(ctype.min, ctype.max);
  switch (ctype.base) {
    case "int":
      return n === 1 ? r.string(DIGITS, 1) : r.string("123456789", 1) + r.string(DIGITS, n - 1);
    case "hex":
      return r.string(HEX, n);
    case "token":
      return r.string(TOKEN, n);
    case "name": {
      const value = r.string(NAME, n);
      return value === "." || value === ".." ? "n" : value;
    }
    case "jira_key": {
      if (long) return r.string(UPPER, 1) + r.string(`${UPPER}${DIGITS}_`, Math.max(0, n - 3)) + "-1";
      return r.string(`${LOWER}${UPPER}`, 1) + r.string(`${UPPER}${DIGITS}_`, n - 1) + "-" + genValue(r, { base: "int", min: 1, max: 6 });
    }
    default: {
      const hex = (k) => r.string(HEX, k);
      const uuid = `${hex(8)}-${hex(4)}-${hex(4)}-${hex(4)}-${hex(12)}`;
      return r.chance(0.3) ? `{${uuid}}` : uuid;
    }
  }
}

/** The canonical value a title capture of the same name should carry. */
function canonical(ctype, value) {
  if (ctype.alts) return value;
  if (ctype.base === "hex" || ctype.base === "uuid") return value.toLowerCase().replace("{", "").replace("}", "");
  if (ctype.base === "jira_key") return value.toUpperCase();
  if (ctype.caseMod === "lower") return value.toLowerCase();
  if (ctype.caseMod === "upper") return value.toUpperCase();
  return value;
}

// ---- Adapter documents -------------------------------------------------------

const ORIGINS = [
  "https://acme.example.com", "https://acme.example.com", "http://tools.example.com:8080", "https://127.0.0.1",
  "https://[::1]:8443", "https://xn--bcher-kva.example", "https://github.com", "https://docs.google.com",
  "https://www.notion.so", "https://linear.app",
];
const SHARED = new Set(["https://github.com", "https://docs.google.com", "https://www.notion.so", "https://www.figma.com", "https://linear.app"]);
// Path literals: ordinary words plus characters the URL parser encodes (`"`,
// `<`, `>`, backtick), keeps (`!$&'*+,;=:@~`), or reads as a separator (`\`).
const PATH_WORDS = [
  "a", "b", "items", "browse", "Doc.aspx", "_layouts", "15", "x-y", "~u", "pull", "v1", "A", "wiki", ":w:", "r",
  "@me", "a+b", "x=1", "k;v", "a,b", "$x", "!", "*", "'q'", "a\"b", "<t>", "`b`", "[x]", "^", "a\\b", "...", "e",
];
const QUERY_KEYS = ["q", "id", "selectedIssue", "sourcedoc", "k", "a b", "x+y", "é", "mode", "p"];
const QUERY_LITERALS = ["edit", "1", "a b", "x+y", "é", "true", "%", "a=b", "&"];

/** One adapter: rules, ID, and optionally modes, scope, titles, alias, refs. */
function genAdapter(r, index, origin) {
  const captureCount = r.range(1, 3);
  const captures = Array.from({ length: captureCount }, (_, i) => ({
    name: r.pick(["id", "key", "owner", "repo", "doc", "n", "x_1"]) + (i === 0 ? "" : String(i)),
    ctype: genCtype(r),
  }));
  const shared = SHARED.has(origin);
  // A scoped capture: a name or token capture restricted to listed values.
  let scoped = null;
  if (shared ? r.chance(0.7) : r.chance(0.1)) {
    const ctype = r.chance(0.5) ? { base: "name", min: 1, max: 100, caseMod: "lower", tail: false, text: "name|lower" } : genCtype(r, { allowAlt: false });
    scoped = { name: "org", ctype, values: [] };
  }
  const useMode = r.chance(0.3);
  const modeAlts = r.shuffle(["edit", "view", "preview", "files"]).slice(0, r.range(1, 3));
  const insensitive = r.chance(0.2);
  const rules = [];
  const ruleCount = r.range(1, 3);
  for (let i = 0; i < ruleCount; i += 1) rules.push(genRule(r, captures, scoped, useMode ? modeAlts : null));
  if (scoped) {
    const count = r.range(1, 2);
    for (let i = 0; i < count; i += 1) scoped.values.push(canonical(scoped.ctype, genValue(r, scoped.ctype)));
    scoped.values = [...new Set(scoped.values)];
  }
  const scheme = r.pick(["acme", "a.b", "x9", "jira"]) + (index === 0 ? "" : String(index));
  const idParts = [];
  for (const c of r.shuffle(captures)) {
    if (r.chance(0.3)) idParts.push(r.pick(["v1", "p", "..", "x-y"]));
    idParts.push(`{${c.name}}`);
  }
  // A repeated capture lets long values pass the 512-byte ID cap.
  if (r.chance(0.1)) for (const c of captures) idParts.push(`{${c.name}}`);
  if (scoped && r.chance(0.4)) idParts.unshift(`{${scoped.name}}`);
  const adapter = {
    type: `t${index}.${r.pick(["item", "page", "issue"])}`,
    origin,
    match: rules.map((rule) => rule.json),
    id: `${scheme}:{host}/${idParts.join("/")}`,
  };
  if (insensitive) adapter.path_case = "insensitive";
  else if (r.chance(0.1)) adapter.path_case = "sensitive";
  if (useMode) adapter.modes = Object.fromEntries(modeAlts.map((alt) => [alt, r.pick(["view", "edit", "review"])]));
  if (scoped) adapter.scope = { [scoped.name]: scoped.values };
  else if (shared) adapter.unscoped = "acknowledged";
  const titled = captures.filter((c) => !c.ctype.alts);
  let titles = null;
  if (r.chance(0.6)) {
    titles = Array.from({ length: r.chance(0.8) ? 1 : r.range(2, 4) }, () => genTitleTemplate(r, titled));
    adapter.title = titles.length === 1 && r.chance(0.7) ? titles[0].text : titles.map((t) => t.text);
    const aliasable = titled.filter((c) => titles.some((t) => t.names.includes(c.name)));
    if (aliasable.length > 0 && r.chance(0.4)) adapter.alias = { from: "tab_title", capture: r.pick(aliasable).name };
  }
  if (r.chance(0.3)) {
    adapter.refs = Array.from({ length: r.chance(0.8) ? r.range(1, 2) : r.range(3, 8) }, () => ({
      type: r.pick(["jira.issue", adapter.type]), find: r.pick(["jira_key", "int", "hex", "uuid"]),
    }));
  }
  // A container (§8.3) from some of the ID and scope captures, sometimes
  // repeated so that only the container ID passes the 512-byte cap.
  if (r.chance(0.3)) {
    const usable = scoped ? [...captures, scoped] : captures;
    const parts = r.shuffle(usable).slice(0, r.range(0, usable.length)).map((c) => `{${c.name}}`);
    if (r.chance(0.2)) parts.push(...parts, ...parts);
    if (r.chance(0.2)) parts.unshift("x-y");
    adapter.within = { type: `c${index}.group`, id: [`${scheme}.c:{host}`, ...parts].join("/") };
  }
  return { json: adapter, rules, captures, scoped, titles, insensitive, modeAlts };
}

/** A rule binding every ID capture once, in the path or the query. */
function genRule(r, captures, scoped, modeAlts) {
  const bound = scoped ? [...captures, scoped] : [...captures];
  const inQuery = bound.filter(() => r.chance(0.2));
  const inPath = bound.filter((c) => !inQuery.includes(c));
  const segments = [];
  const literal = () => r.chance(0.2)
    ? { kind: "alt", alts: [...new Set([r.pick(PATH_WORDS), r.pick(PATH_WORDS)])] }
    : { kind: "lit", text: r.pick(PATH_WORDS) };
  segments.push(literal());
  for (const c of inPath) {
    if (r.chance(0.5)) segments.push(literal());
    if (r.chance(0.2)) segments.push({ kind: "anon" });
    segments.push({ kind: "cap", name: c.name, ctype: c.ctype });
  }
  const shuffled = r.chance(0.3) ? r.shuffle(segments) : segments;
  const optional = [];
  if (modeAlts && r.chance(0.8)) {
    const ctype = { alts: modeAlts, text: `(${modeAlts.join("|")})` };
    if (r.chance(0.7)) optional.push({ kind: "cap", name: "mode", ctype, optional: true });
    else shuffled.push({ kind: "cap", name: "mode", ctype });
  }
  const rest = r.chance(0.35);
  const text = (s) => {
    if (s.kind === "lit") return s.text;
    if (s.kind === "alt") return `(${s.alts.join("|")})`;
    if (s.kind === "anon") return "{_}";
    return `{${s.name}${s.optional ? "?" : ""}:${s.ctype.text}}`;
  };
  const parts = [...shuffled, ...optional].map(text);
  if (rest) parts.push("{*}");
  const json = { path: `/${parts.join("/")}` };
  const query = [];
  const keys = r.shuffle(QUERY_KEYS);
  for (const c of inQuery) query.push({ key: keys.pop(), kind: "cap", capture: c });
  if (r.chance(0.2)) query.push({ key: keys.pop(), kind: "lit", text: r.pick(QUERY_LITERALS) });
  if (r.chance(0.15)) query.push({ key: keys.pop(), kind: "present" });
  if (query.length > 0) {
    json.query = Object.fromEntries(query.map((q) => [q.key, q.kind === "cap" ? `{${q.capture.name}:${q.capture.ctype.text}}` : q.kind === "lit" ? q.text : "{_}"]));
  }
  const absent = r.chance(0.2) ? [keys.pop()] : [];
  if (absent.length > 0) json.absent = absent;
  if (r.chance(0.15)) json.mode = r.pick(["view", "edit", "review"]);
  return { json, required: shuffled, optional, rest, query, absent };
}

const PREFIX_DELIMS = ["] ", " - ", " · ", ": ", " | ", "#", " (", ") ", " — ", "😀 "];
const TITLE_LITERALS = [" - Jira", " - Google Docs", " · GitHub", "[", "(", " - Mail", "Tracker", " — ", "é"];

/** A title template, usually valid under §9.1. Captures reuse ID captures. */
function genTitleTemplate(r, captures) {
  const names = [];
  const cap = (c) => {
    names.push(c.name);
    const bounds = c.ctype.base !== "uuid" && r.chance(0.2) ? `(${c.ctype.min},${c.ctype.max})` : "";
    return `{${c.name}:${c.ctype.base}${bounds}}`;
  };
  const prefix = [];
  const suffix = [];
  if (r.chance(0.3)) prefix.push(r.pick(TITLE_LITERALS));
  if (r.chance(0.3) && captures.length > 0) prefix.push(r.chance(0.5) ? "[" : "", cap(r.pick(captures)), r.pick(PREFIX_DELIMS));
  if (r.chance(0.15)) prefix.push("({~}) ");
  // In Q every skip and capture follows a literal (§9.1).
  if (r.chance(0.3)) suffix.push(r.pick(PREFIX_DELIMS), "{~}");
  if (r.chance(0.3) && captures.length > 0) suffix.push(r.pick(PREFIX_DELIMS), cap(r.pick(captures)));
  if (r.chance(0.6)) suffix.push(r.pick(TITLE_LITERALS));
  const text = `${prefix.join("")}{title}${suffix.join("")}`;
  return { text, names };
}

/** 1–3 adapters, sometimes several on one origin so order matters. */
function genDocument(r) {
  const origins = [];
  const count = r.chance(0.6) ? 1 : r.range(2, 3);
  for (let i = 0; i < count; i += 1) origins.push(i > 0 && r.chance(0.6) ? origins[0] : r.pick(ORIGINS));
  const adapters = origins.map((origin, i) => genAdapter(r, i, origin));
  return { json: { format: 1, artifacts: adapters.map((a) => a.json) }, adapters };
}

// ---- Document mutations ------------------------------------------------------

const MUTATION_CHARS = ["{", "}", "(", ")", "|", ":", "?", "*", "_", "@", ",", "/", "%", " ", "A", "0", "-", ".", "~", "\u0000", "é"];

/** Changes one string, key, or value somewhere in the document. */
function mutateDocument(r, doc) {
  const strings = [];
  const visit = (value, set) => {
    if (typeof value === "string") strings.push(set);
    else if (Array.isArray(value)) value.forEach((v, i) => visit(v, (x) => { value[i] = x; }));
    else if (value && typeof value === "object") {
      for (const key of Object.keys(value)) visit(value[key], (x) => { value[key] = x; });
    }
  };
  visit(doc, null);
  const roll = r.next();
  if (roll < 0.7 && strings.length > 0) {
    // Re-walk to read the current string, then edit one character.
    let target = r.int(strings.length);
    const edit = (value, set) => {
      if (typeof value === "string") {
        if (target === 0) {
          // Edit by code point, so no edit leaves a lone surrogate.
          const chars = [...value];
          const at = r.int(chars.length + 1);
          const ch = r.pick(MUTATION_CHARS);
          chars.splice(at, r.int(2), ...(r.chance(0.66) ? [ch] : []));
          set(chars.join(""));
        }
        target -= 1;
      } else if (Array.isArray(value)) value.forEach((v, i) => edit(v, (x) => { value[i] = x; }));
      else if (value && typeof value === "object") {
        for (const key of Object.keys(value)) edit(value[key], (x) => { value[key] = x; });
      }
    };
    edit(doc, null);
  } else {
    const adapter = r.pick(doc.artifacts);
    const choice = r.int(7);
    if (choice === 0) delete adapter[r.pick(Object.keys(adapter))];
    else if (choice === 1) adapter[r.pick(["extra", "title", "scope", "unscoped", "alias", "modes"])] = r.pick([1, "x", [], {}, ["{title}"], { a: ["b"] }]);
    else if (choice === 2) adapter.match.push({ ...r.pick(adapter.match) });
    else if (choice === 3) adapter.match[0].query = { ...(adapter.match[0].query ?? {}), [r.pick(QUERY_KEYS)]: r.pick(["{_}", "{q:int}", "x", "{q?:int}", ""]) };
    else if (choice === 4) doc.format = r.pick([0, 2, "1", 1.5, null]);
    else if (choice === 5) adapter.title = Array.from({ length: r.range(0, 5) }, () => r.pick(["{title}", "{title} - X", "{~}{title}", "{n:int} {title}"]));
    else {
      // A list or title at or just past its limit (SQ-V25).
      const over = r.int(2);
      const which = r.int(3);
      if (which === 0) adapter.refs = Array.from({ length: 8 + over }, () => ({ type: "jira.issue", find: "jira_key" }));
      else if (which === 1) adapter.title = `{title}${"·".repeat(504 + over)}😀`;
      else adapter.scope = Object.fromEntries(Object.keys(adapter.scope ?? { org: [] }).map((k) => [k, Array.from({ length: 64 + over }, (_, i) => `v${i}`)]));
    }
  }
  return doc;
}

// ---- URLs --------------------------------------------------------------------

function encodeChar(r, ch) {
  const bytes = new TextEncoder().encode(ch);
  const hex = (b) => b.toString(16).padStart(2, "0");
  return [...bytes].map((b) => `%${r.chance(0.5) ? hex(b).toUpperCase() : hex(b)}`).join("");
}

/** Writes a decoded segment, percent-encoding some characters. */
function encodeSegment(r, text) {
  if (!r.chance(0.25)) return text;
  return [...text].map((ch) => (r.chance(0.3) ? encodeChar(r, ch) : ch)).join("");
}

function encodeQuery(r, text) {
  return [...text].map((ch) => {
    if (ch === " ") return r.pick(["+", "%20"]);
    if ("&=+#%".includes(ch)) return encodeChar(r, ch);
    return r.chance(0.2) ? encodeChar(r, ch) : ch;
  }).join("");
}

function flipCase(r, text) {
  return [...text].map((ch) => (r.chance(0.5) ? (ch === ch.toLowerCase() ? ch.toUpperCase() : ch.toLowerCase()) : ch)).join("");
}

/** A URL that the rule should match, and its bound values by name. */
function urlForRule(r, adapter, rule) {
  const values = new Map();
  // Sometimes every value is as long as its type allows.
  const long = r.chance(0.06);
  const bind = (name, ctype) => {
    // Scoped captures usually take a listed value, sometimes case-flipped.
    const scoped = adapter.scoped?.name === name && r.chance(0.8);
    const value = scoped ? flipCase(r, r.pick(adapter.scoped.values)) : genValue(r, ctype, long);
    values.set(name, value);
    if (ctype.tail && r.chance(0.6)) return `${r.pick(["Plan", "a-b", "x"])}-${value}`;
    return value;
  };
  const segmentFor = (s) => {
    if (s.kind === "lit") return adapter.insensitive && r.chance(0.5) ? flipCase(r, s.text) : s.text;
    if (s.kind === "alt") return r.pick(s.alts);
    if (s.kind === "anon") return r.string(TOKEN, r.range(1, 8));
    return bind(s.name, s.ctype);
  };
  const segments = rule.required.map(segmentFor);
  let optionalBound = 0;
  for (const s of rule.optional) {
    if (!r.chance(0.6)) break;
    segments.push(segmentFor(s));
    optionalBound += 1;
  }
  if (rule.rest && (optionalBound === rule.optional.length || r.chance(0.5))) {
    for (let i = r.range(0, 3); i > 0; i -= 1) segments.push(r.pick([...PATH_WORDS, "rest", "7"]));
  }
  const params = [];
  for (const q of rule.query) {
    if (q.kind === "cap") params.push([q.key, bind(q.capture.name, q.capture.ctype)]);
    else if (q.kind === "lit") params.push([q.key, q.text]);
    else params.push([q.key, r.string(TOKEN, r.range(1, 4))]);
  }
  if (r.chance(0.3)) params.push([r.pick(QUERY_KEYS), r.string(TOKEN, 3)]);
  return { segments, params: r.shuffle(params), values };
}

function serializeUrl(r, origin, segments, params) {
  let url = `${origin}/${segments.map((s) => encodeSegment(r, s)).join("/")}`;
  if (r.chance(0.1)) url += "/";
  if (params.length > 0) url += `?${params.map(([k, v]) => (v === null ? encodeQuery(r, k) : `${encodeQuery(r, k)}=${encodeQuery(r, v)}`)).join("&")}`;
  if (r.chance(0.1)) url += `#${r.pick(["x", "/a/b", "%2F"])}`;
  return url;
}

/** One near-miss edit to a URL's parts, or to the serialized URL. */
function nearMiss(r, url) {
  const parts = { ...url, segments: [...url.segments], params: [...url.params] };
  switch (r.int(11)) {
    case 0: // Add a segment.
      parts.segments.splice(r.int(parts.segments.length + 1), 0, r.pick([...PATH_WORDS, "7", "x"]));
      break;
    case 1: // Remove a segment.
      parts.segments.splice(r.int(parts.segments.length), 1);
      break;
    case 2: { // Change one character of a segment.
      const i = r.int(parts.segments.length);
      const s = parts.segments[i] ?? "";
      const at = r.int(s.length + 1);
      parts.segments[i] = s.slice(0, at) + r.pick(["a", "Z", "0", "-", "_", ".", "é", " ", "%"]) + s.slice(at + 1);
      break;
    }
    case 3: // An encoded slash, dot segment, control, or bad escape.
      parts.segments.splice(r.int(parts.segments.length + 1), 0, r.pick(["a%2Fb", "%2e", "%2E%2e", "a%00", "%ZZ", "%FF", "%C3%A9", "%e2%80%8b", ""]));
      break;
    case 4: // Flip case.
      parts.segments = parts.segments.map((s) => (r.chance(0.5) ? flipCase(r, s) : s));
      break;
    case 5: // Duplicate a query key.
      if (parts.params.length > 0) parts.params.push([r.pick(parts.params)[0], "dup"]);
      break;
    case 6: // Drop a query parameter.
      parts.params.splice(r.int(parts.params.length), 1);
      break;
    case 7: // A key the rule requires absent, present, or a bare key.
      parts.params.push([r.pick(QUERY_KEYS), r.chance(0.3) ? null : ""]);
      break;
    case 8: // Another origin: port, scheme, or host case.
      parts.origin = r.pick([`${url.origin}:1`, url.origin.replace("https:", "http:"), url.origin.toUpperCase(), `${url.origin}.`]);
      break;
    case 9: // Overlong segment values.
      parts.segments[r.int(parts.segments.length)] = r.string(DIGITS, r.pick([19, 20, 64, 65, 129, 257]));
      break;
    default: // Many segments.
      for (let i = r.pick([30, 62, 64]); i > 0; i -= 1) parts.segments.push("s");
  }
  return parts;
}

// ---- Titles ------------------------------------------------------------------

const TITLE_WORDS = ["Plan", "A - B", "PROJ-12", "UTF-8", "fix #42", "  spaced  ", "é", "😀", "[x]", "(3)", "0xff", "·", "á", " ", "﻿",
  // Every White_Space code point, which trimming removes, and lookalikes it keeps.
  "\t", "\n", "\u000b", "\f", "\r", "\u0085", "\u1680", "\u2000", "\u2001", "\u2002", "\u2003", "\u2004", "\u2005",
  "\u2006", "\u2007", "\u2008", "\u2009", "\u200a", "\u2028", "\u2029", "\u202f", "\u205f", "\u3000", "\u180e", "\u200b",
  "\u2060",
  // Enough refs of several types to pass the 20-ref cap.
  Array.from({ length: 12 }, (_, i) => `R-${i + 1} ${i + 1} ${(i + 10).toString(16)}f`).join(" "),
];

/** Fills a title template with the bound values, the title text, and skips. */
function titleFor(r, template, values, captures) {
  const byName = new Map(captures.map((c) => [c.name, c]));
  let out = "";
  let i = 0;
  const text = template.text;
  while (i < text.length) {
    if (text.startsWith("{title}", i)) {
      out += Array.from({ length: r.range(1, 4) }, () => r.pick(TITLE_WORDS)).join(r.pick([" ", " - ", ""]));
      i += 7;
    } else if (text.startsWith("{~}", i)) {
      out += r.pick(["3", "Team Space", "a - b", "", "Mail"]);
      i += 3;
    } else if (text[i] === "{") {
      const close = text.indexOf("}", i);
      const name = text.slice(i + 1, text.indexOf(":", i));
      const c = byName.get(name);
      const bound = values.get(name);
      if (c && bound !== undefined && r.chance(0.8)) out += r.chance(0.8) ? bound : canonical(c.ctype, bound);
      else out += c ? genValue(r, c.ctype) : "x";
      i = close + 1;
    } else {
      out += text[i];
      i += 1;
    }
  }
  return out;
}

function titleNearMiss(r, title) {
  switch (r.int(7)) {
    case 0: return `(${r.range(1, 99)}) ${title}`;
    case 1: return title.slice(0, Math.max(0, title.length - r.range(1, 3)));
    case 2: return `  ${title}\t`;
    case 3: return [...title].map((ch) => (r.chance(0.1) ? r.pick(["-", " ", "]", "·"]) : ch)).join("");
    case 4: return title + "x".repeat(r.pick([100, 480, 520]));
    case 5: return null; // Lone surrogates, as UTF-16 units.
    default: return "";
  }
}

/** The URL padded to `n` scalar values with a fragment, which never affects
 * matching, so only the 2048-scalar cap decides the result. */
function padUrl(url, n) {
  const base = url.includes("#") ? url : `${url}#`;
  const length = [...base].length;
  return length >= n ? base : base + "x".repeat(n - length);
}

function utf16(text) {
  return Array.from({ length: text.length }, (_, i) => text.charCodeAt(i));
}

// ---- Cases -------------------------------------------------------------------

/** One request, generated from (seed, index) alone. */
export function genCase(seed, index) {
  const r = generator(caseSeed(seed, index));
  const doc = genDocument(r);
  if (r.chance(0.12)) return { doc: mutateDocument(r, structuredClone(doc.json)), url: `${doc.adapters[0].json.origin}/a` };
  const adapter = r.pick(doc.adapters);
  const rule = r.pick(adapter.rules);
  let url = { origin: adapter.json.origin, ...urlForRule(r, adapter, rule) };
  if (r.chance(0.45)) url = nearMiss(r, url);
  const request = { doc: doc.json, url: serializeUrl(r, url.origin, url.segments, url.params) };
  if (r.chance(0.05)) request.url = r.pick(["not a url", "about:blank", "data:text/plain,a", "https://acme.example.com:99999/"]);
  if (r.chance(0.7)) {
    let title;
    if (adapter.titles && r.chance(0.85)) title = titleFor(r, r.pick(adapter.titles), url.values, adapter.captures);
    else title = Array.from({ length: r.range(0, 5) }, () => r.pick(TITLE_WORDS)).join(" ");
    if (r.chance(0.3)) {
      const missed = titleNearMiss(r, title);
      if (missed === null) {
        const units = utf16(title);
        units.splice(r.int(units.length + 1), 0, r.pick([0xd800, 0xdbff, 0xdc00, 0xdfff]));
        request.title = units;
      } else title = missed;
    }
    request.title ??= title;
  }
  if (r.chance(0.04)) request.url = padUrl(request.url, r.pick([2047, 2048, 2049]));
  if (r.chance(0.05)) request.url = utf16(request.url).concat([0xd83d]);
  // JSON strings cannot portably carry lone surrogates (serde_json rejects
  // them), so any text that is not well-formed goes as UTF-16 units.
  for (const key of ["url", "title"]) {
    if (typeof request[key] === "string" && !request[key].isWellFormed()) request[key] = utf16(request[key]);
  }
  return request;
}

// ---- Runner ------------------------------------------------------------------

function runEngine(command, args, lines) {
  // Node's readline also ends a line at U+2028 and U+2029, which JSON.stringify
  // leaves raw, so they go escaped; in JSON they can only occur inside strings.
  const input = lines.map((line) => line.replaceAll("\u2028", "\\u2028").replaceAll("\u2029", "\\u2029"));
  const result = spawnSync(command, args, {
    input: input.join("\n") + "\n", encoding: "utf8", maxBuffer: 1 << 30,
    // The TypeScript harness loads .js as an ES module; skip the warning.
    env: { ...process.env, NODE_NO_WARNINGS: "1" },
  });
  // An engine that stops early closes stdin, so EPIPE means "see stderr".
  if (result.error && result.error.code !== "EPIPE") throw result.error;
  if (result.status !== 0 || result.error) {
    throw new Error(`${path.basename(command)} ${args.join(" ")} failed:\n${result.stderr}`);
  }
  return result.stdout.trimEnd().split("\n").map((line) => JSON.parse(line));
}

function usage() {
  console.log("Usage: node scripts/adapter-templates-diff.mjs [--cases N] [--seed S] [--only INDEX] [--host PATH]");
  console.log(`Build first with:\n  ${BUILD}`);
}

async function main() {
  const { values: options } = parseArgs({
    options: {
      cases: { type: "string", default: "2000" }, seed: { type: "string", default: "1" },
      only: { type: "string" }, host: { type: "string", default: DEFAULT_HOST }, help: { type: "boolean" },
    },
  });
  if (options.help) return usage();
  const seed = Number(options.seed);
  const count = Number(options.cases);
  if (!Number.isSafeInteger(seed) || !Number.isSafeInteger(count) || count < 1) {
    usage();
    process.exit(2);
  }
  if (!existsSync(options.host) || !existsSync(path.join(REPO, "extension", "dist", "adapter-templates.js"))) {
    console.error(`Build both engines first:\n  ${BUILD}`);
    process.exit(2);
  }
  const indexes = options.only === undefined ? Array.from({ length: count }, (_, i) => i) : [Number(options.only)];
  const stats = { cases: 0, invalid: 0, match: 0, titled: 0, reasons: {} };
  const failures = [];
  for (let start = 0; start < indexes.length; start += BATCH) {
    const batch = indexes.slice(start, start + BATCH);
    const requests = batch.map((i) => genCase(seed, i));
    const lines = requests.map((request) => JSON.stringify(request));
    const rust = runEngine(options.host, ["adapters", "eval"], lines);
    const ts = runEngine(process.execPath, [NODE_HARNESS], lines);
    batch.forEach((index, k) => {
      const [a, b] = [rust[k], ts[k]];
      stats.cases += 1;
      if (a?.error) stats.invalid += 1;
      else if (a?.match) {
        stats.match += 1;
        if (a.title !== undefined) stats.titled += 1;
      } else if (a) stats.reasons[a.reason] = (stats.reasons[a.reason] ?? 0) + 1;
      if (options.only !== undefined) console.log(JSON.stringify({ request: requests[k], rust: a, ts: b }, null, 2));
      if (!isDeepStrictEqual(a, b)) failures.push({ index, request: lines[k], rust: a, ts: b });
    });
  }
  console.log(`adapter-template differential: seed ${seed}, ${stats.cases} cases, ${stats.match} matched`
    + ` (${stats.titled} with a title), ${stats.invalid} invalid documents, no match: ${JSON.stringify(stats.reasons)}`);
  for (const failure of failures.slice(0, SHOWN_FAILURES)) {
    console.error(`\nseed ${seed} case ${failure.index} differs\n  request ${failure.request}`
      + `\n  rust    ${JSON.stringify(failure.rust)}\n  ts      ${JSON.stringify(failure.ts)}`);
  }
  if (failures.length > 0) {
    console.error(`\n${failures.length} of ${stats.cases} cases differ; reproduce one with --seed ${seed} --only <case>`);
    process.exit(1);
  }
  // Guard the generator: a change that stopped producing matches or valid
  // documents would make every run pass while testing nothing.
  if (options.only === undefined && stats.cases >= 500
    && (stats.match < stats.cases * 0.15 || stats.invalid > stats.cases * 0.4)) {
    console.error("the generator produced too few matches or too many invalid documents to be a useful test");
    process.exit(1);
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) await main();
