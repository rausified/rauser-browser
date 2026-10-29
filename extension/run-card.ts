// The panel's summary run cards and the Tasks page's task view (spec §5):
// each polls its run in the Brauser service and offers the controls that fit
// its state. The service owns the run; a view only shows it, so closing the
// panel loses nothing, and the next panel restores the cards from list_runs.
// Agent text is untrusted and is rendered as text only (textContent), never
// as HTML.
import type { RunAnswer, RunView, SummaryView } from "../protocol/ts/generated.js";
import { serviceSetupText } from "./agent.js";
import { APP_NAME, storageKey } from "./brand.js";
import {
  HostError, PROTOCOL_VERSION, UNSUPPORTED_REQUEST_TEXT, newRequestId, type HostClient,
} from "./native.js";
import { summaryLine } from "./summary-text.js";

export const POLL_MS = 1000;
export const RESTORE_FINISHED_WITHIN_MS = 24 * 60 * 60 * 1000;
export const MAX_RESTORED_FINISHED = 5;
const DISMISSED_RUNS_KEY = storageKey("dismissed_runs_v1");
const MAX_DISMISSED_RUNS = 1000;
// When a run was last started or shown, kept until a list_runs finds nothing
// to restore. list_runs starts the on-demand service, so a panel with no
// harness asks only while this says the service may still hold a run.
const RUNS_MAY_EXIST_KEY = storageKey("runs_may_exist_v1");
// A shell word that needs no quoting.
const PLAIN_SHELL_WORD = /^[A-Za-z0-9._-]+$/;

/** What a run's progress view needs: the host, a clock, the clipboard, and
 * whether the panel is visible (it polls only then). */
export interface RunProgressDeps {
  host: Pick<HostClient, "call">;
  now(): number;
  copy(text: string): Promise<void>;
  visible(): boolean;
}

/** What an older host, which does not know `list_runs`' `kind`, needs: the
 * task wording (the Tasks page and the task bar's tracker notice), and the
 * summary restore's. */
export const RUNS_REBUILD_HOST_MESSAGE = `Rebuild the ${APP_NAME} host to see task history.`;
export const SUMMARY_REBUILD_HOST_MESSAGE = `Rebuild the ${APP_NAME} host to restore agent runs.`;

export interface RunCardDeps extends RunProgressDeps {
  storage: ChromeStorageArea;
  openSettings(): void;
  /** A card saw its run succeed and save a summary, so the page's own
   * Summary line can refresh: a summary event does not change the
   * worker's delivery count the panel otherwise refreshes on. */
  summarySaved?(run: RunView): void;
}

