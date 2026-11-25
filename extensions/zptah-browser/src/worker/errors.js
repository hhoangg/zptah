// Protocol errors raised by the read path and the reply helpers shared by the handlers.
import { PROTOCOL } from "./config.js";

export class ZptahError extends Error {
  /** @param {string} code @param {string} message */
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

/** @param {string} id @param {unknown} err */
export function errorReply(id, err) {
  const known = err instanceof ZptahError;
  const code = known ? err.code : "E_INVALID";
  const message = String(err instanceof Error ? err.message : err).slice(0, 500);
  return { v: PROTOCOL, type: "res", id, ok: false, error: { code, message } };
}

/** @param {string} id @param {Record<string, unknown>} result */
export function okReply(id, result) {
  return { v: PROTOCOL, type: "res", id, ok: true, result };
}

/**
 * Runs `fn` and answers E_TIMEOUT when `ms` pass first. The work is not cancelled, its late
 * result is dropped.
 * @template T
 * @param {number} ms
 * @param {() => Promise<T>} fn
 * @param {(fn: () => void, ms: number) => unknown} [setTimer]
 * @param {(id: unknown) => void} [clearTimer]
 * @returns {Promise<T>}
 */
export function withDeadline(ms, fn, setTimer = setTimeout, clearTimer = /** @type {any} */ (clearTimeout)) {
  return new Promise((resolve, reject) => {
    const timer = setTimer(() => reject(new ZptahError("E_TIMEOUT", "the browser did not answer in time")), ms);
    fn().then(
      (v) => {
        clearTimer(timer);
        resolve(v);
      },
      (e) => {
        clearTimer(timer);
        reject(e);
      },
    );
  });
}
