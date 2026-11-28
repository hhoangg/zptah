//! Drives workflow runs: the TS `executeRun` (`workflowRunner.ts`) on the pure engine.
//!
//! A run is one async task on the foreground executor. It asks the engine what is ready
//! (`next_actions`), executes it through a `WorkflowHost` (sessions, commands, AI decisions, the
//! summary), feeds the results back (`apply_event`) and writes the newest state after every
//! event, until the engine says the run is over. It never panics: whatever goes wrong becomes
//! a failed node or a failed run.
//!
//! The host is a seam so the logic runs against a fake in tests; `host.rs` has the real one.
//! Scheduling (which workflow runs when, requests in the workflow files, the startup sweep)
//! is not here: it calls `WorkflowRunner::start`, `resume` and `stop`.
use gpui::AsyncApp;
use monocode_core::cloud_sessions::CloudBackendKind;
use monocode_core::mono::workflows::{
    active::{ActiveNode, find_active, record_output as record_active_output},
    cli::Signals,
    engine::{
        EngineAction, EngineEvent, apply_event, is_asking, next_actions, resume_run, run_scope,
    },
    event::work_item_of,
    model::{
        Checkout, Cwd, DEFAULT_COMMAND_TIMEOUT_MINUTES, DEFAULT_SESSION_ACCESS,
        DEFAULT_SESSION_TIMEOUT_MINUTES, Node, NodeKind, NodeRun, NodeStatus, OutputField,
        RunInstance, RunStatus, RunSummary, RunWorktree, SessionMode, SessionTarget,
        TEMPLATABLE_TARGET_OPTIONS, TargetInstance, WorkflowRun, parse_branch,
    },
    ordered::{Json, OrderedMap, js_number_text},
    output::{check_output_value, output_hint, parse_workflow_output},
    report::{compose_report, decision_prompt, parse_decision},
    run_worktree::{branch_key, slug_of},
    template::render_template,
    validate::{DecisionModelSource, decision_model_for},
};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeSet, HashMap, HashSet},
    future::Future,
    pin::Pin,
    rc::Rc,
    sync::Arc,
    time::Duration,
};
use sync::{Either, Gate, Nudge, Signal, join_all, race};

mod api;
pub(in crate::workflows) mod host;
mod instances;
mod sync;
#[cfg(test)]
mod tests;
mod text;

pub(crate) use api::*;
pub(crate) use host::Live;
pub(in crate::workflows) use host::model_harness;
#[cfg(test)]
pub(crate) use host::{trusting_preflight, trusting_probe};
use text::{cap_map, cap_string, clip, utf16_tail, workflow_folder_name};

pub(crate) type Fut<T> = Pin<Box<dyn Future<Output = T>>>;

const STOPPED_BY_USER: &str = "Stopped by the user.";
/// A cancel can beat the host registering the command, so it is retried briefly.
const CANCEL_ATTEMPTS: usize = 10;
const CANCEL_RETRY_MS: u64 = 50;
/// What the user answered a step in "Wait for me" with, by (node, branch). It is taken out on the
/// first answer, so a step settles once however often Continue is pressed.
type Waits = RefCell<HashMap<(String, String), async_channel::Sender<WaitChoice>>>;
/// How much of a failed command's output goes into the node error.
const COMMAND_DETAIL_MAX: usize = 500;

fn now() -> u64 {
    monocode_core::time::now_ms()
}

/// What a session step is, as far as finishing it goes.
#[derive(Clone, Copy)]
struct SessionKind {
    mode: SessionMode,
    /// The step runs the provider's CLI: no `reply`, no extraction.
    terminal: bool,
    /// The step runs in a cloud instance: no git built-ins.
    cloud: bool,
}

/// The declared fields `have` has no value for yet.
fn missing_fields(
    declared: &OrderedMap<OutputField>,
    have: &OrderedMap<Json>,
) -> OrderedMap<OutputField> {
    declared
        .iter()
        .filter(|(name, _)| !have.contains_key(name))
        .map(|(name, field)| (name.clone(), field.clone()))
        .collect()
}

/// `{name: value}` as a JSON object, to check one value against its declared type.
fn single(name: &str, value: &Json) -> Json {
    let mut one = OrderedMap::new();
    one.insert(name, value.clone());
    Json::Object(one)
}

// --- one run ------------------------------------------------------------------------------------

struct State {
    run: WorkflowRun,
    command_seq: u32,
    /// Ids of the commands this run has started and not yet heard back from.
    commands: BTreeSet<String>,
    dirty: bool,
    writing: bool,
}

struct Exec {
    host: Rc<dyn WorkflowHost>,
    cx: AsyncApp,
    workflow_id: String,
    limits: Limits,
    /// Tells this execution's command ids from those of an earlier one of the same run.
    nonce: String,
    state: RefCell<State>,
    /// Node tasks still going.
    active: Cell<usize>,
    /// Settles when the user stops the run, so waits on a session end at once.
    stop: Signal,
    stopping: Cell<bool>,
    stopped: Signal,
    nudge: Nudge,
    flushed: Nudge,
    waits: Waits,
    /// Sessions start one at a time so the first makes the run's folder and the rest reuse it.
    starts: Gate,
    /// Worktrees of For each branches are made one at a time, so a branch gets exactly one.
    trees: Gate,
    registry: Registry,
    signals: Arc<Signals>,
    /// The app is quitting: the run writes nothing more, so the file keeps saying
    /// `running` and the next start interrupts it (resumable) instead of failing or stopping it.
    quitting: Cell<bool>,
}

/// The live runs by run id: a workflow may have several going.
type Registry = Rc<RefCell<HashMap<String, Rc<Exec>>>>;

impl Exec {
    fn live(&self) -> bool {
        !self.quitting.get() && self.state.borrow().run.status == RunStatus::Running
    }

    /// The app is going down. Waiters end at once and the run is no longer `live`, so whatever the
    /// sessions and commands report from now on changes nothing; `save` writes nothing more.
    fn quit(&self) {
        if self.quitting.replace(true) {
            return;
        }
        self.stop.fire();
        self.nudge.nudge();
    }

    fn sleep(&self, ms: u64) -> gpui::Task<()> {
        self.cx
            .background_executor()
            .timer(Duration::from_millis(ms))
    }

    // --- persistence: one write at a time, always the newest state -----------------------------

    fn save(self: &Rc<Self>) {
        if self.quitting.get() {
            return;
        }
        {
            let mut state = self.state.borrow_mut();
            state.dirty = true;
            if state.writing {
                return;
            }
            state.writing = true;
        }
        let this = self.clone();
        self.cx.spawn(async move |_| this.flush().await).detach();
    }

    async fn flush(self: Rc<Self>) {
        loop {
            let latest = {
                let mut state = self.state.borrow_mut();
                if !state.dirty {
                    state.writing = false;
                    break;
                }
                state.dirty = false;
                state.run.clone()
            };
            let id = latest.id.clone();
            if let Err(error) = self.host.save_run(latest).await {
                eprintln!("Could not save workflow run {id}: {error}");
                // The run goes on in memory; the record says its history is incomplete.
                let mut state = self.state.borrow_mut();
                if !state
                    .run
                    .error
                    .as_deref()
                    .is_some_and(|e| e.starts_with("Could not save"))
                {
                    let note = format!("Could not save the run history: {error}");
                    state.run.error =
                        Some(match state.run.error.take().filter(|e| !e.is_empty()) {
                            Some(old) => format!("{note} {old}"),
                            None => note,
                        });
                }
            }
        }
        self.flushed.nudge();
    }

    async fn saved(&self) {
        while self.state.borrow().writing {
            self.flushed.wait().await;
        }
    }

    // --- events ---------------------------------------------------------------------------------

    fn apply(self: &Rc<Self>, event: EngineEvent) {
        {
            let mut state = self.state.borrow_mut();
            let next = apply_event(&state.run, &event);
            state.run = next;
        }
        self.save();
    }

    fn patch_node(self: &Rc<Self>, node_id: &str, branch: &str, patch: impl FnOnce(&mut NodeRun)) {
        {
            let mut state = self.state.borrow_mut();
            if let Some(node) = state
                .run
                .nodes
                .iter_mut()
                .find(|n| n.node_id == node_id && n.branch == branch)
            {
                patch(node);
            }
        }
        self.save();
    }

    fn fail(self: &Rc<Self>, node_id: &str, branch: &str, error: impl AsRef<str>) {
        self.apply(EngineEvent::NodeFailed {
            node_id: node_id.into(),
            branch: branch.into(),
            error: clip(error.as_ref()),
            at: now(),
        });
    }

    // --- lookups --------------------------------------------------------------------------------

    fn node_of(&self, node_id: &str) -> Option<Node> {
        self.state
            .borrow()
            .run
            .definition
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .cloned()
    }

