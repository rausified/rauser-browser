# Plans and specs

`specs/` holds design specs (`YYYY-MM-DD-<topic>-design.md`); `plans/` holds implementation plans (`YYYY-MM-DD-<topic>.md`) written for agentic workers. A plan starts with the **Goal**, **Architecture**, **Tech Stack** and **Sources** paragraphs, then the two sections below, then its tasks.

## Design challenge

Required for a plan of more than five tasks, and for any security boundary, data migration, or parser/escaping design (AGENTS.md "Before building"). Before task 1, one reviewer spends about five minutes arguing for a simpler or safer alternative to the plan's central mechanism. Record:

- **Challenger:** who argued (session or person).
- **Alternative:** the simpler or safer design they proposed.
- **Outcome:** adopted, or kept the plan, and why in one or two sentences.

Example: the 2026-10-27 Markdown sanitizer first matched Markdown parser behaviour; context-free escaping replaced it after three review rounds. A challenge before task 1 would have proposed escaping first.

## Open decisions

Every user-facing or policy decision the plan assumes, one line each:

- **Decision:** the question. **Status:** resolved (by whom, when) or open. **Default if unresolved:** what the plan does.

The coordinator resolves the open ones with John before task 1; no task that depends on an open decision starts.
