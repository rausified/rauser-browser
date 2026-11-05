# Task completion outside the panel

Bead: `brwr-8qm`. Goal: let a person leave a running task and receive a
completion alert with a route back to its report.

## Open decisions

- Resolved by Cookie under the unattended feature authorization: use the
  optional Chrome `notifications` permission, off until an explicit Settings
  click grants it. Existing installations acquire no required permission.
- Resolved by Cookie: notifications contain only the outcome, never the
  instruction, title or report. Clicking opens the originating window's panel
  and the selected Tasks report, with a Tasks fallback if the window closed.
- Resolved by Cookie: capture pressure `!` wins over unread completion counts.
  Opening the panel clears unread alerts; observed finishes suppress alerts.
- Resolved by Cookie: commit handling before calling Chrome's notification
  API. A worker termination in that narrow gap can lose a toast, but retains
  the unread badge. A retry must never repeat a previously handled toast.

## Design challenge

Reviewer `/root/notification_design_challenge` rejected watching only the run
ID returned to the panel: closing during `start_task` loses that ID. Adopt a
durable discovery intent and alarm before sending the native request, then
walk paginated task history and retain watched IDs. Keep slow host work out
of capture's serialized chain. Defer notification while a panel is open and
accept its observed-finish acknowledgements. Continuous polling whenever
permission is enabled was simpler, but would wake the service while idle.
Encode the originating window and run UUID in the notification ID so opening
the panel starts synchronously inside Chrome's click gesture.

Fresh implementation review identified two defects in timestamp-only
discovery: overlapping windows picked the wrong window, and another browser
profile's task could be attributed to this one. Both were reproduced in a
failing regression. Adopt exact start-request correlation: generate the
existing request UUID before arming, persist it in the worker intent, retain
it in the service's task record and expose optional `RunView.start_request_id`.
Only matching IDs can become watched tasks. This adds no caller-supplied run
identity or authority, and older uncorrelated tasks never become candidates.
The alarm runs once a minute, within the existing minimum Chrome version.

## Delivery

1. Worker ledger and independent polling, permission checks and badge owner.
2. Pre-start handoff, panel acknowledgements, opt-in Settings and click route.
3. Offline lifecycle/integration tests, complete health gate and fresh review.

## Validation and progress

Required baseline checks passed on `cf569cb8`; Chromium and local IPC use the
repository's explicit sandbox guards. Implementation and focused correlation
checks are complete. The final required gate passed: 1,627 Rust-reported
passes (94 IPC guards; two ignored), all 48 extension test files, both Clippy
configurations and accessibility (two fixture passes, 50 sandbox skips). One independent local reviewer
pass and a fresh manual diff review caught and fixed the attribution issues
above, the slow-poll acknowledgement race, and a worker test's inherited
capture-pressure fixture. No live harness, account, API or deployed service
was used. Host and extension must deploy together for task-view correlation;
older services cannot read new correlated task status records on rollback.

The implementation also registers notification clicks after optional permission
bindings become available, without letting a refused binding break capture
startup. Twenty new notification module cases cover the lifecycle and controls.
Real desktop notification and gesture checks remain in TESTING.md; no live
acceptance is claimed. Follow-ups are `brwr-8v4w` (read reports acknowledge
alerts) and `brwr-2d4n` (external permission changes refresh Settings).

The single commit attempt failed on the sandbox's read-only `.git/index.lock`.
The complete reviewed tree is left for the driver, with the exact message in
`.bosun_commit_msg`. No commit retry, push, merge or deployment was attempted.

README extras and final queue consistency passed; all validation processes
have completed. `brwr-8qm` is closed with its shipped metadata. The runtime
log records the complete green uncommitted handoff for the driver.
