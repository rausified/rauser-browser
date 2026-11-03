## 2026-11-02 — A `brauser summary-check` command for iterating on SUMMARY.md

What: add a developer command (host CLI, beside `brauser adapters relearn-plan`) that renders `SUMMARY.md` for a given `page.md` and knowledge base (`--print-prompt`), runs the configured harness once on it without the service, and validates the final answer with `answer::parse_answer`, printing the rendered summary file. Optionally `--compare` runs both harnesses.

Why: testing the SUMMARY.md v2 rewrite against real pages (a Jira ticket and a GitHub PR from earlier runs' `page.md`) meant adding a throwaway ignored test to render the prompt and parse the answer, then removing it before commit. The live runs found a real wording bug (a two-sentence `overview`) that no unit test can catch. John plans to keep evolving SUMMARY.md and may make it user-editable (brwr-sm4), so this loop will recur.

Where: `host/src/main.rs` (CLI), `host/src/service/prompt.rs` `summary_prompt`, `host/src/answer.rs` `parse_answer`, `host/src/summary.rs` `render_summary`; document it in TESTING.md.
