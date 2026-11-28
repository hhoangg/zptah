//! The app side of a workflow run: sessions started like the app starts any session, follow-ups,
//! one-shot AI decisions and the delivery of the run summary. Nothing here knows a Mono: the
//! summary reaches a Mono's chat only through `Monos::workflow_post`.
use super::sync::race;
use super::{
    CloudInstanceInfo, DecisionRequest, ExtractRequest, Fut, Outcome, STOPPED_BY_USER,
    SessionFacts, SessionInfo, SessionRequest, StartedSession, Summary, WaitNotice, WorkflowHost,
};
use crate::conversation::{Conversation, SessionChanged, TurnEnd, WorkflowTurn};
use crate::runtime::locked;
use crate::workflows::Workflows;
use async_channel::{Receiver, Sender, bounded};
use gpui::{AsyncApp, Context, Entity, Subscription, WeakEntity};
use monocode_core::{
    mono::workflows::{
        command::{self, CommandResult},
        global::WorkflowsDir,
        model::WorkflowRun,
        store::update_runs,
    },
    provider::Kind,
};
use std::{collections::HashMap, rc::Rc, time::Duration};

/// How often a watched session is looked at besides when its conversation changes.
const WATCH_POLL: Duration = Duration::from_secs(1);
/// A session that never shows a turn going is not waited for forever.
const UNSTARTED_POLLS: u32 = 10;

/// A probe that finds every harness installed and lists no models, so a harness is never
/// refused; what a test wants refused it says itself.
#[cfg(test)]
pub(crate) fn trusting_probe(_: Kind) -> monocode_core::mono::workflows::models::Probe {
    monocode_core::mono::workflows::models::Probe {
        installed: true,
        models: None,
    }
}

/// Requirements are all found, so a test never reaches a real CLI, login shell or MCP server;
/// what a test wants missing it says itself.
#[cfg(test)]
pub(crate) fn trusting_preflight(
    w: &monocode_core::mono::workflows::model::Workflow,
) -> monocode_core::mono::workflows::preflight::Preflight {
    monocode_core::mono::workflows::preflight::Preflight {
        statuses: w
            .requires
            .iter()
            .map(
                |requirement| monocode_core::mono::workflows::preflight::RequirementStatus {
                    requirement: requirement.clone(),
                    met: true,
                    detail: None,
                },
            )
            .collect(),
    }
}

/// What the app keeps for its workflow runs next to the runner.
pub(crate) struct Live {
    /// Wake-ups for a watched session: its conversation changed.
    wakers: HashMap<i64, (Sender<()>, Receiver<()>)>,
    /// Ends when the run lets the session go.
    watches: HashMap<i64, Subscription>,
    /// Persisted session ids (what run files hold) to local ones.
    pub(in crate::workflows) ids: HashMap<String, i64>,
    /// The pane state of each terminal step's session, by local id.
    pub(in crate::workflows) tui: HashMap<i64, crate::workflows::terminal::TuiTurn>,
    /// The banner of each session a "Wait for me" step waits on, by local id.
    pub(in crate::workflows) waits: HashMap<i64, crate::workflows::WaitBanner>,
    /// The "needs you" notices raised, for tests.
    #[cfg(test)]
    pub(crate) needs_you: Vec<(i64, String)>,
}

impl Live {
    pub(crate) fn new() -> Self {
        Self {
            wakers: HashMap::new(),
            watches: HashMap::new(),
            ids: HashMap::new(),
            tui: HashMap::new(),
            waits: HashMap::new(),
            #[cfg(test)]
            needs_you: Vec::new(),
        }
    }

    /// What is still kept for sessions.
    #[cfg(test)]
    pub(crate) fn kept(&self) -> usize {
        self.watches.len() + self.wakers.len() + self.ids.len() + self.tui.len() + self.waits.len()
    }

    pub(in crate::workflows) fn wake(&mut self, session: i64) -> &(Sender<()>, Receiver<()>) {
        self.wakers.entry(session).or_insert_with(|| bounded(1))
    }
}

