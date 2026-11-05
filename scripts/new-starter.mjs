#!/usr/bin/env node

// Starter family scaffolder (docs/superpowers/specs/2026-11-03-starter-families-design.md
// §4 "Scaffolder"): lays out the files a new starter type needs — a family
// file (or a new type appended to an existing one) and its doc page — with
// deliberately invalid placeholders, then runs scripts/generate-starters.mjs
// in --check mode so its one-pass output names everything still left to
// fill in. protocol/starters/README.md documents the family file shape this
// writes; AGENTS.md "Adding a starter" is the short end-to-end recipe.
//
// Usage: node scripts/new-starter.mjs <family> <type>
//          [--exact <origin> | --subdomain <suffix>]
//          [--scope github_org|tenant_name|unscoped]
//          [--root <dir>]
//
//   <family>  lowercase letters, digits and hyphens: ^[a-z][a-z0-9-]*$
//   <type>    two dot-separated lowercase+digit parts: ^[a-z][a-z0-9]*\.[a-z][a-z0-9]*$
//
// A new family needs exactly one of --exact or --subdomain; an existing
// family (its JSON file already under protocol/starters/) takes neither —
// it already has an origin — and the type must not already exist anywhere
// in the registry. --scope unscoped needs --exact: the engine only allows
// the unscoped marker on one of its hard-coded exact shared origins.
//
// Exits 0 once it has written the family file and the doc page (even though
// the generator it then runs will almost always still refuse them: that is
// the point — its printed problems are the scaffolder's "fill these in"
// list). Exits 2, writing nothing, when the request itself is invalid.

import { spawnSync } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, readdirSync, realpathSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

import { isCanonicalHttpsOrigin, isValidSubdomainSuffix } from "./generate-starters.mjs";

const REPO = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const STARTERS_DIR = "protocol/starters";
const DOCS_ADAPTERS_DIR = "docs/adapters";
const GENERATOR = path.join(path.dirname(fileURLToPath(import.meta.url)), "generate-starters.mjs");

const USAGE = `Usage: node scripts/new-starter.mjs <family> <type>
  [--exact <origin> | --subdomain <suffix>] [--scope github_org|tenant_name|unscoped] [--root <dir>]`;

const FAMILY_RE = /^[a-z][a-z0-9-]*$/;
const TYPE_RE = /^[a-z][a-z0-9]*\.[a-z][a-z0-9]*$/;
const SCOPE_KINDS = ["github_org", "tenant_name", "unscoped"];

const DOC_HEADINGS = [
  "What each rule covers",
  "Titles",
  "Recorded samples",
  "What the draft does not record",
  "Known limits",
  "Live check still needed",
];

