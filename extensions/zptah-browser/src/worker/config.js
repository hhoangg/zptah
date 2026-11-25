// Build-time constants. scripts/build.mjs rewrites HOST_NAME for the dev build.
export const HOST_NAME = "app.zptah.browser_host";
export const PROTOCOL = 1;
export const PING_INTERVAL_MS = 20000;
export const ALARM_NAME = "zptah-keepalive";
// Longest wait for the app to answer a handshake step.
export const HANDSHAKE_TIMEOUT_MS = 10000;
