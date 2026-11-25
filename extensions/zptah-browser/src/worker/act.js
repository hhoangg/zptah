// The act path (spec 6.2, 6.3, 6.5): act.prepare, act.commit, act.extend, act.cancel, nav.go,
// nav.history, dialog.respond, upload.put, status.get and stop.resume.
//
// prepare resolves the element, gathers its signals and returns a short-lived id bound to the
// tab, the page generation, the refs and a SHA-256 of the signals. commit checks all of that
// again, gathers the signals a second time and refuses (E_STALE_PREPARE) when the digest moved,
// so a page cannot change the amount, the recipients or the form target behind an approved
// button. Input goes through the content script first; the debugger (debugger.js) is the
// fallback and is attached once per task.
import { b64encode, toHex } from "./bytes.js";
import { randomBytes } from "./crypto.js";
import { createDebugger } from "./debugger.js";
import { DEFAULT_DEADLINE_MS, errorReply, okReply, withDeadline, ZptahError } from "./errors.js";
import { focusGuard } from "./focus-guard.js";
import { eligibleFrames, runInFrames } from "./inject.js";
import { Lexicon } from "./lexicon.js";
import { checkUrl, currentRoster, requireReadable } from "./origins.js";
import { parseRef, vet } from "./pages.js";
import { pageIsVietnamese } from "./normalize.js";
import { classifyField, digestOf, extRisk, extractAmounts } from "./signals.js";
import { getReadableTab, tabUrl } from "./tabs.js";

export const PREPARE_TTL_MS = 30000;
export const APPROVAL_TIMEOUT_MS = 600000;
export const RETRY_TTL_MS = 30000;
export const DIALOG_WAIT_MS = 3000;
const MAX_LIVE_PER_MONO = 16;
const MAX_UPLOAD_BYTES = 32 * 1024 * 1024;
const MAX_CHUNK_BYTES = 512 * 1024;
const UPLOAD_TTL_MS = APPROVAL_TIMEOUT_MS;
const HEX32 = /^[0-9a-f]{32}$/;
const KINDS = new Set(["click", "type", "fill", "select", "press", "hover", "drag", "upload", "scroll"]);
/** Keys that put a character into a field (or paste one). */
const TEXT_KEYS = /^(?:.|Space|Backspace|Delete)$/;
const PASSWORD_TEXT = "zptah Browser never types into password fields.";
const DIALOG_TEXT = "A JavaScript dialog may be open in the tab and is blocking it. Ask the user to answer it (show the tab).";

/** @typedef {import("./origins.js").RosterEntry} RosterEntry */

/** @param {unknown} v @returns {v is Record<string, any>} */
const isObj = (v) => typeof v === "object" && v !== null && !Array.isArray(v);

/** @param {string[]} list */
const uniq = (list) => [...new Set(list)];

/**
 * @param {any} link
 * @param {object} deps
 * @param {ReturnType<import("./readpath.js").registerReadPath>} deps.read
 * @param {Lexicon | null} deps.lexicon the shared lexicon, or null when the build did not embed it
 * @param {any} [deps.api]
 * @param {() => number} [deps.now]
 * @param {(ms: number) => Promise<void>} [deps.sleep]
 * @param {ReturnType<typeof createDebugger>} [deps.dbg]
 * @param {string[]} [deps.trustedHosts] sites known to ignore untrusted clicks
 * @param {ReturnType<typeof import("./effects.js").createEffects>} [deps.effects] cursor and overlay
 * @param {() => void} [deps.onStopState] the toolbar stop was set or resumed (the badge)
 */
