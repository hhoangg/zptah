// Minimal typings for the chrome.* APIs the extension uses. Extend as tickets add calls.
declare namespace chrome {
  namespace runtime {
    interface Port {
      postMessage(message: unknown): void;
      disconnect(): void;
      onDisconnect: { addListener(cb: () => void): void };
      onMessage: { addListener(cb: (message: unknown) => void): void };
    }
    const lastError: { message?: string } | undefined;
    function connectNative(name: string): Port;
    const id: string;
    function getURL(path: string): string;
    function getManifest(): { version: string; version_name?: string };
    function sendMessage(message: unknown): Promise<any>;
    interface MessageSender {
      id?: string;
      url?: string;
    }
    const onMessage: {
      addListener(cb: (message: unknown, sender: MessageSender, sendResponse: (response?: unknown) => void) => boolean | void): void;
    };
    const onStartup: { addListener(cb: () => void): void };
    const onInstalled: { addListener(cb: () => void): void };
  }
  namespace alarms {
    interface Alarm {
      name: string;
    }
    function create(name: string, info: { periodInMinutes?: number; delayInMinutes?: number }): void;
    const onAlarm: { addListener(cb: (alarm: Alarm) => void): void };
  }
  namespace storage {
    const local: {
      setAccessLevel(options: { accessLevel: "TRUSTED_CONTEXTS" | "TRUSTED_AND_UNTRUSTED_CONTEXTS" }): Promise<void>;
      get(key: string): Promise<Record<string, unknown>>;
      set(items: Record<string, unknown>): Promise<void>;
    };
  }
  namespace tabs {
    interface Tab {
      id?: number;
    }
    function getCurrent(): Promise<Tab | undefined>;
    function remove(id: number): Promise<void>;
  }
}
