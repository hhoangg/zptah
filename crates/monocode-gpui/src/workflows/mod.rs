//! Workflows: the app-wide runner, its clock and the session start that has no Mono behind it.
//! The files are the global store (`<app data>/workflows/`); the entity owns the runner, what the
//! app keeps while runs go, and the checks that start what is due. The pages (`monos::workflows_ui`)
//! and the Monos' `workflows.*` actions call into it; neither owns a run.
use crate::{monos::Monos, runtime::Database, shell::Shell};
use gpui::{App, AppContext as _, Context, Entity, WeakEntity};
use monocode_core::{
    mono::workflows::{
        cli::{Signals, StoreHost},
        global::WorkflowsDir,
        model::{Workflow, WorkflowRun},
        models::Probe,
        preflight::Preflight,
    },
    provider::Kind,
};
use std::sync::Arc;

mod decide;
mod extract;
mod runner;
mod schedule;
mod sessions;
mod summary;
mod terminal;
mod waits;
mod worktrees;

pub(crate) use runner::{Live, WaitChoice, WorkflowRunner};
#[cfg(test)]
pub(crate) use runner::{SessionRequest, WaitNotice};
pub(crate) use schedule::begin_quit;
pub(crate) use waits::WaitBanner;

/// The persisted ids of every session the run files name, for the zap badge. Blocks (reads the
/// files): use it off the UI thread.
pub(crate) fn run_session_ids(dir: &WorkflowsDir) -> Vec<String> {
    use monocode_core::mono::workflows::store::{load_runs, load_workflows};
    let Ok(loaded) = load_workflows(dir) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for workflow in &loaded.workflows {
        for run in load_runs(dir, &workflow.id).unwrap_or_default() {
            for node in &run.nodes {
                ids.extend(node.session_id.clone());
                for round in node.history.iter().flatten() {
                    ids.extend(round.session_id.clone());
                }
            }
        }
    }
    ids.sort();
    ids.dedup();
    ids
}

/// How the app judges what a workflow needs, injected so tests never reach real CLIs, shells or
/// MCP servers.
#[derive(Clone, Copy)]
pub(crate) struct Probes {
    /// How a harness is probed for the model catalog.
    pub(crate) models: fn(Kind) -> Probe,
    /// How a workflow's `requires` are checked.
    pub(crate) requirements: fn(&Workflow) -> Preflight,
    /// Whether a step's session only gets its prompt in the composer and no turn runs. Tests set
    /// it so no step ever reaches a real CLI; the run then waits for a turn that never comes.
    /// The field does not exist in a real build, so no setting can put a run's prompt aside.
    #[cfg(test)]
    pub(crate) draft_sessions: bool,
}

impl Probes {
    pub(crate) fn new() -> Self {
        #[cfg(test)]
        return Self {
            models: runner::trusting_probe,
            requirements: runner::trusting_preflight,
            draft_sessions: true,
        };
        #[cfg(not(test))]
        Self {
            models: monocode_core::mono::workflows::models::native_probe,
            requirements: monocode_core::mono::workflows::preflight::native_preflight,
        }
    }

    /// How a harness is probed when a workflow's models are checked.
    pub(crate) fn probe(&self) -> fn(Kind) -> Probe {
        self.models
    }

    /// How a workflow's `requires` are checked before a run starts.
    pub(crate) fn preflight(&self) -> fn(&Workflow) -> Preflight {
        self.requirements
    }

    /// Swaps how requirements are checked, for a test that wants some missing.
    #[cfg(test)]
    pub(crate) fn set_preflight(&mut self, preflight: fn(&Workflow) -> Preflight) {
        self.requirements = preflight;
    }

    #[cfg(test)]
    pub(crate) fn with_probe(mut self, probe: fn(Kind) -> Probe) -> Self {
        self.models = probe;
        self
    }
}

impl Default for Probes {
    fn default() -> Self {
        Self::new()
    }
}

/// The app's workflows: one runner for every workflow, no Mono owning anything.
pub(crate) struct Workflows {
    pub(in crate::workflows) shell: WeakEntity<Shell>,
    pub(in crate::workflows) monos: WeakEntity<Monos>,
    pub(in crate::workflows) store: Database,
    pub(in crate::workflows) dir: WorkflowsDir,
    /// What the `workflows.*` actions and the runner tell each other (see `Signals`). Shared with
    /// `Monos`, whose actions read `is_running` and ring `request_check_now`.
    pub(crate) signals: Arc<Signals>,
    pub(in crate::workflows) runner: WorkflowRunner,
    pub(in crate::workflows) live: Live,
    pub(in crate::workflows) checks: schedule::Checks,
    /// How workflows are judged (model catalog and requirements); a test swaps it in one place.
    probes: Probes,
}

