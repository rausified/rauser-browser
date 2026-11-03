// Throwaway harness: validates adapters.toml (as JSON) with the repo's
// TypeScript engine and runs vectors.json through evaluate().
import { readFileSync } from "node:fs";
import { compileDocument } from "../../../extension/adapter-templates.ts";

const [docPath, vecPath] = process.argv.slice(2);
const doc = JSON.parse(readFileSync(docPath, "utf8"));
const compiled = compileDocument(doc);
if (!compiled.ok) { console.log("VALIDATION ERROR", JSON.stringify(compiled.error)); process.exit(1); }
console.log(`validated: ${doc.artifacts.length} adapters, ${doc.artifacts.reduce((n, a) => n + a.match.length, 0)} rules`);
const { cases } = JSON.parse(readFileSync(vecPath, "utf8"));
let fail = 0;
for (const c of cases) {
  let d = compiled.doc;
  if (c.origin_override) {
    const o = compileDocument({ ...doc, artifacts: doc.artifacts.map((a) => ({ ...a, origin: c.origin_override })) });
    d = o.doc;
  }
  const r = c.title === undefined ? evaluateCase(d, c.url) : evaluateCase(d, c.url, c.title);
  const e = c.expect, bad = [];
  if (r.match !== e.match) bad.push(`match=${r.match} (${r.reason ?? ""})`);
  for (const k of ["type", "rule", "artifact_id"]) if (k in e && r[k] !== e[k]) bad.push(`${k}=${r[k]}`);
  if ("title" in e && (r.title ?? null) !== e.title) bad.push(`title=${JSON.stringify(r.title)}`);
  if ("title_entry" in e && r.title_entry !== e.title_entry) bad.push(`title_entry=${r.title_entry}`);
  if (bad.length) { fail++; console.log("FAIL", c.id, bad.join(" "), JSON.stringify(r)); }
}
console.log(`${cases.length - fail}/${cases.length} vectors pass`);
process.exit(fail ? 1 : 0);
function evaluateCase(d, url, title) { return evaluateFn(d, url, title); }
import { evaluate as evaluateFn } from "../../../extension/adapter-templates.ts";
