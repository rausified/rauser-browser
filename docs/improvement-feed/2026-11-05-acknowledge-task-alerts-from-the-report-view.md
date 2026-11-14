## 2026-11-05 — Acknowledge task alerts from the report view

When someone selects a finished report in Tasks, acknowledge that task's
completion in the worker ledger. Keep hidden tabs and background list refreshes
from marking a result read. The notification work (`brwr-8qm`) showed that the
panel tracker and notification clicks can acknowledge finishes, while an
already-open Tasks tab has no equivalent signal. Reuse `task_notice_seen` in
`extension/tasks.ts` for a visibly selected terminal task, with tests for
selection changes, hidden tabs and polling. This would prevent an alert for a
result the person has already read.

Tracked as `brwr-8v4w`; keep this separate from notification delivery.
- Review 2026-11-12: **Keep.** Mark a completed task alert read when its report is visibly selected in Tasks. This prevents a later alert for a result already viewed; a hidden tab must not count.
