## 2026-11-02 — Include platform smoke expectations in UI wording reviews

Change: add an explicit check of `scripts/smoke-macos.mjs` when reviewing changes
to panel or run-card text. Why: PR #242 updated the summary unit tests but left
the macOS smoke assertion expecting the old labels; the built panel reproduced
the mismatch on Linux even though the macOS runner itself cannot run there.
Where: the UI change checklist in `TESTING.md` and the platform smoke assertions.
