// The M1 shell uses this narrow subset of Chrome's MV3 APIs. Runtime policy is
// still enforced by the native host; these declarations grant no authority.
interface ChromePort {
  postMessage(message: unknown): void;
  disconnect(): void;
  onMessage: { addListener(listener: (message: unknown) => void): void };
  onDisconnect: { addListener(listener: () => void): void };
}

interface ChromePermissionSet {
  permissions?: string[];
  origins?: string[];
}

interface ChromeTab {
  id?: number;
  windowId?: number;
  active?: boolean;
  status?: string;
  discarded?: boolean;
  url?: string;
  title?: string;
  incognito: boolean;
}

interface ChromeTabActivatedInfo {
  tabId: number;
  windowId: number;
}

interface ChromeTabChangeInfo {
  title?: string;
  url?: string;
  status?: string;
}

interface ChromeStorageArea {
  get(keys: string | string[]): Promise<Record<string, unknown>>;
  set(items: Record<string, unknown>): Promise<void>;
  remove?(keys: string | string[]): Promise<void>;
  setAccessLevel?(options: { accessLevel: "TRUSTED_CONTEXTS" }): Promise<void>;
}

interface ChromeNavigationDetails {
  tabId: number;
  frameId: number;
  documentId?: string;
  documentLifecycle?: string;
  url: string;
  timeStamp: number;
  transitionType?: string;
  transitionQualifiers?: string[];
}

interface ChromeMessageSender {
  id?: string;
  url?: string;
}

interface ChromeContextMenuClickInfo {
  menuItemId: string | number;
}

interface ChromeInjectionTarget {
  tabId: number;
  frameIds?: number[];
}

interface ChromeInjectionResult<T> {
  frameId: number;
  result?: T;
}

interface ChromeApi {
  runtime: {
    id: string;
    getURL(path: string): string;
    reload(): void;
    openOptionsPage(): Promise<void>;
    getManifest(): { options_ui?: { page: string } };
    connectNative(name: string): ChromePort;
    sendMessage<T = unknown>(message: unknown): Promise<T>;
    lastError?: { message: string };
    onMessage: {
      addListener(
        listener: (
          message: unknown,
          sender: ChromeMessageSender,
          sendResponse: (response: unknown) => void,
        ) => boolean | void,
      ): void;
    };
    onInstalled: { addListener(listener: (details: { reason: string }) => void): void };
    onStartup: { addListener(listener: () => void): void };
  };
  alarms: {
    create(name: string, info: { delayInMinutes?: number; periodInMinutes?: number }): Promise<void>;
    get(name: string): Promise<{ name: string } | undefined>;
    onAlarm: { addListener(listener: (alarm: { name: string }) => void): void };
  };
  scripting: {
    executeScript<T>(injection: {
      target: { tabId: number; frameIds: [0] };
      world: "ISOLATED";
      func: (expectedUrl: string, readLinks: boolean) => T;
      args: [string, boolean];
    }): Promise<{ frameId: number; result?: T }[]>;
    executeScript(injection: { target: ChromeInjectionTarget; files: string[] }): Promise<ChromeInjectionResult<unknown>[]>;
    executeScript<Args extends unknown[], Result>(injection: {
      target: ChromeInjectionTarget;
      func: (...args: Args) => Result;
      args: Args;
    }): Promise<ChromeInjectionResult<Awaited<Result>>[]>;
  };
  commands: {
    onCommand: { addListener(listener: (command: string, tab?: ChromeTab) => void): void };
  };
  sidePanel: {
    setPanelBehavior(options: { openPanelOnActionClick: boolean }): Promise<void>;
    open(options: { tabId?: number; windowId?: number }): Promise<void>;
  };
  action: {
    setBadgeText(details: { text: string }): Promise<void>;
    setBadgeBackgroundColor(details: { color: string }): Promise<void>;
  };
  contextMenus: {
    create(
      properties: { id: string; title: string; contexts: string[] },
      callback?: () => void,
    ): void;
    removeAll(callback?: () => void): void;
    onClicked: {
      addListener(listener: (info: ChromeContextMenuClickInfo, tab?: ChromeTab) => void): void;
    };
  };
  permissions: {
    contains(permissions: ChromePermissionSet): Promise<boolean>;
    onRemoved: { addListener(listener: (permissions: ChromePermissionSet) => void): void };
    onAdded: { addListener(listener: (permissions: ChromePermissionSet) => void): void };
  };
  storage: {
    local: ChromeStorageArea & {
      remove(keys: string | string[]): Promise<void>;
      setAccessLevel(options: { accessLevel: "TRUSTED_CONTEXTS" }): Promise<void>;
    };
    session: ChromeStorageArea;
    onChanged: {
      addListener(
        listener: (
          changes: Record<string, { oldValue?: unknown; newValue?: unknown }>,
          areaName: string,
        ) => void,
      ): void;
    };
  };
  tabs: {
    get(tabId: number): Promise<ChromeTab>;
    query(queryInfo: { active?: boolean; currentWindow?: boolean }): Promise<ChromeTab[]>;
    create(properties: { url: string }): Promise<ChromeTab>;
    onUpdated: {
      addListener(
        listener: (tabId: number, changeInfo: ChromeTabChangeInfo, tab: ChromeTab) => void,
      ): void;
    };
    onActivated: {
      addListener(listener: (info: ChromeTabActivatedInfo) => void): void;
    };
    onRemoved?: { addListener(listener: (tabId: number) => void): void };
    onReplaced?: { addListener(listener: (addedTabId: number, removedTabId: number) => void): void };
  };
  webNavigation: {
    onCommitted: { addListener(listener: (details: ChromeNavigationDetails) => void): void };
    onHistoryStateUpdated: { addListener(listener: (details: ChromeNavigationDetails) => void): void };
    onCreatedNavigationTarget?: {
      addListener(listener: (details: { sourceTabId: number; tabId: number }) => void): void;
    };
    onTabReplaced?: {
      addListener(listener: (details: { tabId: number; replacedTabId: number }) => void): void;
    };
    getFrame?(details: { tabId: number; frameId: number }): Promise<{ url?: string } | null>;
  };
  windows?: {
    WINDOW_ID_NONE: number;
    getLastFocused(): Promise<{ id?: number; focused: boolean }>;
    onFocusChanged: { addListener(listener: (windowId: number) => void): void };
  };
  idle?: {
    setDetectionInterval(seconds: number): void;
    queryState(seconds: number): Promise<"active" | "idle" | "locked">;
    onStateChanged: { addListener(listener: (state: "active" | "idle" | "locked") => void): void };
  };
}

declare const chrome: ChromeApi;