/// How a conversation's turn maps onto the outcome a run reads.
pub(crate) fn outcome_of(turn: WorkflowTurn) -> Outcome {
    match turn.end {
        TurnEnd::Completed => Outcome::completed(turn.reply),
        TurnEnd::Cancelled => Outcome {
            text: turn.reply,
            ..Outcome::cancelled(STOPPED_BY_USER)
        },
        TurnEnd::Failed(status) => Outcome {
            text: turn.reply,
            ..Outcome::failed(status)
        },
    }
}

/// The harness a stored model id names (`grok` of `grok:grok-4.6`).
pub(in crate::workflows) fn model_harness(model: &str) -> Option<&str> {
    let (harness, rest) = model.split_once(':')?;
    (Kind::parse(harness).is_some() && !rest.is_empty()).then_some(harness)
}

pub(in crate::workflows) enum Seen {
    Working,
    /// A cloud session whose instance is still being made ready, or whose first prompt waits
    /// for it: there is no turn to see yet, and none to give up on.
    Starting,
    Over(WorkflowTurn),
}

/// What one look at a watched session means: its outcome, or `None` to keep waiting. `armed`
/// says a turn has been seen going; an idle session that never was is given up on after a while,
/// unless it ended in an error (a prompt that failed to start says why).
pub(super) fn look(seen: Seen, armed: &mut bool, unstarted: &mut u32) -> Option<Outcome> {
    match seen {
        Seen::Working => {
            *armed = true;
            None
        }
        // A start takes minutes and the node's own timeout covers it; the wait for a turn to
        // appear begins once it is over.
        Seen::Starting => {
            *unstarted = 0;
            None
        }
        Seen::Over(turn) if *armed || matches!(turn.end, TurnEnd::Failed(_)) => {
            Some(outcome_of(turn))
        }
        Seen::Over(_) => {
            *unstarted += 1;
            (*unstarted > UNSTARTED_POLLS)
                .then(|| Outcome::failed("The session did not start a turn."))
        }
    }
}

/// Waits for the turn a session is in to end. Its conversation wakes this; the poll is a net.
async fn settle(
    workflows: WeakEntity<Workflows>,
    mut cx: AsyncApp,
    session: i64,
    wake: Receiver<()>,
    mut armed: bool,
) -> Outcome {
    let mut unstarted = 0;
    loop {
        match workflows.update(&mut cx, |w, cx| w.workflow_turn(session, cx)) {
            Err(_) => return Outcome::failed("The app is closing."),
            Ok(None) => return Outcome::failed("The session is no longer available."),
            Ok(Some(seen)) => {
                if let Some(outcome) = look(seen, &mut armed, &mut unstarted) {
                    return outcome;
                }
            }
        }
        let _ = race(wake.recv(), cx.background_executor().timer(WATCH_POLL)).await;
    }
}

/// Waits until no turn of a session is running. Its conversation wakes this; the poll is a net.
async fn idle(
    workflows: WeakEntity<Workflows>,
    mut cx: AsyncApp,
    session: i64,
    wake: Receiver<()>,
) {
    loop {
        match workflows.update(&mut cx, |w, cx| w.workflow_busy(session, cx)) {
            Ok(Some(true)) => {}
            // Idle, gone, or the app is closing: nothing is running.
            _ => return,
        }
        let _ = race(wake.recv(), cx.background_executor().timer(WATCH_POLL)).await;
    }
}

impl Workflows {
    pub(in crate::workflows) fn workflow_local_id(&mut self, persisted: &str) -> Option<i64> {
        if let Some(id) = self.live.ids.get(persisted) {
            return Some(*id);
        }
        let store = locked(&self.store).ok()?;
        let id = match store.resolve_session_id(persisted) {
            Ok(id) => id,
            // A cloud session is not in the sessions table, so its persisted id (`remote:...`)
            // is found among the remote sessions. The id a start was given while it was still
            // being made ends in the local id, which stays the session's after the start.
            Err(_) if persisted.starts_with("remote:") => {
                let listed = store
                    .sessions()
                    .ok()?
                    .into_iter()
                    .map(|s| s.id)
                    .filter(|id| *id < 0)
                    .find(|id| store.persisted_session_id(*id).ok().as_deref() == Some(persisted));
                let from_tail = || {
                    let tail = persisted.rsplit(':').next()?.parse::<i64>().ok()?;
                    (tail < 0 && store.remote_session(tail).ok().flatten().is_some())
                        .then_some(tail)
                };
                listed.or_else(from_tail)?
            }
            Err(_) => return None,
        };
        drop(store);
        self.live.ids.insert(persisted.into(), id);
        Some(id)
    }

