#!/usr/bin/env node
// Manual check of the Ardia CLI JSON reply shapes zptah relies on.
//
// - MANUAL and macOS-oriented. It never runs in CI.
// - Needs Ardia installed and running with Control on
//   (Settings > Advanced > "Let the ardia command control Ardia").
// - Run it after Ardia or its CLI updates.
// - It drives the same commands zptah's `ardia` integration uses and compares the
//   reply SHAPES (key paths and JSON types, never values) with the fixtures in
//   crates/monocode-core/src/mono/browser/ardia/fixtures/.
// - The read commands run against https://example.com. The ACT commands (click, fill,
//   type, press, select, check, hover, upload, `get attr`, `form`) never touch a real site:
//   the script writes its OWN local page (a file:// page in its temp dir, with a text input,
//   a password input, a button, a link, a select, a checkbox, a file input and a form) and a
//   temp file to upload, opens the page with `ardia tabs open` (a file URL is fine here, the
//   script does not go through zptah's gate) and acts only there. It never submits the form.
// - Every `ardia browse` call uses zptah's argv form:
//   `browse --session S --json <cmd> [flags] -- <values...>` (nothing after the values).
// - It closes every tab it opens and releases its session on exit (also on SIGINT/SIGTERM).
// - `back` and `forward` are skipped: example.com has no history to go back to.
// - `check`, `hover` (success) and `get attr` of a later form are not recorded in the
//   fixtures (unmeasured): the script only prints what they answered, for the next fixture.
//
// Usage: node scripts/ardia-shapes-check.mjs --profile <id> [--ardia <path>] [--keep-going]
// Exit: 0 all ok, 1 mismatch or failure, 2 usage error.

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const here = path.dirname(fileURLToPath(import.meta.url));
const FIXTURES = path.resolve(here, "../crates/monocode-core/src/mono/browser/ardia/fixtures");
const args = process.argv.slice(2);
const opt = (n) => (args.includes(n) ? args[args.indexOf(n) + 1] : undefined);
const profile = opt("--profile");
const ardia = opt("--ardia") || "ardia";
const keepGoing = args.includes("--keep-going");
const env = { ...process.env, PATH: `${process.env.PATH || ""}:${path.join(os.homedir(), ".local/bin")}` };
const S = `zptah-shapes-check-${process.pid}`;

function run(argv) {
  const r = spawnSync(ardia, argv, { encoding: "utf8", timeout: 30000, env });
  if (r.error) return { error: r.error.message, stdout: "", stderr: "" };
  return { stdout: r.stdout || "", stderr: r.stderr || "", status: r.status };
}
function runJson(argv) {
  const r = run(argv);
  if (r.error) return { error: r.error };
  try {
    return { json: JSON.parse(r.stdout) };
  } catch {
    return { error: `non-JSON output (exit ${r.status}): ${(r.stdout || r.stderr).slice(0, 200).trim()}` };
  }
}
const fixture = (name) => JSON.parse(fs.readFileSync(path.join(FIXTURES, `${name}.json`), "utf8"));

// ---- shape extraction ----
const typeOf = (v) => (v === null ? "null" : Array.isArray(v) ? "array" : typeof v);
// Dynamic ids: keys under a "refs" map, or keys like e1, t12, ref3.
const dynamicKey = (parent, k) => parent === "refs" || /^[a-z]{1,3}\d+$/.test(k);

// Returns { types: Map<path, type>, open: Set<path> } where "open" paths are
// empty containers that can match anything below them.
function shapeOf(value) {
  const types = new Map();
  const open = new Set();
  const walk = (v, p, parentKey) => {
    const t = typeOf(v);
    types.set(p, t);
    if (t === "array") {
      if (v.length === 0) open.add(p);
      else walk(v[0], `${p}[]`, "");
    } else if (t === "object") {
      const keys = Object.keys(v);
      if (keys.length === 0) open.add(p);
      let wildDone = false;
      for (const k of keys) {
        if (dynamicKey(parentKey || p.split(".").pop(), k)) {
          if (wildDone) continue;
          wildDone = true;
          walk(v[k], `${p}.*`, "*");
        } else walk(v[k], `${p}.${k}`, k);
      }
    }
  };
  walk(value, "$", "");
  return { types, open };
}
const under = (p, set) => [...set].some((o) => p.startsWith(`${o}.`) || p.startsWith(`${o}[]`));

