// Reconnect delay: 1 s, 2 s, 4 s ... capped at 5 minutes.
export const BACKOFF_BASE_MS = 1000;
export const BACKOFF_MAX_MS = 5 * 60 * 1000;

/**
 * @param {number} attempt consecutive failures so far, 0 for the first retry
 */
export function backoffDelay(attempt) {
  return Math.min(BACKOFF_BASE_MS * 2 ** Math.min(attempt, 30), BACKOFF_MAX_MS);
}
