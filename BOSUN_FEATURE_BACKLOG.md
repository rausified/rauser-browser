# Brauser Feature Backlog

This file is read by Cookie's overnight loop. DESIGN.md is the authoritative
spec: read it before starting any item below, follow its decisions (§14) and
security rules (AGENTS.md), and update it yourself when an implementation
choice changes a contract it describes. Do not treat this backlog as a
substitute for DESIGN.md — it exists to keep each cycle's scope bounded, not
to repeat the spec.

## Known Issues (Blocking Deploy)

- None.

## Proposed

### M1.5a adapter review UI (DESIGN.md §7.2)

Step 6.3's config, lease, worker prefilter, and host re-check are complete.
The remaining UI work makes that capability usable without editing config by
hand and gives the native confirmation the evidence the design promises.

- [ ] **Build adapter review and match preview.** Add a settings editor that
  shows each adapter's rules, scope, Tier 1 fields, sync caution, overlap names,
  and probe-URL match preview before the existing revision-bound native token
  enables it. This is the shortest path from the now-active recognition engine
  to something a person can configure without hand-editing TOML.

### M1.5a step 6.4 — Measured artifact activity

- [ ] **Replace temporary visit rows with hourly artifact activity.** Track
  cold opens, focused seconds, background seconds, modes, and trail sources in
  durable worker totals, then freeze and deliver one idempotent activity event
  per artifact and local hour. Rebuild the tab map on worker start and handle
  reloads, discarded tabs, SPA artifact changes, idle/lock, UTC-offset changes,
  and browser restarts as specified in DESIGN.md §5.1 and §12.2 step 6.4.

### M1.5a first-use product surface

- [ ] **Ship starter adapters and a cross-tool Recent view.** Package the Jira,
  SharePoint/OneDrive, and scoped GitHub adapters with recorded examples and
  documentation, then expose recognized artifacts in a panel list grouped by
  day with type and title filters. This turns the capture foundation into the
  first complete user promise: one useful history across work tools.

## In Progress / This Iteration

- None.

## Shipped

- **Category:** Security & Privacy. **Activate adapter-only capture at both trust boundaries (2026-10-16).** Every host-confirmed adapter document now travels in the revisioned worker lease, where the TypeScript engine drops rule misses, out-of-scope URLs, and ungranted navigations before reading tab metadata or buffering page-derived content. The native host independently re-evaluates every original URL and title under the config lock and terminally suppresses anything a stale or compromised worker widened; grant removal narrows sites and templates together, and a cached pre-adapter lease self-renews once instead of silently disabling background capture after upgrade. The macOS smoke flow now confirms a fixture adapter through the real native protocol before proving that only its recognized route reaches the log. **Metrics:** 244 Rust tests and 5 extension suites passing; full protocol generation, build, test, format, clippy, npm install, typecheck, lint, and extension checks green.
- **Category:** Testing & Verification. **Restore portable vault and harness verification (2026-10-16).** Directory durability now reopens the current capability before syncing, so Linux `O_PATH` handles no longer turn note saves and visit logging into `EBADF` failures while the filesystem boundary remains capability-scoped. Fake harness installs now use explicit private modes instead of inheriting the runner's umask, and their work-directory assertion selects the platform's actual `stat` syntax, keeping the security suite deterministic without weakening its unsafe-install checks. **Metrics:** 243 Rust tests and 5 extension suites passing; full protocol generation, build, format, clippy, typecheck, lint, and extension checks green.
- **Category:** New Capability. **Make adapter templates real, host-validated protocol v5 configuration (2026-10-16).** Brauser now round-trips `[adapters] format = 1` and `[[artifacts]]`, validates every definition with the Rust adapter-template authority and enabled-origin policy, surfaces bounded overlap warnings, and puts adapter changes behind native consent. Unsupported future formats remain intact on disk while every extension entry point and the autonomous worker suspend capture; unscoped shared origins disclose every-account capture, existing IDs stay frozen until migrations exist, and page notes and AI settings remain usable during adapter repair. **Metrics:** 12 protocol tests, 8 focused host regressions, and 5 extension suites passing; full build, format, clippy, typecheck, lint, and extension checks green.

## Notes / Ideas for Later

- The artifact boundary is now real, but recognized events deliberately still use the M1 visit writer. Step 6.4's hourly attention totals are the next semantic migration; avoid extending the temporary visit format.
- An enabled M1 site with no adapter now records nothing, as DESIGN.md requires. The settings page does not yet offer an adapter editor, so the next user-facing slice should close that first-use gap rather than adding more invisible config machinery.
- The headless macOS smoke flow was updated to confirm and install a fixture adapter through native messaging. It could not be executed on this Linux VM; run `npm run smoke:macos:headless` on macOS before relying on that new end-to-end step.
- Protocol v5 currently freezes an existing adapter's `id` template before any alias migration exists. When activity events land, replace that conservative guard with the designed activity-aware migration and explicit alias events rather than relaxing it in place.
- A future adapter editor should explain overlap pairs with adapter names and example URLs, not raw indices. The indices are stable protocol data for the next UI, while settings currently gives a bounded “earlier rule wins” warning.
