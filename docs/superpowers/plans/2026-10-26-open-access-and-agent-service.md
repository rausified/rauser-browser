# Brauser Open Access, Service, and Summarize & Log Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Brauser gets standing HTTP(S) access with no grant wall, and a per-user macOS service runs full-access Codex or Claude Code agents. These agents summarize any page against the user's knowledge base into an owned summary file, a daily-log event and a `web.page` artifact.

**Architecture:** PR 1 removes the privacy and consent machinery and moves to protocol v6. A site counts as on when it has an adapter, and a knowledge base folder is chosen with the native picker. PR 2 adds `brauser service`, a launchd-managed process on a 0700 Unix socket that owns run records, the queue, timeouts, cancel and recovery. The Chrome-spawned host forwards run messages to it. PR 3 adds the real runners, the §6.4 prompt and answer validation, Readability extraction and the polling run card. PR 4 writes the summary file and log event under the configuration current at finish, and integrates summaries into the index, Recent, the worklog and the panel.

**Tech Stack:** Rust (the `brauser` binary and `brauser-protocol`, plus `rustix`, `cap-std`-style `Dir` vault, `serde`, `toml`, `time`, `uuid`), a JSON Schema protocol generated to Rust and TypeScript by `protocol/generate.py`, a Chrome MV3 extension in TypeScript, macOS launchd, `@mozilla/readability` 0.6.0 (pinned and vendored), and the Node headless smoke `scripts/smoke-macos.mjs`.

**Spec:** docs/superpowers/specs/2026-10-26-open-access-and-agent-service-design.md

## Global Constraints

- Principle 9: nothing may assume AMOS/ENDURANCE, Backstaff, DISCOVERY, or the author's paths. Defaults stay neutral, and the knowledge base folder is a user choice. DISCOVERY is prior art only; copy no code from it.
- macOS is the supported platform. Existing Windows code paths must not break, and Windows CI (`brwr-bzx`), if it lands first, must still pass. The `service` module compiles out (`#[cfg(target_os = "macos")]`) on other platforms.
- Rust `unsafe` code goes only in a small dedicated crate, with a `SAFETY` comment on each block. This plan adds no `unsafe` code.
- Every new crate or crate feature must pass `cargo deny check --all-features --locked`. The sandbox cannot fetch crates or npm packages, so any new dependency is a **[human]** step.
- Protocol version: **6** (`brauser_protocol::PROTOCOL_VERSION == 6`, TS `PROTOCOL_VERSION = 6 as const`).
- Full quality gate (spec §8). Every PR ends with all of these green:
  ```sh
  python3 protocol/generate.py --check
  python3 scripts/check_current_state.py
  cargo fmt --all -- --check
  cargo build --locked --workspace --all-targets
  cargo test --locked --workspace
  cargo clippy --locked --workspace --all-targets -- -D warnings
  cargo test --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs
  cargo clippy --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs --all-targets -- -D warnings
  cargo deny check --all-features --locked
  npm ci --ignore-scripts && npm run typecheck && npm run lint
  npm run build:extension && npm run test:extension
  npm run test:scripts            # from Task 25 on
  cargo build --locked -p brauser && npm run test:adapter-diff -- --cases 5000
  npm audit                       # from Task 38 on
  cargo build --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs && npm run smoke:macos:headless
  ```
- Non-macOS type check. When a non-macOS Rust target is installed (`rustup target list --installed`), also run `cargo check --locked --workspace --target <that target>`. If none is installed, the CI Windows job covers it; record it as a **[human]**/CI check in the task.
- Tests never run a real harness, `launchctl`, or `/usr/bin/open`. They never use a real HOME, and they never mutate the test process's environment.
- Line references are deliberately avoided. Anchors are symbol names (`fn compile_validated_adapters`), smoke step titles (`step("Confirmed setup")`) and element ids. The smoke dialog counter rule is fixed: `dialogsShown` must equal the number of scripted dialog answers the smoke gives, and the final assertion compares it with the dialog log's line count. It never uses a hard-coded total.

## Review Focus

These are the five failure modes the spec implies most strongly but the draft did not pin, most likely first. Each one now has a pinning test in the task that owns it.

1. **The service is not installed, or is a stale build.** After a rebuild, a LaunchAgent service still running old code also reports v6 and answers new request shapes with old code. Pinned by `stale_service_build_is_restarted` and `persistent_build_mismatch_is_service_unavailable` (Task 23). Also pinned by `a summarize failure sent before the panel loads is shown when it opens` (Task 39), so a `service_unavailable` from the shortcut is not lost.
2. **A page with no readable content, or a page over the caps.** CJK text, or text heavy in quotes and control characters, can push a `start_run` frame past `MAX_INBOUND_BYTES`. Readability can also return nothing. Pinned by `maximal extraction stays under the host frame cap` and `an empty page is unreadable, not an empty run` (Task 38), and by `an executeScript failure shows "can't read this page"` (Task 39).
3. **The agent returns malformed JSON twice, or dies before the repair turn.** Pinned by `second_invalid_answer_fails_invalid_output_and_keeps_stream`, `empty_final_text_gets_a_repair_turn` and `repair_turn_after_harness_exit_fails_as_harness_failed` (Task 33).
4. **The notes folder is unmounted mid-run.** The run must fail with Retry save, and a retry after remount must succeed without rerunning the agent. Pinned by `an_unmounted_then_restored_notes_folder_fails_then_retry_save_succeeds` and `save_never_touches_the_page_note` (Task 49).
5. **The user withdraws Chrome site access.** The panel must show the one banner on HTTP(S) tabs, must not show it on chrome:// tabs while access is granted, and must keep the note editor usable once access returns. Pinned by `a chrome:// tab with access granted shows no site-access banner` and `withdrawing then restoring access hides the banner without a reload` (Task 6).

---

## PR 1: Open access and removals, protocol v6

PR 1 gives Brauser standing HTTP(S) access at install. It removes the per-site grant wall, grant reconciliation, the native config confirmation, the AI denylist, `log_incognito`, the sync caution, and the harness token with its native dialog. Harness discovery and the probe stay. `sites` stops being a separate gate and becomes the set of adapter origins. Existing v5 `config.toml` files load unchanged and drop the retired keys on their next save. A knowledge base folder setting (§6.1) is added through the existing native folder picker. It uses a token that is checked against the folder's purpose.

Each task is one vertical slice. It edits `protocol/schema.json`, regenerates the bindings, and updates the host, the extension and `scripts/smoke-macos.mjs` together, so every commit keeps `cargo test`, `npm run typecheck` and the headless smoke green.

Commands used throughout:
- **Regenerate:** `python3 protocol/generate.py && python3 protocol/generate.py --check`
- **Extension:** `npm run build:extension && npm run test:extension`
- **Smoke:** `cargo build --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs && npm run build:extension && npm run smoke:macos:headless`

---

### Task 1: Bump the protocol to version 6

**Files:**
- Modify `protocol/schema.json`: change every `"protocol_version": {"const": 5}` to 6, and update the top-level description.
- Regenerate: `protocol/src/generated.rs`, `protocol/ts/generated.ts`
- Modify: `extension/native.ts` (the `PROTOCOL_VERSION` literal), `scripts/smoke-macos.mjs` (`const PROTOCOL_VERSION`)
- Modify `protocol/README.md`: add a "Version 6" paragraph stub, completed in Task 11.
- Test: `protocol/tests/wire.rs` (every `"protocol_version":5` literal and `v5_*` test name); `host/src/native.rs` tests (beside `protocol_v3_request_gets_unsupported_version`)

**Interfaces:**
- Produces: `brauser_protocol::PROTOCOL_VERSION == 6`; TS `PROTOCOL_VERSION = 6 as const`

- [ ] Write the failing test in `host/src/native.rs`, copying the v3 test's shape:
  ```rust
  #[test]
  fn protocol_v5_request_gets_unsupported_version() {
      let folder = tempfile::tempdir().unwrap();
      let config = ConfigStore::for_test(folder.path().join("config.toml"));
      let request = r#"{"type":"get_config","protocol_version":5,"request_id":"old-5"}"#.to_owned();
      match &served(&[request], config)[0] {
          Response::Error(value) => {
              assert_eq!(value.code, ErrorCode::UnsupportedProtocolVersion);
              assert_eq!(value.request_id, "old-5");
          }
          other => panic!("expected version error, got {other:?}"),
      }
  }
  ```
- [ ] Run `cargo test --locked -p brauser protocol_v5_request_gets_unsupported_version`. Expect FAIL, because v5 is still current and the response is `ConfigResult`.
- [ ] Implement: replace every `"const": 5` with `"const": 6`, then regenerate. Update the `native.ts` literal (`npm run typecheck` fails until it matches the generated constant) and the smoke constant. Rename the `wire.rs` literals and `v5_*` tests to v6.
- [ ] Run the regenerate command, `cargo test --locked --workspace`, `npm run typecheck`, the extension command and the smoke command. Expect PASS.
- [ ] Commit: `git add protocol/schema.json protocol/src/generated.rs protocol/ts/generated.ts protocol/tests/wire.rs protocol/README.md extension/native.ts scripts/smoke-macos.mjs host/src/native.rs && git commit -m "feat(protocol)!: bump native messaging protocol to v6"`

---

### Task 2: Remove the AI denylist and `log_incognito` while v5 config files keep loading

**Files:**
- Modify `protocol/schema.json`, then regenerate:
  - `ConfigSnapshot`: drop `agent_denylist`, `agent_denylist_confirmed` and `log_incognito` from `properties` and `required`.
  - `AgentState`: drop `denylist_unconfirmed`.
  - `CheckAgentRequest`: drop `url`.
  - `AgentChecked`: drop `url_allowed`.
  - `ConfirmHarnessSetupRequest`: drop `agent_denylist`.
- Delete: `host/src/privacy.rs`
- Modify:
  - `host/src/lib.rs`: drop `pub mod privacy;`.
  - `host/src/config.rs`: the `privacy` import; drop `CONFIRM_DENYLIST`; `fn agent_gate`; `fn update_with_grants` (denylist handling); `struct StoredConfig`; `fn from_snapshot`; `impl From<StoredConfig>`; `fn empty_config`; `fn validate_base` (`log_incognito` rule); `fixtures::ready`.
  - `host/src/consent.rs`: `fn check_agent_fields` loses its denylist half and keeps "agent changes only through host setup"; the denylist lines in `fn approval_lines` and `fn harness_confirmation_text`.
  - `host/src/native.rs`: `fn check_agent` (drop the URL gate and `url_allowed`); the `privacy` import.
- Modify the extension:
  - `extension/agent.ts`: delete the denylist helpers and the `denylist_unconfirmed` case of the state text.
  - `extension/protocol-shape.ts`: the three config keys and `url_allowed`.
  - `extension/settings.ts`: drop the denylist parameter and field from `setupHarness`.
  - `extension/options.ts`: delete the denylist imports, state, rendering, handlers and the "AI privacy list" section wiring.
  - `extension/options.html`: delete the "AI privacy list" section.
- Modify `scripts/smoke-macos.mjs`:
  - Delete the steps "AI commands refused until privacy list is confirmed", "Suggestion only fills the box", "Denylist add is immediate, removal asks" and "Denylist stays live after harness removal".
  - Remove `url` from every `check_agent` request.
  - Apply the dialog-counter rule from Global Constraints: replace the hard-coded total in `step("Nothing reached the screen")` with `requireCondition(dialogsShown === dialogLogLines().length, …)`.
- Test:
  - `host/src/config.rs`: replace `log_incognito_true_is_rejected_and_on_disk_true_is_recoverable`.
  - `host/src/native.rs`: delete the denylist `check_agent` tests.
  - `protocol/tests/wire.rs`: rewrite the `PRIVACY` fixture.
  - `extension/tests/agent.test.mjs`: delete the denylist tests and update the config fixture.
  - `extension/tests/options.test.mjs`: `configWith` and the element-id list.
  - `extension/tests/worker.test.mjs`: the config fixture.

**Interfaces:**
- Consumes: `fixtures::write(path, root, storage_extra, top_level_keys, tables)` and `backup_count` in config.rs tests.
- Produces:
  - `StoredConfig` fields `#[serde(default, skip_serializing, rename = "agent_denylist")] _agent_denylist: Option<toml::Value>`, plus the same for `agent_denylist_confirmed` and `log_incognito`. This follows the existing `_near_repeat_secs` pattern.
  - `AgentState` = `not_set_up | harness_problem | ready`.
  - `CheckAgentRequest {request_id}` → `AgentChecked {harness_id, harness_version}`.

- [ ] Write the failing tests:
  ```rust
  // config.rs
  #[test]
  fn v5_config_with_retired_privacy_keys_loads_and_drops_them_on_save() {
      fixtures::write(&path, &root, "", "agent_denylist = [\"bank.example\"]\nagent_denylist_confirmed = true\nlog_incognito = true", "");
      let mut store = loaded(&path);
      assert_eq!(store.config_issue(), None);
      assert!(store.configured());
      assert_eq!(backup_count(dir.path()), 0);
      let rev = store.revision().to_owned();
      store.update(store.snapshot().clone(), &rev).unwrap().unwrap();
      let text = fs::read_to_string(&path).unwrap();
      assert!(!text.contains("agent_denylist") && !text.contains("log_incognito"));
  }
  // wire.rs
  #[test]
  fn v6_config_rejects_retired_privacy_keys() {
      for key in ["agent_denylist", "agent_denylist_confirmed", "log_incognito"] {
          assert!(serde_json::from_str::<Response>(&config_with_extra(key)).is_err());
      }
  }
  ```
  ```js
  // agent.test.mjs
  test("isConfig requires exactly the v6 keys", () => {
    assert.equal(isConfig({ ...config, agent_denylist: [] }), false);
  });
  ```
- [ ] Run `cargo test --locked -p brauser v5_config_with_retired_privacy_keys` and `cargo test --locked -p brauser-protocol v6_config_rejects_retired`. Expect FAIL. The first fails to compile until the fields exist, and the wire still accepts the keys.
- [ ] Implement the schema edits, regenerate, delete `privacy.rs`, and add the three accepted-and-ignored fields. In `agent_gate`, a harness committed by host setup is enough for `ready`. In the extension, remove the denylist UI, its helpers, and the `agent_denylist` argument.
- [ ] Run `cargo test --locked --workspace`, `cargo test --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs`, `npm run typecheck`, `npm run lint`, the extension command and the smoke command. Expect PASS.
- [ ] Commit: `git add -A protocol host/src/lib.rs host/src/config.rs host/src/consent.rs host/src/native.rs host/src/privacy.rs extension/agent.ts extension/protocol-shape.ts extension/settings.ts extension/options.ts extension/options.html extension/tests scripts/smoke-macos.mjs && git commit -m "feat!: remove the AI denylist and log_incognito; accept retired keys in v5 config"`

---

### Task 3: Remove the native confirmation for config changes

**Files:**
- Modify `protocol/schema.json`, then regenerate: delete `ConfirmConfigRequest`, `ConfigConfirmed`, their oneOf entries, and `UpdateConfigRequest.consent_token` (including its `required` entry).
- Modify `host/src/consent.rs`:
  - Delete `ConfigConfirmation`, `ConsentGrant`, the `consent_grants` field and its prune branch, `fn confirm_config`, `fn mint_consent`, `fn approval_lines`, `fn unique_adapter_index`, `fn adapter_approval_identity`, `fn site_covers` and `MAX_CONSENT_SUMMARY_BYTES`.
  - `fn authorize_update` and `fn consume_update` drop the consent token.
- Modify `host/src/native.rs`: delete the `Request::ConfirmConfig` arm; drop `consent_token` from the `Request::UpdateConfig` arm.
- Modify `extension/settings.ts`:
  - `saveConfig`: delete the confirm round trip and `consent_token`.
  - `describe`: `"cancelled"` becomes "Folder selection was canceled. No settings were changed."
- Modify: `extension/protocol-shape.ts` (`config_confirmed`), the comment in `extension/coordination.ts` that mentions confirmation, and `extension/options.ts` (`chooseFolder` cancel copy).
- Modify `extension/options.html`: the intro copy mentioning confirmation; the `Review and save…` button becomes `Save`.
- Modify `scripts/smoke-macos.mjs`:
  - Delete `step("Canceled native consent")`.
  - In `step("Confirmed setup")` and `step("Confirmed adapter becomes the worker and host capture boundary")`, stop answering confirm dialogs.
  - Replace the direct `confirm_config` / `consent_token` exchange with a single `update_config`.
- Test:
  - `host/src/consent.rs` tests: delete the consent and unscoped-adapter tests; keep `fresh_picker_token_can_reconfirm_the_same_path`.
  - `host/src/native.rs` tests.
  - `protocol/tests/wire.rs` `fn update()`.
  - `extension/tests/options.test.mjs`: remove the `confirm_config` host default and rewrite the "saves after native confirmation" test.
  - `extension/tests/worker.test.mjs`.

**Interfaces:**
- Produces:
  - `ConsentAuthority::authorize_update(&self, current: &ConfigSnapshot, revision: &str, next: &ConfigSnapshot, picker_token: Option<&str>, harness_token: Option<&str>) -> Result<Authorized>`
  - `consume_update(...)` with the same token parameters
  - Wire `UpdateConfigRequest {request_id, expected_revision, config, picker_token, harness_token}`

- [ ] Write the failing tests:
  ```rust
  // native.rs
  #[test]
  fn update_config_turning_capture_on_with_a_new_adapter_needs_no_confirmation() {
      // configured_store(root) → get_config → update_config adding one adapter with capture_enabled = true
      assert!(matches!(responses[1], Response::ConfigUpdated(_)));
  }
  #[test]
  fn update_config_still_refuses_a_stale_revision() { /* expected_revision "stale" → ErrorCode::Conflict */ }
  ```
  ```js
  test("saving settings sends one update_config and never confirm_config", async () => {
    /* host has no confirm_config handler, so any call throws "Unexpected host request" */
    assert.deepEqual(hostTypes(), ["get_config", "update_config"]);
    assert.equal("consent_token" in lastUpdate, false);
  });
  ```
- [ ] Run `cargo test --locked -p brauser update_config_turning_capture_on` (FAIL with "native confirmation is required") and the extension command (FAIL, because the page calls `confirm_config`).
- [ ] Implement the deletions. `authorize_update` now calls `config::validate_update`, then the harness-token branch (unchanged until Task 4), then `check_picker` and `check_agent_fields`. The revision check, `validate_config_update` and the adapter-history logic in `update_with_grants` stay untouched.
- [ ] Run `cargo test --locked --workspace`, the scripted-dialogs test, `npm run typecheck`, the extension command and the smoke command. Expect PASS.
- [ ] Commit: `git add -A protocol host/src/consent.rs host/src/native.rs extension scripts/smoke-macos.mjs && git commit -m "feat!: save config with the revision check only; remove confirm_config and consent tokens"`

---

### Task 4: Set up a harness by detecting and probing it, with no native dialog or token

**Files:**
- Modify `protocol/schema.json`, then regenerate:
  - Delete `ConfirmHarnessSetupRequest`, `HarnessSetupConfirmed`, `UpdateConfigRequest.harness_token`, `HarnessOffer.offer_id` and `DiscoverHarnessesRequest.expected_revision`.
  - Add `SetUpHarnessRequest` to the Request oneOf.
- Create `host/src/harness_setup.rs`, which holds the setup flow moved out of `consent.rs`.
- Modify `host/src/consent.rs`:
  - Delete `HarnessOfferGrant`, `HarnessGrant`, `HarnessConfirmation`, the `offer_grants`/`harness_grants` maps, `discover_harnesses`, `offer`, `confirm_harness_setup(_with)`, `check_harness`, `mint_offer`, `mint_harness`, `offered_env`, `bounded`, `CODEX_DISCLOSURE`, `harness_confirmation_text`, `MAX_HARNESS_SUMMARY_BYTES`, `MAX_OFFERS` and `Authorized.harness_commit`.
  - Move `requested_env` and the unsafe-display-path refusal from `offer` into `harness_setup.rs`.
  - Move `DEFAULT_SUMMARIES_DIR` to `config.rs`.
- Modify `host/src/native.rs`:
  - `SetupContext` drops `confirm`.
  - Replace the `DiscoverHarnesses` and `ConfirmHarnessSetup` arms.
  - The `UpdateConfig` arm has no `harness_token`.
- Modify `host/src/config.rs`: update the `HarnessCommit` doc comment. It is now host-internal and passed only by `harness_setup`.
- Modify `extension/settings.ts`: `discoverHarnesses` drops the revision; `setupHarness` changes; remove `harness_token: null`.
- Modify `extension/agent.ts`:
  - `harnessOfferNote` and `harnessFoundText` have no offer IDs and no "Review it below".
  - Delete the extension's `CODEX_DISCLOSURE`.
  - Add `SETUP_WARNING`.
- Modify `extension/options.ts`: `setupHarness`; offers are keyed by `harness_id`.
- Modify `extension/options.html`: `Set up harness…` becomes `Set up harness`, with `#harness-warning` (the `SETUP_WARNING` text) above it.
- Modify `extension/protocol-shape.ts`: `harness_setup_confirmed` becomes `config_updated` for setup.
- Modify `scripts/smoke-macos.mjs`:
  - Delete `step("Harness setup canceled")`.
  - `step("Harness setup confirmed")` and `step("Codex setup confirmed")` answer no dialog.
  - Keep `step("Failing harness refused")`.
- Rewrite: `host/tests/harness_setup.rs` and the `mod harness_setup` tests in `host/src/native.rs`.
- Delete: the harness tests in `consent.rs`.
- Test: `extension/tests/agent.test.mjs`, `extension/tests/options.test.mjs`

**Interfaces:**
- Consumes: `harness::discover(env)`, `harness::probe(...)`, `ConfigStore::update_with_grants(next, rev, selected_identity, harness_commit: Option<HarnessCommit>)`
- Produces:
  - Wire `SetUpHarnessRequest {type:"set_up_harness", protocol_version, request_id, expected_revision: string, harness_id: string, env_names: EnvName[], summaries_dir: string|null}`, answered by the existing `ConfigUpdated {config, revision}`.
  - Wire `DiscoverHarnessesRequest {request_id}` → `HarnessesDiscovered {offers: HarnessOffer[]}`, with no `offer_id`.
  - `pub(crate) fn harness_setup::offers(env: &HarnessEnv) -> Vec<HarnessOffer>`
  - `pub(crate) fn harness_setup::set_up(config: &mut ConfigStore, request: &SetUpHarnessRequest, env: &HarnessEnv) -> Result<String, SetupError>`. In order, it:
    1. refreshes and checks the revision;
    2. refuses a config issue;
    3. rediscovers `harness_id` and refuses a refusal;
    4. probes;
    5. builds `AgentConfig` plus `summaries_dir` (from the request, else the existing value, else `DEFAULT_SUMMARIES_DIR`);
    6. commits through `update_with_grants(.., Some(HarnessCommit{agent, record}))`.
  - TS `HostSession.setupHarness(harnessId: string, envNames: string[], summariesDir: string | null): Promise<void>`
  - TS `export const SETUP_WARNING = "Summaries run an AI agent with full access to your files, on your normal login. A page could contain instructions aimed at the agent. Summarize pages you trust."`

- [ ] Write the failing tests:
  ```rust
  // native.rs mod harness_setup
  #[test] fn set_up_harness_probes_and_commits_without_a_dialog() {
      // fake claude (harness::fake), discover_harnesses then set_up_harness{harness_id:"claude"}
      assert!(matches!(&r[1], Response::ConfigUpdated(u) if u.config.agent.as_ref().unwrap().harness_id == "claude"));
      // get_config → agent_status.state == AgentState::Ready
  }
  #[test] fn set_up_harness_refuses_a_failing_probe_and_leaves_config_unchanged() { /* revision unchanged */ }
  #[test] fn set_up_harness_on_a_stale_revision_is_a_conflict() {}
  #[test] fn update_config_cannot_set_an_agent() { /* ErrorCode::Unauthorized */ }
  ```
  - In `host/tests/harness_setup.rs`, add `harness_setup_commits_after_probe` and `failing_probe_is_refused`. Each asserts that the scripted-dialog log is empty.
  - In `options.test.mjs`, add `harness setup sends set_up_harness and no confirmation`. It asserts that `hostTypes()` includes `"set_up_harness"` and that no request carries `harness_token`. Also add `settings shows the exact setup warning`.
- [ ] Run `cargo test --locked -p brauser set_up_harness` (FAIL: no `SetUpHarness` variant) and the extension command (FAIL).
- [ ] Implement as described in Interfaces. `check_agent_fields` keeps the rule that the extension may only echo `agent` or clear it to null.
- [ ] Run `cargo test --locked --workspace`, `cargo test --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs --test harness_setup`, both clippy commands, `npm run typecheck`, the extension command and the smoke command. Expect PASS.
- [ ] Commit: `git add -A protocol host/src host/tests/harness_setup.rs extension scripts/smoke-macos.mjs && git commit -m "feat!: harness setup is detect + probe with no native dialog or token"`

---

### Task 5: Delete the native Yes/No dialog and the `macos-alert` crate

**Files:**
- Modify `host/src/dialog.rs`. Delete:
  - `CONFIRM_COMMAND`, `CHILD_DIALOG_TIMEOUT`, `CONFIRMED`, `DialogText` and `fn confirm`;
  - the CONFIRM branch of `run_child_command`;
  - `ExternalDialog`/`EXTERNAL_DIALOG` (`exit_when_parent_goes_away` becomes a plain exit);
  - `confirm_in_child` (both cfgs);
  - `parse_confirm_reply`, the rfd `MessageDialog` imports, and the scripted `confirm` branches.
- Modify:
  - `host/src/main.rs`: dispatch only `PICK_FOLDER_COMMAND`.
  - `host/Cargo.toml`: drop `brauser-macos-alert` under the macOS target.
  - `Cargo.toml`: drop the `macos-alert` member.
  - `Cargo.lock`.
