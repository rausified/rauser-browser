## 2026-11-09 — Prove Windows re-key directory durability

Implement and verify a supported Windows directory-publication mechanism before
enabling durable Re-learn apply there. Restoring brwr-gpnm found that
`host/src/vault.rs::sync_directory` and `host/src/config.rs::sync_parent` are
intentional Windows no-ops, insufficient for prepare/history/config ordering.
The private transaction now refuses the unsupported path rather than claiming
it is durable. Track this in brwr-4qt4, cover publication and pending recovery
on Windows CI, and document the real failure model in TESTING.md. Keep the
ordinary-save helpers unchanged until their own contract requires more.
- Review 2026-11-12: **Keep.** Establish durable directory publication before allowing Windows Re-learn apply; current Windows sync is intentionally a no-op.
