---
title: Side panel redesign (dark glass)
date: 2026-11-02
status: in progress
---

# Side panel redesign

Source: the "Brauser Sidebar" design (dark glass, 380 px side panel) John
supplied on 2026-11-02. Decisions John made before the build:

- The page card shows the page's title and URL again. This supersedes the
  2026-11-02 "artifact status only" layout (PR #231).
- Dark glass is the dark theme; a matching light variant follows
  `prefers-color-scheme: light`.
- Build the full design, including the pieces the old panel lacked.

## Constraints

- Extension CSP: `style-src 'self'` (no inline `style` attributes) and
  `font-src 'none'` (no IBM Plex Sans / JetBrains Mono). The design's fonts map
  to `system-ui` and `ui-monospace`; every style lives in `panel.css`, scoped
  under `body.panel-page` so `options.html` and `tasks.html` do not change.
- Every existing element ID and behavior stays: panel tests, a11y tests and
  the macOS smoke drive them. Agent and page text stays `textContent` only.
- No protocol or host change. Everything new derives from data the panel
  already receives (`ArtifactView`, `SummaryView`, `WorkerStatus`, config).

## Mapping

| Design element | Implementation |
| --- | --- |
| Header mark, name, `M2 · build` | `.brand-mark`, `h1`, the build line in monospace |
| Header capture pill | New `#capture-pill`: "Capture on" when capture is on and this page is captured (an adapter recognizes it), "Capture off" otherwise, "N pending" (amber) when records are queued and delivery reports an error |
| Page card: glyph, title, URL | `#page-glyph` from the artifact type or host (`extension/panel-look.ts`), `#page-title` / `#page-url` visible again |
| Kind / mode / measures chips | New `#artifact-kind` (type label), the existing `#artifact-mode`, a short `#artifact-measures-short`; the full measures stay in the details row |
| Within | Existing `#artifact-within`, folder icon |
| Why I'm here | Existing `#artifact-why` with a pencil icon; Enter saves; `#save-why` shows only while the field differs from the saved value |
| Uncaptured actions (Save Page, Capture Site) | Dashed box in the page card: `#save-page` runs Summarize & log (which records the page as a `web.page` artifact with its summary); Capture Site moves into the box |
| Task bar | Gradient-bordered field, round arrow Run button; Tasks ↗ beside the context line |
| Summary card | `#page-summary`: label, time from `summarized_at`, Open summary link, overview, chips (type, category, importance; "high" highlighted), the re-summarize icon button (`#summarize`) |
| My note | Restyled header and textarea |
| Collapsed extras | `<details>` rows: Recorded navigation (with count), Linked and contained, Page patterns |
| Tabs | Segmented control |
| Recent | Search field with icon plus the type select inline; rows with glyph, title, sub line, focused time |
| Worklog | Today / Yesterday / Week segmented radio group replacing the select; Include gaps; Copy with icon |
| Footer strip | `#capture-strip` details: summary line shows pending count and host build; a primary `#capture-strip-action` (Pause capture / Send now / Resume in settings); the existing controls stay in the expanded body |

## Tasks

1. `panel-look.ts`: glyph and hue for artifact types and hosts, importance
   tone, summary time label, capture-pill state. Unit tests.
2. `panel.html` restructure with all IDs kept and the new elements.
3. `panel.css`: tokens for dark and light, scoped components.
4. `panel.ts`: wire the pill, glyph, chips, summary card, Save Page, why
   save-on-Enter, worklog radio group, strip action, Recent rows.
5. Update panel tests, a11y, smoke references (`#worklog-period`).
6. DESIGN.md §5.7, docs/history.md, TESTING.md if coverage changes.
7. Gate, review, PR, deploy.