export function registerActPath(link, deps) {
  const api = deps.api ?? /** @type {any} */ (globalThis).chrome;
  const now = deps.now ?? Date.now;
  const sleep = deps.sleep ?? ((/** @type {number} */ ms) => new Promise((r) => setTimeout(r, ms)));
  const { read, lexicon } = deps;
  const ownerOf = read.ownerOf ?? (() => null);
  const guard = focusGuard(api);
  const dbg = deps.dbg ?? createDebugger({ api, now, sleep, emit: (m, p) => void link.sendEvent(m, p) });
  const trustedHosts = new Set(deps.trustedHosts ?? []);

  /** @typedef {{ id: string, mono: string, tab: number, frameId: number, refs: string[], action: any, generation: number, url: string, frameUrl: string, digest: string, extRisk: string[], created: number, expires: number, state: "prepared" | "committed_once", afterUrl?: string, valid: boolean, inFlight?: boolean, dispatched?: boolean }} Prepare */
  /** @type {Map<string, Prepare>} */
  const records = new Map();
  /** @type {Map<number, { since: number, prepare_id: string }>} */
  const suspected = new Map();
  /** @type {Map<string, { chunks: Uint8Array[], size: number, next: number, last: boolean, mono: string, at: number }>} */
  const uploads = new Map();

  // ---- helpers ----

  /** @param {Record<string, any>} msg */
  function entryFor(msg) {
    if (link.stopped) throw new ZptahError("E_STOPPED", "zptah Browser is stopped.");
    if (typeof msg.mono !== "string" || !HEX32.test(msg.mono)) throw new ZptahError("E_INVALID", "mono is required");
    const entry = currentRoster(link, read.slot).find((m) => m.id === msg.mono);
    if (!entry) throw new ZptahError("E_UNKNOWN_MONO", "This Mono is not known to the browser.");
    return entry;
  }

  /**
   * @param {string} method
   * @param {(entry: RosterEntry, params: any) => Promise<Record<string, unknown>>} fn
   */
  function route(method, fn) {
    link.handlers.set(method, async (/** @type {Record<string, any>} */ msg) => {
      const id = String(msg.id);
      try {
        const entry = entryFor(msg);
        const params = /** @type {any} */ (msg.params ?? {});
        const deadline = Number.isInteger(params.deadline_ms) ? params.deadline_ms : DEFAULT_DEADLINE_MS;
        return okReply(id, await withDeadline(deadline, () => fn(entry, params)));
      } catch (err) {
        return errorReply(id, err);
      }
    });
  }

  /** Writing needs act level, ownership and a tab the origin rules allow. @param {RosterEntry} entry @param {unknown} id */
  async function ownedTab(entry, id) {
    if (entry.level !== "act") throw new ZptahError("E_UNSUPPORTED", "This Mono can only read the browser.");
    const tab = await getReadableTab(api, entry, id);
    if (ownerOf(tab.id) !== entry.id) throw new ZptahError("E_NOT_OWNER", "This Mono does not own that tab.");
    return tab;
  }

  /** @param {number} tab */
  const isSuspected = (tab) => suspected.has(tab);

  /** @param {Prepare} rec */
  function drop(rec) {
    records.delete(rec.id);
    void runInFrames(api, rec.tab, [rec.frameId], "act_release", { token: rec.id }).catch(() => {});
  }

  /** @param {(r: Prepare) => boolean} pred */
  function dropWhere(pred) {
    for (const r of [...records.values()]) if (pred(r)) drop(r);
  }

  function expire() {
    const t = now();
    dropWhere((r) => r.expires <= t);
    for (const [id, u] of [...uploads]) if (t - u.at > UPLOAD_TTL_MS) uploads.delete(id);
  }

  /**
   * One content-script call in one frame; the errors the content script reports become protocol
   * errors.
   * @param {number} tab @param {number} frameId @param {string} op @param {any} args
   */
  async function content(tab, frameId, op, args) {
    const answers = await runInFrames(api, tab, [frameId], op, args);
    const got = answers.find((a) => a.frameId === frameId);
    if (!got) throw new ZptahError("E_STALE_REF", "That ref is not in a readable frame. Take a new snapshot.");
    const r = got.result;
    if (r?.error) throw new ZptahError(typeof r.code === "string" ? r.code : "E_INVALID", String(r.error));
    if (r?.stale) throw new ZptahError("E_STALE_REF", "That ref is gone. Take a new snapshot.");
    return r;
  }

  /** The tab has not changed since the prepare. @param {Prepare} rec @param {RosterEntry} entry */
  async function sameTab(rec, entry) {
    if (entry.level !== "act") throw new ZptahError("E_UNSUPPORTED", "This Mono can only read the browser.");
    const tab = await getReadableTab(api, entry, rec.tab);
    if (ownerOf(tab.id) !== entry.id) throw new ZptahError("E_NOT_OWNER", "This Mono does not own that tab.");
    if (tabUrl(tab) !== rec.url) throw new ZptahError("E_STALE_PREPARE", "The tab moved to another address since the prepare.");
    if (read.gens.get(tab.id) !== rec.generation) throw new ZptahError("E_STALE_PREPARE", "The page was read again since the prepare.");
    return tab;
  }

  /**
   * Raw signals from the content script to what the app gets, plus the extension's own risk.
   * @param {any} raw @param {any} rawFields @param {any} action
   */
  async function signalsOf(raw, rawFields, action) {
    if (!lexicon) throw new ZptahError("E_UNSUPPORTED", "The risk lexicon is not part of this build (run npm run ext:build).");
    const page = { url: raw.page.url, title: raw.page.title, heading: raw.page.heading, vi: pageIsVietnamese(raw.page.lang, raw.page.sample) };
    /** @param {any} r */
    const build = (r) => {
      const el = { ...r.element };
      el.amounts = uniq(extractAmounts(`${el.name} ${el.text} ${el.nearby_text}`).map((a) => a.slice(0, 64))).slice(0, 10);
      /** @type {Record<string, any>} */
      const form = {
        fields: r.form.fields.map((/** @type {any} */ f) => ({ kind: classifyField(lexicon, f.attrs), label: String(f.attrs.label ?? "").slice(0, 200), filled: f.filled === true })),
        recipients: r.form.recipients,
      };
      if (r.form.conversation_title) form.conversation_title = r.form.conversation_title;
      if (r.form.visibility) form.visibility = r.form.visibility;
      return { el, form };
    };
    const main = build(raw);
    const fieldKind = raw.field ? classifyField(lexicon, raw.field) : "other";
    const kind = action.kind;
    /** @type {Set<string>} */
    const risk = new Set();
    /** @type {any[]} */
    const more = [];
    if (kind === "type") {
      if (raw.password || fieldKind === "password") throw new ZptahError("E_PASSWORD_FIELD", PASSWORD_TEXT);
      for (const c of extRisk(lexicon, { kind: "type", fieldKind }, main.el, main.form, page)) risk.add(c);
    } else if (kind === "fill") {
      for (let i = 0; i < rawFields.length; i += 1) {
        const f = build(rawFields[i]);
        const fk = rawFields[i].field ? classifyField(lexicon, rawFields[i].field) : "other";
        if (rawFields[i].password || fk === "password") throw new ZptahError("E_PASSWORD_FIELD", PASSWORD_TEXT);
        const toggles = action.fields[i].type === "checkbox" || action.fields[i].type === "radio";
        for (const c of extRisk(lexicon, toggles ? { kind: "click" } : { kind: "type", fieldKind: fk }, f.el, f.form, page)) risk.add(c);
        if (i > 0) more.push(f.el);
      }
    } else if (kind === "click" || kind === "select") {
      for (const c of extRisk(lexicon, { kind }, main.el, main.form, page)) risk.add(c);
    } else if (kind === "upload") {
      for (const c of extRisk(lexicon, { kind: "upload" }, main.el, main.form, page)) risk.add(c);
    } else if (kind === "press") {
      if ((raw.password || fieldKind === "password") && keyInsertsText(action.key)) throw new ZptahError("E_PASSWORD_FIELD", PASSWORD_TEXT);
    }
    if (kind === "press" && isPaste(action.key)) risk.add("credential_entry");
    /** @type {{ e: any, f: any } | null} */
    let submit = null;
    const submits = (kind === "type" && action.submit === true) || (kind === "press" && /(^|\+)Enter$/.test(action.key));
    if (submits && raw.submit) {
      const s = build(raw.submit);
      submit = { e: s.el, f: s.form };
      for (const c of extRisk(lexicon, { kind: "click" }, s.el, s.form, page)) risk.add(c);
    }
    const ext = [...risk].sort();
    const digest = await digestOf({ e: main.el, f: main.form, m: more, r: ext, s: submit });
    return { element: main.el, form: main.form, ext_risk: ext, digest };
  }

  /** @param {string} key */
  function keyInsertsText(key) {
    const parts = key.split("+");
    const k = parts[parts.length - 1];
    const mods = parts.slice(0, -1);
    if (mods.some((m) => /^(Control|Ctrl|Meta|Command)$/.test(m))) return /^[vVxX]$/.test(k);
    if (mods.includes("Shift") && k === "Insert") return true;
    return TEXT_KEYS.test(k);
  }

  /** Paste shortcuts put clipboard content into a field: they ask like other credential entry. @param {string} key */
  function isPaste(key) {
    const parts = key.split("+");
    const k = parts[parts.length - 1];
    const mods = parts.slice(0, -1);
    return (/^[vV]$/.test(k) && mods.some((m) => /^(Control|Ctrl|Meta|Command)$/.test(m))) || (k === "Insert" && mods.includes("Shift"));
  }

  /** @param {any} action */
  function refsOf(action) {
    switch (action.kind) {
      case "fill":
        return action.fields.map((/** @type {any} */ f) => f.ref);
      case "drag":
        return [action.from_ref, action.to_ref];
      case "press":
      case "scroll":
        return action.ref === undefined ? [] : [action.ref];
      default:
        return [action.ref];
    }
  }

  /** @param {any} action */
  function checkAction(action) {
    if (!isObj(action) || !KINDS.has(action.kind)) throw new ZptahError("E_INVALID", "unknown action");
    if (action.kind === "fill" && (!Array.isArray(action.fields) || action.fields.length === 0 || action.fields.length > 40)) throw new ZptahError("E_INVALID", "fill needs 1 to 40 fields");
    if (action.kind === "type" && typeof action.text !== "string") throw new ZptahError("E_INVALID", "type needs text");
    if (action.kind === "select" && !Array.isArray(action.values)) throw new ZptahError("E_INVALID", "select needs values");
    if (action.kind === "press" && typeof action.key !== "string") throw new ZptahError("E_INVALID", "press needs a key");
  }

  /** The prepare of an upload needs all its bytes. @param {any} action @param {string} mono */
  function takeUpload(action, mono) {
    const u = uploads.get(action.upload_id);
    if (!u || u.mono !== mono || !u.last) throw new ZptahError("E_INVALID", "That upload is not complete. Send all chunks with upload.put first.");
    const bytes = new Uint8Array(u.size);
    let at = 0;
    for (const c of u.chunks) {
      bytes.set(c, at);
      at += c.length;
    }
    // The stream is each file as a 4 byte big-endian length followed by its bytes, in `names` order.
    /** @type {{ name: string, b64: string }[]} */
    const files = [];
    let pos = 0;
    for (const name of action.names) {
      if (pos + 4 > bytes.length) throw new ZptahError("E_INVALID", "The upload has fewer files than names.");
      const len = new DataView(bytes.buffer, bytes.byteOffset + pos, 4).getUint32(0);
      pos += 4;
      if (pos + len > bytes.length) throw new ZptahError("E_INVALID", "The upload is cut short.");
      files.push({ name: String(name).slice(0, 255), b64: b64encode(bytes.subarray(pos, pos + len)) });
      pos += len;
    }
    if (pos !== bytes.length || files.length === 0) throw new ZptahError("E_INVALID", "The upload does not match its names.");
    return files;
  }

  /** Attaches the debugger (once per tab and task) with its event listeners in place. @param {number} tabId */
  function attachTab(tabId) {
    wireDebuggerEvents();
    return dbg.attach(tabId);
  }

  // ---- act.prepare ----

  /**
   * @param {RosterEntry} entry @param {any} p
   */
  async function prepare(entry, p) {
    expire();
    const action = p.action;
    checkAction(action);
    const tab = await ownedTab(entry, p.tab);
    await clearIfAnswered(tab.id);
    if (isSuspected(tab.id)) throw new ZptahError("E_DIALOG_SUSPECTED", DIALOG_TEXT);
    const dlg = dbg.openDialog(tab.id);
    if (dlg) throw new ZptahError("E_DIALOG_SUSPECTED", `A ${dlg.kind} dialog is open: "${dlg.message.slice(0, 120)}". Answer it first.`);
    if ([...records.values()].filter((r) => r.mono === entry.id).length >= MAX_LIVE_PER_MONO) throw new ZptahError("E_BUSY", "Too many prepared actions are waiting. Commit or cancel some first.");
    const refs = refsOf(action);
    const parsed = refs.map((/** @type {string} */ r) => parseRef(r));
    const frameId = parsed[0]?.frameId ?? 0;
    if (parsed.some((/** @type {{ frameId: number }} */ r) => r.frameId !== frameId)) throw new ZptahError("E_INVALID", "All refs of one action must be in the same frame.");
    const frames = await eligibleFrames(api, tab.id, entry);
    if (frameId !== 0 && !frames.some((f) => f.frameId === frameId)) throw new ZptahError("E_STALE_REF", "That ref is not in a readable frame. Take a new snapshot.");
    const generation = read.gens.get(tab.id);
    if (generation === undefined) throw new ZptahError("E_STALE_REF", "Take a snapshot first.");
    if (action.kind === "upload") takeUpload(action, entry.id);
    if (action.kind === "drag" && !(await dbg.permitted())) throw new ZptahError("E_DEBUGGER_DENIED", "Dragging needs the optional debugger permission. Allow it in the zptah Browser options page.");

    const id = toHex(randomBytes(16));
    /** @param {number} f */
    const gather = async (f) => {
      const r = await content(tab.id, f, "act_prepare", { generation, token: id, kind: action.kind, refs: parsed.map((/** @type {{ ref: string }} */ x) => x.ref), action });
      vetFrame(entry, f, r.url);
      return r;
    };
    let raw = await gather(frameId);
    let sig = await signalsOf(raw.raw, raw.rawFields, action);
    // F1: a confirmation dialog can only be read when the debugger was attached before it opened.
    const wantsAttach = sig.ext_risk.includes("destructive") || sig.ext_risk.includes("payment");
    if (wantsAttach && !dbg.isAttached(tab.id) && (await dbg.permitted())) {
      try {
        await attachTab(tab.id);
        // The infobar moved the page: measure everything again on the final layout.
        raw = await gather(frameId);
        sig = await signalsOf(raw.raw, raw.rawFields, action);
      } catch (err) {
        if (!(err instanceof ZptahError) || (err.code !== "E_DEBUGGER_BUSY" && err.code !== "E_DEBUGGER_DENIED")) throw err;
      }
    }
    const after = await getReadableTab(api, entry, tab.id);
    if (tabUrl(after) !== tabUrl(tab)) throw new ZptahError("E_STALE_PREPARE", "The page moved while it was being read.");
    const t = now();
    /** @type {Prepare} */
    const rec = { id, mono: entry.id, tab: tab.id, frameId, refs, action, generation, url: tabUrl(tab), frameUrl: String(raw.raw.page.url), digest: sig.digest, extRisk: sig.ext_risk, created: t, expires: t + PREPARE_TTL_MS, state: "prepared", valid: true };
    records.set(id, rec);
    dbg.touch(tab.id);
    /** @type {Record<string, any>} */
    const out = {
      prepare_id: id,
      expires_ms: rec.expires,
      digest: sig.digest,
      url: rec.url.slice(0, 8192),
      title: String(raw.title ?? after.title ?? "").slice(0, 8192),
      frame_url: rec.frameUrl.slice(0, 8192),
      element: sig.element,
      form: sig.form,
      ext_risk: sig.ext_risk,
      debugger_attached: dbg.isAttached(tab.id),
    };
    if (p.needs_screenshot === true) {
      try {
        const s = await read.shots.screenshot(entry, { tab: tab.id, max_bytes: 600000 });
        out.screenshot = { png_b64: s.png_b64, method: s.method, width: s.width, height: s.height };
      } catch {
        // No picture of a background tab: the card shows the text preview instead.
      }
    }
    return out;
  }

  /** @param {RosterEntry} entry @param {number} frameId @param {unknown} url */
  function vetFrame(entry, frameId, url) {
    if (vet(entry, [{ frameId, result: { url } }]).length === 0) throw new ZptahError("E_ORIGIN_BLOCKED", "That frame is not on this Mono's allowed list, so zptah Browser does not act in it.");
  }

  // ---- act.commit ----

  /**
   * @param {RosterEntry} entry @param {any} p
   */
  async function commit(entry, p) {
    expire();
    const rec = records.get(String(p.prepare_id));
    if (!rec || rec.mono !== entry.id || !rec.valid) throw new ZptahError("E_STALE_PREPARE", "That prepared action is unknown or has expired. Prepare it again.");
    const input = p.input === "trusted" ? "trusted" : "auto";
    // One commit at a time per prepare: a commit that timed out outside keeps running here, and
    // a second one must not run the action again.
    if (rec.inFlight) throw new ZptahError("E_BUSY", "That prepared action is already being committed.");
    rec.inFlight = true;
    rec.dispatched = false;
    try {
      return await commitRecord(entry, rec, p, input);
    } catch (err) {
      // A commit that failed before touching the page keeps its prepare, except when the page
      // moved, or when the input was already dispatched (the outcome is unknown: no second run).
      if (rec.dispatched || (err instanceof ZptahError && err.code === "E_STALE_PREPARE")) drop(rec);
      throw err;
    } finally {
      rec.inFlight = false;
    }
  }

  /** @param {RosterEntry} entry @param {Prepare} rec @param {any} p @param {"auto" | "trusted"} input */
  async function commitRecord(entry, rec, p, input) {
    const tab = await sameTab(rec, entry);
    await clearIfAnswered(tab.id);
    if (isSuspected(tab.id)) throw new ZptahError("E_DIALOG_SUSPECTED", DIALOG_TEXT);
    const dlg = dbg.openDialog(tab.id);
    if (dlg) throw new ZptahError("E_DIALOG_SUSPECTED", `A ${dlg.kind} dialog is open: "${dlg.message.slice(0, 120)}". Answer it first.`);
    if (rec.state === "committed_once" && input !== "trusted") throw new ZptahError("E_STALE_PREPARE", "That action already ran. Prepare it again, or retry it once with trusted input.");
    const frames = await eligibleFrames(api, tab.id, entry);
    if (rec.frameId !== 0 && !frames.some((f) => f.frameId === rec.frameId)) throw new ZptahError("E_STALE_PREPARE", "The frame is gone.");

    // Re-gather everything and compare. A hung answer here means a dialog is already open.
    const attachedBefore = dbg.isAttached(tab.id);
    const first = await withinDialogWait(content(tab.id, rec.frameId, "act_revalidate", { token: rec.id }), tab, attachedBefore);
    assertLive(rec);
    if (first === TIMED_OUT) throw new ZptahError("E_DIALOG_SUSPECTED", `${DIALOG_TEXT} (Nothing was done.)`);
    vetFrame(entry, rec.frameId, first.url);
    if (rec.state === "committed_once" && (first.mutations ?? 0) > 0) throw new ZptahError("E_STALE_PREPARE", "The page changed after the first try, so it is not retried with trusted input.");
    const sig = await signalsOf(first.raw, first.rawFields, rec.action);
    if (sig.digest !== rec.digest) throw new ZptahError("E_STALE_PREPARE", "The page changed since the prepare (the signals no longer match).");
    if (p.approval && p.approval.digest !== rec.digest) throw new ZptahError("E_STALE_PREPARE", "The approval is for a different state of the page.");
    const gained = sig.ext_risk.filter((c) => !rec.extRisk.includes(c));
    if (gained.length > 0) throw new ZptahError("E_STALE_PREPARE", "The risk of this action grew since the prepare.");
    // Hovering does not activate the element, so its risk is not enforced (the app never asks for it).
    if (sig.ext_risk.length > 0 && action0(rec).kind !== "hover") {
      const covered = new Set(Array.isArray(p.approval?.categories) ? p.approval.categories : []);
      if (!p.approval || !sig.ext_risk.every((c) => covered.has(c))) throw new ZptahError("E_NEEDS_APPROVAL", `This action needs the user's approval for: ${sig.ext_risk.join(", ")}.`);
    }

    // The overlay and the cursor (which moves to the target first); never a reason to fail.
    await deps.effects?.beforeAct(entry, rec, first.key);
    assertLive(rec);
    const action = rec.action;
    const files = action.kind === "upload" ? takeUpload(action, rec.mono) : undefined;
    const useDebugger = input === "trusted" || dbg.isSticky(tab.id) || action.kind === "drag" || trustedHosts.has(hostOf(rec.url));
    assertLive(rec);
    const contentOnly = action.kind === "upload" || action.kind === "scroll" || action.kind === "select";
    /** @type {Record<string, any>} */
    let result;
    let attemptedUntrusted = false;
    if (contentOnly || !useDebugger) {
      attemptedUntrusted = true;
      assertLive(rec);
      rec.dispatched = true;
      const r = await runContent(rec, tab, first.key, files);
      if (r === NEEDS_DEBUGGER) {
        attemptedUntrusted = false;
        rec.dispatched = false;
        assertLive(rec);
        result = await runDebugger(entry, rec, tab, first.key, input);
      } else {
        result = r;
      }
    } else {
      assertLive(rec);
      result = await runDebugger(entry, rec, tab, first.key, input);
    }
    // From here the action ran. A trusted run uses the prepare up; an untrusted one that did
    // nothing keeps it for one trusted retry (spec 6.4).
    const fresh = await api.tabs.get(rec.tab).catch(() => null);
    const effects = /** @type {any} */ (result.effects);
    const nothing = effects.dom_mutations === 0 && !effects.url_changed && !effects.focus_changed && effects.submitted !== true;
    if (attemptedUntrusted && nothing && input === "auto" && result.effects.method === "content" && ["click", "type", "fill", "press"].includes(action.kind)) {
      rec.state = "committed_once";
      rec.expires = now() + RETRY_TTL_MS;
      rec.afterUrl = fresh ? tabUrl(fresh) : rec.url;
      effects.hint = effects.hint ?? "Nothing visibly changed. If the page ignored it, retry once with input: trusted.";
    } else {
      drop(rec);
      if (action.kind === "upload") uploads.delete(action.upload_id);
    }
    dbg.touch(tab.id);
    return { url: String(fresh ? tabUrl(fresh) : result.url ?? rec.url).slice(0, 8192), title: String(fresh?.title ?? result.title ?? "").slice(0, 8192), effects };
  }

  /** @param {Prepare} rec */
  const action0 = (rec) => rec.action;

  /** Stop (or a cancel) while a commit runs: nothing more is dispatched. @param {Prepare} rec */
  function assertLive(rec) {
    if (link.stopped) throw new ZptahError("E_STOPPED", "zptah Browser is stopped.");
    if (!records.has(rec.id)) throw new ZptahError("E_STALE_PREPARE", "That prepared action was cancelled.");
  }

  /** @param {string} url */
  function hostOf(url) {
    try {
      return new URL(url).hostname.toLowerCase();
    } catch {
      return "";
    }
  }

  const TIMED_OUT = Symbol("timed out");
  const NEEDS_DEBUGGER = Symbol("needs debugger");

  /**
   * Waits for a page call, but not longer than 3 s on a loaded page without the debugger: that
   * is how a blocking JavaScript dialog shows itself (F1).
   * @template T
   * @param {Promise<T>} pending @param {any} tab @param {boolean} attached
   * @returns {Promise<T | typeof TIMED_OUT>}
   */
  async function withinDialogWait(pending, tab, attached) {
    pending.catch(() => {});
    if (attached || tab.status !== "complete") return pending;
    const out = await Promise.race([pending, sleep(DIALOG_WAIT_MS).then(() => TIMED_OUT)]);
    return /** @type {any} */ (out);
  }

  /**
   * The action through the content script. Returns NEEDS_DEBUGGER when the page needs trusted
   * input and nothing was done.
   * @param {Prepare} rec @param {any} tab @param {string} key @param {any} files
   */
  async function runContent(rec, tab, key, files) {
    const attached = dbg.isAttached(tab.id);
    const pending = content(tab.id, rec.frameId, "act_perform", { token: rec.id, key, action: rec.action, files });
    const got = await withinDialogWait(pending, tab, attached);
    if (got === TIMED_OUT) {
      // The input was dispatched and the page stopped answering: the outcome is unknown, so no
      // retry (trusted or not) and the late answer goes to the app as an event.
      const live = await api.tabs.get(rec.tab).catch(() => null);
      if (live && live.status === "complete" && tabUrl(live) === rec.url) {
        suspected.set(rec.tab, { since: now(), prepare_id: rec.id });
        drop(rec);
        pending.then(
          (r) => link.sendEvent("act.late", { tab: rec.tab, prepare_id: rec.id, ok: true, url: String(r?.url ?? "").slice(0, 8192), title: String(r?.title ?? "").slice(0, 8192), ...(r?.effects ? { effects: r.effects } : {}) }),
          (e) => link.sendEvent("act.late", { tab: rec.tab, prepare_id: rec.id, ok: false, error: { code: e instanceof ZptahError ? e.code : "E_INVALID", message: String(e?.message ?? e).slice(0, 500) } }),
        );
        throw new ZptahError("E_DIALOG_SUSPECTED", `The page stopped answering after the input, so a JavaScript dialog may be open. The outcome is unknown. ${DIALOG_TEXT}`);
      }
      // A page that is still loading is slow, not blocked: keep waiting for it.
      return pending.then((r) => (r?.needs_debugger === true ? NEEDS_DEBUGGER : r));
    }
    if (got.needs_debugger === true) return NEEDS_DEBUGGER;
    return got;
  }

  // ---- the debugger path ----

  /** Chrome's modifier bit mask for Input.dispatchKeyEvent. @param {string[]} mods */
  const modMask = (mods) => (mods.includes("Alt") ? 1 : 0) | (mods.includes("Control") || mods.includes("Ctrl") ? 2 : 0) | (mods.includes("Meta") || mods.includes("Command") ? 4 : 0) | (mods.includes("Shift") ? 8 : 0);

  /** @type {Record<string, { code: string, vk: number }>} */
  const NAMED_KEYS = {
    Enter: { code: "Enter", vk: 13 }, Tab: { code: "Tab", vk: 9 }, Escape: { code: "Escape", vk: 27 }, Backspace: { code: "Backspace", vk: 8 },
    Delete: { code: "Delete", vk: 46 }, ArrowLeft: { code: "ArrowLeft", vk: 37 }, ArrowUp: { code: "ArrowUp", vk: 38 }, ArrowRight: { code: "ArrowRight", vk: 39 },
    ArrowDown: { code: "ArrowDown", vk: 40 }, Home: { code: "Home", vk: 36 }, End: { code: "End", vk: 35 }, PageUp: { code: "PageUp", vk: 33 },
    PageDown: { code: "PageDown", vk: 34 }, Space: { code: "Space", vk: 32 }, " ": { code: "Space", vk: 32 },
  };

  /** A press of one key combination as CDP key events. @param {string} key */
  function keyEvents(key) {
    const parts = key.split("+");
    const k = parts[parts.length - 1] === "" && parts.length > 1 ? "+" : parts[parts.length - 1];
    const mods = parts.slice(0, parts[parts.length - 1] === "" ? -2 : -1);
    const named = NAMED_KEYS[k];
    const mask = modMask(mods);
    if (!named && [...k].length !== 1) throw new ZptahError("E_INVALID", `Unknown key ${JSON.stringify(k.slice(0, 20))}.`);
    const ch = named ? undefined : k;
    const base = { key: named ? (k === " " || k === "Space" ? " " : k) : k, code: named ? named.code : /^[a-z]$/i.test(k) ? `Key${k.toUpperCase()}` : /^[0-9]$/.test(k) ? `Digit${k}` : "", windowsVirtualKeyCode: named ? named.vk : k.toUpperCase().charCodeAt(0), modifiers: mask };
    const text = ch !== undefined && (mask & 6) === 0 ? ch : k === "Enter" ? "\r" : k === "Space" || k === " " ? " " : undefined;
    return [
      { type: text !== undefined ? "keyDown" : "rawKeyDown", ...base, ...(text !== undefined ? { text } : {}) },
      { type: "keyUp", ...base },
    ];
  }

  /** A content answer about a field: password by attribute, or by what the lexicon reads in its label and name. @param {any} r */
  function isPasswordResult(r) {
    if (r?.password === true) return true;
    return Boolean(r?.field && lexicon && classifyField(lexicon, r.field) === "password");
  }

  /**
   * The password rule on the focused element, right before each key or text goes out (6.5 rule 4).
   * @param {number} tabId @param {RosterEntry} entry
   */
  async function refusePasswordFocus(tabId, entry) {
    const ids = (await eligibleFrames(api, tabId, entry)).map((f) => f.frameId);
    if (!ids.includes(0)) ids.unshift(0);
    const answers = await runInFrames(api, tabId, ids, "act_active", {});
    if (answers.some((a) => isPasswordResult(a.result))) throw new ZptahError("E_PASSWORD_FIELD", PASSWORD_TEXT);
  }

  /**
   * @param {RosterEntry} entry @param {Prepare} rec @param {any} tab @param {string} key @param {"auto" | "trusted"} input
   */
  async function runDebugger(entry, rec, tab, key, input) {
    const action = rec.action;
    if (action.kind === "upload" || action.kind === "scroll" || action.kind === "select") throw new ZptahError("E_UNSUPPORTED", "That action does not need trusted input.");
    if (input === "trusted" && rec.state === "committed_once") {
      // Only one trusted retry per prepared action, and only when nothing changed in between.
      const m = await content(tab.id, rec.frameId, "act_mutations", { token: rec.id });
      if (m.mutations > 0) throw new ZptahError("E_STALE_PREPARE", "The page changed after the first try, so it is not retried with trusted input.");
    }
    await attachTab(tab.id);
    // The infobar changed the layout: the content script measures again for every point.
    await dbg.send(tab.id, "Emulation.setFocusEmulationEnabled", { enabled: true });
    await content(tab.id, rec.frameId, "act_observe", { token: rec.id });
    assertLive(rec);
    rec.dispatched = true;
    const ctx = { token: rec.id, key };
    const point = (/** @type {number} */ which) => content(tab.id, rec.frameId, "act_locate", { ...ctx, which });
    /** @type {string | undefined} */
    let hint;
    const run = async () => {
      switch (action.kind) {
        case "click": {
          const { x, y } = await point(0);
          const button = action.button === "right" ? "right" : action.button === "middle" ? "middle" : "left";
          const modifiers = modMask(action.modifiers ?? []);
          await dbg.send(tab.id, "Input.dispatchMouseEvent", { type: "mouseMoved", x, y, modifiers });
          const rounds = action.double === true ? 2 : 1;
          for (let i = 1; i <= rounds; i += 1) {
            await dbg.send(tab.id, "Input.dispatchMouseEvent", { type: "mousePressed", x, y, button, clickCount: i, modifiers });
            await dbg.send(tab.id, "Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button, clickCount: i, modifiers });
          }
          break;
        }
        case "hover": {
          const { x, y } = await point(0);
          await dbg.send(tab.id, "Input.dispatchMouseEvent", { type: "mouseMoved", x, y });
          break;
        }
        case "drag": {
          const a = await point(0);
          const b = await point(1);
          await dbg.send(tab.id, "Input.dispatchMouseEvent", { type: "mouseMoved", x: a.x, y: a.y });
          await dbg.send(tab.id, "Input.dispatchMouseEvent", { type: "mousePressed", x: a.x, y: a.y, button: "left", clickCount: 1 });
          for (let i = 1; i <= 5; i += 1) await dbg.send(tab.id, "Input.dispatchMouseEvent", { type: "mouseMoved", x: a.x + ((b.x - a.x) * i) / 5, y: a.y + ((b.y - a.y) * i) / 5, button: "left", buttons: 1 });
          await dbg.send(tab.id, "Input.dispatchMouseEvent", { type: "mouseReleased", x: b.x, y: b.y, button: "left", clickCount: 1 });
          break;
        }
        case "type":
        case "fill": {
          const fields = action.kind === "type"
            ? [{ text: action.text, which: 0 }]
            : action.fields.map((/** @type {any} */ f, /** @type {number} */ i) => ({ f, which: i })).filter((/** @type {any} */ x) => x.f.type === "text" || x.f.type === "combobox").map((/** @type {any} */ x) => ({ text: x.f.value, which: x.which }));
          if (action.kind === "fill" && fields.length !== action.fields.length) hint = "Checkboxes, radios and sliders are set without the debugger; only text fields were typed.";
          for (const f of fields) {
            const focused = await content(tab.id, rec.frameId, "act_focus", { ...ctx, which: f.which });
            if (isPasswordResult(focused)) throw new ZptahError("E_PASSWORD_FIELD", PASSWORD_TEXT);
            await refusePasswordFocus(tab.id, entry);
            await dbg.send(tab.id, "Input.dispatchKeyEvent", { type: "rawKeyDown", key: "a", code: "KeyA", windowsVirtualKeyCode: 65, commands: ["selectAll"] });
            await dbg.send(tab.id, "Input.dispatchKeyEvent", { type: "keyUp", key: "a", code: "KeyA", windowsVirtualKeyCode: 65 });
            if (action.kind === "type" && action.slowly === true) {
              for (const ch of f.text) {
                await refusePasswordFocus(tab.id, entry);
                for (const ev of keyEvents(ch)) await dbg.send(tab.id, "Input.dispatchKeyEvent", ev);
              }
            } else {
              await refusePasswordFocus(tab.id, entry);
              await dbg.send(tab.id, "Input.insertText", { text: f.text });
            }
          }
          if (action.kind === "type" && action.submit === true) for (const ev of keyEvents("Enter")) await dbg.send(tab.id, "Input.dispatchKeyEvent", ev);
          break;
        }
        case "press": {
          if (action.ref !== undefined) {
            await content(tab.id, rec.frameId, "act_focus", ctx).catch((e) => {
              if (e instanceof ZptahError && e.code === "E_PASSWORD_FIELD" && !keyInsertsText(action.key)) return null;
              throw e;
            });
          }
          if (keyInsertsText(action.key)) await refusePasswordFocus(tab.id, entry);
          for (const ev of keyEvents(action.key)) await dbg.send(tab.id, "Input.dispatchKeyEvent", ev);
          break;
        }
        default:
          throw new ZptahError("E_UNSUPPORTED", "That action cannot use the debugger.");
      }
    };
    const pending = run();
    pending.catch(() => {});
    // A click can open a JavaScript dialog that holds the input call until it is answered.
    let finished = false;
    for (let i = 0; i < 12 && !finished; i += 1) {
      const out = await Promise.race([pending.then(() => "done"), sleep(DIALOG_WAIT_MS / 12).then(() => "tick")]);
      if (out === "done") finished = true;
      else if (dbg.openDialog(tab.id)) break;
    }
    if (!finished) {
      const d = dbg.openDialog(tab.id);
      if (!d) await pending;
      else hint = `A ${d.kind} dialog opened: "${d.message.slice(0, 200)}". Answer it with browser_handle_dialog.`;
    }
    await sleep(150);
    const seen = finished || !dbg.openDialog(tab.id) ? await content(tab.id, rec.frameId, "act_observed", { token: rec.id }).catch(() => ({ dom_mutations: 0, url_changed: true, focus_changed: false })) : { dom_mutations: 0, url_changed: false, focus_changed: false };
    return { effects: { method: "debugger", dom_mutations: seen.dom_mutations, url_changed: seen.url_changed, focus_changed: seen.focus_changed, ...(hint ? { hint } : {}) }, url: "", title: "" };
  }

  // ---- extend, cancel ----

  route("act.extend", async (entry, p) => {
    expire();
    const rec = records.get(String(p.prepare_id));
    if (!rec || rec.mono !== entry.id) throw new ZptahError("E_STALE_PREPARE", "That prepared action is unknown or has expired.");
    if (!Number.isSafeInteger(p.until_ms)) throw new ZptahError("E_INVALID", "until_ms must be an integer.");
    const limit = rec.created + APPROVAL_TIMEOUT_MS;
    rec.expires = Math.max(rec.expires, Math.min(p.until_ms, limit));
    dbg.touch(rec.tab);
    return { expires_ms: rec.expires };
  });

  route("act.cancel", async (entry, p) => {
    const rec = records.get(String(p.prepare_id));
    if (rec && rec.mono === entry.id) {
      // A refused or timed out upload must not leave its bytes in memory.
      if (rec.action?.kind === "upload") uploads.delete(String(rec.action.upload_id));
      drop(rec);
    }
    return {};
  });

  route("act.prepare", prepare);
  route("act.commit", commit);

  // ---- navigation ----

  /**
   * Waits for a navigation the extension started to finish loading (or the budget to run out).
   * @param {number} tabId @param {string} from
   */
  async function settled(tabId, from) {
    for (let i = 0; i < 100; i += 1) {
      const t = await api.tabs.get(tabId).catch(() => null);
      if (!t) throw new ZptahError("E_TAB_GONE", "The tab is gone.");
      const moved = tabUrl(t) !== from || t.status === "loading";
      if (moved && t.status === "complete") return t;
      if (!moved && i >= 3) return t;
      await sleep(100);
    }
    return api.tabs.get(tabId);
  }

  /** @param {number} tabId */
  function forgetPage(tabId) {
    dropWhere((r) => r.tab === tabId);
  }

  route("nav.go", async (entry, p) => {
    const tab = await ownedTab(entry, p.tab);
    let target;
    try {
      target = new URL(String(p.url));
    } catch {
      throw new ZptahError("E_INVALID", "That is not a valid address.");
    }
    if (target.protocol !== "http:" && target.protocol !== "https:") throw new ZptahError("E_INVALID", "Only http and https addresses can be opened.");
    requireReadable(entry, target.href);
    forgetPage(tab.id);
    await guard.tabs.navigate(tab.id, target.href);
    const after = await settled(tab.id, tabUrl(tab));
    requireReadable(entry, tabUrl(after));
    dbg.touch(tab.id);
    return { url: tabUrl(after).slice(0, 8192), title: String(after.title ?? "").slice(0, 8192) };
  });

  route("nav.history", async (entry, p) => {
    const tab = await ownedTab(entry, p.tab);
    forgetPage(tab.id);
    try {
      await (p.dir === "forward" ? guard.tabs.goForward(tab.id) : guard.tabs.goBack(tab.id));
    } catch (err) {
      throw new ZptahError("E_INVALID", String(err instanceof Error ? err.message : err).slice(0, 200));
    }
    const after = await settled(tab.id, tabUrl(tab));
    requireReadable(entry, tabUrl(after));
    dbg.touch(tab.id);
    return { url: tabUrl(after).slice(0, 8192), title: String(after.title ?? "").slice(0, 8192) };
  });

  // ---- dialogs ----

  route("dialog.respond", async (entry, p) => {
    const tab = await ownedTab(entry, p.tab);
    if (!dbg.isAttached(tab.id)) {
      // Attaching now can only help with a dialog that opens later (F1); say so if there is none.
      await attachTab(tab.id);
    }
    try {
      await dbg.send(tab.id, "Page.handleJavaScriptDialog", { accept: p.accept === true, ...(typeof p.prompt_text === "string" ? { promptText: p.prompt_text } : {}) });
    } catch (err) {
      if (err instanceof ZptahError && err.code === "E_INVALID") {
        throw new ZptahError("E_UNSUPPORTED", "No dialog the debugger can answer: it may have opened before the debugger was attached. Ask the user to answer it in the tab (show the tab).");
      }
      throw err;
    }
    suspected.delete(tab.id);
    return {};
  });

  /**
   * A tab marked "dialog suspected" is probed with a quick call: if the page answers the user
   * has answered the dialog and the mark goes away.
   * @param {number} tabId
   */
  async function clearIfAnswered(tabId) {
    if (!suspected.has(tabId)) return;
    const alive = await Promise.race([
      runInFrames(api, tabId, [0], "state", {}).then(() => true, () => false),
      sleep(1500).then(() => false),
    ]);
    if (alive) suspected.delete(tabId);
  }

  // Reads of a tab with a suspected dialog fail fast with the same code instead of waiting 30 s.
  for (const method of ["page.snapshot", "page.read", "page.find", "page.screenshot", "page.wait"]) {
    const inner = link.handlers.get(method);
    if (!inner) continue;
    link.handlers.set(method, async (/** @type {Record<string, any>} */ msg) => {
      const tab = /** @type {any} */ (msg.params)?.tab;
      if (Number.isSafeInteger(tab) && suspected.has(tab)) {
        await clearIfAnswered(tab);
        if (suspected.has(tab)) return errorReply(String(msg.id), new ZptahError("E_DIALOG_SUSPECTED", DIALOG_TEXT));
      }
      return inner(msg);
    });
  }

  // ---- uploads ----

  route("upload.put", async (entry, p) => {
    expire();
    const id = String(p.upload_id);
    let u = uploads.get(id);
    if (!u) {
      if (p.seq !== 0) throw new ZptahError("E_INVALID", "An upload starts at seq 0.");
      u = { chunks: [], size: 0, next: 0, last: false, mono: entry.id, at: now() };
      uploads.set(id, u);
    }
    if (u.mono !== entry.id || u.last || p.seq !== u.next) throw new ZptahError("E_INVALID", "Unexpected upload chunk.");
    const text = String(p.bytes_b64);
    const bin = atob(text);
    if (bin.length > MAX_CHUNK_BYTES) throw new ZptahError("E_TOO_LARGE", "An upload chunk is at most 512 KiB.");
    if (u.size + bin.length > MAX_UPLOAD_BYTES) {
      uploads.delete(id);
      throw new ZptahError("E_TOO_LARGE", "An upload is at most 32 MiB.");
    }
    const bytes = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i += 1) bytes[i] = bin.charCodeAt(i);
    u.chunks.push(bytes);
    u.size += bytes.length;
    u.next += 1;
    u.last = p.last === true;
    u.at = now();
    return {};
  });

  // ---- status, stop ----

  /** Not Mono-scoped. @param {string} method @param {(params: any) => Record<string, unknown>} fn */
  function open(method, fn) {
    link.handlers.set(method, async (/** @type {Record<string, any>} */ msg) => {
      try {
        return okReply(String(msg.id), fn(msg.params ?? {}));
      } catch (err) {
        return errorReply(String(msg.id), err);
      }
    });
  }

  open("status.get", () => ({
    stopped: link.stopped === true,
    debugger_attached: dbg.attachedTabs(),
    monos_active: new Set([...records.values()].map((r) => r.mono)).size,
  }));

  /**
   * A run of the Mono ended: its debugger sessions detach 2 minutes from now unless a new run uses
   * the tab (decision 4). Only tabs the Mono owns are touched, so another Mono's task is not cut.
   */
  route("task.end", async (entry) => {
    dbg.endTask(dbg.attachedTabs().filter((tab) => ownerOf(tab) === entry.id));
    await deps.effects?.taskEnd(entry.id);
    return {};
  });

  open("stop.resume", () => {
    link.stopped = false;
    deps.onStopState?.();
    return {};
  });

  /** Emergency stop (toolbar): every pending action is dropped and every debugger session detached. @param {string} reason */
  async function stopAll(reason) {
    dropWhere(() => true);
    suspected.clear();
    uploads.clear();
    await dbg.detachAll(reason);
    await deps.effects?.clearAll();
  }

  /** The overlay's Stop for one tab. @param {number} tabId */
  async function stopTab(tabId) {
    forgetPage(tabId);
    suspected.delete(tabId);
    await dbg.detach(tabId, "stopped");
    dbg.forget(tabId);
  }

  /** A tab went away. @param {number} tabId */
  function tabGone(tabId) {
    deps.effects?.forget(tabId);
    forgetPage(tabId);
    suspected.delete(tabId);
    dbg.forget(tabId);
  }

  /**
   * The main frame of a tab committed a navigation: a prepare is stale, and a tab that left its
   * Mono's origins loses the debugger (6.5 rule 2).
   * @param {number} tabId @param {string} url
   */
  async function navigated(tabId, url) {
    const owner = ownerOf(tabId);
    const entry = (owner ? currentRoster(link, read.slot).find((m) => m.id === owner) : null) ?? null;
    dropWhere((r) => r.tab === tabId && (r.url !== url || (entry !== null && checkUrl(entry, url) !== "ok")));
    if (entry && checkUrl(entry, url) !== "ok") {
      suspected.delete(tabId);
      await deps.effects?.clear(tabId);
      await dbg.detach(tabId, "origin_left");
    }
  }

  // ---- Chrome events ----

  api.tabs?.onRemoved?.addListener((/** @type {number} */ tabId) => tabGone(tabId));
  api.webNavigation?.onCommitted?.addListener((/** @type {{ tabId: number, frameId: number, url: string }} */ d) => {
    if (d.frameId === 0) void navigated(d.tabId, d.url);
  });
  // A new document has no overlay yet: draw it again for a tab the Mono still controls.
  api.webNavigation?.onDOMContentLoaded?.addListener((/** @type {{ tabId: number, frameId: number }} */ d) => {
    if (d.frameId === 0) void deps.effects?.reapply(d.tabId);
  });
  // chrome.debugger only exists once the optional permission is granted.
  let debuggerWired = false;
  function wireDebuggerEvents() {
    if (debuggerWired || !api.debugger?.onEvent) return;
    debuggerWired = true;
    api.debugger.onEvent.addListener((/** @type {any} */ source, /** @type {string} */ method, /** @type {any} */ params) => dbg.onEvent(source, method, params));
    api.debugger.onDetach?.addListener((/** @type {any} */ source, /** @type {string} */ reason) => dbg.onDetach(source, reason));
  }
  wireDebuggerEvents();
  api.permissions?.onAdded?.addListener(wireDebuggerEvents);
  api.action?.onClicked?.addListener(() => {
    link.stopped = true;
    deps.onStopState?.();
    void stopAll("stopped");
    link.sendEvent("stop", { by: "toolbar" });
  });

  return { records, suspected, uploads, dbg, stopAll, stopTab, tabGone, navigated, sweep: () => dbg.sweep() };
}
