## 2026-11-09 — Bound panel startup waits

Change: Show which startup step is waiting after the native hello, and put a bounded timeout with a retry action around the worker policy/status exchange. Record elapsed time for those steps in local diagnostics. Keep the native host and worker errors distinct.

Why: On the live `e03dbf476a5e` build, both the side panel and settings page displayed “Connecting to native host…” after each had already shown the host version. The host log shows `hello` and `get_config` completed promptly at 19:30:11 UTC; the pages did not reach their ready state until about 19:35 UTC. The generic message made a responsive native host look disconnected. A subsequent settings load and capture status were healthy, so this observation does not establish which worker step waited.

Where: `extension/panel.ts`, `extension/options.ts`, `extension/settings.ts`, and their startup tests. Capture step timing before changing locking or worker behavior, so the eventual fix targets the measured wait.