function diffShapes(expected, actual) {
  const e = shapeOf(expected);
  const a = shapeOf(actual);
  const out = [];
  for (const [p, t] of e.types) {
    if (under(p, a.open)) continue;
    if (!a.types.has(p)) out.push(`missing ${p} (${t})`);
    else {
      const at = a.types.get(p);
      if (at !== t && at !== "null" && t !== "null") out.push(`type ${p}: fixture ${t}, actual ${at}`);
    }
  }
  for (const p of a.types.keys()) {
    if (!e.types.has(p) && !under(p, e.open)) out.push(`extra ${p} (${a.types.get(p)})`);
  }
  return out;
}

// ---- reporting ----
let failed = false;
class Stop extends Error {}
function report(name, diffs, err) {
  if (err) {
    console.log(`FAIL ${name}: ${err}`);
  } else if (diffs.length) {
    console.log(`MISMATCH ${name}`);
    for (const d of diffs) console.log(`  ${d}`);
  } else {
    console.log(`ok ${name}`);
    return true;
  }
  failed = true;
  if (!keepGoing) throw new Stop();
  return false;
}
// Run a command, compare its JSON with a fixture (or just a check function).
function step(name, argv, fixtureName, unwrap = (j) => j) {
  const r = runJson(argv);
  if (r.error) {
    report(name, [], r.error);
    return null;
  }
  const ok = report(name, diffShapes(unwrap(fixture(fixtureName)), unwrap(r.json)));
  return ok ? r.json : r.json;
}

// ---- the act commands, on the script's own local page ----

const TEST_PAGE = `<!doctype html><html><head><title>zptah shapes check</title></head><body>
<h1>Shapes check</h1>
<form id="f" action="/session" method="post">
<label for="n">Name</label><input id="n" name="name" type="text">
<label for="p">Secret</label><input id="p" name="pw" type="password" autocomplete="current-password">
<label for="c">Color</label><select id="c" name="color"><option>red</option><option>blue</option></select>
<label for="a">Agree</label><input id="a" name="agree" type="checkbox">
<label for="u">Attachment</label><input id="u" name="file" type="file">
<button type="submit">Send</button>
</form>
<a id="ext" href="https://www.iana.org/domains/example">Docs</a>
<button id="add" type="button" onclick="var b=document.createElement('button');b.textContent='Inserted';document.body.insertBefore(b,document.body.firstChild)">Add</button>
</body></html>`;

// Finds the ref of an element in a `snapshot -i` reply by role and name.
const refOf = (snapshot, role, name) =>
  Object.entries(snapshot?.data?.refs ?? {}).find(([, v]) => v.role === role && v.name === name)?.[0];

