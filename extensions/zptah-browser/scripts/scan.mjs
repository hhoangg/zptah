// Forbidden-API scan (spec 6.6). Pure functions, no file system access except scanTree.
import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";

/** @type {{ id: string, re: RegExp, why: string }[]} */
const JS_RULES = [
  { id: "eval", re: /(?<![\w$.])eval\s*\(/, why: "eval is remote-code capable" },
  { id: "new-function", re: /\bnew\s+Function\b|(?<![\w$.])Function\s*\(/, why: "Function constructor" },
  { id: "string-timer", re: /\b(?:setTimeout|setInterval)\s*\(\s*['"`]/, why: "string timers evaluate code" },
  { id: "importScripts", re: /\bimportScripts\b/, why: "importScripts loads scripts" },
  { id: "dynamic-import", re: /(?<![\w$.])import\s*\(/, why: "dynamic import" },
  { id: "fetch", re: /(?<![\w$.])fetch\s*\(/, why: "the extension makes no network requests" },
  { id: "XMLHttpRequest", re: /\bXMLHttpRequest\b/, why: "network request" },
  { id: "WebSocket", re: /\bWebSocket\b/, why: "network connection" },
  { id: "EventSource", re: /\bEventSource\b/, why: "network connection" },
  { id: "sendBeacon", re: /\bsendBeacon\b/, why: "telemetry channel" },
  { id: "userScripts", re: /\bchrome\s*\.\s*userScripts\b/, why: "user scripts run arbitrary code" },
  { id: "registerContentScripts", re: /\bregisterContentScripts\b/, why: "persistent injection" },
  { id: "setUninstallURL", re: /\bsetUninstallURL\b/, why: "telemetry channel" },
  {
    id: "tabs-update-active",
    re: /\btabs\s*\.\s*update\s*\([^;]*?\b(?:active|highlighted|selected)\s*:/s,
    why: "must not steal the user's active tab",
  },
  {
    id: "windows-update-focus",
    re: /\bwindows\s*\.\s*update\s*\([^;]*?\b(?:focused|drawAttention)\s*:/s,
    why: "must not steal window focus",
  },
];

/** Applied to HTML pages. */
const HTML_RULES = [
  { id: "inline-script", re: /<script(?![^>]*\bsrc\s*=)[^>]*>/i, why: "inline script (blocked by CSP)" },
  { id: "remote-script", re: /<script[^>]*\bsrc\s*=\s*["']?(?:https?:)?\/\//i, why: "remote script" },
  { id: "remote-resource", re: /<(?:link|img|iframe|embed|object)[^>]*\b(?:href|src|data)\s*=\s*["']?(?:https?:)?\/\//i, why: "remote resource" },
  { id: "inline-handler", re: /<[a-z][^>]*\son[a-z]+\s*=/i, why: "inline event handler" },
];

/** Test hooks must exist only in the dev build. */
const STORE_RULES = [{ id: "test-hook", re: /__zptah/, why: "test symbol in a store build" }];

/**
 * Remove // and block comments while keeping string, template and regex-free code intact.
 * Newlines are kept so line numbers stay correct.
 * @param {string} src
 */
export function stripComments(src) {
  let out = "";
  let i = 0;
  /** @type {string | null} */
  let quote = null;
  while (i < src.length) {
    const c = src[i];
    const n = src[i + 1];
    if (quote) {
      out += c;
      if (c === "\\") {
        out += n ?? "";
        i += 2;
        continue;
      }
      if (c === quote) quote = null;
      i += 1;
    } else if (c === "'" || c === '"' || c === "`") {
      quote = c;
      out += c;
      i += 1;
    } else if (c === "/" && n === "/") {
      while (i < src.length && src[i] !== "\n") i += 1;
    } else if (c === "/" && n === "*") {
      i += 2;
      while (i < src.length && !(src[i] === "*" && src[i + 1] === "/")) {
        if (src[i] === "\n") out += "\n";
        i += 1;
      }
      i += 2;
    } else {
      out += c;
      i += 1;
    }
  }
  return out;
}

/**
 * @param {string} name file name used in messages
 * @param {string} text file content
 * @param {{ store?: boolean }} [opts]
 * @returns {string[]} one message per violation
 */
export function scanSource(name, text, opts = {}) {
  const isHtml = name.endsWith(".html");
  const isJs = /\.(?:m?js)$/.test(name);
  const isCss = name.endsWith(".css");
  const code = isJs ? stripComments(text) : text;
  /** @type {{ id: string, re: RegExp, why: string }[]} */
  const rules = [];
  if (isJs) rules.push(...JS_RULES);
  if (isHtml) rules.push(...HTML_RULES);
  if (opts.store && (isJs || isHtml || isCss)) rules.push(...STORE_RULES);
  /** @type {string[]} */
  const found = [];
  for (const rule of rules) {
    const m = rule.re.exec(code);
    if (!m) continue;
    const line = code.slice(0, m.index).split("\n").length;
    found.push(`${name}:${line}: forbidden ${rule.id} (${rule.why})`);
  }
  return found;
}

/**
 * @param {string} root
 * @param {{ store?: boolean }} [opts]
 * @returns {string[]}
 */
export function scanTree(root, opts = {}) {
  /** @type {string[]} */
  const found = [];
  /** @param {string} dir */
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true }).sort((a, b) => (a.name < b.name ? -1 : 1))) {
      const p = join(dir, entry.name);
      if (entry.isDirectory()) walk(p);
      else found.push(...scanSource(relative(root, p), readFileSync(p, "utf8"), opts));
    }
  };
  walk(root);
  return found;
}
