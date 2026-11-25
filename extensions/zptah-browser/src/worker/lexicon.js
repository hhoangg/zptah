// The risk lexicon and the control-name classifier. A port of
// crates/monocode-core/src/mono/browser/risk/lexicon.rs over the same shared/risk-lexicon.json,
// so the extension's own check (spec 6.2 step 4) and the gate match the same words. The shared
// cases in shared/lexicon-cases.json run against both.
import { isEmpty, normalize, wordCount } from "./normalize.js";
import { extractAmounts } from "./signals.js";

const CATEGORY_CLASSES = new Set(["payment", "destructive", "account_security", "sharing", "terms"]);
const FIELD_KINDS = ["password", "otp", "card_number", "card_cvc", "card_expiry", "bank_account", "amount", "national_id", "email", "file", "terms_checkbox", "other"];
const FIELD_PRIORITY = ["password", "otp", "card_number", "card_cvc", "card_expiry", "bank_account", "national_id", "amount"];

/** @typedef {import("./normalize.js").Norm} Norm */
/** @typedef {import("./normalize.js").Word} Word */
/** @typedef {{ phrase: string, cls: string, words: Word[], maxWords: number, prefix: boolean, ctx: boolean, hasDiacritics: boolean }} Entry */
/** @typedef {{ seg: number, start: number, len: number, entry: number }} Hit */
/** @typedef {{ categories: Set<string>, send: boolean, post: boolean, dismiss: boolean, notRisky: boolean, consent: boolean, informational: boolean }} Classification */
/** @typedef {{ name: string, kind?: string, pageVi?: boolean, inDialog?: boolean, dialogText?: string, dialogHasPrimary?: boolean, dialogPrimaryNames?: string[], dialogMarkers?: string, role?: string, nearbyText?: string, pageHeading?: string, pageTitle?: string, link?: { href: string, pageUrl: string, inNavFooterMenu: boolean } | null }} ControlInput */

/** @param {string} cls */
const isFieldClass = (cls) => cls.startsWith("field_");

/** @returns {Classification} */
const emptyClass = () => ({ categories: new Set(), send: false, post: false, dismiss: false, notRisky: false, consent: false, informational: false });

/**
 * @param {Norm} norm @param {Norm} phrase
 */
function startsWithPhrase(norm, phrase) {
  const first = norm.segments[0];
  const pw = phrase.segments[0];
  if (!first || !pw) return false;
  const plain = first.every((w) => w.acc === w.plain);
  return first.length >= pw.length && pw.every((a, i) => (plain ? a.plain === first[i].plain : a.acc === first[i].acc));
}

/**
 * Whether any needle occurs in the haystack as a whole-word sequence in either form.
 * @param {Norm} hay @param {Norm[]} needles
 */
function containsAny(hay, needles) {
  return needles.some((n) => {
    const nw = n.segments[0];
    if (!nw) return false;
    return hay.segments.some((seg) => {
      for (let i = 0; i + nw.length <= seg.length; i += 1) {
        const win = seg.slice(i, i + nw.length);
        const acc = win.every((w, k) => w.acc === nw[k].acc);
        const plain = seg.every((w) => w.acc === w.plain) && win.every((w, k) => w.plain === nw[k].plain);
        if (acc || plain) return true;
      }
      return false;
    });
  });
}