    pub(in crate::workflows) fn workflow_view(
        &mut self,
        persisted: &str,
        cx: &mut Context<Self>,
    ) -> Option<(i64, Entity<Conversation>)> {
        let id = self.workflow_local_id(persisted)?;
        let view = self
            .shell
            .upgrade()?
            .update(cx, |shell, cx| shell.mono_ensure_session(id, cx))?;
        Some((id, view))
    }

    /// Wakes whoever waits on the session when its conversation changes.
    pub(in crate::workflows) fn workflow_watch(
        &mut self,
        session: i64,
        view: &Entity<Conversation>,
        cx: &mut Context<Self>,
    ) {
        if self.live.watches.contains_key(&session) {
            return;
        }
        let wake = self.live.wake(session).0.clone();
        let watch = cx.subscribe(view, move |_, _, _: &SessionChanged, _| {
            let _ = wake.try_send(());
        });
        self.live.watches.insert(session, watch);
    }

    /// A run is done with a session: its watch, wake-up and cached id go.
    pub(in crate::workflows) fn workflow_release(&mut self, persisted: &str) {
        if let Some(local) = self.live.ids.remove(persisted) {
            self.live.watches.remove(&local);
            self.live.wakers.remove(&local);
            self.live.tui.remove(&local);
            self.live.waits.remove(&local);
        }
    }

    pub(in crate::workflows) fn workflow_settled(
        &mut self,
        session: i64,
        armed: bool,
        cx: &mut Context<Self>,
    ) -> Fut<Outcome> {
        let wake = self.live.wake(session).1.clone();
        Box::pin(settle(
            cx.weak_entity(),
            cx.to_async(),
            session,
            wake,
            armed,
        ))
    }

    /// Whether the session is working on a turn now; `None` when it is gone.
    pub(in crate::workflows) fn workflow_busy(
        &mut self,
        session: i64,
        cx: &mut Context<Self>,
    ) -> Option<bool> {
        if let Some(turn) = self.live.tui.get(&session) {
            return Some(turn.busy());
        }
        let view = self
            .shell
            .upgrade()?
            .update(cx, |shell, cx| shell.mono_ensure_session(session, cx))?;
        Some(view.read(cx).workflow_working())
    }

    /// Waits for the turn the session may be in to end. The banner of a waiting step says it
    /// waits for the turn while it does.
    fn workflow_turn_ends(&mut self, persisted: &str, cx: &mut Context<Self>) -> Fut<()> {
        let Some(local) = self.workflow_local_id(persisted) else {
            return Box::pin(async {});
        };
        if self.workflow_busy(local, cx) != Some(true) {
            return Box::pin(async {});
        }
        // A terminal session has no conversation to watch: its pane's events wake the wait.
        if let Some((_, view)) = self.workflow_view(persisted, cx) {
            self.workflow_watch(local, &view, cx);
        }
        if self.wait_for_turn(local)
            && let Some(shell) = self.shell.upgrade()
        {
            shell.update(cx, |_, cx| cx.notify());
        }
        let wake = self.live.wake(local).1.clone();
        Box::pin(idle(cx.weak_entity(), cx.to_async(), local, wake))
    }

    fn workflow_turn(&mut self, session: i64, cx: &mut Context<Self>) -> Option<Seen> {
        // A terminal step has no conversation: its pane's events say how the turn goes.
        if let Some(turn) = self.live.tui.get(&session) {
            return Some(turn.seen());
        }
        let view = self
            .shell
            .upgrade()?
            .update(cx, |shell, cx| shell.mono_ensure_session(session, cx))?;
        let view = view.read(cx);
        if view.is_cloud_session()
            && let Some(wait) = self.workflow_cloud_wait(session, view)
        {
            return Some(wait);
        }
        Some(match view.workflow_turn(cx) {
            Some(turn) => Seen::Over(turn),
            None => Seen::Working,
        })
    }

