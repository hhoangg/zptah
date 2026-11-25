// Service worker entry. Skeleton: keeps a native messaging port to the zptah host alive.
// The protocol, pairing and tools arrive in later tickets.
import { ALARM_NAME, HOST_NAME, PING_INTERVAL_MS, PROTOCOL } from "./config.js";

/** @type {chrome.runtime.Port | null} */
let port = null;
let counter = 0;

function connect() {
  if (port) return;
  try {
    port = chrome.runtime.connectNative(HOST_NAME);
  } catch {
    port = null;
    return;
  }
  port.onDisconnect.addListener(() => {
    // Read lastError so Chrome does not log it as unchecked.
    void chrome.runtime.lastError;
    port = null;
  });
}

function ping() {
  connect();
  if (!port) return;
  counter += 1;
  port.postMessage({ v: PROTOCOL, type: "evt", id: `x-${counter}`, method: "ping", params: {} });
}

// A worker that only lives while events arrive would be stopped after 30 s idle. The 20 s
// ping keeps it and the port alive; the alarm wakes it again if it was stopped anyway.
setInterval(ping, PING_INTERVAL_MS);
chrome.alarms.create(ALARM_NAME, { periodInMinutes: 0.5 });
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === ALARM_NAME) ping();
});
chrome.runtime.onStartup.addListener(connect);
chrome.runtime.onInstalled.addListener(connect);
connect();