export class Lexicon {
  /** @param {any} raw the parsed risk-lexicon.json @param {any} sites the parsed risk-sites.json */
  constructor(raw, sites) {
    this.sites = sites;
    this.window = raw.button_window_words;
    this.infoMax = raw.informational_max_words;
    /** @type {Entry[]} */
    this.entries = [];
    const seen = new Set();
    for (const e of raw.entries) {
      if (!(CATEGORY_CLASSES.has(e.class) || ["send", "post", "consent", "context", "dismiss", "not_risky", "field_card_cvc_weak"].includes(e.class) || (isFieldClass(e.class) && FIELD_KINDS.includes(e.class.slice(6))))) {
        throw new Error(`unknown class ${e.class}`);
      }
      const norm = normalize(e.phrase);
      if (norm.segments.length !== 1) throw new Error(`phrase ${e.phrase} must be one run of words`);
      const words = norm.segments[0];
      const key = `${words.map((w) => w.acc).join(" ")}\u0000${e.class}`;
      if (seen.has(key)) continue;
      seen.add(key);
      this.entries.push({ phrase: e.phrase, cls: e.class, words, maxWords: e.max_words ?? raw.short_word_max_words, prefix: e.prefix === true, ctx: e.ctx === true, hasDiacritics: norm.hasDiacritics });
    }
    /** @param {string[]} list */
    const norms = (list) => list.map((s) => normalize(s));
    this.signupPrefixes = norms(raw.signup_prefixes);
    this.periodWords = norms(raw.period_words);
    this.ambiguous = new Set(raw.ambiguous_stripped);
    this.infoPrefixes = norms(raw.informational_prefixes);
    this.consentWords = norms(raw.consent_container_words);
    this.consentManagers = raw.consent_managers.map((/** @type {string} */ s) => s.toLowerCase());
    this.publicWords = norms(raw.public_visibility_words);
    this.visibilityWords = norms(raw.visibility_control_words);
  }