    /// What a cloud session still waits for before its first turn: the start the store keeps a
    /// record of while the instance is created, connected or set up (the record is the truth,
    /// whether or not the view has heard of it yet), or the first prompt waiting for the machine.
    /// A start that failed ends the wait with its error.
    fn workflow_cloud_wait(&self, session: i64, view: &Conversation) -> Option<Seen> {
        let record = locked(&self.store)
            .ok()
            .and_then(|store| monocode_core::cloud_sessions::start_record(&store, session));
        match record {
            Some(record) => Some(match record.error {
                Some(error) => Seen::Over(WorkflowTurn {
                    end: TurnEnd::Failed(format!("The cloud instance could not start: {error}")),
                    reply: String::new(),
                }),
                None => Seen::Starting,
            }),
            None => view
                .workflow_first_prompt_waiting()
                .then_some(Seen::Starting),
        }
    }

    /// Sends a follow-up into a session a node started earlier.
    fn workflow_send(
        &mut self,
        persisted: &str,
        prompt: String,
        cx: &mut Context<Self>,
    ) -> Result<Fut<Outcome>, String> {
        let (local, view) = self
            .workflow_view(persisted, cx)
            .ok_or("The session is no longer available.")?;
        view.update(cx, |v, cx| v.mono_external_send(prompt, false, cx))?;
        self.workflow_watch(local, &view, cx);
        let armed = view.read(cx).workflow_working();
        Ok(self.workflow_settled(local, armed, cx))
    }

    fn workflow_stop(&mut self, persisted: &str, cx: &mut Context<Self>) -> Result<(), String> {
        // A terminal step ends with its CLI.
        if let Some(local) = self
            .workflow_local_id(persisted)
            .filter(|local| self.live.tui.contains_key(local))
        {
            if let Some(shell) = self.shell.upgrade() {
                shell.update(cx, |shell, cx| shell.workflow_stop_tui(local, cx));
            }
            return Ok(());
        }
        if let Some((_, view)) = self.workflow_view(persisted, cx) {
            view.update(cx, |v, cx| {
                v.mono_stop_turn(cx);
                // A cloud session still starting must not send its first prompt once it is
                // ready. The instance itself is never stopped or deleted here.
                v.workflow_drop_first_prompt(cx);
            });
        }
        Ok(())
    }

    fn workflow_info(&mut self, persisted: &str, cx: &mut Context<Self>) -> Option<SessionInfo> {
        if let Some(local) = self
            .workflow_local_id(persisted)
            .filter(|local| self.live.tui.contains_key(local))
        {
            let summary = self.shell.upgrade()?.read(cx).workflow_tui_summary(local)?;
            return Some(SessionInfo {
                session_id: persisted.into(),
                local_id: Some(local),
                cwd: summary.cwd,
                title: summary.title,
                harness: summary.provider,
                model: None,
            });
        }
        let (local, view) = self.workflow_view(persisted, cx)?;
        let view = view.read(cx);
        Some(SessionInfo {
            session_id: persisted.into(),
            local_id: Some(local),
            cwd: view.summary.cwd.clone(),
            title: view.summary.title.clone(),
            harness: view.summary.provider.clone(),
            model: view.mono_model(),
        })
    }

    /// The instance a cloud session runs on, from the store alone (nothing is woken or asked).
    /// A session whose instance is not named yet is still starting: the start record names it
    /// once it has one.
    fn workflow_cloud_instance(&mut self, persisted: &str) -> Option<CloudInstanceInfo> {
        let local = self.workflow_local_id(persisted)?;
        let store = locked(&self.store).ok()?;
        let cloud = store
            .remote_session(local)
            .ok()
            .flatten()
            .and_then(|remote| remote.project.cloud())
            .or_else(|| {
                monocode_core::cloud_sessions::start_record(&store, local)
                    .and_then(|record| record.cloud())
            })?;
        Some(CloudInstanceInfo {
            kind: cloud.kind.api_name().into(),
            name: Some(cloud.name),
        })
    }

