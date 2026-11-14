## 2026-11-03 — Settings gallery scope fields have no length cap

Capture Site's scope field sets `box.maxLength = input.maxLength` from the
scope-kind registry (`extension/starter-choice-view.ts`), but the settings
gallery's equivalent field (`renderScopeField` in `extension/options.ts`)
never applies `ScopeInput.maxLength`. Both fields exist for the same reason
and read the same `scopeInput()` registry entry, but only the panel enforces
the bound before the keystroke lands; the gallery relies on `parse()` to
refuse an overlong value only after it is typed. A boundary meant to keep
degenerate scope text out of `config.toml` (`maxlength` 1312 for GitHub
today) should hold at both entry points that write it, not just one.

Change: add `field.maxLength = input.maxLength;` to `renderScopeField` in
`extension/options.ts`, matching `starter-choice-view.ts`'s `box.maxLength`.
Where: `extension/options.ts`.

- Done 2026-11-03: 2355e26
- Review 2026-11-12: **Done.** Caps Settings scope input at the same length as Capture Site.
