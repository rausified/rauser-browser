// Throwaway: runs vectors.json through the extension's TS engine
// (extension/dist/adapter-templates.js, built by `npm run build:extension`).
// Usage: node run-ts.mjs <doc.json> <vectors.json>
import { readFileSync } from "node:fs";
const { compileDocument, evaluate } = await import(new URL("../../../../extension/dist/adapter-templates.js", import.meta.url));
const doc = JSON.parse(readFileSync(process.argv[2], "utf8"));
const vectors = JSON.parse(readFileSync(process.argv[3], "utf8"));
const compiled = compileDocument(doc);
if (!compiled.ok) { console.log("VALIDATION ERROR", JSON.stringify(compiled.error)); process.exit(1); }
console.log("document validates");
let fail = 0, pass = 0;
for (const v of vectors.positive) {
  const r = evaluate(compiled.doc, v.url, v.title);
  const e = v.expect;
  const problems = [];
  if (!r.match) problems.push(`no match (${r.reason})`);
  else {
    if (r.type !== e.type) problems.push(`type ${r.type}`);
    if (r.artifact_id !== e.artifact_id) problems.push(`id ${r.artifact_id}`);
    if ((r.mode ?? null) !== e.mode) problems.push(`mode ${r.mode}`);
    if ("title_entry" in e && (r.title_entry ?? null) !== e.title_entry) problems.push(`title_entry ${r.title_entry}`);
    if ("title" in e && (r.title ?? null) !== e.title) problems.push(`title ${JSON.stringify(r.title)}`);
  }
  if (problems.length) { fail++; console.log("FAIL", v.name, problems.join("; "), JSON.stringify(r)); } else pass++;
}
for (const v of vectors.negative) {
  const r = evaluate(compiled.doc, v.url);
  if (r.match) { fail++; console.log("FAIL negative matched", v.name, JSON.stringify(r)); } else pass++;
}
console.log(`ts engine: ${pass} passed, ${fail} failed`);
process.exit(fail ? 1 : 0);