    /// A step's project text: its own, else the workflow's `workspace.project`.
    fn project_text(&self, own: &str) -> String {
        if own.trim().is_empty() {
            self.state.borrow().run.definition.workspace.project.clone()
        } else {
            own.to_string()
        }
    }

    /// Whether `project_path` is the project the run worktree was made from.
    fn is_workspace_project(&self, project_path: &str) -> bool {
        let workspace = self.state.borrow().run.definition.workspace.project.clone();
        self.project_for(&workspace, "", "").is_some_and(|path| {
            monocode_core::files::path_key(&path) == monocode_core::files::path_key(project_path)
        })
    }

    /// `workspace.mode = perRun`: makes the run's worktree, once (a resumed run keeps its own).
    /// Err is why the run cannot start.
    async fn prepare_workspace(self: &Rc<Self>) -> Result<(), String> {
        let (workspace, name, number, run_id, current) = {
            let state = self.state.borrow();
            let run = &state.run;
            (
                run.definition.workspace.clone(),
                run.definition.name.clone(),
                run.number,
                run.id.clone(),
                run.worktree.clone(),
            )
        };
        if !workspace.per_run() {
            return Ok(());
        }
        match current {
            Some(tree) if !tree.is_removed() => return Ok(()),
            // The user removed it and the run is resumed: it comes back from its branch, so the
            // work the run committed is there again (D48).
            Some(tree) => {
                let again = self
                    .host
                    .restore_worktree(&run_id, tree)
                    .await
                    .map_err(|error| format!("Could not make the run's worktree again: {error}"))?;
                self.state.borrow_mut().run.worktree = Some(again);
                self.save();
                return Ok(());
            }
            None => {}
        }
        let Some(project) = self.project_for(&workspace.project, "", "") else {
            return Err(format!(
                "The workspace project \"{}\" is not one of the app's projects",
                workspace.project
            ));
        };
        let tree = self
            .host
            .create_run_worktree(RunWorktreeRequest {
                project,
                base_branch: workspace.base().to_string(),
                slug: slug_of(&name),
                number: number.unwrap_or(1),
                run_id,
            })
            .await
            .map_err(|error| format!("Could not make the run's worktree: {error}"))?;
        // Kept on the run even when it was stopped meanwhile: the folder exists now, and the
        // user finds it from the run's page.
        self.state.borrow_mut().run.worktree = Some(tree);
        self.save();
        Ok(())
    }

    /// The worktree a step in `branch` works in by default: the branch's own inside a For each
    /// (forked from the run worktree when the branch first needs it, then reused for every round
    /// and every later step), else the run's. `None` when the run has no worktree.
    async fn run_tree(self: &Rc<Self>, branch: &str) -> Result<Option<RunWorktree>, String> {
        let Some(run_tree) = self
            .state
            .borrow()
            .run
            .worktree
            .clone()
            .filter(|t| !t.is_removed())
        else {
            return Ok(None);
        };
        let Some(split) = parse_branch(branch) else {
            return Ok(Some(run_tree));
        };
        let key = branch_key(&split.group_id, split.index);
        let known = |this: &Self| {
            this.state
                .borrow()
                .run
                .branch_worktrees
                .get(&key)
                .filter(|t| !t.is_removed())
                .cloned()
        };
        let removed = |this: &Self| {
            this.state
                .borrow()
                .run
                .branch_worktrees
                .get(&key)
                .filter(|t| t.is_removed())
                .cloned()
        };
        if let Some(tree) = known(self) {
            return Ok(Some(tree));
        }
        let _turn = self.trees.turn().await;
        // Another step of the branch may have made it while this one waited.
        if let Some(tree) = known(self) {
            return Ok(Some(tree));
        }
        let run_id = self.state.borrow().run.id.clone();
        // One the user removed comes back from its branch instead of forking a second one.
        let tree = match removed(self) {
            Some(gone) => self
                .host
                .restore_worktree(&run_id, gone)
                .await
                .map_err(|error| {
                    format!("Could not make the worktree of this branch again: {error}")
                })?,
            None => self
                .host
                .fork_branch_worktree(&run_id, run_tree, split.group_id.clone(), split.index)
                .await
                .map_err(|error| format!("Could not make the worktree of this branch: {error}"))?,
        };
        self.state
            .borrow_mut()
            .run
            .branch_worktrees
            .insert(key, tree.clone());
        self.save();
        Ok(Some(tree))
    }

    /// The session a node ran in this branch, or at the top level.
    fn session_of(&self, node_id: &str, branch: &str) -> Option<String> {
        let state = self.state.borrow();
        let find = |wanted: &str| {
            state
                .run
                .nodes
                .iter()
                .find(|n| n.node_id == node_id && n.branch == wanted && n.session_id.is_some())
                .and_then(|n| n.session_id.clone())
        };
        find(branch).or_else(|| find(""))
    }

    fn project_for(&self, template: &str, node_id: &str, branch: &str) -> Option<String> {
        let rendered = {
            let state = self.state.borrow();
            render_template(
                template,
                &run_scope(&state.run, branch, Some(node_id)),
                false,
            )
        };
        self.host.project_path(&rendered)
    }

    /// The instance a cloud step asks for, resolved against this run, or why it cannot be.
    /// `sameAs` is the instance of that node's session in this branch; a `new` step in a later
    /// loop round (a new session each round) goes on in the instance of its previous round.
    /// Options and the account only apply when an instance is created, so they are dropped for an
    /// existing one. The string options are filled in from the run (like a prompt) and the
    /// RENDERED target is judged again here, so a value that is not valid fails the step before
    /// anything is created. The account is checked for shape here; whether it exists and carries
    /// a setup token or API key is checked by the start itself, before an instance is created.
    fn cloud_request(
        &self,
        node_id: &str,
        branch: &str,
        checkout: &Checkout,
        target: &SessionTarget,
        account: Option<&str>,
    ) -> Result<CloudRequest, String> {
        if *checkout != Checkout::Root {
            return Err(
                "A cloud step works in its instance's own checkout, so its checkout must be root"
                    .into(),
            );
        }
        let kind = CloudBackendKind::from_api_name(&target.kind)
            .ok_or_else(|| {
                format!(
                    "\"{}\" is not a cloud kind a run can start (codespace or sandbox)",
                    target.kind
                )
            })?
            .api_name()
            .to_string();
        // The account belongs to the instance: a step that names an existing one (validation
        // refuses it) never takes an account.
        if account.is_some() && !matches!(target.instance, TargetInstance::New) {
            return Err(monocode_core::mono::api::ACCOUNT_EXISTING_INSTANCE.into());
        }
        let instance = match &target.instance {
            TargetInstance::New => match self.previous_round_session(node_id, branch) {
                Some(previous) => CloudInstance::OfSession(previous),
                None => CloudInstance::New,
            },
            TargetInstance::SameAs(source) => CloudInstance::OfSession(
                self.session_of(source, branch)
                    .ok_or_else(|| format!("No cloud instance is available from \"{source}\""))?,
            ),
            TargetInstance::Invalid(_) => {
                return Err("The cloud instance of this step is not valid: use \"new\" or {\"sameAs\": \"<session node id>\"}".into());
            }
        };
        let (options, account) = match instance {
            CloudInstance::New => {
                let options = self.rendered_options(node_id, branch, target)?;
                let mut asked = options.clone();
                asked.insert("kind".into(), serde_json::json!(kind));
                asked.insert("instance".into(), serde_json::json!("new"));
                monocode_core::mono::cloud_api::validate::validate_target(
                    &serde_json::Value::Object(asked),
                    &serde_json::json!({ "harness": "claude" }),
                )
                .map_err(|problem| format!("The cloud target is not valid: {problem}"))?;
                if let Some(account) = account
                    && (account.trim().is_empty() || account.chars().count() > 128)
                {
                    return Err(
                        "The cloud account must be a non-empty text of at most 128 characters"
                            .into(),
                    );
                }
                (options, account.map(str::to_owned))
            }
            CloudInstance::OfSession(_) => (serde_json::Map::new(), None),
        };
        Ok(CloudRequest {
            kind,
            instance,
            options,
            account,
        })
    }

