// The task bar's start: reads the active tab as Summarize & log does and
// sends start_task with the user's typed instruction. The host chooses the
// run's kind and record key and checks every field again; the page travels
// as untrusted context only. Nothing here passes the instruction, the page
// title or a report to diag (DESIGN.md §15).
import type { RunView } from "../protocol/ts/generated.js";
import { serviceSetupText } from "./agent.js";
import { APP_NAME } from "./brand.js";
import { wellFormed } from "./extract.js";
import { localTimestamp } from "./model.js";
import { HostError, PROTOCOL_VERSION, UNSUPPORTED_REQUEST_TEXT, newRequestId } from "./native.js";
import { markRunsMayExist } from "./run-card.js";
import { readTab, type SummarizeDeps } from "./summarize.js";

/** The textarea's `maxlength`, in UTF-16 units. The host's bound is 4096
 * characters (MAX_TASK_INSTRUCTION_CHARS), which this always fits. */
export const TASK_INPUT_MAX_UTF16_UNITS = 4000;
/** Why the task bar is off when no harness has passed its probe. */
export const TASK_NEEDS_HARNESS_MESSAGE = "Set up an agent (Codex or Claude Code) in settings to run tasks.";
/** A page no frame of which yields text (Chrome's PDF viewer, for one). */
export const TASK_UNREADABLE_MESSAGE = `${APP_NAME} could not read this page for a task.`;
/** A tab that moved to another page while it was read. */
export const TASK_PAGE_CHANGED_MESSAGE = "The page changed before it could be read. Run the task again.";
export const TASK_TOO_LARGE_MESSAGE = "This page is too large to send with a task.";
export const TASK_FINISH_SETUP_MESSAGE =
  `Finish setup in ${APP_NAME} settings to run tasks (home folder, knowledge base folder and agent).`;
export const TASK_REPAIR_MESSAGE = `${APP_NAME}'s settings need repair before a task can run.`;
export const TASK_REBUILD_HOST_MESSAGE = `Rebuild the ${APP_NAME} host to use the task bar.`;
/** start_task's answer was lost after the request may have reached the
 * service, which may have queued it; the Tasks page shows whether it did. */
export const TASK_MAY_HAVE_STARTED_MESSAGE = "The task may have started — check Tasks before running it again.";
// What an older host answers for a request type it does not know, such as
// start_task.
export { UNSUPPORTED_REQUEST_TEXT };
// The host's texts for a service whose answer it lost (its EXCHANGE_TIMEOUT
// or a closed connection) and for a reply it could not use.
const HOST_NO_ANSWER_TEXT = "The Brauser service stopped before answering. Try again in a moment.";
const HOST_UNEXPECTED_REPLY_TEXT = "The Brauser service sent an unexpected reply.";

/** Whether `error` lost start_task's answer rather than refusing the task:
 * the port closed, the extension or the host gave up waiting, or the
 * answer was unusable. The service may have queued the task meanwhile. */
function answerLost(error: HostError): boolean {
  switch (error.code) {
    case "disconnected":
    case "timeout":
    case "invalid_response":
      return true;
    case "service_unavailable":
      return error.message === HOST_NO_ANSWER_TEXT;
    case "internal":
      return error.message === HOST_UNEXPECTED_REPLY_TEXT;
    default:
      return false;
  }
}

// Every control character (Unicode Cc, as Rust's char::is_control) except
// newline and tab: the host refuses them.
const CONTROL = /(?![\n\t])\p{Cc}/gu;

/** The instruction as it is sent and stored: well-formed, every control
 * character other than `\n` and `\t` replaced with a space, then trimmed.
 * Reuse shows exactly this text. */
export function cleanInstruction(text: string): string {
  return wellFormed(text).replace(CONTROL, " ").trim();
}

/** What a failed startTask tells the user. */
export interface TaskFailure {
  text: string;
  /** Settings is the fix, so the line offers Open settings. */
  openSettings: boolean;
  /** The answer was lost, so the task may have started: the panel lists
   * the tasks again to count it. */
  uncertain: boolean;
}

/** What a failed startTask tells the user, whether settings is the fix,
 * and whether the task may have started anyway. Host refusals are mapped by
 * code, never by the host's text, which names summaries. */
export function taskFailureText(error: unknown): TaskFailure {
  const plain = (text: string): TaskFailure => ({ text, openSettings: false, uncertain: false });
  if (error instanceof HostError) {
    if (answerLost(error)) return { text: TASK_MAY_HAVE_STARTED_MESSAGE, openSettings: false, uncertain: true };
    switch (error.code) {
      case "unreadable":
        return plain(TASK_UNREADABLE_MESSAGE);
      case "page_changed":
        return plain(TASK_PAGE_CHANGED_MESSAGE);
      case "message_too_large":
        return plain(TASK_TOO_LARGE_MESSAGE);
      case "service_unavailable":
        return plain(serviceSetupText(error.message));
      case "conflict":
        return plain(error.message);
      case "not_configured":
        return { text: TASK_FINISH_SETUP_MESSAGE, openSettings: true, uncertain: false };
      case "invalid_config":
        return { text: TASK_REPAIR_MESSAGE, openSettings: true, uncertain: false };
      case "invalid_request":
        if (error.message === UNSUPPORTED_REQUEST_TEXT) return plain(TASK_REBUILD_HOST_MESSAGE);
        break;
      default:
        break;
    }
  }
  return plain(`Could not start the task: ${error instanceof Error ? error.message : String(error)}`);
}

/** Starts a task on `tab` with the user's `instruction`. An instruction
 * that cleaning leaves empty sends nothing and returns null. Otherwise the
 * tab is read (readTab, which throws `unreadable` or `page_changed` before
 * anything is sent) and start_task goes to the host, which answers with the
 * new run; a task never joins another. Runs are marked as possibly existing
 * before the start, so a panel closed mid-start still restores it, and
 * again after. */
export async function startTask(
  tab: { id: number; url: string; title: string },
  instruction: string,
  deps: SummarizeDeps,
): Promise<RunView | null> {
  const cleaned = cleanInstruction(instruction);
  if (cleaned === "") return null;
  await markRunsMayExist(deps.storage, deps.now().getTime());
  const page = await readTab(tab, deps);
  const response = await deps.host.call({
    type: "start_task",
    protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId(),
    url: page.url,
    title: page.title,
    content: page.content,
    selection: page.selection,
    instruction: cleaned,
    harness: null,
    requested_at: localTimestamp(deps.now()),
  }, "run_started");
  await markRunsMayExist(deps.storage, deps.now().getTime());
  return response.run;
}
