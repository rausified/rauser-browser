#!/usr/bin/env node

// The Node twin of `brauser adapters eval` (host/src/adapters.rs): evaluates
// adapter templates with the extension's TypeScript engine, reading JSON Lines
// requests on stdin and printing one result per line in the same shape, so the
// differential tests (adapter-templates.md §12) can compare the two engines
// line by line. Run `npm run build:extension` first; this loads
// extension/dist/adapter-templates.js.
//
// A request is {"doc" | "adapter", "url", "title"?}. `url` and `title` are
// strings or arrays of UTF-16 code units (which may hold lone surrogates);
// each prints the shared vectors' `evaluate` result, or {"error": {...}} when
// the document fails validation. Importing this file does not read stdin.

import { createInterface } from "node:readline";
import { fileURLToPath } from "node:url";

const { compileDocument, evaluate } = await import("../extension/dist/adapter-templates.js");

const REQUEST_KEYS = new Set(["doc", "adapter", "url", "title"]);

function text(value, key) {
  if (typeof value === "string") return value;
  if (Array.isArray(value) && value.every((unit) => Number.isInteger(unit) && unit >= 0 && unit <= 0xffff)) {
    return String.fromCharCode(...value);
  }
  throw new Error(`${key} must be a string or an array of UTF-16 code units`);
}

/** Evaluates one parsed request; throws on a malformed request. */
export function evalRequest(request) {
  if (!request || typeof request !== "object" || Array.isArray(request)) throw new Error("request must be an object");
  for (const key of Object.keys(request)) if (!REQUEST_KEYS.has(key)) throw new Error(`unknown request key ${key}`);
  const hasDoc = request.doc !== undefined && request.doc !== null;
  const hasAdapter = request.adapter !== undefined && request.adapter !== null;
  if (hasDoc === hasAdapter) throw new Error("give exactly one of doc and adapter");
  const doc = hasDoc ? request.doc : { format: 1, artifacts: [request.adapter] };
  const url = text(request.url, "url");
  const title = request.title === undefined || request.title === null ? undefined : text(request.title, "title");
  const compiled = compileDocument(doc);
  if (!compiled.ok) return { error: compiled.error };
  return evaluate(compiled.doc, url, title);
}

/** Evaluates every line of `input`, writing one JSON line per request. */
export async function evalLines(input, output) {
  let number = 0;
  for await (const line of createInterface({ input, crlfDelay: Infinity })) {
    number += 1;
    if (line.trim() === "") continue;
    let result;
    try {
      result = evalRequest(JSON.parse(line));
    } catch (error) {
      throw new Error(`request on line ${number}: ${error.message}`, { cause: error });
    }
    if (!output.write(`${JSON.stringify(result)}\n`)) await new Promise((resolve) => output.once("drain", resolve));
  }
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  if (process.argv.length > 2) {
    console.error("Usage: node scripts/adapter-templates-eval.mjs < requests.jsonl");
    process.exit(2);
  }
  try {
    await evalLines(process.stdin, process.stdout);
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
