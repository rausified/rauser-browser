/**
 * Whether a newer extension build is on disk than the one Chrome is running.
 *
 * Chrome keeps an unpacked extension's manifest from when it loaded the
 * extension, so `chrome.runtime.getManifest()` names the running build, while
 * a fetch of the extension's own `manifest.json` reads the file on disk. A
 * deploy (`scripts/dev/deploy-live`) rebuilds `extension/dist`; until the
 * extension is reloaded, the service worker and any open panel keep the old
 * build, and a host started for them is refused as outdated. The build is the
 * manifest's `version_name`, which `extension/build.mjs` fills with the build
 * ID. A packaged install reads back the manifest it runs, so it never differs.
 */

export interface ManifestReader {
  loaded(): unknown;
  onDisk(): Promise<unknown>;
}

/** The manifest's `version_name`, trimmed, or null when absent or blank. */
export function manifestBuild(manifest: unknown): string | null {
  if (typeof manifest !== "object" || manifest === null) return null;
  const name = (manifest as { version_name?: unknown }).version_name;
  if (typeof name !== "string") return null;
  const trimmed = name.trim();
  return trimmed === "" ? null : trimmed;
}

/** The build on disk when it differs from the running one; null when they
 * agree or either cannot be read. */
export async function newerBuildOnDisk(reader: ManifestReader): Promise<string | null> {
  try {
    const running = manifestBuild(reader.loaded());
    if (running === null) return null;
    const disk = manifestBuild(await reader.onDisk());
    return disk !== null && disk !== running ? disk : null;
  } catch {
    return null;
  }
}

/** Reads the running manifest from Chrome and the one on disk with an
 * uncached fetch of the extension's own file. */
export function chromeManifests(): ManifestReader {
  return {
    loaded: () => chrome.runtime.getManifest(),
    onDisk: async () => {
      const response = await fetch(chrome.runtime.getURL("manifest.json"), { cache: "no-store" });
      return response.ok ? await response.json() : null;
    },
  };
}
