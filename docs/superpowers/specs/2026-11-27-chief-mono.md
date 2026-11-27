# Chief: one assistant above the Monos - design

Status: spec for review, not scheduled. Native app first.
Source: planning session 2026-11-27, revised once after an independent review. Line references
are to `main` at 3605151d.

## Goal

The owner (the user) talks to one assistant, the **Chief**. The Chief sits above the Monos, and
the Monos sit above the sessions they run:

```
Owner
  └─ Chief            (one per app; talks only to the owner)
       ├─ Mono A      (owns its project(s), its launches, its memory)
       │    ├─ session 1
       │    └─ session 2
       └─ Mono B
            └─ session 3
```

Two jobs:

1. **Status on demand.** "What is going on?", "Is the auth refactor done?" The Chief looks
   only when asked. Monos and sessions never push anything to the Chief.
2. **Command a session through its Mono (phase 1).** "Tell the login session to also add tests."
   The Chief does not send to the session. It hands the command to the Mono that owns the
   session, and that Mono sends it. The Mono stays the only voice its sessions hear, so its
   context (what it asked, what came back) stays complete.

## Non-goals (phase 1)

- The Chief starting new sessions, creating Monos or editing a Mono's soul, habits or workflows.
- The Chief running on a schedule (no heartbeat, no habits): it only acts when the owner asks.
- Monos or sessions sending anything to the Chief.
- Web and phone access, the tray entry, a History page (phase 2).
- Commands that skip a Mono and go straight to a session.

## What exists today (research)

**Monos.**
- A Mono is an entry in the `Roster`, one JSON blob under the settings key `mono-roster`
  (`crates/monocode-core/src/mono.rs:339-385`; desktop reconcile in `mono/desktop.rs:24`). Its
  chat is an ordinary hidden `Conversation` in `$HOME` with `AccessMode::Auto` and low effort
  (`crates/monocode-gpui/src/monos/mod.rs:897-919`). Soul and memory files live under
  `<appdata>/monos/<id>/` (`mono/files.rs:1-20`, `load_files` at `mono.rs:430-438`).
- Prompt: standing rules from `mono::instructions` (`mono.rs:462-505`) plus a per-turn
  `<monocode_context>` with soul and memory (`mono.rs:534-555`). No summarising compaction: at
  80% context the provider session is rotated and gets a `rotation_brief`
  (`mono.rs:564-612`, `gpui/runtime.rs:505-567`).
- Tools: one MCP tool `app {action,input,requestId}` on a loopback bridge with a per-session
  token (`mono/bridge.rs:125-231`, `389-405`). Dispatch: `Monos::app_call`
  (`gpui/monos/host.rs:829`) → `app_caller`, which resolves the calling chat to a roster Mono
  and applies context checks (`host.rs:384-459`) → `mono::api::validate`
  (`core/mono/api.rs:31-330`) → idempotency receipts → `app_call_ready`. No `monos.*` actions
  on `main`.
- Much of the chat runtime assumes a roster Mono: the outbox and `chat_changed` look up
  `roster.get(id)` (`mod.rs:1151-1172`, `1290-1345`), `configure_mono(Mono)`
  (`conversation/mono.rs:174`) and `MonoContext.mono` (`agent.rs:182`) feed instructions and
  files, the bridge tool text is the fixed Mono help (`bridge.rs:392`), and a browser MCP server
  is added when a browser database is set (`conversation/mono.rs:197-205`).

**Ownership.** A Mono owns a session only through `Mono.launches: Vec<Launch>`
(`mono.rs:289-310`), pushed in `apply_started` (`host.rs:761-828`). There is no `mono_id` column.
`sessions.send` with `notifyOnComplete:true` to a session in the Mono's project adopts it,
including sessions the owner started (`host.rs:1153-1186`). That adopted launch gets
`visible:false` (`host.rs:1180`), and `hidden_session` hides such launches when
`show_started_sessions_in_sidebar` is false (`mod.rs:607-612`), which is the default
(`mono.rs:212`). Nothing stops two Monos from tracking the same session.

