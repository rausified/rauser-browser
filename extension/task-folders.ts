// Working folders are host-owned. This UI sends opaque IDs, never paths.
import type { TaskFolderChoice } from "../protocol/ts/generated.js";
import { newRequestId, PROTOCOL_VERSION, type HostClient } from "./native.js";

export async function listTaskFolders(host: Pick<HostClient, "call">) {
  return host.call({ type: "list_task_folders", protocol_version: PROTOCOL_VERSION,
    request_id: newRequestId() }, "task_folders_listed");
}

/** Retain a removed Reuse choice visibly, so it can never become the default. */
export class TaskFolderPicker {
  private folders: TaskFolderChoice[] = [];
  private id: string | null = null;
  private generation = 0;
  constructor(private readonly select: HTMLSelectElement, private readonly changed: () => void) {
    this.render();
    select.addEventListener("change", () => {
      this.id = select.value || null;
      this.changed();
    });
  }
  get selectedId(): string | null { return this.id; }
  get selectedPath(): string | null { return this.folders.find((folder) => folder.id === this.id)?.path ?? null; }
  get missing(): boolean { return this.id !== null && !this.folders.some((folder) => folder.id === this.id); }
  choose(id: string | null): void {
    this.id = id;
    this.render();
    this.changed();
  }
  async refresh(host: Pick<HostClient, "call">): Promise<void> {
    const generation = ++this.generation;
    try {
      const response = await listTaskFolders(host);
      if (generation !== this.generation) return;
      this.folders = response.folders;
    } catch {
      // Older hosts support only the default. A missing reused choice stays
      // explicit and blocked, even when a list could not be read.
      if (generation !== this.generation) return;
      this.folders = [];
    }
    this.render();
    this.changed();
  }
  private render(): void {
    const choice = (value: string, text: string) => {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = text;
      return option;
    };
    const options = [choice("", "Knowledge base (default)"),
      ...this.folders.map((folder) => choice(folder.id, folder.path))];
    if (this.missing) options.push(choice(this.id!, "Unavailable folder — choose a working folder"));
    this.select.replaceChildren(...options);
    this.select.value = this.id ?? "";
  }
}
