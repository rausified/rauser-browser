---
title: GitHub repositories as container artifacts, and scoped tenants as anchors
date: 2026-10-27
status: draft
tags: [brauser, adapters, design]
---

# GitHub repositories as container artifacts

## Intent

John enabled GitHub capture and then worked in a repository: its home page, its files, its PRs. On the home page and the file pages, the panel said "Not an artifact", because the GitHub starters only recognize pull requests and issues. The work he wants remembered is the repository itself, and also the specific PRs and issues inside it.

**The model, in John's words:** "any time you visit anything inside a GitHub repo we should log the specific adapter page and also separately the repo that it's in. We should only log the repo once."

- A visit to PR #42 in `cisco-sbg/cloudsec-skills-marketplace` is recorded against the PR, **and** against the repository.
- A visit to any other page inside the repository (code on any branch, Actions, the PR list) is recorded against the repository only.
- The repository is **one** record. Its focused time, background time, opens, and days are John's real total in that repository. Each second is counted once, never once for the PR and again for the repo.

**Second goal, a product principle: adapter creation should be as easy as possible, with Brauser doing the work.** When enabling GitHub, Brauser asked John to type his work org even though the URL (`github.com/cisco-sbg/…`) named it. Anything the current page already tells us gets pre-filled, and the user's part is to confirm.

## Goals

1. **Containers: a general mechanism, not a GitHub special case.** An adapter can declare that its artifacts sit *within* a container artifact. The container's type and ID template are built from captures the adapter's rules already bind. Example:

   ```json
   "within": { "type": "github.repo", "id": "github.repo:{host}/{owner}/{repo}" }
   ```

   The same shape will later cover a Jira issue in a project, a Confluence page in a space, a Notion page in a workspace, and so on. Validation follows the existing `id` rules (adapter-templates.md §8.1). Every capture the container ID uses must be bound by every rule. The container ID must differ from the adapter's own ID template. It follows the same frozen-once-shipped stability as `id` (§8.2).

2. **Roll-up semantics.** For every visit recognized as artifact A within container C:
   - C gains the visit's focused and background seconds, its opens, and its local day. Days are a set, so C's `days_opened` is the union of its own days and its contained days, never a sum.
   - C's record holds this roll-up as a single total: its own pages plus everything within it, each second counted once. A visit is recognized by exactly one adapter (first match wins, §3), so it contributes to C exactly once.
   - A gets nothing extra. Its record is unchanged from today.
   - The relationship is visible: A's card names its container (link to C), and C's card lists the contained artifacts John worked on (reusing the edges table with a new kind, e.g. `within`, alongside `mentions`).
   - Recent and the worklog show C as one row with its rolled-up totals, and A as its own row.
   - C's title is `owner/repo` (see goal 4). A container that has rolled-up activity but no direct visit yet still gets a title from its ID and a URL built from its captures (`https://github.com/<owner>/<repo>`), so its card and Recent row are usable.
   - **Existing history rolls up (my call; drop it if infeasible).** John's existing PR and issue visits should count toward their repositories as soon as this ships. The index is regenerated from the append-only activity logs, so the preferred mechanism derives the container when the index is built, not when the event is written, giving the roll-up retroactively for free. The builder decides the mechanism, but must report in the PR whether history rolls up.

3. **A `github.repo` starter.** Every page inside `github.com/<owner>/<repo>/…` that no PR or issue adapter claims is a repository visit. The rule is a catch-all, `/{owner:name|lower}/{repo:name|lower}/{*}` (the rest also matches zero segments, so the home page is included), scoped by `owner`. ID: `github.repo:{host}/{owner}/{repo}`. That covers code on any branch, tag, or SHA, blame, history, the PR and issue lists, Actions, wiki, settings, and any section GitHub adds later. Branches never split a repository: John said returning to a file on `main`, a PR branch, or an old commit is "one thing, not a different thing each time".
   - The `github.pr` and `github.issue` starters gain `within: github.repo` (goal 1).
   - **Precedence.** PR and issue adapters must be tried before `github.repo` on `https://github.com`, because adapters on one origin are tried in config order (§3). Every way of adding any of the three (panel flow, settings gallery, JSON editor save) must leave PR and issue ahead of repo, whatever order they were enabled in. Because a PR visit already rolls up into its repo, the repo catch-all claiming a PR URL would only happen through misordering, and that's what this guards against.
   - **The route-history guard needs one amendment.** `validate_adapter_transition_with_history` (`host/src/config.rs`, DESIGN.md §5.1) refuses a new adapter whose routes overlap a retired or removed contract under a different type or ID. Contracts include `scope`, so any past edit to a PR or issue org list leaves a retired contract, and the repo catch-all overlaps it, so enabling repos would be a hard error for John today. Amendment: **an adapter may overlap a retired or removed contract when it is exactly that contract's declared container (same type, same ID template over the same captures).** Rationale: those URLs were already counting toward that container, so its history does not split. Only the finer-grained PR identity stops being recorded, and only if the PR adapter is later removed. Nothing else about the guard changes, and the amendment needs tests in both directions.

