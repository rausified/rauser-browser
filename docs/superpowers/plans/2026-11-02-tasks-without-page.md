# Tasks without a page (brwr-qin)

## Goal

Run an instruction in the configured knowledge base without attaching a browser
page, including from a new tab. Preserve that scope through history and Reuse.

**Architecture.** Extend the existing task request and run records with explicit
scope; keep shared summary/site-draft machinery unchanged. Native chooses the
internal identity and service revalidates it before ordinary task setup.

**Tech stack.** Rust host/service, schema-generated Rust and TypeScript protocol,
TypeScript panel and Tasks controller, built-page Node and headless Chrome tests.

**Sources.** DESIGN.md §4.5/§5.7/§5.8, protocol/README.md “Agent tasks”,
TESTING.md, and bead brwr-qin.

## Open decisions

Resolved by Cookie as this run's coordinator, within brwr-qin's approved scope:

- Ordinary web tasks still include the page by default. A visible **Include this
  page** checkbox lets the user opt out; non-web tabs always say **No page**.
- A withheld HTTP(S) page still requires site access while inclusion is checked.
  Extraction failures never fall back to a task without a page.
- Reuse carries the original scope; keyboard recall continues to reuse only the
  instruction against the visibly selected current scope.
- No page means no extraction, page file, page-reading permission, Office/Miro
  retrieval paragraph, capture-limit claim, or Open page action. The working
  directory, file protections, setup, limits and task restart policy are unchanged.
- Deploy host and extension together. Legacy tasks default to including a page;
  older hosts refuse the new request shape, older extensions reject its new run
  field, and older services reject new stored task records. Rollback requires
  no active tasks, as before.

## Design challenge

AGENTS.md required a reviewer for this boundary. A read-only Codex reviewer
challenged making URL/key optional across all summary and site-draft machinery.
Adopted its simpler alternative: explicit optional `no_page` scope plus a fixed
host-owned non-HTTP marker/key in existing internal fields. The flag, never the
marker alone, selects the no-page branch. External requests omit page fields;
the host refuses mixed inputs, chooses the marker/key/kind, and the service
checks the exact combination again. Stored TaskStatus keeps scope explicitly;
RunView projects it only for a no-page task. The marker is not a captured URL
and is never opened or written to logs/notes as captured activity.

The review also caught Reuse silently attaching the active web tab and the
prompt's two separate page-read exemptions. Both must be covered before closure.

## Tasks

1. Add and validate the additive protocol shape at native and service boundaries.
2. Persist explicit task scope and omit page artifacts and prompt references.
3. Add panel scope choice and preserve scope through history/Reuse; cover real
   behavior with protocol, host, service, panel and browser regressions.
4. Run required and README checks, read the complete diff fresh, document results,
   export queue and prepare the driver commit.

## Validation

All four implementation steps are complete. The required full gate and README
supplement pass after correcting a Clippy `writeln!` warning: 1,382 Rust-reported
outcomes include 94 explicit IPC skips; all 44 extension test files pass;
accessibility reports two fixture passes and 40 explicit browser sandbox skips.
Eight new Rust/wire and seven new extension regressions cover the feature, plus
one new real-keyboard journey awaiting external Chromium execution. The existing
page-prompt suite has 42 passing checks. Supplementary checks include 10,696
inert Markdown renderings, eight script test files, and 221 Python tests (92
platform skips; queue and dev-tools both pass). Logs are in
`target/overnight/no-page/`.

The author read the complete code, tests, generated types and documentation diff
fresh. One local Codex review found no actionable findings; the external CLI
review is not run because this run forbids real external API/harness calls. Browser/IPC sandbox
skips never establish external acceptance; driver and platform CI own those
checks, independent review and publication. No live harness, API, service or git
remote was touched. Smoke steps and native dialog counts are unchanged.
