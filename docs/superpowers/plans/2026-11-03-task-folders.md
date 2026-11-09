# Per-task working folders

Bead: `brwr-zla`. Let a user choose a native-picked project folder for a task,
keeping the knowledge base as the default.

## Open decisions

Resolved by Cookie as cycle coordinator, within the bead's configured-allowlist
contract: folder selection is an explicit settings action through the existing
native picker; task starts carry opaque IDs only. Entries are host-owned in
`config.toml`, with immutable UUIDs and filesystem identities. Removing an entry
never deletes its folder and never redirects queued tasks. Custom tasks need a
valid home/protected paths but do not need the default knowledge base mounted.
The choice is per task; history shows the recorded cwd and Reuse retains the ID.
This changes the full-access agent's starting folder, not its filesystem access.

## Design challenge

Reviewer `/root/task_folder_design_challenge` recommended retaining host-only
entries instead of exposing writable folders in ConfigSnapshot, reusing purpose-
bound picker grants, and extending the queue's cwd check with the immutable ID.
Adopted: a path-only check could revive a queued task after removal/re-add at the
same path. Preserve the existing protected-path check and fail closed on a removed
or replaced destination. Default tasks keep their knowledge-base check.

## Delivery

1. Add bounded host-owned folder entries, grant purpose and dedicated revisioned
   list/add/remove messages; preserve entries on ordinary config saves.
2. Carry the optional folder ID through task/native/service records, resolve it
   under lock, and recheck it before launch alongside protected paths.
3. Add settings management, a task-bar selector, history context and Reuse.
4. Exercise forged/stale/removed/replaced choices and offline UI behavior; run
   full required/README gates, review the complete diff, and document/close.

## Review corrections

One independent Codex review after the full required/README gate found no P1,
two P2 and one P3. All three were reproduced through existing test seams:
folder listing failed to recover at the same revision after reload/reconnect;
selected-folder refusals used the generic setup error code; and a non-task
view could expose task metadata. The fixes refresh choices after successful
config reload and host reconnection, return the existing conflict response
that preserves the actionable folder explanation, and filter the field by
run kind. Existing projection/fake-harness tests were strengthened, two new
panel retry cases and a complete Tasks-to-Reuse case pass. No second review
is needed under AGENTS.md's one-pass rule; no P1 behavior changes occurred.
The complete gate is rerun on these corrections before closeout.

## Completion

All four delivery steps are complete locally. The final full required and README
gate passed after the review corrections: 1,562 Rust-reported outcomes (+9; 94
explicit IPC guards and two ignored), all 45 extension files (+11 offline cases),
both Clippy builds, scripts/229 Python checks and 10,696 inert Markdown renderings.
Two accessibility fixtures pass; 43 browser cases explicitly skip in this sandbox,
including the new task-folder keyboard journey. Real picker/browser/platform
acceptance and publication remain with the driver. `brwr-zla` is closed;
`brwr-m5dm` tracks the build-time import validation suggestion.
