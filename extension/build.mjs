import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync } from "node:fs";
import { copyFile, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { APP_NAME, FULL_NAME } from "./brand.ts";
import { fillPlaceholders, gitBuildId } from "./build-id.mjs";

const extensionDir = dirname(fileURLToPath(import.meta.url));
const repoDir = dirname(extensionDir);
const outputDir = join(extensionDir, "dist");
const intermediateDir = join(extensionDir, ".build");
// The bundled Readability is pinned to this exact file; bump both together.
const READABILITY_VERSION = "0.6.0";
const READABILITY_SHA256 = "34dcab3d0832d0019f02990eed6b6124e029e8c32b9f0c6f2550544ff8dff174";
const readabilityDir = join(repoDir, "node_modules", "@mozilla", "readability");

if (!existsSync(readabilityDir)) throw new Error("Run npm ci first: @mozilla/readability is missing");
const readability = await readFile(join(readabilityDir, "Readability.js"));
const readabilityHash = createHash("sha256").update(readability).digest("hex");
if (readabilityHash !== READABILITY_SHA256) {
  throw new Error(
    `node_modules/@mozilla/readability/Readability.js is not the pinned ${READABILITY_VERSION} ` +
    `(sha256 ${readabilityHash}, expected ${READABILITY_SHA256}); run npm ci`,
  );
}

await rm(outputDir, { recursive: true, force: true });
await rm(intermediateDir, { recursive: true, force: true });
const compiler = join(repoDir, "node_modules", "typescript", "bin", "tsc");
const result = spawnSync(process.execPath, [compiler, "-p", join(extensionDir, "tsconfig.json")], {
  stdio: "inherit",
});
if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);

await mkdir(outputDir, { recursive: true });
const placeholders = { APP_NAME, FULL_NAME, BUILD_ID: gitBuildId(repoDir) };
for (const name of ["manifest.json", "options.html", "panel.html"]) {
  const source = await readFile(join(extensionDir, name), "utf8");
  await writeFile(join(outputDir, name), fillPlaceholders(source, placeholders, name));
}
await copyFile(join(extensionDir, "panel.css"), join(outputDir, "panel.css"));
await mkdir(join(outputDir, "vendor"));
await writeFile(join(outputDir, "vendor", "readability.js"), readability);
await copyFile(join(readabilityDir, "LICENSE.md"), join(outputDir, "vendor", "readability.LICENSE.md"));
for (const name of [
  "capture-site.js", "capture-site-panel.js", "activity.js", "adapter-review.js", "adopted-state-settings.js", "artifact-refresh.js", "adapter-templates.js", "agent.js", "brand.js", "build-info.js", "coordination.js", "extract.js",
  "model.js", "native.js", "note-editor.js", "note-recovery.js", "options.js", "panel.js", "protocol-shape.js",
  "run-card.js", "settings.js", "site-patterns.js", "site-pattern-panel.js", "site-pattern-settings.js", "starter-adapters.js", "summarize.js", "summary-text.js", "worker.js",
]) {
  await copyFile(join(intermediateDir, "extension", name), join(outputDir, name));
}
await rm(intermediateDir, { recursive: true, force: true });
console.log(`Built unpacked Chrome extension ${placeholders.BUILD_ID} at ${outputDir}`);
