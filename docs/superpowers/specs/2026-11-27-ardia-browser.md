# Mono Browser on Ardia - design

Status: design spec. No code. Date: 2026-11-27. Native app only.

Supersedes the transport of `2026-11-25-zptah-browser-extension.md` (the zptah Browser Chrome
extension) and the recipes of `2026-11-22-browser-recipes-design.md`. The policy layer of those
specs (levels, approvals, origins, audit, soul rules, defense) stays.

Abbreviations: `B` = `crates/monocode-core/src/mono/browser.rs`, `BD` =
`crates/monocode-core/src/mono/browser/`, `GPUI` = `crates/monocode-gpui/src`.

## 1. Summary

A Mono drives the user's **Ardia** browser (https://github.com/ardia-browser/ardia, a Chromium
browser for macOS, Linux and Windows) instead of Chrome through our extension. Ardia ships an
`ardia` command that drives the running browser:

- `ardia browse <cmd>`: agent-browser's command set (snapshot with `@eN` refs, click, fill, type,
  press, screenshot, console, errors, storage, flows), per-agent sessions with their own background
  tabs, page content already wrapped in `UNTRUSTED_PAGE_CONTENT nonce=...` markers.
- `ardia call <method>`: any method of Ardia's JSON-lines control socket (protocol 4): tabs,
  windows, profiles, `tabs.input`, `tabs.diagnostics`, `tabs.hold`, history, bookmarks, downloads.
- `ardia events` / `subscribe`: an event stream (`tab.navigated`, `page.error`, `tab.crashed`, ...).

The chain becomes:

```
Mono (Claude Code / Codex)
  -> zptah_browser MCP gate (unchanged role: levels, origins, risk, approvals, audit, defense)
  -> ArdiaUpstream (new): spawns `ardia browse --session <s> ... --json` and `ardia call ... --json`
  -> Ardia
```

Decisions taken with the user on 2026-11-27:

1. **Ardia replaces the extension outright.** The extension, its pairing and native messaging
   host are deleted in the same release. No parallel backend.
2. **The zptah gate stays.** A Mono reaches the browser only through the `zptah_browser` MCP
   server. A raw `ardia` call from the Mono's shell is blocked, otherwise levels, origin rules and
   approvals would be bypassed. Claude gets `--disallowedTools "Bash(ardia:*)"`; Codex has no
   per-command shell deny, so its instructions forbid it and the audit flags it. Neither is
   airtight (a Mono can wrap the call in another shell form), so this is a guard against mistakes,
   not a security boundary. M0 checks whether Ardia can restrict which callers get control.
3. **Ardia flows replace zptah recipes.** The recipe store, runner, repair, sharing and the
   Recipes UI are deleted.
4. **Linux and Windows are assumed to work like macOS** through the same `ardia` CLI. Ardia's
   platform flags (`apps/browser/src/lib/platformFeatures.{linux,win}.ts`) turn off some extras
   there (`readsCliConsole`, `nativeTabTools`, `controlDoorbell`, and on Windows
   `drivesTabsFromEngine`); tools that depend on them must fail with a clear message, not hang.

## 2. Prerequisites and readiness

Ardia is usually closed (section 2a), so readiness is split in two. The Browser tab shows the
first check that fails, from either group, with the step to fix it.

**Static checks** work with Ardia closed. They run at Mono launch (`resolve`), when the Browser tab
opens and on Try again. Only these can keep browser tools away from a Mono:

| Check | How | Fix shown to the user |
|---|---|---|
| `ardia` on PATH | resolve `ardia` in the login-shell PATH the harness gets (plus `~/.local/bin`) | Install Ardia, then Settings > Plugins > Command line |
| Ardia installed | the app bundle / binary is found (macOS: `Ardia.app` via LaunchServices) | Install Ardia |
| A profile saved | the Mono has an `ardiaProfile` id | Pick a profile |

A static failure at launch keeps today's behavior: the Mono starts without browser tools and the
reason goes into its instructions (`browser_unavailable`).

**Live checks** need a running Ardia. They run lazily on the first browser tool call of a
conversation and after every launch, once the gate has launched Ardia if needed (section 2a), and
again when the Browser tab's Try again runs while Ardia is up:

| Check | How | Fix shown to the user |
|---|---|---|
| Ardia reachable | `ardia call hello` exits 0 (exit 2 = can't reach); fails only if the launch in section 2a fails | Open Ardia |
| Control on | the reply is not `Control is off.` | Settings > Advanced > "Let the ardia command control Ardia" |
| Protocol | `hello` reports `protocol >= 4` | Update Ardia |
| Profile exists | the Mono's `ardiaProfile` is in `profiles.list` | Pick a profile |

A live failure fails that tool call with a clear tool error naming the check and its fix, and the
Browser tab's readiness row shows the same failure. It never removes the browser tools from a
running Mono and never blocks the Mono's launch: Ardia being closed is the normal state. The gate
caches the last good live result (with the Ardia version and protocol it saw) and shows it while
Ardia is closed; a later live failure replaces it.

## 2a. Launching Ardia on demand

Expected use: Chrome stays the user's main browser; Ardia holds only the logins used for
automation and is usually closed. So "Ardia running" is not a prerequisite. On any browser tool
call (never at Mono launch) where `hello` cannot reach Ardia, for example the first call of a
conversation or after the user quit Ardia mid-turn, the gate:

1. launches Ardia in the background without taking focus:
   - macOS: `open -g -j -a Ardia` (background and hidden, measured below),
   - Linux and Windows: the installed binary (path found in M0), detached;
2. polls `ardia call hello` until it answers (timeout about 20 s), runs the live checks
   (section 2), then continues the call;
3. on timeout, fails the tool call with "Ardia didn't start" and the readiness row shows it.

The gate records that it launched this Ardia (the process id it started, and that `hello` was
unreachable before), so it can tell its own launch from one the user made.

The `ardia` CLI 0.7.0 does not launch Ardia itself (no launch path found in its strings); zptah
does it. Control (Settings > Advanced) is a saved Ardia setting, so it survives a relaunch.

Measured on macOS 2026-11-27 (Ardia 0.2.49, CLI 0.7.0, Ardia quit first):

- `ardia call hello` with Ardia closed exits 2.
- `open -g -a Ardia`: reachable after 1.6 s; Chrome stayed frontmost; Ardia restored its window
  behind it.
- `open -g -j -a Ardia`: reachable after 1.0 s; Ardia is hidden (`visible false`, `frontmost
  false`). `ardia browse --session ... open <url>` and `get title` worked while hidden, and Ardia
  stayed hidden afterwards. **Use `-g -j`.**

### Exposure window

While Ardia runs with Control on, any local process (not only the gate) can drive the logged-in
profile through the `ardia` command or the control socket. A hidden Ardia that zptah started and
then forgot keeps that door open with nobody watching. So zptah keeps the window short:

- **Idle quit.** zptah quits an Ardia that it launched after an idle timeout: 10 minutes (setting,
  default 10) with no browser tool call and no running Mono turn that has used the browser. It
  quits the app gracefully (macOS: `quit` via the app's bundle id; Linux and Windows: found in
  M0), after it has closed the session tabs (section 4).
- **Indicator.** While a zptah-launched Ardia runs, the app shows "Ardia running (started by
  zptah)" with a Quit button (in the Mono's Browser tab and in the conversation's browser row).
  Quit closes the session tabs and quits Ardia at once.
- **Never quit the user's Ardia.** If Ardia was already reachable when the gate first needed it,
  or the user brought it to the front or opened a window in it after zptah launched it (M0 checks
  which signal is reliable), zptah treats it as the user's: no idle quit, no Quit button, only the
  session tabs are closed.

Linux and Windows: no auto-launch in v1 (section 12b).

## 3. Settings

`MonoBrowser` (stored in `mono-roster`):

| Field | Change |
|---|---|
| `enabled`, `trust`, `blockedOrigins`, `allowedOrigins` | keep |
| `profileDir`, `profileName` | replace with `ardiaProfile` (id from `profiles.list`) and `ardiaProfileName` (display only) |
| `showActions`, `ready`, `connection`, `migrationNotice`, `migrated`, `suggestNewDefaults` | remove |

`connection` is never written today, so `needs_reconnect()` is always true for a Mono with a
profile; it goes away with the field.

Migration: none. The extension was never used outside development (confirmed by the user on
2026-11-27), so there is no user data to carry over or clean up.

- Old fields in `mono-roster` are ignored on load (serde skips unknown keys) and dropped on the
  next save. A Mono with `enabled` but no `ardiaProfile` simply shows the profile picker.
- The Playwright-era `migration.rs` and its settings keys are deleted with the code.
- Recipe tables (`mono_browser_recipe_*`) are no longer created; existing ones are dropped with
  `DROP TABLE IF EXISTS` at store open. `<mono>/recipes/` and `_shared_recipes/` are not touched.
- Leftovers on development machines (native messaging manifests, keychain service
  `app.zptah.browser-link[.dev]`, `browser-links.json`, the unpacked extension) are removed by
  hand; the app ships no cleanup code for them.

