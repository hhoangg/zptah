// Text normalization for the risk lexicon. A port of
// crates/monocode-core/src/mono/browser/risk/normalize.rs: the same input must give the same
// words, forms and boundaries (tested against shared/lexicon-cases.json).
//
// Two forms are kept for every word: `acc` (lowercase, tone placement unified, diacritics kept)
// and `plain` (every combining mark removed, `đ` is `d`). Punctuation is a hard boundary (a new
// segment), except a hyphen, underscore or apostrophe between two letters or digits.

/** @typedef {{ acc: string, plain: string }} Word */
/** @typedef {{ segments: Word[][], hasDiacritics: boolean }} Norm */

const TONE_MARKS = new Set(["̀", "́", "̃", "̉", "̣"]);
// Marks that occur in Vietnamese: the tones plus circumflex, breve and horn.
const VI_MARKS = new Set(["̀", "́", "̂", "̃", "̆", "̉", "̛", "̣"]);
const MARK = /^\p{M}$/u;
const WORD = /^[\p{Alphabetic}\p{N}]$/u;
const SPACE = /^\p{White_Space}$/u;

/** @typedef {{ base: string, marks: string[] }} Cluster */

/** @param {string} text @returns {Cluster[]} */
function clusters(text) {
  /** @type {Cluster[]} */
  const out = [];
  for (const ch of text.toLowerCase().normalize("NFD")) {
    const last = out[out.length - 1];
    if (MARK.test(ch) && last) last.marks.push(ch);
    else out.push({ base: ch, marks: [] });
  }
  return out;
}

/** @param {Cluster} c */
const toneOf = (c) => c.marks.find((m) => TONE_MARKS.has(m));

/**
 * Puts the tone of `oa`, `oe` and `uy` on the first vowel, so both spelling styles agree.
 * `qu` + `y` keeps its tone: there the `u` is part of the consonant.
 * @param {Cluster[]} cs
 */
function unifyTonePlacement(cs) {
  for (let i = 1; i < cs.length; i += 1) {
    const afterQ = i >= 2 && cs[i - 2].base === "q";
    const first = cs[i - 1];
    const second = cs[i];
    const pair = (first.base === "o" && (second.base === "a" || second.base === "e")) || (first.base === "u" && second.base === "y");
    if (!pair || first.marks.length > 0) continue;
    const tone = toneOf(second);
    if (tone === undefined) continue;
    if (second.marks.length !== 1) continue;
    if (first.base === "u" && afterQ) continue;
    second.marks = [];
    first.marks.push(tone);
  }
}

/** @param {Cluster} c */
const compose = (c) => (c.base + c.marks.join("")).normalize("NFC");
/** @param {Cluster} c */
const plainOf = (c) => (c.base === "đ" ? "d" : c.base);
/** @param {Cluster | undefined} c */
const isWordChar = (c) => c !== undefined && WORD.test(c.base);

/** @param {string} text @returns {Norm} */
export function normalize(text) {
  const cs = clusters(text);
  unifyTonePlacement(cs);
  /** @type {Norm} */
  const norm = { segments: [], hasDiacritics: false };
  /** @type {Word[]} */
  let segment = [];
  let acc = "";
  let plain = "";
  const flushWord = () => {
    if (acc !== "") segment.push({ acc, plain });
    acc = "";
    plain = "";
  };
  const flushSegment = () => {
    if (segment.length > 0) norm.segments.push(segment);
    segment = [];
  };
  for (let i = 0; i < cs.length; i += 1) {
    const c = cs[i];
    if (isWordChar(c)) {
      if (c.base === "đ" || c.marks.some((m) => VI_MARKS.has(m))) norm.hasDiacritics = true;
      acc += compose(c);
      plain += plainOf(c);
    } else if (SPACE.test(c.base)) {
      flushWord();
    } else {
      const joins = i > 0 && i + 1 < cs.length && isWordChar(cs[i - 1]) && isWordChar(cs[i + 1]);
      if (joins && (c.base === "-" || c.base === "_" || c.base === "‐" || c.base === "‑")) {
        flushWord();
      } else if (joins && (c.base === "'" || c.base === "’")) {
        // dropped: "don't" is "dont"
      } else {
        flushWord();
        flushSegment();
      }
    }
  }
  flushWord();
  flushSegment();
  return norm;
}

/** @param {Norm} n */
export const wordCount = (n) => n.segments.reduce((sum, s) => sum + s.length, 0);
/** @param {Norm} n */
export const isEmpty = (n) => n.segments.every((s) => s.length === 0);

/**
 * Words joined with single spaces in the form the text is compared in.
 * @param {Norm} n @param {boolean} plain
 */
export const joined = (n, plain) => n.segments.flat().map((w) => (plain ? w.plain : w.acc)).join(" ");

/** @param {string} text */
export function diacriticShare(text) {
  let total = 0;
  let marked = 0;
  for (const w of normalize(text).segments.flat()) {
    total += 1;
    if (w.acc !== w.plain) marked += 1;
  }
  return total === 0 ? 0 : marked / total;
}

/**
 * Whether a page is Vietnamese: the `lang` attribute when there is one, else the share of words
 * with diacritics in a sample of the page text.
 * @param {string | null | undefined} lang @param {string} sample
 */
export function pageIsVietnamese(lang, sample) {
  const l = (lang ?? "").trim();
  return l === "" ? diacriticShare(sample) >= 0.2 : l.toLowerCase().startsWith("vi");
}
