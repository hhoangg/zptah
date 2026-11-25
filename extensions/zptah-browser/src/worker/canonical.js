// Canonical JSON for frame MACs. Must match canonical_json in
// crates/monocode-core/src/mono/browser/link/auth.rs byte for byte: object keys sorted, no
// whitespace, plain safe integers only, ASCII object keys only, strings as JSON.stringify.

const LONE_SURROGATE = /[\ud800-\udbff](?![\udc00-\udfff])|(?<![\ud800-\udbff])[\udc00-\udfff]/;

export class CanonicalError extends Error {}

/**
 * @param {unknown} value
 * @returns {string}
 */
export function canonicalJson(value) {
  if (value === null) return "null";
  switch (typeof value) {
    case "boolean":
      return value ? "true" : "false";
    case "number":
      // Number.isSafeInteger refuses floats, NaN, Infinity and anything above 2^53 - 1.
      if (!Number.isSafeInteger(value)) throw new CanonicalError("numbers must be safe integers");
      return String(value === 0 ? 0 : value);
    case "string":
      if (LONE_SURROGATE.test(value)) throw new CanonicalError("string has a lone surrogate");
      return JSON.stringify(value);
    case "object": {
      if (Array.isArray(value)) return `[${value.map(canonicalJson).join(",")}]`;
      const proto = Object.getPrototypeOf(value);
      if (proto !== Object.prototype && proto !== null) throw new CanonicalError("not a plain object");
      const record = /** @type {Record<string, unknown>} */ (value);
      const keys = Object.keys(record).sort();
      const parts = keys.map((k) => {
        if (!/^[\x00-\x7f]*$/.test(k)) throw new CanonicalError("object keys must be ASCII");
        return `${JSON.stringify(k)}:${canonicalJson(record[k])}`;
      });
      return `{${parts.join(",")}}`;
    }
    default:
      throw new CanonicalError(`cannot encode ${typeof value}`);
  }
}

/**
 * Throws unless every number literal in the JSON text is a plain integer (`1.0`, `1e2` and
 * `-1e19` are refused even though JSON.parse would accept them).
 * @param {string} text
 */
export function assertIntegerLiterals(text) {
  let i = 0;
  while (i < text.length) {
    const c = text[i];
    if (c === '"') {
      i += 1;
      while (i < text.length && text[i] !== '"') i += text[i] === "\\" ? 2 : 1;
      i += 1;
    } else if (c === "-" || (c >= "0" && c <= "9")) {
      let j = i + 1;
      while (j < text.length && /[0-9.eE+-]/.test(text[j])) j += 1;
      if (!/^-?(?:0|[1-9][0-9]*)$/.test(text.slice(i, j))) {
        throw new CanonicalError("numbers must be plain integers");
      }
      i = j;
    } else {
      i += 1;
    }
  }
}

/**
 * Canonical form of JSON text, applying the integer-literal rule first.
 * @param {string} text
 * @returns {string}
 */
export function canonicalizeText(text) {
  assertIntegerLiterals(text);
  return canonicalJson(JSON.parse(text));
}