impl Workflows {
    /// Makes the entity. It drives the runner and the signals `monos` made (the pages and the
    /// Monos' actions read them there). Called once, when the shell is made.
    pub(crate) fn create(
        store: Database,
        shell: WeakEntity<Shell>,
        monos: &Entity<Monos>,
        cx: &mut Context<Shell>,
    ) -> Entity<Self> {
        let (signals, runner, dir) = {
            let monos = monos.read(cx);
            (
                monos.workflow_signals.clone(),
                monos.workflows.clone(),
                monos.workflow_dir.clone(),
            )
        };
        let monos_weak = monos.downgrade();
        let this = cx.new(|cx| Self::new(store, shell, monos_weak, signals, runner, dir, cx));
        let weak = this.downgrade();
        monos.update(cx, |monos, _| monos.workflows_entity = Some(weak));
        this
    }

    fn new(
        store: Database,
        shell: WeakEntity<Shell>,
        monos: WeakEntity<Monos>,
        signals: Arc<Signals>,
        runner: WorkflowRunner,
        dir: WorkflowsDir,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            shell,
            monos,
            store,
            dir,
            runner,
            signals,
            live: Live::new(),
            checks: Default::default(),
            probes: Probes::new(),
        };
        cx.set_global(schedule::QuitTarget(this.runner.clone()));
        this.start_workflow_checks(cx);
        this
    }

    /// What the app judges workflows with.
    pub(crate) fn probes(&self) -> Probes {
        self.probes
    }

    /// What the app judges workflows with, for a test to swap.
    #[cfg(test)]
    pub(crate) fn probes_mut(&mut self) -> &mut Probes {
        &mut self.probes
    }

    /// The global files, as a host the `workflows.*` actions run on. Blocks (files, CLI probes):
    /// use it off the UI thread. Without a caller it is the user: any app project may be named.
    pub(crate) fn store_host(&self, cx: &App) -> StoreHost {
        let probes = self.probes;
        let host = StoreHost::new(self.dir.clone(), self.signals.clone())
            .with_probe(probes.probe())
            .with_preflight(probes.preflight());
        match crate::monos::workflow_account_check(cx) {
            Some(check) => host.with_accounts(check),
            None => host,
        }
    }

    pub(crate) fn dir(&self) -> &WorkflowsDir {
        &self.dir
    }

    /// Whether a run of the workflow is going.
    #[cfg(test)]
    pub(crate) fn is_running(&self, workflow_id: &str) -> bool {
        self.runner.is_running(workflow_id)
    }

    /// Workflow runs going now, as `(run id, workflow name)`.
    pub(crate) fn running_workflows(&self) -> Vec<(String, String)> {
        self.runner
            .running()
            .into_iter()
            .map(|(workflow, run)| {
                let name = self
                    .runner
                    .with_run_id(&run, |run| run.definition.name.clone())
                    .unwrap_or(workflow);
                (run, name)
            })
            .collect()
    }

    /// Runs `run` (made by the engine's `start_run`). False when the workflow has no free slot.
    pub(crate) fn start_run(&mut self, run: WorkflowRun, cx: &mut Context<Self>) -> bool {
        let host = self.workflow_host(cx);
        self.runner.start(host, &cx.to_async(), run)
    }

    /// Continues an interrupted run. False when the workflow is running or the run was not
    /// interrupted.
    pub(crate) fn resume_run(&mut self, stored: &WorkflowRun, cx: &mut Context<Self>) -> bool {
        let host = self.workflow_host(cx);
        self.runner.resume(host, &cx.to_async(), stored)
    }

    /// A `workflow.output` call (see `WorkflowRunner::record_output`): the step the caller
    /// session is running gets the values, and the reply says what it still lacks. `ids` are
    /// every id the caller goes by (its local id and the persisted one).
    pub(crate) fn accept_output(
        &mut self,
        ids: &[String],
        input: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let values = monocode_core::mono::workflows::ordered::Json::from(input);
        let done = self.runner.record_output(ids, &values)?;
        Ok(serde_json::json!({
            "step": done.node_id,
            "round": done.round,
            "recorded": done.recorded,
            "missing": done.missing,
        }))
    }

    /// Stops the run `run_id` of the workflow, or every run it has going when none is named.
    /// False when none was going.
    pub(crate) fn stop_run(&mut self, workflow_id: &str, run_id: Option<&str>) -> bool {
        self.runner.stop(workflow_id, run_id)
    }
}