- Delete: `macos-alert/`, `scripts/check-macos-alert.mjs`
- Modify: `package.json` (drop `check:macos-alert`), and the `macos-alert` mentions in `README.md`, `TESTING.md` and `AGENTS.md`.
- Test `host/tests/scripted_dialogs.rs`: delete `harness_confirmation_text_round_trips`; keep the picker tests.

**Interfaces:**
- Produces: `dialog::pick_folder(title: &str) -> Result<Option<PathBuf>>` is the only dialog. `brauser __dialog-confirm` exits non-zero with "unknown command".

- [ ] Write the failing test:
  ```rust
  #[test] fn retired_confirm_child_command_is_refused() {
      let out = Command::new(HOST).arg("__dialog-confirm").env_clear().output().unwrap();
      assert!(!out.status.success());
      assert!(String::from_utf8_lossy(&out.stderr).contains("unknown command"));
  }
  ```
- [ ] Run `cargo test --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs --test scripted_dialogs retired_confirm`. Expect FAIL.
- [ ] Implement the deletions. Run `cargo build --workspace` once without `--locked` so `Cargo.lock` drops the crate. This only removes entries; nothing is fetched.
- [ ] Run `cargo build --locked --workspace --all-targets`, `cargo test --locked --workspace`, the scripted-dialogs test and clippy, `cargo deny check --all-features --locked` and the smoke command. Expect PASS.
- [ ] Commit: `git add -A host/src/dialog.rs host/src/main.rs host/Cargo.toml Cargo.toml Cargo.lock macos-alert scripts/check-macos-alert.mjs package.json README.md TESTING.md AGENTS.md host/tests/scripted_dialogs.rs && git commit -m "refactor!: remove the native confirmation dialog and macos-alert crate"`

---

### Task 6: Standing site access: manifest, one site-access banner, no Chrome grant requests

**Files:**
- Modify `extension/manifest.json`:
  - `permissions` becomes `["sidePanel","nativeMessaging","storage","contextMenus","alarms","idle","webNavigation","scripting"]`. `contextMenus` is **kept** so Task 39 only adds a menu item. `scripting` is granted ahead of use (spec §4); Task 11 records that in DESIGN §8.
  - `host_permissions` becomes `["http://*/*","https://*/*"]`.
  - Delete `activeTab`, `optional_permissions`, `optional_host_permissions` and `commands.note`.
  - Keep `"incognito": "not_allowed"`.
- Delete: `extension/grants.ts`
- Modify: `extension/build.mjs` (drop `'grants.js'` from the module list)
- Modify `extension/worker.ts`: delete the `note` command listener and the note context-menu item (create and click). Keep `setPanelBehavior` and any `contextMenus.removeAll` teardown.
- Modify `extension/panel.ts`:
  - Delete the `grants.js` import and `noteUnavailable`, `noteGrantOrigin`, `grantOrigin`, `refreshGrantButton` and `requestNoteOrigin`, plus their listeners.
  - Replace the "can't see this tab's address" branch with `refreshSiteAccess`.
- Modify `extension/panel.html`: delete the grant button and the address-unavailable note. Add `#site-access-warning` containing `#site-access-reenable`.
- Modify `extension/panel.css`: add `.site-access-warning`, reusing the `.setup-warning` tokens. Read `DESIGN.md` first.
- Modify `extension/chrome.d.ts`: add `tabs.create`; drop `permissions.request`/`remove` once they are unused, and keep `permissions.contains`/`onAdded`/`onRemoved`.
- Modify `extension/options.ts`:
  - Delete the `grants.js` import.
  - In `enableSite`, delete the `permissions.request` call, `rollbackNewPermissions`, `forgetNoteOrigin` and `runtime.reload`.
  - Delete the Chrome-grant removal in `removeSite`, the `permissions.onAdded`/`onRemoved` listeners and `pruneNoteOrigins`.
- Modify `scripts/smoke-macos.mjs`:
  - Delete the `developerPrivate.addHostPermission` pre-grants and the extension reload, plus the `chromeGrants`/rollback assertions.
  - Add `step("Panel shows URL and title on a never-visited page")`.
- Create: `extension/tests/panel.test.mjs` (FakeElement and fake-chrome pattern from `options.test.mjs`)
- Modify: `package.json` `test:extension` (append `extension/tests/panel.test.mjs`); `extension/tests/options.test.mjs` (delete the `grants.js` test and the `NOTE_ORIGINS_KEY` fixtures)

**Interfaces:**
- Produces: `async function refreshSiteAccess(tab: chrome.tabs.Tab | undefined): Promise<void>` in `panel.ts`, re-run on `chrome.permissions.onAdded`/`onRemoved` and on tab activation and update. Let `granted = await chrome.permissions.contains({ origins: ["http://*/*", "https://*/*"] })`:
  - When `tab.url` is defined and HTTP(S): the note editor shows. The banner is hidden even when `granted` is false, because a URL means Chrome granted this site.
  - When `tab.url` is undefined and `granted` is true: the existing "not a web page" state shows, with no banner. This covers chrome://, New Tab, extension pages and file://.
  - When `tab.url` is undefined and `granted` is false: `#site-access-warning` shows "Brauser needs site access — re-enable in Chrome".
  - `#site-access-reenable` runs `chrome.tabs.create({ url: \`chrome://extensions/?id=${chrome.runtime.id}\` })`.

- [ ] Write the failing tests in `panel.test.mjs`:
  ```js
  test("an HTTP(S) tab shows the note editor and no grant or address warning", async () => {
    const page = await install({ tab: { id: 1, url: "https://new.example/a", title: "A" }, contains: true });
    assert.equal(page.el("note-editor").hidden, false);
    assert.equal(page.el("site-access-warning").hidden, true);
    assert.equal(page.calls("permissions.request"), 0);
  });
  test("a tab whose address Chrome withholds while access is withdrawn shows the single banner", async () => {
    const page = await install({ tab: { id: 1, url: undefined }, contains: false });
    assert.equal(page.el("site-access-warning").hidden, false);
    assert.match(page.el("site-access-warning").textContent, /Brauser needs site access — re-enable in Chrome/);
  });
  test("a chrome:// tab with access granted shows no site-access banner", async () => {
    const page = await install({ tab: { id: 1, url: undefined }, contains: true });
    assert.equal(page.el("site-access-warning").hidden, true);
  });
  test("withdrawing then restoring access hides the banner without a reload", async () => {
    /* contains false → banner; flip contains true, fire permissions.onAdded, tab url now defined → banner hidden, editor visible */
  });
  test("re-enable opens this extension's Chrome page", async () => {
    /* click #site-access-reenable */
    assert.deepEqual(page.created, [{ url: "chrome://extensions/?id=test-id" }]);
  });
  ```
  In `options.test.mjs`, add `enabling a site never requests a Chrome permission`, where the fake `chrome.permissions.request` throws.
- [ ] Run the extension command. Expect FAIL: the module is missing, and the page throws "Missing page element" for `site-access-warning`.
- [ ] Implement. The implementation must achieve three things:
  1. No code path calls `chrome.permissions.request` or `remove`.
  2. The panel decides between note editor, not-a-web-page and banner only through `refreshSiteAccess`.
  3. The manifest grants HTTP(S) at install, and `npm run build:extension` emits no `grants.js`.
- [ ] Run `npm run typecheck`, `npm run lint`, the extension command and the smoke command. Expect PASS.
- [ ] Commit: `git add -A extension/manifest.json extension/grants.ts extension/build.mjs extension/worker.ts extension/panel.ts extension/panel.html extension/panel.css extension/chrome.d.ts extension/options.ts extension/tests package.json scripts/smoke-macos.mjs && git commit -m "feat(extension)!: install-time HTTP(S) access; one site-access banner replaces per-origin grants"`

---

### Task 7: Remove grant reconciliation from the worker and settings; carry pending removals across the upgrade

**Files:**
- Modify `extension/worker.ts`:
  - Delete `readRevocations`, `sameSite`, `withSites`, `validSite`, `readLocallyRemovedSites`, `matchesAnySite`, `checkedGrant` and `purgeRevokedQueueWithoutPolicy`.
  - Replace `reconcileGrants` with `activeLease` at every caller.
  - Delete the removed-sites part of `installPolicy`.
  - Delete the `remove_site`, `ack_reenabled_origin` and `ack_revocations` handlers and both `chrome.permissions` listeners.
  - Drop `revoked_origins`, `navigation_ready` and `locally_removed_sites` from `currentStatus`.
  - `registerNavigationListeners` runs unconditionally at top level, without `?.` guards on `chrome.webNavigation`.
  - Add the upgrade migration `migratePendingRemovals` (below).
  - `onInstalled` removes `note_origins_v1` immediately.
- Modify:
  - `extension/model.ts`: `WorkerStatus` and `WorkerRequest` drop the revocation fields and kinds, and gain `pending_removals: string[]`.
  - `extension/chrome.d.ts`: `webNavigation` is no longer optional.
  - `extension/settings.ts`: delete `reconcileRevocations` and its call, and `sameSite`. Add `applyPendingRemovals`.
  - `extension/panel.ts`: delete the revocation acknowledgement call and its messages, and fix the comment about revocations.
  - `extension/options.ts`: `removeSite` no longer sends `remove_site`; delete the locally-removed note. On load, run `applyPendingRemovals` after `get_config`.
- Test:
  - `extension/tests/worker.test.mjs`: delete the revocation and grant tests and the `grants`/`permissionChecks` fixtures; retitle the "grant-gated capture" test.
  - `extension/tests/options.test.mjs`: `statusFor` and the `remove_site` stub.

**Interfaces:**
- Produces:
  - `async function activeLease(): Promise<PolicyLease | null>` in `worker.ts`. It returns null when there is no policy or a pause is pending. When `!capture_enabled` it clears the queue, calls `narrowActivity(null)` and returns null. It never calls `chrome.permissions`. It drops every artifact whose `origin` is in `PENDING_REMOVALS_KEY`.
  - `const PENDING_REMOVALS_KEY = storageKey("pending_site_removals_v1")`.
  - `async function migratePendingRemovals(): Promise<void>`, run from `onInstalled` (reason `update`) and once at worker start:
    1. Read the legacy `revocations_v1` and `locally_removed_sites_v1` origins.
    2. Union them into `PENDING_REMOVALS_KEY`.
    3. Delete only the two legacy keys.
  - `ConfigSession.applyPendingRemovals(config, revision): Promise<boolean>` in `settings.ts`. It reads `pending_removals` through `get_status`. When the list is non-empty, it sends one `update_config` that removes every adapter whose origin is listed, and then sends `{kind:"clear_pending_removals"}` to the worker. The worker clears the key only once a received lease has no artifact on those origins.

- [ ] Write the failing tests in `worker.test.mjs`:
  ```js
  test("a matching adapter is captured without any permission check", async () => {
    chrome.permissions.contains = () => { throw new Error("no permission checks"); };
    /* install lease(), commit a navigation on the adapter origin */
    assert.equal((await send({ kind: "get_status" })).queued, 1);
  });
  test("a capture-disabled lease clears the queue and blocks capture", async () => {
    /* lease({ capture_enabled: false }) */ assert.equal(status.queued, 0);
  });
  test("worker status carries no revocation fields", async () => {
    const s = await send({ kind: "get_status" });
    for (const k of ["revoked_origins", "locally_removed_sites", "navigation_ready"]) assert.equal(k in s, false);
  });
  test("pending local removals survive upgrade until the host drops those adapters", async () => {
    /* storage: locally_removed_sites_v1 = ["https://a.example"]; lease has an adapter on a.example
       → fire onInstalled({reason:"update"}); navigation on a.example → queued 0; pending_removals == ["https://a.example"];
       clear_pending_removals with a lease still holding a.example → key kept;
       new lease without a.example + clear_pending_removals → key removed */
  });
  ```
  In `options.test.mjs`, add `settings applies pending removals with one update_config`.
- [ ] Run the extension command. Expect FAIL.
- [ ] Implement. The implementation must achieve three things:
  1. No worker or settings code reads Chrome grants.
  2. Capture stays blocked on legacy-removed origins until the host config no longer has their adapters.
  3. `webNavigation` listeners are always registered.
- [ ] Run `npm run typecheck`, `npm run lint`, the extension command and the smoke command. Expect PASS.
- [ ] Commit: `git add extension/worker.ts extension/model.ts extension/chrome.d.ts extension/settings.ts extension/panel.ts extension/options.ts extension/tests && git commit -m "refactor(extension)!: drop Chrome grant reconciliation; carry pending site removals across upgrade"`

---

### Task 8: A site is on when it has an adapter (settings page)

**Files:**
- Modify `extension/options.html`. Read `DESIGN.md` first.
  - Replace `#site-url`, `#site-path` and `#enable-site` with one `#site-origin` input labeled "Site address". Both the starter gallery and New adapter use it.
  - `#sites-list` becomes derived, and each row gets a "Remove site" button.
  - Add `#resume-capture` beside `#pause-capture`.
  - Rewrite the sites intro.
  - Delete the sync caution.
- Modify: `extension/panel.css` (delete `.sync-caution`)
- Modify `extension/options.ts`:
  - Delete `parseSite`, `refreshPreflight`, the `preflight` state, `enableSite` and `removeSite`.
  - `readAdapterCandidate`: drop the origin-in-sites gate.
  - `updateControls`: New adapter no longer requires a site.
  - `beginAdapterEdit`: the default origin comes from `#site-origin`.
  - `renderStarterGallery` and `currentStarterOffer` take the origin.
  - `renderConfig` and `showConfigState`.
  - `saveAdapterDocument`.
- Modify `extension/settings.ts`: add `siteOrigins` and `captureStateText`; delete `adaptersForSites`; `setupProblem` keeps only the folder and repair cases.
- Modify: `extension/starter-adapters.ts` (`starterOffers` signature)
- Modify `scripts/smoke-macos.mjs`:
  - In `step("Confirmed setup")`, enable the site by saving a starter adapter for `#site-origin`, instead of driving `#site-url`/`#enable-site`.
  - `step("Site removal")` clicks that row's Remove site.
  - Retitle `step("Choosing a folder saves it before any site")` to "Choosing a folder saves it before any adapter".
- Test:
  - `extension/tests/options.test.mjs`: delete the site-enable, preflight and path-prefix tests; rewrite the site-list, removal and starter tests; update `typeSite`/`removeListedSite` and the id list.
  - `extension/tests/starter-adapters.test.mjs`.

**Interfaces:**
- Produces:
  - `export function siteOrigins(adapters: AdapterDocument): string[]`: the sorted, distinct `artifact.origin` values.
  - `export function setupProblem(config, issue): string | null`: only "The host configuration needs repair: …" or "No notes folder is chosen.".
  - `export function captureStateText(config): string`: "Capture is paused." or "Capturing on sites with adapters.".
  - `export function starterOffers(origin: string, githubOrganizations: string[]): StarterOffer[]`
  - The `saveAdapterDocument` rule:
    - When the current adapters are empty, `storage` is set, the result is non-empty and no pause is pending, the save sets `capture_enabled: true`.
    - Every save writes `sites: siteOrigins(next).map(origin => ({ origin, path_prefix: "/" }))`. This transitional line is removed in Task 9.
  - `resumeCapture()` saves `{...config, capture_enabled: true}`.

- [ ] Write the failing tests:
  ```js
  test("the site list is the distinct adapter origins", async () => {
    const page = await install({ adapters: [jira("https://a.example"), gdoc("https://b.example"), jira2("https://a.example")] });
    assert.deepEqual(page.listText("sites-list"), ["https://a.example", "https://b.example"]);
  });
  test("saving the first adapter turns its site and capture on, with no permission request", async () => {
    /* type https://acme.atlassian.net into #site-origin, choose the Jira starter, Save */
    assert.equal(lastUpdate.config.capture_enabled, true);
    assert.deepEqual(lastUpdate.config.sites, [{ origin: "https://acme.atlassian.net", path_prefix: "/" }]);
  });
  test("removing a site removes all its adapters", async () => { /* lastUpdate.config.adapters.artifacts has none for that origin */ });
  test("resume capture saves capture_enabled true", async () => {});
  test("setupProblem ignores sites and pause", () => {
    assert.equal(setupProblem({ ...configured, capture_enabled: false, adapters: { format: 1, artifacts: [] } }, null), null);
  });
  ```
- [ ] Run the extension command. Expect FAIL: there is no `site-origin` element and `siteOrigins` is not exported.
- [ ] Implement as described in Interfaces.
- [ ] Run `npm run typecheck`, `npm run lint`, the extension command, `cargo build --locked -p brauser && npm run test:adapter-diff` and the smoke command. Expect PASS.
- [ ] Commit: `git add extension/options.html extension/options.ts extension/settings.ts extension/starter-adapters.ts extension/panel.css extension/tests scripts/smoke-macos.mjs && git commit -m "feat(settings)!: a site is on when it has an adapter; remove the site enable flow and sync caution"`

---

### Task 9: Drop `sites` from protocol v6; the host derives capture origins from adapters

**Files:**
- Modify `protocol/schema.json`, then regenerate: remove `ConfigSnapshot.sites` (and its `required` entry) and the `SiteConfig` def.
- Modify `host/src/config.rs`:
  - `StoredConfig.sites` becomes `#[serde(default, skip_serializing, rename = "sites")] _sites: Option<toml::Value>`.
  - Update `from_snapshot` and `From<StoredConfig>`.
  - `fn validate_base`: delete `MAX_SITES` and `validate_site`; the capture rule becomes "capture requires a selected notes folder".
  - `fn compile_validated_adapters`: delete the "origin … is not enabled for capture" loop.
  - Add `capture_origins`.
- Modify `host/src/capture.rs`:
  - `CapturePolicy.sites` becomes `origins`.
  - `fn is_authorized` becomes an exact-origin membership test.
  - **Delete** `fn url_allowed` and replace its callers with `policy.origins.iter().any(|o| o == &origin)`.
  - Update every `CapturePolicy { sites: … }` construction.
- Modify `host/src/native.rs`: every `CapturePolicy` construction uses `origins: &config.capture_origins()`.
- Modify the test fixtures: the `sites = [...]` TOML lines in `native.rs` test helpers, and the `SiteConfig` literals in `capture.rs` tests.
- Modify `extension/model.ts`:
  - `PolicyLease` drops `sites`.
  - Replace `matchingSite`/`siteMatchesUrl` with `leaseActive` and `captureUrl`.
  - `isPolicyLeaseShape` drops the sites checks and keeps the per-artifact `origin` string check. A stored lease that still has `sites` is accepted, and the key is ignored.
- Modify `extension/worker.ts`: `legacyPolicyLease`, `matchingArtifact`, `leaseNeedsRenewalFor`.
- Modify: `extension/settings.ts` (`leaseFor`), `extension/options.ts` (`saveAdapterDocument` stops writing `sites`), `extension/protocol-shape.ts` (config keys), `scripts/smoke-macos.mjs` (drop `sites` from hand-built configs).
- Test: `host/src/config.rs`, `host/src/native.rs`, `protocol/tests/wire.rs`, `extension/tests/worker.test.mjs` (`lease()`)

**Interfaces:**
- Produces:
  - `pub(crate) fn capture_origins(config: &ConfigSnapshot) -> Vec<String>`: the distinct `Adapter::origin()` values from `compile_adapters`, or empty while `adapter_issue` is set.
  - `ConfigStore::capture_origins(&self) -> Vec<String>`.
  - `CapturePolicy<'a> { enabled: bool, origins: &'a [String], .. }`.
  - TS `export function leaseActive(lease: PolicyLease, now = Date.now()): boolean`, which is `capture_enabled && expires_at > now`.
  - TS `export function captureUrl(raw: string): URL | null`: HTTP(S) only, with no username or password.

- [ ] Write the failing tests:
  ```rust
  // config.rs
  #[test] fn v5_sites_key_is_accepted_and_dropped_on_save() {
      fixtures::write(&path, &root, "", "capture_enabled = true\nsites = [{ origin = \"https://work.example\", path_prefix = \"/allowed\" }]", "");
      /* config_issue() == None; after update the file has no "sites" */
  }
  #[test] fn capture_enabled_needs_only_a_notes_folder() { /* validate_base(enabled, no adapters) is Ok */ }
  // native.rs
  #[test] fn adapter_origin_is_captured_without_a_sites_entry() { /* write_adapter_config w/o sites → record_activity Persisted */ }
  #[test] fn host_still_suppresses_deliveries_outside_every_adapter_origin() { /* other origin → Suppressed/Rejected */ }
  // wire.rs
  #[test] fn v6_config_rejects_sites() {}
  ```
  ```js
  test("a stored v5 lease that still has sites captures its adapters", async () => {
    /* storage holds { ...lease(), sites: [{ origin, path_prefix: "/" }] } */ assert.equal(status.queued, 1);
  });
  test("an expired lease blocks capture", async () => { /* lease({ expires_at: Date.now() - 1 }) → queued 0 */ });
  ```
- [ ] Run `cargo test --locked -p brauser v5_sites_key` (FAIL: `sites` is still a live field) and the extension command (FAIL).
- [ ] Implement as described in Interfaces. Path-prefix narrowing is intentionally dropped, which Task 11 records. v5 validation already forced every adapter origin into `sites`, so deriving from adapters widens nothing at origin level.
- [ ] Run the regenerate command, then `cargo test --locked --workspace`. That includes the existing `host_suppresses_unrecognized_and_out_of_scope_worker_deliveries` and `host_revalidates_activity_identity_and_trail_sources`, which stay unchanged. Then run both clippy commands, `npm run typecheck`, the extension command and the smoke command. Expect PASS.
- [ ] Commit: `git add -A protocol host/src extension scripts/smoke-macos.mjs && git commit -m "feat!: derive capture sites from adapters; drop sites from protocol v6"`

---

### Task 10: A knowledge base folder chosen with the native picker

**Files:**
- Modify `protocol/schema.json`, then regenerate:
  - `StorageConfig`: add a required-nullable `knowledge_base_dir`.
  - Add `$defs.FolderPurpose`.
  - `ChooseFolderRequest`: add `purpose`.
  - `UpdateConfigRequest`: add a required-nullable `knowledge_base_token`.
- Modify `host/src/consent.rs`:
  - `PickerGrant` gains `purpose`.
  - Update `choose_folder` and `mint_picker`.
  - `check_picker` requires `NotesFolder`.
  - Add `check_knowledge_base`.
  - Update `authorize_update`/`consume_update` and `Authorized`.
- Modify `host/src/config.rs`:
  - `StoredStorage` gains `#[serde(default)] knowledge_base_dir: Option<String>`.
  - `StoredConfig` gains a host-only `#[serde(default)] knowledge_base_identity: Option<String>` beside `root_identity`.
  - `validate_storage`: validate the knowledge base path like the root (absolute, bounded), outside the relative-path and overlap loop.
  - `update_with_grants`: verify the knowledge base identity the same way it verifies the root identity.
  - Add `knowledge_base_dir()`.
- Modify `host/src/native.rs`: the `ChooseFolder` arm forwards `purpose`; the `UpdateConfig` arm forwards `knowledge_base_token` and `knowledge_base_identity`.
- Modify every `StorageConfig { .. summaries_dir: None }` literal in the `capture.rs`, `index.rs`, `native.rs`, `config.rs`, `note.rs` and `vault.rs` tests: add `knowledge_base_dir: None`.
- Modify the extension:
  - `extension/options.html`: a "Knowledge base folder" row with `#kb-path`, `#choose-kb` ("Choose…") and `#kb-use-notes` ("Use notes folder").
  - `extension/options.ts`: `storageFor` carries `knowledge_base_dir`; `chooseFolder` sends `purpose: "notes_folder"`; add `chooseKnowledgeBase`.
  - `extension/settings.ts`: `saveConfig` gains `knowledgeBaseToken`.
  - `extension/protocol-shape.ts`: the storage key check.
- Modify `scripts/smoke-macos.mjs`: add `step("Knowledge base folder is chosen with the native picker")`. It answers one scripted picker dialog and asserts `hostConfig().storage.knowledge_base_dir`. The dialog counter follows the rule.
- Test: `host/src/consent.rs`, `host/src/config.rs`, `host/tests/scripted_dialogs.rs`, `protocol/tests/wire.rs`, `extension/tests/options.test.mjs`

**Interfaces:**
- Produces:
  - Wire:
    - `FolderPurpose = "notes_folder" | "knowledge_base"`
    - `ChooseFolderRequest {request_id, purpose}`
    - `StorageConfig.knowledge_base_dir: string | null` (maxLength 4096; null means use `root`)
    - `UpdateConfigRequest.knowledge_base_token: string | null`
  - `ConsentAuthority::choose_folder(&mut self, revision: &str, purpose: FolderPurpose) -> Result<Option<FolderSelection>>`. The picker title is "Choose Brauser notes folder" or "Choose Brauser knowledge base folder".
  - `fn check_knowledge_base(&self, current: &ConfigSnapshot, revision: &str, next: &ConfigSnapshot, token: Option<&str>) -> Result<Option<String>>`:
    - Returns `Ok(None)` when the knowledge base is null, or unchanged and no token is given.
    - Otherwise it requires `purpose == KnowledgeBase`, `grant.root == next kb`, the same revision, an unexpired grant, and an unchanged `vault::selected_root_identity`.
  - `authorize_update(&self, current, revision, next, picker_token: Option<&str>, knowledge_base_token: Option<&str>) -> Result<Authorized { selected_identity, knowledge_base_identity }>`
  - `ConfigStore::update_with_grants(next, rev, selected_identity, knowledge_base_identity: Option<&str>, harness_commit)`
  - `ConfigStore::knowledge_base_dir(&self) -> Option<PathBuf>`: the knowledge base folder if set, otherwise `storage.root`.
  - `ConfigStore::knowledge_base_identity(&self) -> Option<&str>`: the stored identity, or `root_identity` when following root.
  - TS `ConfigSession.saveConfig(config, pickerToken: string | null, knowledgeBaseToken: string | null = null)`

