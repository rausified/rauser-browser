# Replace GitHub Actions with independent CI

Status: proposal for approval, 2026-11-14. No replacement service has been installed and no workflow or merge gate is changed by this document.

**Goal:** Remove GitHub Actions completely while retaining automatic verification, safe integration through Mergify, PR maintenance, work-queue checks, and release packaging.

**Architecture:** Use Buildkite to orchestrate the existing repository commands on dedicated native macOS and Windows workers, plus Linux for portable checks and maintenance. Keep GitHub for source, PRs, commit statuses and releases, and keep Mergify for integration batches. Use Buildkite's direct GitHub integration; there is no Actions trigger or Actions runner in this design.

**Tech stack:** Existing Rust, Node, Python, bd/Dolt, packaging scripts and Mergify; Buildkite pipelines and agents. Keep Brauser's build infrastructure independent of any other product or personal knowledge base.

**Sources:** Repository inventory at main `d8ef0026fba9601711c8991b511a514a4caa4de2`, the seven files under `.github/workflows/`, `.mergify.yml`, `worker-contract.json`, the PR scripts and `TESTING.md`. Vendor facts were checked against the primary documentation linked below on 2026-11-14. Pricing is USD and may change.

## Recommendation and cost

Start with **Buildkite Free and dedicated self-hosted agents**, if suitable machines are available. It avoids operating a CI server while retaining logs, artifacts, scheduling and GitHub statuses. Buildkite's control plane and agents are independent of Actions. [Architecture](https://buildkite.com/docs/pipelines/architecture)