4. **Repo title.** The repo's card title is always `owner/repo`. It must not change to whatever sub-page title was seen last (the index keeps the latest title, `host/src/index.rs`). The builder chooses the mechanism: title-template entries verified against GitHub's recorded repo-page title forms, or a small language addition such as a title built from captures. A language addition goes through the both-engines and vectors discipline.

5. **Scoped tenants count as anchors (general language change).** `/{owner}/{repo}/{*}` has no literal word, so §4.1 rejects it today. New rule: **a required capture that `scope` restricts to a literal list counts as an anchor segment**, because a listed tenant (a GitHub org, a Notion or Linear workspace) is effectively a literal the user chose. This is a general rule, not a GitHub exception. John expects it "to happen a lot on many sites", so the docs must say it plainly and use GitHub as the worked example. An `unscoped = "acknowledged"` adapter gets no such anchor, and an unrestricted capture still never anchors.

6. **Org pre-fill.** When GitHub is set up from the panel's Capture Site flow on a GitHub page, the org comes from the current URL's owner segment (also from `/orgs/<org>/…`). The org is pre-filled, and the user confirms or edits it. The flow has one org field shared by repositories, PRs, and issues, replacing today's separate PR and issue fields, and all three are checked by default. In settings, the gallery pre-fills new GitHub adapters with the union of orgs already configured on GitHub adapters. It never rewrites an existing adapter's scope. A page with no usable owner (`github.com/`, reserved top-level routes such as `/settings`, `/notifications`, `/search`, `/orgs` with no org) gets no pre-fill and asks instead.

7. **Existing users can add repos in one step.** John already has GitHub capture rules, so the panel says "Manage them in settings." Settings must offer `github.repo` pre-filled from his existing scope. Adding it also offers to add `within` to his existing PR and issue adapters. That is a contract change, so it goes through whatever confirmation the host already requires for one. It must not trip the route-history guard (see the amendment above).

## Non-goals

- Files as their own artifacts. Deferred. If they're ever added, John's answer sets the identity: a file is keyed by repo and path, ignoring branch, and the file would also sit `within` its repo.
- Nesting deeper than one level (container of a container), and an artifact with more than one container.
- Adding `within` to Jira and SharePoint now. The mechanism is general, but only GitHub uses it in this change.
- GitHub Enterprise on other origins, gists, user and org profile pages, and repository rename aliases (those wait for the alias flow, DESIGN.md §5.1 M1.5b).
- Changing the Tier 2 drafter's rule that a `name` capture is not an *identifier* (§10.4). This changes only *anchors* (§4.1).

## Anti-goals

- **Never double count.** A second spent on a PR appears once in the PR's total and once in the repo's total. It never appears twice in the repo's total, and the repo's total is never "repo pages + PRs + repo again".
- **Never widen capture silently.** Scope on a shared origin is a privacy boundary. A pre-filled org is shown and confirmed, never applied unseen, and widening still goes through the existing confirmation path (§6). The confirmation names the owner plainly, because the URL can't tell a person from an org.
- **Repo capture never swallows pages outside a repo.** Only `/<listed-org>/<repo>/…` matches. `/<listed-org>` alone (the org page) and everything outside listed orgs stay non-artifacts.
- **No engine drift.** The worker (TS) and host (Rust) must accept and reject the same configs and produce the same IDs and container IDs. The shared conformance vectors are the proof.

## Constraints