- [ ] Write the failing tests:
  ```rust
  // consent.rs
  #[test] fn notes_folder_token_cannot_authorize_the_knowledge_base() { /* mint NotesFolder for dir; next.kb = dir → Err */ }
  #[test] fn knowledge_base_token_cannot_change_the_notes_folder() {}
  #[test] fn stale_or_changed_knowledge_base_selection_is_refused() { /* other revision; dir replaced → "choose it again" */ }
  // config.rs
  #[test] fn v5_storage_without_knowledge_base_dir_loads_as_null_and_follows_root() {
      assert_eq!(store.snapshot().storage.as_ref().unwrap().knowledge_base_dir, None);
      assert_eq!(store.knowledge_base_dir().unwrap(), root);
  }
  #[test] fn relative_knowledge_base_dir_is_invalid() {}
  // scripted_dialogs.rs
  #[test] fn knowledge_base_picker_uses_its_own_title() { /* scripted request title == "Choose Brauser knowledge base folder" */ }
  // wire.rs
  #[test] fn choose_folder_requires_purpose() {}
  ```
  ```js
  test("choosing a knowledge base folder saves it with its own token", async () => {
    /* host.choose_folder replies { path: "/kb", picker_token: "t" } for purpose knowledge_base */
    assert.equal(lastChoose.purpose, "knowledge_base");
    assert.equal(lastUpdate.config.storage.knowledge_base_dir, "/kb");
    assert.equal(lastUpdate.knowledge_base_token, "t");
    assert.equal(lastUpdate.picker_token, null);
  });
  ```
- [ ] Run `cargo test --locked -p brauser knowledge_base` (FAIL: compile error, no `purpose`) and the extension command (FAIL).
- [ ] Implement as described in Interfaces. The host never writes into the knowledge base folder.
- [ ] Run the regenerate command, `cargo test --locked --workspace`, the scripted-dialogs test, both clippy commands, `npm run typecheck`, the extension command and the smoke command. Expect PASS.
- [ ] Commit: `git add -A protocol host/src host/tests/scripted_dialogs.rs extension scripts/smoke-macos.mjs && git commit -m "feat: knowledge base folder setting via the native folder picker"`

---

### Task 11: Documentation, backlog, doc gate, and the full quality gate

**Files:**
- Modify `DESIGN.md`:
  - The status line.
  - Principle 2: "Brauser can see any page; it records automatically only through enabled adapters".
  - §4.1/§4.5, wherever they describe the grant wall, the denylist or the harness token.
  - §5.3: no first-note gesture.
  - §7.1: remove the harness token and native confirmation. Note that the argv templates and cleared environment remain until PR 3.
  - §7.3: only `strip_params` remains.
  - §8 permission table: `host_permissions` HTTP(S), `webNavigation`, `scripting` (granted ahead of PR 3's use), `contextMenus` (kept for PR 3's summarize item), and no `activeTab`.
  - §14: record the 2026-10-26 decisions, including that path-prefix narrowing is dropped.
- Modify `AGENTS.md`:
  - "Current state": open access, sites derived from adapters, protocol v6, and the knowledge base folder.
  - Security rules: remove the denylist, harness-token, native-confirmation and per-site grant rules. Keep vault confinement, ownership, frozen IDs, and the host re-check of each delivered event. Add pending-removal migration as a capture-safety rule.
  - Repository map: no `privacy.rs`, `grants.ts` or `macos-alert/`.
- Modify `README.md`: "Status"; dev step 4 drops the grant and toolbar workaround; the headless smoke paragraph mentions no privacy list or confirmations.
- Modify `TESTING.md`:
  - "Native dialogs": picker only.
  - "Browser permission prompts": install-time access, with no grant-ahead step.
  - Learning log: the dialog-counter rule.
- Modify `protocol/README.md`: complete the "Version 6" paragraph (what was removed, what was added, the v5 TOML migration, path-prefix narrowing dropped) and correct the v5 paragraph.
- Modify `scripts/check_current_state.py`: change the required wording to "enabled adapter document now travels in the worker's revisioned policy lease", and add stale claims for "AI privacy list", "harness token" and "can't see this tab".
- Modify `BOSUN_FEATURE_BACKLOG.md`:
  - Add items for PRs 1-4 and mark PR 1 done.
  - Rewrite `brwr-6wz`, `brwr-0ij` and `brwr-deo` to point at the spec.
  - Note that `brwr-1hj`'s ungranted-tab flow and grant rollback no longer apply.
  - Close `brwr-ahb` as obsolete, because consent identity is removed.
  - Do **not** close `brwr-9yc` here; Task 27 closes it once `agent.env` exists.

**Interfaces:**
- Consumes: `scripts/check_current_state.py` `required_agents_facts`, `stale_claims`

- [ ] Edit `check_current_state.py` first, with the new required wording and stale claims.
- [ ] Run `python3 scripts/check_current_state.py`. Expect FAIL, because AGENTS.md still says "confirmed adapter document".
- [ ] Update the documents. Read `DESIGN.md` in full before editing it.
- [ ] Run the full quality gate from Global Constraints. Expect every command to PASS.
- [ ] Commit: `git add DESIGN.md AGENTS.md README.md TESTING.md protocol/README.md scripts/check_current_state.py BOSUN_FEATURE_BACKLOG.md && git commit -m "docs: record open access, protocol v6 removals, and the knowledge base folder"`

---

## PR 2: The Brauser service skeleton

PR 2 adds a per-user `brauser service` process. It owns agent runs: run records on disk, a queue, timeout and cancel through the run's process group, recovery, retention, and idle exit. It speaks the v6 run messages over a 0700 Unix socket using the native-messaging framing.

The Chrome-spawned host forwards those messages to it. Before forwarding, it checks the service's build identity. On a failed connect or a stale build, it runs `launchctl kickstart`. If the service is not installed, it answers `service_unavailable`.

Every run in this PR goes through a fake command harness. The real runners, prompt and answer validation come in PR 3, behind the `HarnessRunner`/`HarnessSession` boundary defined here. The summary file comes in PR 4, behind `RunFinisher`.

Assumes PR 1 is merged. PR 1 adds neither `HarnessId` nor `ErrorCode::service_unavailable`; Task 12 adds both. The existing `HarnessAdapter` enum (`claude_code | codex`) deliberately coexists with the new `HarnessId` (`codex | claude`) until Task 34 removes `HarnessAdapter`.

Every function, constant and arm that names `service::*` types sits behind `#[cfg(target_os = "macos")]`. Non-macOS builds get `#[cfg(not(target_os = "macos"))]` arms that answer `service_unavailable` "The Brauser service is only supported on macOS."

---

### Task 12: Protocol v6 run messages

**Files:**
- Modify: `protocol/schema.json` (Request oneOf, Response oneOf, `ErrorCode`, new `$defs`)
- Regenerate: `protocol/src/generated.rs`, `protocol/ts/generated.ts`
- Modify: `extension/protocol-shape.ts` (`ERROR_CODES`, `isResponseShape`)
- Test: `protocol/tests/wire.rs`, `extension/tests/agent.test.mjs`

**Interfaces:**
- Consumes: `RecordKey`, plus the envelope fields `protocol_version` (const 6) and `request_id`.
- Produces (schema `$defs`, generated Rust and TS):
  - `HarnessId` enum `["codex","claude"]`.
  - `RunState` enum `["queued","running","succeeded","failed","canceled"]`.
  - `RunErrorKind` enum `["harness_failed","timed_out","canceled","service_restarted","invalid_output","save_failed"]`.
  - `ErrorCode` gains `"service_unavailable"`.
  - `RunView`. All keys are required, and nullable keys are written `oneOf [X, null]`:
    - `run_id` (string, 36 chars), `state: RunState`, `harness: HarnessId`
    - `record_key: RecordKey`, `written_record_key: RecordKey|null`
    - `url`, `title`, `cwd: string|null`
    - `created_at`, `started_at|null`, `finished_at|null` (RFC 3339)
    - `error: string|null` (maxLength 2048), `error_kind: RunErrorKind|null`
    - `thread_id: string|null`, `session_id: string|null`, `summary_path: string|null`
    - `stream_bytes: integer`, `can_retry_save: boolean`
  - `StartRunRequest` `{type:"start_run", protocol_version, request_id, url, title, content, selection: string|null, harness: HarnessId|null, record_key: RecordKey|null, requested_at: string}`:
    - url and title use `RecordVisitRequest`'s bounds.
    - `content` maxLength 1048576 and `selection` maxLength 65536 are character bounds. The binding limit is the host frame cap, so Task 38 caps by encoded bytes.
    - `requested_at` maxLength 32 is the user's local click time with offset.
    - The extension always sends `record_key: null`.
  - `RunStarted` `{type:"run_started", …, run: RunView, existing: boolean}`
  - `GetRunRequest` `{type:"get_run", …, run_id, since_offset: integer}` → `RunFetched` `{type:"run_fetched", …, run: RunView, stream_text: string, next_offset: integer, more: boolean}`
  - `ListRunsRequest` `{type:"list_runs", …}` → `RunsListed` `{type:"runs_listed", …, runs: RunView[] maxItems 20}`
  - `CancelRunRequest` / `RetrySaveRequest` `{…, run_id}` → `RunUpdated` `{type:"run_updated", …, run: RunView}`
  - `ServiceHelloRequest` `{type:"service_hello", …}` → `ServiceReady` `{type:"service_ready", …, build_id: string (≤256)}`. This pair is host↔service only, and the host refuses it from the extension.
  - Generated Rust: `Request::{StartRun, GetRun, ListRuns, CancelRun, RetrySave, ServiceHello}`, `Response::{RunStarted, RunFetched, RunsListed, RunUpdated, ServiceReady}`.
  - TS: `isRunView(value): boolean`; `isResponseShape` gains `run_started | run_fetched | runs_listed | run_updated`.

- [ ] Write the failing tests in `wire.rs`:
  ```rust
  #[test] fn start_run_requires_nullable_selection_harness_record_key_and_requested_at() {
      let ok = r#"{"type":"start_run","protocol_version":6,"request_id":"r","url":"https://a.example/x","title":"T","content":"body","selection":null,"harness":null,"record_key":null,"requested_at":"2026-10-26T14:00:00-07:00"}"#;
      assert!(serde_json::from_str::<Request>(ok).is_ok());
      assert!(serde_json::from_str::<Request>(&ok.replace(r#","record_key":null"#, "")).is_err());
      assert!(serde_json::from_str::<Request>(&ok.replace(r#","requested_at":"2026-10-26T14:00:00-07:00""#, "")).is_err());
  }
  #[test] fn run_view_round_trips_every_state_and_error_kind() {}
  #[test] fn get_run_requires_since_offset() {}
  #[test] fn service_unavailable_error_code_round_trips() {}
  #[test] fn service_ready_requires_build_id() {}
  ```
  In `agent.test.mjs`, test that `isResponseShape({type:"run_fetched", run: view, stream_text:"x", next_offset:1, more:false})` is true, and false when `run.state === "paused"`.
- [ ] Run `cargo test --locked -p brauser-protocol --test wire start_run`. Expect FAIL.
- [ ] Implement, following the `generate.py` constraints: integers are u32, optional fields are required-nullable, and non-message objects have no `type` property. Regenerate, then add the `protocol-shape.ts` cases.
- [ ] Run `python3 protocol/generate.py --check && cargo test --locked -p brauser-protocol --test wire && npm run typecheck && npm run build:extension && npm run test:extension`. Expect PASS.
- [ ] Commit: `git add protocol/schema.json protocol/src/generated.rs protocol/ts/generated.ts protocol/tests/wire.rs extension/protocol-shape.ts extension/tests/agent.test.mjs && git commit -m "feat(protocol): add v6 run messages, service hello, and service_unavailable"`

### Task 13: Shared frame module

**Files:**
- Create: `host/src/frame.rs`
- Modify: `host/src/native.rs` (move `MAX_INBOUND_BYTES`, `MAX_OUTBOUND_BYTES`, `FrameError`, `read_frame` and `write_frame`); `host/src/lib.rs` (add `pub mod frame;`)
- Test: `host/src/frame.rs` `mod tests`

**Interfaces:**
- Produces:
  - `pub const MAX_INBOUND_BYTES: usize = 4 * 1024 * 1024;`, `pub const MAX_OUTBOUND_BYTES: usize = 900 * 1024;`
  - `pub enum FrameError { TooLarge, InvalidLength, Io(io::Error) }`
  - `pub fn read_frame<R: Read>(input: &mut R) -> Result<Option<Vec<u8>>, FrameError>`
  - `pub fn write_frame<W: Write>(output: &mut W, response: &Response) -> anyhow::Result<()>`
  - `pub fn write_request<W: Write>(output: &mut W, request: &Request) -> anyhow::Result<()>`, capped at `MAX_INBOUND_BYTES`. An oversized request is an error.

- [ ] Write the failing tests: `request_and_response_round_trip_native_endian`, `oversized_response_becomes_message_too_large` (same request_id) and `zero_length_frame_is_invalid`.
- [ ] Run `cargo test --locked -p brauser frame::`. Expect FAIL.
- [ ] Implement by moving the code verbatim with `pub` visibility, and add `write_request`. `native.rs` imports from `crate::frame`.
- [ ] Run `cargo test --locked -p brauser`. Expect PASS.
- [ ] Commit: `git add host/src/frame.rs host/src/native.rs host/src/lib.rs && git commit -m "refactor(host): extract native-messaging framing into frame module"`

### Task 14: Service names, support-directory paths, and build identity

**Files:**
- Modify: `host/src/brand.rs` (add constants), `extension/brand.ts` (add `SERVICE_LABEL`)
- Create: `host/src/service/mod.rs`, `host/src/service/paths.rs`
- Modify: `host/src/lib.rs` (`#[cfg(target_os = "macos")] pub mod service;`)
- Test: `host/src/service/paths.rs` `mod tests`

**Interfaces:**
- Produces in `brand.rs`:
  - `pub const SERVICE_LABEL: &str = concat!("com.rauser.", namespace!(), ".service");`
  - `SOCKET_NAME = "service.sock"`, `AGENT_ENV_NAME = "agent.env"`, `RUNS_DIR = "runs"`.
  - Under `#[cfg(any(test, feature = "scripted-dialogs"))]`: `FAKE_HARNESS_ENV = "BRAUSER_FAKE_HARNESS"`, `SERVICE_SPAWN_ENV = "BRAUSER_SERVICE_SPAWN"`, `SERVICE_IDLE_SECS_ENV = "BRAUSER_SERVICE_IDLE_SECS"`, `RUN_TIMEOUT_SECS_ENV = "BRAUSER_RUN_TIMEOUT_SECS"`.
- Produces in `brand.ts`: `export const SERVICE_LABEL = "com.rauser.brauser.service";`
- Produces in `paths.rs`: `pub struct ServicePaths { support: PathBuf }` with:
  - `pub fn for_config(config: &ConfigStore) -> Result<Self>` (support = `config.dir()`) and `pub fn at(support: PathBuf) -> Self`
  - `pub fn ensure_private(&self) -> Result<()>`
  - `pub fn socket(&self) -> Result<PathBuf>`, `pub fn runs(&self) -> PathBuf`, `pub fn agent_env(&self) -> PathBuf`
  - `pub fn lock(&self) -> PathBuf` (`service.lock`) and `pub fn pid(&self) -> PathBuf` (`service.pid`)
- `pub fn build_id() -> String`: `"<CARGO_PKG_VERSION>:<dev>:<ino>:<mtime_ns>"` of `std::env::current_exe()` canonicalized. The host and the service run the same binary path, so equal ids mean the same build.
- `ensure_private` creates the directory. It refuses a symlink (via `symlink_metadata`) and a directory owned by a uid other than `rustix::process::getuid()`, and narrows the mode to 0700.
- `socket()` fails with "support directory path is too long for a Unix socket" at 104 bytes or more (macOS `sun_path`).

- [ ] Write the failing tests:
  ```rust
  #[test] fn ensure_private_narrows_mode_to_0700() {}
  #[test] fn ensure_private_refuses_a_symlinked_support_dir() {}
  #[test] fn socket_path_over_103_bytes_is_refused() {
      let long = PathBuf::from(format!("/tmp/{}", "a".repeat(100)));
      assert!(ServicePaths::at(long).socket().unwrap_err().to_string().contains("too long"));
  }
  #[test] fn build_id_is_stable_within_a_process_and_names_the_version() {
      assert_eq!(build_id(), build_id()); assert!(build_id().starts_with(env!("CARGO_PKG_VERSION")));
  }
  #[test] fn label_is_reverse_dns_and_matches_brand_ts() {
      assert_eq!(SERVICE_LABEL, "com.rauser.brauser.service");
      assert!(include_str!("../../../extension/brand.ts").contains(r#"SERVICE_LABEL = "com.rauser.brauser.service""#));
  }
  ```
- [ ] Run `cargo test --locked -p brauser service::paths`. Expect FAIL.
- [ ] Implement. `service/mod.rs` declares submodules only.
- [ ] Run `cargo test --locked -p brauser service::paths && npm run typecheck`. Expect PASS.
- [ ] Commit: `git add host/src/brand.rs extension/brand.ts host/src/service/mod.rs host/src/service/paths.rs host/src/lib.rs && git commit -m "feat(service): add service label, private support-directory paths, and build identity"`

### Task 15: agent.env overlay

**Files:**
- Create: `host/src/service/agent_env.rs`
- Test: same file

**Interfaces:**
- Produces: `pub struct AgentEnv { vars: Vec<(String, String)> }` with:
  - `pub fn read(path: &Path) -> Result<AgentEnv, AgentEnvError>`
  - `pub fn names(&self) -> Vec<String>`, sorted. No value ever leaves this type, and `Debug` is hand-written to print names only.
  - `pub fn overlay(&self, base: impl IntoIterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)>`
- `pub enum AgentEnvError { NotPrivate { mode: u32 }, Symlink, Line { number: usize, reason: &'static str }, Io(io::Error) }`.
- Rules:
  - A missing file is an empty `AgentEnv`.
  - The mode must be exactly 0600, and the file must be regular and not a symlink. The size cap is 64 KiB.
  - Lines have the form `KEY=VALUE`. Blank lines and `#` comments are allowed.
  - Keys match `[A-Z_][A-Z0-9_]*`, checked by hand without a regex crate, and may not repeat.
  - There is no quote stripping and no `$` expansion. When the file and the base both set a key, the file wins.
- Consumer: the server's `start_run` (Task 21) calls `read` for every run. Task 36's `get_agent_env` uses `names()`.

- [ ] Write the failing tests: `missing_file_is_empty`, `overlay_replaces_base_values`, `group_readable_file_is_refused`, `bad_key_reports_line_number` and `debug_never_prints_values`.
- [ ] Run `cargo test --locked -p brauser service::agent_env`. Expect FAIL.
- [ ] Implement.
- [ ] Run the same command. Expect PASS.
- [ ] Commit: `git add host/src/service/agent_env.rs host/src/service/mod.rs && git commit -m "feat(service): read optional 0600 agent.env overlay"`

### Task 16: Run record store

**Files:**
- Create: `host/src/service/record.rs`
- Test: same file

