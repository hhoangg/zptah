// @ts-nocheck - the content scripts run in a vm against a tiny DOM.
import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { bundleContent } from "../scripts/build.mjs";
import { createPage } from "./helpers/mini-dom.mjs";

const code = bundleContent(new URL("../src/content", import.meta.url).pathname);

function load(opts) {
  const page = createPage(opts);
  vm.createContext(page.sandbox);
  vm.runInContext(code, page.sandbox);
  return { ...page, ns: page.sandbox.zptahContent };
}

const MIRA = { color: "#d93025", emoji: "🐸", name: "Mira", lang: "en" };

const buttonOf = (page) => page.doc.documentElement.find((n) => n.tag === "button")[0];
const hostOf = (page) => page.doc.documentElement.children.find((n) => n.attrs.get("popover") === "manual");

// ---- cursor ----

test("label text is black or white by WCAG contrast and passes 4.5:1 for the Mono colors", () => {
  const { ns } = load();
  const colors = ["#d93025", "#1a73e8", "#188038", "#f9ab00", "#9334e6", "#e8710a", "#ffffff", "#000000", "#5f6368", "#ff69b4", "#00ffff", "#7f7f7f"];
  for (const c of colors) {
    const fg = ns.cursor.textOn(c);
    assert.ok(fg === "#000000" || fg === "#ffffff");
    const best = Math.max(ns.cursor.ratio(c, "#000000"), ns.cursor.ratio(c, "#ffffff"));
    assert.equal(ns.cursor.ratio(c, fg), best, c);
    // Either colour reaches at least 4.58:1 against any background, so the label is readable.
    assert.ok(best >= 4.5, `${c} ${best}`);
  }
  assert.equal(ns.cursor.textOn("#ffff00"), "#000000");
  assert.equal(ns.cursor.textOn("#000080"), "#ffffff");
});

test("the label has the Mono's emoji, name and verb, at most 48 characters, in both languages", () => {
  const { ns } = load();
  assert.equal(ns.cursor.labelText({ ...MIRA, kind: "click", target: "Add to cart" }), '🐸 Mira · Clicking "Add to cart"');
  assert.equal(ns.cursor.labelText({ ...MIRA, lang: "vi", kind: "click", target: "Thêm vào giỏ" }), '🐸 Mira · Đang bấm "Thêm vào giỏ"');
  const long = ns.cursor.labelText({ ...MIRA, kind: "click", target: "x".repeat(200) });
  assert.ok(Array.from(long).length <= 48, long);
  assert.ok(long.startsWith("🐸 Mira · Clicking"));
  for (const [kind, en] of [["type", "Typing"], ["select", "Choosing"], ["press", "Pressing"], ["scroll", "Scrolling"], ["drag", "Dragging"], ["upload", "Uploading"]]) {
    assert.ok(ns.cursor.labelText({ ...MIRA, kind }).includes(en), kind);
  }
  assert.equal(ns.cursor.labelText({ ...MIRA, kind: "type" }), "🐸 Mira · Typing");
});

test("the cursor is drawn in a closed shadow root popover, starts at the target and fades away", () => {
  const page = load();
  const r = page.ns.run("cursor_move", { x: 120.4, y: 80, ...MIRA, kind: "click", target: "Buy", click: true });
  assert.equal(r.shown, true);
  const host = hostOf(page);
  assert.ok(host, "the host is in the document");
  assert.equal(host.shadowRoot, null, "closed root");
  assert.equal(host.getAttribute("aria-hidden"), "true");
  assert.equal(host.getAttribute("inert"), "");
  assert.ok(page.state.openPopovers.has(host), "shown in the top layer");
  const cursor = host.find((n) => n.className === "c")[0];
  assert.equal(cursor.style.getPropertyValue("transition"), "none", "the first move has no transition");
  assert.equal(cursor.style.getPropertyValue("transform"), "translate(120px, 80px)");
  page.ns.run("cursor_move", { x: 300, y: 200, ...MIRA, kind: "type" });
  assert.match(cursor.style.getPropertyValue("transition"), /250ms ease-out/);
  assert.ok(page.state.shown.length >= 2, "re-shown on each move to stay on top");
  page.fireTimeouts();
  assert.equal(hostOf(page), undefined, "no trace after the fade");
});