async function actCommands() {
  const page = path.join(tmp, "act.html");
  const upload = path.join(tmp, "up.txt");
  fs.writeFileSync(page, TEST_PAGE);
  fs.writeFileSync(upload, "zptah shapes check\n");
  // A tab of our own for the local page, in the same profile; `tabs open` is the only way that
  // makes a tab in the right profile. Never a real site here.
  const opened = step("act: tabs open (local page)", ["tabs", "open", `file://${page}`, "--profile", profile, "--json"], "tabs_open");
  const tab = opened?.results?.[0]?.tabs?.[0] ?? null;
  if (!tab) return report("act: tab id", [], "no tab id in tabs open reply");
  TABS.push(tab);
  const held = () => runJson(["call", "tabs.hold", "--params", JSON.stringify({ session: S, tabs: [tab], seconds: 120 }), "--json"]);
  held();
  let bound = null;
  for (let i = 0; i < 6; i++) {
    const r = runJson(browse(["tab", tab]));
    if (r.error) return report("act: browse tab", [], r.error);
    bound = r.json;
    if (bound.code !== "tab_asleep" || i === 5) break;
    spawnSync("sleep", ["1"]);
    held();
  }
  if (!bound || bound.success !== true) return report("act: browse tab", [], "could not bind the local page tab");

  const snap = step("act: snapshot -i", browse(["snapshot", "-i"]), "browse_snapshot_interactive", (j) => ({ ...j, data: { ...j.data, snapshot: "" } }));
  const refs = {
    name: refOf(snap, "textbox", "Name"),
    secret: refOf(snap, "textbox", "Secret"),
    color: refOf(snap, "combobox", "Color"),
    agree: refOf(snap, "checkbox", "Agree"),
    file: Object.entries(snap?.data?.refs ?? {}).find(([, v]) => /Attachment|Choose|file/i.test(v.name))?.[0],
    docs: refOf(snap, "link", "Docs"),
    add: refOf(snap, "button", "Add"),
  };
  for (const [what, ref] of Object.entries(refs)) {
    if (!ref) return report(`act: ref of ${what}`, [], "not in the snapshot of the local page");
  }

  // Reads zptah's fresh read depends on.
  const at = (ref) => `@${ref}`;
  step("act: get attr type (password)", browse(["get", "attr"], [at(refs.secret), "type"]), "get_attr_type");
  step("act: get attr autocomplete", browse(["get", "attr"], [at(refs.secret), "autocomplete"]), "get_attr_autocomplete");
  step("act: get attr href (link)", browse(["get", "attr"], [at(refs.docs), "href"]), "get_attr_href");
  step("act: get attr form action", browse(["get", "attr"], ["form", "action"]), "get_attr_form_action");
  step("act: form", browse(["form"]), "browse_form_login");

  // The act commands. Values are after `--`; a text that looks like an option is data there.
  step("act: fill", browse(["fill"], [at(refs.name), "--json"]), "act_fill_password");
  step("act: type", browse(["type"], [at(refs.name), "-5"]), "act_type");
  step("act: press Tab", browse(["press"], ["Tab"]), "act_press_tab");
  step("act: select", browse(["select"], [at(refs.color), "blue"]), "act_select");
  step("act: upload", browse(["upload"], [at(refs.file), upload]), "act_upload");
  step("act: click (adds a node)", browse(["click"], [at(refs.add)]), "act_click_add");
  // Not in the fixtures (unmeasured): print what they answer, to record it.
  for (const [name, cmd, values] of [
    ["check", ["check"], [at(refs.agree)]],
    ["uncheck", ["uncheck"], [at(refs.agree)]],
    ["hover", ["hover"], [at(refs.add)]],
  ]) {
    const r = runJson(browse(cmd, values));
    if (r.error || r.json?.success !== true) report(`act: ${name}`, [], r.error || JSON.stringify(r.json).slice(0, 200));
    else console.log(`INFO act: ${name} answered keys: ${Object.keys(r.json.data).join(", ")} (unmeasured, no fixture)`);
  }
  // A ref nobody has: the gate maps this to "the element changed".
  const unknown = runJson(browse(["click"], ["@e99999"]));
  if (unknown.error) report("act: unknown ref", [], unknown.error);
  else report("act: unknown ref", diffShapes(fixture("act_unknown_ref"), unknown.json));
}

// ---- main ----
if (!profile) {
  console.error("Usage: node scripts/ardia-shapes-check.mjs --profile <ardia-profile-id> [--ardia <path>] [--keep-going]");
  const r = runJson(["call", "profiles.list", "--json"]);
  if (r.json && Array.isArray(r.json)) {
    console.error("Profiles:");
    for (const p of r.json) console.error(`  ${p.id}  ${p.name}${p.default ? " (default)" : ""}`);
  } else console.error(`Could not list profiles: ${r.error || "unexpected reply"}`);
  process.exit(2);
}

const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "ardia-shapes-")));
let TAB = null;
// Every tab this script opened (the example.com tab first): only these are ever closed.
const TABS = [];
let cleaned = false;
function cleanup() {
  if (cleaned) return;
  cleaned = true;
  run(["browse", "--session", S, "close", "--json"]);
  run(["call", "tabs.release", "--params", JSON.stringify({ session: S }), "--json"]);
  // Only ever close the tabs this script opened.
  for (const tab of TABS) {
    const r = runJson(["tabs", "close", tab, "--json"]);
    if (r.json) {
      try {
        report("tabs close", diffShapes(fixture("tabs_close"), r.json));
      } catch {
        /* already failed; keep going */
      }
    } else console.log(`FAIL tabs close: ${r.error}`);
  }
  fs.rmSync(tmp, { recursive: true, force: true });
}
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.on(signal, () => {
    cleanup();
    process.exit(1);
  });
}

// The events zptah relies on: `tab.opened` and `tab.closed` (this script causes them) and
// `app.activated` and `window.opened` (the idle quit's signals that the user is at Ardia, which
// nothing here triggers). The stream is read asynchronously for the whole run (the script's own
// commands are synchronous, so it yields to the event loop at the waits below). With
// `--watch-events N` the script waits N seconds so you can bring Ardia to the front and open a
// window by hand.
const watchSeconds = Number(opt("--watch-events") || 0);
const seen = new Set();
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const events = spawn(ardia, ["events", "tab.opened", "tab.closed", "app.activated", "window.opened", "--json"], { stdio: ["ignore", "pipe", "ignore"], env });
events.stdout.on("data", (chunk) => {
  for (const line of String(chunk).split("\n")) {
    try {
      seen.add(JSON.parse(line).event);
    } catch {
      /* not a JSON line */
    }
  }
});
events.on("error", () => {});
process.on("exit", () => events.kill());

