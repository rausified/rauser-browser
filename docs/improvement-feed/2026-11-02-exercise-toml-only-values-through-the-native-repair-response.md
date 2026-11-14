## 2026-11-02 — Exercise TOML-only values through the native repair response

Extend the native adapter-repair fixtures with a small TOML value corpus,
including positive/negative infinity and nested unknown fields. Independent
review reproduced a repair workspace that could not open when an unknown
artifact field held NaN: copying from the JSON snapshot lost its type. The
config-level regression now covers that case and dates; checking the same
values through `get_adapter_repair` in `host/src/native.rs` would protect the
complete transport path as it evolves.
- Review 2026-11-12: **Keep.** Tests NaN, infinity, and nested unknown values through native repair so config can be opened without losing types.