**Interfaces:**
- Produces `#[derive(Serialize, Deserialize)] #[serde(deny_unknown_fields)] pub struct RunStatusFile`, the content of `status.json`. Its fields:
  - `version: u32` (=1), `run_id: String` (validated with `Uuid::parse_str` on read, so uuid's `serde` feature is not needed), `state: RunState`, `harness: HarnessId`
  - `service_pid: Option<u32>`, `process_group: Option<i32>`, `heartbeat_at: Option<String>`
  - `thread_id`, `session_id`, `cwd` (all `Option<String>`)
  - `start_key: RecordKey`, `written_key: Option<RecordKey>`, `url: String`, `title: String`, `requested_at: String`
  - `created_at: String`, `started_at` and `finished_at` (`Option<String>`, RFC 3339)
  - `error: Option<String>`, `error_kind: Option<RunErrorKind>`, `summary_path: Option<String>`, `stream_truncated: bool`
- `impl RunStatusFile { pub fn view(&self, stream_bytes: u32, has_result: bool) -> RunView }`. `can_retry_save` is `state == Failed && error_kind == Some(SaveFailed) && has_result`.
- `pub struct RunStore { runs: PathBuf }` with:
  - `pub fn open(runs: PathBuf) -> Result<Self>`, mode 0700.
  - `pub fn create(&self, status: &RunStatusFile, page_md: &str) -> Result<()>`: the run dir is 0700 and `page.md` 0600. It refuses an existing dir.
  - `pub fn write_status(&self, status: &RunStatusFile) -> Result<()>`: temp file plus rename in the run dir, with fsync.
  - `pub fn status(&self, id: Uuid) -> Result<RunStatusFile>`
  - `pub fn list(&self) -> Vec<RunStatusFile>`: skips unparsable dirs and ignores non-UUID names.
  - `pub fn append_stream(&self, id: Uuid, text: &str) -> Result<bool>`: returns false once `STREAM_CAP = 4 MiB` would be exceeded, and writes nothing past the cap.
  - `pub fn read_stream(&self, id: Uuid, since_offset: u32) -> Result<StreamPage, StreamError>`
  - `pub fn append_log(&self, id: Uuid, bytes: &[u8]) -> Result<()>`: writes `run.log`, capped at 1 MiB.
  - `pub fn write_result(&self, id, value: &serde_json::Value)` and `pub fn read_result(&self, id) -> Result<Option<serde_json::Value>>`
  - `pub fn page_path(&self, id) -> PathBuf` and `pub fn remove(&self, id) -> Result<()>`
- `pub struct StreamPage { pub text: String, pub next_offset: u32, pub more: bool }`. A page is at most `STREAM_PAGE_BYTES = 128 * 1024` raw bytes, so even 6× JSON escaping stays under 900 KiB. `StreamError::NotCharBoundary` covers an offset that is not on a UTF-8 boundary.
- `pub fn page_markdown(title: &str, url: &str, content: &str, selection: Option<&str>) -> String`. It produces `# <title>\n\nSource: <url>\n\n<content>`, plus `\n\n## Highlighted by the user\n\n<selection>` when a selection is present.

- [ ] Write the failing tests: `stream_pages_by_byte_offset_on_char_boundaries`, `stream_stops_at_cap`, `status_write_is_atomic_and_round_trips`, `run_view_offers_retry_save_only_for_save_failed_with_result`, `page_markdown_puts_selection_in_its_own_section`, `run_files_are_private` and `status_with_a_non_uuid_run_id_is_skipped_by_list`.
- [ ] Run `cargo test --locked -p brauser service::record`. Expect FAIL.
- [ ] Implement. When the stream file's end is torn mid-character, `read_stream` snaps the page end back to the last char boundary.
- [ ] Run the same command. Expect PASS.
- [ ] Commit: `git add host/src/service/record.rs host/src/service/mod.rs && git commit -m "feat(service): add on-disk run records with paged stream reads"`

### Task 17: Run queue

**Files:**
- Create: `host/src/service/queue.rs`
- Test: same file

**Interfaces:**
- Produces: `pub const MAX_RUNNING: usize = 2;` and `pub enum Submit { New, Existing(Uuid) }`.
- `pub struct Scheduler { running: Vec<Uuid>, waiting: VecDeque<Uuid>, by_key: HashMap<KeyId, Uuid> }`, where `type KeyId = (u8, String)`. The `u8` is from `match kind { RecordKeyKind::Url => 0, RecordKeyKind::Artifact => 1 }`. The generator is not changed: `generate.py` emits a fixed derive list without `Hash`.
- Methods:
  - `submit(&mut self, key: &RecordKey, id: Uuid) -> Submit`
  - `next_ready(&mut self) -> Option<Uuid>`: FIFO, while `running.len() < MAX_RUNNING`
  - `finished(&mut self, id: Uuid)`
  - `dequeue(&mut self, id: Uuid) -> bool`
  - `is_idle(&self) -> bool`

- [ ] Write the failing tests:
  ```rust
  #[test] fn two_run_at_once_rest_fifo_one_per_key() {
      let mut s = Scheduler::default();
      assert!(matches!(s.submit(&k1, a), Submit::New));
      s.submit(&k2, b); s.submit(&k3, c);
      assert_eq!([s.next_ready(), s.next_ready(), s.next_ready()], [Some(a), Some(b), None]);
      assert!(matches!(s.submit(&k1, d), Submit::Existing(x) if x == a));
      s.finished(a);
      assert_eq!(s.next_ready(), Some(c));
      assert!(matches!(s.submit(&k1, e), Submit::New));
  }
  #[test] fn url_and_artifact_keys_with_the_same_value_are_distinct() {}
  #[test] fn dequeued_run_frees_its_key() {}
  #[test] fn idle_only_when_nothing_queued_or_running() {}
  ```
- [ ] Run `cargo test --locked -p brauser service::queue`. Expect FAIL.
- [ ] Implement the pure state machine.
- [ ] Run the same command. Expect PASS.
- [ ] Commit: `git add host/src/service/queue.rs host/src/service/mod.rs && git commit -m "feat(service): add two-slot FIFO run queue keyed by record key"`

### Task 18: Harness runner boundary and the fake command runner

**Files:**
- Create: `host/src/service/harness_api.rs`, `host/src/service/command_runner.rs`
- Test: both files' `mod tests`

**Interfaces (the boundary PR 3 implements):**
```rust
pub struct RunSpec {
    pub run_id: Uuid, pub harness: HarnessId, pub title: String,
    pub cwd: PathBuf,            // PR2: storage.root; PR3 (Task 31): knowledge_base_dir
    pub page_path: PathBuf,      // absolute path to page.md
    pub prompt: String,          // PR2: draft_prompt(); PR3: §6.4 prompt
    pub env: Vec<(OsString, OsString)>, // built by the server: service env overlaid with agent.env
}
pub enum SessionRef { CodexThread(String), ClaudeSession(String) }
pub enum HarnessEvent { Text(String), Session(SessionRef), Diagnostic(Vec<u8>) }
pub enum TurnOutcome { Answered { final_text: String }, Failed { reason: String } }

/// Every child a runner starts goes through `ProcessSlot::spawn`, so cancel
/// and timeout can kill its process group (§5.2).
#[derive(Clone, Default)] pub struct ProcessSlot { /* Arc<Mutex<{ group: Option<i32>, killed: bool }>> */ }
impl ProcessSlot {
    pub fn spawn(&self, command: &mut Command) -> io::Result<Child>; // process_group(0); after kill() → ErrorKind::Interrupted; records pid as group
    pub fn kill(&self);              // killed = true; rustix kill_process_group(SIGKILL)
    pub fn group(&self) -> Option<i32>;
    pub fn was_killed(&self) -> bool;
}
pub trait HarnessRunner: Send + Sync {
    fn harness(&self) -> HarnessId;
    fn start(&self, spec: &RunSpec, slot: &ProcessSlot) -> anyhow::Result<Box<dyn HarnessSession>>;
}
pub trait HarnessSession: Send {
    /// First call: spec.prompt. PR3 may call once more with the JSON-correction follow-up.
    fn turn(&mut self, prompt: &str, sink: &mut dyn FnMut(HarnessEvent)) -> TurnOutcome;
    fn close(self: Box<Self>);
}
pub struct HarnessRegistry { runners: Vec<Box<dyn HarnessRunner>> }
impl HarnessRegistry {
    pub fn get(&self, id: Option<HarnessId>) -> Option<&dyn HarnessRunner>;
    pub fn from_environment() -> Self;   // = from_lookup(|k| std::env::var_os(k))
    pub fn from_lookup(get: impl Fn(&str) -> Option<OsString>) -> Self;
}
pub fn draft_prompt(page_path: &Path) -> String; // placeholder naming page.md; PR3 replaces
```
- `CommandRunner { harness: HarnessId, program: PathBuf }` implements `HarnessRunner`. Each `turn` spawns `program <page_path>` with `cwd`, `env_clear()` followed by `spec.env`, and the prompt on stdin. Output handling:
  - Each stdout line becomes `Text(line + "\n")`, except `session: <id>`, which becomes `Session(ClaudeSession(id))`.
  - Stderr becomes `Diagnostic`.
  - Exit 0 gives `Answered{final_text: stdout minus session lines}`. Anything else gives `Failed{reason: "harness exited with status N: <last stderr line>"}`.
- `from_lookup`:
  - In production PR 2 registers no runner, and `start_run` then fails with `not_configured`.
  - Under `#[cfg(any(test, feature = "scripted-dialogs"))]`, if `FAKE_HARNESS_ENV` names an absolute executable, it registers `CommandRunner` for both `HarnessId::Codex` and `HarnessId::Claude`.

- [ ] Write the failing tests in `command_runner.rs`, using `#!/bin/sh` scripts in a tempdir: `streams_lines_and_answers_with_stdout`, `prompt_arrives_on_stdin_and_page_path_as_argv`, `nonzero_exit_fails_with_last_stderr_line`, `kill_ends_the_whole_process_group` (`sleep 30 & echo started; wait`, then `slot.kill()`; within 2 s, `rustix::process::test_kill_process_group(group)` is Err/ESRCH) and `spawn_after_kill_is_refused`. In `harness_api.rs`, add `registry_is_empty_without_fake_env` and `registry_serves_fake_for_both_ids`, both through `from_lookup`.
- [ ] Run `cargo test --locked -p brauser service::command_runner service::harness_api`. Expect FAIL.
- [ ] Implement. Reuse the drain-thread and mpsc pattern of `harness::run_bounded`, but stream line by line.
- [ ] Run the same command. Expect PASS.
- [ ] Commit: `git add host/src/service/harness_api.rs host/src/service/command_runner.rs host/src/service/mod.rs && git commit -m "feat(service): define harness runner boundary with process-group slot and fake command runner"`

### Task 19: Run supervisor: lifecycle, timeout, cancel, finisher, retry save

**Files:**
- Create: `host/src/service/supervisor.rs`
- Test: same file

**Interfaces:**
```rust
pub const DEFAULT_RUN_TIMEOUT: Duration = Duration::from_secs(600);
pub const HEARTBEAT: Duration = Duration::from_secs(5);
pub struct SavedRun { pub written_key: RecordKey, pub summary_path: Option<String> }
pub trait RunFinisher: Send + Sync {
    /// PR2: final_text must be a JSON object ≤ 256 KiB. PR3 (Task 33) replaces with §6.4 validation.
    fn accept(&self, final_text: &str) -> Result<serde_json::Value, String>;
    /// PR2: re-reads config under lock_current+refresh and requires a configured, openable vault. PR4 writes here.
    fn save(&self, run: &RunStatusFile, result: &serde_json::Value) -> Result<SavedRun, String>;
}
pub struct VaultCheckFinisher;
pub struct ServiceOptions { pub run_timeout: Duration, pub idle_exit: Duration, pub service_pid: u32 }
pub struct Supervisor { /* Arc<Inner{store, scheduler: Mutex<Scheduler>, slots: Mutex<HashMap<Uuid, ProcessSlot>>, registry, finisher, options, last_activity}> */ }
impl Supervisor {
    pub fn new(store: RunStore, registry: HarnessRegistry, finisher: Box<dyn RunFinisher>, options: ServiceOptions) -> Self;
    pub fn start_run(&self, req: StartRun) -> Result<(RunView, bool /*existing*/), (ErrorCode, String)>;
    pub fn get_run(&self, id: Uuid, since_offset: u32) -> Result<(RunView, StreamPage), (ErrorCode, String)>;
    pub fn list_runs(&self) -> Vec<RunView>;          // queued/running + newest finished, ≤ 20, newest first
    pub fn cancel_run(&self, id: Uuid) -> Result<RunView, (ErrorCode, String)>;  // finished → unchanged view
    pub fn retry_save(&self, id: Uuid) -> Result<RunView, (ErrorCode, String)>;  // !can_retry_save → Conflict
    pub fn is_idle(&self) -> bool;
}
pub struct StartRun { pub url: String, pub title: String, pub content: String, pub selection: Option<String>,
    pub harness: Option<HarnessId>, pub record_key: RecordKey, pub requested_at: String,
    pub cwd: PathBuf, pub env: Vec<(OsString, OsString)> }
```
- The supervisor never reads `agent.env`; the server builds `env`.
- Lifecycle on a worker thread:
  1. Write `running` with `started_at`.
  2. Call `runner.start` and record `process_group` from the slot.
  3. Call `turn(prompt)`. Each `Text` goes to `append_stream`, each `Diagnostic` to `append_log`, and each `Session` updates `thread_id`/`session_id`.
- A watchdog thread rewrites `heartbeat_at` every `HEARTBEAT`, and calls `slot.kill()` at the deadline.
- Outcome mapping:
  - Killed by cancel: `canceled`/`canceled`.
  - Killed by the deadline: `failed`/`timed_out`, "Timed out after <N> minutes".
  - `Failed`: `failed`/`harness_failed`.
  - `accept` returns `Err`: `failed`/`invalid_output`.
  - Otherwise `write_result`, then `save`. `Err` gives `failed`/`save_failed`. `Ok` gives `succeeded` with `written_key` and `summary_path`.
- After any outcome, call `scheduler.finished` and pump the next run.
- Cancelling a queued run dequeues it and writes `canceled` without spawning.
- Every status transition goes through `write_status` under a per-run mutex.

- [ ] Write the failing tests with `CommandRunner` fakes and a `TestFinisher { fail_saves: AtomicUsize }`:
  - `run_succeeds_and_writes_every_record_file`
  - `third_run_waits_until_a_slot_frees`
  - `same_record_key_returns_existing_run`
  - `cancel_kills_process_group_and_marks_canceled`
  - `timeout_fails_with_timed_out` (`run_timeout` = 300 ms)
  - `non_json_answer_is_invalid_output_and_keeps_stream`
  - `save_failure_offers_retry_save_and_retry_succeeds` (the harness ran once)
  - `retry_save_on_succeeded_run_is_conflict`
  - `start_run_env_reaches_the_harness` (`env: vec![("FOO","bar")]`, the fake prints `$FOO`, and the stream contains `bar`)
  - `missing_runner_is_not_configured`
- [ ] Run `cargo test --locked -p brauser service::supervisor`. Expect FAIL.
- [ ] Implement.
- [ ] Run the same command. Expect PASS.
- [ ] Commit: `git add host/src/service/supervisor.rs host/src/service/mod.rs && git commit -m "feat(service): supervise runs with timeout, cancel, finisher, and retry save"`

### Task 20: Recovery on start and retention

**Files:**
- Create: `host/src/service/maintenance.rs`
- Test: same file

**Interfaces:**
- Produces: `pub const KEEP_DAYS: i64 = 30; pub const KEEP_NEWEST: usize = 200; pub const ORPHAN_KILL_WINDOW: Duration = Duration::from_secs(60);`
- `pub fn recover(store: &RunStore, now: OffsetDateTime, own_pid: u32, alive: &dyn Fn(i32) -> bool, kill: &dyn Fn(i32)) -> Vec<Uuid>` handles every `running` record:
  - It kills the group only when all of these hold:
    - `service_pid` is not alive, or differs from `own_pid`;
    - `heartbeat_at` is within `ORPHAN_KILL_WINDOW` of `now`;
    - `alive(group)` is true.

    An old heartbeat means a reboot or a long gap, when the PID may have been reused, so the group is left alone.
  - It then writes `failed`/`service_restarted`, with the error "service restarted".
  - It returns the `queued` IDs in `created_at` order.
- `pub fn prune(store: &RunStore, now: OffsetDateTime) -> usize` deletes finished runs that are both older than 30 days and outside the newest 200. It never touches queued or running runs.
- `Supervisor::new` calls `recover` (with `test_kill_process_group(..).is_ok()` and `kill_process_group(SIGKILL)`), then `prune`, then re-submits the queued IDs. `prune` also runs after each finished run.

- [ ] Write the failing tests:
  - `running_record_with_dead_group_becomes_service_restarted`
  - `running_record_with_live_group_and_fresh_heartbeat_is_killed_then_failed` (spawn `sleep 30` via `ProcessSlot`)
  - `stale_heartbeat_group_is_never_killed` (live group, heartbeat 10 min old, so `kill` is never called)
  - `queued_records_are_returned_oldest_first`
  - `prune_keeps_newest_200_of_old_runs`
  - `prune_keeps_all_recent_runs_beyond_200`
  - `prune_never_removes_running_or_queued`
- [ ] Run `cargo test --locked -p brauser service::maintenance`. Expect FAIL.
- [ ] Implement, and wire it into `Supervisor::new` and the finish path.
- [ ] Run `cargo test --locked -p brauser service::`. Expect PASS.
- [ ] Commit: `git add host/src/service/maintenance.rs host/src/service/supervisor.rs host/src/service/mod.rs && git commit -m "feat(service): recover interrupted runs on start and prune old run records"`

### Task 21: Socket server and `brauser service` subcommand

**Files:**
- Create: `host/src/service/server.rs`
- Modify:
  - `host/src/service/mod.rs`: `pub fn run(args) -> Result<()>` and `pub const COMMAND: &str = "service";`.
  - `host/src/main.rs`: command dispatch.
  - `host/src/native.rs`: extract `parse_envelope`.
- Test: `host/src/service/server.rs` `mod tests`

**Interfaces:**
- Produces: `pub fn serve(paths: &ServicePaths, supervisor: Supervisor, options: &ServiceOptions) -> Result<ServeExit>` and `pub enum ServeExit { Idle, AlreadyRunning }`. Startup sequence:
  1. `ensure_private`.
  2. `File::try_lock` on `service.lock`. If it is held, exit `AlreadyRunning` with status 0.
  3. Write `service.pid`.
  4. Unlink a stale `service.sock`, but only if it is a socket.
  5. `UnixListener::bind`, then chmod 0600.
  6. Accept with one thread per connection. Each connection loops `frame::read_frame` → `dispatch_service` → `frame::write_frame`.
- `pub(crate) fn parse_envelope(body: &[u8]) -> Result<(String, Request), Response>` is extracted from `native::dispatch_json` and shared by both dispatchers. A wrong version answers `unsupported_protocol_version`.
- `dispatch_service`:
  - Answers `service_hello` with `ServiceReady { build_id: paths::build_id() }`, plus the five run requests. Anything else gets `invalid_request` "not a service request".
  - `start_run` with `record_key: null` is `invalid_request`.
- In `start_run`, the server builds the run inputs:
  - `cwd` comes from its own `ConfigStore::load()`. In PR 2 it is `storage.root`; otherwise `not_configured` "Choose a notes folder in Brauser settings first."
  - `env` is `std::env::vars_os()` overlaid with `AgentEnv::read(paths.agent_env())`.
  - An agent.env error gives `invalid_config` with text that never includes values.
- Idle exit: a non-blocking accept poll every 250 ms. Exit `Idle` when `supervisor.is_idle()` holds and nothing has arrived for `options.idle_exit` (default 600 s). On exit, remove the socket and the pid file.
- `service::run` builds `ServiceOptions`. Under `scripted-dialogs`, it honors `SERVICE_IDLE_SECS_ENV` and `RUN_TIMEOUT_SECS_ENV`.
- `main.rs` gets `#[cfg(target_os = "macos")] Some(service::COMMAND) => service::run(args)` and `#[cfg(not(target_os = "macos"))] Some("service") => bail!("the Brauser service is only supported on macOS")`.

- [ ] Write the failing tests. Use `tempfile::Builder::new().prefix("bs").tempdir_in("/tmp")` to stay under the `sun_path` limit.
  - `framed_start_and_get_run_over_the_socket`
  - `service_hello_reports_this_build`
  - `version_5_request_gets_unsupported_protocol_version`
  - `start_run_without_record_key_is_invalid_request`
  - `agent_env_file_reaches_the_harness` (agent.env 0600 `FOO=bar`; the fake prints `$FOO`)
  - `group_readable_agent_env_fails_start_without_values` (0644 `FOO=secret`; code `InvalidConfig`; the message does not contain `secret`)
  - `second_instance_reports_already_running`
  - `idle_service_exits_and_removes_socket`
  - `socket_is_0600_in_a_0700_dir`
- [ ] Run `cargo test --locked -p brauser service::server`. Expect FAIL.
- [ ] Implement.
- [ ] Run `cargo test --locked -p brauser service:: && cargo clippy --locked --workspace --all-targets -- -D warnings`. Expect PASS.
- [ ] Commit: `git add host/src/service/server.rs host/src/service/mod.rs host/src/main.rs host/src/native.rs && git commit -m "feat(service): serve run messages on a private Unix socket with idle exit"`

### Task 22: Reusable record-key resolution

**Files:**
- Modify: `host/src/native.rs` (`fn note_context`)
- Test: `host/src/native.rs` `mod tests`

**Interfaces:**
- Produces: `pub(crate) fn resolve_record_key(config: &mut ConfigStore, url: &str) -> Result<RecordKey, (ErrorCode, &'static str)>`. It holds the key-only part of `note_context`:
  - the refresh and repair check;
  - storage required;
  - `canonical_url` with `strip_params`;
  - an adapter match on the original URL;
  - artifact-note and index recovery;
  - the `url_record_key` fallback.

  It does not list older notes. `note_context` delegates to it, and so do the run proxy (Task 23) and the PR 4 finisher.

- [ ] Write the failing test `run_key_matches_page_note_key`. For an adapter URL and a plain URL on `configured_store`, `resolve_record_key` must equal the `load_note` key.
- [ ] Run `cargo test --locked -p brauser native::tests::run_key_matches_page_note_key`. Expect FAIL.
- [ ] Implement the extraction.
- [ ] Run `cargo test --locked -p brauser native::`. Expect PASS.
- [ ] Commit: `git add host/src/native.rs && git commit -m "refactor(host): extract record-key resolution shared by notes and runs"`

### Task 23: Native host proxy, build check, kickstart, and service_unavailable

**Files:**
- Create: `host/src/service/client.rs`
- Modify: `host/src/native.rs` (`fn dispatch` arms, the per-connection context struct, `serve_with_io`); `host/src/capture.rs` (`parse_event_time` and `check_event_window` become `pub(crate)`)
- Test: `host/src/service/client.rs` and `host/src/native.rs` `mod tests`

**Interfaces:**
```rust
pub enum Launch { Started, NotInstalled, Failed(String) }
pub struct ServiceLink { pub socket: PathBuf, pub launch: fn(restart: bool) -> Launch, pub wait: Duration /* 5s */, pub build_id: String }
pub enum ServiceError { Unavailable(String), Protocol(String) }
impl ServiceLink {
    pub fn for_config(config: &ConfigStore) -> Result<Self>;
    pub fn request(&self, request: &Request) -> Result<Response, ServiceError>;
}
pub fn launchd_kickstart(restart: bool) -> Launch; // /bin/launchctl kickstart [-k] gui/<getuid>/<SERVICE_LABEL>; exit 113 or "Could not find service" → NotInstalled
pub const NOT_INSTALLED: &str = "The Brauser service is not installed. Reinstall Brauser, or for a development build run scripts/register-dev-host.mjs.";
pub const WRONG_BUILD: &str = "The Brauser service is running a different build of Brauser. Reinstall Brauser, or for a development build run scripts/register-dev-host.mjs again.";
```
- `request` behavior:
  1. Connect. On `NotFound` or `ConnectionRefused`, call `launch(false)` once and retry every 100 ms until `wait`.
  2. On each new connection, send `service_hello` first. If `build_id` differs from `self.build_id`, or the reply is `UnsupportedProtocolVersion`, call `launch(true)` once, reconnect, and say hello again. A second mismatch gives `Unavailable(WRONG_BUILD)`.
  3. Send the request.

  `NotInstalled` maps to `Unavailable(NOT_INSTALLED)`. The settings page and panel also key their setup instructions off `ErrorCode::ServiceUnavailable`, not off the text.
- Test safety: under `cfg(any(test, feature = "scripted-dialogs"))`, `for_config` never selects `launchd_kickstart`:
  - With `SERVICE_SPAWN_ENV=1`, `launch(false)` spawns `current_exe() service` detached with the same env. `launch(true)` first sends SIGTERM to the pid in `service.pid`, waits up to 2 s for the socket to go, then spawns.
  - Otherwise it returns `NotInstalled`.
  - Unit tests pass their own `fn`.
- `native.rs` changes:
  - The context struct gains `#[cfg(target_os = "macos")] service: Option<&'a ServiceLink>`.
  - On macOS, `Request::StartRun`:
    1. Returns `invalid_request` "record_key is chosen by the host" when one was sent.
    2. Validates `requested_at` with `capture::parse_event_time` and `check_event_window`; failure is `invalid_request`.
    3. Resolves the key with `resolve_record_key` and forwards `record_key: Some(key)`.
  - `GetRun`, `ListRuns`, `CancelRun` and `RetrySave` are forwarded verbatim. `ServiceHello` from the extension gets `invalid_request`.
  - `ServiceError::Unavailable(m)` becomes `error(id, ErrorCode::ServiceUnavailable, m)`.
  - Under `#[cfg(not(target_os = "macos"))]`, the same arms return `ServiceUnavailable` "The Brauser service is only supported on macOS."

- [ ] Write the failing tests. A fake service is a `UnixListener` thread in a short `/tmp` dir that records requests and answers canned responses.
  - `start_run_forwards_host_resolved_key`
  - `extension_supplied_record_key_is_rejected_without_connecting`
  - `start_run_with_an_unparseable_requested_at_is_invalid_request_without_connecting` (the launch fn panics if called)
  - `not_installed_service_gives_service_unavailable`
  - `kickstart_then_retry_reaches_a_late_listener`
  - `stale_service_build_is_restarted`: the first hello reports `build_id "old"`, so `launch(true)` runs once; the second hello matches and the request succeeds.
  - `persistent_build_mismatch_is_service_unavailable` (message `WRONG_BUILD`)
  - `version_skew_restarts_once`
  - `service_hello_from_the_extension_is_refused`
  - `notes_and_capture_do_not_touch_the_service`
- [ ] Run `cargo test --locked -p brauser service::client native::tests::start_run`. Expect FAIL.
- [ ] Implement.
- [ ] Run `cargo test --locked --workspace && cargo test --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs`, and the non-macOS `cargo check` per Global Constraints. Expect PASS.
- [ ] Commit: `git add host/src/service/client.rs host/src/service/mod.rs host/src/native.rs host/src/capture.rs && git commit -m "feat(host): forward run messages to the service with build check, kickstart, and service_unavailable"`

### Task 24: Binary-level service integration test

**Files:**
- Create: `host/tests/service.rs` (`#![cfg(all(feature = "scripted-dialogs", target_os = "macos"))]`)

**Interfaces:**
- Consumes: `env!("CARGO_BIN_EXE_brauser")`; the `serve` and `service` subcommands; the four env seam names; `brauser_host::frame`.
- Uses the pattern of `host/tests/harness_setup.rs`'s `World`: `env_clear()`, a short HOME from `tempdir_in("/tmp")`, `PATH=<fakeBin>:/usr/bin:/bin`, and a config with a notes folder.

- [ ] Write the tests:
  - `host_spawns_service_and_run_succeeds`: all five run files exist under `$HOME/Library/Application Support/Brauser/runs/<id>/`.
  - `run_survives_host_exit`
  - `cancel_kills_fake_group`
  - `service_exits_when_idle`
  - `without_spawn_seam_host_reports_service_unavailable`
  - `rebuilt_binary_restarts_the_running_service`: start the service, `touch` the binary's mtime, and expect the next `serve` request to restart it and succeed.
- [ ] Run `cargo test --locked -p brauser --features scripted-dialogs --target-dir target/scripted-dialogs --test service`. Expect FAIL until it is wired.
- [ ] If a test fails, change only the spawn seam's env pass-through (`client.rs`), the socket and pid paths (`paths.rs`), or the pid-file write (`server.rs`).
- [ ] Run the same command plus the scripted-dialogs clippy. Expect PASS.
- [ ] Commit: `git add host/tests/service.rs host/src/service && git commit -m "test(service): cover host-to-service runs end to end with a fake harness"`

### Task 25: Dev LaunchAgent registration

**Files:**
- Create: `scripts/dev-service.mjs`, `scripts/tests/dev-service.test.mjs`
- Modify:
  - `scripts/register-dev-host.mjs`: usage, argument parsing, and the steps after the manifest write.
  - `package.json`: add `"test:scripts": "node --test scripts/tests/dev-service.test.mjs"`.
  - `.github/workflows/ci.yml`: add `npm run test:scripts` after `test:extension`.

**Interfaces:**
- `export function servicePlist({ label, binary, home, user }): string` builds an XML plist:
  - It begins with `<!-- ${APP_NAME} development service -->`.
  - `Label`; `ProgramArguments [binary, "service"]`.
  - `EnvironmentVariables {HOME, USER, LANG: "en_US.UTF-8", PATH: "/opt/homebrew/bin:/usr/local/bin:<home>/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin"}`.
  - `RunAtLoad false`, no `KeepAlive`, `ProcessType Standard`.
  - `StandardOutPath`/`StandardErrorPath` set to `<home>/Library/Logs/Brauser/service.log`.
  - All strings are XML-escaped, and any relative path throws.
- `export function plistPath(home, label)` and `export function isDevPlist(text)`.
- `register-dev-host.mjs`:
  - The `<extensionId> <binary>` contract is unchanged.
  - On darwin it writes the plist (temp file plus rename, mode 0644), refuses to replace a plist without the dev marker, and creates the Logs dir.
  - It **always** runs `/bin/launchctl bootout gui/<uid>/<label>` (failure ignored), then `bootstrap gui/<uid> <plist>`. Re-running it after a rebuild is the documented way to replace the running service.
  - `--no-service` skips the service steps. `--unregister-service` takes no positional arguments, boots the service out and removes a dev plist.

- [ ] Write the failing tests:
  ```js
  test("plist runs the binary's service subcommand on demand", () => {
    const xml = servicePlist({ label: SERVICE_LABEL, binary: "/a/brauser", home: "/Users/u", user: "u" });
    assert.match(xml, /<string>\/a\/brauser</string>\s*<string>service</string>/);
    assert.match(xml, /\/opt\/homebrew\/bin:\/usr\/local\/bin:\/Users\/u\/.local\/bin/);
    assert.match(xml, /<key>RunAtLoad</key>\s*<false\/>/);
    assert.doesNotMatch(xml, /KeepAlive/);
    assert.ok(isDevPlist(xml));
  });
  test("paths are escaped and must be absolute", () => {});
  test("plist lints", { skip: process.platform !== "darwin" }, () => { /* plutil -lint on a tmp file */ });
  test("registration always boots out before bootstrapping", () => { /* injected runner records ["bootout", …], then ["bootstrap", …] */ });
  ```
- [ ] Run `npm run test:scripts`. Expect FAIL.
- [ ] Implement. Give `register-dev-host.mjs` an injectable `run(argv)` for the test.
- [ ] Run `npm run test:scripts && npm run lint && npm run typecheck`. Expect PASS.
- [ ] Commit: `git add scripts/dev-service.mjs scripts/tests/dev-service.test.mjs scripts/register-dev-host.mjs package.json .github/workflows/ci.yml && git commit -m "feat(scripts): register the dev Brauser service as a per-user LaunchAgent"`

### Task 26: Headless smoke covers the service

**Files:**
- Modify `scripts/smoke-macos.mjs`:
  - `testHome` moves to `mkdtemp("/tmp/bsh-")` because of `sun_path`, and is removed in `finally`.
  - The host wrapper passes `BRAUSER_FAKE_HARNESS`.
  - `installFakeHarnesses` gains a `fake-agent` script.
  - New service steps go after the existing host checks.
  - The service child is killed in `finally`.

**Interfaces:**
- Consumes: `hostRequest(wrapper, request)` and `PROTOCOL_VERSION`. `SERVICE_LABEL` is never passed to launchctl here.
- The fake agent is a `/bin/sh` script whose behavior comes from `$HOME/fake-agent.mode`:
  - `ok`: prints `working`, `session: smoke-session` and `{"ok":true}`.
  - `slow`: `sleep 60`.
  - `fail`: writes `fake failure` to stderr and exits 3.
- Every direct `start_run` carries `requested_at`.

- [ ] Add the steps:
  1. Before the service runs, `start_run` gives `service_unavailable`.
  2. Spawn `wrapper service` (`env -i`, temp HOME) and wait for `service.sock`.
  3. `ok`: poll until `succeeded`. Assert `stream_text` includes `working`, `session_id === "smoke-session"`, and that the five run files exist.
  4. `slow`: a second start on the same URL returns the same `run_id` with `existing === true`. `cancel_run` gives `canceled`, and `process.kill(-process_group, 0)` throws `ESRCH`.
  5. `fail`: `failed`/`harness_failed`, with `error` containing `fake failure`.
  6. `list_runs` contains all three runs.
- [ ] Run the smoke command. Expect FAIL before the steps are wired and PASS after.
- [ ] Commit: `git add scripts/smoke-macos.mjs && git commit -m "test(smoke): exercise service runs, reattach, cancel, and failure headlessly"`

### Task 27: Docs, backlog, and full gate

**Files:**
- Modify `DESIGN.md`:
  - Principle 8: an on-demand per-user service exists.
  - §12.2: a new step for this work, pointing at the spec, with PR 2 marked as the service skeleton. Mark the harness parts of steps 8.2-8.5, and 8.3's host-side streaming protocol, as superseded by it. Record that record keys and summary-file identity carry over.
  - §14, dated 2026-10-26: the "where a Chrome-spawned host gets auth values" question is closed by agent.env, and the build-identity check is recorded.
- Modify `AGENTS.md`:
  - "Current state".
  - Security rules: add the service, the 0700 support dir and 0600 socket, the agent.env 0600 rule with settings showing names only, the build check, and that tests never call launchctl.
- Modify `README.md`: dev step 3 also registers the service, and re-registration after a rebuild replaces it; document `--unregister-service`.
- Modify `TESTING.md`:
  - External programs: the runner starts the service child, the fake agent, and the short HOME under /tmp.
  - "Cannot prove": launchd bootstrap and kickstart, and SMAppService.
  - A learning-log line about the 104-byte socket limit.
- Modify `BOSUN_FEATURE_BACKLOG.md`: mark the PR 2 item done; close `brwr-9yc` (decided: agent.env, spec §5.1); mark `brwr-jfe` and `brwr-0cj` superseded by `get_run` polling.
- Modify `protocol/README.md`: the v6 run messages, `service_hello`, `requested_at` and `service_unavailable`.
- Modify `scripts/check_current_state.py`, only if required phrases change.

- [ ] Update each section.
- [ ] Run the full quality gate, including the non-macOS `cargo check` per Global Constraints. Expect PASS.
- [ ] Record a **[human]** check in TESTING.md and the backlog item:
  1. Register the dev build and close Chrome.
  2. Run `start_run` from the panel console.
  3. Confirm that `launchctl print gui/$UID/com.rauser.brauser.service` shows the service, and that it exits after 10 idle minutes.
  4. Confirm that agent.env values reach a run.
  5. Confirm that a rebuild plus a request restarts the service.
- [ ] Commit: `git add DESIGN.md AGENTS.md README.md TESTING.md BOSUN_FEATURE_BACKLOG.md protocol/README.md scripts/check_current_state.py && git commit -m "docs: record the Brauser service, agent.env, and polling run protocol"`

---

## PR 3: Real runners, extraction, and the panel run card

PR 3 replaces the fake-only registry with the real runners from spec §6.2 and §6.3:
- Codex runs as a `codex app-server` JSON-RPC thread with full access.
- Claude Code runs as `claude -p` stream-json with `--dangerously-skip-permissions`. Spec §6.3 specifies this flag as the equivalent of Codex's full access.

Both run in the knowledge base folder with the §6.4 prompt, under strict answer validation with one repair turn. Harness detection and the probe move into the service, and the harness lockdown code is deleted.

In the extension, every HTTP(S) page gets **Summarize & log**, a `summarize` command and a context-menu item. The page is extracted with a pinned, bundled Readability. A polling run card is restored from `list_runs`.

The summary file and log event stay in PR 4. In PR 3, `summary_path` is always null, and the run card has no Open summary control (Task 52 adds it).

Assumes PRs 1 and 2 are merged. PR 3 changes three PR 2 interfaces:
- `RunFinisher::accept` gains `run: &RunStatusFile`.
- `RunStatusFile::view` takes the parsed result.
- `RunSpec.cwd` becomes the knowledge base folder.

Commands are as in PR 1, plus **Rust:** `cargo test --locked -p brauser <filter>`.

---

### Task 28: Line-streaming child process primitive

**Files:**
- Create: `host/src/service/child.rs`
- Modify: `host/src/service/command_runner.rs` (use `LineChild`), `host/src/service/mod.rs`

**Interfaces:**
```rust
pub const MAX_LINE_BYTES: usize = 1024 * 1024;
pub enum ChildLine { Stdout(String), Stderr(Vec<u8>), Closed }
pub struct LineChild { /* Child, Option<ChildStdin>, Receiver<ChildLine> */ }
impl LineChild {
    pub fn spawn(slot: &ProcessSlot, command: Command) -> io::Result<Self>;
    pub fn write_line(&mut self, line: &str) -> io::Result<()>;
    pub fn close_stdin(&mut self);
    pub fn next(&self, timeout: Duration) -> Option<ChildLine>;
    pub fn wait(self) -> io::Result<ExitStatus>;
}
```
- Each pipe has one reader thread. Stdout is split on `\n` and decoded lossily.
- An overlong line is cut at a char boundary, and the rest is discarded up to the next `\n`.
- Stderr arrives in chunks of at most 64 KiB.
- `Closed` is sent once, after both pipes reach EOF.

- [ ] Write the failing tests: `stdout_lines_split_across_writes_arrive_whole`, `overlong_line_is_truncated_and_next_line_survives`, `stderr_is_separate_from_stdout`, `write_line_reaches_child_stdin` and `killed_slot_yields_closed`.
- [ ] Run `cargo test --locked -p brauser service::child`. Expect FAIL.
- [ ] Implement, then switch `CommandRunner::turn` to `LineChild` without changing its behavior.
- [ ] Run `cargo test --locked -p brauser service::`. Expect PASS.
- [ ] Commit: `git add host/src/service/child.rs host/src/service/command_runner.rs host/src/service/mod.rs && git commit -m "refactor(service): add line-streaming child primitive shared by runners"`

### Task 29: Codex app-server runner

**Files:**
- Create: `host/src/service/codex.rs`
- Modify: `host/src/service/mod.rs`; `host/src/harness.rs` (`fn find_on_path` becomes `pub(crate)`, in both its `cfg(unix)` and `cfg(not(unix))` variants); `TESTING.md` (External programs)

**Interfaces:**
```rust
pub const APP_SERVER_ARGS: [&str; 3] = ["app-server", "--listen", "stdio://"];
pub const RPC_TIMEOUT: Duration = Duration::from_secs(30);
pub struct CodexRunner { search_path: OsString, output_schema: Option<serde_json::Value> }
impl CodexRunner { pub fn new(search_path: OsString, output_schema: Option<serde_json::Value>) -> Self; }
impl HarnessRunner for CodexRunner { /* harness() == HarnessId::Codex */ }
pub fn thread_start_params(cwd: &Path) -> Value;   // {cwd, approvalPolicy:"never", sandbox:"danger-full-access"}
pub fn turn_start_params(thread_id: &str, cwd: &Path, prompt: &str, schema: Option<&Value>) -> Value;
    // {threadId, cwd, approvalPolicy:"never", sandboxPolicy:{type:"dangerFullAccess"}, input:[{type:"text",text:prompt}], [outputSchema]}
pub fn thread_name(title: &str) -> String;          // "Brauser: <title>", ≤ 100 chars
enum Notice { Delta { item: String, text: String }, Completed { item: String, text: String }, TurnDone { failed: Option<String> }, Other }
fn parse_notice(value: &Value) -> Notice;
```
- The protocol is JSON-RPC 2.0 over newline-delimited JSON, with ids counting up from 1.
- `start`:
  1. Resolves `codex` with `find_on_path` and spawns it with `APP_SERVER_ARGS`, `cwd = spec.cwd`, and `env_clear()` followed by `spec.env`.
  2. Sends `initialize` (`{clientInfo:{name:"brauser",version:CARGO_PKG_VERSION}, capabilities:{experimentalApi:true}}`), then `thread/start`, then `thread/name/set`. An error from `thread/name/set` becomes a `Diagnostic` only.
  3. Emits `Session(CodexThread(id))`.
- Each `turn` sends `turn/start` and reads until `turn/completed`:
  - `item/agentMessage/delta` becomes `Text`.
  - `item/completed` of type `agentMessage` sets that turn's final text.
  - `status:"failed"` gives `Failed{turn.error.message}`.
  - `Closed` first gives `Failed{"codex app-server exited before the turn finished"}`.
- A second `turn` reuses the thread and first emits `Text("\n\n")`.
- `close` closes stdin, waits 2 s, then calls `slot.kill()`.

- [ ] Verify the installed flags (writes only under `target/`):
  1. Run `codex --version` and `codex app-server generate-json-schema --out target/codex-schema`.
  2. Run `grep -rl '"outputSchema"' target/codex-schema` to check whether TurnStart params accept `outputSchema`.
  3. In TESTING.md, record the version, the argv, the `sandbox`/`sandboxPolicy` casing difference, the absence of `--session-source`, and the `outputSchema` finding.
- [ ] Write the failing tests. The fake is a `#!/bin/sh` script that answers ids 1-4 in order and logs stdin to `rpc.log` and env names to `env.log`.
  - `handshake_starts_a_full_access_thread_in_cwd`
  - `deltas_stream_and_completed_item_is_the_answer`
  - `failed_turn_reports_codex_error_message`
  - `exit_before_turn_completed_is_a_failure`
  - `second_turn_reuses_the_thread`
  - `second_turn_after_app_server_exit_fails_cleanly`
  - `thread_name_is_prefixed_and_bounded`
  - `output_schema_is_sent_only_when_configured`
  - `service_env_and_codex_home_reach_codex`
- [ ] Run `cargo test --locked -p brauser service::codex`. Expect FAIL.
- [ ] Implement. `parse_notice` follows the shapes recorded in TESTING.md.
- [ ] Run `cargo test --locked -p brauser service::codex && cargo clippy --locked --workspace --all-targets -- -D warnings`. Expect PASS.
- [ ] Commit: `git add host/src/service/codex.rs host/src/service/mod.rs host/src/harness.rs TESTING.md && git commit -m "feat(service): Codex app-server runner with full-access thread"`

### Task 30: Claude Code stream-json runner

**Files:**
- Create: `host/src/service/claude.rs`
- Modify: `host/src/service/mod.rs`, `TESTING.md`

**Interfaces:**
```rust
pub struct ClaudeRunner { search_path: OsString }
impl ClaudeRunner { pub fn new(search_path: OsString) -> Self; }
impl HarnessRunner for ClaudeRunner { /* harness() == HarnessId::Claude */ }
pub fn first_turn_args(session_id: &Uuid) -> Vec<String>;
    // ["-p","--output-format","stream-json","--verbose","--include-partial-messages","--dangerously-skip-permissions","--session-id",<uuid>]
pub fn resume_turn_args(session_id: &str) -> Vec<String>; // same flags, "--resume",<id> instead of "--session-id"
pub enum ClaudeLine { Session(String), Delta(String), AssistantText(String), Result { is_error: bool, text: String }, Other }
pub fn parse_line(line: &str) -> ClaudeLine;
```
- Event mapping:
  - `system/init` gives `Session`.
  - `stream_event` `content_block_delta` with `text_delta` gives `Delta`.
  - `assistant` text gives `AssistantText`, emitted as `Text` only when no delta arrived this turn.
  - `result` gives `Result`.
- The session id is pre-assigned as `Uuid::new_v4()`. The init id is recorded through `Session(ClaudeSession(id))`.
- Each `turn` spawns `claude` through `ProcessSlot`, with the prompt on stdin, then closes stdin. It runs with `cwd = spec.cwd` and `env_clear()` followed by `spec.env`.
- A turn succeeds only with exit 0 and `Result{is_error:false}`. Anything else gives `Failed`, using the result text, otherwise the last stderr line.
- Never pass `--no-session-persistence`.

- [ ] Verify the flags with `claude --version` and `claude --help`, and record the argv and version in TESTING.md.
- [ ] Write the failing tests: `parses_init_delta_assistant_and_result_lines`, `argv_is_full_access_print_mode_and_prompt_is_on_stdin`, `is_error_result_fails_even_with_exit_zero`, `nonzero_exit_fails_with_last_stderr_line`, `deltas_suppress_duplicate_assistant_text` and `second_turn_resumes_the_init_session`.
- [ ] Run `cargo test --locked -p brauser service::claude`. Expect FAIL.
- [ ] Implement.
- [ ] Run the same command. Expect PASS.
- [ ] Commit: `git add host/src/service/claude.rs host/src/service/mod.rs TESTING.md && git commit -m "feat(service): Claude Code stream-json runner with resumable session"`

### Task 31: Production registry, knowledge base cwd, per-run timeout

**Files:**
- Modify:
  - `host/src/service/harness_api.rs` (`from_lookup`)
  - `host/src/service/server.rs` (start_run `cwd`, harness choice and timeout)
  - `host/src/service/supervisor.rs` (`StartRun`)
  - `host/src/harness.rs` (`fn search_path` becomes `pub(crate)`)
  - `host/src/config.rs` (a temporary `impl From<HarnessAdapter> for HarnessId`, deleted in Task 34)

**Interfaces:**
- `from_lookup(get)`:
  - Without `FAKE_HARNESS_ENV`, or in a non-test build, it registers `CodexRunner::new(search_path(PATH, HOME), None)` and `ClaudeRunner::new(..)`.
  - With it, PR 2's fake behavior is unchanged.
- `start_run` harness and cwd:
  - The harness is `request.harness.or(config.agent.map(|a| HarnessId::from(a.adapter)))`. If neither is set, the answer is `not_configured` "Set up a summary harness in Brauser settings first."
  - `cwd = ConfigStore::knowledge_base_dir()`.
  - The folder must be a directory with `vault::selected_root_identity(&kb) == config.knowledge_base_identity()`. Otherwise the answer is `not_configured` "The knowledge base folder changed or is missing. Choose it again in settings."
- `StartRun` gains `pub timeout: Option<Duration>`, set from `agent.timeout_secs`. The supervisor uses `timeout.unwrap_or(options.run_timeout)`. The scripted `RUN_TIMEOUT_SECS_ENV` still wins.

- [ ] Write the failing tests: `production_registry_serves_codex_and_claude`, `start_run_uses_the_knowledge_base_folder_as_cwd`, `start_run_without_agent_or_harness_is_not_configured`, `replaced_knowledge_base_folder_is_refused`, `agent_timeout_secs_bounds_the_run` and `harness_adapter_maps_to_harness_id`.
- [ ] Run `cargo test --locked -p brauser service::server service::harness_api`. Expect FAIL.
- [ ] Implement.
- [ ] Run `cargo test --locked -p brauser service::`. Expect PASS.
- [ ] Commit: `git add host/src/service/harness_api.rs host/src/service/server.rs host/src/service/supervisor.rs host/src/harness.rs host/src/config.rs && git commit -m "feat(service): real runners by default, knowledge base cwd, per-run timeout"`

### Task 32: The §6.4 prompt and an editable type/category vocabulary

**Files:**
- Modify:
  - `protocol/schema.json`: add `$defs.SummaryVocabulary` and a required `ConfigSnapshot.summary_vocabulary`, then regenerate.
  - `host/src/config.rs`: `StoredConfig` gets a `#[serde(default)] summary_vocabulary` table; also update `validate_base`, `from_snapshot`/`From<StoredConfig>`, `empty_config` and `fixtures`.
  - `extension/protocol-shape.ts` (`isConfig`), the `extension/tests/*` config fixtures, and the `scripts/smoke-macos.mjs` hand-built configs.
- Create: `host/src/service/prompt.rs`
- Modify: `host/src/service/server.rs`: `RunSpec.prompt = summary_prompt(..)`.

**Interfaces:**
- Wire `SummaryVocabulary { document_types: string[], categories: string[] }`: 1-64 items, each 1-64 chars. It is editable through plain `update_config`.
- `pub const DEFAULT_DOCUMENT_TYPES: [&str; 6] = ["report","announcement","ticket","specification","email","article"];` and `pub const DEFAULT_CATEGORIES: [&str; 7] = ["technical specification","product plan","administrative","project plan","marketing material","research paper","news"];`. Validation rejects case-insensitive duplicates, control characters and an empty list.
- `pub fn summary_prompt(page_path: &Path, knowledge_base: &Path, vocabulary: &SummaryVocabulary) -> String`:
  - It names `page_path` by absolute path and marks its content as untrusted, telling the agent to ignore any instructions inside it.
  - It tells the agent to read the knowledge base in its working directory to judge importance.
  - It gives the exact §6.4 JSON shape and both vocabularies, with "pick one; propose a new value only if none fits".
  - It requires `related_notes` relative to the knowledge base.
  - It ends with "Your final message must be only the JSON object."
  - It never includes page text.
- `pub fn repair_prompt(problem: &str) -> String`: "Your previous answer was not valid: <problem>. Reply with only the corrected JSON object, nothing else."

- [ ] Write the failing tests: `prompt_names_page_by_path_and_marks_it_untrusted` (it also contains every default type, `"importance"` and `high | medium | low`), `prompt_never_contains_page_text`, `v5_config_without_vocabulary_loads_the_defaults`, `duplicate_vocabulary_entry_is_invalid_config` and wire `config_requires_summary_vocabulary`.
- [ ] Run `cargo test --locked -p brauser service::prompt v5_config_without_vocabulary`. Expect FAIL.
- [ ] Implement the schema change, regenerate, and implement.
- [ ] Run the regenerate command, `cargo test --locked --workspace`, `npm run typecheck`, the extension command and the smoke command. Expect PASS.
- [ ] Commit: `git add -A protocol host/src/config.rs host/src/service/prompt.rs host/src/service/server.rs extension/protocol-shape.ts extension/tests scripts/smoke-macos.mjs && git commit -m "feat(service): §6.4 summary prompt with editable type and category vocabulary"`

### Task 33: Validated structured answer with one repair turn

**Files:**
- Create: `host/src/answer.rs`. It is cross-platform (`pub(crate) mod answer;` in `lib.rs`, not under `service`), so PR 4's `summary.rs`, `capture.rs` and `index.rs` can import it.
- Modify:
  - `host/src/service/supervisor.rs`: the `RunFinisher::accept` signature, the repair turn, `VaultCheckFinisher::accept`.
  - `host/src/service/record.rs`: `RunStatusFile::view`.
  - `host/src/service/harness_api.rs`: pass `Some(answer_schema())` to `CodexRunner` only if Task 29 found `outputSchema` support.
- Modify `protocol/schema.json`: add `$defs.ImportanceLevel` (`["high","medium","low"]`), `$defs.RunAnswer` and a required `RunView.answer`, then regenerate.
- Modify: `extension/protocol-shape.ts` (`isRunView`)

**Interfaces:**
```rust
use brauser_protocol::ImportanceLevel;   // the single importance enum used everywhere (PR 4 included)
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)] pub struct Entities { pub technologies: Vec<String>, pub products: Vec<String>, pub people: Vec<String>, pub companies: Vec<String> }
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)] pub struct Importance { pub level: ImportanceLevel, pub reason: String }
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)] pub struct SummaryAnswer { pub document_type: String, pub category: String, pub entities: Entities,
    pub description: String, pub importance: Importance, pub assessment: String, pub related_notes: Vec<String> }
pub fn parse_answer(final_text: &str, knowledge_base: &Path) -> Result<SummaryAnswer, String>;
pub fn answer_schema() -> serde_json::Value;
```
- `parse_answer` validation:
  - Trim, then strip one surrounding fenced block. The result must be a single JSON object, and unknown keys are ignored.
  - `document_type` and `category`: 1-64 chars, with no control characters.
  - Entity lists: at most 32 items of 1-128 chars each. Blanks are dropped and duplicates removed.
  - `description` is 1-2000 chars, `importance.reason` 1-300 and `assessment` 1-4000.
  - `level` is matched case-insensitively.
  - `related_notes`: at most 32 are kept. An entry is dropped silently when it is absolute, contains `..`, is missing, is not a regular file, or canonicalizes outside the knowledge base.
  - Every error names its field.
- `result.json` stores the normalized `SummaryAnswer` re-serialized, so PR 4 can deserialize it strictly.
- The PR 2 interfaces change:
  - `fn accept(&self, run: &RunStatusFile, final_text: &str) -> Result<Value, String>`. `VaultCheckFinisher` runs `let Some(cwd) = run.cwd.as_deref() else { return Err("run has no working directory".into()) };`, then `parse_answer(final_text, Path::new(cwd))`, then `serde_json::to_value`.
  - `RunStatusFile::view(&self, stream_bytes: u32, result: Option<&Value>) -> RunView`. `can_retry_save` uses `result.is_some()`.
- Wire `RunAnswer {document_type, category, importance: ImportanceLevel, importance_reason}`; `RunView.answer: RunAnswer|null`, filled from `result.json`.
- The supervisor's repair turn:
  - On `accept` `Err(e)`, call `session.turn(&repair_prompt(&e))` once, then `accept` again.
  - A second `Err` gives `failed`/`invalid_output`, "The agent's answer was not valid after one correction: <e>".
  - If the repair turn itself returns `Failed`, the run is `failed`/`harness_failed`.
  - `stream.txt` keeps both turns.

- [ ] Write the failing tests:
  - `valid_answer_parses_and_lowercases_level`
  - `fenced_json_is_accepted`
  - `related_notes_outside_or_missing_are_dropped`
  - `overlong_description_names_the_field`
  - `missing_importance_level_is_invalid`
  - supervisor `invalid_answer_gets_one_repair_turn_and_succeeds`
  - supervisor `second_invalid_answer_fails_invalid_output_and_keeps_stream`
  - supervisor `empty_final_text_gets_a_repair_turn` (Review Focus 3)
  - supervisor `repair_turn_after_harness_exit_fails_as_harness_failed` (Review Focus 3)
  - supervisor `accept_without_cwd_fails_invalid_output`
  - JS `isRunView requires answer and rejects an unknown importance`
- [ ] Run `cargo test --locked -p brauser answer:: service::supervisor`. Expect FAIL.
- [ ] Implement, regenerate, and update `TestFinisher` to the new signature.
- [ ] Run the regenerate command, `cargo test --locked --workspace`, `npm run typecheck` and the extension command. Expect PASS.
- [ ] Commit: `git add -A protocol host/src/answer.rs host/src/lib.rs host/src/service extension/protocol-shape.ts extension/tests/agent.test.mjs && git commit -m "feat(service): validate the structured summary answer with one repair turn"`

### Task 34: Harness setup in the service: detect on its PATH, probe the real invocation

**Files:**
- Modify `protocol/schema.json`, then regenerate:
  - `AgentConfig` becomes `{harness: HarnessId, binary, version: string|null, timeout_secs (60..3600), probed_at}`.
  - `HarnessOffer` becomes `HarnessFound`, and `HarnessesDiscovered.offers` becomes `harnesses`.
  - `SetUpHarnessRequest` becomes `{expected_revision, harness: HarnessId, timeout_secs: integer|null, summaries_dir: string|null}`.
  - Delete `CheckAgentRequest`/`AgentChecked`, `EnvName` and `HarnessAdapter`.
- Create: `host/src/service/setup.rs`
- Delete: `host/src/harness_setup.rs`
- Modify:
  - `host/src/harness.rs`: add `detect`; `HarnessEnv` drops `vars`/`work_root`.
  - `host/src/config.rs`:
    - The new AgentConfig is stored in a TOML table `[summarizer]` (`StoredSummarizer {harness, binary, version, timeout_secs, probed_at}`). The wire field stays `ConfigSnapshot.agent`, mapped from and to `summarizer`.
    - The v5 `agent` and `harness_record` keys become retired `#[serde(default, skip_serializing, rename = …)] Option<toml::Value>` values. A v5 file therefore loads as not set up and drops them on its next save.
    - `agent_gate`/`agent_status`: ready means `summarizer.is_some()`.
    - `HarnessCommit { summarizer: StoredSummarizer }`.
    - Delete `validate_agent`, `HarnessRecord`, `StoredAgent`, `StoredHarness`, `parse_agent_table`, the `agent-work` sweep and the temporary `From<HarnessAdapter>`.
  - `host/src/service/server.rs`: answer `discover_harnesses` and `set_up_harness`.
  - `host/src/native.rs`: forward both to `ServiceLink` (non-macOS answers `service_unavailable`); delete `CheckAgent` and `check_agent`; drop `SetupContext.env`.
  - `host/src/lib.rs`, `extension/settings.ts`, `extension/protocol-shape.ts`, and `scripts/smoke-macos.mjs` (remove the `check_agent` calls).
- Rewrite: `host/tests/harness_setup.rs`, which now spawns `brauser service` (`SERVICE_SPAWN_ENV=1`) with fake `codex`/`claude` on `PATH`.

**Interfaces:**
- Wire `HarnessFound {harness: HarnessId, binary: string|null, version: string|null, problem: string|null}`. Discovery always returns both harnesses, codex then claude.
```rust
// harness.rs
pub struct Detected { pub adapter: Adapter, pub binary: Option<PathBuf>, pub version: Option<String>, pub problem: Option<String> }
pub fn detect(env: &HarnessEnv, run_env: &[(OsString, OsString)]) -> Vec<Detected>; // find_on_path, check_install, --version under VERSION_LIMITS; no version refusal
impl From<Adapter> for HarnessId
// service/setup.rs
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(90);
pub fn discover(env: &HarnessEnv, run_env: &[(OsString, OsString)]) -> Vec<HarnessFound>;
pub fn probe(runner: &dyn HarnessRunner, spec: &RunSpec, timeout: Duration) -> Result<(), String>;
pub fn set_up(config: &mut ConfigStore, registry: &HarnessRegistry, request: &SetUpHarnessRequest, env: Vec<(OsString, OsString)>) -> Result<(ConfigSnapshot, String), (ErrorCode, String)>;
```
- `probe` sends `harness::PROBE_PROMPT` with cwd = the knowledge base, `page_path = /dev/null` and the agent.env overlay. A timer thread calls `slot.kill()` at the timeout. Success requires `Answered` whose trimmed, case-folded text contains `ok`.
- `set_up`:
  1. Detects and probes with no lock held.
  2. Takes `lock_current`, refreshes, and checks `expected_revision`; a mismatch is `conflict`.
  3. Commits `StoredSummarizer{harness, binary, version, timeout_secs: request or 600, probed_at: now}` plus `summaries_dir` (the request's, else the existing, else `DEFAULT_SUMMARIES_DIR`) through `update_with_grants(.., Some(HarnessCommit{summarizer}))`.

  A probe failure is `invalid_config`, naming the harness and the reason. `check_agent_fields` still limits `update_config` to echoing or clearing `agent`.
- TS: `HostSession.discoverHarnesses(): Promise<HarnessFound[]>` and `HostSession.setupHarness(harness: HarnessId, timeoutSecs: number | null, summariesDir: string | null): Promise<void>` (`timeoutMs` 120 000).

- [ ] Write the failing tests:
  - `v5_agent_and_harness_record_tables_load_as_not_set_up` (no config issue, `backup_count == 0`, and after save the file has no `[agent` or `harness_record`)
  - `summarizer_table_round_trips_as_wire_agent`
  - `discover_reports_both_harnesses_with_versions_from_the_service_path`
  - `unreviewed_codex_version_is_not_refused`
  - `passing_probe_commits_agent_and_reports_ready`
  - `failing_probe_leaves_config_unchanged`
  - `stale_revision_after_probe_is_a_conflict`
  - `discover_harnesses_is_forwarded_to_the_service`
  - `update_config_cannot_set_an_agent`
  - in `host/tests/harness_setup.rs`: `service_detects_and_probes_fake_claude` and `probe_runs_with_agent_env_overlay`
- [ ] Run `cargo test --locked -p brauser v5_agent_and_harness_record service::setup`. Expect FAIL.
- [ ] Implement and regenerate.
- [ ] Run the regenerate command, `cargo test --locked --workspace`, the scripted-dialogs test, both clippy commands, `npm run typecheck`, the extension command, and the non-macOS check. Expect PASS.
- [ ] Commit: `git add -A protocol host/src host/tests/harness_setup.rs extension/settings.ts extension/protocol-shape.ts extension/tests scripts/smoke-macos.mjs && git commit -m "feat!: harness setup runs in the service: detect on its PATH and probe the real invocation"`

### Task 35: Delete the harness lockdown

**Files:**
- Modify `host/src/harness.rs`. Delete:
  - `REQUIRED_ENV`, `CODEX_HOME`, `CLAUDE_AUTH_NAMES`, `CLAUDE_TEMPLATE`, `CODEX_TEMPLATE`, `CLAUDE_CHOICES`/`CODEX_CHOICES`, `REVIEWED_EXTRA_DISABLES`, `REVIEWED_KEEP_ENABLED`, `HELP_LIMITS`
  - `template_args`, `codex_args`, `template_matches`, `reviewed`
  - `EnvName`, `known_env_names`, `required_env`, `proposed_env_names`
  - `Candidate`, `discover`, `examine`, and the help, flag and features checks
  - `WORK_ROOT_NAME`, `WORK_DIR_PREFIXES`, `WorkDir`, `prepare_work_root`, `sweep_stale_work_dirs`
  - `probe`/`probe_with_limits`, `ProbeFailure`, `judge_*`, `mentions_tool_use`, `CODEX_EVENTS`/`CODEX_TEXT_ITEMS`
  - From `Adapter`: `help_command`, `choices`, `auth_names` and `home_names`

  **Both** `run_bounded` variants (`cfg(unix)` and `cfg(not(unix))`) change to `run_bounded(program, args, env: &[(OsString, OsString)], limits)`, used only for `--version`.
- Delete: `host/tests/fixtures/harness/{claude_help,codex_exec_help,codex_features,codex_help}.txt`
- Tests: delete the template, help, features, probe-judgement, env-cleared, workdir and event-allowlist tests. Keep `search_path_keeps_absolute_entries_and_adds_local_bin`, `claude_version_must_parse_exactly`, `discover_uses_only_injected_search_path` (renamed `detect_…`), `discover_follows_symlink_to_real_path_identity`, `refuses_world_writable_binary_or_dir` and `discover_refuses_unparseable_version`.

**Interfaces:**
- Produces: `harness.rs` keeps `HarnessEnv`, `search_path`, `find_on_path`, `Adapter {harness_id, program, display_name}`, `BinaryIdentity`/`identity`, `check_install`, `parse_*_version`, `Detected`/`detect`, `PROBE_PROMPT` and the slimmed `run_bounded`.

- [ ] Write the failing test `version_check_passes_the_service_env_through`.
- [ ] Run `cargo test --locked -p brauser harness::process_tests::version_check_passes`. Expect FAIL.
- [ ] Implement the deletions.
- [ ] Run `cargo build --locked --workspace --all-targets && cargo test --locked --workspace && cargo clippy --locked --workspace --all-targets -- -D warnings` and the non-macOS check. Expect PASS with no dead-code warnings.
- [ ] Commit: `git add -A host/src/harness.rs host/tests/fixtures/harness && git commit -m "refactor!: delete the harness lockdown (templates, help checks, cleared env, work dirs)"`

### Task 36: Cross-platform opener, service `open_path`, and `get_agent_env`

**Files:**
- Modify `protocol/schema.json`, then regenerate. Add:
  - `$defs.OpenTarget` (`["codex_thread","agent_env"]`; there is no `summary` target, because PR 4's host-side `open_summary` owns that);
  - `OpenPathRequest {type:"open_path", …, target: OpenTarget, run_id: string|null}` → `PathOpened`;
  - `GetAgentEnvRequest` → `AgentEnvListed {names: string[] maxItems 64, problem: string|null}`.
- Create:
  - `host/src/open.rs`, cross-platform: `pub(crate) mod open;`.
  - `host/src/service/open.rs`.
- Modify:
  - `host/src/brand.rs`: `#[cfg(any(test, feature = "scripted-dialogs"))] OPEN_LOG_ENV = "BRAUSER_OPEN_LOG"`, defined here once only.
  - `host/src/service/supervisor.rs` (`ServiceOptions.opener`), `host/src/service/server.rs`, `host/src/native.rs` (forward both), `extension/protocol-shape.ts`.

**Interfaces:**
```rust
// host/src/open.rs (cross-platform)
pub type Opener = fn(args: &[OsString]) -> io::Result<()>;
pub fn default_opener() -> Opener;
    // macOS release: system_open (/usr/bin/open <args>; non-zero → Err)
    // scripted-dialogs build: append one line to $OPEN_LOG_ENV, or Err("open is disabled in test builds") when unset
    // other platforms: Err(Unsupported)
// host/src/service/open.rs
pub fn open_target(paths: &ServicePaths, store: &RunStore, target: OpenTarget, run_id: Option<Uuid>, opener: Opener) -> Result<(), (ErrorCode, String)>;
pub const AGENT_ENV_HEADER: &str = "# KEY=VALUE lines added to every Brauser agent run. Settings shows names, never values.\n";
```
- `open_target` cases:
  - `codex_thread`: the thread id must match `^[A-Za-z0-9_-]{1,128}$` (checked by hand), and the call is `opener(["codex://threads/<id>"])`.
  - `agent_env`: create the file at 0600 with the header if it is missing, then call `opener(["-R", path])`.
  - A wrong state is `not_configured` or `invalid_request`.
- `get_agent_env` returns `AgentEnv::read(..).names()`. An error becomes `problem` text, never values.

- [ ] Write the failing tests: `codex_thread_opens_the_deep_link`, `run_without_thread_is_not_opened`, `malformed_thread_id_is_refused`, `reveal_creates_a_private_agent_env_and_selects_it`, `agent_env_listing_shows_names_never_values`, `bad_agent_env_mode_is_a_problem_not_an_error` and `scripted_opener_without_log_env_refuses`.
- [ ] Run `cargo test --locked -p brauser service::open open::`. Expect FAIL.
- [ ] Implement and regenerate.
- [ ] Run the regenerate command, `cargo test --locked --workspace`, both clippy commands, `npm run typecheck` and the non-macOS check. Expect PASS.
- [ ] Commit: `git add -A protocol host/src/brand.rs host/src/open.rs host/src/lib.rs host/src/service host/src/native.rs extension/protocol-shape.ts && git commit -m "feat(service): open Codex threads and agent.env; list agent.env names"`

### Task 37: Settings: summary harness section

**Files:**
- Modify `extension/options.html`. Read `DESIGN.md` first. Replace the "AI harness (optional)" section with:
  - `#harness-warning`, containing `SETUP_WARNING`
  - `#detect-harnesses`
  - `#harness-list`, with one radio and version per harness
  - `#harness-timeout`, a minutes input defaulting to 10
  - `#test-harness` ("Test and use")
  - `#harness-state`
  - `#agent-env-names` and `#reveal-agent-env` ("Reveal agent.env in Finder")
  - `#vocab-types` and `#vocab-categories`, textareas with one entry per line
  - `#save-vocabulary`

  Keep `#summaries-dir` and `#remove-harness`.
- Modify:
  - `extension/options.ts`: delete the offers, env checkboxes and the PR 1 setup code; add the handlers.
  - `extension/agent.ts`: `harnessFoundText`, `agentStateText`, `agentEnvText`, `vocabularyFromText`.
  - `extension/settings.ts`: `listAgentEnv`, `revealAgentEnv`.
- Test: `extension/tests/options.test.mjs`, `extension/tests/agent.test.mjs`

**Interfaces:**
```ts
export function harnessFoundText(found: HarnessFound): string;   // "Codex 0.144.4" | "Claude Code not found on the service PATH" | "<name>: <problem>"
export function agentEnvText(names: string[], problem: string | null): string;
export function vocabularyFromText(text: string): string[];      // trim, drop blanks, dedupe case-insensitively
listAgentEnv(): Promise<{ names: string[]; problem: string | null }>;
revealAgentEnv(): Promise<void>;
```
- "Test and use" calls `setupHarness(selected, minutes * 60, summariesDir)` and reloads the config.
- On `service_unavailable`, `#harness-state` shows setup instructions keyed off the error code, followed by the host's message.
- Saving the vocabulary sends `update_config` with `summary_vocabulary`.

- [ ] Write the failing tests: `settings shows the exact setup warning`, `detect lists both harnesses with versions and Test sends set_up_harness` (`lastSetUp` deep-equals `{ harness: "codex", timeout_secs: 600, summaries_dir: "summaries", expected_revision: "r1" }`), `agent.env names are shown and values never requested`, `reveal agent.env sends open_path agent_env`, `vocabulary edits save through update_config` and `service_unavailable shows setup instructions`.
- [ ] Run the extension command. Expect FAIL.
- [ ] Implement.
- [ ] Run `npm run typecheck && npm run lint` and the extension command. Expect PASS.
- [ ] Commit: `git add extension/options.html extension/options.ts extension/agent.ts extension/settings.ts extension/tests && git commit -m "feat(settings): detect, test, and choose the summary harness; agent.env names; vocabulary"`

### Task 38: Bundle pinned Readability and a self-contained page extractor

**Files:**
- **Already done (commit "build: add @mozilla/readability 0.6.0 for plan Task 38"):** `@mozilla/readability@0.6.0` is installed with `--save-exact` and recorded in the `package.json` `dependencies` and `package-lock.json` (resolved from `registry.npmjs.org`, license Apache-2.0, `npm audit` 0 vulnerabilities). This is the first runtime npm dependency. Do not run `npm install`; `npm ci` installs it from the lockfile. The CI `dependency-review` job runs with no license allow/deny list, so it does not block Apache-2.0.
  - Record the `sha256` of `node_modules/@mozilla/readability/Readability.js` (after `npm ci`, e.g. `shasum -a 256` or `sha256sum`) as `READABILITY_SHA256` below. At install time it was `34dcab3d0832d0019f02990eed6b6124e029e8c32b9f0c6f2550544ff8dff174`; verify yours matches before pinning it.
- Modify `extension/build.mjs`:
  - Add `READABILITY_VERSION = "0.6.0"` and `READABILITY_SHA256` side by side.
  - When `node_modules/@mozilla/readability` is missing, fail with "Run npm ci first: @mozilla/readability is missing".
  - Copy `Readability.js` to `dist/vendor/readability.js` and `LICENSE.md` to `dist/vendor/readability.LICENSE.md`.
  - Fail on a hash mismatch.
  - Add `"extract.js"` to the module list.
- Create: `extension/extract.ts`, `extension/readability.d.ts`, `extension/tests/extract.test.mjs`
- Modify: `package.json` (`test:extension`)

**Interfaces:**
```ts
export interface ExtractLimits { maxRequestBytes: number; maxSelectionBytes: number } // 3 * 1024 * 1024, 64 * 1024
export interface ExtractedPage { title: string; content: string; selection: string | null; truncated: boolean; readable: boolean }
export const EXTRACT_LIMITS: ExtractLimits;
export function extractReadablePage(limits: ExtractLimits): ExtractedPage;
```
- The function must be self-contained, because Chrome serializes it. It may reference only `document`, `window`, `TextEncoder`, `JSON` and the global `Readability`.
  - It calls `new Readability(document.cloneNode(true), { serializer: (el) => el }).parse()`.
  - It walks the element tree into Markdown-ish text: headings become `#`s, `li` becomes `- `, `pre` text is kept, blocks are separated by a blank line, and link text is kept without URLs. `img`, `script` and `style` are dropped.
  - When Readability returns null, it falls back to `document.body.innerText` and sets `readable: false`.
- Caps are in **encoded bytes**:
  - The selection is truncated until `new TextEncoder().encode(JSON.stringify(selection)).length <= maxSelectionBytes`.
  - The content is truncated, by binary search on a char index, until the same JSON-encoded measure plus the selection's is `<= maxRequestBytes`. Then `truncated` is set.

  3 MiB of encoded content plus the envelope stays under the host's 4 MiB `MAX_INBOUND_BYTES` whatever the script or escaping.
- Text that is empty after trimming, from both Readability and the fallback, gives `content: ""` and `readable: false`. Task 39 treats that as unreadable.
- `readability.d.ts`: `declare class Readability { constructor(doc: Node, options?: { serializer?: (el: Element) => unknown }); parse(): { title: string; content: unknown } | null; }`

- [ ] Write the failing tests, using `vm.runInNewContext("(" + extractReadablePage + ")(limits)", fakes)`:
  - `extracts article blocks as text and drops images`
  - `falls back to body text when Readability finds no article`
  - `maximal extraction stays under the host frame cap` (Review Focus 2): 1 000 000 CJK characters with interleaved `"` and `\u0001`, plus a 65 536-char selection. The full `start_run` JSON is under 4 MiB and `truncated` is true.
  - `an empty page is unreadable, not an empty run`
  - `runs with only document, window and Readability in scope`
  - `the vendored license ships with the bundle` (`dist/vendor/readability.LICENSE.md` exists after the build)
- [ ] Run the extension command. Expect FAIL.
- [ ] Implement.
- [ ] Run the extension command, `npm run typecheck && npm run lint && npm audit`. Expect PASS, with `dist/vendor/readability.js` present.
- [ ] Commit: `git add package.json extension/build.mjs extension/extract.ts extension/readability.d.ts extension/tests/extract.test.mjs && git commit -m "feat(extension): bundle pinned Readability 0.6.0 and a byte-capped page extractor"`

### Task 39: Summarize & log button, `summarize` command, context-menu item

**Files:**
- Modify `extension/manifest.json`: add `commands.summarize` with `suggested_key` default `Ctrl+Shift+9`, mac `Command+Shift+9`, and description `"Summarize this page with {{APP_NAME}}"`. `contextMenus` and `scripting` are already present from Task 6.
- Create: `extension/summarize.ts`
- Modify:
  - `extension/build.mjs` (`"summarize.js"`)
  - `extension/chrome.d.ts`: `scripting.executeScript`, `commands`, `contextMenus`, `runtime.sendMessage`, `storage.session`
  - `extension/model.ts` (`PanelNotice`)
  - `extension/worker.ts`: command and menu handlers beside `setPanelBehavior`
  - `extension/panel.html`: `#summarize` and `#run-area` in `#page-view`
  - `extension/panel.ts`, `extension/panel.css`
- Test: `extension/tests/panel.test.mjs`, `extension/tests/worker.test.mjs`

**Interfaces:**
```ts
export interface SummarizeDeps { scripting: typeof chrome.scripting; host: Pick<HostClient, "call">; now(): Date }
export async function summarizeTab(tab: { id: number; url: string; title: string }, deps: SummarizeDeps): Promise<{ run: RunView; existing: boolean }>;
export type PanelNotice = { kind: "run_started"; run_id: string; tab_id: number; at: number } | { kind: "summarize_failed"; message: string; tab_id: number; at: number };
export const PANEL_NOTICE_KEY: string;  // storageKey("panel_notice_v1"), in chrome.storage.session
export const SUMMARIZE_MENU_ID: string; // `${BINARY_NAME}-summarize`, title `Summarize with ${APP_NAME}`, contexts ["page","selection"]
```
- `summarizeTab`:
  1. Runs `executeScript({target:{tabId, frameIds:[0]}, files:["vendor/readability.js"]})`, then `executeScript({target, func: extractReadablePage, args:[EXTRACT_LIMITS]})`.
  2. If the script throws, or the result has empty `content`, it throws `HostError("unreadable", "Brauser can't read this page's content.")`.
  3. Otherwise it calls `host.call({type:"start_run", …, url: tab.url, title: page.title || tab.title, content, selection, harness: null, record_key: null, requested_at: localTimestamp(deps.now())}, "run_started")`.
- Worker:
  - The command and the menu click call `chrome.sidePanel.open({tabId})` synchronously, before any await.
  - They then run `summarizeTab` and build a `PanelNotice`.
  - The notice is written to `chrome.storage.session[PANEL_NOTICE_KEY]` **and** sent by `chrome.runtime.sendMessage`. A send error, such as a panel that has not loaded, is ignored.
  - A non-HTTP(S) tab gets `summarize_failed` "Summaries work on http(s) pages.".
  - Host errors (`service_unavailable`, `not_configured`) become `summarize_failed` with the host message.
- Panel:
  - On load, the panel reads `PANEL_NOTICE_KEY`. It shows a notice for the active tab that is under 2 minutes old, then clears the key. It also listens for live messages.
  - `#summarize` is visible on every HTTP(S) tab. It is enabled only when `agent_status.state === "ready"`; otherwise its title is "Set up a summary harness in settings first.".
  - A click calls `summarizeTab` and hands the run to the run card (Task 40).
  - The first click ever shows `SETUP_WARNING` once, non-blocking, in `#run-area` with "Got it". The flag is `storageKey("summarize_warning_seen_v1")` in `storage.local`.

- [ ] Write the failing tests:
  - panel `Summarize & log shows on any HTTP(S) tab and needs a ready harness`
  - panel `clicking Summarize extracts in the top frame then sends start_run with no record key and a local requested_at` (`requested_at` matches `/[+-]\d\d:\d\d$/`)
  - panel `an executeScript failure shows "can't read this page"`
  - panel `the setup warning shows once`
  - panel `a summarize failure sent before the panel loads is shown when it opens` (Review Focus 1)
  - worker `summarize command opens the panel before any await and starts a run` (the host saw `start_run` with `requested_at`; the notice is stored and sent)
  - worker `service_unavailable from the command path is stored as a notice`
  - worker `context menu item is registered on install`
- [ ] Run the extension command. Expect FAIL.
- [ ] Implement. Read `DESIGN.md` before changing the panel layout.
- [ ] Run `npm run typecheck && npm run lint` and the extension command. Expect PASS.
- [ ] Commit: `git add extension/manifest.json extension/summarize.ts extension/build.mjs extension/chrome.d.ts extension/model.ts extension/worker.ts extension/panel.html extension/panel.ts extension/panel.css extension/tests && git commit -m "feat(extension): Summarize & log button, summarize command, and context-menu item"`

### Task 40: Run card: polling, Cancel, Retry save, Open in Codex / Copy resume command, restore

**Files:**
- Create: `extension/run-card.ts`, `extension/tests/run-card.test.mjs`
- Modify: `extension/build.mjs` (`"run-card.js"`), `package.json` (`test:extension`), `extension/panel.ts` (construct `RunCards` on `#run-area`, `restore()` on load, `PanelNotice`), `extension/panel.css` (`.run-card`)

**Interfaces:**
```ts
export const POLL_MS = 1000;
export const RESTORE_FINISHED_WITHIN_MS = 24 * 60 * 60 * 1000;
export const MAX_RESTORED_FINISHED = 5;
export function elapsedText(run: RunView, now: number): string;
export function stateText(run: RunView): string;      // "Queued" | "Running" | "Summarized" | "Failed: <error>" | "Canceled"
export function answerLine(answer: RunAnswer): string; // "Type: … · Category: … · Importance: high — <reason>"
export function resumeCommand(cwd: string, sessionId: string): string; // `cd '<cwd, ' as '\''>' && claude --resume <id>`
export function restorable(runs: RunView[], now: number, dismissed: ReadonlySet<string>): RunView[];
export interface RunCardDeps { host: Pick<HostClient, "call">; now(): number; copy(text: string): Promise<void>; visible(): boolean; storage: chrome.storage.StorageArea }
export class RunCards { constructor(container: HTMLElement, deps: RunCardDeps); show(run: RunView): void; restore(): Promise<void>; dispose(): void; }
```
- Streamed text goes into a `<pre>` through `textContent` only.
- Polling:
  - A card polls `get_run(run_id, next_offset)` every `POLL_MS` while it is not terminal and `visible()` holds.
  - It loops while `more`, and makes one final fetch after a terminal state.
- Buttons:
  - Cancel, while queued or running, sends `cancel_run`.
  - "Open in Codex" shows for `harness === "codex" && thread_id` and sends `open_path{codex_thread}`.
  - "Copy resume command" shows for `harness === "claude" && session_id && cwd`.
  - Copy shows on every failure and copies the stream text, or the error when there is no stream text.
  - Retry save shows when `can_retry_save`. It sends `retry_save`, renders the returned view and resumes polling.
  - Dismiss on a finished card stores the id in `storageKey("dismissed_runs_v1")`, capped at 200.
- On success the card shows `answerLine(run.answer)`. PR 4 (Task 52) adds the saved-summary line and Open summary.
- `service_unavailable` from `list_runs` shows setup instructions with "Open settings".

- [ ] Write the failing tests:
  - `elapsedText counts from start to finish`
  - `resume command quotes the knowledge base path`
  - `answer line shows type, category and importance`
  - `polling appends stream text as text and stops after success`
  - `polling pauses while the panel is hidden`
  - `cancel sends cancel_run and shows Canceled`
  - `codex run offers Open in Codex; claude run offers Copy resume command`
  - `a save failure offers Retry save and retrying resumes polling`
  - `other failures offer no Retry save but keep Copy`
  - `restore shows in-flight runs and at most 5 recent finished, skipping dismissed and older than a day`
  - `service_unavailable shows setup instructions`
- [ ] Run the extension command. Expect FAIL.
- [ ] Implement.
- [ ] Run `npm run typecheck && npm run lint` and the extension command. Expect PASS.
- [ ] Commit: `git add extension/run-card.ts extension/build.mjs extension/panel.ts extension/panel.css extension/tests/run-card.test.mjs package.json && git commit -m "feat(panel): run card with polling, cancel, retry save, Codex/Claude reopen, and restore"`

### Task 41: Headless smoke covers real runners against fake harnesses

**Files:**
- Modify `scripts/smoke-macos.mjs`:
  - `installFakeHarnesses` writes fake `codex` and `claude` with a `#!${process.execPath}` shebang:
    - `codex --version` prints `codex-cli 0.144.4`.
    - `codex app-server --listen stdio://` answers `initialize`, `thread/start` (`fake-thread-1`), `thread/name/set` and `turn/start`, then streams deltas, `item/completed` and `turn/completed`.
    - `claude --version` prints `2.1.310 (Claude Code)`.
    - `claude -p …` prints init (`fake-session-1`), deltas and `result`.
    - Both read `$HOME/fake-agent.mode` (`ok`, `slow`, `fail`, `badjson-once`, `badjson`) and log argv, stdin and env names to `harness-argv.log`.
  - The host wrapper drops `BRAUSER_FAKE_HARNESS` and adds `BRAUSER_OPEN_LOG=<runDir>/open.log`.
  - The fixture server gains `/article` (a nav block plus an article body).
  - Port PR 2's `fake-agent` steps to the fakes.

**Interfaces:**
- Consumes: `hostRequest`, `openPanel`, `step`, PR 2's `wrapper service` child, and the knowledge base scripted picker step from Task 10.

- [ ] Add the steps:
  1. **Harness setup in settings.** Detect shows "Codex 0.144.4" and "Claude Code 2.1.310". Test and use Codex, then check `hostConfig().agent.harness === "codex"`. `#harness-warning` has the exact §6.1 text.
  2. **Summarize a page with Codex.** Open `/article`, open the panel and click `#summarize`; the card reaches "Summarized" and the answer line matches `/Importance: high/`. Then check:
     - `page.md` has the article text and not the nav text.
     - `harness-argv.log` shows `app-server --listen stdio://`, `thread/start` with `approvalPolicy:"never"` and `sandbox:"danger-full-access"`, and cwd equal to the knowledge base.
     - No argv contains article text.
  3. **Open in Codex.** `open.log` gets `codex://threads/fake-thread-1`.
  4. **Claude Code and resume.** In `badjson-once` mode the run succeeds after exactly two `claude` invocations, and the second has `--resume fake-session-1`. The argv includes `--dangerously-skip-permissions`, and the prompt arrived on stdin. The Copy resume text equals `cd '<kb>' && claude --resume fake-session-1`.
  5. **Second invalid answer.** In `badjson` mode the card shows "Failed", mentions "not valid", and offers Copy.
  6. **Cancel.** In `slow` mode, Cancel gives "Canceled" and `process.kill(-process_group, 0)` throws `ESRCH`.
  7. **Restore.** Close and reopen the panel; the cards return from `list_runs`.
  8. **agent.env.** Settings shows the names from a 0600 `agent.env` and never the values.
- [ ] Run the smoke command. Expect FAIL before the steps are wired and PASS after.
- [ ] Commit: `git add scripts/smoke-macos.mjs && git commit -m "test(smoke): summarize with fake Codex and Claude Code through the real runners"`

### Task 42: Docs, backlog, and full gate

**Files:**
- Modify `DESIGN.md`. Read it in full first.
  - §5.2: the Summarize & log flow (extraction with byte caps, `start_run`, the service run, the answer validation). The summary file and log event are PR 4.
  - §7.1: detect and probe in the service. Runs are full-access Codex app-server or Claude `-p` in the knowledge base folder, with the service env plus agent.env. Remove the lockdown wording.
  - §8 permission table: `contextMenus` (now the summarize item), `scripting` (now used) and `commands.summarize`.
  - §12.2: the PR 3 step.
  - §14, dated 2026-10-26: the verified flags and the `outputSchema` finding.
- Modify `AGENTS.md`:
  - "Current state".
  - Security rules: remove the cleared-env and reviewed-argv rules. Add these: page text never goes on a command line; the prompt marks page content untrusted; streamed agent text is rendered as text only; tests never run a real harness, `/usr/bin/open` or launchctl.
  - Repository map: `answer.rs`, `open.rs`, `service/{codex,claude,prompt,setup,open,child}.rs`, `extract.ts`, `summarize.ts`, `run-card.ts`, `vendor/readability.js`.
- Modify `README.md`: "Status", setting up a summary harness, and Summarize & log.
- Modify `TESTING.md`:
  - The fake Codex and Claude behavior.
  - "Cannot prove": real harness logins, model output quality, the Codex Desktop deep link, and Readability on arbitrary sites.
  - A learning-log line about encoded-byte extraction caps.
- Modify `BOSUN_FEATURE_BACKLOG.md`: mark the PR 3 item done, with a **[human]** check: a real Codex and a real Claude Code summary from the dev build, and Open in Codex opening Codex Desktop.
- Modify `scripts/check_current_state.py`: add stale claims for "reviewed argv" and "cleared environment" where they apply.

- [ ] Edit `check_current_state.py` first, and expect FAIL.
- [ ] Update the documents.
- [ ] Run the full quality gate. Expect PASS.
- [ ] Commit: `git add DESIGN.md AGENTS.md README.md TESTING.md BOSUN_FEATURE_BACKLOG.md scripts/check_current_state.py && git commit -m "docs: record full-access summary runners, extraction, and the run card"`

---

## PR 4: Outputs: summary file, summary log event, artifact integration

PR 4 makes a succeeded run durable. The service writes the §7.1 summary file and appends a `summary` daily-log event under the configuration current at finish. That event creates a `web.page` artifact, or attaches to an adapter artifact. As a result, the summary appears in the artifact file's Timeline, in Recent, in the worklog and after `brauser reindex`. The panel shows a page's existing summary and opens it.

Assumes PRs 1-3 are merged. PR 4 uses PR 3's `crate::answer::{SummaryAnswer, Entities, Importance}` and the generated `brauser_protocol::ImportanceLevel`, and it defines no second answer or importance type. `RunStatusFile.requested_at` exists from Task 16. Opening files goes through `crate::open` (Task 36).

**Built on PR #59.** PR #59 (merge `fbdf7e2`, brought into this branch by `5273817`) already shipped the storage layer for summaries, and PR 4 reuses it rather than building a second one:
- `host/src/summary.rs`: `PageSummary`, `SummaryEvent`, `validate_page_summary`, `validate_summary_event` and `web_page_artifact_id`.
- `host/src/capture.rs`: `CaptureStore::record_summary`, the `summary` marker kind, `EntryGate::AnyHttpPage` (a summary is logged on any HTTP(S) page and while capture is paused), the `Summarized:` log line and `relative_log_link`.
- `host/src/index.rs`: `CachedEvent::Summary`, the `summary` arm of `fn parse_marker_event`, the Summary arm of `fn derive`, `ArtifactRow.summaries`, `VisitRow.summary`, `ArtifactIndex::record_summary`, `STATE_VERSION = 3`, the artifact file's `Summary:` line and Timeline lines through `fn relative_artifact_link`, the 20-line summary cap, and the worklog's `Summarized:` wording.

PR #59 has no production caller, so no daily log holds a #59-shape summary line. Its commit message lists four known gaps, and PR 4 closes each one in the task named:
1. **No harness field.** `SummaryEvent` has no `harness` and no record key (Task 46).
2. **A raw summary link.** Both link sites concatenate `../` with the raw `summary_path` instead of building an encoded relative link (Task 43).
3. **Timeline ordering.** Summary lines are printed above all activity rollups instead of interleaved by date (Task 47).
4. **Summaries on a page an adapter already recognizes.** `validate_summary_event` and `fn record_entry` force every summary onto the URL's `web.page` identity, so spec §7.2's "attaches to that adapter's artifact" cannot happen (Tasks 46 and 47).

Identity decision: the spec's `brauser.record_key` is written with the encoding that note.rs and DESIGN §6.6 already use. The `brauser:` block holds `kind: summary`, then `canonical_url`, then either `url_id` or `artifact_id`. Ownership checks read only that block.

---

### Task 43: Shared Markdown safety helpers, the remote-resource neutralizer, and encoded summary links

**Files:**
- Create: `host/src/markdown.rs`, `host/src/markdown_corpus.rs` (test-only), `host/tests/fixtures/markdown-neutralize-corpus.json`, `scripts/check-markdown-neutralizer.mjs`
- Modify:
  - `host/src/lib.rs` (`pub(crate) mod markdown;`, `#[cfg(test)] mod markdown_corpus;`)
  - `package.json` (exact-pinned `markdown-it`, `micromark`, `micromark-extension-gfm` and `marked` devDependencies; `test:markdown-neutralizer`) and `.github/workflows/ci.yml` (a step after `test:extension`)
  - `host/src/index.rs`: move `yaml_string` and `safe_markdown` into `markdown.rs`; `render_artifact` calls `markdown::inline(.., 2048)`; `fn relative_artifact_link` builds its link with `markdown::relative_link`.
  - `host/src/capture.rs`: `fn relative_log_link` is deleted, and `record_summary` calls `markdown::relative_link(&day_dir, ..)`.

**Interfaces:**
- `pub(crate) fn yaml_string(value: &str) -> String`
- `pub(crate) fn inline(value: &str, limit: usize) -> String`: today's `safe_markdown` with a limit.
- `pub(crate) fn neutralize_remote(text: &str) -> String`
- `pub(crate) enum LinkStyle { Relative, Wikilink }` and `pub(crate) fn link_style(profile: &str) -> LinkStyle`
- `pub(crate) fn relative_link(from_dir: &Path, to: &Path) -> String`: percent-encodes each segment except unreserved characters. No new crate.
- `pub(crate) fn wikilink(target: &str, label: Option<&str>) -> String`: strips `[ ] | # ^` and control characters.
- The neutralizer is a **context-free escape** (revised 2026-10-27; it replaced a scanner that turned each construct into a code span and had to mirror each viewer's parser, which three review rounds kept defeating). For every agent-sourced text written into a summary body: every `<` becomes `&lt;` (an entity never creates HTML structure in CommonMark or GFM, so no tag, autolink, comment, CDATA section, processing instruction or declaration can form in any context); every `!` directly before `[` becomes `\!` unless an odd run of backslashes already escapes it (no inline, reference, collapsed or shortcut image, and no `![[embed]]`); a `!` at the end of the input is escaped the same way and a trailing odd run of backslashes gets one more, so neutralized fragments can be joined with each other and with host text. Revised again 2026-10-27: every backtick, `$` and tab becomes a numeric entity (`&#96;`, `&#36;`, `&#9;`), a form feed or vertical tab becomes `&#32;`, every `~` in a run of three or more becomes `&#126;`, and every space from the fourth of a run on becomes `&#32;`, so no code span, fence, indented code block or math can form (Obsidian hands those to Mermaid, Dataview and math engines; a backslash escaping a backtick, `$` or `~` is dropped, since the entity already shows the plain character, and one before a whitespace entity is doubled so it stays visible instead of escaping the `&`); a trailing run of `~` or spaces is written as entities whatever its length, so joins cannot form a fence or indented code either. Numeric entities, not backslashes, because agent backslashes cannot undo them; every renderer decodes them to the plain characters outside code. Nothing else changes. Agent prose loses code formatting by design (Mermaid, Dataview and math run in Obsidian code spans, fences and `$`), lines indented four or more spaces lose their structure, and autolinks show as text, which is accepted. `markdown::inline` writes backticks, `$` and `~` as the same entities, and each space from the fourth of a run on as `&#32;`, so a field placed after a list marker cannot make indented code. No `regex` crate is added.
- `yaml_string` also writes `<` and an image `!` as JSON `\u` escapes, so the raw front matter holds neither while a YAML reader gets the same value. `wikilink` also strips `<`, `>`, backticks and `$`, so a viewer without wikilinks forms no tag, code span or math inside the brackets. The corpus check renders every input as `inline` and `wikilink` output too, with no `<code>` or `<pre>` allowed.
- The daily-log line and the artifact file's `Summary:` line keep #59's bracketed `[..](<link>)` form. Only the destination changes: it is now encoded, so a `summaries_dir` with a space gives `%20`. `validate_page_summary` keeps rejecting `<`, `>`, `#`, `?` and control characters.

- [ ] Write the failing tests:
  ```rust
  #[test] fn every_lt_becomes_an_entity_and_every_image_bang_is_escaped() {
      for (input, expected) in [
          ("see ![x](https://e.example/a.png) now", "see \\![x](https://e.example/a.png) now"),
          ("![x][r]\n\n[r]: https://e.example/b.png", "\\![x][r]\n\n[r]: https://e.example/b.png"),
          ("![x][]", "\\![x][]"), ("![x]", "\\![x]"), ("![[https://e.example/x.png]]", "\\![[https://e.example/x.png]]"),
          ("<IFRAME src=x></IFRAME>", "&lt;IFRAME src=x>&lt;/IFRAME>"),
          ("<!-- c --><img src=u>", "&lt;!-- c -->&lt;img src=u>"),
          ("```\n<img src=u>\n```", "&#96;&#96;&#96;\n&lt;img src=u>\n&#96;&#96;&#96;"),
          ("`$= dv.current()` $x$", "&#96;&#36;= dv.current()&#96; &#36;x&#36;"),
          ("~~~mermaid\nA@{ img: \"u\" }\n~~~", "&#126;&#126;&#126;mermaid\nA@{ img: \"u\" }\n&#126;&#126;&#126;"),
          ("    = this.file.name", "   &#32;= this.file.name"),
          ("\\\\![x](u)", "\\\\\\![x](u)"),
          // ... every input the earlier scanner's tests used (backticks, multi-line constructs,
          // indented and under-indented fences, table cells, reference definitions, CDATA, PIs).
      ] { assert_eq!(neutralize_remote(input), expected, "{input:?}"); }
  }
  #[test] fn every_tag_in_the_list_is_escaped_in_any_case() { /* img iframe video audio object embed link style svg picture source script image */ }
  #[test] fn ordinary_markdown_is_left_alone() {
      for text in ["[a](https://x.example)", "- [ ] open\n- [x] done", "| a | b |\n|---|---|\n| 1 | 2 |"] { assert_eq!(neutralize_remote(text), text); }
  }
  // host/src/markdown_corpus.rs: over host/tests/fixtures/markdown-neutralize-corpus.json, no output holds a
  // raw `<`, an unescaped `![`, a backtick, `$`, tab, run of three `~` or run of four spaces, and nothing
  // else changes; with BRAUSER_NEUTRALIZE_CORPUS_OUT set it writes the outputs for
  // scripts/check-markdown-neutralizer.mjs (`npm run test:markdown-neutralizer`, a CI step after
  // test:extension), which renders them with markdown-it, micromark+GFM and marked and fails on any
  // element outside an inert allowlist, and on any <code> or <pre> in neutralized body text. The corpus
  // includes Mermaid `@{ img: … }`, dataview/dataviewjs and math fences (backtick and tilde, indented 1-3
  // spaces, in lists and quotes), Dataview inline queries, `$x$` / `$$\href{…}$$` math, backslashes before
  // backticks and `$`, and double-backtick spans.
  #[test] fn relative_links_climb_and_percent_encode() {
      assert_eq!(relative_link(Path::new("summaries"), Path::new("pages/a b.md")), "../pages/a%20b.md");
      assert_eq!(relative_link(Path::new("log/2026/10"), Path::new("summaries/x.summary.md")), "../../../summaries/x.summary.md");
  }
  #[test] fn wikilinks_cannot_break_out() { assert_eq!(wikilink("a]]|b#^", None), "[[ab]]"); }
  ```
- [ ] Change the expectation of `a_summary_path_with_a_space_still_produces_a_bracket_delimited_link` in `capture.rs` to `[summary](<../../../My%20Summaries/hn-item.summary.md>)`. Add `the_artifact_summary_link_is_percent_encoded` in `index.rs`, modeled on `summary_event_appends_once_on_a_page_no_adapter_covers`, with a `summary_path` of `My Summaries/hn-item.summary.md`.
- [ ] Run `cargo test --locked -p brauser -- markdown:: summary_path_with_a_space artifact_summary_link`. Expect FAIL.
- [ ] Implement `neutralize_remote` as one pass over chars: `<` becomes `&lt;`; `!` followed by `[`, or at the end of the input, becomes `\!` when the run of backslashes before it is even; backticks, `$` and tabs become `&#96;`, `&#36;`, `&#9;`, a `~` run of three or more (or at the end) becomes `&#126;` each, spaces from the fourth of a run on (or a run at the end) become `&#32;`, and a backslash escaping a backtick, `$` or `~` that becomes an entity is dropped; a trailing odd run of backslashes gets one more; every other character is copied. The two end-of-input rules make outputs safe to join: neither a following fragment nor host text can re-form `![` or cancel a leading `\!`.

  Move `yaml_string` and `safe_markdown` (as `inline`) unchanged. Switch both #59 link sites to `relative_link`.
- [ ] Run `cargo test --locked -p brauser markdown:: && cargo test --locked -p brauser index:: && cargo test --locked -p brauser capture::`. Expect PASS, including #59's `summary_event_appends_once_on_a_page_no_adapter_covers` unchanged (its path has no reserved characters).
- [ ] Commit: `git add host/src/markdown.rs host/src/lib.rs host/src/index.rs host/src/capture.rs && git commit -m "feat(host): shared markdown escaping, remote-resource neutralizer, encoded summary links"`

### Task 44: Lift the owned-file writer out of index.rs

**Files:**
- Modify: `host/src/vault.rs` (add the writer and ownership parser after `fn move_note_no_clobber`); `host/src/index.rs` (`ArtifactWrite` becomes `vault::OwnedWrite`, `fn replace_artifact` delegates, `fn owned_artifact` calls `owned_frontmatter`)

**Interfaces:**
- `pub(crate) enum OwnedWrite { Written, Unchanged, Conflict }`
- `pub(crate) fn replace_owned_file(root: &Dir, relative: &Path, markdown: &str, kind: &str, owned: &dyn Fn(&[u8]) -> bool) -> Result<OwnedWrite>`:
  - It creates the parent chain, writes and fsyncs `.{NAMESPACE}-{kind}-{uuid}.tmp`, and publishes with a hard link when the name is absent.
  - It renames only over a regular, non-symlink file of at most 4 MiB that `owned` accepts. Anything else is `Conflict`, and `AlreadyExists` on the hard link is also `Conflict`.
- `pub(crate) fn owned_frontmatter(bytes: &[u8], kind: &str, ids: &[(&str, &str)], forbidden: &[&str]) -> bool`

- [ ] Write the failing tests: `owned_replace_creates_replaces_and_skips_unchanged`, `unowned_symlink_or_directory_is_a_conflict_and_untouched` and `ownership_needs_exactly_one_block_kind_and_id`.
- [ ] Run `cargo test --locked -p brauser vault::tests::owned`. Expect FAIL.
- [ ] Implement by moving the `replace_artifact` body into `replace_owned_file`, parameterized by `kind` and `owned`.
- [ ] Run `cargo test --locked -p brauser`. Expect PASS, including `an_unowned_file_at_the_artifact_path_is_never_adopted`.
- [ ] Commit: `git add host/src/vault.rs host/src/index.rs && git commit -m "refactor(host): shared ownership-checked replace for Brauser-owned files"`

### Task 45: Summary file rendering and the checked summary write

**Files:**
- Modify: `host/src/summary.rs`. It already exists from #59; add the file rendering and ownership check beside #59's event types, and rewrite its module comment, which says this module "never writes a summary file".
- Modify: `host/src/vault.rs` (`Vault` gains `summaries_dir: Option<PathBuf>`, set in `open_checked`; `sweep_stale_temporaries` also sweeps the summaries dir)

**Interfaces:**
```rust
use crate::answer::SummaryAnswer;
use brauser_protocol::{HarnessId, ImportanceLevel, RecordKey};
pub enum SessionLink { CodexThread(String), ClaudeSession(String), None }
pub struct SummaryDocument<'a> { pub key: &'a RecordKey, pub canonical_url: &'a str, pub title: &'a str,
    pub summarized_at: &'a str, pub harness: HarnessId, pub session: SessionLink, pub note_link: String, pub answer: &'a SummaryAnswer }
pub fn importance_str(level: ImportanceLevel) -> &'static str;   // "high" | "medium" | "low"
pub fn render_summary(doc: &SummaryDocument<'_>, style: LinkStyle) -> String;
pub fn owned_summary(bytes: &[u8], key: &RecordKey) -> bool;
pub fn summary_filename(key: &RecordKey, canonical_url: &str) -> Result<String>; // note stem + ".summary.md"
impl Vault {
    pub fn summary_relative_path(&self, key: &RecordKey, canonical_url: &str) -> Result<PathBuf>;
    pub fn note_link(&self, key: &RecordKey, canonical_url: &str, style: LinkStyle) -> Result<String>;
    pub fn write_summary(&self, key: &RecordKey, canonical_url: &str, markdown: &str) -> Result<OwnedWrite>;
}
```
- Rendering:
  - Frontmatter values go through `yaml_string`, and entity lists are flow sequences of JSON strings.
  - Single-line agent text (type, category, importance reason, entity names, related notes) goes through `inline(.., 200)`. It backslash-escapes `<`, `!`, `[`, `]` and `\` as well as Obsidian's `#` tags, `%%` comments, `==` highlights and `|`, so no tag, image, embed, Obsidian tag, comment or highlight can form, and its output never ends in an unescaped `!` or an odd run of backslashes. In the `Wikilink` style, entities and related notes use `wikilink` instead.
  - `description` and `assessment` go through `neutralize_remote`, each placed at the start of a line after a host-written newline (after `## Summary\n`), never after a list marker or other prefix ending in a space (DESIGN.md §5.2 step 6's joining contract; marked can make a code block otherwise).
  - Empty entity groups are omitted. The Entities line goes when all groups are empty, and Related goes when the list is empty.
- `summary_relative_path` always yields a path that #59's `validate_page_summary` accepts: relative, ending `.summary.md`, and free of `..`, `<`, `>`, `#`, `?` and control characters.

- [ ] Write the failing tests:
  - `summary_file_has_the_exact_layout`, with the exact expected string:
    ```
    ---\nbrauser:\n  kind: summary\n  canonical_url: "…"\n  url_id: "<id>"\n
    canonical_url / title / summarized_at / document_type / category / importance: high /
    entities (flow lists) / harness: codex / thread: "codex://threads/t-1"\n---\n
    [Page](<url>) · [Page note](../pages/<stem>.md)\n\n**Type:** … · **Category:** … · **Importance:** high — <reason>\n\n
    **Entities:** …\n\n## Summary\n…\n\n## Assessment\n…\n\nRelated: projects/widget.md\n
    ```
  - `claude_runs_record_the_session_not_a_thread`
  - `obsidian_style_uses_wikilinks`
  - `agent_text_cannot_forge_identity_or_fetch_remote_resources`
  - `neutralized_text_starts_at_column_zero`: in the rendered file, every line holding `description` or `assessment` text starts with that text, never after host text ending in a space.
  - `summary_name_never_equals_the_note_name`
  - `summary_paths_pass_the_summary_event_path_check`: for a URL key, an artifact key and a page title with `#`, `?` and `<`, `validate_page_summary` accepts the `summary_relative_path`.
  - `write_summary_never_adopts_a_note_or_foreign_summary`
  - `stale_summary_temporaries_are_swept`
- [ ] Run `cargo test --locked -p brauser summary::`. Expect FAIL.
- [ ] Implement. `write_summary` calls `replace_owned_file(root, path, markdown, "summary", &|b| owned_summary(b, key))`.
- [ ] Run `cargo test --locked -p brauser summary:: && cargo test --locked -p brauser note::`. Expect PASS, including `a_summary_file_at_the_same_name_is_a_conflict_not_a_note`.
- [ ] Commit: `git add host/src/summary.rs host/src/vault.rs && git commit -m "feat(host): render and write Brauser-owned summary files"`

### Task 46: Extend #59's summary event: record key, harness, and adapter artifacts

**Files:**
- Modify: `host/src/summary.rs` (`PageSummary`, `SummaryEvent`, `validate_page_summary`, `validate_summary_event`)
- Modify: `host/src/capture.rs` (`record_summary`; the `kind == "summary"` branch of `fn record_entry`; the `page_summary` test helper; every `store.record_summary(..)` call in its tests)
- Modify: `host/src/index.rs` (the `SummaryEvent` literals in its tests; the `page_summary` test helper; every `capture.record_summary(..)` call in its tests)

**Interfaces:**
```rust
pub struct PageSummary {             // #59's fields, plus:
    pub record_key: RecordKey,       // resolved by the caller under lock_current (Task 49)
    pub artifact_type: String,       // "web.page" for a Url key; the adapter's type for an Artifact key
    pub harness: HarnessId,
    pub importance: ImportanceLevel, // was String
    /* event_id, url, title, occurred_at, document_type, category, summary_path unchanged */
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)] #[serde(deny_unknown_fields)]
pub struct SummaryEvent {            // #59's fields, plus record_key and harness; importance: ImportanceLevel
    pub event_id: String, pub artifact_id: String, pub artifact_type: String, pub record_key: RecordKey,
    pub url: String, pub title: String, pub occurred_at: String,
    pub document_type: String, pub category: String, pub importance: ImportanceLevel,
    pub summary_path: String, pub harness: HarnessId,
}
impl CaptureStore {
    pub fn record_summary(&self, policy: CapturePolicy<'_>, summary: &PageSummary) -> (CaptureOutcome, Option<SummaryEvent>);
}
```
- The new fields are required, not `#[serde(default)]`: #59 has no caller, so no log holds a #59-shape line. `ImportanceLevel` serializes as #59's `"high" | "medium" | "low"` strings, so `validate_page_summary` drops its string match.
- Identity, in the `kind == "summary"` branch of `fn record_entry`, replacing the unconditional `web_page_artifact_id`:
  - A `Url` key: `artifact_id = web_page_artifact_id(normalized)`, `artifact_type` must be `web.page`, and `artifact_id` must equal `web:{host}/{record_key.value}`. A mismatch means the caller's key and the host's normalization disagree, and is `Rejected { reason: "invalid_summary_key" }`.
  - An `Artifact` key: `artifact_id = record_key.value`. It must not start with `web:`, `artifact_type` must be 1-64 bytes and not `web.page`, and the reason on failure is the same.
- `validate_summary_event` (log replay) checks the same invariants. It still accepts a `web.page` event only for the `web_page_artifact_id` of its own `url`.
- `record_summary` returns the `SummaryEvent` exactly as written in the marker payload, so the caller indexes it with `ArtifactIndex::record_summary` without deriving it a second time. The event is `Some` only for `Persisted`.
- Unchanged from #59: the `EntryGate::AnyHttpPage` gate, logging while capture is paused, the `Summarized:` line, the 1-64 type and category bounds, the `summary_path` checks and UUID canonicalization.

- [ ] Write the failing tests:
  - `a_summary_on_an_adapter_page_attaches_to_that_artifact` (an `Artifact` key with `artifact_type: "jira.issue"`; the marker payload has that ID and type, not a `web:` one)
  - `a_url_key_that_disagrees_with_the_normalized_url_is_rejected`
  - `an_artifact_key_cannot_claim_web_page_or_a_web_id`
  - `the_event_records_the_harness_and_record_key`
  - `record_summary_returns_the_event_it_wrote` (equal to the event `parse_marker_event` replays from the file)
  - `a_replayed_adapter_summary_with_a_web_page_type_is_ignored`
- [ ] Run `cargo test --locked -p brauser -- summary:: capture::tests::`. Expect FAIL.
- [ ] Implement, and update #59's `PageSummary`/`SummaryEvent` test literals to the new fields.
- [ ] Update #59's `record_summary` call sites to the tuple return. They match on a bare `CaptureOutcome` and stop compiling otherwise:
  - In `capture.rs` tests, destructure `(outcome, event)` and match on `outcome` in `a_summary_persists_on_a_page_no_site_rule_covers`, `a_summary_persists_while_capture_is_paused`, `a_malformed_summary_is_rejected`, `a_credential_url_is_rejected_for_a_summary`, `a_noncanonical_uuid_summary_hashes_and_serializes_the_same_spelling` and `a_summary_path_with_a_space_still_produces_a_bracket_delimited_link`. The two `Rejected` tests also assert `event.is_none()`.
  - `a_malformed_summary_is_rejected` in `capture.rs` and `an_unknown_importance_is_rejected` in `summary.rs` set `importance = "urgent"`, which no longer compiles once `importance` is an `ImportanceLevel`. Make the capture test set `category` to an empty string and expect `invalid_summary_category`. Rewrite the summary test to assert that a `SummaryEvent` JSON with `"importance":"urgent"` fails to deserialize, so a forged log line with that value is still skipped on replay.
  - In `index.rs` tests, `summary_event_appends_once_on_a_page_no_adapter_covers` matches `capture.record_summary(policy, &summary).0` and `capture.record_summary(policy, &again).0` against `CaptureOutcome::Persisted { .. }`.
- [ ] Run `cargo test --locked -p brauser -- summary:: capture:: index::`. Expect PASS, including all of #59's summary tests.
- [ ] Commit: `git add host/src/summary.rs host/src/capture.rs host/src/index.rs && git commit -m "feat(host): summary events carry the record key and harness and attach to adapter artifacts"`

### Task 47: Index: adapter-attached summaries, a chronological Timeline, and a version 4 cache

**Files:**
- Modify `host/src/index.rs`: the Summary arm of `fn derive`, `fn render_artifact`, `STATE_VERSION`, and `fn artifact_candidates_for_url` (tests only).

**Interfaces:**
- `STATE_VERSION = 4`, so a version 3 cache, whose cached `SummaryEvent`s lack `record_key` and `harness`, is rescanned instead of failing to load.
- Derive rules, on top of #59's Summary arm:
  - When a summary creates an adapter artifact's row, it is typed by the event's `artifact_type`. When an activity event for that artifact arrives later, `site`, `title` and `url` follow the usual latest-event rule.
  - A summary on an adapter artifact pushes the same zero-measure `VisitRow` as on a `web.page`, so it never changes measured focus or opens.
  - `artifact_candidates_for_url` already excludes `web:` IDs and reads `CachedEvent::urls`, so an adapter summary's URL becomes a note-key candidate with no code change. Pin it with a test.
- Timeline rules, replacing #59's "all summaries, then all rollups":
  - Summary lines and `timeline_rollups` rows are merged newest first. A rollup sorts by its `last_date`, and a summary sorts before a rollup with the same date.
  - A summary line is `- YYYY-MM-DD HH:MM · Summarized: <title> — <type>, <importance> importance`, with the time in the event's own offset, so the file has no seconds to churn. It keeps #59's title, so `summary_event_appends_once_on_a_page_no_adapter_covers` still finds `Summarized: A good discussion`, and it matches the worklog's wording. The title and type go through `markdown::inline`.
  - #59's cap stays: the 20 newest summary lines are kept, and older ones become one `- N earlier summaries` line at the end.

- [ ] Write the failing tests:
  - `timeline_interleaves_summaries_and_rollups_by_date` (a summary on day 3 between rollups on days 1 and 5 is printed between them, and a summary on day 5 comes before day 5's rollup)
  - `a_timeline_summary_line_has_the_title_and_no_seconds` (an event at `2026-10-20T09:15:42-07:00` titled "A good discussion" renders exactly `- 2026-10-20 09:15 · Summarized: A good discussion — discussion, high importance`)
  - `a_summary_on_an_adapter_artifact_without_activity_creates_that_row` (it is typed `jira.issue`, and its later activity keeps one row)
  - `an_adapter_summary_is_a_note_key_candidate` and `web_summaries_never_redirect_note_keys`
  - `a_version_3_state_cache_is_rescanned` (modeled on #59's `a_version_1_state_cache_is_rescanned`)
  - `reusing_a_summary_event_id_with_other_content_bails`
- [ ] Run `cargo test --locked -p brauser index::tests::`. Expect FAIL.
- [ ] Implement.
- [ ] Run `cargo test --locked -p brauser index::`. Expect PASS, including #59's `a_summary_joins_the_m1_legacy_visit_of_the_same_url`, `forged_summary_markers_are_ignored` and `summary_event_appends_once_on_a_page_no_adapter_covers` with its `Summarized: A good discussion` assertion unchanged.
- [ ] Commit: `git add host/src/index.rs && git commit -m "feat(index): adapter-attached summaries and a chronological Timeline"`

### Task 48: Recent, worklog, and a page's summary on the wire

**Files:**
- Modify `protocol/schema.json`, then regenerate: add `$defs.SummaryView` using the existing `ImportanceLevel`, with no new importance def; add a required-nullable `summary` to `ArtifactListItem` and `ArtifactResult`.
- Modify: `host/src/index.rs` (`fn recent`, `fn worklog`, new `summary_for`), `host/src/native.rs` (`fn get_artifact`, `fn list_artifacts`), `extension/protocol-shape.ts`
- Test: `host/src/index.rs`, `host/src/native.rs`, `protocol/tests/wire.rs`, `extension/tests/agent.test.mjs`

**Interfaces:**
- Wire `SummaryView { summarized_at: string (≤32), document_type: string (1-64), category: string (1-64), importance: ImportanceLevel, summary_path: string (≤1024), harness: HarnessId }`.
- `ArtifactListItem.summary` and `ArtifactResult.summary` are `SummaryView | null`, from the artifact's latest summary. On an unrecognized page, `get_artifact` looks up `web_page_artifact_id(canonical)` and still returns `recognized: false, artifact: null`. On a recognized page it reads the adapter artifact's summary.
- `pub fn summary_for(&self, artifact_id: &str) -> Result<Option<SummaryView>>`; TS `isSummaryView`.
- `fn recent` already lists #59's summary-only rows. In addition, a row whose local day has a summary carries that summary.
- Worklog changes to #59's wording:
  - Today #59 counts a summary-only artifact as opened. Summary visits are now excluded from the measured counts, so `opened` counts only `opens > 0`.
  - Each summary in range keeps #59's `Summarized: <plain_text(title)> — <type>, <importance> importance` line, including the measures #59 appends when the artifact also has activity.
  - When K > 0, the header becomes `Opened {o} artifact{s}, focused on {c} left open from an earlier day, and summarized {k} page{s}.`

- [ ] Write the failing tests: `recent_lists_a_summary_only_page_with_its_summary` (`ImportanceLevel::High`), `worklog_names_summaries_without_counting_them_as_opened` (update #59's `worklog_names_a_summary_only_artifact_instead_of_just_seen` to the new header), `get_artifact_reports_the_summary_of_an_unrecognized_page`, `get_artifact_reports_an_adapter_artifacts_summary`, wire `artifact_list_item_requires_a_nullable_summary` and JS `artifact_result requires summary`.
- [ ] Run `cargo test --locked -p brauser -- summary_only_page worklog_names_summaries get_artifact_reports` and the extension command. Expect FAIL.
- [ ] Implement and regenerate.
- [ ] Run `cargo test --locked --workspace && npm run typecheck` and the extension command. Expect PASS.
- [ ] Commit: `git add protocol/schema.json protocol/src/generated.rs protocol/ts/generated.ts protocol/tests/wire.rs host/src/index.rs host/src/native.rs extension/protocol-shape.ts extension/tests/agent.test.mjs && git commit -m "feat: summaries in Recent, the worklog, and get_artifact"`

### Task 49: `SummaryFinisher`: write under the configuration current at finish

**Files:**
- Create: `host/src/service/finisher.rs`
- Modify: `host/src/service/mod.rs` (`service::run` uses `SummaryFinisher::new(ConfigStore::default_path()?)`; delete `VaultCheckFinisher` from `supervisor.rs`); `host/src/config.rs` (`fn load_from` becomes `pub(crate)`)

**Interfaces:**
- Consumes: `RunFinisher`, `RunStatusFile` (`start_key`, `url`, `title`, `harness`, `thread_id`, `session_id`, `requested_at`, `run_id`), `SavedRun`, `native::resolve_record_key` (Task 22; the caller holds `lock_current`), `answer::parse_answer` (Task 33), and #59's `CaptureStore::record_summary` and `ArtifactIndex::record_summary` as extended in Task 46.
```rust
pub struct SummaryFinisher { config_path: PathBuf, now: fn() -> OffsetDateTime }
impl SummaryFinisher { pub fn new(config_path: PathBuf) -> Self; }
impl RunFinisher for SummaryFinisher { fn accept(..); fn save(&self, run: &RunStatusFile, result: &serde_json::Value) -> Result<SavedRun, String>; }
pub(crate) fn summarized_at(now: OffsetDateTime, requested_at: &str) -> String; // now in requested_at's offset, whole seconds; unparseable → UTC "Z"
pub(crate) fn summary_artifact(config: &ConfigStore, url: &str, key: &RecordKey, index: Option<&ArtifactIndex>) -> (RecordKey, String);
```
- `summary_artifact` picks the key and `artifact_type` the log event uses:
  - A `Url` key gives `(key, "web.page")`.
  - An `Artifact` key takes its type from `config.adapter_match(url, None)`. If no adapter matches (a key recovered from an older note), it takes the type of the indexed row.
  - If neither knows the type, the event falls back to the URL key and `web.page`. The summary file still uses the resolved key.
- The `save` algorithm holds one lock from step 2 to the end, because the index has no lock of its own (`brwr-7vf`):
  1. Deserialize `SummaryAnswer`. On failure: "The stored answer is not a valid summary."
  2. `load_from`, then `lock_current`, then `refresh`. A config issue gives "The configuration needs repair. Fix it in settings, then Retry save."
  3. No storage gives "Choose a notes folder in settings, then Retry save." No `summaries_dir` gives "No summaries folder is set. Set one in settings, then Retry save."
  4. `Vault::open_checked(storage, root_identity)` fails (missing, unmounted or replaced): "The notes folder is unavailable or was replaced. Choose it again in settings, then Retry save."
  5. `key = resolve_record_key(url)`, which may differ from `start_key`; `canonical = canonical_url(url, strip_params)`.
  6. Render with `link_style(storage.profile)`, then `write_summary`. `Conflict` gives "A file Brauser does not own is at <path>. Move it, then Retry save."
  7. Build a `PageSummary` with `event_id = run_id`, `occurred_at = summarized_at(..)`, the `summary_artifact` key and type, `harness = run.harness` and the written `summary_path`, then call `CaptureStore::record_summary`:
     - Persisted: call `index.record_summary` with the returned event.
     - `duplicate_event`: call `reconcile_from_logs`.
     - Rejected or Retryable: "The daily log could not be written (<reason>). Retry save."
  8. Return `SavedRun { written_key: key, summary_path: Some(path) }`.
- Retry is idempotent: the same `run_id` dedupes the log line, and an identical file is `Unchanged`.
- Tests build times with `OffsetDateTime::parse(.., &Rfc3339)`, so `time`'s `macros` feature is not needed and `Cargo.toml` is unchanged.

- [ ] Write the failing tests:
  - `save_writes_the_summary_file_the_log_event_and_an_index_row`
  - `save_never_touches_the_page_note`: write a page note first; after save, its bytes and mtime are unchanged.
  - `an_adapter_added_mid_run_is_followed` (the log event attaches to the adapter's artifact, not a `web.page`)
  - `a_recovered_artifact_key_without_a_known_type_logs_under_the_web_page`
  - `a_summaries_dir_changed_mid_run_receives_the_file`
  - `a_replaced_notes_folder_fails_with_the_retry_save_message`
  - `an_unmounted_then_restored_notes_folder_fails_then_retry_save_succeeds` (Review Focus 4): rename the root away, save fails with "Choose it again"; rename it back, and a second `save` succeeds with one `kind=summary` line.
  - `an_unowned_file_at_the_summary_name_fails_and_is_untouched`
  - `retry_after_the_fix_writes_once_and_is_idempotent`
  - `summarized_at_uses_the_click_offset`: `"2026-10-26T14:05:00-07:00"` for `now` = 21:05 UTC with `requested_at` = `"2026-10-26T14:00:00-07:00"`, and `…Z` for an unparseable value.
  - Extend `save_failure_offers_retry_save_and_retry_succeeds` in `supervisor.rs` to run against `SummaryFinisher`, with an unowned file at the summary name as the first failure.
- [ ] Run `cargo test --locked -p brauser service::finisher`. Expect FAIL.
- [ ] Implement.
- [ ] Run `cargo test --locked -p brauser service:: && cargo clippy --locked --workspace --all-targets -- -D warnings && cargo deny check --all-features --locked`. Expect PASS.
- [ ] Commit: `git add host/src/service host/src/config.rs && git commit -m "feat(service): write the summary and log event under the config current at finish"`

### Task 50: `open_summary`: open a page's owned summary file

**Files:**
- Modify: `protocol/schema.json` (add `OpenSummaryRequest {type:"open_summary", …, url}` and `SummaryOpened {type:"summary_opened", …, relative_path}`, then regenerate)
- Modify: `host/src/native.rs` (a new arm in `fn dispatch`; the per-connection context gains `open: crate::open::Opener`, defaulting to `open::default_opener()`). `OPEN_LOG_ENV` and the scripted seam are reused from Task 36 and not redefined.
- Modify: `extension/settings.ts` (`openSummary`), `extension/protocol-shape.ts`

**Interfaces:**
- The arm runs under `lock_current`: `resolve_record_key`, `Vault::open_checked`, `summary_relative_path`, read, `owned_summary`. Then:
  - An absent file gives `invalid_request` "This page has no summary yet."
  - An unowned file gives `conflict` "The file at the summary path is not a Brauser summary."
  - Otherwise it calls `open(&["--".into(), root.join(relative).into()])`.

  This works on any platform where the opener does. On non-macOS release builds it returns `Unsupported` as `invalid_request` "Opening files is only supported on macOS."
- TS `HostSession.openSummary(url: string): Promise<void>`.

- [ ] Write the failing tests: `open_summary_opens_only_an_owned_summary` (a recording opener), `open_summary_without_a_summary_is_invalid_request` and wire `open_summary_requires_url`.
- [ ] Run `cargo test --locked -p brauser open_summary`. Expect FAIL.
- [ ] Implement and regenerate.
- [ ] Run `cargo test --locked --workspace`, the scripted-dialogs test, `npm run typecheck` and the non-macOS check. Expect PASS.
- [ ] Commit: `git add protocol host/src/native.rs extension/settings.ts extension/protocol-shape.ts && git commit -m "feat: open a page's owned summary file from the panel"`

### Task 51: Panel: the page's existing summary and summarized Recent rows

**Files:**
- Create: `extension/summary-text.ts`
- Modify:
  - `extension/build.mjs` (`'summary-text.js'`)
  - `extension/panel.html`: `#page-summary` with `#page-summary-text` and `#page-summary-open` ("Open summary")
  - `extension/panel.css`
  - `extension/panel.ts`:
    - `refreshArtifact` shows `response.summary` whether or not the page is recognized.
    - `renderArtifact`: a summary-only artifact reads "No activity measured yet".
    - `recentRow`: a summarized row shows `Summarized · <type>, <importance> importance`.

**Interfaces:**
- `export function summaryLine(s: SummaryView): string`, which returns `` `${s.document_type} · ${s.category} · ${s.importance} importance` ``; and `export function recentSummaryText(s: SummaryView): string`.
- `#page-summary-open` sends `openSummary(currentUrl)`; a Recent row sends `openSummary(item.url)`. All text is set through `textContent`.

- [ ] Read `DESIGN.md` §5.7 and the panel tokens.
- [ ] Write the failing tests: `an unrecognized page with a summary shows its line and opens it`, `a recognized page shows its adapter artifact's summary`, `a page without a summary hides the line` and `a summarized Recent row says so and renders agent text as text`.
- [ ] Run the extension command. Expect FAIL.
- [ ] Implement.
- [ ] Run `npm run typecheck && npm run lint` and the extension command. Expect PASS.
- [ ] Commit: `git add extension/summary-text.ts extension/build.mjs extension/panel.html extension/panel.css extension/panel.ts extension/tests/panel.test.mjs && git commit -m "feat(panel): show and open a page's existing summary; summarized Recent rows"`

### Task 52: Run card: saved-summary line and Open summary

**Files:**
- Modify: `extension/run-card.ts` only, plus `extension/tests/run-card.test.mjs`. Retry save already exists from Task 40.

**Interfaces:**
- Consumes: `RunView.url`, `RunView.state`, `RunView.summary_path`, `get_artifact`, `summaryLine` (Task 51) and `openSummary` (Task 50).
- When a card reaches `succeeded` with a `summary_path`, it calls `get_artifact {url: run.url}` once. It shows `summaryLine(result.summary)` and an "Open summary" button that sends `openSummary(run.url)`.

- [ ] Write the failing tests: `a succeeded card shows the saved summary line and opens it` and `a succeeded card without summary_path makes no get_artifact call`.
- [ ] Run the extension command. Expect FAIL.
- [ ] Implement.
- [ ] Run `npm run typecheck && npm run lint` and the extension command. Expect PASS.
- [ ] Commit: `git add extension/run-card.ts extension/tests/run-card.test.mjs && git commit -m "feat(panel): run card shows the saved summary and opens it"`

### Task 53: Headless smoke covers outputs end to end

**Files:**
- Modify `scripts/smoke-macos.mjs`:
  - The fake harnesses gain a `summary` mode. It emits a §6.4 answer with `document_type: "specification"`, `category: "technical specification"` and `importance: {level: "high", reason: "Fixture reason"}`, an entity `<img src=https://evil.example/x.png>`, and a description containing `![t](https://evil.example/t.png)`, `<img src=https://evil.example/d.png>`, a Mermaid fence (```` ```mermaid ```` with `A@{ img: "https://evil.example/m.png" }`) and a Dataview inline query (`` `$= dv.el("img", "", {attr: {src: "https://evil.example/q.png"}})` ``).
  - `BRAUSER_OPEN_LOG` is already passed (Task 41).

- [ ] Add the steps:
  1. Write a page note on an unrecognized fixture page, then summarize it. Expect `succeeded`, with the page note's bytes unchanged.
  2. The summary file starts with `---\nbrauser:\n  kind: summary\n`. From the description (`neutralize_remote`) it contains `\![t](https://evil.example/t.png)` and `&lt;img src=https://evil.example/d.png>`. The entity is written by `inline` as `\<img src=https://evil.example/x.png\>` in the neutral profile, or by `wikilink` as `[[img src=https://evil.example/x.png]]` in the obsidian profile. From the Mermaid fence and Dataview query it contains `&#96;&#96;&#96;mermaid` and `&#96;&#36;= dv.el(`, and the summary body holds no raw backtick fence (no line starting with three backticks) and no raw `` `$= ``; backticks and `$` are written as entities because Obsidian runs Mermaid, Dataview and math from code spans, fences and `$`, so agent prose loses code formatting by design. No raw `<img` appears anywhere (front matter values write `<` as a JSON `\u` escape).
  3. Today's daily log has exactly one ` kind=summary ` line, and its payload has `harness: "codex"` and a `url` record key.
  4. `list_artifacts` returns a `web.page` item with `summary.importance === "high"`.
  5. `worklog` contains `Summarized: <fixture title> — specification, high importance`.
  6. After `refresh_artifacts`, the artifact file links to the summary, and its Timeline has the summary line before the day's rollup.
  7. Summarize a fixture page that an enabled adapter recognizes. `list_artifacts` shows the summary on that adapter's artifact, and no `web.page` item exists for the URL.
  8. Overwrite the summary with unowned text and summarize again. Expect `failed`/`save_failed` with `can_retry_save`, and the file untouched. Delete the file and send `retry_save`: expect `succeeded` and one more `kind=summary` line. A second identical `retry_save` adds none.
  9. Run `wrapper reindex`. `list_artifacts` still shows both summaries.
  10. In the panel, the fixture page shows `#page-summary`. Open summary writes the path to `open.log`.
- [ ] Run the smoke command. Expect FAIL before the steps are wired and PASS after.
- [ ] Commit: `git add scripts/smoke-macos.mjs && git commit -m "test(smoke): summary file, log event, artifacts, worklog, retry save, and reindex"`

### Task 54: Docs, backlog, and the full gate

**Files:**
- Modify `DESIGN.md`:
  - §5.2: steps 5-6 as built. A summary now appends a `summary` log event and creates or attaches an artifact, which reverses "never creates a log entry". Replace #59's interim parenthetical in step 5 ("Storage-layer support for this lands ahead of the full Summarize & log feature…") with the shipped behavior. Record the neutralizer's escape rule (done in Task 43's 2026-10-27 revision), and the date and result of the manual Obsidian (`\![[embed]]`) and VS Code preview check once it is performed.
  - §6.2: the `Summary` fields.
  - §6.6: the §7.1 layout and the two-key identity encoding.
  - §6.7: the Summary line and the date-ordered Timeline summary lines.
  - §5.7: the panel's page-summary line and the run card's saved summary.
  - §12.2: mark the PR 4 step done.
  - §14, dated 2026-10-26: summarized pages become `web.page` artifacts, and a page an adapter recognizes attaches to that artifact; config applies at write time and a changed key is followed; summaries log while capture is paused; `requested_at` carries the user's offset; `open_summary` uses the default app.
- Modify `AGENTS.md`:
  - "Current state".
  - Security rules: only the service writes summaries, from its own run; agent text is YAML-escaped and neutralized; summary files are never adopted; `open_summary` opens only owned files.
- Modify `README.md`: "Status".
- Modify `TESTING.md`:
  - A **[human]** check that every neutralized construct stays inert in Obsidian and VS Code preview.
  - The `BRAUSER_OPEN_LOG` seam.
  - A learning-log line about `requested_at`.
- Modify `BOSUN_FEATURE_BACKLOG.md`: mark the PR 4 item done; close `brwr-deo` as superseded by PRs 3-4.
- Modify `protocol/README.md`: `SummaryView`, `open_summary`/`summary_opened`, and the `summary` log marker kind with its `record_key` and `harness` fields.
- Modify `scripts/check_current_state.py`, only if required phrases change.

- [ ] Update each section.
- [ ] Run the full quality gate. Expect PASS.
- [ ] Commit: `git add DESIGN.md AGENTS.md README.md TESTING.md BOSUN_FEATURE_BACKLOG.md protocol/README.md scripts/check_current_state.py && git commit -m "docs: record summary files, the summary log event, and summarized-page artifacts"`

---

## Critic notes

- **Duplicate critiques were merged.** Two critiques covered `open_path{summary}` vs `open_summary`, two the duplicate answer and importance types, two the macOS cfg on the native context, two the `requested_at` sender, and two the frame-cap extraction bound. Each fix was applied once.
- **`requested_at` moved into Task 12** instead of only retargeting the old Task 66. That removed a whole task, which is why PR 4 has 12 tasks.
- **Recover PID-reuse guard (Task 20) deviates from the critique.** It uses a 60 s heartbeat-freshness window plus a dead or foreign `service_pid` instead of `sysctl kern.boottime`. Boot time would need FFI or a subprocess. A fresh heartbeat already rules out the reboot case the critique targets.
- **uuid `serde` feature (Task 16) not added.** `run_id` is stored as a validated `String`. Newer uuid releases may pull a crate (`serde_core`) that is not in `Cargo.lock`, which would break `--locked` offline.
- **`time` `macros` feature (Task 49) not added.** Tests use `OffsetDateTime::parse`, so `Cargo.toml` is unchanged.
- **Linux `cargo check` is conditional.** The critique proposed `cargo check --target x86_64-unknown-linux-gnu`, but the sandbox may not have that target's std installed and cannot fetch it. When no non-macOS target is installed, it is recorded as a CI/**[human]** check.
- **`scripting` stays in Task 6**, because spec §4 lists it as required. Task 11 records in DESIGN §8 that it is granted ahead of use. `contextMenus` is kept throughout rather than removed and re-added.
- **PR 4 re-planned on PR #59 (2026-10-27).** #59 merged the summary storage layer (`summary.rs`, `CaptureStore::record_summary`, `CachedEvent::Summary`, artifact-file Summary and Timeline lines, worklog wording) to main first. Tasks 45-48 now extend that code instead of creating it, and they close its four known gaps: Task 46 adds the harness and record key and attaches adapter-page summaries, Task 43 encodes the summary links, and Task 47 orders the Timeline by date. The task numbers and count are unchanged, so cross-references from PR 3 and the Review Focus still hold.
- **`--dangerously-skip-permissions` is plan content, not an instruction.** The harness flagged it as instruction-shaped. It is the Claude Code flag that spec §6.3 itself specifies, and it appears only as the runner's planned argv, so it was kept.
