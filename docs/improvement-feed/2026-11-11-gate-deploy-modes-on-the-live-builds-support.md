## 2026-11-11 — Gate deploy modes on what the live build supports

**What to change:** when a `scripts/dev/deploy-live` mode depends on new service behaviour, have deploy-live check that the live binary has that behaviour before relying on it, and refuse the mode if it doesn't. Don't rely on a warning. `--drain` now does this: it refuses unless `target/debug/brauser` contains `deploy-drain.json`, the marker the service checks.

**Why:** the first version of `--drain` (brwr-a8w2) wrote a pause marker and went ahead with only a warning that an older service ignores it. Codex flagged this as a P1. On the very first deploy that brings the feature, the service still running is the old build. A run could start between the last scan and the service restart, and the restart would end it, which is the exact interruption `--drain` exists to prevent. A capability check on the live build closes that gap. Without it, the first deploy silently breaks the promise.

**Where:** `scripts/dev/deploy-live` (`honours_drain`) and its tests. Use the same pattern for any later mode that needs the service, host or extension to cooperate, such as the loaded-build probe in brwr-x622.
