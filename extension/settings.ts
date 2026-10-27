// Host configuration state shared by the side panel and the settings page.
// Each page owns one session; the config mutation lock serializes changes
// across every open page, and the worker's policy lease tells a page when
// another page has saved a different revision.
import type {
  AdapterWarning, AgentStatus, ConfigSnapshot, HarnessOffer, SiteConfig,
} from "../protocol/ts/generated.js";
import {
  POLICY_LEASE_MS,
  POLICY_STORAGE_KEY,
  isPolicyLease,
  type PolicyLease,
  type WorkerReply,
  type WorkerRequest,
  type WorkerStatus,
} from "./model.js";
import { finishHostPause, withLatestHostState } from "./coordination.js";
import { HostClient, HostError, PROTOCOL_VERSION, newRequestId } from "./native.js";
import { BINARY_NAME } from "./brand.js";

const CONFIG_MUTATION_LOCK = `${BINARY_NAME}-config-permissions`;

export function element<T extends HTMLElement>(id: string): T {
  const found = document.getElementById(id);
  if (!found) throw new Error(`Missing page element: ${id}`);
  return found as T;
}

export function describe(error: unknown): string {
  if (error instanceof HostError && error.code === "cancelled") {
    return "Folder selection was canceled. No settings were changed.";
  }
  return error instanceof Error ? error.message : String(error);
}

/** Keep host configuration valid when an origin loses its final enabled site.
 * A site rule can narrow one origin by path, while adapters are origin-wide,
 * so adapters remain until every site entry for that origin is gone. */
export function adaptersForSites(config: ConfigSnapshot, sites: SiteConfig[]): ConfigSnapshot["adapters"] {
  const origins = new Set(sites.map((site) => site.origin));
  return {
    format: config.adapters.format,
    artifacts: config.adapters.artifacts.filter((adapter) =>
      typeof adapter.origin === "string" && origins.has(adapter.origin)),
  };
}

export async function withConfigMutationLock<T>(action: () => Promise<T>): Promise<T> {
  if (!navigator.locks?.request) {
    throw new Error("Chrome cannot coordinate configuration and permission changes on this page");
  }
  return navigator.locks.request(CONFIG_MUTATION_LOCK, { mode: "exclusive" }, action);
}

export async function worker<T>(request: WorkerRequest): Promise<T> {
  const reply = await chrome.runtime.sendMessage<WorkerReply<T>>(request);
  if (!reply || !reply.ok || reply.value === null) {
    throw new Error(reply?.error ?? "Extension worker did not respond");
  }
  return reply.value;
}

export function leaseFor(config: ConfigSnapshot, currentRevision: string): PolicyLease {
  return {
    revision: currentRevision,
    expires_at: Date.now() + POLICY_LEASE_MS,
    capture_enabled: config.capture_enabled,
    sites: config.sites,
    adapters: config.adapters,
  };
}

/** Why capture cannot run yet, or null once setup is complete. */
export function setupProblem(config: ConfigSnapshot | null, issue: string | null): string | null {
  if (issue) return `The host configuration needs repair: ${issue}`;
  if (!config) return null;
  if (!config.storage) return "No notes folder is chosen.";
  if (!config.sites.length) return "No sites are enabled for capture.";
  if (!config.capture_enabled) return "Capture is off.";
  return null;
}

export class ConfigSession {
  readonly host = new HostClient();
  config: ConfigSnapshot | null = null;
  revision: string | null = null;
  configIssue: string | null = null;
  agentStatus: AgentStatus | null = null;
  adapterIssue: string | null = null;
  adapterWarnings: AdapterWarning[] = [];
  status: WorkerStatus | null = null;
  // While this page pauses, the worker clears the lease before the host
  // commits. That null lease is this page's own write, not another page's.
  private pausing = 0;

  constructor(
    private readonly onChange: () => void,
    // Called whenever the host revision changes, from any page. Anything bound
    // to the previous revision, such as a folder selection, is now stale.
    private readonly onRevisionChange: () => void = () => undefined,
  ) {
    // A host that exits is restarted by the next call; one that cannot start
    // leaves the page disconnected until a later call reaches it again.
    this.host.onStateChange(() => this.onChange());
  }

  get connected(): boolean {
    return this.config !== null && this.revision !== null && this.host.connected;
  }

  hello() {
    return this.host.call({
      type: "hello", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
    }, "hello_result");
  }

  readHostConfig() {
    return this.host.call({
      type: "get_config", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
    }, "config_result");
  }