export function isTerminal(run: RunView): boolean {
  return run.state === "succeeded" || run.state === "failed" || run.state === "canceled";
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function twoDigits(value: number): string {
  return String(value).padStart(2, "0");
}

/** How long the run has run: from its start to its finish, or to `now` while
 * it runs. A run that never started has no elapsed time. */
export function elapsedText(run: RunView, now: number): string {
  const start = run.started_at === null ? NaN : Date.parse(run.started_at);
  const end = run.finished_at === null ? now : Date.parse(run.finished_at);
  if (!Number.isFinite(start) || !Number.isFinite(end)) return "";
  const seconds = Math.max(0, Math.floor((end - start) / 1000));
  if (seconds < 60) return `${seconds}s`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m ${twoDigits(seconds % 60)}s`;
  return `${Math.floor(minutes / 60)}h ${twoDigits(minutes % 60)}m`;
}

export function stateText(run: RunView): string {
  switch (run.state) {
    case "queued":
      return "Queued";
    case "running":
      return "Running";
    case "succeeded":
      switch (run.kind) {
        case "task":
          return "Done";
        case "site_draft":
          return "Drafted";
        default:
          return "Summarized";
      }
    case "failed":
      return `Failed: ${run.error ?? "the run did not finish"}`;
    case "canceled":
      return "Canceled";
  }
}

export function answerLine(answer: RunAnswer): string {
  return `Type: ${answer.document_type} · Category: ${answer.category} · ` +
    `Importance: ${answer.importance} — ${answer.importance_reason}`;
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

/** The Terminal command that reopens a Claude Code run's session where it
 * ran. The session id came from the harness, so it is quoted unless plain. */
export function resumeCommand(cwd: string, sessionId: string): string {
  const id = PLAIN_SHELL_WORD.test(sessionId) ? sessionId : shellQuote(sessionId);
  return `cd ${shellQuote(cwd)} && claude --resume ${id}`;
}

/** Whether a run for `runUrl` belongs to the page at `pageUrl`. A fragment
 * names a place in the page, not another page. */
export function samePage(runUrl: string, pageUrl: string): boolean {
  return runUrl.split("#")[0] === pageUrl.split("#")[0];
}

/** The summary runs a newly opened panel shows: every queued or running
 * run, then the newest {@link MAX_RESTORED_FINISHED} finished in the last day
 * that the user has not dismissed. A run without a kind is a summary; a
 * task shows on the Tasks page, and a Capture Site draft run belongs to its
 * review, so neither is ever a card. */
export function restorable(runs: RunView[], now: number, dismissed: ReadonlySet<string>): RunView[] {
  const ofKind = runs.filter((run) => (run.kind ?? "summary") === "summary");
  const active = ofKind.filter((run) => !isTerminal(run));
  const finished = ofKind
    .filter((run) => isTerminal(run) && !dismissed.has(run.run_id))
    .map((run) => ({ run, at: run.finished_at === null ? NaN : Date.parse(run.finished_at) }))
    .filter(({ at }) => Number.isFinite(at) && now - at <= RESTORE_FINISHED_WITHIN_MS)
    .sort((a, b) => b.at - a.at)
    .slice(0, MAX_RESTORED_FINISHED)
    .map(({ run }) => run);
  return [...active, ...finished];
}

/** Records, at `at` (ms), that the service may hold a run to restore. */
export async function markRunsMayExist(storage: Pick<ChromeStorageArea, "set">, at: number): Promise<void> {
  try {
    await storage.set({ [RUNS_MAY_EXIST_KEY]: at });
  } catch {
    // A panel with a harness set up still restores; this matters only once
    // the harness is removed.
  }
}

/** Whether the service may hold a run a panel should restore. An unreadable
 * record counts as yes: waking the service costs less than hiding a run. */
export async function runsMayExist(storage: Pick<ChromeStorageArea, "get">): Promise<boolean> {
  try {
    return typeof (await storage.get(RUNS_MAY_EXIST_KEY))[RUNS_MAY_EXIST_KEY] === "number";
  } catch {
    return true;
  }
}

/** Forgets the record unless a run was recorded after `listedAt`, when the
 * list_runs that found nothing was sent. */
async function clearRunsMayExist(storage: ChromeStorageArea, listedAt: number): Promise<void> {
  try {
    const at = (await storage.get(RUNS_MAY_EXIST_KEY))[RUNS_MAY_EXIST_KEY];
    if (typeof at === "number" && at > listedAt) return;
    if (storage.remove) await storage.remove(RUNS_MAY_EXIST_KEY);
    else await storage.set({ [RUNS_MAY_EXIST_KEY]: null });
  } catch {
    // The next panel asks again.
  }
}

function make<K extends keyof HTMLElementTagNameMap>(tag: K, className: string): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  element.className = className;
  return element;
}

function actionButton(text: string, action: () => void): HTMLButtonElement {
  const button = make("button", "secondary compact");
  button.type = "button";
  button.textContent = text;
  button.addEventListener("click", action);
  return button;
}

/** What every run view shares: it polls `get_run` while the run is
 * unfinished and the panel is visible, fetches once more after the run
 * finishes so it holds all of its stream, and offers Cancel and the
 * harness's own resume (Open in Codex, Copy resume command). The stream is
 * untrusted agent text, kept as text only. Subclasses build their elements,
 * then call `render()`. */
abstract class RunWatcher {
  protected stream = "";
  private offset = 0;
  // Whether a get_run has seen the run finished since the last action.
  private finalFetched = false;
  // Bumped by each action, so a fetch sent before it cannot end the polling.
  private epoch = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private fetching = false;
  protected acting = false;
  private gone = false;
  protected disposed = false;
  protected problem: string | null = null;

  constructor(protected view: RunView, protected readonly baseDeps: RunProgressDeps) {}

  get runId(): string {
    return this.view.run_id;
  }

  start(): void {
    void this.tick();
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }

  protected abstract render(): void;

  /** Called after each fetch or action that may have finished the run. */
  protected settled(): void {}

  /** Called with each view a get_run returned, even one too old to apply. */
  protected fetchedView(run: RunView): void {
    void run;
  }

  protected done(): boolean {
    return this.gone || (isTerminal(this.view) && this.finalFetched);
  }

  // A finished run never goes back to running, so a get_run answered before
  // a cancel cannot undo it; Retry save goes from one finished state to
  // another.
  protected setView(run: RunView): void {
    if (isTerminal(this.view) && !isTerminal(run)) return;
    this.view = run;
  }

  protected schedule(): void {
    if (this.disposed || this.timer !== null || this.done()) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.tick();
    }, POLL_MS);
  }

  private async tick(): Promise<void> {
    if (this.disposed || this.fetching || this.done()) return;
    if (this.baseDeps.visible()) await this.fetch();
    this.schedule();
  }

  /** Reads the run and its new stream text, following `more` to the end. */
  private async fetch(): Promise<void> {
    this.fetching = true;
    const epoch = this.epoch;
    try {
      for (;;) {
        const fetched = await this.baseDeps.host.call({
          type: "get_run", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
          run_id: this.view.run_id, since_offset: this.offset,
        }, "run_fetched");
        if (this.disposed) return;
        const progressed = fetched.next_offset > this.offset;
        if (progressed) {
          this.stream += fetched.stream_text;
          this.offset = fetched.next_offset;
        }
        this.setView(fetched.run);
        this.fetchedView(fetched.run);
        this.problem = null;
        // The final fetch is done only once a finished run's stream is all
        // here; a later page that fails is retried on the next poll.
        if (!fetched.more && isTerminal(fetched.run) && epoch === this.epoch) this.finalFetched = true;
        if (!fetched.more || !progressed) break;
      }
    } catch (error) {
      if (this.disposed) return;
      // The service forgets runs past its retention; nothing more will come.
      if (error instanceof HostError && error.code === "invalid_request") {
        this.gone = true;
        this.problem = "This run is no longer available.";
      } else {
        this.problem = `Could not refresh this run: ${errorText(error)}`;
      }
    } finally {
      this.fetching = false;
      if (!this.disposed) {
        this.render();
        this.settled();
      }
    }
  }

  protected async act(action: () => Promise<RunView | void>, failure: string): Promise<void> {
    if (this.acting) return;
    this.acting = true;
    this.render();
    try {
      const run = await action();
      if (this.disposed) return;
      this.problem = null;
      if (run) {
        this.epoch += 1;
        this.setView(run);
        this.finalFetched = false;
        this.schedule();
      }
    } catch (error) {
      this.problem = `${failure}: ${errorText(error)}`;
    } finally {
      this.acting = false;
      if (!this.disposed) this.render();
    }
  }

  protected cancelRun(): Promise<RunView> {
    return this.baseDeps.host.call({
      type: "cancel_run", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
      run_id: this.view.run_id,
    }, "run_updated").then((updated) => updated.run);
  }

  private cancel(): void {
    void this.act(() => this.cancelRun(), "Could not cancel");
  }

  private openInCodex(): void {
    void this.act(async () => {
      await this.baseDeps.host.call({
        type: "open_path", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
        target: "codex_thread", run_id: this.view.run_id,
      }, "path_opened");
    }, "Could not open Codex");
  }

  protected copy(text: string): void {
    void this.act(() => this.baseDeps.copy(text), "Could not copy");
  }

  /** The state line: the state, then the elapsed time when there is one. */
  protected stateLineText(): string {
    const elapsed = elapsedText(this.view, this.baseDeps.now());
    return elapsed ? `${stateText(this.view)} · ${elapsed}` : stateText(this.view);
  }

  /** Cancel, while the run is unfinished. */
  protected cancelButtons(): HTMLButtonElement[] {
    return isTerminal(this.view) ? [] : [actionButton("Cancel", () => this.cancel())];
  }

  /** The harness's own way back into the run's session. */
  protected resumeButtons(): HTMLButtonElement[] {
    const run = this.view;
    if (run.harness === "codex" && run.thread_id) return [actionButton("Open in Codex", () => this.openInCodex())];
    if (run.harness === "claude" && run.session_id && run.cwd) {
      const command = resumeCommand(run.cwd, run.session_id);
      return [actionButton("Copy resume command", () => this.copy(command))];
    }
    return [];
  }
}

/** One summary run's card. Beyond the shared progress it shows the
 * answer and the saved summary, and offers Open summary, Retry save, Copy
 * and Dismiss. */
class RunCard extends RunWatcher {
  readonly element: HTMLDivElement;
  private readonly titleLine: HTMLParagraphElement;
  private readonly stateLine: HTMLParagraphElement;
  private readonly answerText: HTMLParagraphElement;
  private readonly savedText: HTMLParagraphElement;
  private readonly streamText: HTMLPreElement;
  private readonly problemLine: HTMLParagraphElement;
  private readonly actions: HTMLDivElement;
  // The summary file the card last asked get_artifact about, so it asks once
  // per saved file, and the page's saved summary that call returned. A
  // failed call is retried once, a poll interval later (`summaryRetried`
  // names the file it was retried for).
  private summaryAskedFor: string | null = null;
  private summaryRetried: string | null = null;
  private summaryTimer: ReturnType<typeof setTimeout> | null = null;
  private saved: SummaryView | null = null;

  get url(): string {
    return this.view.url;
  }

  constructor(
    view: RunView,
    private readonly deps: RunCardDeps,
    private readonly onDismiss: (card: RunCard) => void,
  ) {
    super(view, deps);
    this.element = make("div", "run-card");
    this.element.dataset.runId = view.run_id;
    this.titleLine = make("p", "run-card-title");
    this.stateLine = make("p", "run-card-state");
    this.answerText = make("p", "run-card-answer");
    this.savedText = make("p", "run-card-summary");
    this.streamText = make("pre", "run-card-stream");
    this.problemLine = make("p", "run-card-problem warning");
    this.actions = make("div", "run-card-actions");
    this.element.append(
      this.titleLine, this.stateLine, this.answerText, this.savedText, this.streamText, this.problemLine,
      this.actions,
    );
    this.render();
    void this.loadSavedSummary();
  }

  /** A newer view from outside the card, such as list_runs. */
  update(run: RunView): void {
    this.setView(run);
    this.render();
  }

  override dispose(): void {
    super.dispose();
    if (this.summaryTimer !== null) clearTimeout(this.summaryTimer);
    this.summaryTimer = null;
  }

  protected override setView(run: RunView): void {
    const before = this.savedPath();
    super.setView(run);
    if (this.view !== run) return;
    if (before === null && this.savedPath() !== null) this.deps.summarySaved?.(run);
    void this.loadSavedSummary();
  }

  /** The summary file a succeeded run saved, or null. */
  private savedPath(): string | null {
    return this.view.state === "succeeded" ? this.view.summary_path : null;
  }

  /** Asks get_artifact once for the page's saved summary when the run has
   * saved one, retrying a failure once on a later poll. A failure leaves
   * only the line out: Open summary still works from the run's url. */
  private async loadSavedSummary(): Promise<void> {
    const path = this.savedPath();
    if (path === null || path === this.summaryAskedFor || this.disposed) return;
    this.summaryAskedFor = path;
    this.saved = null;
    try {
      const result = await this.deps.host.call({
        type: "get_artifact", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
        url: this.view.url, title: this.view.title || null,
      }, "artifact_result");
      if (this.disposed || this.summaryAskedFor !== path) return;
      this.saved = result.summary;
    } catch {
      if (this.disposed || this.summaryAskedFor !== path || this.summaryRetried === path) return;
      this.summaryRetried = path;
      this.summaryTimer = setTimeout(() => {
        this.summaryTimer = null;
        if (this.summaryAskedFor !== path) return;
        this.summaryAskedFor = null;
        void this.loadSavedSummary();
      }, POLL_MS);
      return;
    }
    this.render();
  }

  private retrySave(): void {
    void this.act(async () => (await this.deps.host.call({
      type: "retry_save", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
      run_id: this.view.run_id,
    }, "run_updated")).run, "Could not retry the save");
  }

  private openSummary(): void {
    void this.act(async () => {
      await this.deps.host.call({
        type: "open_summary", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
        // The file this run wrote, whatever record its URL resolves to now.
        url: this.view.url, summary_path: this.view.summary_path,
      }, "summary_opened");
    }, "Could not open the summary");
  }

  protected render(): void {
    const run = this.view;
    this.titleLine.textContent = run.title || run.url;
    this.stateLine.textContent = this.stateLineText();
    const answer = run.state === "succeeded" && run.answer ? answerLine(run.answer) : "";
    this.answerText.textContent = answer;
    this.answerText.hidden = answer === "";
    const savedPath = this.savedPath();
    const saved = savedPath !== null && this.saved ? summaryLine(this.saved) : "";
    this.savedText.textContent = saved;
    this.savedText.hidden = saved === "";
    this.streamText.textContent = this.stream;
    this.streamText.hidden = this.stream === "";
    this.problemLine.textContent = this.problem ?? "";
    this.problemLine.hidden = this.problem === null;

    const buttons: HTMLButtonElement[] = [...this.cancelButtons()];
    // A failed save can still have written the file (the daily log then
    // refused its event); the run keeps its path, so it opens too.
    if (isTerminal(run) && run.summary_path !== null) {
      buttons.push(actionButton("Open summary", () => this.openSummary()));
    }
    buttons.push(...this.resumeButtons());
    if (run.can_retry_save) buttons.push(actionButton("Retry save", () => this.retrySave()));
    if (run.state === "failed") {
      buttons.push(actionButton("Copy", () => this.copy(this.stream || (this.view.error ?? ""))));
    }
    if (isTerminal(run)) buttons.push(actionButton("Dismiss", () => this.onDismiss(this)));
    for (const button of buttons) button.disabled = this.acting;
    this.actions.replaceChildren(...buttons);
  }
}

const HARNESS_NAMES: Record<RunView["harness"], string> = { codex: "Codex", claude: "Claude Code" };
const CHANGES_STAY = "Changes the agent already made stay; check before running it again.";

/** What the Tasks page's {@link TaskDetail} needs. */
export interface TaskDetailDeps extends RunProgressDeps {
  storage: ChromeStorageArea;
  /** Puts the full instruction back in the panel's task bar. */
  reuse(instruction: string): void;
  /** Opens the run's own `url` (never a URL from agent text). */
  openPage(url: string): void;
  /** The run was dismissed, and its id remembered. */
  dismissed(runId: string): void;
  /** Each new view of the run, so the page can update its list row. */
  changed(run: RunView): void;
}

/** The local start time of a run, or null before it starts. */
function startedText(run: RunView): string | null {
  const start = run.started_at === null ? NaN : Date.parse(run.started_at);
  if (!Number.isFinite(start)) return null;
  return new Date(start).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/**
 * One task on the Tasks page (decision 20): the full instruction as the
 * headline, the meta line, the actions, then the report and a collapsible
 * Agent output, hidden until the agent has streamed something. The output
 * starts open while the run is unfinished and closes once when it finishes
 * (or starts closed for a run already finished); after that the user's
 * toggle stays across re-renders (decision 43). It
 * offers Cancel, Copy report, Copy (a failed task's error), Reuse, the
 * harness's resume, Open page and Dismiss, and never Open summary or Retry
 * save: a task writes nothing. The full instruction and the report come
 * from get_run views only (list_runs cuts the instruction and leaves the
 * report out, so a list view never replaces either). All agent text is
 * shown with textContent only.
 */
export class TaskDetail extends RunWatcher {
  readonly element: HTMLDivElement;
  private fullInstruction: string | null = null;
  private report: string | null = null;
  private readonly headline: HTMLHeadingElement;
  private readonly metaLine: HTMLParagraphElement;
  private readonly actions: HTMLDivElement;
  private readonly problemLine: HTMLParagraphElement;
  private readonly noteLine: HTMLParagraphElement;
  private readonly reportLabel: HTMLHeadingElement;
  private readonly reportText: HTMLDivElement;
  private readonly output: HTMLDetailsElement;
  private readonly streamText: HTMLPreElement;
  // Whether the output has been closed for the finish (or started closed).
  private outputSettled: boolean;

  constructor(view: RunView, private readonly deps: TaskDetailDeps) {
    super(view, deps);
    this.element = make("div", "task-detail");
    this.element.dataset.runId = view.run_id;
    this.headline = make("h2", "task-detail-instruction");
    this.metaLine = make("p", "task-detail-meta");
    this.actions = make("div", "run-card-actions");
    this.problemLine = make("p", "run-card-problem warning");
    this.noteLine = make("p", "task-detail-note");
    this.reportLabel = make("h3", "task-detail-report-label");
    this.reportLabel.textContent = "Report";
    this.reportText = make("div", "task-detail-report");
    this.output = make("details", "task-detail-output");
    const summary = make("summary", "");
    summary.textContent = "Agent output";
    this.streamText = make("pre", "run-card-stream");
    this.output.append(summary, this.streamText);
    this.outputSettled = isTerminal(view);
    this.output.open = !this.outputSettled;
    this.element.append(
      this.headline, this.metaLine, this.actions, this.problemLine, this.noteLine, this.reportLabel,
      this.reportText, this.output,
    );
    this.render();
  }

  /** The run as last seen. */
  get run(): RunView {
    return this.view;
  }

  protected override setView(run: RunView): void {
    super.setView(run);
    if (this.view === run) this.deps.changed(run);
  }

  protected override fetchedView(run: RunView): void {
    if (typeof run.instruction === "string") this.fullInstruction = run.instruction;
    if (typeof run.task_report === "string") this.report = run.task_report;
  }

  /** The instruction to show: the full one once a get_run has it. */
  private instructionText(): string {
    return this.fullInstruction ?? this.view.instruction ?? "";
  }

  /** The agent's report, once the run succeeded with one. */
  private shownReport(): string | null {
    return this.view.state === "succeeded" ? this.report : null;
  }

  /** Whether the run failed or was canceled after the agent started. */
  private changesMayStay(): boolean {
    const run = this.view;
    return (run.state === "failed" || run.state === "canceled") && run.started_at !== null;
  }

  private copyReportButtons(): HTMLButtonElement[] {
    const report = this.shownReport();
    return report === null ? [] : [actionButton("Copy report", () => this.copy(report))];
  }

  /** Copy, on a failed task: its error, or else its stream. */
  private copyFailureButtons(): HTMLButtonElement[] {
    if (this.view.state !== "failed") return [];
    return [actionButton("Copy", () => this.copy(this.view.error || this.stream))];
  }

  /** Reuse, on a finished task once a get_run gave its full instruction. */
  private reuseButtons(): HTMLButtonElement[] {
    const instruction = this.fullInstruction;
    if (!isTerminal(this.view) || instruction === null) return [];
    return [actionButton("Reuse", () => this.deps.reuse(instruction))];
  }

  private dismiss(): void {
    const runId = this.view.run_id;
    void rememberDismissed(this.deps.storage, runId).then(() => this.deps.dismissed(runId));
  }

  protected render(): void {
    const run = this.view;
    this.headline.textContent = this.instructionText();
    const started = startedText(run);
    const elapsed = elapsedText(run, this.deps.now());
    this.metaLine.textContent = [
      `on ${run.title || run.url}`, HARNESS_NAMES[run.harness], started === null ? "" : `started ${started}`,
      stateText(run), elapsed,
    ].filter((part) => part !== "").join(" · ");
    this.problemLine.textContent = this.problem ?? "";
    this.problemLine.hidden = this.problem === null;
    const changed = this.changesMayStay();
    this.noteLine.textContent = changed ? CHANGES_STAY : "";
    this.noteLine.hidden = !changed;
    const report = this.shownReport();
    this.reportText.textContent = report ?? "";
    this.reportText.hidden = report === null;
    this.reportLabel.hidden = report === null;
    this.streamText.textContent = this.stream;
    // No disclosure until the agent has streamed something (decision 43).
    this.output.hidden = this.stream === "";
    if (!this.outputSettled && isTerminal(run)) {
      this.outputSettled = true;
      this.output.open = false;
    }

    const buttons: HTMLButtonElement[] = [
      ...this.cancelButtons(), ...this.copyReportButtons(), ...this.copyFailureButtons(),
      ...this.reuseButtons(), ...this.resumeButtons(),
      actionButton("Open page", () => this.deps.openPage(this.view.url)),
    ];
    if (isTerminal(run)) buttons.push(actionButton("Dismiss", () => this.dismiss()));
    for (const button of buttons) button.disabled = this.acting;
    this.actions.replaceChildren(...buttons);
  }
}

/**
 * A run's progress without the summary card's parts: the state line and
 * elapsed time, the streamed text (as text only), Cancel, and Open in Codex
 * or Copy resume command. It has no answer line, Open summary or Retry
 * save; the Capture Site review shows it for an Improve with AI run and
 * judges the finished run itself. `onFinished` is called once, with the
 * finished run, after its last stream page is fetched.
 */
export class RunProgress extends RunWatcher {
  readonly element: HTMLDivElement;
  private readonly stateLine: HTMLParagraphElement;
  private readonly streamText: HTMLPreElement;
  private readonly problemLine: HTMLParagraphElement;
  private readonly actions: HTMLDivElement;
  private reported = false;

  constructor(view: RunView, deps: RunProgressDeps, private readonly onFinished: (run: RunView) => void) {
    super(view, deps);
    this.element = make("div", "run-card");
    this.element.dataset.runId = view.run_id;
    this.stateLine = make("p", "run-card-state");
    this.streamText = make("pre", "run-card-stream");
    this.problemLine = make("p", "run-card-problem warning");
    this.actions = make("div", "run-card-actions");
    this.element.append(this.stateLine, this.streamText, this.problemLine, this.actions);
    this.render();
  }

  /** The run as last seen. */
  get run(): RunView {
    return this.view;
  }

  /** Stops watching, and cancels the run when it is unfinished: closing
   * the review discards the shapes it was drafting from. A cancel that
   * fails is not retried; the run then finishes on its own. */
  async close(): Promise<void> {
    const unfinished = !isTerminal(this.view);
    this.dispose();
    if (!unfinished) return;
    try {
      await this.cancelRun();
    } catch {
      // The service's timeout still bounds the run.
    }
  }

  protected override settled(): void {
    if (this.reported || this.disposed || !isTerminal(this.view) || !this.done()) return;
    this.reported = true;
    this.onFinished(this.view);
  }

  protected render(): void {
    this.stateLine.textContent = this.stateLineText();
    this.streamText.textContent = this.stream;
    this.streamText.hidden = this.stream === "";
    this.problemLine.textContent = this.problem ?? "";
    this.problemLine.hidden = this.problem === null;
    const buttons = [...this.cancelButtons(), ...this.resumeButtons()];
    for (const button of buttons) button.disabled = this.acting;
    this.actions.replaceChildren(...buttons);
  }
}

/** Every panel's dismissals write one after another, whichever cards they
 * come from, so each read sees the last write and two quick dismissals (a
 * summary and a task, say) both stay remembered. */
let dismissWrites: Promise<void> = Promise.resolve();

export async function dismissedIds(storage: Pick<ChromeStorageArea, "get">): Promise<string[]> {
  try {
    const stored = (await storage.get(DISMISSED_RUNS_KEY))[DISMISSED_RUNS_KEY];
    return Array.isArray(stored) ? stored.filter((id): id is string => typeof id === "string") : [];
  } catch {
    return [];
  }
}

export function rememberDismissed(storage: ChromeStorageArea, runId: string): Promise<void> {
  const write = dismissWrites.then(async () => {
    const ids = (await dismissedIds(storage)).filter((id) => id !== runId);
    ids.push(runId);
    try {
      await storage.set({ [DISMISSED_RUNS_KEY]: ids.slice(-MAX_DISMISSED_RUNS) });
    } catch {
      // The card is gone for this panel; a later panel may restore it.
    }
  });
  dismissWrites = write;
  return write;
}

// The restore notice each area shows, at most one.
const notices = new WeakMap<HTMLElement, HTMLElement>();

function removeChild(area: HTMLElement, element: HTMLElement): void {
  area.replaceChildren(...Array.from(area.children).filter((child) => child !== element));
}

function showNotice(area: HTMLElement, text: string, openSettings: (() => void) | null): void {
  clearNotice(area);
  const box = make("div", "setup-warning run-notice");
  const line = make("p", "");
  line.textContent = text;
  box.append(line);
  if (openSettings) box.append(actionButton("Open settings", openSettings));
  notices.set(area, box);
  area.replaceChildren(box, ...Array.from(area.children));
}

/** Removes the run-list notice from `area`, if it shows one. */
export function clearNotice(area: HTMLElement): void {
  const notice = notices.get(area);
  if (notice) removeChild(area, notice);
  notices.delete(area);
}

/** Shows the one notice in `area` for a failed `list_runs`, of either kind:
 * the service setup instructions with Open settings, `rebuildText` for an
 * older host that refuses `kind`, or "Could not list agent runs: …". */
export function showListFailure(
  area: HTMLElement, error: unknown, openSettings: () => void, rebuildText = RUNS_REBUILD_HOST_MESSAGE,
): void {
  if (error instanceof HostError && error.code === "service_unavailable") {
    showNotice(area, serviceSetupText(error.message), openSettings);
  } else if (error instanceof HostError && error.code === "invalid_request" && error.message === UNSUPPORTED_REQUEST_TEXT) {
    showNotice(area, rebuildText, null);
  } else {
    showNotice(area, `Could not list agent runs: ${errorText(error)}`, null);
  }
}

/** What one restore for several card sets needs. */
export type RestoreDeps = Pick<RunCardDeps, "host" | "now" | "storage" | "openSettings">;

/**
 * Lists the summary runs once and shows each card set the runs it should
 * (see {@link restorable}). A failed list shows one notice, in `noticeArea`;
 * a list that works clears it. The record that runs may exist is cleared
 * only when no set shows a card and the caller does not `keepRecord` (it
 * knows of an active task, say).
 */
export async function restoreRunCards(
  containers: readonly RunCards[], noticeArea: HTMLElement, deps: RestoreDeps,
  options: { keepRecord?: boolean } = {},
): Promise<void> {
  let runs: RunView[];
  const listedAt = deps.now();
  try {
    runs = (await deps.host.call({
      type: "list_runs", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(), kind: "summary",
    }, "runs_listed")).runs;
  } catch (error) {
    if (containers.some((cards) => cards.isDisposed)) return;
    showListFailure(noticeArea, error, () => deps.openSettings(), SUMMARY_REBUILD_HOST_MESSAGE);
    return;
  }
  const dismissed = new Set(await dismissedIds(deps.storage));
  if (containers.some((cards) => cards.isDisposed)) return;
  clearNotice(noticeArea);
  let shown = 0;
  for (const cards of containers) shown += cards.restoreFrom(runs, dismissed);
  if (shown === 0 && options.keepRecord !== true) await clearRunsMayExist(deps.storage, listedAt);
}

/** The summary run cards in `container`, which they share with other
 * panel content; they add and remove only their own elements. */
export class RunCards {
  private readonly cards = new Map<string, RunCard>();
  private disposed = false;
  // The page the panel follows: only its cards show. Undefined until the
  // panel first follows a tab, when every card shows; null for no web page.
  private followed: string | null | undefined = undefined;

  constructor(private readonly container: HTMLElement, private readonly deps: RunCardDeps) {}

  get isDisposed(): boolean {
    return this.disposed;
  }

  /** Shows `run` at the top, or updates its card if it has one. A run just
   * started proves the host answers, so a restore failure notice in this
   * set's container goes. */
  show(run: RunView): void {
    if (this.disposed) return;
    clearNotice(this.container);
    this.add(run, true);
  }

  /** Lists the runs and shows the ones a new panel should, with any
   * failure notice in this set's own container. */
  restore(): Promise<void> {
    return restoreRunCards([this], this.container, this.deps);
  }

  /** Shows the runs a new panel should, from one list_runs answer, and
   * returns how many cards the set now holds. */
  restoreFrom(runs: RunView[], dismissed: ReadonlySet<string>): number {
    if (this.disposed) return 0;
    for (const run of restorable(runs, this.deps.now(), dismissed)) this.add(run, false);
    return this.cards.size;
  }

  /** Shows only the cards for the page at `url` (null: no web page). The
   * others stay, hidden and still polling, until their page is followed. */
  follow(url: string | null): void {
    this.followed = url;
    for (const card of this.cards.values()) this.place(card);
  }

  dispose(): void {
    this.disposed = true;
    for (const card of this.cards.values()) card.dispose();
  }

  private add(run: RunView, atTop: boolean): void {
    if (this.disposed) return;
    const existing = this.cards.get(run.run_id);
    if (existing) {
      existing.update(run);
      return;
    }
    const card = new RunCard(run, this.deps, (dismissed) => void this.dismiss(dismissed));
    this.place(card);
    const cardElements = new Set<Element>(Array.from(this.cards.values(), (other) => other.element));
    const children = Array.from(this.container.children);
    const cardPositions = children.flatMap((child, index) => cardElements.has(child) ? [index] : []);
    const position = cardPositions.length === 0
      ? children.length
      : atTop ? cardPositions[0]! : cardPositions[cardPositions.length - 1]! + 1;
    children.splice(position, 0, card.element);
    this.container.replaceChildren(...children);
    this.cards.set(run.run_id, card);
    void markRunsMayExist(this.deps.storage, this.deps.now());
    card.start();
  }

  private place(card: RunCard): void {
    const followed = this.followed;
    card.element.hidden = followed !== undefined && (followed === null || !samePage(card.url, followed));
  }

  private async dismiss(card: RunCard): Promise<void> {
    card.dispose();
    this.cards.delete(card.runId);
    removeChild(this.container, card.element);
    await rememberDismissed(this.deps.storage, card.runId);
  }
}
