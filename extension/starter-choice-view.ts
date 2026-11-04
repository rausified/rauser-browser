// Renders one Capture Site choice group from a starter family's own data,
// replacing the hard-coded per-family blocks that used to live in
// panel.html. A family with new UI fields (protocol/starters/<family>.json)
// needs no change here: the group's checkboxes, status labels and scope
// field all come from `starter-adapters.ts` and `starter-scopes.ts`.
import type { CaptureChoice } from "./capture-site.js";
import {
  STARTER_STATUS_TEXT, choiceNoun, displayTypes, starterFamily,
} from "./starter-adapters.js";
import { scopeInput, unscopedNotice } from "./starter-scopes.js";

export interface StarterChoiceView {
  choice(): CaptureChoice;
  /** Every control off while saving or stale; a scope control also while none
   * of a multi-type family's boxes is checked. */
  setDisabled(disabled: boolean): void;
  /** Blanks typed scope values, then removes the controls. */
  clear(): void;
}

const EMPTY_VIEW: StarterChoiceView = {
  choice: () => ({ types: new Set(), scopes: {} }),
  setDisabled: () => undefined,
  clear: () => undefined,
};

function sentenceCase(text: string): string {
  return text.length ? text.charAt(0).toUpperCase() + text.slice(1) : text;
}

/**
 * One choice group for `origin`'s starter family, appended to `container`
 * (whose previous contents are replaced). `onInput` runs on every checkbox
 * or scope-field edit, as the panel's preview listeners did for the old
 * static controls. A family with no known starter, or one type and no scope
 * (SharePoint, Miro), renders nothing and reports an empty choice.
 */
export function renderStarterChoice(
  container: HTMLElement, origin: string, url: string, onInput: () => void,
): StarterChoiceView {
  const family = starterFamily(origin);
  if (!family) {
    container.replaceChildren();
    return EMPTY_VIEW;
  }
  const types = displayTypes(family);
  const input = scopeInput(family);
  const notice = unscopedNotice(family);
  if (types.length === 1 && family.scope === null) {
    container.replaceChildren();
    return EMPTY_VIEW;
  }

  const group = document.createElement("div");
  group.className = "starter-choice";
  group.setAttribute("role", "group");
  group.setAttribute("aria-labelledby", "capture-site-starter-prompt");
  group.dataset.family = family.family;

  const prompt = document.createElement("p");
  prompt.id = "capture-site-starter-prompt";
  prompt.textContent = family.review_prompt ?? "";
  group.append(prompt);

  if (notice) {
    const hint = document.createElement("p");
    hint.className = "hint";
    hint.textContent = notice;
    group.append(hint);
  }

  const boxes: HTMLInputElement[] = [];
  if (types.length >= 2) {
    const field = document.createElement("div");
    field.className = "field";
    for (const type of types) {
      const label = document.createElement("label");
      label.className = "check-label";
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = true;
      box.dataset.starterType = type.type;
      box.addEventListener("input", onInput);
      const span = document.createElement("span");
      span.textContent = `${sentenceCase(choiceNoun(type))} · ${STARTER_STATUS_TEXT[type.status.state]}`;
      label.append(box, span);
      field.append(label);
      boxes.push(box);
    }
    group.append(field);
  }

  let scopeBox: HTMLInputElement | null = null;
  if (input) {
    const field = document.createElement("div");
    field.className = "field";
    const label = document.createElement("label");
    label.htmlFor = "capture-site-starter-scope";
    label.textContent = input.label;
    const box = document.createElement("input");
    box.id = "capture-site-starter-scope";
    box.type = "text";
    box.maxLength = input.maxLength;
    box.placeholder = input.placeholder;
    box.autocomplete = "off";
    box.spellcheck = false;
    box.dataset.starterScope = family.family;
    // Shown and confirmed, never applied unseen: the user still has to save.
    box.value = input.fromUrl?.(url) ?? "";
    box.addEventListener("input", onInput);
    field.append(label, box);
    group.append(field);
    scopeBox = box;
  }

  container.replaceChildren(group);

  return {
    choice(): CaptureChoice {
      return {
        types: new Set(boxes.filter((box) => box.checked).map((box) => box.dataset.starterType!)),
        scopes: scopeBox ? { [family.family]: scopeBox.value } : {},
      };
    },
    setDisabled(disabled: boolean): void {
      for (const box of boxes) box.disabled = disabled;
      if (scopeBox) scopeBox.disabled = disabled || (boxes.length > 0 && !boxes.some((box) => box.checked));
    },
    clear(): void {
      if (scopeBox) scopeBox.value = "";
      container.replaceChildren();
    },
  };
}