## 4. Profiles and tabs

- Each Mono picks one Ardia profile (space). The picker lists `profiles.list`.
- Session name: `zptah-<monoId>-<conversationId>`, passed as `--session` on every `ardia browse`
  call, so each conversation has its own tabs and refs.
- Opening a tab: the session's tab must be a background tab **in the Mono's profile**.
  `ardia browse open` opens a background tab but takes no profile; `windows.create {profile}`
  takes one but steals focus (seen on 2026-11-27). The M0 spike picks the mechanism (candidates:
  `apply` with an `open` op and a profile, `offscreen`, or a dedicated off-screen window per
  profile), then binds the session with `ardia browse --session <s> tab <tab-id>`.
- `tabs.hold {untilDisconnect}` keeps the session's tabs awake while the gate is connected.
- Cleanup: the gate tracks the tab ids it opened and closes them with `ardia tabs close` when the
  conversation ends. `windows.close` alone is not enough: Ardia moved the tabs of a closed window
  into the remaining window, including a tab from another profile.
- The user's own tab is used only when the Mono asks for it (`browser_tabs adopt_active`, act level
  only). The gate finds it without binding anything (`ardia call windows`: the active tab of the
  focused window), refuses it unless it is in the Mono's Ardia profile, shows its address on the
  approval card (once per conversation per tab id, recorded before the bind), and only then binds
  that exact id (`tab <tab-id>`) and checks the id Ardia says it bound.
- **Tab ownership.** For each session the gate keeps the set of tab ids it opened for that session,
  plus the user's active tab once the approval above is given (that exact tab id only). The gate
  binds (`tab <tab-id>`), switches to and acts in only tab ids in that set, and closes only the
  tab ids it opened itself. The user's tab is never closed, only unbound. Any other tab id a Mono
  passes (another session's tab, another Mono's, or one the user opened) is rejected with a tool
  error and audited. `browser_tabs` lists only the set. "Session tabs" below means the tabs the
  gate opened.

## 5. Tools

The gate keeps its MCP shape (`--mcp-config` for Claude, `-c mcp_servers.zptah_browser.*` for
Codex, `--allowedTools mcp__zptah_browser`, level-based `disabled_tools`). The tool list is
re-cut to Ardia's commands; names stay `browser_*` so the prompt and the parked-process key keep
working.

