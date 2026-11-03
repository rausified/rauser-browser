## 2026-11-02 — Record which frame a page read came from and whether the Word wait timed out

Change: have `readTab` return a small read report (each frame's host, trimmed
character count and `word` flag, which frame was chosen, and whether the Word
paragraph wait ended by paragraphs, timeout or navigation, with its elapsed
ms), and log it through the extension's diagnostics and the host's
`request done kind=start_run|start_task` line. Why: diagnosing run 92dcfad9
("Word is starting." as page.md) had only the 17-character page.md and the
host's `thin_office` flag; nothing said which frame produced the text, so the
fix had to key on "no Word paragraphs yet" without ever seeing the splash's
markup, and a future timeout (empty Word content) will look the same as an
unrendered document. Where: extension/summarize.ts `readTab`/`choosePage`,
extension/diag.ts, protocol/schema.json (an optional field on `start_run` and
`start_task`), host/src/service/capture_quality.rs for the log line.
