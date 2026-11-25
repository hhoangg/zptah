// Signals for the risk classifier (spec 7.2): money patterns, form field kinds and the
// extension's own `ext_risk` (spec 6.2 step 4). Ports of extract_amounts, classify_field and the
// element-level part of gate_categories in crates/monocode-core/src/mono/browser/risk/signals.rs.
// ext_risk leaves out what only the app knows: whether a recipient was named (`send_unnamed`),
// the task's domains (`unrelated_domain`) and site grants (`sensitive_site`).
import { canonicalJson } from "./canonical.js";

const AMOUNT = /[$€£₫]\s?\p{Nd}[\p{Nd}.,]*|\p{Nd}[\p{Nd}.,]*\s?(?:₫|[$€£]|đ|vnd|vnđ|usd|eur|gbp)/giu;
const ALNUM = /^[\p{Alphabetic}\p{N}]$/u;
const LETTER = /^\p{Alphabetic}$/u;

/**
 * Money patterns in a text (`₫189,000`, `500.000đ`, `$20`, `12 USD`).
 * @param {string} text
 * @returns {string[]}
 */
export function extractAmounts(text) {
  /** @type {string[]} */
  const out = [];
  for (const m of text.matchAll(AMOUNT)) {
    const matched = m[0];
    const lastChar = [...matched].pop() ?? "";
    const next = [...text.slice((m.index ?? 0) + matched.length)][0];
    if (LETTER.test(lastChar) && next !== undefined && ALNUM.test(next)) continue;
    out.push(matched.trim());
  }
  return out;
}

/**
 * @typedef {object} FieldAttrs
 * @property {string} [type] @property {string} [autocomplete] @property {string} [name] @property {string} [id]
 * @property {string} [label] @property {string} [placeholder] @property {string} [inputmode]
 * @property {number | null} [maxlength] @property {string} [nearby_text]
 */

/**
 * The kind of a form field from its attributes. Values are never read.
 * @param {import("./lexicon.js").Lexicon} lex @param {FieldAttrs} a
 * @returns {string}
 */
export function classifyField(lex, a) {
  const ty = (a.type ?? "").toLowerCase();
  const acTokens = (a.autocomplete ?? "").toLowerCase().split(/\s+/).filter(Boolean);
  const acHas = (/** @type {string} */ t) => acTokens.some((x) => x === t || x.startsWith(t));
  if (acHas("one-time-code")) return "otp";
  if (acHas("current-password") || acHas("new-password") || ty === "password") return "password";
  if (acHas("cc-number")) return "card_number";
  if (acHas("cc-csc")) return "card_cvc";
  if (acHas("cc-exp")) return "card_expiry";
  if (ty === "file") return "file";
  const found = new Set([a.name, a.id, a.label, a.placeholder].map((t) => lex.fieldKindOf(t ?? "")).filter((k) => k !== null));
  for (const kind of ["password", "otp", "card_number", "card_cvc", "card_expiry", "bank_account", "national_id", "amount"]) {
    if (found.has(kind)) return kind;
  }
  const numericShort = (a.inputmode ?? "").toLowerCase() === "numeric" && typeof a.maxlength === "number" && a.maxlength >= 4 && a.maxlength <= 8;
  if (numericShort && lex.fieldKindOf(a.nearby_text ?? "") === "otp") return "otp";
  if (ty === "checkbox" && [a.label, a.name].some((t) => lex.textHasPhraseClass(t ?? "", false, "terms"))) return "terms_checkbox";
  if (ty === "email" || acHas("email")) return "email";
  return "other";
}

/** @param {string} url */
function parseUrl(url) {
  try {
    return new URL(url);
  } catch {
    return null;
  }
}

/** Whether a URL path has a checkout segment. @param {any} sites @param {string} url */
export function isCheckoutPath(sites, url) {
  const u = parseUrl(url);
  if (!u) return false;
  const segs = u.pathname.split("/").slice(1).map((s) => s.toLowerCase());
  return segs.some((s) => sites.checkout_segments.includes(s));
}

/** Whether a host is a listed sensitive site (a domain covers its subdomains). @param {any} sites @param {string} url */
export function isSensitiveSite(sites, url) {
  const u = parseUrl(url);
  if (!u) return false;
  const host = u.hostname.replace(/\.$/, "").toLowerCase();
  return Object.values(sites.categories).some((domains) => /** @type {string[]} */ (domains).some((d) => host === d || host.endsWith(`.${d}`)));
}

/**
 * @typedef {object} PageContext
 * @property {string} url @property {string} title @property {string} heading @property {boolean} vi
 */