- **Both engines change.** The anchor rule lives in `extension/adapter-templates.ts` (`isAnchorSegment`, checked in `compileRule`) and in the Rust crate `adapter-templates/`. The normative validation order and error codes live in `protocol/adapter-templates/README.md`, with vectors in `protocol/adapter-templates/vectors.json`. `no_literal_segment` is checked per rule (step 3.3.5) before `scope` is validated, so the new rule needs `scope` in view. The builder decides where that check moves and whether it keeps its code. Specify the order normatively, and cover it with vectors in both directions. `within` validation also gets vectors. Amending format 1 rather than bumping it is acceptable; the fixed-shape anchor amendment set that precedent.
- **The host stays authoritative** (§10.2). The worker only prefilters. Container derivation is a host or index concern; the worker may compute it only for display.
- **Protocol.** If `within` needs to reach the panel (the card's container link and contained list), the `get_artifact` and `list_artifacts` shapes in `protocol/schema.json` change. That is a protocol version bump under the repo's rules (DESIGN.md: "any later schema change … takes the next version").
- **Scope misses are terminal** (`evaluate`, `extension/adapter-templates.ts`). If the PR scope is `[a]` and the repo scope is `[a, b]`, a PR in org `b` is dropped rather than falling to the repo. The shared org field makes this unlikely. Accept it, document it in `docs/adapters/github.repo.md`, and pin it with a test.
- **Docs to update:**
  - adapter-templates.md: §3 (`within` key), §4.1 (anchor rule), §6 (scope as anchor, GitHub worked example), §8 (container ID), §11 (new starter)
  - DESIGN.md: §5 (artifacts gain containers, the roll-up rules, the starter list), §5.1 (guard amendment), §6.2 (record keys), and the M1.5a / step 6.8 notes
  - a new `docs/adapters/github.repo.md`, plus updates to `github.pr.md` and `github.issue.md` for `within`
  - the protocol README validation order

  Mark `github.repo` as a development draft until it has had a live GitHub UI check, like the others.
- **Concurrent work.** The Cookie overnight worker opens PRs here (#92 and #93 are open), and peer sessions have several feature branches checked out in worktrees. Branch fresh from `origin/main`, and check for overlap on `DESIGN.md`, `host/src/index.rs`, `host/src/config.rs`, and `extension/capture-site*.ts` before editing.
- **Live install.** The live extension loads `extension/dist`, and the native host manifest must point at a build with a matching protocol. Rebuild both after a protocol bump, and the user has to reload the extension in `chrome://extensions`.

## Decided (John)

- Repositories are artifacts. Files are not, for now.
- A repository is one artifact regardless of branch or commit.
- Every visit inside a repo is logged against the specific artifact (PR, issue) and also against the repo, and the repo is counted once.
- A scoped org name works as the literal part of the URL. This is a general rule for shared sites, and it must be clearly documented.
- Pre-fill the org from the URL, and do the work on our side.

## My calls (flag if wrong)

- `within` is a general adapter mechanism, used only by GitHub for now.
- Existing PR and issue history rolls up into repos, if the index-time derivation allows it.
- A catch-all repo rule, with PR and issue ordered first and the guard amended for containers. This replaces the earlier enumerated-routes idea, so new GitHub sections count automatically.
- One shared org field in Capture Site, with all three types checked by default. Settings seeds new adapters with the union of existing orgs and never rewrites existing scopes.
- Pre-fill whatever owner is in the URL, personal accounts included, and name it plainly in the confirmation.
- A scope mismatch between the PR and repo adapters drops the PR page instead of falling through (documented limit).

## Left to the builder

- Event-time vs. index-time container derivation (index-time preferred, for retroactive roll-up).
- How the `owner/repo` title is produced.
- Where the anchor check moves in validation order, and its error code.
- How config order is enforced (insert position vs. reorder on save).
- The reserved top-level GitHub route list for pre-fill.
- Panel presentation of container and contained artifacts, within DESIGN.md's existing card style. Read DESIGN.md before any panel change.
- Test design. At minimum:
  - conformance vectors (anchor rule, `within`)
  - engine parity on example URLs:
    - home page, `blob` on a slashed branch, `tree` at a SHA, the `pulls` list → repo
    - `pull/7/files` and `issues/3` → PR or issue, within the repo
    - `/cisco-sbg`, `/other-org/x`, `/settings` → not an artifact
  - roll-up totals: PR + repo-page time sums once into the repo; days union; the PR total is unchanged
  - precedence across enable orders
  - the guard amendment, in both directions
  - the pre-fill test
