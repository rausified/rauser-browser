// Settings view of the host's record of adopted adapters
// (adopted-contracts.json). A damaged record pauses notes on pages the
// current adapters do not recognize; Forget recorded adapters is its repair.
// Recording warnings are notices only: nothing is paused. Forget is also
// offered for a full record (adopted_state.full), which a fresh record fixes.
import { HostError } from "./native.js";
import { type ConfigSession, describe, element } from "./settings.js";

const FORGOTTEN = "Recorded adapters forgotten. The old record was backed up beside it, and a fresh record holds the adapters you have now.";
const CHANGED = "The record of adopted adapters changed since this page read it. Review it again before forgetting it.";

export class AdoptedStateControls {
  private readonly panel = element<HTMLDivElement>("adopted-state");
  private readonly issue = element<HTMLParagraphElement>("adopted-issue");
  private readonly warnings = element<HTMLUListElement>("adopted-warnings");
  private readonly forgetButton = element<HTMLButtonElement>("forget-adopted");
  private readonly confirmBox = element<HTMLDivElement>("forget-adopted-confirm");
  private readonly confirmButton = element<HTMLButtonElement>("forget-adopted-yes");
  private readonly keepButton = element<HTMLButtonElement>("forget-adopted-no");
  private forgetting = false;

  constructor(
    private readonly session: ConfigSession,
    private readonly show: (message: string, warning?: boolean) => void,
  ) {
    // The first click only opens the in-page confirmation.
    this.forgetButton.addEventListener("click", () => {
      this.confirmBox.hidden = false;
      this.render();
    });
    this.keepButton.addEventListener("click", () => {
      this.confirmBox.hidden = true;
      this.render();
    });
    this.confirmButton.addEventListener("click", () => void this.forget());
  }

  render(): void {
    const state = this.session.adoptedState;
    const issue = state?.issue ?? null;
    const warnings = state?.warnings ?? [];
    this.panel.hidden = issue === null && warnings.length === 0;
    this.issue.hidden = issue === null;
    this.issue.textContent = issue ?? "";
    this.warnings.replaceChildren(...warnings.map((warning) => {
      const item = document.createElement("li");
      item.textContent = warning;
      return item;
    }));
    // A full record is the one recording warning forgetting repairs.
    const forgettable = issue !== null || state?.full === true;
    if (!forgettable) this.confirmBox.hidden = true;
    this.forgetButton.hidden = !forgettable || !this.confirmBox.hidden;
    const usable = this.session.connected && !this.forgetting;
    this.forgetButton.disabled = !usable;
    this.confirmButton.disabled = !usable;
    this.keepButton.disabled = this.forgetting;
  }

  private async forget(): Promise<void> {
    if (this.forgetting) return;
    this.forgetting = true;
    this.render();
    try {
      await this.session.forgetAdoptedContracts();
      this.show(FORGOTTEN);
    } catch (error) {
      if (error instanceof HostError && error.code === "conflict") {
        try {
          await this.session.reload();
        } catch (reloadError) {
          this.show(`${CHANGED} ${describe(reloadError)}`, true);
          return;
        }
        this.show(CHANGED, true);
      } else {
        this.show(`Could not forget recorded adapters: ${describe(error)}`, true);
      }
    } finally {
      this.forgetting = false;
      this.confirmBox.hidden = true;
      this.render();
    }
  }
}
