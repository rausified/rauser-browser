## 2026-11-11 — Make reviewed feed status visible

Change: add a small feed review command or generated index that distinguishes open, implemented pending verification, done, duplicate, and retired suggestions without rewriting their original text. It should point to dated review decisions and report entries whose `Done` marker is missing after verified implementation.

Why: the 2026-11-11 review found 28 implemented or likely implemented entries among 115, while only 20 entries contain a `Done` marker. Reading “open entries” alone therefore overstates the work left and repeats completed proposals.

Where: `scripts/check_improvement_feed.py`, `BRAUSER_IMPROVEMENT_FEED.md`, and `docs/improvement-feed-review-2026-11-11.md`.
- +1 2026-11-12: Review dispositions were appended to all 115 assessed entries; the feed now needs a status-aware index so Keep and Revise can be read without scanning each file.
