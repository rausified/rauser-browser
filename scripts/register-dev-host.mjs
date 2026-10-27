#!/usr/bin/env node

// Local development registration only. Release registration belongs to the
// signed installers; this command never runs during npm install or build.
import { spawnSync } from "node:child_process";
import { randomUUID } from "node:crypto";
import { constants } from "node:fs";
import { access, mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { APP_NAME, NATIVE_HOST_NAME, SERVICE_LABEL } from "../extension/brand.ts";
import { installService, uninstallService } from "./dev-service.mjs";

const HOST_NAME = NATIVE_HOST_NAME;
const DESCRIPTION = `${APP_NAME} development native host`;
const EXTENSION_ID = /^[a-p]{32}$/;

function usage() {
  throw new Error([
    "Usage: node scripts/register-dev-host.mjs <Chrome extension ID> <absolute host binary path> [--no-service]",
    "       node scripts/register-dev-host.mjs --unregister-service",
  ].join("\n"));
}

function launchctl(argv) {
  return spawnSync("/bin/launchctl", argv, { encoding: "utf8" });
}

function parse(argv) {
  const flags = new Set();
  const positionals = [];
  for (const argument of argv) {
    if (argument === "--no-service" || argument === "--unregister-service") {
      if (flags.has(argument)) usage();
      flags.add(argument);
    } else if (argument.startsWith("--")) {
      usage();
    } else {
      positionals.push(argument);
    }
  }
  if (flags.has("--unregister-service")) {
    if (flags.size !== 1 || positionals.length !== 0) usage();
    return { unregister: true };
  }
  if (positionals.length !== 2) usage();
  const [extensionId, binaryArgument] = positionals;
  return { unregister: false, service: !flags.has("--no-service"), extensionId, binaryArgument };
}

// `deps` exists for tests: they inject a fake launchctl runner and a temp home.
export async function main(argv, deps = {}) {
  const {
    run = launchctl,
    home = os.homedir(),
    uid = process.getuid?.(),
    user = os.userInfo().username,
    platform = process.platform,
    env = process.env,
    log = console.log,
  } = deps;
  const options = parse(argv);

  if (options.unregister) {
    if (platform !== "darwin") throw new Error("the development service is supported on macOS only");
    const file = await uninstallService({ label: SERVICE_LABEL, home, uid, run });
    log(`Unregistered ${SERVICE_LABEL}`);
    log(`Plist: ${file}`);
    return;
  }

  const { extensionId, binaryArgument } = options;
  if (!EXTENSION_ID.test(extensionId)) {
    throw new Error("Chrome extension ID must be 32 lowercase letters from a to p");
  }
  if (!path.isAbsolute(binaryArgument)) {
    throw new Error("host binary path must be absolute");
  }
  if (platform !== "darwin" && platform !== "win32") {
    throw new Error("development registration supports macOS and Windows only");
  }

  const binary = path.resolve(binaryArgument);
  await access(binary, platform === "darwin" ? constants.X_OK : constants.F_OK);

  const directory = platform === "darwin"
    ? path.join(home, "Library", "Application Support", "Google", "Chrome", "NativeMessagingHosts")
    : path.join(env.APPDATA ?? "", APP_NAME);
  if (!path.isAbsolute(directory)) {
    throw new Error("cannot determine the per-user application data directory");
  }
  const manifestPath = path.join(directory, `${HOST_NAME}.json`);
  const manifest = {
    name: HOST_NAME,
    description: DESCRIPTION,
    path: binary,
    type: "stdio",
    allowed_origins: [`chrome-extension://${extensionId}/`],
  };

  await mkdir(directory, { recursive: true });
  try {
    const previous = JSON.parse(await readFile(manifestPath, "utf8"));
    if (previous.description !== DESCRIPTION) {
      throw new Error(`refusing to replace a non-development host manifest at ${manifestPath}`);
    }
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }

  if (platform === "win32") {
    const registryKey = `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${HOST_NAME}`;
    const existing = spawnSync("reg.exe", ["query", registryKey, "/ve"], {
      encoding: "utf8",
      windowsHide: true,
    });
    if (existing.status === 0 && !existing.stdout.toLowerCase().includes(manifestPath.toLowerCase())) {
      throw new Error(`refusing to replace an existing Chrome host registration at ${registryKey}`);
    }
  }

  const temporaryPath = path.join(directory, `.${HOST_NAME}.${randomUUID()}.tmp`);
  const temporary = await open(temporaryPath, "wx", 0o600);
  try {
    await temporary.writeFile(`${JSON.stringify(manifest, null, 2)}\n`);
    await temporary.sync();
  } finally {
    await temporary.close();
  }
  try {
    await rename(temporaryPath, manifestPath);
  } catch (error) {
    await unlink(temporaryPath);
    throw error;
  }

  if (platform === "win32") {
    const registryKey = `HKCU\\Software\\Google\\Chrome\\NativeMessagingHosts\\${HOST_NAME}`;
    const result = spawnSync("reg.exe", ["add", registryKey, "/ve", "/t", "REG_SZ", "/d", manifestPath, "/f"], {
      encoding: "utf8",
      windowsHide: true,
    });
    if (result.status !== 0) {
      throw new Error(`could not register the Chrome user-level host: ${result.stderr || result.stdout}`);
    }
  }

  log(`Registered ${HOST_NAME} for Chrome extension ${extensionId}`);
  log(`Manifest: ${manifestPath}`);

  if (platform === "darwin" && options.service) {
    // Always boots out first, so re-running after a rebuild replaces the
    // running service with the new binary.
    const plist = await installService({ label: SERVICE_LABEL, binary, home, user, uid, run });
    log(`Registered ${SERVICE_LABEL} as a per-user LaunchAgent`);
    log(`Plist: ${plist}`);
  }
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main(process.argv.slice(2));
}
