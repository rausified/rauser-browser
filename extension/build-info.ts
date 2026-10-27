// The header's build line. The build stamps the extension's own commit into
// the page (extension/build-id.mjs); the host reports its commit as semver
// build metadata in hello's host_version (host/build.rs).

export function hostCommit(hostVersion: string): string | null {
  const plus = hostVersion.indexOf("+");
  const build = plus < 0 ? "" : hostVersion.slice(plus + 1);
  return build === "" ? null : build;
}

function commitOf(build: string): string | null {
  const commit = build.replace(/-dirty$/, "");
  return commit === "unknown" || commit === "" ? null : commit;
}

// Uncommitted changes on either side are not a mismatch, and abbreviations
// of one commit may differ in length, so only a differing prefix is flagged.
export function hostBuildText(extensionBuild: string, hostVersion: string): string {
  const host = hostCommit(hostVersion);
  if (host === null) return "";
  const ours = commitOf(extensionBuild);
  const theirs = commitOf(host);
  const differs = ours !== null && theirs !== null && !ours.startsWith(theirs) && !theirs.startsWith(ours);
  return ` · host ${host}${differs ? " (not the extension's commit; rebuild both)" : ""}`;
}
