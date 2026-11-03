// Throwaway: validates doc.json with the extension's TS engine (compiled from
// extension/adapter-templates.ts into .build/) and checks every vector.
import { readFileSync } from "node:fs";
const here = new URL(".", import.meta.url);
const { compileDocument, evaluate } = await import(new URL(".build/adapter-templates.js", here));
const doc = JSON.parse(readFileSync(new URL("doc.json", here)));
const vectors = JSON.parse(readFileSync(new URL("vectors.json", here)));
const compiled = compileDocument(doc);
if (!compiled.ok) { console.log("VALIDATION ERROR", JSON.stringify(compiled.error)); process.exit(1); }
console.log(`validated: ${doc.artifacts.length} adapters`);
let fail = 0;
for (const c of vectors.cases) {
  const r = evaluate(compiled.doc, c.url, c.title);
  const e = c.expect; const bad = [];
  if (r.match !== e.match) bad.push(`match ${r.match} (${r.reason ?? r.type})`);
  else if (e.match) {
    if (r.type !== e.type) bad.push(`type ${r.type}`);
    if (r.artifact_id !== e.artifact_id) bad.push(`id ${r.artifact_id}`);
    if ((r.mode ?? null) !== e.mode) bad.push(`mode ${r.mode}`);
    if ("title" in e && r.title !== e.title) bad.push(`title ${JSON.stringify(r.title)} entry ${r.title_entry}`);
  }
  if (bad.length) { fail++; console.log(`FAIL ${c.name}: ${bad.join("; ")}`); }
  if (process.argv.includes("-v")) console.log(c.name, JSON.stringify(r));
}
console.log(`${vectors.cases.length - fail}/${vectors.cases.length} vectors pass`);
process.exit(fail ? 1 : 0);
