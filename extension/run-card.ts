// The panel's summary run cards (spec §5): each polls its run in the Brauser
// service and offers the controls that fit its state. The service owns the
// run; a card only shows it, so closing the panel loses nothing, and the next
// panel restores the cards from list_runs. Agent text is untrusted and is
// rendered as text only (textContent), never as HTML.
import type { RunAnswer, RunView } from "../protocol/ts/generated.js";
import { serviceSetupText } from "./agent.js";
import { storageKey } from "./brand.js";
import { HostError, PROTOCOL_VERSION, newRequestId, type HostClient } from "./native.js";

export const POLL_MS = 1000;
export const RESTORE_FINISHED_WITHIN_MS = 24 * 60 * 60 * 1000;
export const MAX_RESTORED_FINISHED = 5;
const DISMISSED_RUNS_KEY = storageKey("dismissed_runs_v1");
const MAX_DISMISSED_RUNS = 200;
// A shell word that needs no quoting.
const PLAIN_SHELL_WORD = /^[A-Za-z0-9._-]+$/;

export interface RunCardDeps {
  host: Pick<HostClient, "call">;
  now(): number;
  copy(text: string): Promise<void>;
  visible(): boolean;
  storage: ChromeStorageArea;
  openSettings(): void;
}

function isTerminal(run: RunView): boolean {
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
      return "Summarized";
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

/** The runs a newly opened panel shows: every queued or running run, then the
 * newest {@link MAX_RESTORED_FINISHED} finished in the last day that the user
 * has not dismissed. */
export function restorable(runs: RunView[], now: number, dismissed: ReadonlySet<string>): RunView[] {
  const active = runs.filter((run) => !isTerminal(run));
  const finished = runs
    .filter((run) => isTerminal(run) && !dismissed.has(run.run_id))
    .map((run) => ({ run, at: run.finished_at === null ? NaN : Date.parse(run.finished_at) }))
    .filter(({ at }) => Number.isFinite(at) && now - at <= RESTORE_FINISHED_WITHIN_MS)
    .sort((a, b) => b.at - a.at)
    .slice(0, MAX_RESTORED_FINISHED)
    .map(({ run }) => run);
  return [...active, ...finished];
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

/** One run's card. It polls while the run is unfinished and the panel is
 * visible, and fetches once more after the run finishes so the card holds
 * all of its stream. */
class RunCard {
  readonly element: HTMLDivElement;
  private readonly titleLine: HTMLParagraphElement;
  private readonly stateLine: HTMLParagraphElement;
  private readonly answerText: HTMLParagraphElement;
  private readonly streamText: HTMLPreElement;
  private readonly problemLine: HTMLParagraphElement;
  private readonly actions: HTMLDivElement;
  private stream = "";
  private offset = 0;
  // Whether a get_run has seen the run finished since the last action.
  private finalFetched = false;
  // Bumped by each action, so a fetch sent before it cannot end the polling.
  private epoch = 0;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private fetching = false;
  private acting = false;
  private gone = false;
  private disposed = false;
  private problem: string | null = null;

  constructor(
    private view: RunView,
    private readonly deps: RunCardDeps,
    private readonly onDismiss: (card: RunCard) => void,
  ) {
    this.element = make("div", "run-card");
    this.element.dataset.runId = view.run_id;
    this.titleLine = make("p", "run-card-title");
    this.stateLine = make("p", "run-card-state");
    this.answerText = make("p", "run-card-answer");
    this.streamText = make("pre", "run-card-stream");
    this.problemLine = make("p", "run-card-problem warning");
    this.actions = make("div", "run-card-actions");
    this.element.append(
      this.titleLine, this.stateLine, this.answerText, this.streamText, this.problemLine, this.actions,
    );
    this.render();
  }

  get runId(): string {
    return this.view.run_id;
  }

  start(): void {
    void this.tick();
  }

  /** A newer view from outside the card, such as list_runs. */
  update(run: RunView): void {
    this.setView(run);
    this.render();
  }

  dispose(): void {
    this.disposed = true;
    if (this.timer !== null) clearTimeout(this.timer);
    this.timer = null;
  }

  private done(): boolean {
    return this.gone || (isTerminal(this.view) && this.finalFetched);
  }

  // A finished run never goes back to running, so a get_run answered before
  // a cancel cannot undo it; Retry save goes from one finished state to
  // another.
  private setView(run: RunView): void {
    if (isTerminal(this.view) && !isTerminal(run)) return;
    this.view = run;
  }

  private schedule(): void {
    if (this.disposed || this.timer !== null || this.done()) return;
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.tick();
    }, POLL_MS);
  }

  private async tick(): Promise<void> {
    if (this.disposed || this.fetching || this.done()) return;
    if (this.deps.visible()) await this.fetch();
    this.schedule();
  }

  /** Reads the run and its new stream text, following `more` to the end. */
  private async fetch(): Promise<void> {
    this.fetching = true;
    const epoch = this.epoch;
    try {
      for (;;) {
        const fetched = await this.deps.host.call({
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
        if (isTerminal(fetched.run) && epoch === this.epoch) this.finalFetched = true;
        this.problem = null;
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
      if (!this.disposed) this.render();
    }
  }

  private async act(action: () => Promise<RunView | void>, failure: string): Promise<void> {
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

  private cancel(): void {
    void this.act(async () => (await this.deps.host.call({
      type: "cancel_run", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
      run_id: this.view.run_id,
    }, "run_updated")).run, "Could not cancel");
  }

  private retrySave(): void {
    void this.act(async () => (await this.deps.host.call({
      type: "retry_save", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
      run_id: this.view.run_id,
    }, "run_updated")).run, "Could not retry the save");
  }

  private openInCodex(): void {
    void this.act(async () => {
      await this.deps.host.call({
        type: "open_path", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
        target: "codex_thread", run_id: this.view.run_id,
      }, "path_opened");
    }, "Could not open Codex");
  }

  private copy(text: string): void {
    void this.act(() => this.deps.copy(text), "Could not copy");
  }

  private render(): void {
    const run = this.view;
    this.titleLine.textContent = run.title || run.url;
    const elapsed = elapsedText(run, this.deps.now());
    this.stateLine.textContent = elapsed ? `${stateText(run)} · ${elapsed}` : stateText(run);
    const answer = run.state === "succeeded" && run.answer ? answerLine(run.answer) : "";
    this.answerText.textContent = answer;
    this.answerText.hidden = answer === "";
    this.streamText.textContent = this.stream;
    this.streamText.hidden = this.stream === "";
    this.problemLine.textContent = this.problem ?? "";
    this.problemLine.hidden = this.problem === null;

    const buttons: HTMLButtonElement[] = [];
    if (!isTerminal(run)) buttons.push(actionButton("Cancel", () => this.cancel()));
    if (run.harness === "codex" && run.thread_id) {
      buttons.push(actionButton("Open in Codex", () => this.openInCodex()));
    }
    if (run.harness === "claude" && run.session_id && run.cwd) {
      const command = resumeCommand(run.cwd, run.session_id);
      buttons.push(actionButton("Copy resume command", () => this.copy(command)));
    }
    if (run.can_retry_save) buttons.push(actionButton("Retry save", () => this.retrySave()));
    if (run.state === "failed") {
      buttons.push(actionButton("Copy", () => this.copy(this.stream || (this.view.error ?? ""))));
    }
    if (isTerminal(run)) buttons.push(actionButton("Dismiss", () => this.onDismiss(this)));
    for (const button of buttons) button.disabled = this.acting;
    this.actions.replaceChildren(...buttons);
  }
}

/** The run cards in `container`, which they share with other panel content;
 * they add and remove only their own elements. */
export class RunCards {
  private readonly cards = new Map<string, RunCard>();
  private notice: HTMLElement | null = null;
  private disposed = false;

  constructor(private readonly container: HTMLElement, private readonly deps: RunCardDeps) {}

  /** Shows `run` at the top, or updates its card if it has one. */
  show(run: RunView): void {
    this.add(run, true);
  }

  /** Shows the runs a new panel should (see {@link restorable}). */
  async restore(): Promise<void> {
    let runs: RunView[];
    try {
      runs = (await this.deps.host.call({
        type: "list_runs", protocol_version: PROTOCOL_VERSION, request_id: newRequestId(),
      }, "runs_listed")).runs;
    } catch (error) {
      if (this.disposed) return;
      if (error instanceof HostError && error.code === "service_unavailable") {
        this.showNotice(serviceSetupText(error.message), true);
      } else {
        this.showNotice(`Could not list summary runs: ${errorText(error)}`, false);
      }
      return;
    }
    const dismissed = new Set(await this.dismissedIds());
    if (this.disposed) return;
    this.clearNotice();
    for (const run of restorable(runs, this.deps.now(), dismissed)) this.add(run, false);
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
    const cardElements = new Set<Element>(Array.from(this.cards.values(), (other) => other.element));
    const children = Array.from(this.container.children);
    const cardPositions = children.flatMap((child, index) => cardElements.has(child) ? [index] : []);
    const position = cardPositions.length === 0
      ? children.length
      : atTop ? cardPositions[0]! : cardPositions[cardPositions.length - 1]! + 1;
    children.splice(position, 0, card.element);
    this.container.replaceChildren(...children);
    this.cards.set(run.run_id, card);
    card.start();
  }

  private remove(element: HTMLElement): void {
    this.container.replaceChildren(...Array.from(this.container.children).filter((child) => child !== element));
  }

  private async dismissedIds(): Promise<string[]> {
    try {
      const stored = (await this.deps.storage.get(DISMISSED_RUNS_KEY))[DISMISSED_RUNS_KEY];
      return Array.isArray(stored) ? stored.filter((id): id is string => typeof id === "string") : [];
    } catch {
      return [];
    }
  }

  private async dismiss(card: RunCard): Promise<void> {
    card.dispose();
    this.cards.delete(card.runId);
    this.remove(card.element);
    const ids = (await this.dismissedIds()).filter((id) => id !== card.runId);
    ids.push(card.runId);
    try {
      await this.deps.storage.set({ [DISMISSED_RUNS_KEY]: ids.slice(-MAX_DISMISSED_RUNS) });
    } catch {
      // The card is gone for this panel; a later panel may restore it.
    }
  }

  private showNotice(text: string, withSettings: boolean): void {
    this.clearNotice();
    const box = make("div", "setup-warning run-notice");
    const line = make("p", "");
    line.textContent = text;
    box.append(line);
    if (withSettings) box.append(actionButton("Open settings", () => this.deps.openSettings()));
    this.notice = box;
    this.container.replaceChildren(box, ...Array.from(this.container.children));
  }

  private clearNotice(): void {
    if (this.notice) this.remove(this.notice);
    this.notice = null;
  }
}
