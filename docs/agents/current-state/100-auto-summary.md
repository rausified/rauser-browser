# Auto-summary

Built: the worker summarizes each recognized artifact with no summary once its page loads (after an optional `dwell_secs` of focus), one at a time through the service's FIFO queue (`start_auto_summary`, additive within protocol v8;
`extension/auto-summary.ts`);
the host skips one that is off, not set up, not an artifact or already summarized, and an answer whose `access` is a sign-in, no-access or not-found wall saves nothing

Pointer: DESIGN.md §5.2 Auto-summary
