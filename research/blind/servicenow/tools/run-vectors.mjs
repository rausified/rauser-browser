// Throwaway: evaluate vectors.json against adapters.toml (converted to JSON by
// toml2json.py) with the extension's TypeScript engine. Read-only; no product
// code edited. Usage: node run-vectors.mjs <doc.json> <vectors.json>
import { readFileSync } from "node:fs";
const engine = await import(new URL("../../../../extension/dist/adapter-templates.js", import.meta.url));
const [docPath, vecPath] = process.argv.slice(2);
const compiled = engine.compileDocument(JSON.parse(readFileSync(docPath, "utf8")));
if (!compiled.ok) { console.log("VALIDATION ERROR", JSON.stringify(compiled.error)); process.exit(1); }
console.log(`document valid: ${compiled.doc.adapters.length} adapters`);
const vec = JSON.parse(readFileSync(vecPath, "utf8"));
let fail = 0;
for (const v of vec.positives) {
  const r = engine.evaluate(compiled.doc, v.url, v.title);
  const okId = r.match && r.type === v.type && r.artifact_id === v.id;
  const gotTitle = r.match ? (r.title ?? null) : null;
  const okTitle = v.title === undefined || gotTitle === v.expected_title;
  if (!okId || !okTitle) { fail++; console.log("FAIL+", v.url, JSON.stringify(r)); }
}
for (const v of vec.negatives) {
  const r = engine.evaluate(compiled.doc, v.url);
  if (r.match) { fail++; console.log("FAIL-", v.url, v.note, JSON.stringify(r)); }
}
console.log(`positives ${vec.positives.length}, negatives ${vec.negatives.length}, failures ${fail}`);
process.exit(fail ? 1 : 0);