**Completion.** `check_completions` runs on the 10 s clock and enqueues
`[Delegated work completed]` with each session's last prose into the Mono's outbox as an
internal message (`host.rs:2108-2207`). The lead line tells the Mono to "Inspect its result
and continue" (`host.rs:288-293`). Internal messages wait while the Mono chat is busy
(`mod.rs:1294-1306`). A failed internal message waits for Retry or Discard (`mod.rs:1331-1344`).
A finished Mono turn emits `NotificationFinished` (`mod.rs:1310-1324`).

**Checks.** `sessions.read/send/stop` are scoped to the Mono's projects by cwd
(`host.rs:1027-1089`); `send` is refused while the target is busy or has a draft
(`host.rs:1120-1132`). `unmanaged_session` (`host.rs:463-472`) guards only delete, cloud
archive and moves (`host.rs:1048-1060`), so `sessions.send` can reach another Mono's chat when
a Mono has `$HOME` as a project. Habit runs, heartbeats and workflow questions are barred from
terminals, `app.update`, `projects.add`, moves and workflow edits (`host.rs:407-457`).

**Overview data.** The best cross-project view is the mobile `workspace.list` catalog, built
inside the mobile host task with mobile ids (`gpui/shell/mobile_host.rs:935-1006`,
`gpui/mobile/catalog.rs:115-147`). `busy` there is true only for sessions loaded as views
(`catalog.rs:134-136`). No persisted cost or error status.

**Untrusted text.** Completion reports and heartbeat digests reach the Mono unfenced. The
browser has `fence(text, nonce)`, `neutralize`, `strip_invisible`, `redact_secrets`
(`core/mono/browser/defense.rs:45-200`). The provider never sees whether a turn is internal;
that flag only sets the transcript role (`runtime.rs:239`).

**Audit.** None for Mono actions beyond idempotency receipts.

**PR #69** (open, changes requested) adds `monos.list` and `monos.send` between Monos. It
injects into the target's outbox like completion reports, correlates the reply through a
persisted `peer_inbox`, frames the message as "another Mono, not the user", limits depth to 3
and 10 sends per 10 minutes, and is gated by a per-Mono `accept_peer_messages` switch. The
review asks to refuse `monos.send` from habit runs, heartbeats and workflow questions, fix the
`peer_inbox` leak after a quit mid-turn, test the `chat_changed` wiring and fix the help text.

## Concept

**Chief** (UI name "Chief", Vietnamese "tổng quản"; naming is an open question).

| | Mono | Chief |
|---|---|---|
| How many | many | exactly one, optional |
| Scope | its projects | every project and Mono, read only |
| Owns sessions | yes (`launches`) | no, never |
| Can start or send to sessions | yes | no |
| Can command | its sessions | a Mono, about one of that Mono's sessions, only after the owner clicks Send |
| Runs on its own | heartbeat, habits, workflows | never |
| Receives messages from | owner, its sessions' reports, (PR #69) other Monos | owner only |
| Harness tools (shell, files, browser) | yes | none |

