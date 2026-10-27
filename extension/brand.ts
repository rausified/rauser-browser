// Product naming for the extension and dev scripts. The host's copy is
// host/src/brand.rs; NATIVE_HOST_NAME must stay in step with installers.
// Keep this file free of imports and non-erasable TypeScript so Node can load it directly.
export const APP_NAME = "Brauser";
export const FULL_NAME = "Rauser Browser Assistant";
export const NATIVE_HOST_NAME = "com.rauser.brauser";
export const BINARY_NAME = "brauser";
// launchd label of the per-user service; host/src/brand.rs has the same value.
export const SERVICE_LABEL = "com.rauser.brauser.service";

export function storageKey(name: string): string {
  return `${BINARY_NAME}_${name}`;
}