  private setRevision(revision: string): void {
    const changed = this.revision !== null && this.revision !== revision;
    this.revision = revision;
    if (changed) this.onRevisionChange();
  }

  private setHostConfig(response: Awaited<ReturnType<ConfigSession["readHostConfig"]>>): void {
    this.config = response.config;
    this.setRevision(response.revision);
    this.configIssue = response.config_issue;
    this.agentStatus = response.agent_status;
    this.adapterIssue = response.adapter_issue;
    this.adapterWarnings = response.adapter_warnings;
    this.onChange();
  }

  /** config_updated carries no agent status, so read it after a save. A
   * newer revision belongs to another page, whose lease change reloads.
   * The save itself has committed, but an unverified adapter status keeps the
   * worker policy suspended. */
  private async refreshDerivedStatus(): Promise<boolean> {
    try {
      const latest = await this.readHostConfig();
      if (latest.revision === this.revision) {
        this.configIssue = latest.config_issue;
        this.agentStatus = latest.agent_status;
        this.adapterIssue = latest.adapter_issue;
        this.adapterWarnings = latest.adapter_warnings;
        return true;
      }
    } catch {
      this.agentStatus = null;
    }
    return false;
  }

  async applyHostConfig(response: Awaited<ReturnType<ConfigSession["readHostConfig"]>>): Promise<void> {
    this.setHostConfig(response);
    if (response.config_issue || response.adapter_issue) {
      await worker<WorkerStatus>({ kind: "suspend_policy" });
    }
    else await this.installHostPolicy();
  }

  async installHostPolicy(
    resumeAfterConfirmation = false,
    resumeAfterPauseToken: string | null = null,
  ): Promise<void> {
    if (!this.config || !this.revision) return;
    if (!this.config.storage && this.config.capture_enabled) {
      await worker<WorkerStatus>({ kind: "suspend_policy" });
      return;
    }
    await worker<WorkerStatus>({
      kind: "install_policy",
      lease: leaseFor(this.config, this.revision),
      // A disabled config returned by the host proves that a pending local pause
      // reached the host and can now be cleared safely.
      resume_after_confirmation: resumeAfterConfirmation || !this.config.capture_enabled,
      resume_after_pause_token: resumeAfterPauseToken,
    });
  }

  async refreshStatus(): Promise<WorkerStatus> {
    this.status = await worker<WorkerStatus>({ kind: "get_status" });
    this.onChange();
    return this.status;
  }

  /** Read the host and publish its policy. */
  async reload(): Promise<Awaited<ReturnType<ConfigSession["readHostConfig"]>>> {
    // Keep the host read and worker lease update together. Otherwise an older
    // page can reinstall a site after another page has removed it.
    const response = await withLatestHostState(
      withConfigMutationLock, () => this.readHostConfig(), (state) => this.applyHostConfig(state));
    await this.refreshStatus();
    return response;
  }

  async saveConfig(
    next: ConfigSnapshot,
    pickerToken: string | null,
    onCommitted?: () => void,
    resumeAfterConfirmation = false,
    resumeAfterPauseToken: string | null = null,
  ): Promise<void> {
    if (!this.revision) throw new Error("Host configuration has not loaded");
    // The revision check is the only gate on an ordinary settings change.
    const updated = await this.host.call({
      type: "update_config",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
      expected_revision: this.revision,
      config: next,
      picker_token: pickerToken,
    }, "config_updated");
    this.config = updated.config;
    this.setRevision(updated.revision);
    onCommitted?.();
    this.onChange();
    const statusVerified = await this.refreshDerivedStatus();
    if (statusVerified && this.configIssue === null && this.adapterIssue === null) {
      await this.installHostPolicy(resumeAfterConfirmation, resumeAfterPauseToken);
    } else {
      await worker<WorkerStatus>({ kind: "suspend_policy" });
    }
    await this.refreshStatus();
  }

  /** The harnesses the host found on its own search path. */
  async discoverHarnesses(): Promise<HarnessOffer[]> {
    const reply = await this.host.call({
      type: "discover_harnesses",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
    }, "harnesses_discovered", 60_000);
    return reply.offers;
  }