    /// The real host for the runs of this entity.
    pub(crate) fn workflow_host(&self, cx: &mut Context<Self>) -> Rc<dyn WorkflowHost> {
        Rc::new(AppHost {
            workflows: cx.weak_entity(),
            cx: cx.to_async(),
            dir: self.dir.clone(),
        })
    }
}

/// `WorkflowHost` over the app's `Workflows` entity. Its methods are called from the run's tasks,
/// never from inside an update of `Workflows`.
pub(crate) struct AppHost {
    workflows: WeakEntity<Workflows>,
    cx: AsyncApp,
    dir: WorkflowsDir,
}

impl AppHost {
    fn with<R>(&self, f: impl FnOnce(&mut Workflows, &mut Context<Workflows>) -> R) -> Option<R> {
        self.workflows.update(&mut self.cx.clone(), f).ok()
    }
}

fn closing<T>() -> Result<T, String> {
    Err("The app is closing".into())
}

impl WorkflowHost for AppHost {
    fn start_session(&self, req: SessionRequest) -> Fut<Result<StartedSession, String>> {
        let (workflows, mut cx) = (self.workflows.clone(), self.cx.clone());
        Box::pin(async move {
            let (tx, rx) = bounded(1);
            let started = workflows.update(&mut cx, |w, cx| {
                #[cfg(test)]
                let draft = w.probes().draft_sessions && req.target.is_none();
                #[cfg(not(test))]
                let draft = false;
                w.workflow_session(req, draft, tx, cx)
            });
            match started {
                Err(_) => return closing(),
                Ok(Err(error)) => return Err(error),
                Ok(Ok(())) => {}
            }
            rx.recv().await.unwrap_or_else(|_| closing())
        })
    }

    fn send(&self, session_id: &str, prompt: String) -> Fut<Outcome> {
        let sent = self.with(|w, cx| w.workflow_send(session_id, prompt, cx));
        match sent {
            Some(Ok(settled)) => settled,
            Some(Err(error)) => Box::pin(async move { Outcome::failed(error) }),
            None => Box::pin(async { Outcome::failed("The app is closing") }),
        }
    }

    fn stop(&self, session_id: &str) -> Fut<Result<(), String>> {
        let stopped = self
            .with(|w, cx| w.workflow_stop(session_id, cx))
            .unwrap_or_else(closing);
        Box::pin(async move { stopped })
    }

    fn final_reply(&self, session_id: &str) -> String {
        self.with(|w, cx| {
            w.workflow_view(session_id, cx)
                .map(|(_, view)| view.read(cx).workflow_reply(cx))
        })
        .flatten()
        .unwrap_or_default()
    }

    fn waiting_approval(&self, session_id: &str) -> bool {
        self.with(|w, cx| {
            w.workflow_view(session_id, cx)
                .is_some_and(|(_, view)| view.read(cx).needs_approval())
        })
        .unwrap_or(false)
    }

    fn release_session(&self, session_id: &str) {
        self.with(|w, _| w.workflow_release(session_id));
    }

    fn session_info(&self, session_id: &str) -> Option<SessionInfo> {
        self.with(|w, cx| w.workflow_info(session_id, cx)).flatten()
    }

    fn cloud_instance(&self, session_id: &str) -> Option<CloudInstanceInfo> {
        self.with(|w, _| w.workflow_cloud_instance(session_id))
            .flatten()
    }

    fn run_command(
        &self,
        cwd: String,
        command: String,
        timeout_ms: u64,
        id: String,
    ) -> Fut<Result<CommandResult, String>> {
        // The call blocks until the command ends, possibly for hours. Not on the UI thread, and
        // not on the executor's pool either (fixed at the CPU count on Linux): a few long
        // commands would starve every other background job. Each gets a thread of its own.
        let (done, result) = bounded(1);
        let spawned = std::thread::Builder::new()
            .name("workflow-command".into())
            .spawn(move || {
                let _ = done.send_blocking(command::run_command(&id, &command, &cwd, timeout_ms));
            });
        Box::pin(async move {
            if let Err(error) = spawned {
                return Err(format!("Could not start the command: {error}"));
            }
            result
                .recv()
                .await
                .unwrap_or_else(|_| Err("The command ended without a result".into()))
        })
    }