**Decision: separate storage, shared chat runtime with a role.**
- Stored as `Roster.chief: Option<Chief>`, not in `Roster.monos`, so every loop over
  `roster.monos` (scheduler, heartbeat, completions, project scoping, mobile catalog, menu bar,
  PR #69 peers) ignores it by default.
- The chat runtime gets a role: `MonoContext` carries `Role::Mono(Mono) | Role::Chief(Chief)`
  (or a sibling `ChiefContext`), which selects instructions, tool text, files root and "no
  browser server".
- App calls from the Chief chat go to a `chief_call` dispatcher that runs before `app_caller`
  and refuses every action not on its list. It does not borrow `app_caller`'s Mono lookup.
- The Chief has its own small outbox for owner messages; it never receives internal messages.
- Cost acknowledged: this touches `conversation/mono.rs`, `runtime.rs`, `monos/mod.rs` and
  `host.rs`. The alternative, a Mono with a `chief` flag, needs fewer type changes but a
  "skip the Chief" check in every roster loop; a missed one hands the Chief projects, habits or
  sessions. We prefer the compile-time separation.

**No harness tools.** The Chief chat runs with no shell, file or browser tools, only the `app`
MCP tool. Otherwise text injected through a session read could drive shell commands with no
click, or read transcripts directly past the fence. The first implementation task is a spike
proving each harness (Claude, Codex) can run with its own tools off and the `app` tool on
(likely `AccessMode::ReadOnly` plus an empty tool allowlist; not yet verified), with a test
that the Chief cannot run a shell command. If a harness cannot do this, the Chief does not
support it.

## Workflows

### W1. Ask status

1. Owner: "What's running?" in the Chief chat.
2. Chief calls `chief.overview {project?, mono?, status?}`. A shared builder, extracted from the
   mobile `workspace.list` code, returns projects → sessions (id, title, project, branch,
   harness/model, busy, waiting, updatedAt, cloud state, owning Mono(s)) and Monos (status,
   projects, last line). Archived and hidden sessions are left out. Default: active in the last
   7 days, at most 50 rows, with a count of what was cut. `busy` is exact only for loaded
   sessions; others report last activity.
3. For detail it calls `chief.readSession {sessionId}`: its own read path (not the
   project-scoped `sessions.read`), last 3 prose exchanges, 4k characters each.
4. Every string from sessions and Monos is passed through `strip_invisible`, `redact_secrets`
   and `fence` before the Chief sees it.
5. Nothing is written into any Mono's chat; no Mono spends tokens.

### W2. Command a session through its Mono

1. Owner: "Tell the login session to add tests for the expired-token case."
2. Chief resolves the session (see below) and calls
   `chief.propose {sessionId, instruction, mono?}`.
3. The app checks the route: the session exists, is not archived, hidden, a Mono chat or the
   Chief chat; its owning Mono (rules in W3); that Mono's chat is available; the session is in
   that Mono's projects (or is its cloud twin). It stores a **proposal** and draws a card in the
   Chief chat from the stored record:
   `Mono A → session #412 "Login refactor" (project web, branch feat/login)` and the full
   instruction text (invisible characters stripped, never clipped), with **Send**, **Edit** and
   **Cancel**. Warnings: session busy, Mono busy (will queue), cloud sandbox asleep (waking
   costs money), Mono will adopt this session.
4. The tool returns `{proposalId, status:"awaitingOwner"}` and the Chief ends its turn. **The
   model cannot confirm; only a click in the app does.**
5. On **Send** the app checks the route again, then enqueues an internal message into Mono A's
   outbox:

   ```
   [Command from the owner, relayed by the Chief]
   Proposal: p_7f3a  Session: #412 "Login refactor" (project web)
   The owner confirmed this text:
   <instruction>
   Send it with sessions.send {session: 412, proposalId: "p_7f3a", notifyOnComplete: true}.
   ```

6. Mono A handles it on its next turn (now, or when its current turn ends). It sends the text
   to the session with `sessions.send` carrying `proposalId`. It may add context before or
   after the text, but the confirmed text must appear in the prompt unchanged.
7. **The app enforces the authority, not the model.** `sessions.send` with a `proposalId` is
   accepted only when the proposal is confirmed, unused, for this Mono and this session, and
   the prompt contains the confirmed text verbatim. Then the proposal is marked used. The Mono
   rules say owner authority exists only when that send is accepted. A fake header anywhere
   else (in a session's reply, a completion report, a peer message) carries no proposal the app
   will accept.
8. The launch records `origin: Chief(proposalId)`. When the session stops, the completion
   report for that launch uses a different lead: "An owner command relayed by the Chief has
   finished. Report the result in one line. Do not continue unless the owner asks." This
   keeps the Mono from following up on its own.
9. **Nothing is pushed to the Chief.** The owner later asks "Did it finish?" and the Chief calls
   `chief.commands {}` (proposal states) plus `chief.overview` / `chief.readSession`.

**Proposal states:** `awaitingOwner` → `cancelled` | `queued` (in the Mono outbox) →
`sent` (the proposal-bound send was accepted) | `refused(reason)` (the send was refused, for
example the session is busy) | `notAttempted` (the Mono's turn ended without a proposal-bound
send) | `failed(reason)` (the internal message errored, was discarded, or the Mono was reset or
deleted). The Chief reads the session's later state live.

**When the Mono's send is refused,** its rules say: do not stop the session, retry, draft or
start another session; reply with the reason in one line. The app refuses a second send for the
same proposal. The owner retries through the Chief.

**Notifications.** The owner is not notified about the Mono's relay turn (`NotificationFinished`
is suppressed for turns that carry a Chief command). The Mono's completion report turn notifies
as any Mono turn does today; that is a notice to the owner, not a push to the Chief.

### W3. Special cases

| Case | Behaviour (phase 1) |
|---|---|
| Session has no owning Mono (the owner started it, or the Mono dismissed it) | If exactly one Mono covers the session's project, the card says "Mono A will adopt this session". If several do, the Chief asks the owner which. If none does, the Chief refuses and explains. No direct send. An adopted launch gets `adopted: true` and stays visible in the sidebar (today it would be hidden, `host.rs:1180`, `mod.rs:607-612`). |
| Two Monos track the same session | Default to the one with the most recent active launch; the card names it; the owner can say "no, Mono B" and the Chief proposes again. |
| Several sessions match the words | The Chief asks, listing candidates. It proposes only after one is chosen. |
| Mono busy | The command waits in the Mono outbox (existing). Card: "Queued: Mono A is busy". |
| Mono chat missing (`unavailable_session`) | Refused at propose and at Send. |
| Target session busy or has a draft | Card warns. If still busy at send time the send is refused → `refused`. |
| Cloud session | Allowed when the Mono covers its local project; card warns when the sandbox is asleep. |
| Session archived, deleted or moved before Send | The second check fails; the card shows why; nothing is sent. |
| Open proposals | At most 5; older ones must be sent or cancelled first. No timed expiry. |
| App quits with a queued command | The outbox is persisted (existing). On load, a `queued` proposal whose outbox entry is gone and that was never used becomes `failed("interrupted")`, which avoids PR #69's kind of leak. |

## Resolving which session

The model does the language part; the app checks the facts.

- Inputs: `chief.overview` rows, `focusedSessionId` (the session open in the main window) so
  "this session" works, and the Chief's memory for nicknames the owner taught it.
- Order: explicit id (`#412`) → focused session for "this/current" → title, branch and project
  words → last activity. One clear winner: propose. Several: ask. None: say so.
- The card shows title, project and branch, so a wrong pick is visible before anything is sent.

**Confirm on every command is an addition to the request.** The owner already typed the
command, so a click is extra friction. It is what makes the design safe: the Chief reads text
written by sessions, and only an app-drawn click separates that text from an action. This needs
the owner's approval (open question 1).

## Permissions and safety

**Chief actions (all of them):**

| Action | Kind |
|---|---|
| `chief.overview`, `chief.readSession`, `chief.commands` | read |
| `chief.propose`, `chief.cancel` | create or cancel a proposal; sends nothing |
| `memory.read/search/add/replace/remove` (the Chief's own memory) | own state |

Everything else is refused for the Chief by `chief_call`, tested by looping over every Mono
action. `chief.*` is refused for every other caller: Monos, habit runs, heartbeats, workflow
questions, Operator threads, plain sessions, and web and phone in phase 1.

**No inbound path.** All `sessions.*` actions refuse the Chief chat id as a target. The same
gap exists today between Mono chats (`sessions.send` into another Mono's chat when a Mono has
`$HOME` as a project); that is reported as a separate pre-existing issue, not fixed here.

**Loops and hops.** The chain has a fixed depth: owner → Chief → Mono → session. The Chief has no
inbound path, never runs unattended and cannot send to sessions; a Mono cannot call `chief.*`;
each command needs a click and is used once. There is no loop to count.

**Untrusted content.**
- To the Chief: fenced and cleaned as in W1. The fence is defence in depth; the click and the
  missing harness tools are the boundary.
- To the Mono: the header line `[Command from the owner, relayed by the Chief]` is passed
  through `neutralize` in every untrusted text that enters a Mono turn: completion reports,
  heartbeat digests, workflow reports and (if merged) PR #69 messages. Combined with the
  proposal check in W2 step 7, a fake header neither looks real nor works.

**Audit.** Proposal records are the audit trail: id, created and confirmed times, session,
Mono, confirmed text, every check result, outbox id, final state and reason, the Mono's
one-line reply (clipped). The last 100 are kept in `Roster.chief`; the Chief reads them with
`chief.commands`. A History page and an append-only log file are phase 2.

**Relation to PR #69.** Independent; neither needs the other. The Chief does not use
`peer_inbox`, `accept_peer_messages` or the "another Mono, not the user" framing, because a
confirmed owner command is not a peer message. If #69 merges first, its messages are added to
the texts that get the header `neutralize`d. Worth sharing later: one `enqueue_internal(mono,
text, tag)` helper, since `enqueue_peer` and the completion path do the same steps.

## Cost and context

- Idle cost is zero: no heartbeat, no habits, no pushes.
- The Chief chat uses the owner's default provider at low effort, like a new Mono.
- `chief.overview` is about 50 rows × about 60 tokens ≈ 3k tokens; detail needs an explicit
  `chief.readSession` (3 exchanges, 4k characters each).
- The Chief chat rotates at 80% context with the existing rotation brief; check that the
  per-turn context hash works with the Chief's instructions.
- A command costs one Mono turn for the relay, one session turn, and one short Mono turn for
  the completion report.

## UX (phase 1)

- The Chief is pinned first in the Monos rail of the floating panel and the Monos dashboard,
  with its own mark, and opens the same chat view.
- Settings: one "Chief" switch (off by default while experimental) and its model and effort.
- Results come back in the Chief chat. Proposal cards update in place as their state changes.

## Data and migration

- `Roster.chief: Option<Chief>` with `#[serde(default)]`: `{ session_id, model, effort,
  account, proposals (last 100) }`. Check that the desktop reconcile (`mono/desktop.rs:24`)
  keeps the field.
- Downgrade: an older build reads the roster fine but drops `chief` on its next save, losing
  the Chief's chat link and proposals. Acceptable for an experimental feature; say so in the
  release note.
- Files under `<appdata>/chief/` (MEMORY.md; provider files), a separate root from
  `<appdata>/monos/<id>/`. `load_files`, emoji and `provider_files` paths take the root as a
  parameter.
- `Launch` gains `origin: Option<LaunchOrigin>` and `adopted: bool`, both `#[serde(default)]`.
- No database migration. Turning the Chief off keeps its files; deleting it asks first.

## Impact on existing features

- Hidden-session handling must include the Chief chat: sidebar, Mono `sessions.list`,
  heartbeat digests, mobile `workspace.list`, notifications (`shell/notifications.rs`), Today.
- `app_caller` behaviour for Monos is unchanged; current refusal tests must still pass.
- `sessions.send` gains an optional `proposalId`; without it nothing changes.
- Completion reports change only for launches with a Chief origin.
- Adoption visibility changes only for Chief-driven adoptions (`adopted: true`); plain Mono
  adoption keeps today's behaviour.
- Mono instructions gain one section; untrusted texts entering Mono turns get one more
  `neutralize` pass.

## Phased rollout

1. **Phase 1 (this request).** Chief entity and chat with no harness tools, `chief.overview`,
   `chief.readSession`, `chief.propose/cancel/commands`, confirm card, routing through the
   owning Mono with proposal-bound send and adoption, proposal audit, fencing, switch off by
   default. Desktop only.
2. **Phase 2.** Tray "Ask Chief…", web and phone (Send needs a Control device and a desktop
   toggle that defaults off, like remote approvals), History page and log file,
   `chief.askMono` (a Mono answers from its own context, costs a turn), queue until a busy
   session is idle, overview text search.
3. **Phase 3.** Route "start new work on X" to the right Mono (the Mono starts the session);
   on-demand digests ("what did everyone do today"); renaming and a soul for the Chief.
4. **Phase 4, only if wanted.** Per-Mono standing rules for low-risk commands without a click.
   This removes the main safety boundary and needs its own design and review.

## Test strategy

Core (`monocode-core`):
- `owner_of(session)`: none, one, two Monos, dismissed launch, cloud twin, archived.
- `chief` field validation for every `chief.*` action.
- Proposal lifecycle: create, edit, cancel, cap of 5, second-check failures, every terminal
  state, load-time `queued` → `failed("interrupted")`.
- Proposal-bound send check: wrong Mono, wrong session, used twice, text altered or missing.
- Command text builder; `neutralize` of the header in completion reports and digests; fence
  output with nested fake fence tags.
- Roster round trip with and without `chief`, including the desktop reconcile; `Launch`
  round trip with old JSON.

GPUI (Linux-portable, `secondary-` keystrokes):
- The Chief cannot run a shell command; its chat has no browser server.
- `chief_call` refuses every non-Chief action; Monos, habit runs, heartbeats and Operator are
  refused `chief.*`; `sessions.*` refuse the Chief chat as a target.
- Send click: second check, enqueue into the right Mono outbox; busy Mono queues; missing Mono
  chat refused.
- Through `chat_changed`: proposal reaches `sent`, `refused`, `notAttempted` and `failed`
  (including a failed internal message that is discarded).
- Adopted session stays visible; Chief-origin completion uses the one-line lead; relay turn
  sends no notification.
- The Chief chat is absent from Mono `sessions.list`, heartbeat digest and mobile catalog.

Manual: a real Mono with a real session; command while the Mono is busy; while the session is
busy; an owner-started session adopted; a cloud session asleep; a session whose reply contains
a fake Chief header.

## Risks

- **A harness cannot run with its tools off.** Then that harness cannot host the Chief;
  found by the first spike, before any other work.
- **Wrong session.** The card shows title, project and branch before Send.
- **The Mono rewrites the command.** The app requires the confirmed text verbatim in the
  proposal-bound send.
- **The owner expects reports without asking.** By design there are none; proposal states and
  phase 2 options cover this.
- **Hidden-session leaks** if one list misses the Chief chat. One test per list.
- **Runtime refactor** (`Role` in the Mono context) touches shared Mono code; existing Mono
  tests must pass unchanged.

## Open questions for the owner

1. Confirm every command with a click (recommended; this is the safety boundary), or send
   straight away once the Chief is sure?
2. Name in the UI: "Chief", "Tổng quản", or a name the owner picks?
3. Status: read sessions directly (cheap, Mono context untouched, recommended), or also ask
   the owning Mono (richer, costs a Mono turn)?
4. Session with no owning Mono: let a Mono that covers the project adopt it (recommended), or
   refuse?
5. May the Chief phrase the command, or must it pass the owner's words as typed? Either way the
   card shows the exact text before Send.
6. Target session busy: tell the owner (phase 1), or queue until it is idle?
7. Should Mono-to-Mono messages (PR #69) stay a separate feature?
