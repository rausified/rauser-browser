#!/usr/bin/env node
// Throwaway validator for the Vidcast blind starter (not product code).
// Parses adapters.toml (Python tomllib, so no npm TOML dependency), maps the
// §3 shape ([adapters] format + [[artifacts]]) to the engine's document shape
// ({format, artifacts}), compiles it with the extension's TypeScript engine
// (extension/dist/adapter-templates.js) and checks every case in vectors.json.
// With --emit-requests it instead prints JSON Lines for `brauser adapters eval`
// / scripts/adapter-templates-eval.mjs.
//
// Usage (repo root, Node 24):
//   node research/blind/vidcast/run-vectors.mjs
//   node research/blind/vidcast/run-vectors.mjs --emit-requests > /tmp/req.jsonl
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const REPO = path.resolve(HERE, "../../..");
const raw = JSON.parse(execFileSync("python3", ["-I", "-c",
  "import json,sys,tomllib;print(json.dumps(tomllib.load(open(sys.argv[1],'rb'))))",
  path.join(HERE, "adapters.toml")], { encoding: "utf8" }));
const doc = { format: raw.adapters.format, artifacts: raw.artifacts };
const vectors = JSON.parse(readFileSync(path.join(HERE, "vectors.json"), "utf8"));

if (process.argv.includes("--emit-requests")) {
  for (const c of vectors.cases) {
    const req = { doc, url: c.url };
    if (c.title !== undefined) req.title = c.title;
    process.stdout.write(JSON.stringify(req) + "\n");
  }
  process.exit(0);
}

const { compileDocument, evaluate } = await import(path.join(REPO, "extension/dist/adapter-templates.js"));
const compiled = compileDocument(doc);
if (!compiled.ok) {
  console.error("VALIDATION FAILED", JSON.stringify(compiled.error));
  process.exit(1);
}
console.log(`document valid: ${doc.artifacts.length} adapters`);

let failures = 0;
for (const c of vectors.cases) {
  const r = evaluate(compiled.doc, c.url, c.title);
  const e = c.expect;
  const problems = [];
  if (Boolean(r.match) !== e.match) problems.push(`match ${r.match} != ${e.match}`);
  if (e.match && r.match) {
    if (r.type !== e.type) problems.push(`type ${r.type} != ${e.type}`);
    if (r.artifact_id !== e.artifact_id) problems.push(`id ${r.artifact_id} != ${e.artifact_id}`);
    if ((r.mode ?? null) !== e.mode) problems.push(`mode ${r.mode} != ${e.mode}`);
    if (e.title !== undefined) {
      // The engine reports the extracted title when an entry matches; when
      // none matches Brauser records the whole tab title.
      const got = r.title ?? c.title;
      if (got !== e.title) problems.push(`title ${JSON.stringify(got)} != ${JSON.stringify(e.title)}`);
    }
  }
  if (problems.length) {
    failures += 1;
    console.log(`FAIL ${c.id}: ${problems.join("; ")}  result=${JSON.stringify(r)}`);
  }
}
console.log(`${vectors.cases.length - failures}/${vectors.cases.length} cases pass`);
process.exit(failures ? 1 : 0);