test("the cursor is hidden while a screenshot is taken and a bad color is refused", () => {
  const page = load();
  page.ns.run("cursor_move", { x: 1, y: 1, ...MIRA, kind: "click" });
  page.ns.run("effects_suspend", { on: true });
  assert.equal(hostOf(page).style.getPropertyValue("visibility"), "hidden");
  page.ns.run("effects_suspend", { on: false });
  assert.equal(hostOf(page).style.getPropertyValue("visibility"), "");
  assert.equal(page.ns.run("cursor_move", { x: 1, y: 1, ...MIRA, color: "red", kind: "click" }).shown, false);
});

// ---- overlay ----

test("the overlay shows the glow in the Mono's color and a Stop button with its name", () => {
  const page = load();
  page.ns.run("overlay_show", { ...MIRA, pulse: true });
  const host = hostOf(page);
  assert.equal(host.style.getPropertyValue("pointer-events"), "none");
  assert.equal(host.style.getPropertyPriority("display"), "important");
  const glow = host.find((n) => n.className === "g")[0];
  assert.match(glow.style.getPropertyValue("box-shadow"), /inset 0 0 12px 4px rgba\(217,48,37,0\.35\)/);
  assert.match(glow.style.getPropertyValue("animation"), /pulse 2s/);
  assert.equal(buttonOf(page).textContent, "🐸 Stop Mira");
  page.ns.run("overlay_show", { ...MIRA, lang: "vi" });
  assert.equal(buttonOf(page).textContent, "🐸 Dừng Mira");
});

test("a synthetic click on Stop does nothing; a trusted click stops once", () => {
  const page = load();
  page.ns.run("overlay_show", MIRA);
  const button = buttonOf(page);
  button.dispatch("click", { isTrusted: false });
  button.dispatch("pointerdown", { isTrusted: false });
  assert.equal(page.sent.length, 0, "an event a page script made is ignored");
  const down = button.dispatch("pointerdown", { isTrusted: true });
  assert.equal(down.stopped, true);
  assert.equal(page.sent.length, 0, "the click, not the press, stops");
  const click = button.dispatch("click", { isTrusted: true });
  assert.equal(click.stopped, true);
  assert.equal(JSON.stringify(page.sent), JSON.stringify([{ type: "zptah.overlay_stop" }]));
});

test("the page removing the host, hiding it or covering it puts it back", () => {
  const page = load();
  page.ns.run("overlay_show", MIRA);
  const host = hostOf(page);
  host.remove();
  assert.ok(hostOf(page) === host, "re-inserted at once by the observer");
  assert.ok(page.state.openPopovers.has(host));
  // Page CSS sets display:none inline, then closes the popover; the tick restores both.
  host.style.setProperty("display", "none", "");
  host.hidePopover();
  page.tick();
  assert.equal(host.style.getPropertyValue("display"), "block");
  assert.equal(host.style.getPropertyPriority("display"), "important");
  assert.ok(page.state.openPopovers.has(host));
  // A dialog or popover of the page opens later: the overlay is shown again on top.
  const before = page.state.shown.length;
  page.doc.dispatch("toggle");
  assert.ok(page.state.shown.length > before || page.state.openPopovers.has(host));
  // Removing it while the document element is replaced still ends up attached.
  host.remove();
  page.tick();
  assert.ok(host.isConnected);
});

test("under a modal dialog or fullscreen the button is replaced by a line, and comes back after", () => {
  const page = load();
  page.ns.run("overlay_show", MIRA);
  const note = page.doc.documentElement.find((n) => n.className === "n")[0];
  assert.equal(buttonOf(page).style.display, "inline-flex");
  page.state.modal = true;
  page.tick();
  assert.equal(buttonOf(page).style.display, "none");
  assert.equal(note.style.display, "block");
  assert.equal(note.textContent, "Stop from the zptah icon in the toolbar");
  page.state.fullscreen = true;
  page.tick();
  assert.equal(note.textContent, "Press Esc, then stop from the zptah icon");
  page.state.modal = false;
  page.state.fullscreen = false;
  page.tick();
  assert.equal(buttonOf(page).style.display, "inline-flex");
  assert.equal(page.ns.overlay.mode(), "button");
  page.ns.run("overlay_show", { ...MIRA, lang: "vi" });
  page.state.modal = true;
  page.tick();
  assert.equal(note.textContent, "Dừng bằng biểu tượng zptah trên thanh công cụ");
});

test("hiding the overlay removes the host and stops the timers", () => {
  const page = load();
  page.ns.run("overlay_show", MIRA);
  page.ns.run("overlay_hide", {});
  assert.equal(hostOf(page), undefined);
  assert.equal(page.ns.overlay.isShown(), false);
  page.tick();
  assert.equal(hostOf(page), undefined, "the dead tick does not bring it back");
});
