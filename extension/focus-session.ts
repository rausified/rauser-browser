// A user-chosen label for measured activity, never capture authority.
import type { FocusSession } from "../protocol/ts/generated.js";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

export function focusName(value: unknown): string {
  if (typeof value !== "string" || !value.trim() || [...value.trim()].length > 120 ||
      /\p{Cc}|\p{Cf}|\p{Cs}|\u2028|\u2029/u.test(value)) {
    throw new Error("Use a session name of 1–120 characters without line breaks or controls.");
  }
  return value.trim();
}

export function isFocusSession(value: unknown): value is FocusSession {
  if (typeof value !== "object" || value === null) return false;
  const session = value as Record<string, unknown>;
  try {
    return Object.keys(session).length === 3 && typeof session.id === "string" && UUID.test(session.id) &&
      focusName(session.name) === session.name && typeof session.revision === "number" &&
      Number.isInteger(session.revision) && session.revision >= 1 && session.revision <= 0xffff_ffff;
  } catch { return false; }
}

export function sameFocus(left: FocusSession | null | undefined, right: FocusSession | null | undefined): boolean {
  return left == null ? right == null : right != null && left.id === right.id &&
    left.revision === right.revision && left.name === right.name;
}

export function nextFocus(
  current: FocusSession | null, expected: FocusSession | null, name: string | null, uuid: () => string,
): FocusSession | null {
  if ((expected !== null && !isFocusSession(expected)) || !sameFocus(current, expected)) {
    throw new Error("The focus session changed in another panel. Review it and try again.");
  }
  if (name === null) return null;
  const cleaned = focusName(name);
  if (current?.name === cleaned) return current;
  if (current?.revision === 0xffff_ffff) throw new Error("End this session before starting another name.");
  return { id: current?.id ?? uuid(), name: cleaned, revision: (current?.revision ?? 0) + 1 };
}