  /**
   * Harness setup. The host rediscovers `harnessId` on its own search path,
   * runs one test prompt through it, and commits the harness only when the
   * test passes. The extension never sends a harness path or arguments.
   * Call while holding the config mutation lock.
   */
  async setupHarness(
    harnessId: string,
    envNames: string[],
    summariesDir: string | null,
  ): Promise<void> {
    if (!this.revision) throw new Error("Host configuration has not loaded");
    // Discovery plus a test prompt of up to the harness timeout.
    const updated = await this.host.call({
      type: "set_up_harness",
      protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(),
      expected_revision: this.revision,
      harness_id: harnessId,
      env_names: envNames,
      summaries_dir: summariesDir,
    }, "config_updated", 6 * 60_000);
    this.config = updated.config;
    this.setRevision(updated.revision);
    this.onChange();
    const statusVerified = await this.refreshDerivedStatus();
    if (statusVerified && this.configIssue === null && this.adapterIssue === null) {
      await this.installHostPolicy();
    }
    else await worker<WorkerStatus>({ kind: "suspend_policy" });
    await this.refreshStatus();
  }

  /**
   * Write the sites and adapters an earlier build removed or saw revoked out
   * of the host config, in one update_config, then let the worker lift its
   * capture block once the host's lease no longer holds them. `config` and
   * `revision` are what this page last read. Returns whether it saved.
   */
  async applyPendingRemovals(config: ConfigSnapshot, revision: string): Promise<boolean> {
    const observed = await worker<WorkerStatus>({ kind: "get_status" });
    if (!observed.pending_removals.length) return false;
    let saved = false;
    await withConfigMutationLock(async () => {
      // Another page may have saved while this one waited for the lock.
      if (this.revision !== revision) return;
      const state = await worker<WorkerStatus>({ kind: "get_status" });
      if (!state.pending_removals.length) return;
      const removed = new Set(state.pending_removals);
      const sites = config.sites.filter((site) => !removed.has(site.origin));
      // An invalid or future adapter document can only pass the host's
      // narrow repair exception when echoed without structural changes, so
      // it stays as it is; the worker keeps its block until a later fix.
      const adapters = this.adapterIssue ? config.adapters : {
        format: config.adapters.format,
        artifacts: config.adapters.artifacts.filter((adapter) =>
          typeof adapter.origin !== "string" || !removed.has(adapter.origin)),
      };
      if (sites.length !== config.sites.length ||
          adapters.artifacts.length !== config.adapters.artifacts.length) {
        await this.saveConfig({
          ...config,
          sites,
          adapters,
          capture_enabled: config.capture_enabled && sites.length > 0,
        }, null);
        saved = true;
      }
      await worker<WorkerStatus>({ kind: "clear_pending_removals" });
    });
    await this.refreshStatus();
    return saved;
  }

  /** Call while holding the config mutation lock. */
  async completePausedHostConfig(): Promise<void> {
    this.pausing += 1;
    try {
      await this.finishPause();
    } finally {
      this.pausing -= 1;
    }
  }

  private async finishPause(): Promise<void> {
    await finishHostPause(
      async () => {
        const latest = await this.readHostConfig();
        this.setHostConfig(latest);
        return latest;
      },
      async () => { await worker<WorkerStatus>({ kind: "pause_capture" }); },
      async (latest) => {
        await this.saveConfig({ ...latest.config, capture_enabled: false }, null);
      },
      async (latest) => {
        if (latest.adapter_issue) await worker<WorkerStatus>({ kind: "suspend_policy" });
        else await this.installHostPolicy();
      },
      (error) => error instanceof HostError && error.code === "conflict",
    );
  }

  async pauseCapture(): Promise<void> {
    this.pausing += 1;
    try {
      // Stop local buffering immediately, even if the host is unavailable.
      await worker<WorkerStatus>({ kind: "pause_capture" });
      await withConfigMutationLock(() => this.completePausedHostConfig());
    } finally {
      this.pausing -= 1;
    }
  }

  /**
   * Call `onExternalChange` when the worker's lease no longer matches this
   * session, which means another page saved or paused the configuration.
   * Leases this session installs itself carry its own revision and are ignored.
   */
  watchPolicy(onExternalChange: () => void): void {
    chrome.storage.onChanged.addListener((changes, areaName) => {
      if (areaName !== "local" || !(POLICY_STORAGE_KEY in changes)) return;
      const lease = changes[POLICY_STORAGE_KEY]!.newValue;
      if (isPolicyLease(lease)) {
        if (lease.revision !== this.revision) onExternalChange();
      } else if (this.pausing === 0 && this.expectsLease()) {
        onExternalChange();
      }
    });
  }

  private expectsLease(): boolean {
    return this.config !== null && this.revision !== null && this.configIssue === null &&
      this.adapterIssue === null &&
      (this.config.storage !== null || !this.config.capture_enabled);
  }
}
