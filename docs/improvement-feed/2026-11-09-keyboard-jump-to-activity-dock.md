## 2026-11-09 — Keyboard jump to activity dock

Add a keyboard shortcut or focusable activity control that moves focus to the newest nonempty dock row, and returns focus to the prior control when its details close. The new bottom dock keeps run details out of the page layout, but a keyboard user can otherwise need to tab through the page before reaching a run's Cancel or result actions. Implement this in `extension/panel.ts` and cover focus return in the panel keyboard tests.
