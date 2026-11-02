import assert from "node:assert/strict";
import test from "node:test";

const { manifestBuild, newerBuildOnDisk } = await import("../dist/build-update.js");

const reader = (loaded, onDisk) => ({ loaded: () => loaded, onDisk: async () => onDisk() });

test("the build is the manifest's version_name, trimmed, or nothing", () => {
  assert.equal(manifestBuild({ version: "0.1.0", version_name: " 0.1.0 (029a398fa684) " }), "0.1.0 (029a398fa684)");
  for (const manifest of [null, undefined, "x", {}, { version_name: "" }, { version_name: "  " }, { version_name: 7 }]) {
    assert.equal(manifestBuild(manifest), null, JSON.stringify(manifest));
  }
});

test("a different build on disk than the one Chrome loaded is newer", async () => {
  const loaded = { version_name: "0.1.0 (33a108070e87)" };
  assert.equal(await newerBuildOnDisk(reader(loaded, () => ({ version_name: "0.1.0 (029a398fa684)" }))), "0.1.0 (029a398fa684)");
  assert.equal(await newerBuildOnDisk(reader(loaded, () => ({ version_name: "0.1.0 (33a108070e87)" }))), null);
});

test("a missing, unreadable or unnamed build on either side is never reported", async () => {
  const named = { version_name: "0.1.0 (33a108070e87)" };
  assert.equal(await newerBuildOnDisk(reader({}, () => named)), null);
  assert.equal(await newerBuildOnDisk(reader(named, () => ({}))), null);
  assert.equal(await newerBuildOnDisk(reader(named, () => { throw new Error("fetch failed"); })), null);
  assert.equal(await newerBuildOnDisk({ loaded: () => { throw new Error("no runtime"); }, onDisk: async () => named }), null);
});
