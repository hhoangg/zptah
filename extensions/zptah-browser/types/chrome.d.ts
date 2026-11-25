// Minimal typings for the chrome.* APIs the extension uses. Extend as tickets add calls.
declare namespace chrome {
  namespace runtime {
    interface Port {
      postMessage(message: unknown): void;
      onDisconnect: { addListener(cb: () => void): void };
      onMessage: { addListener(cb: (message: unknown) => void): void };
    }
    const lastError: { message?: string } | undefined;
    function connectNative(name: string): Port;
    function getManifest(): { version: string };
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
}