/**
 * @param {import("./lexicon.js").Lexicon} lex @param {any} el ElementSignals @param {string} name
 * @param {PageContext} page @returns {import("./lexicon.js").ControlInput}
 */
function controlInput(lex, el, name, page) {
  const isLink = typeof el.href === "string" && el.href.trim() !== "" && !el.is_submit && el.role.toLowerCase() !== "button";
  return {
    name,
    kind: isLink ? "link" : el.is_submit ? "submit" : "button",
    pageVi: page.vi,
    inDialog: el.in_dialog,
    dialogText: el.dialog_text ?? "",
    dialogHasPrimary: el.dialog_has_primary,
    dialogPrimaryNames: el.dialog_primary_names,
    dialogMarkers: el.dialog_markers ?? "",
    role: el.role,
    nearbyText: el.nearby_text,
    pageHeading: page.heading,
    pageTitle: page.title,
    link: typeof el.href === "string" ? { href: el.href, pageUrl: page.url, inNavFooterMenu: el.in_landmark_menu } : null,
  };
}

/**
 * Classifies the accessible name and the visible text separately (an aria-label can say
 * something harmless over a button that shows "Pay now") and merges the results.
 * @param {import("./lexicon.js").Lexicon} lex @param {any} el @param {PageContext} page
 */
function classifyElement(lex, el, page) {
  const first = el.name.trim() === "" ? el.text : el.name;
  const input = controlInput(lex, el, first, page);
  const cls = lex.classifyControl(input);
  const text = el.text.trim();
  if (el.name.trim() !== "" && text !== "" && text !== el.name.trim()) {
    const second = lex.classifyControl(controlInput(lex, el, text, page));
    for (const c of second.categories) cls.categories.add(c);
    cls.send ||= second.send;
    cls.post ||= second.post;
    cls.consent &&= second.consent;
    cls.dismiss &&= second.dismiss;
    cls.notRisky &&= second.notRisky;
    cls.informational &&= second.informational;
  }
  return { input, cls };
}

const CARD_KINDS = new Set(["card_number", "card_cvc", "card_expiry"]);

/**
 * The risk categories the extension can derive from the page alone.
 * @param {import("./lexicon.js").Lexicon} lex
 * @param {{ kind: "click" | "type" | "select" | "upload" | "other", fieldKind?: string }} action
 * @param {any} el ElementSignals @param {any} form FormSignals @param {PageContext} page
 * @returns {string[]} sorted, without duplicates
 */
export function extRisk(lex, action, el, form, page) {
  /** @type {Set<string>} */
  const cats = new Set();
  const onCheckout = isCheckoutPath(lex.sites, page.url);
  const sensitive = isSensitiveSite(lex.sites, page.url);
  if (action.kind === "type") {
    const k = action.fieldKind ?? "other";
    if (k === "otp" || k === "national_id" || CARD_KINDS.has(k)) cats.add("credential_entry");
  } else if (action.kind === "upload") {
    cats.add("upload");
  } else if (action.kind === "click" || action.kind === "select") {
    const { input, cls } = classifyElement(lex, el, page);
    for (const c of cls.categories) cats.add(c);
    const submitLike = el.is_submit || input.kind === "button";
    const hasMoneyField = form.fields.some((/** @type {{ kind: string }} */ f) => CARD_KINDS.has(f.kind) || f.kind === "bank_account" || f.kind === "amount");
    if ((el.is_submit && hasMoneyField) || (submitLike && el.amounts.length > 0 && onCheckout)) cats.add("payment");
    const label = el.label ?? el.name;
    if (el.type === "checkbox" && lex.textHasPhraseClass(label, page.vi, "terms")) cats.add("terms");
    const isPublic = typeof form.visibility === "string" && lex.isPublicVisibility(form.visibility);
    if (cls.post || (isPublic && (el.is_submit || cls.send))) cats.add("post_public");
    const changesControl = action.kind === "select" || ["combobox", "radio", "switch", "menuitemradio", "option", "listbox"].includes(el.role);
    if (changesControl && [el.name, el.label ?? ""].some((t) => lex.isVisibilityControl(t))) cats.add("sharing");
    if (el.name.trim() === "" && el.text.trim() === "" && (sensitive || onCheckout) && !cls.consent) cats.add("payment");
  }
  return [...cats].sort();
}

/**
 * SHA-256 of the risk-relevant signals (hex). Strings are made well formed first because the
 * canonical form refuses lone surrogates.
 * @param {unknown} value
 */
export async function digestOf(value) {
  const text = canonicalJson(value);
  const bytes = new TextEncoder().encode(text);
  const hash = new Uint8Array(await globalThis.crypto.subtle.digest("SHA-256", bytes));
  return [...hash].map((b) => b.toString(16).padStart(2, "0")).join("");
}
