// Saved instructions remain literal when brought back from service history.
// A leading slash is escaped so Reuse/recall cannot invoke today's registry.
export function literalTaskInstruction(instruction: string): string {
  return instruction.trimStart().startsWith("/") ? `/${instruction.trimStart()}` : instruction;
}
