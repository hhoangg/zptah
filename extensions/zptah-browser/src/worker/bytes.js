// Standard base64 with padding, the encoding of every binary value on the wire.

/** @param {Uint8Array} bytes */
export function b64encode(bytes) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}

/**
 * Strict decode: only canonical padded base64 of exactly `length` bytes is accepted.
 * @param {unknown} text
 * @param {number} length
 * @returns {Uint8Array}
 */
export function b64decodeFixed(text, length) {
  if (typeof text !== "string") throw new TypeError("expected a base64 string");
  let bin;
  try {
    bin = atob(text);
  } catch {
    throw new TypeError("not base64");
  }
  const out = Uint8Array.from(bin, (c) => c.charCodeAt(0));
  if (out.length !== length || b64encode(out) !== text) throw new TypeError(`not ${length} bytes of base64`);
  return out;
}

/** @param {...Uint8Array} parts */
export function concatBytes(...parts) {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

const enc = new TextEncoder();
/** @param {string} s */
export const utf8 = (s) => enc.encode(s);

/** @param {Uint8Array} bytes */
export function toHex(bytes) {
  return [...bytes].map((b) => b.toString(16).padStart(2, "0")).join("");
}
