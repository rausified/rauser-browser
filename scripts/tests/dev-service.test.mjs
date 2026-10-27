import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { chmod, mkdir, mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import { SERVICE_LABEL } from "../../extension/brand.ts";
import { isDevPlist, plistPath, servicePlist } from "../dev-service.mjs";
import { main } from "../register-dev-host.mjs";

const EXTENSION_ID = "a".repeat(32);

async function world(t) {
  const home = await mkdtemp(path.join(os.tmpdir(), "bds-"));
  t.after(() => rm(home, { recursive: true, force: true }));
  const binary = path.join(home, "brauser");
  await writeFile(binary, "#!/bin/sh\n");
  await chmod(binary, 0o755);
  const calls = [];
  const run = (argv) => {
    calls.push(argv);
    return argv[0] === "bootout" ? { status: 3, stdout: "", stderr: "Boot-out failed: 3: No such process" } : { status: 0, stdout: "", stderr: "" };
  };
  const deps = { run, home, uid: 501, user: "u", platform: "darwin", log: () => {} };
  return { home, binary, calls, deps };
}

test("plist runs the binary's service subcommand on demand", () => {
  const xml = servicePlist({ label: SERVICE_LABEL, binary: "/a/brauser", home: "/Users/u", user: "u" });
  assert.ok(xml.includes("<!-- Brauser development service -->"));
  assert.match(xml, /<string>\/a\/brauser<\/string>\s*<string>service<\/string>/);
  assert.match(xml, /<key>Label<\/key>\s*<string>com\.rauser\.brauser\.service<\/string>/);
  assert.match(xml, /\/opt\/homebrew\/bin:\/usr\/local\/bin:\/Users\/u\/.local\/bin:\/usr\/bin:\/bin:\/usr\/sbin:\/sbin/);
  assert.match(xml, /<key>HOME<\/key>\s*<string>\/Users\/u<\/string>/);
  assert.match(xml, /<key>USER<\/key>\s*<string>u<\/string>/);
  assert.match(xml, /<key>LANG<\/key>\s*<string>en_US\.UTF-8<\/string>/);
  assert.match(xml, /<key>RunAtLoad<\/key>\s*<false\/>/);
  assert.match(xml, /<key>ProcessType<\/key>\s*<string>Standard<\/string>/);
  assert.match(xml, /<key>StandardOutPath<\/key>\s*<string>\/Users\/u\/Library\/Logs\/Brauser\/service\.log<\/string>/);
  assert.match(xml, /<key>StandardErrorPath<\/key>\s*<string>\/Users\/u\/Library\/Logs\/Brauser\/service\.log<\/string>/);
  assert.doesNotMatch(xml, /KeepAlive/);
  assert.ok(isDevPlist(xml));
  assert.ok(!isDevPlist("<plist></plist>"));
  assert.equal(plistPath("/Users/u", SERVICE_LABEL), "/Users/u/Library/LaunchAgents/com.rauser.brauser.service.plist");
});

test("paths are escaped and must be absolute", () => {
  const xml = servicePlist({ label: "a&b", binary: "/x/<y>&\"z'", home: "/Users/a&b", user: "<u>" });
  assert.ok(xml.includes("<string>/x/&lt;y&gt;&amp;&quot;z&apos;</string>"));
  assert.ok(xml.includes("<string>a&amp;b</string>"));
  assert.ok(xml.includes("<string>&lt;u&gt;</string>"));
  assert.ok(xml.includes("/Users/a&amp;b/.local/bin"));
  assert.ok(!xml.includes("<u>"));
  assert.throws(() => servicePlist({ label: SERVICE_LABEL, binary: "brauser", home: "/Users/u", user: "u" }), /absolute/);
  assert.throws(() => servicePlist({ label: SERVICE_LABEL, binary: "/a/brauser", home: "Users/u", user: "u" }), /absolute/);
  assert.throws(() => plistPath("relative", SERVICE_LABEL), /absolute/);
});

test("plist lints", { skip: process.platform !== "darwin" }, async (t) => {
  const directory = await mkdtemp(path.join(os.tmpdir(), "bdl-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  const file = path.join(directory, "service.plist");
  await writeFile(file, servicePlist({ label: SERVICE_LABEL, binary: "/a/b & c", home: "/Users/u", user: "u" }));
  const result = spawnSync("/usr/bin/plutil", ["-lint", file], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stdout + result.stderr);
});

test("registration always boots out before bootstrapping", async (t) => {
  const { home, binary, calls, deps } = await world(t);
  await main([EXTENSION_ID, binary], deps);
  const plist = plistPath(home, SERVICE_LABEL);
  assert.deepEqual(calls, [
    ["bootout", `gui/501/${SERVICE_LABEL}`],
    ["bootstrap", "gui/501", plist],
  ]);
  const xml = await readFile(plist, "utf8");
  assert.ok(isDevPlist(xml));
  assert.ok(xml.includes(`<string>${binary}</string>`));
  assert.equal((await stat(plist)).mode & 0o777, 0o644);
  assert.ok((await stat(path.join(home, "Library", "Logs", "Brauser"))).isDirectory());
  const manifest = path.join(home, "Library", "Application Support", "Google", "Chrome", "NativeMessagingHosts", "com.rauser.brauser.json");
  assert.equal(JSON.parse(await readFile(manifest, "utf8")).path, binary);

  // Re-running after a rebuild replaces the dev plist and the running service.
  await main([EXTENSION_ID, binary], deps);
  assert.deepEqual(calls.slice(2).map((argv) => argv[0]), ["bootout", "bootstrap"]);
});

test("a failed bootstrap is an error", async (t) => {
  const { binary, deps } = await world(t);
  const run = (argv) => argv[0] === "bootstrap" ? { status: 5, stdout: "", stderr: "Bootstrap failed: 5" } : { status: 0, stdout: "", stderr: "" };
  await assert.rejects(main([EXTENSION_ID, binary], { ...deps, run }), /Bootstrap failed: 5/);
});

test("a non-development plist is never replaced or booted out", async (t) => {
  const { home, binary, calls, deps } = await world(t);
  const plist = plistPath(home, SERVICE_LABEL);
  await mkdir(path.dirname(plist), { recursive: true });
  await writeFile(plist, "<plist>release</plist>");
  await assert.rejects(main([EXTENSION_ID, binary], deps), /non-development service plist/);
  await assert.rejects(main(["--unregister-service"], deps), /non-development service plist/);
  assert.deepEqual(calls, []);
  assert.equal(await readFile(plist, "utf8"), "<plist>release</plist>");
});

test("--no-service skips the service steps", async (t) => {
  const { home, binary, calls, deps } = await world(t);
  await main([EXTENSION_ID, binary, "--no-service"], deps);
  assert.deepEqual(calls, []);
  await assert.rejects(stat(plistPath(home, SERVICE_LABEL)), { code: "ENOENT" });
});

test("--unregister-service boots out and removes the dev plist", async (t) => {
  const { home, binary, calls, deps } = await world(t);
  await main([EXTENSION_ID, binary], deps);
  calls.length = 0;
  await main(["--unregister-service"], deps);
  assert.deepEqual(calls, [["bootout", `gui/501/${SERVICE_LABEL}`]]);
  await assert.rejects(stat(plistPath(home, SERVICE_LABEL)), { code: "ENOENT" });
  // Unregistering again is harmless.
  await main(["--unregister-service"], deps);
  await assert.rejects(main(["--unregister-service", EXTENSION_ID], deps), /Usage/);
});

test("arguments are validated", async (t) => {
  const { binary, deps } = await world(t);
  await assert.rejects(main([], deps), /Usage/);
  await assert.rejects(main([EXTENSION_ID], deps), /Usage/);
  await assert.rejects(main([EXTENSION_ID, binary, "extra"], deps), /Usage/);
  await assert.rejects(main([EXTENSION_ID, binary, "--bogus"], deps), /Usage/);
  await assert.rejects(main([EXTENSION_ID, "brauser"], deps), /absolute/);
  await assert.rejects(main(["not-an-id", binary], deps), /extension ID/);
  await assert.rejects(main(["--unregister-service"], { ...deps, platform: "win32" }), /macOS/);
});