function isPlainObject(value) {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** Refuses the request (exit 2) before anything is written. */
class Refusal extends Error {}

function refuse(message) {
  throw new Refusal(message);
}

/** Every type name already declared by any family file under `root`'s
 * protocol/starters/, read directly rather than through
 * generate-starters.mjs's strict loader: a scaffolder deciding whether a
 * name collides should not itself refuse to run just because some other,
 * unrelated family file currently fails validation (for example, one an
 * earlier, still-unfinished `new-starter` run left mid-placeholder). */
function knownTypes(root) {
  const dir = path.join(root, STARTERS_DIR);
  const known = new Set();
  for (const name of readdirSync(dir)) {
    if (!name.endsWith(".json") || name === "ids.lock.json") continue;
    let data;
    try {
      data = JSON.parse(readFileSync(path.join(dir, name), "utf8"));
    } catch {
      continue;
    }
    if (!isPlainObject(data) || !Array.isArray(data.types)) continue;
    for (const type of data.types) {
      if (isPlainObject(type) && typeof type.type === "string") known.add(type.type);
    }
  }
  return known;
}

/** The family JSON file at `<root>/protocol/starters/<family>.json`, or
 * `null` when the family does not exist yet. */
function readFamily(root, family) {
  const familyPath = path.join(root, STARTERS_DIR, `${family}.json`);
  if (!existsSync(familyPath)) return null;
  return JSON.parse(readFileSync(familyPath, "utf8"));
}

/** The default offer `key` for a brand-new type (builder's call 8: new
 * types default to `<type>:{origin}` on a subdomain family, so every
 * tenant's offer is distinct, or the bare `<type>` on an exact-origin
 * family, which already names one site). */
function defaultKey(type, originKind) {
  return originKind === "subdomain" ? `${type}:{origin}` : type;
}

/** The adapter-level markers a scoped or unscoped family's type needs
 * (protocol/starters/README.md "Scope kinds"): `{"<key>": {"$scope":
 * true}}` for `github_org`/`tenant_name` — a real scoped adapter's `scope`
 * is a capture-keyed table, not the bare marker (final review, Minor 2) —
 * or `unscoped: "acknowledged"` for `unscoped`. A `scope: null` family's
 * type gets neither. */
function scopeMarkers(scopeKind, key) {
  if (scopeKind === "github_org" || scopeKind === "tenant_name") return { scope: { [key]: { "$scope": true } } };
  if (scopeKind === "unscoped") return { unscoped: "acknowledged" };
  return {};
}

/** The capture key a new scoped type's `adapter.scope` marker should use
 * (Minor 2): the key an existing type in this family already marks (adding
 * a type to, say, GitHub reuses `owner`); otherwise, for a brand-new
 * `tenant_name` family on a catalog shared origin, that origin's own
 * `tenant.capture` (the key `host/src/capture_site.rs`'s `taken_types`
 * reads); otherwise `"github_org"`'s one real family's key, `"owner"`; and
 * failing all of that, `"TODO"`, since nothing here can safely guess at a
 * brand-new tenant-scoped origin's capture name. */
function scopeMarkerKey(root, existingFamily, scopeKind, origin) {
  if (existingFamily !== null) {
    for (const type of existingFamily.types ?? []) {
      const scope = type?.adapter?.scope;
      if (isPlainObject(scope)) {
        const keys = Object.keys(scope);
        if (keys.length === 1) return keys[0];
      }
    }
  }
  if (scopeKind === "tenant_name" && origin.kind === "exact") {
    try {
      const catalog = JSON.parse(readFileSync(path.join(root, "protocol", "capture-site", "catalog.json"), "utf8"));
      const capture = catalog?.shared_origins?.[origin.origin]?.tenant?.capture;
      if (typeof capture === "string") return capture;
    } catch {
      // Falls through to "TODO"; the generator's gap 1/7 checks still
      // refuse a tenant_name family that isn't on a real shared origin.
    }
  }
  if (scopeKind === "github_org") return "owner";
  return "TODO";
}

/** A brand-new type object, every free string a literal `"TODO"`
 * placeholder (spec §4 "Scaffolder"), `match: []` so it fails closed until
 * a real rule is written, and one placeholder recorded sample so
 * `samples_shape: true`'s "must not be empty" check does not itself hide
 * the sample fields that still need filling in. */
function newType(type, originKind, scopeKind, scopeKey) {
  return {
    type,
    key: defaultKey(type, originKind),
    label: "TODO",
    label_variants: [],
    description: "TODO",
    record_label: "TODO",
    example_path: null,
    adapter: { type, match: [], ...scopeMarkers(scopeKind, scopeKey), id: "TODO" },
    history: [],
    status: { state: "development", method: null, checked_on: null, checked_by: null },
    docs: `${DOCS_ADAPTERS_DIR}/${type}.md`,
    samples: [{ url: "TODO", title: "TODO", shape: "TODO", note: "TODO" }],
    samples_shape: true,
    vectors: [],
  };
}

/** A brand-new family object holding exactly `type`. `review_prompt` only
 * needs a "TODO" placeholder when the family has a scope kind
 * (protocol/starters/README.md: it may be `null` for a one-type, unscoped
 * family, which a fresh scaffold always is); `sample_scope` is a one-entry
 * placeholder list for a scoped family, else empty, matching
 * `validateSampleScope`'s "non-empty exactly when scoped" rule. */
function newFamily(family, type, origin, scopeKind) {
  const scope = scopeKind === undefined ? null : scopeField(scopeKind);
  return {
    family,
    display_name: "TODO",
    review_prompt: scope === null ? null : "TODO",
    origin,
    scope,
    sample_origin: "TODO",
    sample_scope: scope === null ? [] : ["TODO"],
    overlaps: [],
    types: [type],
  };
}

function scopeField(scopeKind) {
  if (scopeKind === "github_org") return { kind: "github_org" };
  if (scopeKind === "tenant_name") return { kind: "tenant_name", max: 4, label: "TODO", placeholder: "TODO" };
  return { kind: "unscoped", notice: "TODO" };
}

/** `docs/adapters/<type>.md`'s starting content: the title, the `Status:`
 * line a fresh "development" type always opens with (so it matches
 * checkRegistry's check 3 unedited), and the six section headings spec §4
 * names (`docs/adapters/confluence.page.md:38-156`), each a placeholder the
 * author fills in once they have recorded real samples. */
function docText(type) {
  const sections = DOC_HEADINGS.map((heading) => `## ${heading}\n\nTODO\n`).join("\n");
  return `# ${type} starter draft\n\nStatus: development draft.\n\n${sections}`;
}

function createBeadCommand(type) {
  return (
    `scripts/dev/create-bead "Starter: ${type}" -d "Draft and certify the ${type} starter adapter." ` +
    `--acceptance "${type} is release-certified against the live tool." -t task -p 2`
  );
}

/** Prints the generator's `--check` output (Task 6 review):
 *  - Minor 1: a crash (`result.error`) or an exit code other than 0/1 is
 *    reported, not silently swallowed into an empty list; exit 0 says so
 *    plainly instead of leaving the "Fill these in" header dangling over
 *    nothing.
 *  - Minor 2: the lines naming `targets` (the family file and doc page this
 *    run just wrote or extended) print first, ahead of every other family's
 *    unrelated, already-present problems, so the one list this run cares
 *    about isn't buried in a repo-wide one. */
function printGeneratorOutput(result, targets) {
  if (result.error) {
    console.error(`could not run the generator: ${result.error.message}`);
  } else if (result.status === 0) {
    console.log("Nothing left for npm run gen:starters --check to flag; run npm run gen:starters.");
  } else if (result.status === 1) {
    const lines = result.stdout.split("\n").filter((line) => line.trim() !== "");
    const isTarget = (line) => targets.some((target) => line.startsWith(`${target}:`));
    const targetLines = lines.filter(isTarget);
    const otherLines = lines.filter((line) => !isTarget(line));
    console.log("Fill these in, then run npm run gen:starters:");
    for (const line of targetLines) console.log(`  ${line}`);
    if (otherLines.length > 0) {
      console.log("  (already-present problems elsewhere in the registry, unrelated to this change):");
      for (const line of otherLines) console.log(`    ${line}`);
    }
  } else {
    console.error(`the generator exited ${result.status} unexpectedly`);
  }
  if (typeof result.stderr === "string" && result.stderr.trim() !== "") console.error(result.stderr.trim());
}

function main(argv) {
  const { values, positionals } = parseArgs({
    args: argv,
    allowPositionals: true,
    options: {
      exact: { type: "string" },
      subdomain: { type: "string" },
      scope: { type: "string" },
      root: { type: "string" },
      help: { type: "boolean", default: false },
    },
  });
  if (values.help) {
    console.log(USAGE);
    return 0;
  }
  const root = values.root ? path.resolve(values.root) : REPO;

  try {
    const [family, type] = positionals;
    if (!family || !type || positionals.length > 2) refuse(USAGE);
    if (!FAMILY_RE.test(family)) refuse(`family "${family}" must match ${FAMILY_RE} (lowercase letters, digits and hyphens)`);
    if (!TYPE_RE.test(type)) {
      refuse(`type "${type}" must match ${TYPE_RE} (two dot-separated parts, lowercase letters and digits, no underscore)`);
    }
    if (values.scope !== undefined && !SCOPE_KINDS.includes(values.scope)) {
      refuse(`--scope must be one of ${SCOPE_KINDS.join(", ")}, not "${values.scope}"`);
    }
    if (values.exact !== undefined && values.subdomain !== undefined) {
      refuse("pass at most one of --exact or --subdomain");
    }
    if (values.exact !== undefined && !isCanonicalHttpsOrigin(values.exact)) {
      refuse(`--exact "${values.exact}" must be a canonical https origin (no path, query or trailing slash)`);
    }
    if (values.subdomain !== undefined && !isValidSubdomainSuffix(values.subdomain)) {
      refuse(`--subdomain "${values.subdomain}" must start with "." and have at least two labels (e.g. ".example.com")`);
    }
    if (knownTypes(root).has(type)) refuse(`type "${type}" already exists`);
    const docPath = path.join(root, DOCS_ADAPTERS_DIR, `${type}.md`);
    const docRelPath = path.posix.join(DOCS_ADAPTERS_DIR, `${type}.md`);
    if (existsSync(docPath)) {
      // An orphan doc, a hand-started one, or the doc of a type whose
      // family file doesn't currently parse (so knownTypes above missed
      // it): never overwritten (Task 6 review, Important 1).
      refuse(`${docRelPath} already exists`);
    }

    const existingFamily = readFamily(root, family);
    let origin;
    let scopeKind;
    if (existingFamily === null) {
      if (values.exact === undefined && values.subdomain === undefined) {
        refuse(`a new family needs exactly one of --exact or --subdomain`);
      }
      scopeKind = values.scope;
      if (scopeKind === "unscoped" && values.exact === undefined) {
        refuse(`scope "unscoped" needs --exact, not --subdomain`);
      }
      origin = values.exact !== undefined
        ? { kind: "exact", origin: values.exact }
        : { kind: "subdomain", suffix: values.subdomain, labels: 1, exclude: ["www"] };
    } else {
      if (values.exact !== undefined || values.subdomain !== undefined || values.scope !== undefined) {
        // The family's origin and scope are already set; --scope is as
        // meaningless here as --exact/--subdomain (Task 6 review, Minor 6).
        refuse(`family "${family}" already exists; --exact/--subdomain/--scope only apply to a new family`);
      }
      origin = existingFamily.origin;
      scopeKind = isPlainObject(existingFamily.scope) ? existingFamily.scope.kind : undefined;
    }

    const scopeKey = scopeMarkerKey(root, existingFamily, scopeKind, origin);
    const type_ = newType(type, origin.kind, scopeKind, scopeKey);
    const familyPath = path.join(root, STARTERS_DIR, `${family}.json`);
    const familyJson = existingFamily === null
      ? newFamily(family, type_, origin, scopeKind)
      : { ...existingFamily, types: [...existingFamily.types, type_] };
    writeFileSync(familyPath, `${JSON.stringify(familyJson, null, 2)}\n`);

    mkdirSync(path.dirname(docPath), { recursive: true });
    writeFileSync(docPath, docText(type));

    const familyRelPath = path.posix.join(STARTERS_DIR, `${family}.json`);
    console.log(`wrote ${familyRelPath}`);
    console.log(`wrote ${docRelPath}`);
    console.log("");

    const result = spawnSync(process.execPath, [GENERATOR, "--check", "--root", root], { encoding: "utf8" });
    printGeneratorOutput(result, [familyRelPath, docRelPath]);

    // check 4 (extension/tests/starter-registry.test.mjs: a positive and a
    // negative evaluate vector per match rule, and a titled sample) needs
    // the built engine, so --check above can never see it (Task 6 review,
    // Important 2). This block is fixed, not computed from the still-empty
    // `match: []`, since the real rule count only exists once the author
    // has filled it in.
    console.log("");
    console.log("Also fill in (check 4, extension/tests/starter-registry.test.mjs, not covered by --check above):");
    console.log("  - vectors: at least one positive evaluate vector per adapter.match rule, and at least one negative evaluate vector for the type (a sibling type's positive on a shared origin does not count)");
    console.log("  - samples: a sample with a non-null title, if the adapter uses a title template");

    console.log("");
    console.log("Then file its bead:");
    console.log(`  ${createBeadCommand(type)}`);
    return 0;
  } catch (error) {
    if (error instanceof Refusal) {
      console.error(`REFUSED: ${error.message}`);
      return 2;
    }
    throw error;
  }
}

// Compare real paths: argv[1] may go through a symlink or contain characters
// a file URL escapes (spaces, Windows drive letters).
function isEntryPoint() {
  try {
    return import.meta.url === pathToFileURL(realpathSync(process.argv[1] ?? "")).href;
  } catch {
    return false;
  }
}

if (isEntryPoint()) {
  try {
    process.exit(main(process.argv.slice(2)));
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