  /** @param {Entry} e */
  #wholeNameOnly(e) {
    return (e.cls === "not_risky" || e.cls === "dismiss") && !e.prefix;
  }

  /**
   * @param {Norm} norm @param {boolean} pageVi @param {"name" | "context" | "anywhere" | "field"} mode
   * @returns {Hit[]}
   */
  #find(norm, pageVi, mode) {
    const total = wordCount(norm);
    /** @type {Hit[]} */
    const hits = [];
    norm.segments.forEach((seg, si) => {
      // A segment without Vietnamese diacritics is compared stripped, even when another segment
      // of the same name has them.
      const plain = seg.every((w) => w.acc === w.plain);
      for (let start = 0; start < seg.length; start += 1) {
        this.entries.forEach((e, ei) => {
          if ((mode === "field") !== isFieldClass(e.cls)) return;
          const end = start + e.words.length;
          if (end > seg.length) return;
          if (!e.words.every((ew, k) => (plain ? ew.plain === seg[start + k].plain : ew.acc === seg[start + k].acc))) return;
          const len = e.words.length;
          if (plain && len === 1 && e.hasDiacritics && !pageVi && this.ambiguous.has(e.words[0].plain)) return;
          const whole = norm.segments.length === 1 && start === 0 && len === total;
          let allowed;
          if (mode === "field" || mode === "anywhere") allowed = true;
          else if (this.#wholeNameOnly(e)) allowed = whole && mode === "name";
          else if (e.prefix) allowed = si === 0 && start === 0;
          else if (len === 1) allowed = mode === "context" ? start === 0 && seg.length <= e.maxWords : whole || (si === 0 && start === 0 && total <= e.maxWords);
          else allowed = true;
          if (allowed) hits.push({ seg: si, start, len, entry: ei });
        });
      }
    });
    return hits;
  }

  /**
   * Longest phrase wins: a hit that overlaps a longer one is dropped. Hits on the exact same
   * span stay together.
   * @param {Hit[]} hits
   */
  #longest(hits) {
    const sorted = [...hits].sort((a, b) => b.len - a.len || a.seg - b.seg || a.start - b.start);
    /** @type {Hit[]} */
    const chosen = [];
    for (const h of sorted) {
      const clash = chosen.some((c) => c.seg === h.seg && h.start < c.start + c.len && c.start < h.start + h.len && !(c.start === h.start && c.len === h.len));
      if (!clash) chosen.push(h);
    }
    return chosen;
  }

  /** @param {Norm} norm @returns {"long" | "prefix" | null} */
  #informationalReason(norm) {
    if (wordCount(norm) > this.infoMax) return "long";
    return this.infoPrefixes.some((p) => startsWithPhrase(norm, p)) ? "prefix" : null;
  }

  /**
   * @param {Norm} norm @param {string} kind @param {boolean} pageVi
   * @returns {{ out: Classification, context: boolean, ctxDismiss: boolean }}
   */
  #classifyNorm(norm, kind, pageVi) {
    const out = emptyClass();
    if (isEmpty(norm)) return { out, context: false, ctxDismiss: false };
    let hits = this.#find(norm, pageVi, "name");
    if (kind === "link" || kind === "text") {
      const reason = this.#informationalReason(norm);
      if (reason) {
        // A long or policy-like link only feeds nearby text, but a multi-word destructive or
        // account-security phrase near its start still counts (and a payment phrase in a long
        // name or after a colon or dash).
        const kept = this.#longest(hits).filter((h) => {
          const e = this.entries[h.entry];
          const paymentOk = reason === "long" || h.seg > 0;
          return h.start < this.window && e.words.length >= 2 && (e.cls === "destructive" || e.cls === "account_security" || (e.cls === "payment" && paymentOk));
        });
        if (kept.length === 0) {
          out.informational = true;
          return { out, context: false, ctxDismiss: false };
        }
        hits = kept;
      }
    }
    if (kind === "button" || kind === "submit") hits = hits.filter((h) => h.start < this.window);
    let context = false;
    let ctxDismiss = false;
    for (const h of this.#longest(hits)) {
      const e = this.entries[h.entry];
      if (CATEGORY_CLASSES.has(e.cls)) out.categories.add(e.cls);
      else if (e.cls === "send") out.send = true;
      else if (e.cls === "post") out.post = true;
      else if (e.cls === "consent") out.consent = true;
      else if (e.cls === "context") context = true;
      else if (e.cls === "dismiss") {
        out.dismiss = true;
        ctxDismiss ||= e.ctx;
      } else if (e.cls === "not_risky") out.notRisky = true;
    }
    return { out, context, ctxDismiss };
  }

  /** @param {string} name @param {string} kind @param {boolean} pageVi */
  classifyName(name, kind, pageVi) {
    return this.#classifyNorm(normalize(name), kind, pageVi).out;
  }

  /**
   * The categories a text implies (page heading, nearby text, dialog text).
   * @param {string} text @param {boolean} pageVi @param {boolean} anywhere
   * @returns {Set<string>}
   */
  textCategories(text, pageVi, anywhere) {
    const norm = normalize(text);
    const out = new Set();
    for (const h of this.#longest(this.#find(norm, pageVi, anywhere ? "anywhere" : "context"))) {
      const e = this.entries[h.entry];
      if (!CATEGORY_CLASSES.has(e.cls)) continue;
      if (anywhere && e.words.length === 1 && e.cls !== "destructive") continue;
      out.add(e.cls);
    }
    return out;
  }

  /** @param {ControlInput} input */
  #contextCategories(input) {
    /** @type {Set<string>} */
    const out = new Set();
    const pageVi = input.pageVi === true;
    if (input.inDialog) for (const c of this.textCategories(input.dialogText ?? "", pageVi, true)) out.add(c);
    for (const t of [input.pageHeading ?? "", input.nearbyText ?? "", input.pageTitle ?? ""]) {
      for (const c of this.textCategories(t, pageVi, false)) out.add(c);
    }
    return out;
  }

  /** Classifies a control: its name, the dialog it is in and the page around it. @param {ControlInput} input @returns {Classification} */
  classifyControl(input) {
    const kind = input.kind ?? "button";
    const pageVi = input.pageVi === true;
    const norm = normalize(input.name);
    const { out, context, ctxDismiss } = this.#classifyNorm(norm, kind, pageVi);
    if (out.informational) return out;
    if (out.dismiss) {
      if (ctxDismiss && !(input.inDialog && input.dialogHasPrimary)) {
        // A bare Cancel outside a dialog (or in one with no primary button) takes the category
        // of the page.
        out.dismiss = false;
        for (const c of this.#contextCategories(input)) out.categories.add(c);
      } else if (ctxDismiss && this.#dialogCancelIsTheRiskyOne(input)) {
        out.dismiss = false;
        out.categories.add("destructive");
        return out;
      } else {
        // Dismissing is never risky, whatever the dialog says.
        out.categories.clear();
        return out;
      }
    }
    if (context) for (const c of this.#contextCategories(input)) out.categories.add(c);
    if (out.notRisky && this.#signupWithPrice(input, norm)) {
      // A sign-up with a price or a billing period creates a paid subscription.
      out.notRisky = false;
      out.categories.add("payment");
    }
    if (input.inDialog) {
      // A consent-looking name skips the dialog text only inside a confirmed cookie banner. Any
      // payment, account, sharing or terms signal in the dialog text disqualifies it.
      const inBanner = out.consent && this.looksLikeConsentContainer(input.dialogText ?? "", input.dialogMarkers ?? "") && this.textCategories(input.dialogText ?? "", pageVi, true).size === 0;
      if (!inBanner) {
        out.consent = false;
        for (const c of this.textCategories(input.dialogText ?? "", pageVi, true)) out.categories.add(c);
      }
    }
    if (kind === "link" && this.#linkExempt(input, out)) {
      out.categories.clear();
      out.send = false;
      out.post = false;
    }
    return out;
  }

  /** @param {ControlInput} input */
  #dialogCancelIsTheRiskyOne(input) {
    const names = input.dialogPrimaryNames ?? [];
    const pageVi = input.pageVi === true;
    if (names.length === 0 || !this.textCategories(input.dialogText ?? "", pageVi, true).has("destructive")) return false;
    // A primary button carries the meaning when its own name is destructive or is a bare confirm
    // word (Yes, OK, Confirm) that takes the dialog's meaning.
    return !names.some((name) => {
      const r = this.#classifyNorm(normalize(name), "button", pageVi);
      return r.context || r.out.categories.has("destructive");
    });
  }

  /** @param {ControlInput} input @param {Norm} name */
  #signupWithPrice(input, name) {
    if (!this.signupPrefixes.some((p) => startsWithPhrase(name, p))) return false;
    const text = `${input.name} ${input.nearbyText ?? ""}`;
    return extractAmounts(text).length > 0 || containsAny(normalize(text), this.periodWords);
  }

  /** The footer and menu link exemption (spec 7.3, F8). @param {ControlInput} input @param {Classification} cls */
  #linkExempt(input, cls) {
    const link = input.link;
    if (!link) return false;
    const href = link.href.trim();
    if (!link.inNavFooterMenu || href === "" || href.startsWith("#") || (input.role ?? "").toLowerCase() === "button" || input.inDialog || cls.categories.has("destructive") || cls.categories.has("account_security")) return false;
    /** @type {URL} */
    let page;
    /** @type {URL} */
    let target;
    try {
      page = new URL(link.pageUrl);
      target = new URL(link.href, page);
    } catch {
      return false;
    }
    // Rust's Url::query() is Some for a bare `?`; JS search is empty then.
    if (target.origin !== page.origin || target.href.split("#")[0].includes("?")) return false;
    const segs = target.pathname.split("/").slice(1).map((s) => s.toLowerCase());
    const bad = new Set([...this.sites.checkout_segments, ...this.sites.action_segments]);
    return !segs.some((s) => bad.has(s));
  }

  /**
   * The field kind named by label-like text (name, id, label, placeholder), if any.
   * @param {string} text @returns {string | null}
   */
  fieldKindOf(text) {
    const hits = this.#longest(this.#find(normalize(text), true, "field"));
    const classes = hits.map((h) => this.entries[h.entry].cls);
    for (const kind of FIELD_PRIORITY) if (classes.includes(`field_${kind}`)) return kind;
    return classes.includes("field_card_cvc_weak") ? "card_cvc" : null;
  }

  /** Whether `text` classifies as the given risk category (used for checkbox labels). @param {string} text @param {boolean} pageVi @param {string} wanted */
  textHasPhraseClass(text, pageVi, wanted) {
    return this.classifyName(text, "text", pageVi).categories.has(wanted) || this.textCategories(text, pageVi, false).has(wanted);
  }

  /** @param {string} text */
  isPublicVisibility(text) {
    return containsAny(normalize(text), this.publicWords);
  }

  /** @param {string} text */
  isVisibilityControl(text) {
    return containsAny(normalize(text), this.visibilityWords);
  }

  /** @param {string} text @param {string} markers */
  looksLikeConsentContainer(text, markers) {
    const m = markers.toLowerCase();
    return this.consentManagers.some((/** @type {string} */ c) => m.includes(c)) || containsAny(normalize(text), this.consentWords);
  }
}