    fn cancel_command(&self, id: &str) -> Fut<Result<bool, String>> {
        let cancelled = command::cancel(id);
        Box::pin(async move { Ok(cancelled) })
    }

    fn decide(&self, request: DecisionRequest) -> Fut<Result<String, String>> {
        let setup = self
            .with(|w, cx| w.decision_setup(&request, cx))
            .unwrap_or_else(|| Err("The app is closing".into()));
        let executor = self.cx.background_executor().clone();
        Box::pin(crate::workflows::decide::decide(setup, request, executor))
    }

    fn head_commit(&self, dir: String) -> Fut<Option<String>> {
        let work = self.cx.background_executor().spawn(async move {
            monocode_core::mono::workflows::session_facts::head_commit(std::path::Path::new(&dir))
        });
        Box::pin(work)
    }

    fn session_facts(&self, session_id: &str, base: Option<String>) -> Fut<SessionFacts> {
        let dir = self.worktree_of(session_id);
        let work = self.cx.background_executor().spawn(async move {
            match dir {
                Some(dir) => monocode_core::mono::workflows::session_facts::session_facts(
                    std::path::Path::new(&dir),
                    base.as_deref(),
                ),
                None => SessionFacts::default(),
            }
        });
        Box::pin(work)
    }

    fn extract(&self, request: ExtractRequest) -> Fut<Result<String, String>> {
        let voice = self
            .with(|w, cx| w.extraction_voice(&request.session_id, cx))
            .unwrap_or_else(|| Err("The app is closing".into()));
        let store = self.with(|w, _| w.store.clone());
        let executor = self.cx.background_executor().clone();
        match store {
            Some(store) => Box::pin(crate::workflows::extract::extract(
                voice, store, request, executor,
            )),
            None => Box::pin(async { Err("The app is closing".into()) }),
        }
    }

    fn wait_for_user(&self, notice: WaitNotice) {
        self.with(|w, cx| w.workflow_wait(notice, cx));
    }

    fn turn_ends(&self, session_id: &str) -> Fut<()> {
        self.with(|w, cx| w.workflow_turn_ends(session_id, cx))
            .unwrap_or_else(|| Box::pin(async {}))
    }

    fn wait_over(&self, session_id: &str) {
        self.with(|w, cx| w.workflow_wait_over(session_id, cx));
    }

    fn post(&self, summary: Summary) {
        self.with(|w, cx| w.deliver(summary, cx));
    }

    fn project_path(&self, input: &str) -> Option<String> {
        let named = input.trim();
        if named.is_empty() {
            return None;
        }
        self.with(|w, cx| w.app_project(named, cx)).flatten()
    }

    fn worktree_of(&self, session_id: &str) -> Option<String> {
        self.with(|w, cx| {
            if let Some(local) = w
                .workflow_local_id(session_id)
                .filter(|local| w.live.tui.contains_key(local))
            {
                return w.shell.upgrade()?.read(cx).workflow_tui_dir(local);
            }
            let (_, view) = w.workflow_view(session_id, cx)?;
            let view = view.read(cx);
            Some(
                view.settings
                    .worktree_cwd
                    .clone()
                    .unwrap_or_else(|| view.summary.cwd.clone()),
            )
        })
        .flatten()
    }

    fn save_run(&self, run: WorkflowRun) -> Fut<Result<(), String>> {
        let executor = self.cx.background_executor().clone();
        let dir = self.dir.clone();
        Box::pin(async move {
            executor
                .spawn(async move {
                    update_runs(&dir, &run.workflow_id.clone(), |runs| {
                        let list = if runs.iter().any(|entry| entry.id == run.id) {
                            runs.into_iter()
                                .map(|entry| {
                                    if entry.id == run.id {
                                        run.clone()
                                    } else {
                                        entry
                                    }
                                })
                                .collect()
                        } else {
                            std::iter::once(run.clone()).chain(runs).collect()
                        };
                        Ok((list, ()))
                    })
                })
                .await
        })
    }
}