const hold = () =>
  runJson(["call", "tabs.hold", "--params", JSON.stringify({ session: S, tabs: [TAB], seconds: 120 }), "--json"]);
// `browse --session S --json <cmd> [flags] -- <values...>`: options and the command first, every
// value after `--` (taken literally there), nothing after the values.
const browse = (cmd, values = []) => ["browse", "--session", S, "--json", ...cmd, ...(values.length ? ["--", ...values] : [])];

try {
  const hello = step(
    "hello",
    ["call", "hello", "--params", JSON.stringify({ protocol: 4, caller: { name: "zptah-shapes-check", app: "zptah" } }), "--json"],
    "hello",
  );
  if (hello && !(hello.protocol >= 4)) report("hello protocol >= 4", [], `protocol is ${hello.protocol}`);

  const profiles = step("profiles.list", ["call", "profiles.list", "--json"], "profiles", (j) => (Array.isArray(j) ? j[0] : j));
  if (Array.isArray(profiles) && !profiles.some((p) => p.id === profile)) {
    report("profile present", [], `profile ${profile} not found`);
  }

  const opened = step("tabs open", ["tabs", "open", "https://example.com", "--profile", profile, "--json"], "tabs_open");
  TAB = opened?.results?.[0]?.tabs?.[0] ?? null;
  if (TAB) TABS.push(TAB);
  if (!TAB) report("tab id", [], "no tab id in tabs open reply");

  step("tabs.hold", ["call", "tabs.hold", "--params", JSON.stringify({ session: S, tabs: [TAB], seconds: 120 }), "--json"], "hold");

  // Binding can report tab_asleep while the tab wakes up: renew the hold and retry.
  let bind = null;
  for (let i = 0; i < 6; i++) {
    const r = runJson(browse(["tab", TAB]));
    if (r.error) {
      report("browse tab", [], r.error);
      break;
    }
    bind = r.json;
    if (bind.code !== "tab_asleep" || i === 5) break;
    spawnSync("sleep", ["1"]);
    hold();
  }
  if (bind) report("browse tab", diffShapes(fixture("browse_tab_bind"), bind));
  // Nothing below can work without a bound tab, and `open` without one would make a tab in the
  // wrong profile: stop here.
  if (!bind || bind.success !== true) throw new Stop();

  const cmds = [
    ["get url", ["get", "url"], [], "browse_get_url"],
    ["get title", ["get", "title"], [], "browse_get_title"],
    ["snapshot", ["snapshot"], [], "browse_snapshot"],
    ["snapshot -i", ["snapshot", "-i"], [], "browse_snapshot_interactive"],
    ["read", ["read"], [], "browse_read"],
    ["console", ["console"], [], "browse_console"],
    ["errors", ["errors"], [], "browse_errors"],
    ["scroll down 200", ["scroll"], ["down", "200"], "browse_scroll"],
    ["screenshot", ["screenshot"], [path.join(tmp, "shot.png")], "browse_screenshot"],
    ["open", ["open"], ["https://example.com"], "browse_open"],
    ["reload", ["reload"], [], "browse_reload"],
    ["tab list", ["tab", "list"], [], "browse_tab_list"],
  ];
  for (const [name, cmd, values, fx] of cmds) step(`browse ${name}`, browse(cmd, values), fx);

  await actCommands();
  if (watchSeconds > 0) {
    console.log(`INFO bring Ardia to the front and open a window within ${watchSeconds} s`);
  }
  // Yield so the piped event lines are read (a synchronous wait would not).
  await sleep(watchSeconds > 0 ? watchSeconds * 1000 : 1000);
  if (!seen.has("tab.opened")) report("event tab.opened", [], "not seen");
} catch (e) {
  if (!(e instanceof Stop)) {
    console.log(`FAIL unexpected: ${e.message}`);
    failed = true;
  }
} finally {
  cleanup();
}
// The cleanup closed the tab: that is the `tab.closed` event.
await sleep(1000);
console.log(`INFO events seen: ${[...seen].sort().join(", ") || "none"}`);
try {
  if (!seen.has("tab.closed")) report("event tab.closed", [], "not seen");
  if (watchSeconds > 0) {
    for (const name of ["app.activated", "window.opened"]) if (!seen.has(name)) report(`event ${name}`, [], "not seen");
  }
} catch (e) {
  if (!(e instanceof Stop)) throw e;
}
events.kill();
process.exit(failed ? 1 : 0);
