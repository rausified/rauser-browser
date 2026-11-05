// Optional extension permission; no host config, capture setting or service
// is touched. Permission requests start directly in the button's click event.
export function taskNotificationSettings(
  api: Pick<ChromeApi, "permissions" | "runtime">,
  button: HTMLButtonElement,
  status: HTMLElement,
): void {
  let enabled = false;
  let busy = false;
  const permission = { permissions: ["notifications"] };
  function render(): void {
    button.textContent = enabled ? "Turn off task notifications" : "Enable task notifications";
    button.disabled = busy;
    status.textContent = enabled ? "On. Tasks can notify you when the side panel is closed."
      : "Off. Task progress and reports remain available in Tasks.";
  }
  async function refresh(): Promise<void> {
    try { enabled = await api.permissions.contains(permission); render(); }
    catch { status.textContent = "Could not read task notification permission. Try again."; }
  }
  button.addEventListener("click", () => {
    if (busy) return;
    const hadFocus = typeof document !== "undefined" && document.activeElement === button;
    busy = true;
    button.disabled = true;
    // Do not await a storage read or host call before this user gesture.
    const change = enabled ? api.permissions.remove(permission) : api.permissions.request(permission);
    void change.then(async (granted) => {
      if (granted) {
        // Clear the old ledger on both explicit disable and re-enable, so
        // re-enabling never alerts for tasks from an earlier opt-in period.
        const reply = await api.runtime.sendMessage<{ ok: boolean }>({ kind: "task_notice_reset" });
        if (!reply?.ok) throw new Error("Task monitoring could not be reset.");
      }
      await refresh();
      if (!enabled && !granted) status.textContent = "Permission was not granted. Task notifications stay off.";
    }).catch(() => {
      status.textContent = "Could not change task notifications. Check Chrome's extension permissions and try again.";
    }).finally(() => {
      busy = false;
      button.disabled = false;
      if (hadFocus && (document.activeElement === document.body || document.activeElement === button)) button.focus();
    });
  });
  void refresh();
}
