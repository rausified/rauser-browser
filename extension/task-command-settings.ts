import type { TaskCommandDefinition } from "../protocol/ts/generated.js";
import { HostError, newRequestId, PROTOCOL_VERSION } from "./native.js";
import { ConfigSession, describe, element, withConfigMutationLock } from "./settings.js";

type Row = { fieldset: HTMLFieldSetElement; fields: (HTMLInputElement | HTMLTextAreaElement)[]; remove: HTMLButtonElement };

/** Only user-authored values enter this editor. Refreshes never replace a draft. */
export class TaskCommandSettingsControls {
  private readonly open = element<HTMLButtonElement>("commands-open");
  private readonly editor = element<HTMLElement>("commands-editor");
  private readonly rowsElement = element<HTMLElement>("commands-rows");
  private readonly add = element<HTMLButtonElement>("commands-add");
  private readonly repair = element<HTMLElement>("commands-repair");
  private readonly original = element<HTMLTextAreaElement>("commands-original");
  private readonly toml = element<HTMLTextAreaElement>("commands-toml");
  private readonly status = element<HTMLElement>("commands-status");
  private readonly save = element<HTMLButtonElement>("commands-save");
  private readonly review = element<HTMLButtonElement>("commands-review");
  private readonly current = element<HTMLTextAreaElement>("commands-current");
  private readonly currentLabel = element<HTMLElement>("commands-current-label");
  private readonly cancel = element<HTMLButtonElement>("commands-cancel");
  private rows: Row[] = [];
  private nextRow = 0;
  private revision: string | null = null;
  private repairing = false;

  constructor(
    private readonly session: ConfigSession,
    private readonly busy: () => boolean,
    private readonly setBusy: (value: boolean) => void,
  ) {
    this.open.addEventListener("click", () => void this.begin());
    this.add.addEventListener("click", () => {
      if (this.add.disabled) return;
      const row = this.addRow({ name: "", description: "", prompt: "" });
      this.render();
      row.fields[0]!.focus();
    });
    this.save.addEventListener("click", () => void this.submit());
    this.review.addEventListener("click", () => void this.reviewCurrent());
    this.cancel.addEventListener("click", () => {
      if (this.busy()) return;
      this.editor.hidden = true;
      this.revision = null;
      this.render();
      this.open.focus();
    });
  }

  render(): void {
    const unavailable = !this.session.connected || this.busy();
    this.open.disabled = unavailable || this.revision !== null;
    this.add.hidden = this.repairing;
    this.add.disabled = unavailable || this.revision === null || this.rows.length >= 32;
    this.cancel.disabled = this.busy();
    this.toml.disabled = this.busy();
    for (const row of this.rows) {
      for (const field of row.fields) field.disabled = this.busy();
      row.remove.disabled = this.busy();
    }
    const stale = this.revision !== null && this.revision !== this.session.revision;
    this.save.disabled = unavailable || this.revision === null || stale;
    this.review.hidden = !stale;
    this.review.disabled = unavailable;
    if (stale) this.status.textContent = "Settings changed. Your draft is kept. Review current commands before replacing the saved list.";
  }

  private read(expected: string) {
    return this.session.host.call({
      type: "get_task_command_settings", protocol_version: PROTOCOL_VERSION,
      request_id: newRequestId(), expected_revision: expected,
    }, "task_command_settings");
  }

