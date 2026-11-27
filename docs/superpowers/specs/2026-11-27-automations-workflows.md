# Workflows replace Automations - design

Status: direction approved by the owner on 2026-11-27. Not implemented. Each phase below ships
as its own PR after explicit approval.

## Why

- **Automations** (upstream code, about 6,850 lines) save one prompt, an agent and a project,
  plus a schedule or Inbox event trigger. On the owner's machine there are 2, both disabled, and
  the last run was on 2026-11-05. The one thing worth keeping is the event trigger ("a PR or
  ticket appeared"). Running a single prompt from it is too limited.
- **Mono Workflows** (fork code) have a full engine, board and run graph, but each workflow
  belongs to one Mono. The one real workflow on the owner's machine has 0 runs. The main
  friction: a session node appends "end your final reply with a `workflow-output` JSON block" to
  its first prompt whenever it declares output fields (`mono/workflows/output.rs:43`). Every follow-up the user types into that
  session then gets JSON back, so the session is unpleasant to use by hand.
- **Habits** stay as they are: the simple scheduled Mono prompt the owner uses every day.

Result: Workflows become a global, app-level tool that takes over the Automations entry point
and its event triggers. Automations are deleted.

## Decisions

| Topic | Decision |
|---|---|
| Ownership | Workflows are global (app-level). Monos no longer own workflows |
| Mono access | Monos keep `workflows.*` (create, update, run, read runs) and `@workflow/<slug>` mentions. A workflow a Mono creates is saved paused until the user enables it |
| Old per-Mono workflows | Not migrated. The new code does not read `monos/<id>/workflows.json` or `workflow-runs/`. The files are left on disk untouched |
| Old automations | Deleted (2 disabled entries), with their tables |
| Automation templates | Converted to workflow templates |
| Nodes removed | `askMono`, `report` |
| Node added | `event` trigger |
| Human in the loop | Session node mode "Wait for me". No separate Approve node for now |
| Output when the agent did not write it | Extracted by the session's own harness and model. Never a hardcoded "cheap" model |
| Run report | Automatic at the end of every run (no node) |
| Concurrency | Up to N runs per workflow at once (default 3, 1-10). The rest are queued |
| Workspace | Default: one worktree per run. Each For each branch gets its own |
| Where runs execute | Desktop only. A Mono reached from the web can start a run, which runs on the desktop |
| Canvas | Left to right |

## Non-goals

- Webhooks, a daemon, or running while the app is closed. Event triggers keep the Inbox polling
  they use today.
- Version history or diffs of workflow definitions. Auto-resume after an app restart.
- Editing or running workflows from web or mobile devices directly. A Mono reached from the web
  can still call `workflows.*`, and the run happens on the desktop.
- Changes to Habits.

## Data model

### Storage

Global files in the app data dir, reusing today's format and its atomic hash-checked writes
(`mono/workflows/store.rs`):

- `<app data>/workflows/workflows.json`
- `<app data>/workflows/runs/<workflowId>.json`

Slugs used by `@workflow/<slug>` become globally unique (today they are unique per Mono).
The limit of 20 workflows per Mono goes away. Keep a global cap of 100 workflows. Keep the 20
newest runs per workflow.

### Workflow, new fields

- `workspace`: `{ mode: "perRun" | "none", project, baseBranch? }`. The default is `perRun`.
  `project` is a project path or `{{inputs.X}}`.
- `concurrency`: `{ max: 1-10 }`. The default is 3.
- `decisionModel`: `{ harness, model, effort? }`. Optional (see "If and Switch").
- `reportTo`: `{ mono?: <monoId> }`. Optional.
- `summaryTemplate`: optional. It replaces the Report node's template.

### Triggers

A workflow has 1 to 5 trigger nodes. Today `validate.rs:1206` allows exactly one. Any trigger
node can start a run, and every trigger connects to the same first node. A run's scope gains
`trigger`:

```json
{ "node": "<trigger node id>", "kind": "schedule|manual|event|mono",
  "item": { "source": "clickup", "kind": "issue", "ref": "CU-1287", "title": "...", "url": "...", "repo": "..." } }
```

`item` is set only for event runs.

### Run record, new fields

- `caller`: `{ kind: "manual" | "schedule" | "event" | "mono", monoId?, triggerNode, item? }`.
  It replaces the never-set `RunTrigger::Cli` (`model.rs:842`).
- `worktree`: `{ path, branch, base }` when `workspace.mode = perRun`.
- New run status `queued`. New node status `waitingForUser` (for "Wait for me"). It is distinct
  from the existing `WaitingApproval` (`model.rs:692`), which means a tool-permission wait.

## Nodes

The nodes kept are `schedule`, `manual`, `event`, `session`, `command`, `if`, `switch` and
`forEach`. Loops stay on edges, as they are today.

### Event trigger (new, moved from Automations)

- **Sources and events.** These are the ones Automations supports today
  (`automations/editor.rs:30-56`):
  - GitHub: `draft_opened`, `pull_request_opened`, `issue_opened`
  - GitLab: `merge_request_opened`, `issue_opened`
  - Linear, Jira, ClickUp: `issue_created`
  - Azure DevOps: `pull_request_appeared`, `work_item_appeared`
  - Drop the event names that nothing produces (pull_request_merged, review_requested,
    issue_assigned, check_failed).
- **Filters.** Project, repos or spaces, and branch.
  - The actor filter is dropped. Today it only works for "anyone" (`core/automations.rs:1378`).
- **Source of events.** The Inbox's `ItemsChanged(appeared)`, gated by `NotificationTracker`.
  - Items that existed before the trigger was enabled never fire.
  - The background Inbox starts when any enabled workflow has an event trigger. Move
    `ensure_background_inbox` from `shell/automation_runs.rs`.
  - Latency is 2-5 minutes (`inbox.rs:237`), and only while the app is open. The inspector says
    so.
- **Dedup.** A new table in the session DB, `workflow_event_claims(workflow_id, trigger_node,
  event_key)`, created with `CREATE TABLE IF NOT EXISTS` like `automation_event_claims`
  (`core/automations.rs:209`) and claimed atomically before the run is created.
- **Inspector.** Connected sources first, unconnected ones with a Connect link. The 3 latest
  matching items, each with a "Test run with this item" button.
- **Untrusted input.** Issue and PR text from outside people goes straight into prompts. Session
  nodes downstream of an event trigger default to `auto-accept-edits`, never `full-access`.
  Choosing `full-access` there shows a warning.

### Session

New or changed fields:

- `surface`: `chat` or `terminal`. It uses today's TUI rules (`core/surface.rs:73`). Rename
  `automation_surface` to `session_surface`, because Inbox "start session" uses it too.
- `access`: `supervised`, `auto-accept-edits`, `auto` or `full-access`. The default is
  `auto-accept-edits`. Today workflow sessions always run `full-access`
  (`workflow_runner/host.rs:416`); `full-access` becomes opt-in.
- `project`: optional. It defaults to `workspace.project`. A node may override it with a path or
  `{{inputs.X}}`. Today it is required and must be `{{inputs.X}}` (`model.rs:307`).
- `checkout`: `run` (the default: the run worktree), `root`, `newWorktree`, `worktreeOf`, or
  `existing` (a path).
- `folder` (a session folder) and `linkWorkItem` (link the trigger item on GitHub).
- `mode`: `auto` (the default: the node finishes when the turn ends) or `waitForMe`.
- `outputs`: optional declared fields (see "Passing data"). The chat JSON contract is removed.
- **Cloud targets** (codespace, sandbox) keep working, with limits: the app endpoint listens on
  `127.0.0.1` (`mono/bridge.rs:126`), so a cloud session cannot call `workflow.output`, and it
  gets no local worktree (`workflow_runner/host.rs:433`). Cloud nodes get only the `reply` and
  `sessionId` built-ins plus fallback extraction. Validation rejects `checkout: run` and the
  git built-ins on them.

### Command

- Today a non-zero exit fails the node; only exit 0 produces `exitCode`
  (`workflow_runner.rs:955`). Add `onFailure: "fail" | "continue"`. With `continue`, the node
  finishes with `exitCode`, `stdout` and `stderr`, so an If can branch on a failed test run.
- It runs in the run worktree unless a cwd is set.
- It still does not run on Windows.

### If and Switch

- `decide: "mono"` becomes `decide: "ai"`. It is a one-shot call with no tools, given the
  question, the candidate answers and the scope.
- **Model.** The workflow's `decisionModel` when it is set. Otherwise the harness and model of
  the nearest upstream session node. With neither, validation fails.
- This replaces the `askMono` node. A question with fixed answers is a Switch with `decide: "ai"`.

### Removed nodes

- **`askMono`.** Use Switch with `decide: "ai"`. For a human decision, use "Wait for me".
- **`report`.** Every run ends with a summary built by `compose_report` (`report.rs:497`),
  optionally shaped by `summaryTemplate`. The summary goes to:
  - always: the run page
  - always: a desktop and phone notification
  - when `reportTo.mono` is set: that Mono's chat
  - when a Mono started the run: that Mono's chat

## Passing data between steps

### 1. Built-in outputs (no agent cooperation)

| Node | Fields |
|---|---|
| Session | `reply` (the final reply text), `sessionId`, `worktree`, `branch`, `commit` (HEAD), `changedFiles` (compared with the run base), `diffStat` |
| Command | `exitCode`, `stdout`, `stderr` (capped, as today) |

These are enough for most chains, for example: "Review the changes on `{{build.branch}}`".

### 2. Declared fields via `workflow.output`

- A new app action: `app workflow.output {"approved": true}`. It is called through the app CLI
  that sessions already have.
- **No run id is passed.** The app resolves the target from the caller. Every session gets
  `MONOCODE_APP_SESSION` and a per-session token `sha256(secret:sessionId)`
  (`mono/bridge.rs:168-180`). The app then looks up the run, node and For each branch whose
  active session is that id. So concurrent runs and branches cannot write into each other.
- **Rules:**
  - A session is the active session of at most one node at a time. Reuse only happens across
    loop rounds of one node (`reuseSession`, `model.rs:320`): outputs are cleared at the start of
    each round, and a call after the node (or its round) finished is refused.
  - `MONOCODE_APP_SESSION` is the local numeric id, while run records store persisted ids
    (`remote:...` and similar). The lookup maps between them.
  - A call from a session that is not an active node returns a clear error.
  - Values are type-checked against the declared fields. The call may be repeated, and the last
    value wins per field.
- **Prompt hint.** The first prompt gets one short paragraph naming the fields and the command. It
  does not say "end every reply with JSON".
- **Terminal sessions** do not get the app environment today (only chat sessions do,
  `shell.rs:1762`). Inject it into TUI sessions as part of this work.

### 3. Fallback extraction

- This applies when a node finishes with required declared fields still missing. The app runs a
  hidden one-shot call with **the same harness and model as that session**. It gets the
  transcript (capped) and the field schema, and its answer is parsed with the existing
  `parse_workflow_output`.
- It never re-asks in the user's session (this removes `output_reask`). If extraction fails, the
  node fails with the reason.

### "Wait for me"

- The node goes to `waiting` when the first turn ends.
- Any tool approvals of the session show in the session itself, as for an ordinary session, plus
  a "needs you" notification. No Mono is involved.
- The session shows a banner: "Workflow <name> - step <node> - [Continue] [Fail step]".
- A "needs you" notification goes to the desktop and the phone.
- The user can chat freely. Continue finishes the node, taking outputs from `workflow.output` or
  from extraction over the whole conversation.
- The wait limit reuses the habit approval wait (`APPROVAL_WAIT_MS`, 12 h).

## Workspace per run

- **Run start.** When `workspace.mode = perRun`, the run first creates one worktree from
  `workspace.project` at `baseBranch`. The default is the project's current branch. The branch
  is named `wf/<slug>-<run number>`.
  - Every session and command defaults to it.
  - The scope gains `{{run.worktree}}` and `{{run.branch}}`.
- **For each.** Each branch gets its own worktree, forked from the run worktree's HEAD when the
  branch starts.
- **Validation.** It warns when two nodes that can run at the same time write into the same
  worktree.
- **Concurrent runs** each have their own worktree.
- **Ownership.** The run owns its worktree, not any one session, so the per-session worktree
  removal (`session_worktree.rs:64`) must skip it.
- **After the run.** The worktree is kept (for example, for the PR a `/ship` step opened). The app
  has no merged-worktree cleanup today, so it stays until the user removes it with "Remove
  worktree" on the run page. It is never deleted automatically, because it may hold unmerged
  work.
- **Example.** An "Implement" chat session, then a final terminal session whose prompt is
  `/ship`, both in the run worktree. The TUI types the first prompt into the CLI, so slash
  commands work there. Whether chat sessions accept slash commands is unverified (see "Open
  points").

## Concurrency and queue

- Up to `concurrency.max` live runs per workflow. Today it is one (`workflow_runner.rs:1296`).
- Extra starts become `queued` runs, persisted in the runs file and started first in, first out.
  - At most 50 queued runs per workflow. Beyond that, an event is recorded as a `skipped` run with
    the reason.
  - A schedule occurrence that is due while one is already queued is skipped.
- Stop and resume work per run.
- At startup, `running` runs become `interrupted` (as today). Queued runs stay queued.

## UI

Reference: the HTML mockup reviewed in the design session. It is not checked in.

- **Entry point.** The `AppView::Automations` sidebar entry becomes **Workflows**, with the zap
  icon in the same place.
  - Phase 1 adds Workflows as a new sidebar entry next to Automations. Phase 6 removes
    Automations and renames `sidebar_automations_visible` (`workspace.rs:107`) with a serde alias,
    so the saved preference survives.
  - The Workflows tab in the Mono overview is removed.
- **Layout.** The pane header holds:
  - a breadcrumb with a workflow switcher, the Enabled toggle and Edit | Runs | Settings
  - Run (with a popover for inputs and requirements), Save, and a Focus-canvas toggle

  Below the header:
  - **Inner list:** a 44px rail in Edit mode; the runs list (240px) in Runs mode.
  - **Center:** the canvas.
  - **Right:** a **docked 360px inspector** (not a slide-over).
  - **Bottom:** a Problems bar. Clicking a problem selects its node.
- **Canvas.** Left to right. This changes `monos/workflows_ui/graph/geometry.rs` and the edge
  routing. Triggers sit in one group frame.
  - Opening the canvas fits the whole graph, but never goes below 70% zoom, centered on the
    selected node.
  - Loops are dashed edges with a "≤ N rounds" pill.
- **Inspector.** One form per node type. A session node has an "Available variables" list for
  that node (`trigger.*`, `inputs.*`, `run.*`, upstream outputs and built-ins). Clicking one
  inserts it.
- **Runs mode.** The same canvas, colored by node status, with round badges. The inspector shows
  the node's run details: output, sessions, decision and reason, error and the log tail.
- **Templates.** `automations/templates.json` is converted to workflow templates: a schedule or
  event trigger followed by a session node.
- Sessions started by a workflow get the zap badge in the sidebar. Today the badge comes from
  `automation_state.session_automations` (`rails.rs:538`); Phase 1 adds the workflow-run source
  and Phase 6 removes the automation one.

## Mono integration

- `workflows.*` actions (`workflows/cli.rs:261-273`) now work on the global store.
  - `workflows.guide` is rewritten for the new nodes and data passing.
  - `bridge.rs:81` lists the node types; update it, because it already omits `switch`.
- Runs a Mono starts record `caller.kind = "mono"` and report back to that Mono's chat.
- Habit runs may still call only `list`, `get`, `runs` and `run` (`monos/host.rs:427-442`).
- **Project scope.** A Mono works only on its assigned projects (`workflows/cli.rs:1132`). It may
  `list`, `get` and read `runs` of every workflow, but `add`, `update` and `run` only for
  workflows whose projects are all among its own.
- **Old ids.** A habit that calls `workflows.run` with an old per-Mono id gets a clear error:
  "This workflow was per Mono and is no longer used; recreate it in Workflows". The Mono shows a
  one-time notice about it.
- **Review.** Workflows created by a Mono or imported from YAML keep `needs_review`
  (`workflows/cli.rs:255`). Approving moves from the Mono page (`workflows_ui/page.rs:149`) to the
  global workflow page.
- `@workflow/<slug>` mentions stay.

## Removing Automations

- **Delete:**
  - `crates/monocode-core/src/automations.rs`
  - `crates/monocode-gpui/src/automations.rs` and `automations/` (keep `templates.json` until it
    is converted)
  - `crates/monocode-gpui/src/shell/automation_runs.rs` and `automation_runs/`
  - their tests, and the call sites in `shell.rs`, `app_views.rs`, `sidebar_list.rs`,
    `project_rail.rs`, `rails.rs`, `title_tabs.rs`, `workspace.rs`, `input.rs`,
    `managed_sessions.rs`, `store.rs`, `shell/tui_sessions.rs:176,198` (`automation_tui_gone`,
    `automation_tui_event`), and the module declarations in `main.rs:14` and `core/lib.rs:7`
  - `tui_launch::run_error`, which only `automation_runs/tui.rs:102` uses
  - the bits in shared test files (`classic_surface_tests.rs`, `surface_tests.rs`,
    `tui_sessions/tests.rs`, `tui_session/tests.rs`, `project_removal.rs`)
- **Keep:**
  - `src/features/automations/` (legacy TS), because `monoHabits.ts` imports it (and
    `monoCards.ts` and `workflow.ts` import it transitively, through `monoHabits.ts`)
  - the `sessions.automation_id` column, because the schema is shared. Stop writing it.
- **Data.** The store has no migration framework, and `shared_schema.sql` is never altered. Only
  `core/automations.rs:209` creates the three automation tables. So the store open path in
  `core/store.rs` runs `DROP TABLE IF EXISTS` for `automations`, `automation_runs` and
  `automation_event_claims`, and deletes the `automation:*` and `native-automation-inbox-retries`
  settings. Upstream does not touch the shared schema here, so this does not conflict.
- **Docs.** Update `README.md`, `docs/native/parity.md`, `docs/tui-sessions.md`,
  `docs/native/verification.md`, `docs/agent-access.md`, `docs/jira.md`, `docs/clickup.md` and
  `docs/cloud-dev-environment.md`.
- **Upstream merges.** Add a rule to `CLAUDE.md` "After every merge from upstream": a
  modify/delete conflict on an Automations path keeps the deletion, the same as the Tauri paths.

## Phases (one PR each)

1. **Global workflows.**
   - Move the runner and session start out of `crate::monos`. Today `workflow_session` needs a
     Mono id, the Mono's chat and `mono.works_on(project)`
     (`monos/workflow_runner/host.rs:383-399`), and approvals go to the Mono
     (`workflow_runner.rs:546`).
   - Global store, `AppView::Workflows` as a new sidebar entry, Mono tab removed, review page
     moved.
   - `caller` on runs and the automatic run summary with `reportTo`.
   - Remove `askMono` and `report`, and add `decide: "ai"`.
   - Allow several trigger nodes.
2. **Data passing.**
   - Built-in outputs, `workflow.output` and fallback extraction.
   - "Wait for me".
   - App environment for TUI sessions, and the session `surface` field, so the `/ship` example
     works from this phase.
   - Remove the chat JSON contract.
3. **Workspace and concurrency.**
   - The per-run worktree, For each worktrees and the parallel-writer warning.
   - `concurrency.max` and the queue.
   - `onFailure` on command nodes.
4. **Event triggers.**
   - The event node and its Inbox wiring.
   - `workflow_event_claims`.
   - Session `access`, `folder` and `linkWorkItem`.
   - The untrusted-input default.
5. **Board redesign.** Left-to-right canvas, docked inspector, Runs mode, the variables list and
   templates.
6. **Delete Automations.** Code, tables and settings, plus the docs and the `CLAUDE.md` merge
   rule.

Phase 6 needs phases 1-4, because Automations stay usable until Workflows cover event triggers.
Phase 5 can run in parallel with phases 2-4.

## Open points to verify during implementation

- Whether chat (non-terminal) sessions of each harness accept slash commands such as `/ship`.
- How to get the final reply and the transcript of a terminal session for the `reply` built-in
  and for extraction.
- The `baseBranch` default when the project is on a feature branch.
- Who owns sessions of global workflow runs for web device visibility. Today that ownership
  assumes a Mono (`shell/mobile_host.rs:1108-1131`, H-02).

## Testing

Each phase follows "Developing the native app" in `CLAUDE.md`:

- Run the related tests in one pass with the `native-dev` profile.
- Keep the tests Linux-portable, with `secondary-` keystrokes.
- Use no realistic tokens in tests.

Core tests to add:

- caller resolution and the rejection of non-active sessions
- concurrent runs and the queue order
- the per-run worktree and For each worktrees
- event dedup
- extraction fallback parsing
- `decide: "ai"` model resolution
- that old per-Mono files are ignored
