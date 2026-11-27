# Mono Browser

A Mono can work in your [Ardia](https://github.com/ardia-browser/ardia) browser: it is already
signed in, so the Mono can read and use the sites you use. zptah drives Ardia through its `ardia`
command. It replaces the zptah Browser Chrome extension and the old Playwright MCP backend.
Design: [the spec](../superpowers/specs/2026-11-27-ardia-browser.md).

The Mono never runs `ardia` itself. It reaches the browser only through the `zptah_browser` tools,
and every call passes zptah's checks first: the level, the origin lists, the risk rules and your
approvals. A raw `ardia` call from the Mono's shell is refused for Claude and forbidden in the
instructions for Codex; zptah records it in the audit log if it happens. This is a guard against
mistakes, not a security boundary.

## Prerequisites

1. Install Ardia and its command line: in Ardia open Settings > Plugins > Command line. `ardia`
   must be on your PATH (zptah also looks in `~/.local/bin`).
2. In Ardia turn on Settings > Advanced > "Let the ardia command control Ardia" (Control).
3. In the Mono's settings open the Browser tab, switch the browser on and pick an Ardia profile.

Ardia 0.2.49 with the `ardia` command 0.7.0 (protocol 4) is the version this was built and
measured against. An older protocol is reported as "Update Ardia".

## Readiness

The Browser tab shows the first check that fails, with the step to fix it, and a Try again button.

- Checks that work with Ardia closed: `ardia` on PATH, Ardia installed, a profile chosen. If one
  fails when a Mono starts, the Mono starts without browser tools and its instructions say why.
- Checks that need Ardia running: Ardia reachable, Control on, protocol 4 or newer, the profile
  exists. They run on the first browser call of a conversation. A failure fails that call with the
  check and its fix; it never removes the tools from a running Mono, because Ardia being closed is
  the normal state. The last good result is shown while Ardia is closed.

## Profile and tabs

Each Mono uses one Ardia profile, chosen from the list Ardia reports. Every conversation gets its
own session, with its own background tabs and refs. The Mono only sees and uses the tabs zptah
opened for that conversation (at most 8 stay open); they close when the conversation ends. Your own
tabs are never touched, except when you ask the Mono to work in the tab you have open now: at the
Act level the Mono can ask for that, you approve it with the address shown, and the tab is let go
of afterwards, never closed.

## Levels

Set per Mono in its Browser settings:

- **Read only**: the Mono can open and read pages (snapshots, text, screenshots, console and page
  errors, scrolling, its own tabs) and list and read the flows saved in Ardia. It never clicks,
  types, uploads or runs a flow.
- **Act for me** (default): the Mono can also click, type, fill, press keys, select, check and hover
  for the task, upload files, and run and save flows. It asks first for risky steps (see below).

There is no "ask before every action" level.

## What asks

zptah reads the target again right before every step. If the element changed since the snapshot the
Mono used, the step fails and the Mono takes a new snapshot. Then it decides:

- Risky steps, such as payments, deleting data, sending or posting, account changes, sharing and
  accepting terms, show an approval card in the Mono's conversation, in English or Vietnamese like
  the app. Buttons: Allow once, Allow in this conversation (not offered for payments or
  credentials) and Don't allow. A banking or payment site asks before its first read. Blocked sites
  are never opened, and when you set allowed sites the Mono stays inside them.
- When zptah cannot read enough of a page to tell what a step does, it asks too ("zptah could not
  read enough of this page to tell what this does"), and that answer is never kept for the
  conversation.
- The Mono never types passwords or one-time codes. zptah refuses it, with no approval that
  overrides it, and on a login page it asks before typing into a field it cannot identify.
- Navigation a page starts by itself (a script, a redirect) to a blocked site is caught after the
  call: the Mono's tab is closed, the turn's further browser calls stop and the audit log records it.
- Page content is data, never instructions: it is fenced, scanned for prompt injection, and a turn
  that read untrusted content asks before acting on anything it did not declare first.

## Uploads

`browser_upload` always asks, at every level and with no "allow in this conversation". The card
shows the exact path and size of each file. Allowed files are inside the Mono's project or output
folder, or a path you gave in the conversation. Dotfiles and dot folders, `~/.ssh`, `~/.aws`,
`~/.gnupg`, keychains and the app's own folders are never uploaded. zptah uploads a private copy
that it deletes when the conversation ends.

Where there is no private folder for those copies (Windows, and Linux without `XDG_RUNTIME_DIR`),
uploads are not available: the tool is not offered and the Browser tab says "Uploads are not
available on this system".

## Flows

Flows are sequences Ardia records and replays. The Mono can list and read them (`flow_list`,
`flow_show`) at any level. At the Act level it can run one (`flow_run`) and save the steps of its
session as one (`flow_save`, which always asks). Before a run zptah reads the flow, classifies each
step with the same risk rules, checks every address it names against your origin lists and includes
the parameters in the check. A run that could change something asks once for the whole run, and the
approval covers exactly that content: if the flow changes before the run, it is checked again. A
flow with a password, an upload step or any file path is never run.

## Stop

There is no glow or Stop button in the page. While a Mono works in Ardia, its conversation shows a
"Working in Ardia" row with Stop. Stop ends the `ardia` call in flight, releases the tabs, withdraws
open approval cards and refuses further browser calls for that turn. Your next message starts
again.

## Opening Ardia and the exposure window

Ardia is usually closed. On macOS zptah opens it in the background, hidden and without taking
focus, the first time a browser call needs it, and waits up to about 20 seconds for it to answer.
On Linux and Windows zptah does not open Ardia: open it yourself. A browser call while it is closed
fails and tells the Mono that Ardia is not running.

While Ardia runs with Control on, any local program can drive its logged-in profile through the
`ardia` command, not only zptah. That is how Ardia's Control works. So zptah keeps the window short:

- An Ardia that zptah started shows "Ardia running (started by zptah)" in the Browser tab and in
  the conversation, with a Quit button that closes the Mono's tabs and quits Ardia at once.
- zptah quits an Ardia it started after it has been idle for 10 minutes (a setting in the Browser
  tab) with no browser call and no running turn that used the browser.
- An Ardia you started, or one you took over by bringing it to the front or opening a window in it,
  is never quit by zptah. Only the Mono's own tabs are closed.
- zptah also asks an Ardia it started to quit when you quit zptah.

## Audit log

Every browser call is recorded locally: the tool, the address (origin and path, never the query or
fragment) and a short summary. Typed text, field values and selected options are never recorded, and
secrets in page content are masked. The log stays on your computer, for 90 days or 20,000 entries
per Mono. The Browser tab shows it.

## Troubleshooting

- **Ardia isn't running, open it and try again**: open Ardia (Linux and Windows) or press Try again;
  on macOS zptah opens it itself.
- **Control is off**: in Ardia, Settings > Advanced > "Let the ardia command control Ardia".
- **`ardia` not found**: install the command line in Ardia, Settings > Plugins > Command line, then
  make sure it is on your PATH.
- **Update Ardia**: the browser protocol of your Ardia is older than 4.
- **The profile is gone**: pick a profile again in the Mono's Browser tab.
- **A Mono from the extension era**: its old Chrome settings are ignored. Pick an Ardia profile.