  private addRow(command: TaskCommandDefinition): Row {
    const fieldset = document.createElement("fieldset");
    const legend = document.createElement("legend");
    const number = ++this.nextRow;
    legend.textContent = `Command ${number}`;
    fieldset.append(legend);
    const fields = ([
      ["name", "Name (without /)", command.name],
      ["description", "Description", command.description],
      ["prompt", "Instruction", command.prompt],
    ] as const).map(([key, title, value]) => {
      const label = document.createElement("label");
      const field = key === "prompt" ? document.createElement("textarea") : document.createElement("input");
      field.id = `command-${number}-${key}`;
      field.setAttribute("autocomplete", "off");
      if (key === "prompt") field.setAttribute("rows", "5");
      label.setAttribute("for", field.id);
      label.textContent = title;
      field.value = value;
      fieldset.append(label, field);
      return field;
    });
    const remove = document.createElement("button");
    remove.type = "button";
    remove.className = "secondary";
    remove.textContent = "Remove command";
    fieldset.append(remove);
    const row = { fieldset, fields, remove };
    remove.addEventListener("click", () => {
      if (remove.disabled) return;
      const index = this.rows.indexOf(row);
      this.rows.splice(index, 1);
      this.rowsElement.replaceChildren(...this.rows.map((r) => r.fieldset));
      this.render();
      (this.rows[index]?.fields[0] ?? this.rows[index - 1]?.fields[0] ?? this.add).focus();
    });
    this.rows.push(row);
    this.rowsElement.append(fieldset);
    return row;
  }

  private async begin(): Promise<void> {
    if (this.open.disabled || !this.session.revision) return;
    this.setBusy(true);
    // Show failures here too, including an older host without these messages.
    this.editor.hidden = false;
    try {
      const result = await this.read(this.session.revision);
      this.revision = result.revision;
      this.repairing = result.issue !== null;
      this.repair.hidden = !this.repairing;
      this.original.value = result.original_toml;
      this.toml.value = result.original_toml;
      this.rows = [];
      this.rowsElement.replaceChildren();
      for (const command of result.commands) this.addRow(command);
      this.current.hidden = this.currentLabel.hidden = true;
      this.status.textContent = result.issue ?? "Edit the complete list, then Save commands. Removing a row takes effect only when saved.";
      element<HTMLElement>("commands-editor-heading").focus();
    } catch (error) {
      this.status.textContent = `Could not open commands: ${describe(error)}`;
      if (error instanceof HostError && error.code === "conflict") await this.session.reload().catch(() => undefined);
    } finally {
      this.setBusy(false);
      this.render();
    }
  }

  private async reviewCurrent(): Promise<void> {
    if (this.review.disabled) return;
    let reviewed = false;
    this.setBusy(true);
    try {
      // Refresh first; a conflict response must never rebase the draft silently.
      await this.session.reload();
      const result = await this.read(this.session.revision!);
      this.current.value = result.original_toml || "# No saved commands";
      this.current.hidden = this.currentLabel.hidden = false;
      this.revision = result.revision;
      reviewed = true;
      this.status.textContent = "Current saved values are shown below. Your draft is unchanged. Save commands replaces that entire list with your draft.";
    } catch (error) {
      this.status.textContent = `Could not review current commands: ${describe(error)}`;
    } finally {
      this.setBusy(false);
      this.render();
      (reviewed ? this.current : this.review).focus();
    }
  }

  private async submit(): Promise<void> {
    this.render();
    if (this.save.disabled || !this.revision) return;
    // JSON string quoting is compatible with these validated TOML basic
    // strings. The host parses and validates every row before publication.
    const text = this.repairing ? this.toml.value : this.rows.length === 0 ? "commands = []\n" : this.rows.map((row) => {
      const [name, description, prompt] = row.fields.map((field) => JSON.stringify(field.value));
      return `[[commands]]\nname = ${name}\ndescription = ${description}\nprompt = ${prompt}\noutput = "sidebar"\n`;
    }).join("\n");
    this.setBusy(true);
    try {
      await withConfigMutationLock(() => this.session.saveTaskCommands(text, this.revision!));
      this.revision = this.session.revision;
      this.current.hidden = this.currentLabel.hidden = true;
      this.status.textContent = "Commands saved. Open Saved commands in the task bar and refresh its list to insert one. No task was started.";
    } catch (error) {
      this.status.textContent = `Could not save commands: ${describe(error)} Your draft is kept.`;
      if (error instanceof HostError && error.code === "conflict") await this.session.reload().catch(() => undefined);
    } finally {
      this.setBusy(false);
      this.render();
      // Disabling an active button removes browser focus. Restore it only
      // after the busy state is cleared, or use the stale-review action.
      const target = !this.save.disabled ? this.save : !this.review.hidden && !this.review.disabled ? this.review : this.cancel;
      target.focus();
    }
  }
}
