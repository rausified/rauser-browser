## 2026-11-02 — Name the working folder before running a task

Show the configured knowledge-base folder beside the task scope in the panel
(`brwr-5xpb`), using host-projected configuration as text. The no-page task work
(`brwr-qin`) makes the instruction independent of a tab, but its context line
still says only “Runs in your knowledge base”; a person choosing among multiple
note/project folders cannot see the actual working location before Run. Extend
`extension/panel.ts`'s task context and its built panel tests, retaining setup
hints and leaving the separate per-task chooser (`brwr-zla`) for later.
- Review 2026-11-12: **Keep.** Names the actual folder before Run so users know where the agent will work.
