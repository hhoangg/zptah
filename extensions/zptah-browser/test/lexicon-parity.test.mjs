// @ts-nocheck - cases come from JSON.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { Lexicon } from "../src/worker/lexicon.js";
import { diacriticShare, joined, normalize, pageIsVietnamese } from "../src/worker/normalize.js";
import { classifyField, extractAmounts } from "../src/worker/signals.js";

const read = (name) => JSON.parse(readFileSync(new URL(`../shared/${name}`, import.meta.url), "utf8"));
const lex = new Lexicon(read("risk-lexicon.json"), read("risk-sites.json"));
const cases = read("lexicon-cases.json").cases;

test("normalization folds case, tone placement and marks like normalize.rs", () => {
  const a = normalize("THANH TOÁN");
  assert.equal(joined(a, false), joined(normalize("thanh-toán"), false));
  assert.equal(joined(a, true), "thanh toan");
  assert.equal(joined(normalize("Thanh toan"), true), "thanh toan");
  assert.ok(a.hasDiacritics && !normalize("Thanh toan").hasDiacritics);
  assert.equal(joined(normalize("hoá"), false), joined(normalize("hóa"), false));
  assert.equal(joined(normalize("thuỷ"), false), joined(normalize("thủy"), false));
  assert.equal(joined(normalize("hoà"), false), joined(normalize("hòa"), false));
  assert.equal(joined(normalize("quý"), false), "quý");
  assert.ok(normalize("Đăng").hasDiacritics);
  assert.equal(joined(normalize("Đăng"), true), "dang");
});

test("punctuation is a hard boundary but a hyphen between letters is not", () => {
  assert.equal(normalize("Sim số - Thẻ cào").segments.length, 2);
  assert.equal(normalize("check-out").segments.length, 1);
  assert.equal(normalize("check-out").segments[0].length, 2);
  assert.equal(joined(normalize("don't"), false), "dont");
  assert.equal(normalize("Mua, ngay").segments.length, 2);
  assert.equal(normalize("   ").segments.length, 0);
});

test("page language uses lang first, then the diacritic share", () => {
  assert.equal(pageIsVietnamese("vi-VN", "hello"), true);
  assert.equal(pageIsVietnamese("en", "Thanh toán đơn hàng"), false);
  assert.equal(pageIsVietnamese(null, "Thanh toán đơn hàng của bạn"), true);
  assert.equal(pageIsVietnamese("", "Pay for your order now"), false);
  assert.equal(diacriticShare("Thanh toán"), 0.5);
});

test("every shared lexicon case gives the same answer as the Rust classifier", () => {
  assert.ok(cases.length > 500);
  const failures = [];
  for (const c of cases) {
    const got = lex.classifyControl({
      name: c.name,
      kind: c.kind,
      pageVi: c.page_vi,
      inDialog: c.in_dialog,
      dialogText: c.dialog_text,
      dialogHasPrimary: c.dialog_has_primary,
      dialogPrimaryNames: c.dialog_primary_names,
      dialogMarkers: c.dialog_markers,
      role: c.role,
      nearbyText: c.nearby_text,
      pageHeading: c.page_heading,
      pageTitle: c.page_title,
      link: c.link ? { href: c.link.href, pageUrl: c.link.page_url, inNavFooterMenu: c.link.in_nav_footer_menu } : null,
    });
    const want = c.expect;
    const ok =
      JSON.stringify([...got.categories].sort()) === JSON.stringify([...(want.categories ?? [])].sort()) &&
      got.send === (want.send ?? false) &&
      got.post === (want.post ?? false) &&
      got.dismiss === (want.dismiss ?? false) &&
      got.notRisky === (want.not_risky ?? false) &&
      got.consent === (want.consent ?? false) &&
      got.informational === (want.informational ?? false);
    if (!ok) failures.push(`${JSON.stringify(c.name)} (${c.kind ?? "button"}) -> ${JSON.stringify({ ...got, categories: [...got.categories] })}`);
  }
  assert.deepEqual(failures, []);
});

test("amounts and field kinds follow signals.rs", () => {
  assert.deepEqual(extractAmounts("Total ₫189,000 and 500.000đ, $20 or 12 USD"), ["₫189,000", "500.000đ", "$20", "12 USD"]);
  assert.deepEqual(extractAmounts("500 dollars and 5 users"), []);
  assert.equal(classifyField(lex, { type: "password" }), "password");
  assert.equal(classifyField(lex, { type: "text", autocomplete: "one-time-code" }), "otp");
  assert.equal(classifyField(lex, { type: "text", autocomplete: "cc-number" }), "card_number");
  assert.equal(classifyField(lex, { type: "text", label: "Mật khẩu" }), "password");
  assert.equal(classifyField(lex, { type: "text", name: "email-address", autocomplete: "email" }), "email");
  assert.equal(classifyField(lex, { type: "checkbox", label: "I accept the terms and conditions" }), "terms_checkbox");
  assert.equal(classifyField(lex, { type: "text", name: "q", placeholder: "Search" }), "other");
});
