// WebCrypto helpers. HMAC-SHA256 and HKDF-SHA256 only; verification is constant time because
// it goes through subtle.verify.
import { concatBytes } from "./bytes.js";

const subtle = globalThis.crypto.subtle;
const HMAC = { name: "HMAC", hash: "SHA-256" };

/** @param {Uint8Array} bytes @param {KeyUsage[]} usages */
const importHmac = (bytes, usages) => subtle.importKey("raw", bytes, HMAC, false, usages);

/**
 * @param {Uint8Array} key
 * @param {...Uint8Array} parts
 * @returns {Promise<Uint8Array>}
 */
export async function hmac(key, ...parts) {
  const k = await importHmac(key, ["sign"]);
  return new Uint8Array(await subtle.sign("HMAC", k, concatBytes(...parts)));
}

/**
 * @param {Uint8Array} key
 * @param {Uint8Array} mac
 * @param {...Uint8Array} parts
 * @returns {Promise<boolean>}
 */
export async function hmacVerify(key, mac, ...parts) {
  const k = await importHmac(key, ["verify"]);
  return subtle.verify("HMAC", k, mac, concatBytes(...parts));
}

/**
 * HKDF-SHA256 with `length` bytes of output.
 * @param {Uint8Array} ikm
 * @param {Uint8Array} salt
 * @param {Uint8Array} info
 * @param {number} length
 * @returns {Promise<Uint8Array>}
 */
export async function hkdf(ikm, salt, info, length) {
  const k = await subtle.importKey("raw", ikm, "HKDF", false, ["deriveBits"]);
  const bits = await subtle.deriveBits({ name: "HKDF", hash: "SHA-256", salt, info }, k, length * 8);
  return new Uint8Array(bits);
}

/** @param {number} n */
export function randomBytes(n) {
  return globalThis.crypto.getRandomValues(new Uint8Array(n));
}
