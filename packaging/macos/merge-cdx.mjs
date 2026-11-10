#!/usr/bin/env node
// Merge per-target CycloneDX SBOMs of one crate into one SBOM for the
// universal binary: components are united by bom-ref, and each component's
// dependsOn list is the union across targets. The first file supplies the
// metadata; every file must describe the same root component.
//
//   node packaging/macos/merge-cdx.mjs <a.cdx.json> <b.cdx.json>... > merged.cdx.json
import { readFileSync } from "node:fs";

const files = process.argv.slice(2);
if (files.length === 0) {
  console.error("usage: merge-cdx.mjs <bom.json>...");
  process.exit(2);
}
const boms = files.map((file) => JSON.parse(readFileSync(file, "utf8")));
const [base] = boms;
const rootRef = base.metadata?.component?.["bom-ref"];
for (const [index, bom] of boms.entries()) {
  if (bom.bomFormat !== "CycloneDX" || bom.specVersion !== base.specVersion) {
    throw new Error(`${files[index]} is not a CycloneDX ${base.specVersion} SBOM`);
  }
  if (bom.metadata?.component?.["bom-ref"] !== rootRef) {
    throw new Error(`${files[index]} describes another root component`);
  }
}

const components = new Map();
const dependencies = new Map();
for (const bom of boms) {
  for (const component of bom.components ?? []) {
    const ref = component["bom-ref"];
    const known = components.get(ref);
    if (known && JSON.stringify(known) !== JSON.stringify(component)) {
      throw new Error(`component ${ref} differs between targets`);
    }
    components.set(ref, component);
  }
  for (const { ref, dependsOn = [] } of bom.dependencies ?? []) {
    const set = dependencies.get(ref) ?? new Set();
    for (const dep of dependsOn) set.add(dep);
    dependencies.set(ref, set);
  }
}

const merged = {
  ...base,
  components: [...components.values()],
  dependencies: [...dependencies].map(([ref, set]) => ({ ref, dependsOn: [...set].sort() })),
};
process.stdout.write(`${JSON.stringify(merged, null, 2)}\n`);
