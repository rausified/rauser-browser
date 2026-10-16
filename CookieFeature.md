# Cookie Feature Notes

## 2026-10-16 — Host-validated adapter configuration in protocol v5

Brauser's adapter-template engine is no longer an isolated library. Protocol v5 carries a required format-versioned adapter document, and the native host serializes it as `[adapters] format = 1` plus `[[artifacts]]` tables in `config.toml`. Every adapter is parsed by the existing Rust authority, must use an origin already enabled for capture, and inherits the engine's shared-origin scope protections. The host also reports conservative overlap pairs so settings can warn that an earlier rule wins.

The failure behavior is intentionally narrow and recoverable. A newer or malformed adapter format is preserved without rewriting the user's file and disables only automatic capture; page notes and AI settings continue to work. The side panel, settings page, and autonomous service-worker renewal all suspend their policy consistently, while the host independently suppresses a stale queued visit before vault I/O. Adapter additions, edits, and precedence-changing reorders enter the native confirmation flow, with a plain every-account warning and separate-work-profile recommendation for unscoped shared origins. Existing adapter ID templates are conservatively frozen until alias migrations exist, so a config edit cannot silently orphan future artifact record keys.

This is the safe configuration foundation, not a claim that artifact logging is complete. The confirmed templates still need to travel in the worker lease; the TypeScript engine must prefilter navigation before buffering, and the host must re-evaluate every delivered event before writing it.

## 2026-10-16 — Portable vault and harness verification repair

The full Rust suite exposed two assumptions that made otherwise valid builds look catastrophically broken outside the macOS CI environment. On Linux, `cap-std` can hold a directory capability as an `O_PATH` descriptor, which supports confined relative lookups but cannot itself be synced. Brauser now opens the capability's own `.` entry as a readable directory before `fsync`, preserving the same confinement while restoring the durability sequence used by page notes and visit logs.

The harness process fixtures also depended on the shell's umask: a cooperative `0002` umask made fake install directories group-writable, so the host correctly rejected the fixtures before the tests reached their intended assertions. Those fixtures now set private permissions explicitly, including the separate symlink target directory, and the work-directory check chooses BSD or GNU `stat` syntax by platform. This does not expand Brauser's supported runtime platforms; it makes the repository's security and filesystem verification reproducible and leaves the deliberate unsafe-install rejection coverage intact.