The Free plan is $0 for up to five users and ten concurrent self-hosted jobs, with 30-day build/artifact retention. Hardware, isolation, updates and uptime remain our responsibility. Hosted macOS and Windows require Pro: $30 per active user/month plus compute. Current medium rates are $0.12/minute for Mac and $0.032/minute for Windows. An illustration of 100 runs using 15 Mac minutes and 10 Windows minutes each is $242/month before other usage and taxes; it is not a workload estimate. Measure actual minutes, including queue batches and retries, before choosing hosted capacity. [Pricing](https://buildkite.com/pricing/)

Buildkite's hosted Windows offering is in public preview, so it needs a native test pilot before it can become our merge gate. [Windows hosting](https://www.buildkite.com/platform/pipelines/hosting-options/windows-hosted-agents/)

We have not established that a dedicated Mac or native Windows x64 worker is available. A Mac-to-Windows cross-build does not execute Windows tests. The daily development Mac may run manual checks, but should not be the only unattended worker: sleep, load and a user's session must not determine whether the queue advances. Native self-hosted agents are supported on [macOS](https://buildkite.com/docs/agent/self-hosted/install/macos) and [Windows](https://buildkite.com/docs/agent/self-hosted/install/windows).

The fallback is **TeamCity Professional** if we want to operate the controller ourselves. Its free edition allows three agents, 100 build configurations and ten pipelines. That fits one Mac, one Windows and one Linux worker, but adds server upgrades, backups and access administration. Buildkite is the smaller operational commitment for this repository. [TeamCity licensing](https://www.jetbrains.com/help/teamcity/licensing-policy.html)

A custom local CI daemon is not recommended: it would recreate reliable event discovery, a durable work queue, retries, cancellation, logs, secrets and an operator interface. Jenkins is viable but adds a controller and plugin maintenance without an existing installation to reuse. Cirrus CI is excluded because its official shutdown date was June 1, 2026. [Official announcement](https://cirruslabs.org/)

## Design challenge

- **Challenger:** Independent gate-design reviewer in this session; a second reviewer inventoried the workflows and a third checked vendor capabilities and pricing.
- **Alternative:** A single local command plus a launchd timer could run checks and publish commit statuses, avoiding another CI service.
- **Outcome:** Keep that command as a useful manual entry point, but use established independent CI for the continuous gate. A timer still needs native Windows capacity and would otherwise grow into a second CI platform. Preserve the current CLI implementation rather than rewriting the tests or merge queue.

## Open decisions

- **Decision:** GitHub Actions is removed completely. **Status:** resolved by the user. **Default if unresolved:** not applicable; no Actions-backed replacement or temporary reactivation.
- **Decision:** Adopt Buildkite. **Status:** proposed, awaiting user choice. **Default if unresolved:** no account installation or service purchase.
- **Decision:** Dedicated self-hosted Mac and Windows workers versus paid hosted capacity. **Status:** open; suitable dedicated capacity is unverified. **Default if unresolved:** no automated claim of native platform coverage and no purchase.
- **Decision:** Preserve selective pre-merge macOS coverage and full main coverage. **Status:** proposed, matching the inventoried main policy. **Default if unresolved:** do not silently downgrade coverage. PR #476 separately proposes main-only macOS; reconcile it before cutover.
- **Decision:** Release provenance identity and retention. **Status:** open. **Default if unresolved:** build unsigned test artifacts; do not claim equivalence with GitHub's attestation or publish a signed public release.
- **Decision:** Repository-admin installation and protected-branch settings. **Status:** owner step needed. This session's account has write access, not admin; live GraphQL showed no branch-protection rule. **Default if unresolved:** keep Mergify's gate and do not claim direct pushes are technically prevented.

## What Actions does today, and its replacement

| Current workflow | Replacement | Existing implementation to retain |
|---|---|---|
| `ci.yml` | Buildkite verification pipeline, direct PR/main webhooks, change classification, aggregate result | `ci-changes.mjs`, `ci-macos.mjs`, docs checks, privacy scan, dependency policy, accessibility and fuzz commands |
| `build-os.yml` | Native macOS and Windows jobs with pinned tools, caches and timeouts | Rust build/test/clippy, protocol/starter checks, extension tests, Markdown safety, script tests, adapter differential tests, npm audit, scripted-dialog tests and macOS headless smoke |
| `pr-hygiene.yml` | Metadata/policy jobs, including PR title/body edits without a new commit | `pr-owner.mjs`, privacy scan and the full Python dev-tools suite |
| `pr-doctor.yml` | Trusted maintenance pipeline: main changes, verification completion and a 15-minute schedule | Doctor plan/repair/publish, PR keeper, safe conflict repair, requeue/refresh, sticky alerts and repair beads |
| `queue-check.yml` | Six-hour schedule plus checks on changes to queue tooling | Bootstrap Dolt from `refs/dolt/data`, consistency checks and failure issue; keep this separate from checking main's source tree |
| `mergify-enqueue.yml` | Disable GitHub native auto-merge during cutover; maintenance detects drift | Keep Mergify as the supported path and use the existing queue client; polling alone cannot prevent an already-green auto-merge |
| `release-macos.yml` | Separate trusted release pipeline, started explicitly or by a version tag | Universal app/DMG, signing/notary hooks, verification, SBOMs, checksums and draft GitHub release |

Most work is already ordinary shell commands. Replace checkout/setup/cache/artifact/event wrappers, not the test implementation. The existing `merge_group` trigger is unused by Mergify. The private-repository dependency-review job is dormant, but Rust dependency policy and npm audit are active. Packaging's unsigned path exists; no Developer ID signed/notarized release has yet been demonstrated.

## Proposed pipeline and trust contracts

Use a verification pipeline, a trusted maintenance pipeline, and a trusted release pipeline. Verification runs code from PRs on isolated workers without repository-write, Apple or release credentials. Maintenance runs trusted main tools. Its doctor retains **credentialed plan → isolated credential-free repair → fresh credentialed publication**, including independent tree verification and a final PR-head check. Release credentials are available only to its trusted release path.

Agent queue names route work; they are not a security boundary. Put untrusted verification and trusted operations in separate clusters or an equivalently enforced boundary. Restrict credentials outside branch-controlled YAML, and test that a verification job cannot dispatch into the trusted pool or obtain its secrets. Validate the selected plan's controls during the pilot. [Buildkite security controls](https://buildkite.com/docs/platform/security/enforcing-security-controls)

The trusted pipeline definition decides required jobs. A branch cannot delete a job and mint its own passing aggregate receipt. Keep clean checkouts, isolated job state, current pinned agents, private read-only checkout access, explicit timeouts and a documented runner owner. Preserve main-only writes to trusted dependency caches: PR jobs must not poison caches restored by main or release jobs. Retain the headless smoke's temporary HOME/profile and fake harnesses; tests never use a live user profile or real model.

### Checks and Mergify

Use distinctly named external status contexts, for example `buildkite/brauser-verify` and a separately reported PR-policy result. Confirm their actual names in the pilot, then use them in Mergify. Buildkite statuses can be required directly through `check-success`. [Mergify integration](https://docs.mergify.com/integrations/buildkite/)

Build ordinary PRs and Mergify's **draft** `mergify/merge-queue/*` PRs. Verify the actual checked-out commit, tree, PR head and base. Publish to the precise SHA Mergify is waiting on; contributor-head success never substitutes for success on a different integration head. Keep batches of up to three and speculative checks in Mergify, adjusting concurrency to available workers. [Batch CI contract](https://docs.mergify.com/merge-queue/batches/ci/)

Preserve native Windows runtime tests for every code change, selective native macOS/service/smoke coverage before merge, full macOS and Windows on main, and Linux accessibility/fuzz checks. Preserve docs-only checks and policy checks, including full dev-tools coverage currently supplied by hygiene. Failed or unknown classification runs full coverage. Missing workers, interrupted tests and missing results block the aggregate gate.

Each run stores an immutable receipt with repository/run/event IDs, exact checkout/head/base/tree IDs, classifier and pipeline revision, native OS/architecture and tool versions, command outcomes, allowed skips, retry history, timestamps, and hashes/links for logs and artifacts. Keep merge receipts beyond the provider's short retention window; signed-release evidence travels with its release assets. A trusted publisher verifies required coverage before success. PR metadata gets its own revision/digest so body edits invalidate the old policy result even when the head SHA is unchanged.

Update `.mergify.yml`, `worker-contract.json`, `scripts/dev/queue-pr.mjs`, `pr-watch.mjs`, `pr-keeper.mjs`, `pr-constants.mjs` and their tests. They already accept GitHub statuses, but still contain Actions gate names, workflow rerun hints and trusted-bot defaults. Replace those assumptions with provider result URLs, rerun commands and the new authenticated service identity. [Direct GitHub integration](https://buildkite.com/docs/pipelines/source-control/github)

### Maintenance, artifacts and releases

Buildkite schedules support the existing 15-minute doctor and six-hour queue check; scheduling is approximate, so add reconciliation for missed events and overlapping runs. Keep one active scheduler for each maintenance function. [Scheduled builds](https://buildkite.com/docs/pipelines/configure/workflows/scheduled-builds)

The maintenance wrapper must also poll/reconcile PR title/body changes, disabled native auto-merge settings, stale statuses and keeper alerts. Reconciliation detects drift; it cannot stop an auto-merge that already happened. Disable native auto-merge at the repository setting, and configure protected-branch checks where available. Preserve Dolt authentication errors as failures; an unavailable database is not an empty healthy queue. Run from trusted main, with the database isolated from developers' live checkout.

Move release artifact transfer to the external store and GitHub Releases. GitHub Actions OIDC attestation is not portable to another issuer: define and exercise a replacement signing/provenance identity before claiming its guarantees. Checksums and SBOMs remain useful but are not provenance. Keep Apple credentials out of verification and maintenance jobs.

## Migration and acceptance

1. **Prove capacity and access.** Choose the service and worker budget; obtain repository-admin integration approval. Run one representative native macOS smoke and native Windows suite on isolated candidates. Record cold/warm duration and cost.
2. **Extract provider-neutral entry points.** Wrap the existing check commands and receipt format. Test classification, failed commands, allowed skips and interruptions. Keep verification separate from status publication.
3. **Pilot external checks without Actions.** Demonstrate passing and deliberately failing code/docs PRs; title/body edits; a two-PR Mergify batch; a new push invalidating old success; and a worker killed mid-test producing no successful gate. Verify cancellation does not cancel main history.
4. **Move maintenance.** Exercise doctor repair and needs-person cases, keeper requeue/refresh, accidental auto-merge handling, queue bootstrap/auth failure and issue alerts. Verify the trusted/untrusted boundary. Keep old scheduled jobs disabled during the new scheduler's activation.
5. **Exercise release packaging.** Build and verify an unsigned universal DMG with SBOMs and checksums. Retain explicit pending status for signing, notarization and replacement provenance until demonstrated separately.
6. **Cut over and remove Actions.** Freeze queue admission briefly, install external required checks, disable GitHub native auto-merge, migrate status consumers/instructions, remove all seven workflow YAML files, disable repository Actions, and revoke Actions-only tokens/secrets. Use a reviewed external/local receipt for this migration commit. Verify zero active workflows/schedules and that no gate or operator command requires `gh run`.
7. **Prove unattended operation.** Observe a maintenance cycle and a normal merge end to end, test missed-event recovery and restore an archived receipt. If external CI is unavailable, pause merging; rollback must not reactivate Actions or bypass checks.

This task delivers the proposal only. Service installation, purchases, secrets, scheduler activation, workflow removal and merge-policy changes belong to the approved implementation.
