// Byte-offset paging of text results (UTF-8 bytes, cut on line boundaries).
const enc = new TextEncoder();
const dec = new TextDecoder();

/**
 * @param {string} text the full text
 * @param {number} from byte offset into the UTF-8 form of `text`
 * @param {number} maxBytes
 * @returns {{ text: string, total_bytes: number, truncated: boolean, next_from?: number }}
 */
export function pageText(text, from, maxBytes) {
  const bytes = enc.encode(text);
  const total = bytes.length;
  let start = Math.min(from, total);
  while (start < total && (bytes[start] & 0xc0) === 0x80) start += 1;
  let end = Math.min(start + maxBytes, total);
  if (end < total) {
    let cut = -1;
    for (let i = end - 1; i >= start; i -= 1) {
      if (bytes[i] === 0x0a) {
        cut = i + 1;
        break;
      }
    }
    if (cut > start) end = cut;
    else while (end > start && (bytes[end] & 0xc0) === 0x80) end -= 1;
    // A first character wider than the budget cannot be cut: take it whole.
    if (end === start && start < total) {
      end = start + 1;
      while (end < total && (bytes[end] & 0xc0) === 0x80) end += 1;
    }
  }
  const truncated = end < total;
  const out = { text: dec.decode(bytes.subarray(start, end)), total_bytes: total, truncated };
  return truncated ? { ...out, next_from: end } : out;
}