    /// The options of a new instance with the `{{...}}` of the string options filled in from the
    /// run (inputs, upstream outputs, the loop item), the way a prompt is. A template anywhere
    /// else is refused: validation never lets one through, and an unfilled one is not a value.
    fn rendered_options(
        &self,
        node_id: &str,
        branch: &str,
        target: &SessionTarget,
    ) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        let scope = run_scope(&self.state.borrow().run, branch, Some(node_id));
        let mut options = serde_json::Map::new();
        for (key, value) in target.options.iter() {
            let rendered = match value {
                Json::String(text)
                    if text.contains("{{")
                        && TEMPLATABLE_TARGET_OPTIONS.contains(&key.as_str()) =>
                {
                    serde_json::Value::String(render_template(text, &scope, false))
                }
                other => {
                    let plain = other.to_value();
                    if plain.to_string().contains("{{") {
                        return Err(format!(
                            "target.{key} cannot hold a template: it is used as written"
                        ));
                    }
                    plain
                }
            };
            options.insert(key.clone(), rendered);
        }
        Ok(options)
    }

    /// Keeps the instance a cloud session runs on in the run record, as soon as it has a name,
    /// so a run that is interrupted and never resumed still says what it left running. The
    /// instance is only read from the host's store; nothing is woken, stopped or deleted.
    fn record_instance(self: &Rc<Self>, node_id: &str, session_id: &str, created: bool) {
        let Some(info) = self.host.cloud_instance(session_id) else {
            return;
        };
        let Some(name) = info.name else {
            return;
        };
        self.record_named_instance(node_id, &info.kind, name, Some(session_id), created);
    }

    /// The same for an instance whose name is known without a session: one a start created
    /// before it failed (its error says so), which is billable all the same.
    fn record_named_instance(
        self: &Rc<Self>,
        node_id: &str,
        kind: &str,
        name: String,
        session_id: Option<&str>,
        created: bool,
    ) {
        {
            let mut state = self.state.borrow_mut();
            let kept = &mut state.run.cloud_instances;
            let at = match kept.iter().position(|i| i.kind == kind && i.name == name) {
                Some(at) => at,
                None => {
                    kept.push(RunInstance {
                        kind: kind.to_string(),
                        name,
                        created: false,
                        nodes: Vec::new(),
                        sessions: Vec::new(),
                    });
                    kept.len() - 1
                }
            };
            let entry = &mut kept[at];
            entry.created |= created;
            if !entry.nodes.iter().any(|n| n == node_id) {
                entry.nodes.push(node_id.to_string());
            }
            if let Some(session_id) = session_id
                && !entry.sessions.iter().any(|s| s == session_id)
            {
                entry.sessions.push(session_id.to_string());
            }
        }
        self.save();
    }

    /// An earlier session of this node in this branch (an earlier loop round, or one an
    /// interrupted run left behind). They all run on one instance.
    fn previous_round_session(&self, node_id: &str, branch: &str) -> Option<String> {
        let state = self.state.borrow();
        let node = state
            .run
            .nodes
            .iter()
            .find(|n| n.node_id == node_id && n.branch == branch)?;
        node.session_id
            .clone()
            .filter(|id| !id.is_empty())
            .or_else(|| {
                node.history
                    .iter()
                    .flatten()
                    .rev()
                    .find_map(|round| round.session_id.clone())
            })
            .or_else(|| {
                node.interrupted_sessions
                    .iter()
                    .flatten()
                    .next_back()
                    .cloned()
            })
    }

    /// Sessions an interrupted run left on this node that this run replaces: a cloud one may
    /// still hold its first prompt, which must not go out once its instance is ready.
    fn interrupted_of(&self, node_id: &str, branch: &str) -> Vec<String> {
        self.state
            .borrow()
            .run
            .nodes
            .iter()
            .find(|n| n.node_id == node_id && n.branch == branch)
            .and_then(|n| n.interrupted_sessions.clone())
            .unwrap_or_default()
    }

    /// A start that lands after the run was stopped: its session is kept on the node (so the
    /// report lists the instance), ended, and, when the report is already out, the instance is
    /// announced in a short follow-up.
    async fn settle_late_session(
        self: &Rc<Self>,
        node_id: &str,
        branch: &str,
        session_id: &str,
        cloud: Option<bool>,
    ) {
        if let Some(created) = cloud {
            let session = session_id.to_string();
            self.patch_node(node_id, branch, |n| {
                let kept = n.interrupted_sessions.get_or_insert_with(Vec::new);
                if !kept.contains(&session) {
                    kept.push(session);
                }
            });
            self.record_instance(node_id, session_id, created);
        }
        let _ = self.host.stop(session_id).await;
        if cloud.is_some() {
            let run = self.state.borrow().run.clone();
            if run.report_posted_at.is_some()
                && let Some(listing) =
                    instances::report_section(&run, |id| self.host.cloud_instance(id))
            {
                self.host.post(Summary {
                    title: format!("{} \u{b7} cloud instance left running", run.definition.name),
                    text: format!(
                        "**{}**\n\nA cloud step was still starting when the run was stopped, and its instance came up afterwards.\n\n{listing}",
                        run.definition.name
                    ),
                    launches: Vec::new(),
                    run,
                });
            }
        }
        self.host.release_session(session_id);
    }

    // --- waiting on a turn ------------------------------------------------------------------------

    /// Waits for a turn, showing any approval it asks for. Only time spent working counts toward
    /// the timeout; time waiting on the user has its own, longer limit.
    async fn watch_turn(
        self: &Rc<Self>,
        node_id: &str,
        branch: &str,
        session_id: &str,
        mut turn: Fut<Outcome>,
        timeout_ms: u64,
    ) -> Outcome {
        let poll = self.limits.poll_ms;
        let (mut working, mut waiting, mut asking) = (0u64, 0u64, false);
        let result = loop {
            let tick = self.sleep(poll);
            if let Either::Left(Either::Left(outcome)) =
                race(race(&mut turn, self.stop.wait()), tick).await
            {
                break outcome;
            }
            if !self.live() {
                break Outcome::cancelled(STOPPED_BY_USER);
            }
            let pending = self.host.waiting_approval(session_id);
            if pending != asking {
                asking = pending;
                self.apply(EngineEvent::WaitingApproval {
                    node_id: node_id.into(),
                    branch: branch.into(),
                    waiting: asking,
                });
            }
            if pending {
                waiting += poll;
            } else {
                working += poll;
            }
            let approval_expired = waiting >= self.limits.approval_wait_ms;
            if working >= timeout_ms || approval_expired {
                let error = if approval_expired {
                    "Nobody answered its approval request.".to_string()
                } else {
                    let minutes = (timeout_ms as f64 / 60_000.0 + 0.5).floor();
                    format!(
                        "It took longer than {} minutes and was stopped.",
                        js_number_text(minutes)
                    )
                };
                let _ = self.host.stop(session_id).await;
                break Outcome::limit(error);
            }
        };
        if asking && self.live() {
            self.apply(EngineEvent::WaitingApproval {
                node_id: node_id.into(),
                branch: branch.into(),
                waiting: false,
            });
        }
        result
    }

    // --- node execution -----------------------------------------------------------------------------

    async fn run_session(
        self: &Rc<Self>,
        node_id: String,
        branch: String,
        prompt: String,
        reuse_session_id: Option<String>,
    ) {
        let Some(Node {
            kind:
                NodeKind::Session {
                    project,
                    checkout,
                    output,
                    harness,
                    model,
                    effort,
                    nest_under,
                    timeout_minutes,
                    target,
                    account,
                    folder,
                    ..
                },
            ..
        }) = self.node_of(&node_id)
        else {
            return;
        };
        let (access, links_work_item) = self.node_of(&node_id).map_or_else(
            || (DEFAULT_SESSION_ACCESS.to_string(), false),
            |n| {
                (
                    n.kind.session_access().to_string(),
                    n.kind.links_work_item(),
                )
            },
        );
        // The trigger item of an event run, linked locally when the node asks for it (D32).
        let work_item = links_work_item
            .then(|| {
                let state = self.state.borrow();
                let item = state.run.caller.item.as_ref()?;
                work_item_of(&serde_json::to_value(item).ok()?)
            })
            .flatten();
        let project = self.project_text(&project);
        let Some(project_path) = self.project_for(&project, &node_id, &branch) else {
            return self.fail(
                &node_id,
                &branch,
                format!("\"{project}\" is not one of the app's projects"),
            );
        };
        let timeout_ms =
            (timeout_minutes.unwrap_or(DEFAULT_SESSION_TIMEOUT_MINUTES as f64) * 60_000.0) as u64;
        let (mode, terminal) = self
            .node_of(&node_id)
            .map(|n| {
                (
                    n.kind.session_mode(),
                    n.kind.session_surface().is_terminal(),
                )
            })
            .unwrap_or_default();
        // The declared outputs are recorded by a command the session runs. A cloud session
        // cannot reach this app, so it gets no hint and is read by the fallback instead. The hint
        // is part of the first prompt only: a later round talks to a session that has read it.
        let prompt = if target.is_none() && reuse_session_id.is_none() {
            let hint = output_hint(&output, &text::app_cli());
            if hint.is_empty() {
                prompt
            } else {
                format!("{prompt}\n\n{hint}")
            }
        } else {
            prompt
        };
        // The commit the step starts from, to tell what it changed (D18).
        let mut base = None;

        let (session_id, turn): (String, Fut<Outcome>) = if let Some(reuse) = reuse_session_id {
            if target.is_none()
                && let Some(dir) = self.host.worktree_of(&reuse)
            {
                base = self.host.head_commit(dir).await;
            }
            let turn = self.host.send(&reuse, prompt);
            (reuse, turn)
        } else {
            // A cloud step is resolved before anything is created: whatever cannot be resolved
            // fails the node here, and a step that slips past validation never starts an instance.
            let cloud = match target
                .as_ref()
                .map(|t| self.cloud_request(&node_id, &branch, &checkout, t, account.as_deref()))
            {
                None => None,
                Some(Ok(request)) => Some(request),
                Some(Err(error)) => return self.fail(&node_id, &branch, error),
            };
            let mut worktree_cwd = None;
            // The run's own worktree (or its branch's), when the step is in the workspace project.
            let mut run_base = None;
            if target.is_none() && checkout == Checkout::Run {
                match self.run_tree(&branch).await {
                    Ok(Some(tree)) => {
                        if !self.is_workspace_project(&project_path) {
                            return self.fail(
                                &node_id,
                                &branch,
                                "This step works in another project than the run worktree: use checkout root, newWorktree or existing",
                            );
                        }
                        worktree_cwd = Some(tree.path.clone());
                        run_base = Some(tree.base);
                    }
                    Ok(None) => {}
                    Err(error) => return self.fail(&node_id, &branch, error),
                }
            }
            if let Checkout::Existing(path) = &checkout {
                let scope = run_scope(&self.state.borrow().run, &branch, Some(&node_id));
                worktree_cwd = Some(render_template(path, &scope, false));
            }
            if let Checkout::WorktreeOf(source_node) = &checkout {
                worktree_cwd = self
                    .session_of(source_node, &branch)
                    .and_then(|source| self.host.worktree_of(&source));
                if worktree_cwd.is_none() {
                    return self.fail(
                        &node_id,
                        &branch,
                        format!("No worktree is available from \"{source_node}\""),
                    );
                }
            }
            let parent_session_id = nest_under
                .as_ref()
                .and_then(|parent| self.session_of(parent, &branch));
            let folder_name = workflow_folder_name(&self.state.borrow().run.definition.name);
            let is_cloud = cloud.is_some();
            // Only a step that asks for a new instance creates one: a later round or a resumed
            // step goes on in an instance that exists.
            let creates = cloud
                .as_ref()
                .is_some_and(|c| c.instance == CloudInstance::New);
            // A cloud session an interrupted run left behind may still hold its first prompt:
            // it is dropped so it does not run unattended next to this new session. The instance
            // is left alone.
            if cloud.is_some() {
                for old in self.interrupted_of(&node_id, &branch) {
                    let _ = self.host.stop(&old).await;
                }
            }
            if !is_cloud {
                // In the run worktree the changes are measured from where the run started (D18).
                base = match run_base {
                    Some(base) => Some(base),
                    None => {
                        let dir = worktree_cwd.clone().unwrap_or_else(|| project_path.clone());
                        self.host.head_commit(dir).await
                    }
                };
            }
            let request = SessionRequest {
                project: project_path.clone(),
                prompt,
                harness: harness.filter(|h| !h.is_empty()),
                model: model.filter(|m| !m.is_empty()),
                effort: effort.filter(|e| !e.is_empty()),
                new_worktree: checkout == Checkout::NewWorktree,
                worktree_cwd,
                parent_session_id,
                folder_name: Some(folder_name),
                // A folder is for local sessions: a cloud one is filed by its instance.
                folder_id: folder.filter(|_| !is_cloud),
                access,
                work_item: work_item.filter(|_| !is_cloud),
                target: cloud,
                terminal,
            };
            // The start runs on its own so Stop does not wait for one that hangs.
            let (done, started) = async_channel::bounded(1);
            let this = self.clone();
            self.cx
                .spawn(async move |_| {
                    let _turn = this.starts.turn().await;
                    // Starts queued behind another one wait here; a stopped run must not launch them.
                    let result = if this.live() {
                        this.host.start_session(request).await.map(Some)
                    } else {
                        Ok(None)
                    };
                    let _ = done.send(result).await;
                })
                .detach();
            let started = match race(started.recv(), self.stop.wait()).await {
                Either::Right(()) => {
                    // One that comes through later is ended.
                    let this = self.clone();
                    let (node_id, branch) = (node_id.clone(), branch.clone());
                    self.cx
                        .spawn(async move |_| {
                            if let Ok(Ok(Some(late))) = started.recv().await {
                                this.settle_late_session(
                                    &node_id,
                                    &branch,
                                    &late.session_id,
                                    is_cloud.then_some(creates),
                                )
                                .await;
                            }
                        })
                        .detach();
                    return;
                }
                Either::Left(Err(_)) | Either::Left(Ok(Ok(None))) => return,
                Either::Left(Ok(Err(error))) => {
                    // A start can fail after it created the instance (its error says so, and
                    // the instance is kept): it is billable all the same, so the run names it.
                    if is_cloud
                        && let Some((_, kind, name)) =
                            monocode_core::cloud_sessions::split_created_note(&error)
                    {
                        self.record_named_instance(
                            &node_id,
                            kind.api_name(),
                            name.to_string(),
                            None,
                            true,
                        );
                    }
                    return self.fail(&node_id, &branch, error);
                }
                Either::Left(Ok(Ok(Some(started)))) => started,
            };
            if !self.live() {
                // Stopped while it was starting: nothing else knows this session yet.
                self.settle_late_session(
                    &node_id,
                    &branch,
                    &started.session_id,
                    is_cloud.then_some(creates),
                )
                .await;
                return;
            }
            // Kept in the run record at once: an interrupted run that is never resumed still
            // says what it left running.
            if is_cloud {
                self.record_instance(&node_id, &started.session_id, creates);
            }
            (started.session_id, started.settled)
        };
        let session = session_id.clone();
        self.patch_node(&node_id, &branch, |n| n.session_id = Some(session));

        let outcome = self
            .watch_turn(&node_id, &branch, &session_id, turn, timeout_ms)
            .await;
        if !self.live() {
            return;
        }
        // "Wait for me" hands the session to the user when its first turn ends, however it ended:
        // a turn they stopped or one that failed leaves a session they can still talk to. Only
        // a session that is gone fails the step.
        let user_has_it = mode == SessionMode::WaitForMe
            && !terminal
            && target.is_none()
            && !outcome.by_limit
            && matches!(outcome.status, Status::Cancelled | Status::Failed)
            && self.host.session_info(&session_id).is_some();
        if outcome.status != Status::Completed && !user_has_it {
            // A cloud session that did not finish may still hold its first prompt (its start
            // failed): drop it so a later Retry does not run the step outside the run. The
            // instance is left alone.
            if target.is_some() {
                let _ = self.host.stop(&session_id).await;
            }
            return self.fail(
                &node_id,
                &branch,
                outcome.error.unwrap_or_else(|| {
                    if outcome.status == Status::Cancelled {
                        "The session was stopped".into()
                    } else {
                        "The session did not finish".into()
                    }
                }),
            );
        }
        // A provider failure that was handed over stays visible in the node's details.
        let mut noted = false;
        if outcome.status == Status::Failed
            && let Some(error) = &outcome.error
        {
            let note = format!("The first turn failed: {error}");
            self.patch_node(&node_id, &branch, |n| n.error = Some(note));
            noted = true;
        }
        let reply = self.reply_of(&session_id, outcome.text);
        let finished = self
            .finish_session(
                &node_id,
                &branch,
                &session_id,
                reply,
                &output,
                SessionKind {
                    mode,
                    terminal,
                    cloud: target.is_some(),
                },
                base,
            )
            .await;
        if !self.live() {
            return;
        }
        match finished {
            Err(problem) => self.fail(&node_id, &branch, problem),
            Ok(value) => {
                // Continued past the failure: the note is not an error of the finished node.
                if noted {
                    self.patch_node(&node_id, &branch, |n| n.error = None);
                }
                self.apply(EngineEvent::NodeFinished {
                    node_id,
                    branch,
                    output: cap_map(&value),
                    at: now(),
                });
            }
        }
    }

    /// The last thing the session said, else what its turn reported.
    fn reply_of(&self, session_id: &str, fallback: String) -> String {
        let reply = self.host.final_reply(session_id);
        if reply.is_empty() { fallback } else { reply }
    }

    /// A session step's first turn ended: what the node's output is. Everything a step reports
    /// meets here. "Wait for me" holds the step until the user answers. The declared fields are
    /// what the session recorded with `workflow.output` (a `workflow-output` block the session
    /// wrote on its own counts too); the ones still missing are read from the conversation by a
    /// hidden call with the session's own harness, model and account (never by asking the user's
    /// session again), and a terminal session, which has no transcript to read, fails the step.
    /// The built-in fields are added last. Err is why the step fails.
    #[allow(clippy::too_many_arguments)]
    async fn finish_session(
        self: &Rc<Self>,
        node_id: &str,
        branch: &str,
        session_id: &str,
        mut reply: String,
        declared: &OrderedMap<OutputField>,
        kind: SessionKind,
        base: Option<String>,
    ) -> Result<OrderedMap<Json>, String> {
        if kind.mode == SessionMode::WaitForMe {
            self.wait_for_user(node_id, branch, session_id).await?;
            if !kind.terminal {
                reply = self.reply_of(session_id, reply);
            }
        }
        if !self.live() {
            return Err(STOPPED_BY_USER.into());
        }
        let mut output = self.pending_output(node_id, branch);
        let mut missing = missing_fields(declared, &output);
        if !missing.is_empty()
            && let Ok(block) = parse_workflow_output(&reply, &missing)
        {
            for (name, value) in block.iter() {
                if missing.contains_key(name)
                    && check_output_value(&missing, &single(name, value)).is_ok()
                {
                    output.insert(name.clone(), value.clone());
                }
            }
            missing = missing_fields(declared, &output);
        }
        if !missing.is_empty() {
            let names = missing.keys().cloned().collect::<Vec<_>>().join(", ");
            if kind.terminal {
                return Err(format!(
                    "The terminal session did not record these output fields with workflow.output: {names}"
                ));
            }
            let found = self.extract(session_id, &missing).await?;
            for (name, value) in found.iter() {
                output.insert(name.clone(), value.clone());
            }
        }
        if !self.live() {
            return Err(STOPPED_BY_USER.into());
        }
        for (name, value) in self.builtins(session_id, &reply, kind, base).await.iter() {
            output.insert(name.clone(), value.clone());
        }
        // The session may have recorded more while the fields were read: what it recorded wins
        // over what was read, and nothing more is taken after this (the node finishes without
        // waiting, so a later call finds it over).
        for (name, value) in self.pending_output(node_id, branch).iter() {
            output.insert(name.clone(), value.clone());
        }
        Ok(output)
    }

    /// Reads `missing` out of the session's conversation. Only the fields asked for come back,
    /// each checked against its declared type.
    async fn extract(
        self: &Rc<Self>,
        session_id: &str,
        missing: &OrderedMap<OutputField>,
    ) -> Result<OrderedMap<Json>, String> {
        let names = missing.keys().cloned().collect::<Vec<_>>().join(", ");
        let why = |reason: String| {
            format!("Could not read the output fields ({names}) from the session: {reason}")
        };
        let call = self.host.extract(ExtractRequest {
            session_id: session_id.into(),
            fields: missing.clone(),
            timeout_ms: self.limits.decision_timeout_ms,
        });
        let answer = match race(call, self.stop.wait()).await {
            Either::Right(()) => return Err(STOPPED_BY_USER.into()),
            Either::Left(Err(error)) => return Err(why(error)),
            Either::Left(Ok(answer)) => answer,
        };
        let block = parse_workflow_output(&answer, missing).map_err(why)?;
        let mut found = OrderedMap::new();
        for (name, value) in block.iter() {
            if !missing.contains_key(name) {
                continue;
            }
            check_output_value(missing, &single(name, value)).map_err(why)?;
            found.insert(name.clone(), value.clone());
        }
        let absent: Vec<&String> = missing.keys().filter(|n| !found.contains_key(n)).collect();
        if !absent.is_empty() {
            return Err(why(format!(
                "the answer had no value for {}",
                absent
                    .iter()
                    .map(|n| n.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(found)
    }

    /// The fields every session step has without declaring anything: `reply` (chat only),
    /// `sessionId`, and, for a step on this computer, what Git says about its checkout.
    async fn builtins(
        self: &Rc<Self>,
        session_id: &str,
        reply: &str,
        kind: SessionKind,
        base: Option<String>,
    ) -> OrderedMap<Json> {
        let mut out = OrderedMap::new();
        if !kind.terminal {
            out.insert("reply", Json::String(reply.to_string()));
        }
        out.insert("sessionId", Json::String(session_id.to_string()));
        if !kind.cloud {
            let facts = self.host.session_facts(session_id, base).await;
            out.insert("worktree", Json::String(facts.worktree));
            out.insert("branch", Json::String(facts.branch));
            out.insert("commit", Json::String(facts.commit));
            out.insert(
                "changedFiles",
                Json::Array(facts.changed_files.into_iter().map(Json::String).collect()),
            );
            out.insert("diffStat", Json::String(facts.diff_stat));
        }
        out
    }

    /// "Wait for me": the step stays open for the user. Ok when they pressed Continue; Err is why
    /// the step fails (Fail step, Stop, or nobody answered within the wait limit).
    async fn wait_for_user(
        self: &Rc<Self>,
        node_id: &str,
        branch: &str,
        session_id: &str,
    ) -> Result<(), String> {
        let key = (node_id.to_string(), branch.to_string());
        let (answer_tx, answer_rx) = async_channel::bounded(1);
        self.waits.borrow_mut().insert(key.clone(), answer_tx);
        self.apply(EngineEvent::WaitingForUser {
            node_id: node_id.into(),
            branch: branch.into(),
            waiting: true,
        });
        self.host.wait_for_user(WaitNotice {
            session_id: session_id.into(),
            workflow: self.state.borrow().run.definition.name.clone(),
            step: node_id.into(),
        });
        let timer = self.sleep(self.limits.approval_wait_ms);
        let answer = race(race(answer_rx.recv(), self.stop.wait()), timer).await;
        self.waits.borrow_mut().remove(&key);
        let answer = match answer {
            // The user may have chatted on and a turn may still be running: the outputs are
            // taken once it has ended (or the run is stopped), and the step stays waiting until
            // then.
            Either::Left(Either::Left(Ok(WaitChoice::Continue))) => {
                match race(self.host.turn_ends(session_id), self.stop.wait()).await {
                    Either::Left(()) => answer,
                    Either::Right(()) => Either::Left(Either::Right(())),
                }
            }
            other => other,
        };
        self.host.wait_over(session_id);
        match answer {
            Either::Left(Either::Left(Ok(WaitChoice::Continue))) => {
                // From here the step works again, reading what it needs.
                self.apply(EngineEvent::WaitingForUser {
                    node_id: node_id.into(),
                    branch: branch.into(),
                    waiting: false,
                });
                Ok(())
            }
            Either::Left(Either::Left(Ok(WaitChoice::Fail))) => {
                Err("You ended this step with Fail step.".into())
            }
            Either::Left(Either::Left(Err(_))) | Either::Left(Either::Right(())) => {
                Err(STOPPED_BY_USER.into())
            }
            Either::Right(()) => {
                Err("Nobody pressed Continue or Fail step before the wait limit ran out.".into())
            }
        }
    }

    /// The user answered the step waiting in `session_ids`. Settles at most once: the first
    /// answer takes the step's slot, so another press, or an answer that comes after Stop, finds
    /// nothing to answer.
    fn answer_wait(&self, session_ids: &[String], choice: WaitChoice) -> Result<(), String> {
        const GONE: &str = "This step is no longer waiting for you.";
        if !self.live() {
            return Err(GONE.into());
        }
        let key = {
            let state = self.state.borrow();
            let waits = self.waits.borrow();
            state
                .run
                .nodes
                .iter()
                .find(|n| {
                    n.session_id
                        .as_ref()
                        .is_some_and(|id| session_ids.contains(id))
                        && waits.contains_key(&(n.node_id.clone(), n.branch.clone()))
                })
                .map(|n| (n.node_id.clone(), n.branch.clone()))
        };
        let sender = key
            .and_then(|key| self.waits.borrow_mut().remove(&key))
            .ok_or(GONE)?;
        sender.try_send(choice).map_err(|_| GONE.to_string())
    }

    /// What the node's session wrote with `workflow.output` this round.
    fn pending_output(&self, node_id: &str, branch: &str) -> OrderedMap<Json> {
        self.state
            .borrow()
            .run
            .nodes
            .iter()
            .find(|n| n.node_id == node_id && n.branch == branch)
            .and_then(|n| n.pending_output.clone())
            .unwrap_or_default()
    }

    /// Records what a session wrote with `workflow.output` into the run record, when `target` is
    /// still the step it is running.
    fn record_output(
        self: &Rc<Self>,
        target: &ActiveNode,
        values: &Json,
    ) -> Result<Vec<String>, String> {
        if !self.live() {
            return Err(monocode_core::mono::workflows::active::ActiveError::Ended.to_string());
        }
        let recorded = {
            let mut state = self.state.borrow_mut();
            let mut next = record_active_output(&state.run, target, values)?;
            if let Some(node) = next
                .nodes
                .iter_mut()
                .find(|n| n.node_id == target.node_id && n.branch == target.branch)
                && let Some(pending) = &node.pending_output
            {
                node.pending_output = Some(cap_map(pending));
            }
            state.run = next;
            values
                .as_object()
                .map(|given| given.keys().cloned().collect())
                .unwrap_or_default()
        };
        self.save();
        Ok(recorded)
    }

    async fn run_command(self: &Rc<Self>, node_id: String, branch: String, command: String) {
        let Some(Node {
            kind:
                NodeKind::Command {
                    project,
                    cwd,
                    timeout_minutes,
                    on_failure,
                    ..
                },
            ..
        }) = self.node_of(&node_id)
        else {
            return;
        };
        let project = self.project_text(&project);
        let Some(mut dir) = self.project_for(&project, &node_id, &branch) else {
            return self.fail(
                &node_id,
                &branch,
                format!("\"{project}\" is not one of the app's projects"),
            );
        };
        if let Some(Cwd::WorktreeOf(source_node)) = &cwd {
            match self
                .session_of(source_node, &branch)
                .and_then(|source| self.host.worktree_of(&source))
            {
                Some(worktree) => dir = worktree,
                None => {
                    return self.fail(
                        &node_id,
                        &branch,
                        format!("No worktree is available from \"{source_node}\""),
                    );
                }
            }
        }
        // Without a cwd the command runs in the run worktree (or its branch's).
        if cwd.is_none() && self.is_workspace_project(&dir) {
            match self.run_tree(&branch).await {
                Ok(Some(tree)) => dir = tree.path,
                Ok(None) => {}
                Err(error) => return self.fail(&node_id, &branch, error),
            }
        }
        let minutes = timeout_minutes.unwrap_or(DEFAULT_COMMAND_TIMEOUT_MINUTES as f64);
        let id = {
            let mut state = self.state.borrow_mut();
            state.command_seq += 1;
            let id = format!(
                "{}:{}:{node_id}:{branch}:{}",
                state.run.id, self.nonce, state.command_seq
            );
            state.commands.insert(id.clone());
            id
        };
        let result = self
            .host
            .run_command(dir, command, (minutes * 60_000.0) as u64, id.clone())
            .await;
        self.state.borrow_mut().commands.remove(&id);
        if !self.live() {
            return;
        }
        let result = match result {
            Ok(result) => result,
            Err(error) => return self.fail(&node_id, &branch, error),
        };
        if result.cancelled {
            return self.fail(&node_id, &branch, "The command was cancelled");
        }
        if result.timed_out {
            return self.fail(
                &node_id,
                &branch,
                format!(
                    "The command timed out after {} minutes",
                    js_number_text(minutes)
                ),
            );
        }
        let carry_on = on_failure.as_deref() == Some("continue");
        if result.exit_code != Some(0) && !carry_on {
            let detail = [result.stderr.trim(), result.stdout.trim()]
                .into_iter()
                .find(|text| !text.is_empty())
                .unwrap_or("");
            let code = result
                .exit_code
                .map_or("unknown".to_string(), |code| code.to_string());
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!(": {}", utf16_tail(detail, COMMAND_DETAIL_MAX))
            };
            return self.fail(
                &node_id,
                &branch,
                format!("The command exited with code {code}{detail}"),
            );
        }
        let mut output = OrderedMap::new();
        // A failed command only gets here with `onFailure: continue`; a process that left no
        // exit code (killed by a signal) reads as -1.
        output.insert(
            "exitCode",
            Json::Number(f64::from(result.exit_code.unwrap_or(if carry_on {
                -1
            } else {
                0
            }))),
        );
        output.insert("stdout", Json::String(result.stdout));
        output.insert("stderr", Json::String(result.stderr));
        self.apply(EngineEvent::NodeFinished {
            node_id,
            branch,
            output: cap_map(&output),
            at: now(),
        });
    }

    /// The app project an AI decision runs in and takes its account from: the project of the
    /// session node the model came from, else the first project of any session node that
    /// resolves. `None` when the run has none (the host then uses the account default).
    fn decision_project(&self, source: Option<&str>, branch: &str) -> Option<String> {
        let nodes = self.state.borrow().run.definition.nodes.clone();
        let project_of = |node: &Node| match &node.kind {
            NodeKind::Session { project, .. } | NodeKind::Command { project, .. } => {
                self.project_for(&self.project_text(project), &node.id, branch)
            }
            _ => None,
        };
        source
            .and_then(|id| nodes.iter().find(|n| n.id == id))
            .and_then(project_of)
            .or_else(|| {
                nodes
                    .iter()
                    .filter(|n| matches!(n.kind, NodeKind::Session { .. }))
                    .find_map(project_of)
            })
    }

    /// An AI decision (If, Switch or loop edge): a one-shot call with no tools on the model of
    /// `decision_model_for`. A reply that is not one of the answers is put back once.
    async fn run_decide(self: &Rc<Self>, action: &EngineAction) {
        let EngineAction::Decide {
            node_id,
            branch,
            answers,
            ..
        } = action
        else {
            return;
        };
        if !self.live() {
            return;
        }
        let definition = self.state.borrow().run.definition.clone();
        let Some(prompt) = decision_prompt(action, &definition.name) else {
            return;
        };
        let Some(model) = decision_model_for(&definition, node_id) else {
            return self.fail(
                node_id,
                branch,
                "This decision has no model: set the workflow's decisionModel or put it after a session step",
            );
        };
        let source = match &model.source {
            DecisionModelSource::Session(id) => Some(id.as_str()),
            DecisionModelSource::Workflow => None,
        };
        let project = self.decision_project(source, branch);
        let mut asked = prompt.clone();
        for attempt in 0..2 {
            let call = self.host.decide(DecisionRequest {
                harness: model.harness.clone(),
                model: model.model.clone(),
                effort: model.effort.clone(),
                project: project.clone(),
                prompt: asked.clone(),
                timeout_ms: self.limits.decision_timeout_ms,
            });
            let reply = match race(call, self.stop.wait()).await {
                Either::Right(()) => return,
                Either::Left(Err(error)) => return self.fail(node_id, branch, error),
                Either::Left(Ok(reply)) => reply,
            };
            if !self.live() {
                return;
            }
            match parse_decision(&reply, answers) {
                Ok(decision) => {
                    return self.apply(EngineEvent::Decided {
                        node_id: node_id.clone(),
                        branch: branch.clone(),
                        value: decision.value,
                        reason: decision.reason.map(|reason| cap_string(&reason)),
                        at: now(),
                    });
                }
                Err(problem) if attempt == 1 => {
                    return self.fail(
                        node_id,
                        branch,
                        format!("The AI did not give a usable answer. {problem}"),
                    );
                }
                Err(problem) => {
                    asked = format!(
                        "{prompt}\n\nYour previous reply could not be used: {problem}\nAnswer again with only the JSON object."
                    );
                }
            }
        }
    }

    /// Kills every command this run still has going. Never fails.
    async fn cancel_commands(self: &Rc<Self>) {
        let ids: Vec<String> = self.state.borrow().commands.iter().cloned().collect();
        join_all(
            ids.into_iter()
                .map(|id| {
                    let this = self.clone();
                    Box::pin(async move {
                        for _ in 0..CANCEL_ATTEMPTS {
                            if !this.state.borrow().commands.contains(&id) {
                                return;
                            }
                            match this.host.cancel_command(&id).await {
                                Ok(true) => return,
                                Ok(false) => {}
                                Err(error) => {
                                    eprintln!("Could not cancel command {id}: {error}");
                                    return;
                                }
                            }
                            this.sleep(CANCEL_RETRY_MS).await;
                        }
                    }) as Fut<()>
                })
                .collect(),
        )
        .await;
    }

    fn track(self: &Rc<Self>, action: EngineAction) {
        self.active.set(self.active.get() + 1);
        let this = self.clone();
        self.cx
            .spawn(async move |_| {
                match action {
                    EngineAction::StartSession {
                        node_id,
                        branch,
                        prompt,
                        reuse_session_id,
                        ..
                    } => {
                        this.run_session(node_id, branch, prompt, reuse_session_id)
                            .await
                    }
                    EngineAction::RunCommand {
                        node_id,
                        branch,
                        command,
                    } => this.run_command(node_id, branch, command).await,
                    ref decide @ EngineAction::Decide { .. } => this.run_decide(decide).await,
                    EngineAction::PostSummary => {}
                }
                this.active.set(this.active.get() - 1);
                this.nudge.nudge();
            })
            .detach();
    }

    // --- report -----------------------------------------------------------------------------------

    fn launches_of(&self, run: &WorkflowRun) -> Vec<SessionInfo> {
        let mut seen = BTreeSet::new();
        let mut launches = Vec::new();
        for node in &run.nodes {
            let Some(id) = &node.session_id else { continue };
            if !seen.insert(id.clone()) {
                continue;
            }
            launches.extend(self.host.session_info(id));
        }
        launches
    }

    /// The summary of a finished run: what `compose_report` makes of it, and the instances it
    /// left running.
    fn summary_of(&self, finished: &WorkflowRun) -> RunSummary {
        let report = compose_report(finished);
        let mut text = report.text;
        if let Some(listing) =
            instances::report_section(finished, |id| self.host.cloud_instance(id))
        {
            text.push_str("\n\n");
            text.push_str(&listing);
        }
        RunSummary {
            title: report.title,
            text,
        }
    }

    /// Keeps the summary on the run (for the run page) and hands it to the app to deliver.
    fn deliver_summary(self: &Rc<Self>, finished: WorkflowRun) {
        let summary = self.summary_of(&finished);
        let run = {
            let mut state = self.state.borrow_mut();
            state.run.summary = Some(summary.clone());
            state.run.clone()
        };
        self.save();
        let launches = self.launches_of(&finished);
        self.host.post(Summary {
            run,
            title: summary.title,
            text: summary.text,
            launches,
        });
    }

    /// Posts the summary. The run's final status is known once the summary counts as posted.
    fn post_report(self: &Rc<Self>) -> bool {
        let posted = {
            let mut state = self.state.borrow_mut();
            let posted = apply_event(&state.run, &EngineEvent::ReportPosted { at: now() });
            if posted == state.run {
                return false;
            }
            state.run = posted.clone();
            posted
        };
        self.deliver_summary(posted);
        true
    }

    /// Starts everything the engine has ready. A question or session is marked started first.
    fn pump(self: &Rc<Self>) {
        for _ in 0..1_000 {
            if !self.live() {
                return;
            }
            let action = {
                let state = self.state.borrow();
                next_actions(&state.run).into_iter().next()
            };
            let Some(action) = action else { return };
            let (node_id, branch, session_id) = match &action {
                EngineAction::PostSummary => {
                    if !self.post_report() {
                        return;
                    }
                    continue;
                }
                EngineAction::StartSession {
                    node_id,
                    branch,
                    reuse_session_id,
                    ..
                } => (node_id, branch, reuse_session_id.clone()),
                EngineAction::RunCommand {
                    node_id, branch, ..
                }
                | EngineAction::Decide {
                    node_id, branch, ..
                } => (node_id, branch, None),
            };
            let before = self.state.borrow().run.clone();
            self.apply(EngineEvent::NodeStarted {
                node_id: node_id.clone(),
                branch: branch.clone(),
                session_id,
                at: now(),
            });
            // The engine refused: do not loop on an action it will keep offering.
            if self.state.borrow().run == before {
                return;
            }
            self.track(action);
        }
    }

    // --- stop -------------------------------------------------------------------------------------

    /// Stops the run: ends it first (so whatever the sessions report as they stop changes
    /// nothing), then its commands and sessions, and posts the report once.
    fn request_stop(self: &Rc<Self>) -> bool {
        if !self.live() || self.stopping.replace(true) {
            return false;
        }
        let this = self.clone();
        self.cx
            .spawn(async move |_| {
                this.stop_run().await;
                this.stopped.fire();
            })
            .detach();
        true
    }

    async fn stop_run(self: &Rc<Self>) {
        let mut sessions: Vec<String> = Vec::new();
        {
            let state = self.state.borrow();
            for node in &state.run.nodes {
                let busy = matches!(
                    node.status,
                    NodeStatus::Running | NodeStatus::WaitingApproval | NodeStatus::WaitingForUser
                );
                if let Some(id) = &node.session_id
                    && busy
                    && !is_asking(node)
                    && !sessions.contains(id)
                {
                    sessions.push(id.clone());
                }
            }
        }
        self.apply(EngineEvent::Stopped { at: now() });
        self.stop.fire();
        self.nudge.nudge();
        let mut work: Vec<Fut<()>> = vec![{
            let this = self.clone();
            Box::pin(async move { this.cancel_commands().await })
        }];
        for id in sessions {
            let this = self.clone();
            work.push(Box::pin(async move {
                if let Err(error) = this.host.stop(&id).await {
                    eprintln!("Could not stop session {id}: {error}");
                }
            }));
        }
        join_all(work).await;
        // A stopped run reports once, with what had finished by then.
        let report = {
            let mut state = self.state.borrow_mut();
            if state.run.report_posted_at.is_none() {
                state.run.report_posted_at = Some(now());
                Some(state.run.clone())
            } else {
                None
            }
        };
        if let Some(run) = report {
            self.deliver_summary(run);
        }
    }

    // --- the run ----------------------------------------------------------------------------------

    async fn execute(self: Rc<Self>) -> WorkflowRun {
        // The record exists before anything runs, so a failure to save is known up front.
        self.save();
        self.saved().await;
        // The run's own worktree comes first: every step defaults to it.
        if let Err(error) = self.prepare_workspace().await
            && self.live()
        {
            {
                let mut state = self.state.borrow_mut();
                state.run.status = RunStatus::Failed;
                state.run.ended_at = Some(now());
                state.run.error = Some(error);
            }
            self.save();
        }
        while self.live() {
            self.pump();
            if !self.live() {
                break;
            }
            if self.active.get() == 0 {
                {
                    let mut state = self.state.borrow_mut();
                    let note = "The run could not make progress.";
                    state.run.status = RunStatus::Failed;
                    state.run.ended_at = Some(now());
                    state.run.error =
                        Some(match state.run.error.take().filter(|e| !e.is_empty()) {
                            Some(old) => format!("{old} {note}"),
                            None => note.into(),
                        });
                }
                self.save();
                break;
            }
            self.nudge.wait().await;
        }
        // The run has ended on its own: deliver its summary once (a stop posts its own).
        if !self.stopping.get() && !self.quitting.get() {
            self.post_report();
        }
        // Stop ends commands and sessions at once, a session notices within a poll and a command
        // has its node timeout. A decision and a start that has not returned are
        // abandoned on Stop, so they do not hold the run; a run that ended some other way waits
        // for them. Then the workflow is not seen as idle (and a scheduled run does not start in
        // the same directory) while something of this run is still alive.
        if self.stopping.get() {
            self.stopped.wait().await;
        }
        self.cancel_commands().await;
        while self.active.get() > 0 {
            self.nudge.wait().await;
        }
        self.saved().await;
        let run = self.state.borrow().run.clone();
        let mut sessions: Vec<&String> = Vec::new();
        for node in &run.nodes {
            let past = node
                .history
                .iter()
                .flatten()
                .filter_map(|r| r.session_id.as_ref());
            for id in node.session_id.iter().chain(past) {
                if !sessions.contains(&id) {
                    sessions.push(id);
                }
            }
        }
        for id in sessions {
            self.host.release_session(id);
        }
        self.signals.release_run(&self.workflow_id, &run.id);
        self.registry.borrow_mut().remove(&run.id);
        // A slot is free: a run that waits in the queue may start now.
        self.signals.request_check_now();
        run
    }
}

// --- the runner ---------------------------------------------------------------------------------

/// The live runs of the app: a workflow may have several, each in the registry under its run id.
/// Clones share the runs: the quit path keeps one so it can reach the runner without updating the
/// app's entities.
#[derive(Clone)]
pub(crate) struct WorkflowRunner {
    live: Registry,
    signals: Arc<Signals>,
    limits: Limits,
    /// The app is quitting: nothing new starts.
    quitting: Rc<Cell<bool>>,
    /// Runs whose worktrees are being removed: none of them starts or resumes meanwhile.
    removing: Rc<RefCell<HashSet<String>>>,
}

/// Held while a run's worktrees are removed; the run cannot start or resume until it drops.
pub(crate) struct RemovalGuard {
    removing: Rc<RefCell<HashSet<String>>>,
    run_id: String,
}

impl Drop for RemovalGuard {
    fn drop(&mut self) {
        self.removing.borrow_mut().remove(&self.run_id);
    }
}

impl WorkflowRunner {
    pub(crate) fn new(signals: Arc<Signals>) -> Self {
        Self {
            live: Default::default(),
            signals,
            limits: Limits::default(),
            quitting: Default::default(),
            removing: Default::default(),
        }
    }

    /// Takes the run for the removal of its worktrees. Err says why the run is still using them:
    /// it is going or stopping, a start or a resume was claimed for it, or a removal is going.
    pub(crate) fn begin_removal(
        &self,
        workflow_id: &str,
        run_id: &str,
    ) -> Result<RemovalGuard, String> {
        if self.is_run_live(run_id) {
            return Err(
                "The run is still going or stopping. Wait until it ends, then remove its worktree"
                    .into(),
            );
        }
        if self.signals.has_run(workflow_id, run_id) {
            return Err("The run is about to start or resume. Try again in a moment".into());
        }
        if !self.removing.borrow_mut().insert(run_id.to_string()) {
            return Err("This run's worktree is being removed already".into());
        }
        Ok(RemovalGuard {
            removing: self.removing.clone(),
            run_id: run_id.to_string(),
        })
    }

    #[cfg(test)]
    pub(crate) fn with_limits(mut self, limits: Limits) -> Self {
        self.limits = limits;
        self
    }

    /// The live runs of `workflow_id`, oldest first.
    fn execs_of(&self, workflow_id: &str) -> Vec<Rc<Exec>> {
        let mut execs: Vec<Rc<Exec>> = self
            .live
            .borrow()
            .values()
            .filter(|exec| exec.workflow_id == workflow_id)
            .cloned()
            .collect();
        execs.sort_by_key(|exec| exec.state.borrow().run.started_at);
        execs
    }

    /// Whether the workflow has a run going.
    pub(crate) fn is_running(&self, workflow_id: &str) -> bool {
        self.live_count(workflow_id) > 0
    }

    /// How many runs of the workflow are going.
    pub(crate) fn live_count(&self, workflow_id: &str) -> usize {
        self.live
            .borrow()
            .values()
            .filter(|exec| exec.workflow_id == workflow_id)
            .count()
    }

    /// The runs of the workflow that are going, oldest first, as `(run id, run number, start
    /// time)`, for a page that asks which one to stop.
    pub(crate) fn live_runs_of(&self, workflow_id: &str) -> Vec<(String, Option<u64>, u64)> {
        self.execs_of(workflow_id)
            .iter()
            .map(|exec| {
                let state = exec.state.borrow();
                (state.run.id.clone(), state.run.number, state.run.started_at)
            })
            .collect()
    }

    /// Whether the run is going.
    pub(crate) fn is_run_live(&self, run_id: &str) -> bool {
        self.live.borrow().contains_key(run_id)
    }

    /// Reads the newest run the runner has of the workflow, without copying it out.
    pub(crate) fn with_run<R>(
        &self,
        workflow_id: &str,
        read: impl FnOnce(&WorkflowRun) -> R,
    ) -> Option<R> {
        let exec = self.execs_of(workflow_id).pop()?;
        let state = exec.state.borrow();
        Some(read(&state.run))
    }

    /// Reads one run the runner has, by its id.
    pub(crate) fn with_run_id<R>(
        &self,
        run_id: &str,
        read: impl FnOnce(&WorkflowRun) -> R,
    ) -> Option<R> {
        let exec = self.live.borrow().get(run_id).cloned()?;
        let state = exec.state.borrow();
        Some(read(&state.run))
    }

    /// A `workflow.output` call: records `values` as output of the step whose active session is
    /// one of `ids` (the caller's local id and persisted id). Err says why the call belongs to no
    /// running step, or what is wrong with the values.
    pub(crate) fn record_output(
        &self,
        ids: &[String],
        values: &Json,
    ) -> Result<RecordedOutput, String> {
        let execs: Vec<Rc<Exec>> = self.live.borrow().values().cloned().collect();
        let runs: Vec<WorkflowRun> = execs.iter().map(|e| e.state.borrow().run.clone()).collect();
        let refs: Vec<&WorkflowRun> = runs.iter().collect();
        let target = find_active(&refs, ids).map_err(|e| e.to_string())?;
        let exec = execs
            .iter()
            .find(|e| e.state.borrow().run.id == target.run_id)
            .ok_or_else(|| "The workflow run is no longer going".to_string())?;
        let recorded = exec.record_output(&target, values)?;
        let missing = {
            let state = exec.state.borrow();
            let declared = match state.run.definition.node(&target.node_id).map(|n| &n.kind) {
                Some(NodeKind::Session { output, .. }) => output.keys().cloned().collect(),
                _ => Vec::new(),
            };
            let have = exec.pending_output(&target.node_id, &target.branch);
            declared
                .into_iter()
                .filter(|name: &String| !have.contains_key(name))
                .collect()
        };
        Ok(RecordedOutput {
            node_id: target.node_id,
            round: target.round,
            recorded,
            missing,
        })
    }

    /// The user's answer to a step in "Wait for me" whose session is one of `ids` (its local and
    /// persisted ids). Err says there is nothing to answer: the step was answered, stopped or
    /// failed already.
    pub(crate) fn answer_wait(&self, ids: &[String], choice: WaitChoice) -> Result<(), String> {
        let execs: Vec<Rc<Exec>> = self.live.borrow().values().cloned().collect();
        let mut last = Err("This step is no longer waiting for you.".to_string());
        for exec in execs {
            last = exec.answer_wait(ids, choice);
            if last.is_ok() {
                break;
            }
        }
        last
    }

    /// The runs going, as `(workflow id, run id)`.
    pub(crate) fn running(&self) -> Vec<(String, String)> {
        self.live
            .borrow()
            .iter()
            .map(|(run, exec)| (exec.workflow_id.clone(), run.clone()))
            .collect()
    }

    /// Runs `run` (made by the engine's `start_run` or `resume_run`). False when the run is
    /// going already or the workflow has all the slots of its `concurrency.max` taken; a run
    /// that is already over is only written.
    pub(crate) fn start(
        &self,
        host: Rc<dyn WorkflowHost>,
        cx: &AsyncApp,
        run: WorkflowRun,
    ) -> bool {
        let workflow_id = run.workflow_id.clone();
        let run_id = run.id.clone();
        if self.quitting.get()
            || self.removing.borrow().contains(&run_id)
            || self.is_run_live(&run_id)
            || self.live_count(&workflow_id) >= run.definition.concurrency.limit()
        {
            return false;
        }
        let exec = Rc::new(Exec {
            host,
            cx: cx.clone(),
            workflow_id: workflow_id.clone(),
            limits: self.limits,
            nonce: monocode_core::mono::new_id().chars().take(6).collect(),
            state: RefCell::new(State {
                run,
                command_seq: 0,
                commands: BTreeSet::new(),
                dirty: false,
                writing: false,
            }),
            active: Cell::new(0),
            stop: Signal::default(),
            stopping: Cell::new(false),
            stopped: Signal::default(),
            nudge: Nudge::default(),
            flushed: Nudge::default(),
            waits: Default::default(),
            starts: Gate::default(),
            trees: Gate::default(),
            registry: self.live.clone(),
            signals: self.signals.clone(),
            quitting: Cell::new(false),
        });
        self.live.borrow_mut().insert(run_id.clone(), exec.clone());
        self.signals.claim_run(&workflow_id, &run_id);
        cx.spawn(async move |_| {
            exec.execute().await;
        })
        .detach();
        true
    }

    /// Continues an interrupted run from where it stopped. False when the workflow has no free
    /// slot or the run was not interrupted.
    pub(crate) fn resume(
        &self,
        host: Rc<dyn WorkflowHost>,
        cx: &AsyncApp,
        stored: &WorkflowRun,
    ) -> bool {
        stored.status == RunStatus::Interrupted && self.start(host, cx, resume_run(stored))
    }

    /// Stops runs of the workflow: the one named by `run_id`, or, without one, every run it has
    /// going. False when none was going or each is already stopping.
    pub(crate) fn stop(&self, workflow_id: &str, run_id: Option<&str>) -> bool {
        let execs = match run_id {
            Some(run_id) => self
                .live
                .borrow()
                .get(run_id)
                .filter(|exec| exec.workflow_id == workflow_id)
                .cloned()
                .into_iter()
                .collect(),
            None => self.execs_of(workflow_id),
        };
        let mut stopped = false;
        for exec in execs {
            stopped |= exec.request_stop();
        }
        stopped
    }

    /// The app is quitting. Every run lets go at once and writes nothing more: its file keeps
    /// saying `running`, so the next start marks it interrupted and it can be resumed. Nothing new
    /// starts afterwards. Nothing is posted (a quit is no stop) and no node fails because a command or session was killed with
    /// the app. Commands are killed by the caller (`command::cancel_all`), not waited for.
    pub(crate) fn quit(&self) {
        self.quitting.set(true);
        let all: Vec<_> = self.live.borrow().values().cloned().collect();
        for exec in all {
            exec.quit();
        }
    }

    pub(crate) fn is_quitting(&self) -> bool {
        self.quitting.get()
    }
}