| Tool | Ardia call | Level |
|---|---|---|
| `browser_open` | open tab in profile (section 4), then navigate | read |
| `browser_navigate`, `browser_back`, `browser_forward`, `browser_reload` | `ardia browse open` / `back` / `forward` / `reload` | read |
| `browser_snapshot` | `ardia browse snapshot [-i] [-s css]` | read |
| `browser_read` | `ardia browse read` / `get text` | read |
| `browser_screenshot` | `ardia browse screenshot <path>` into the Mono's output folder | read |
| `browser_tabs` | session tabs only (`tab list`), new/close of own tabs | read |
| `browser_console`, `browser_errors` | `console`, `errors` (or `tabs.diagnostics`) | read |
| `browser_scroll` | `scroll` | read |
| `browser_click`, `browser_type`, `browser_fill`, `browser_press`, `browser_select`, `browser_check`, `browser_hover`, `browser_upload` | matching `ardia browse` commands | act, risk-checked |
| `browser_request_approval`, `browser_declare_intent` | gate only | act |
| `flow_list`, `flow_show` | `ardia browse flow list` / `show` | read |
| `flow_run` | `ardia browse flow run` after the check in section 7 | act |
| `flow_save` | `ardia browse flow save` (from the session's steps) | act |

Not exposed: `eval`, storage writes, cookies, network routing, `ardia call` methods that change
the browser outside the session (bookmarks, history, profiles, windows of the user). They can be
added later one at a time.

### Origin rules

Blocked and allowed origins are checked in two ways, and the spec does not pretend the second one
can prevent anything.

**Checked before the call** (the call does not run on a hit):

- `browser_open` and `browser_navigate`: the URL argument.
- `browser_click` on a link: the link's `href`, taken from the fresh scoped read of the target
  (section 6).
- `browser_click` on a submit control, and `browser_press` Enter in a form: the form's `action`
  (resolved against the page URL) from the same scoped read.
- `flow_run`: every URL the flow names (section 7).
- The page the tab is on before any act tool.

**Detected only after the call:** navigation that page scripts start (a click handler, a redirect,
`window.open`, a meta refresh, a form submitted by script). The gate reads the tab's URL from the
reply (and `tab.navigated` events) after every call. On a hit it:

1. stops further browser calls in that tab for the rest of the turn,
2. closes the tab if it is one of the session's own tabs (section 4); a user tab is left open but
   unbound,
3. writes an audit row with the before URL, the hit URL and the action that led there,
4. returns a tool error that says what happened.

## 6. Risk and approvals

Unchanged: `BrowserLevel`, `TrustState::evaluate`, `ApprovalQueue`, conversation grants, soul
rules, audit rows, approval cards (desktop, web, phone), `copy.rs`.

Changed: the inputs. The extension computed `ElementSignals` / `FormSignals` from the DOM. With
Ardia the gate builds them from what it can read:

- the target's role, name and value from a fresh scoped read (see "Act tools" below),
- the form around it and the page URL/title (`ardia browse get` and a scoped `snapshot -s`),
- the risk lexicon and site table, moved from `extensions/zptah-browser/shared/*.json` into
  `BD/risk/data/` **before** the extension folder is deleted (the core build embeds them).

Fidelity drops where the extension used live DOM facts (autocomplete hints, hidden fields).
When signals are incomplete, the gate leans to asking.

### Passwords and one-time codes

Passwords and one-time codes stay blocked for the Mono, as today (`fill-otp` is not exposed). M0
checks whether `snapshot` or a scoped read exposes an input's `type` (`password`) and
`autocomplete` (`current-password`, `new-password`, `one-time-code`).

- If it does, the gate refuses `browser_fill` and `browser_type` into those fields.
- If it does not, the gate cannot see the field type, so:
  - a field whose name or label matches the password/OTP lexicon (`BD/risk/data/`, for example
    password, passcode, PIN, one-time code, verification code, 2FA, and their Vietnamese forms) is
    never typed into, with no approval that overrides it;
  - `browser_fill` or `browser_type` into any textbox whose kind is unknown on a login-like page
    (the URL, title or form matches the login lexicon, or the form has a field from the lexicon)
    asks first.

### Act tools: fresh read, then act

The extension's prepare/commit digest goes away. A `@eN` ref comes from an earlier snapshot, and
the page can change between that snapshot and the action (time-of-check to time-of-use). So per act
tool the gate:

1. does a **fresh scoped read** of the target right before evaluating: its role, name and value,
   the form around it (fields, `action`), the link `href` if any, and the tab URL;
2. fails the call with "the element changed, take a new snapshot" if the ref no longer resolves,
   or its role or name differs from the snapshot the Mono acted on, or the tab URL changed;
3. evaluates the risk on the fresh read (not on the old snapshot) and checks origins (section 5);
4. asks if needed;
5. runs the `ardia browse` command and audits its result, including the URL after it.

When the gate cannot read enough of the page to tell what an action does (a destination it could not read, a bare word such as Confirm with no dialog or page text to explain it, a focus it could not identify), the action asks with its own category `unclear` (card line: "zptah could not read enough of this page to tell what this does."), which is never granted for the conversation.

M0 documents how stable `@eN` refs are: whether they survive a re-snapshot, a scroll, a DOM update
and a navigation, and whether a stale ref errors or silently points at another element. If a stale
ref can silently resolve to another element, the gate passes a selector or a role-and-name locator
from the fresh read instead of the bare ref.

### Uploads

`browser_upload` always asks, at every level and with no conversation grant. The approval shows the
exact absolute path and the file size of each file. Allowed paths:

- inside the Mono's project folder or its output folder, or
- a path the user gave in the conversation (verbatim, resolved after symlinks).

Always blocked, even when inside an allowed folder: dotfiles and dot folders (`.env`, `.env.*`,
`.git/`, ...), `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config/gcloud`, `~/.kube`, keychains
(`~/Library/Keychains`, `*.keychain-db`) and the app's own data folders. Paths are checked after
resolving symlinks.

## 7. Flows

Flows are recorded and replayed by Ardia. A replay runs inside Ardia, so the gate does not see each
step. Policy for `flow_run`:

1. `flow_show <name>` and classify every step with the same risk rules (step kind, target name,
   site). A step kind the gate does not know, and any script or eval step, counts as risky.
2. The parameters passed to `flow_run` are part of the check: they are classified with the step
   that uses them (a parameter filled into a password or OTP field blocks the run, section 6), and
   they are part of the hash below.
3. Every URL a step names, on any origin, is checked against the blocked origins before the run; a
   blocked one refuses the run. A step that leaves the Mono's allowed origins counts as risky.
4. If any step is risky, ask once for the whole run, listing those steps and the parameters. The
   approval and the grant "allow this flow in this conversation" are keyed by a content hash of the
   `flow show` output plus the parameters.
5. Right before the run the gate reads the flow again with `flow show` and runs it only if the hash
   equals the one it classified in step 1 (and approved in step 4, when it asked). If the flow
   changed in between, it is classified, and approved if needed, again.
6. After the run, the origin checks of section 5 (detected after) apply to the tab the flow used.
7. Read-only Monos cannot run flows.

`flow heal` is not exposed in v1; the user heals flows from a terminal.

Command form: a flow name matches `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$` and goes after the `--`; a
parameter is `--param`, `k=v` (two arguments before the `--`, `k` matching `^[A-Za-z_][A-Za-z0-9_]{0,63}$`
so the `k=v` token can never be read as an option, whatever `v` says); `--yes` is the gate's and only
for a flow with `writes: true`. A conversation grant for a flow is stored under `flow:<name>:<hash>`
and covers that content only; `flow_save` always asks (never granted), and refuses a name that is not
a flow name.

Open: the format of `flow show` output and whether a flow is tied to a profile. The M0 spike
records one flow and reads it back.

## 8. Defense

- Ardia already wraps page content in nonce markers. The gate does not fence `ardia browse`
  output again; it still runs `strip_invisible`, `redact_secrets` and the injection scan on it,
  and keeps the per-turn taint guard.
- `defense.rs` stays whole: the Chief spec reuses its helpers.

## 9. Stop

There is no in-page glow or Stop button. Stop lives in zptah: the Mono's conversation shows a
"Working in Ardia" row with Stop, which kills the in-flight `ardia` process, releases the holds and
denies further browser calls for that turn. Resume is the next user message.

## 10. Browser tab (GPUI)

Order: readiness row (section 2), "Ardia running (started by zptah)" with Quit when it applies
(section 2a), profile picker, level, blocked/allowed origins, grants and rule review, activity
audit. Removed: Browser connection card, migration and Reconnect cards, Chrome and Full Disk
Access checks, Show cursor, Recipes section.

## 11. Removal list

- `extensions/zptah-browser/` (after moving the risk data), `package.json` `ext:*` scripts,
  `native.yml` extension jobs and path filter, `extension-release.yml`, `README.md` mention,
  `docs/browser/zptah-browser.md` (rewritten for Ardia).
- Store listing and assets: `extensions/zptah-browser/store/` (`listing.md`, `justifications.md`,
  `icon-128.png`, `promo-tile-440x280.png`, `screenshots/`) go with the extension folder.
- Privacy page: `web/landing/src/html/privacy.mjs` describes the extension (native messaging,
  extension storage, debugger permission, Chrome Web Store User Data Policy). Rewrite it for Ardia
  (what the gate sends to the `ardia` command, what stays local, the audit log, that Ardia's
  Control is Ardia's own model) and bump `UPDATED`. The page is served at `/privacy`, so this needs
  a web deploy (listed in M4).
- Other mentions: the extension comments in `web/landing/build.mjs`, `web/worker/src/landing.ts`
  and `web/docs/deploy-runbook.md` (`/privacy`), the "zptah Browser extension" section of
  `docs/native/release-flow.md`, and the `CHANGELOG.md` entry (a new entry, the old one stays as
  history).
- Chrome Web Store: the store draft prepared on 2026-11-26 (`~/Documents/zptah-webstore`, outside
  the repo) is abandoned and **must not be submitted**. The user deletes it by hand; nothing in the
  repo or the app refers to it.
- `BD`: `connect`, `link/`, `links/`, `peer/`, `host_mode`, `roster`, `ownership`, `e2e`,
  old `migration`, `gate/extension.rs`, `gate/extension/act.rs`, `gate/extension/act/own_tabs.rs`,
  `gate/refs.rs`, `gate/tabs.rs`, `recipe/` (all), Chrome profile checks in `B`.
- `main.rs`: host-mode early exit, `connect::start_app`, `connect::shutdown`.
- `mono.rs`: `roster::monos_changed` calls.
- `GPUI`: `monos/browser_connect*`, `monos/browser_migration.rs`, `monos/recipes_ui.rs`, the
  stopped banner in `conversation/browser_cards.rs`, Resume in `monos/browser_approval.rs`.
- Cargo: `test-support` browser e2e feature and the `browser_e2e_app` example.
- Store: recipe tables (dropped at store open, section 3).

Kept: `gate.rs` (with a new `Upstream`), `gate/guard.rs`, `gate/http.rs`, `defense.rs`, `trust/`,
`risk/`, `soul_rules.rs`, `copy.rs`, `output.rs`, `server.rs`, `provider/browser_gate.rs`,
approval cards, audit UI.

## 12. Milestones

- **M0 spike** (no product code): background tab in a chosen profile without focus steal; flow
  record/show/run format; `--json` output of the commands in section 5; exit codes; behavior with
  Ardia closed mid-call; `@eN` ref stability (section 6); whether a snapshot or scoped read
  exposes input `type` and `autocomplete` (section 6); telling a zptah-launched Ardia from the
  user's, and quitting it (section 2a); Linux run of the same commands. M0 also records the
  `ardia` output fixtures the tests use (section 12a).
- **M1** `ArdiaUpstream` + static and live readiness + on-demand launch, idle quit and indicator
  + profile setting + read tools. Mono can read pages.
- **M2** act tools on the new risk inputs (fresh read, origin checks, password/OTP, uploads, tab
  ownership), approvals, audit, Stop.
- **M3** flows (section 7).
- **M4** removal (section 11), docs, CI, the rewritten privacy page and its web deploy.

M4 ships in the same release as M1-M3: there is no release with two backends. The privacy page
deploy goes out with that release.

## 12a. Tests

CI runs on Linux and Windows and has no Ardia, so no automated test needs a real Ardia.

- The `ardia` output shapes (`hello`, `profiles.list`, snapshot, scoped read, tab list, flow show,
  errors, `Control is off.`, exit codes) are recorded from a real Ardia in M0 and checked in as
  fixtures.
- Gate tests (readiness, launch, idle quit, origins before and after, fresh read and changed refs,
  password/OTP, uploads, tab ownership, flow hash and parameters) put a fake `ardia` binary first
  on PATH that replays those fixtures by argument and can be told to fail, hang or exit 2. The
  install detection (LaunchServices on macOS), launch and quit steps are behind a trait, so tests
  on every platform fake them and never look up or start an app.
- Real-Ardia checks are a manual macOS script (in `scripts/`, not run in CI) that drives an
  installed Ardia through the same commands and compares the shapes with the fixtures. It is run
  by hand when Ardia or its CLI updates.

## 12b. M0 findings (2026-11-27)

Measured on macOS with Ardia 0.2.49 and the `ardia` CLI 0.7.0, protocol 4, Ardia closed first and
quit again afterwards. Only `example.com`, `iana.org` and local `file://` test pages were used, in
the Secondary profile; every tab opened was closed. Linux and Windows were not run (no machine).
The raw replies are checked in as fixtures (section 12a).

**Decision (user, 2026-11-27): no auto-launch on Linux and Windows in v1.** The gate does not
start or quit Ardia there; with Ardia closed a tool call fails with "Open Ardia" and the readiness
row shows the same step. No idle quit and no Quit button on those platforms. macOS works as in
section 2a.

Tabs and profiles (section 4):

- `ardia tabs open <url> --profile <id> --json` opens a background tab that belongs to that
  profile, in the last window (whatever profile the window shows), without focusing the window,
  changing its active tab or unhiding Ardia. Reply: `results[0].tabs[0]` is the `tab-...` id.
  **This is the mechanism.**
- Bind with `ardia browse --session <s> tab <tab-id> --json`. Right after opening, the bind can
  fail with `code: "tab_asleep"` (Ardia keeps new background tabs asleep). `tabs.hold {session,
  tabs, seconds}` before the bind loads it; the gate holds, binds, and retries a `tab_asleep` bind
  a few times.
- `ardia browse tab new` and `ardia browse open` with no bound tab open the tab in the **default**
  profile, and `tab new` also made it the window's active tab. `read <url>`, `read-urls`,
  `click --new-tab` and `window new` open tabs of their own too. The gate never sends those forms:
  it creates every tab with `ardia tabs open --profile`, sends `open` only to a bound tab and
  `read` without URLs.
- `ardia browse tab list` lists every tab Ardia can drive (the user's, extension pages), marked
  `openedBySession: false`; a bound tab opened with `tabs open` is also `openedBySession: false`.
  `browser_tabs` filters by the gate's own set (section 4).
- `ardia browse tab close` refuses a tab the session did not open (`command_failed`: "one of the
  user's tabs"), so the gate closes its tabs with `ardia tabs close <tab-id> --json`.
- `tabs.hold {untilDisconnect}` lasts only as long as the connection, and every `ardia call` is
  its own connection, so the gate holds with `seconds` (120) and renews on every call;
  `tabs.release {session}` at cleanup.
- Ardia restores tabs at relaunch (`restoreSession` is on by default), with the same `tab-...`
  ids. A tab left open when Ardia quit comes back, so the gate keeps the ids it opened in its own
  state and closes leftovers on the next live check.
- Session names must match `^[A-Za-z0-9._-]{1,64}$` (`tabs.hold`); the gate uses
  `zptah-<monoId>-<conversationId>` when it fits and a short hash of it otherwise.

Output and exit codes (section 5):

- `ardia browse ... --json` prints `{success, data, error, code?, remedy?}`, exit 0 on success,
  1 on a refused or failed command, 2 when Ardia or the browse daemon can't be reached. Codes
  seen: `unknown_command`, `unknown_ref`, `stale_ref`, `vocab_superseded`, `not_visible`,
  `tab_asleep`, `no_session_tab`, `no_flow`, `failed`, `command_failed`, `daemon_unreachable`.
- `ardia call ... --json` prints the bare result; exit 1 on refusal (message on stderr), 2 when
  unreachable (`ardia: Ardia isn't running, ...` on stderr). `ardia tabs open/close --json` reply
  like `apply`: `{ok, applied, results, tabs, groups, windows}`.
- Arguments (measured 2026-11-28): every option goes before the command and every value after a
  `--`: `ardia browse --session <s> --json <cmd> [flags] -- <values...>`. After `--` everything
  is taken literally, including `--json`, so nothing may follow the values. Without `--` a value
  like `-5` or `--session=x` happened to be filled literally too, but the gate does not rely on
  that. Checked with `fill`, `type`, `press`, `open`, `scroll`, `screenshot`, `get attr`, `click`,
  `upload`, `hover`, `check`, `flow show`.
- Act replies carry `changes` (`document`, `url {from,to}`, `title`, added/removed/changed nodes,
  `network` with the POSTs caused). `fill` into a password field succeeds and reports
  `verify.kind: "password"` after the fact: Ardia itself does not stop it.
- **`--json` output has no `UNTRUSTED_PAGE_CONTENT` markers**; only the text output has them. The
  gate uses `--json` and fences page fields itself with `defense.rs` (correcting section 8).
- "Control is off." was not measured (Control was not switched off on the user's Ardia; it is
  `cliControl` in `settings.schema` and the CLI cannot change it). The string is in Ardia's
  bundle; the fixture assumes `ardia call` exits 1 with it on stderr.
- Ardia has no per-caller restriction of Control (`hello` takes an informational `caller`), so
  the shell block of section 1 stays a guard against mistakes (section 13).

Refs and fields (section 6):

- `@eN` refs are numbered per session and only grow. A ref survives a re-snapshot, a scroll and a
  DOM insert (the new node gets a new ref). After a navigation or reload every older ref fails
  with `vocab_superseded`; a removed element fails with `stale_ref`; an unknown one with
  `unknown_ref`. No stale ref resolved to another element. A ref whose element changed its name
  keeps the ref (`e8` "IANA" became `e8` "Changed"), so the gate still compares role and name
  from the fresh read.
- `snapshot` shows role and name only. `form` lists fields with `type` (`password`, `email`),
  `name`, `label`, `value`, `required` and the submit buttons, not `autocomplete`.
  `get attr @eN type|autocomplete|href|formaction` reads one attribute of a ref
  (`one-time-code`, `current-password` came back). **So the first branch of "Passwords and
  one-time codes" applies**: the gate refuses `browser_fill` and `browser_type` into a field whose
  `type` is `password` or whose `autocomplete` is `current-password`, `new-password` or
  `one-time-code`, and keeps the lexicon rule as a second check.
- The enclosing form's `action` has no ref form; the gate reads the page's forms (`get attr form
  action` and, if there are several, every form's action resolved against the page URL) and
  treats any blocked one as a hit (leans to refusing).

Act inputs (measured 2026-11-28, local test pages):

- `snapshot` never marks the focused element. The gate reads focus with a fixed script of its own
  through `eval --isolated -- '<script>'`. Without `--isolated` the script runs in the page's
  world, and a page that replaced `JSON.stringify` made it report `{"tag":"BODY"}` for a focused
  password field; with `--isolated` it reported `INPUT`/`password`. Every gate `eval` uses
  `--isolated`; the Mono never gets `eval`.
- `press @eN <key>` presses in that element (`{"pressed":"Enter","in":"@e3",...}`) and submitted
  its form; `Control+Enter` is accepted the same way.
- `type`/`fill`/`keyboard type` with a trailing newline did not submit a form on Ardia 0.7.0; the
  gate still judges such text as typing then Enter.
- `check @eN` on a button or a link clicks it (its script ran) and only then fails with
  `command_failed`, so the gate allows check and uncheck only on checkable roles.
- `get count form` and `get attr "form >> nth=<i>" action` read every form;
  `form:nth-of-type(<i>)` misses forms under different parents. An absent attribute is
  `value: null` with exit 0. On a page without forms `form` has no `submits` key.
- With a dialog open, `snapshot -i` is scoped to it and the reply carries
  `modal {role, name, ref}`; the full `snapshot` shows the dialog's text nested under its
  `- dialog "<name>"` line. Quotes inside names are escaped (`"x\" [ref=e99]"`).
- `ardia call windows` reports `focused: true` for Ardia's last focused window even while Ardia is
  hidden and another app is in front.
- A click on a `target=_blank` link (and, by the same path, `window.open`) opened a page target
  that `ardia tabs` does not list (`appTabId: null` in `tab list`, `changes` reports nothing),
  and switched the user's window to the Mono's profile. The gate reads the link's `target`
  before a click and loads such a link in the session's own tab instead (after the origin check);
  after every act it closes any new page target without an `appTabId` (`cdp Target.closeTarget`,
  gate-internal) and puts the window back on the profile it showed before.

Flows (section 7):

- Stored per site, not per profile: `<Ardia data>/Flows/<site>/<name>.json`, shared by every
  profile, session and the user's own terminal. A run uses the session's bound tab, so the logins
  of the Mono's profile.
- Two kinds: data flows (`kind` absent, `writes: false`: GET requests only, returns
  `{list, total, matched, items}`) and page-action flows (`kind: "actions"`, `writes: true`,
  `flow save <name> --actions`). An action flow refuses to run without `--yes` (exit 1, code
  `failed`); the gate passes `--yes` only after its own check.
- `flow show <name> --json` returns `data.flow`: `version` (2), `name`, `site`, `origin`,
  `startUrl`, `hosts`, `params`, `steps[] {seq, command[], url, action {type, intent, candidates,
  fingerprint, value}}`, `kind`, `writes`. Step `action.type` seen: `open`, `click`; `command[0]`
  is an `ardia browse` command (`open`, `find ... click|fill`). The hash covers the `data.flow`
  JSON with `createdAt` kept. Every `startUrl`, `hosts` entry and step `url` is checked against the
  origins. `flow run` returns per-step `ok` but not the final URL; the gate reads `get url` after.
- `flow list --json`: `data.flows[] {site, name, description, params, verified, kind, writes}`.
  Typed passwords are kept as params with no value (`<redacted>` in `flow steps`).

Ardia closed, launch and quit (section 2a):

- Closed: `ardia call hello` and `ardia browse ...` exit 2. Quit mid-call: the running
  `ardia browse` exits 2 with `code: "daemon_unreachable"`.
- `open -g -j -a Ardia` reached `hello` in 1.0-1.5 s again, hidden and not frontmost; tabs opened
  and driven while hidden left it hidden.
- Quit: `osascript -e 'tell application id "com.ardia.browser" to quit'`, gone after about 7 s.
  `warnBeforeQuitting` (default off) would make Ardia ask first; the gate reads it with
  `settings.get {keys}` and does not idle-quit when it is on.
- Telling the user's Ardia from zptah's: not measured live (bringing Ardia forward would have
  taken focus from the user). Signals the gate uses, both leaning to "the user's": an
  `app.activated` or `window.opened` event on an `ardia events app.activated window.opened --json`
  stream kept open while a zptah-launched Ardia runs, and Ardia no longer being hidden (macOS
  `NSRunningApplication.isHidden`) at idle-quit time. Events seen: `tab.opened`, `tab.navigated`,
  `tab.closed` with `{windowId, tabId, profile, url, title}`.

Risk data: `crates/monocode-core` reads the risk lexicon and site table generated into
`extensions/zptah-browser/shared/`. They move to `BD/risk/data/` with their generator
(`gen-risk-lexicon.mjs`, its manual input and its test), and the CI check that the generated
file is in sync moves with them, before the extension folder is deleted (M4).

## 13. Risks

- Ardia is v0.x and third party; the CLI output and socket protocol can change. The gate checks the
  protocol at `hello`, pins the `ardia` output shapes in tests with recorded fixtures, and fails
  closed with a readable message.
- Risk signals are weaker than the extension's DOM signals (section 6).
- Ardia's Control lets any local program drive the browser; that is Ardia's model, not ours. The
  Browser tab says so next to the readiness row, and zptah keeps the window short by quitting an
  Ardia it launched when idle (section 2a).
