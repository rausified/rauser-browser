## 2026-11-03 — Exercise config repair for host-owned fields

Extend the invalid-config cases in `host/src/config.rs` to cover typed but
invalid host-owned fields, checking that refresh returns a repairable revision
and repair backs up the exact original bytes. The task-folder review found
that a validation error escaped as an I/O failure, preventing the native picker
from repairing the configuration. The new task-folder regression covers that
case; a shared matrix for future fields would catch the same boundary mistake.
