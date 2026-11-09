## 2026-11-09 — Exercise scoped modes in the save model

Change: add scoped mode captures with different alternations per rule to the
generated adapter-save scenarios, including safe disjoint suffix additions.

Why: independent review reproduced a valid save rejected with `scope_bad_value`
because its suffix was compiled without the prefix that supported a scope
value. The exact-path model did not exercise this whole-adapter validation
dependency. The review fix uses complete adapters with indexed rule comparison;
the regression now covers a real revision-checked save.

Where: the generated save model and scoped-mode regression in
`host/src/config.rs`; the existing mode/scope fixtures in
`adapter-templates/src/document.rs` show the supported grammar.
