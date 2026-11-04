## 2026-11-04 — Check Unicode wire boundaries

Audit bounded human-readable strings in `extension/protocol-shape.ts` against
their schema and host validators, and add shared non-BMP boundary fixtures.
The saved-command catalogue accepted 256 Unicode characters in Rust but
rejected descriptions over 256 UTF-16 units in the extension, disconnecting
the host for valid emoji-containing descriptions. This review fixes that
field; shared fixtures would catch the same mismatch in future wire fields.
