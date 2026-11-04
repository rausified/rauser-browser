# Reusable sidebar task commands

Child: `brwr-f410`; parent: `brwr-cdq`.

Goal: run a locally configured, named task from the task bar without repeatedly
pasting its instructions. Use the existing task execution and report flow.

## Open decisions

All decisions for this slice are resolved by Cookie (2026-11-04), consistent
with the existing task trust and output contracts:

- Accept `[[commands]]` with ASCII `name`, plain `description`, literal `prompt`
  and `output = "sidebar"` (also the default). Reserve documented built-ins.
- Defer selective `include`, template variables, built-in actions and other
  outputs to the parent. Reject unsupported fields and documented template
  placeholders visibly instead of silently broadening context.
- The existing Include this page checkbox controls attachment. Page text and
  selection remain solely untrusted context in `page.md`.
- `/name` plus optional trailing user input invokes a command. `//` escapes a
  literal leading slash. Unknown or invalid commands refuse; ordinary tasks
  continue even when the command registry needs repair.
- Store the effective literal instruction in existing task history. Reuse and
  recall escape its leading slash, so they never resolve a changed command.
- A command invocation must carry the reviewed config revision. A stale choice
  keeps the draft, refreshes the list and needs another explicit Run.
- Results use the existing Tasks report view; no new report renderer or file
  destination is added.
- No additional permission, credential, output file or background run is added.

## Design challenge

Before implementation, `/root/command_design_challenge` recommended removing
template expansion and `include` from this first slice. Adopt that alternative:
commands compose literal trusted instructions with trailing user input, while
page-derived data never enters a directive. Preserve the raw TOML registry
through unrelated settings saves and expose invalid registries separately from
general config health. Resolve command, revision and page key under one common
config lock, including no-page tasks. Escape leading slashes when returning
stored effective instructions to the bar. These points avoid a new template
language, broadened context, split-lock snapshots and accidental command reuse.

## Delivery

1. Host-owned registry, bounded validation, raw preservation and descriptor API.
2. Revision-bound native resolution into the existing privileged task request.
3. Task-bar chooser, explicit execution, literal history/reuse/recall behavior.
4. Config, wire, native and built-panel regressions; full diff review and gate.
5. Current design, testing guidance, feature narrative and driver handoff.

## Progress

Implementation complete. The required full health gate passes: 1,619
Rust-reported passes (+10; 94 explicit IPC guards and two ignored tests), all
47 extension files, both Clippy configurations, protocol generation,
build/type/lint checks and accessibility's two offline fixture tests. Its
48 real-browser cases explicitly skip in the sandbox. The new regressions
include six JavaScript cases and a browser keyboard journey; driver/CI must
exercise that journey in Chromium. README current-state, script and Markdown
checks pass (10,696 inert renderings). Both Python tooling commands pass all
252 cases with 92 explicit platform skips, and queue consistency passes.

Fresh complete manual diff review corrected the page-key refresh ordering,
fixture assumptions, Clippy warnings and the native-select focus concern. The
chooser uses a separate Insert action, preserving normal select keyboard
behavior and keeping Run explicit. Command resolution follows the page-key
refresh under the common lock. A fixed command-name heading also keeps every
new command instruction literal when reused.

The single final independent in-session Codex pass
(`/root/command_design_challenge`) found no actionable issues and accepted the
existing Tasks report as the documented first-slice output. No external API
or real harness was invoked. The driver owns unsandboxed/platform validation,
commit fallback, independent external approval and publication. `brwr-cdq`
retains built-ins, templates, selective context and other outputs;
`brwr-kqt4` tracks the Settings command editor. Assigned Re-learn work remains
blocked on its documented recovery/history policy decisions.

Closeout: `brwr-f410` is source-complete and closed after the full local
gate and review. The single commit attempt failed because `.git` is mounted
read-only; no retry or remote action was attempted. The green working tree
and exact `.bosun_commit_msg` are ready for the driver commit fallback.
