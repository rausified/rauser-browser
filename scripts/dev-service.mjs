// Development LaunchAgent for the Brauser service. Release builds register the
// service from the signed app bundle; this module only serves
// scripts/register-dev-host.mjs.
import { randomUUID } from "node:crypto";
import { chmod, mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import path from "node:path";
import { APP_NAME } from "../extension/brand.ts";

const DEV_MARKER = `<!-- ${APP_NAME} development service -->`;

function escapeXml(value) {
  return String(value)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll("\"", "&quot;")
    .replaceAll("'", "&apos;");
}

function requireAbsolute(name, value) {
  if (typeof value !== "string" || !path.isAbsolute(value)) {
    throw new Error(`${name} must be an absolute path`);
  }
}

function requireText(name, value) {
  if (typeof value !== "string" || value === "") {
    throw new Error(`${name} must be a non-empty string`);
  }
}

export function logPath(home) {
  requireAbsolute("home", home);
  return path.join(home, "Library", "Logs", APP_NAME, "service.log");
}

export function plistPath(home, label) {
  requireAbsolute("home", home);
  requireText("label", label);
  if (label.includes("/") || label.startsWith(".")) throw new Error("label must be a plain name");
  return path.join(home, "Library", "LaunchAgents", `${label}.plist`);
}

export function isDevPlist(text) {
  return typeof text === "string" && text.includes(DEV_MARKER);
}

export function servicePlist({ label, binary, home, user }) {
  requireText("label", label);
  requireText("user", user);
  requireAbsolute("binary", binary);
  requireAbsolute("home", home);
  const log = logPath(home);
  const searchPath = ["/opt/homebrew/bin", "/usr/local/bin", path.join(home, ".local", "bin"), "/usr/bin", "/bin", "/usr/sbin", "/sbin"].join(":");
  const string = (value) => `<string>${escapeXml(value)}</string>`;
  const key = (name) => `<key>${escapeXml(name)}</key>`;
  return [
    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
    "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">",
    DEV_MARKER,
    "<plist version=\"1.0\">",
    "<dict>",
    `  ${key("Label")}`,
    `  ${string(label)}`,
    `  ${key("ProgramArguments")}`,
    "  <array>",
    `    ${string(binary)}`,
    `    ${string("service")}`,
    "  </array>",
    `  ${key("EnvironmentVariables")}`,
    "  <dict>",
    `    ${key("HOME")}`,
    `    ${string(home)}`,
    `    ${key("USER")}`,
    `    ${string(user)}`,
    `    ${key("LANG")}`,
    `    ${string("en_US.UTF-8")}`,
    `    ${key("PATH")}`,
    `    ${string(searchPath)}`,
    "  </dict>",
    `  ${key("RunAtLoad")}`,
    "  <false/>",
    `  ${key("ProcessType")}`,
    `  ${string("Standard")}`,
    `  ${key("StandardOutPath")}`,
    `  ${string(log)}`,
    `  ${key("StandardErrorPath")}`,
    `  ${string(log)}`,
    "</dict>",
    "</plist>",
    "",
  ].join("\n");
}

async function readExisting(file) {
  try {
    return await readFile(file, "utf8");
  } catch (error) {
    if (error.code === "ENOENT") return null;
    throw error;
  }
}

async function refuseForeign(file) {
  const existing = await readExisting(file);
  if (existing !== null && !isDevPlist(existing)) {
    throw new Error(`refusing to replace a non-development service plist at ${file}`);
  }
  return existing;
}

function bootout(run, uid, label) {
  // Failure is expected when the service is not loaded.
  run(["bootout", `gui/${uid}/${label}`]);
}

// Writes the dev plist, then always boots out any loaded copy before
// bootstrapping, so re-running after a rebuild replaces the running service.
export async function installService({ label, binary, home, user, uid, run }) {
  const file = plistPath(home, label);
  const xml = servicePlist({ label, binary, home, user });
  await refuseForeign(file);
  await mkdir(path.dirname(logPath(home)), { recursive: true });
  const directory = path.dirname(file);
  await mkdir(directory, { recursive: true });
  const temporaryPath = path.join(directory, `.${label}.${randomUUID()}.tmp`);
  const temporary = await open(temporaryPath, "wx", 0o644);
  try {
    await temporary.writeFile(xml);
    await temporary.sync();
  } finally {
    await temporary.close();
  }
  try {
    await chmod(temporaryPath, 0o644);
    await rename(temporaryPath, file);
  } catch (error) {
    await unlink(temporaryPath).catch(() => {});
    throw error;
  }
  bootout(run, uid, label);
  const result = run(["bootstrap", `gui/${uid}`, file]);
  if (result.status !== 0) {
    throw new Error(`could not bootstrap ${label}: ${result.stderr || result.stdout || `exit ${result.status}`}`);
  }
  return file;
}

export async function uninstallService({ label, home, uid, run }) {
  const file = plistPath(home, label);
  const existing = await refuseForeign(file);
  bootout(run, uid, label);
  if (existing !== null) await unlink(file);
  return file;
}
