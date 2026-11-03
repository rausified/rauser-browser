# Personal summary style

**Goal:** Let a user tailor summary language, emphasis and presentation without
rebuilding Brauser or replacing its required answer and core instructions.

**Architecture:** Read one optional plain-text file at the picked home root.
Snapshot it in the run directory before publishing queued status. Render its
JSON-quoted value once through the compiled `SUMMARY.md` template. Existing
answer validation, summary storage and full-access harness behavior apply.

**Tech stack:** Existing Rust capability-scoped file access, run store and
prompt renderer; static settings help. No new service, credential or dependency.

**Sources:** `brwr-sm4`, `SUMMARY.md`, DESIGN.md §5.2, §7.1, AGENTS.md.

## Design challenge

- **Challenger:** `/root/summary_guidance_challenge`, 2026-11-03, read-only.
- **Alternative:** Save a per-run sidecar rather than extending strict
  `RunStatusFile`. Older services reject unknown status fields, which can
  prevent their process-group recovery.
- **Outcome:** Adopted. Write the sidecar before status publication; resumed
  runs read only that snapshot. Missing sidecars mean compiled defaults for
  legacy records. Reuse one-pass substitution rather than a new template
  language. JSON quoting and ordering are prompt framing, not tool enforcement.

## Open decisions

All resolved by Cookie as this unattended cycle's coordinator, 2026-11-03:

- Use exactly `SUMMARY_STYLE.md` at the selected Brauser home root. No search,
  includes, frontmatter parsing or replacement of the compiled root file.
- At most 8192 UTF-8 bytes, with ordinary text/Markdown and newline/tab/CRLF.
  Missing or whitespace-only means default. Invalid, oversized, unreadable,
  symlink and nonregular files refuse summaries with an actionable message;
  tasks, site drafting, notes and capture do not read the file.
- Read when the service accepts a new summary; the first queued run's snapshot
  wins when a second request joins it. Changes affect subsequent new runs.
  Snapshot reads are bounded and validated again before starting a harness.
- Personal guidance may refine style, language and emphasis. Compiled rules
  take priority, including the answer format, untrusted-page treatment,
  file-write and sending instructions. The answer schema remains enforced;
  tool restrictions are still prompt-level with a full-access harness.
- Older hosts ignore the sidecar and resume queued summaries using their
  compiled defaults. This feature changes no status/wire/config format.

## Tasks

1. Read and validate fixed home guidance; store a private bounded per-run
   snapshot before queued status becomes visible.
2. Integrate summary-only setup and rendering; preserve queue/restart behavior
   and test fake-harness prompts, literal placeholders and fixed core rules.
3. Explain the file in settings/README/design, run the required and README
   gates, review the complete diff and record closeout evidence.

## Completion

All three tasks completed on cycle base `41c59484`. Nine new offline Rust
cases cover file reading, setup, private storage, prompt construction and
new/restarted in-process harness runs. The complete required gate and README
checks pass: 1,562 Rust-reported outcomes (94 explicit sandbox socket skips,
2 ignored), 44 extension files, both Clippy variants, 229 Python cases
(92 platform skips), 11 script files and 10,696 inert Markdown renderings.
Accessibility reports 2 fixture passes and 42 explicit browser sandbox skips.
Logs: `target/cookie-summary-style-final/01..17.log`.

Complete fresh manual diff review and one independent in-session Codex review
reported no P1/P2/P3 findings. External review harness calls were forbidden by
the offline cycle guardrails; the available in-session reviewer performed the
pass without APIs, remotes or edits. Driver owns outside-sandbox verification,
final commit SHA and publication. Real model obedience/style quality and live
browser/platform acceptance remain unclaimed. Follow-up `brwr-myn8` proposes
an explicit read-only settings preview of effective personal guidance.
