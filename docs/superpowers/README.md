# Plans and specs

`specs/` holds design specs (`YYYY-MM-DD-<topic>-design.md`); `plans/` holds implementation plans (`YYYY-MM-DD-<topic>.md`) written for agentic workers. A plan starts with the **Goal**, **Architecture**, **Tech Stack** and **Sources** paragraphs, then the sections below, then its tasks.

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

## Guard invariant

Required for a plan that changes a record-ownership or history guard in `host/src/config.rs` (AGENTS.md "Before building"). Fill it in before task 1, and have the test green before the first review:

- **Invariant:** one sentence the guard protects, for example "an accepted save never changes which type wins an already-recorded URL".
- **Function:** the one pure function that checks it.
- **Randomized test:** its name and fixed seed; it saves through `ConfigStore::update` and checks each accepted save against every page recorded so far in the sequence.
- **Generator coverage:** each kind of change the invariant covers (narrowing, widening, reordering, removal, coverage-changing edits), with an assertion that the generator reaches each.
- **Mutations caught:** each guard mutation the test was checked against, also listed in the test's doc comment.
