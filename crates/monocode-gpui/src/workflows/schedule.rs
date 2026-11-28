//! When workflows run: the app side of `useMonoWorkflows.ts`, now over the one global store. The
//! clock ticks every 60 s (as in TS) and the `workflows.*` actions ring a waker, so a `run`,
//! `stop` or `resumeRun` acts at once instead of on the next look. The per-Mono workflow files
//! of older versions are never looked at, so what an old file had enabled no longer fires.
//!
//! A look reads the workflow files and probes the model catalog, so it runs off the UI thread
//! (`core::tick`); what it found is started here, on the UI thread, through the runner. Only one
//! look goes at a time; one asked for meanwhile follows as soon as it ends. The first look after
//! startup also marks runs a previous session left `running` as interrupted.
use super::{
    Probes, Workflows,
    runner::{Summary, WorkflowRunner},
};
use gpui::Context;
use monocode_core::mono::workflows::{
    cli::Signals,
    global::WorkflowsDir,
    model::RunSummary,
    tick::{self, Found, Start, Tick},
};
use std::time::Duration;

/// How often workflows are looked at besides when asked.
const CHECK_MS: u64 = 60_000;

#[derive(Default)]
pub(super) struct Checks {
    /// A look is going.
    pub(super) checking: bool,
    /// A look was asked for meanwhile.
    pub(super) again: bool,
    /// The sweep for interrupted runs went through.
    pub(super) swept: bool,
    _tasks: Vec<gpui::Task<()>>,
}

/// What a look found.
#[derive(Default)]
pub(super) struct Looked {
    /// The sweep ran (it was asked for) and met no error.
    pub(super) swept: bool,
    pub(super) found: Option<Found>,
    pub(super) errors: Vec<String>,
}

/// One look at the workflow files. Blocks (files, CLI probes): call it off the UI thread.
/// `driven` is the runs the runner is driving, which the sweep leaves alone.
pub(super) fn look_at(
    dir: &WorkflowsDir,
    driven: &[(String, String)],
    sweep: bool,
    signals: &Signals,
    probes: Probes,
    now: u64,
) -> Looked {
    let mut looked = Looked {
        swept: sweep,
        ..Default::default()
    };
    if sweep && let Err(error) = tick::sweep_interrupted(dir, driven) {
        looked.swept = false;
        looked.errors.push(format!(
            "Could not check for interrupted workflow runs: {error}"
        ));
    }
    let running = |workflow: &str| signals.is_running(workflow);
    let tick = Tick {
        now,
        is_running: &running,
        probe: probes.models,
        preflight: &probes.requirements,
        on_claim: &|workflow: &str| signals.set_running(workflow, true),
    };
    match tick::check(dir, &tick) {
        Ok(found) if found.starts.is_empty() && found.stops.is_empty() => {}
        Ok(found) => looked.found = Some(found),
        Err(error) => looked
            .errors
            .push(format!("Could not check workflows: {error}")),
    }
    looked
}

impl Workflows {
    /// Starts the clock and the waker. Called once, when the entity is made.
    pub(super) fn start_workflow_checks(&mut self, cx: &mut Context<Self>) {
        let (wake, woken) = async_channel::bounded::<()>(1);
        self.signals.set_waker(move || {
            let _ = wake.try_send(());
        });
        let asked = cx.spawn(async move |this, cx| {
            while woken.recv().await.is_ok() {
                if this
                    .update(cx, |this, cx| this.check_workflows(cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        let clock = cx.spawn(async move |this, cx| {
            loop {
                if this
                    .update(cx, |this, cx| this.check_workflows(cx))
                    .is_err()
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(CHECK_MS))
                    .await;
            }
        });
        self.checks._tasks = vec![asked, clock];
    }

    /// Looks at the workflows now: requests in the files, due schedules, runs to resume.
    pub(crate) fn check_workflows(&mut self, cx: &mut Context<Self>) {
        // Once the app is quitting nothing new is claimed, started or written.
        if self.runner.is_quitting() {
            return;
        }
        if self.checks.checking {
            self.checks.again = true;
            return;
        }
        self.checks.checking = true;
        self.checks.again = false;
        // Whatever was asked for before this look is answered by it.
        self.signals.take_check_now();
        let driven = self.runner.running();
        let sweep = !self.checks.swept;
        let signals = self.signals.clone();
        let dir = self.dir.clone();
        let probes = self.probes();
        let work = cx.background_executor().spawn(async move {
            look_at(
                &dir,
                &driven,
                sweep,
                &signals,
                probes,
                monocode_core::time::now_ms(),
            )
        });
        cx.spawn(async move |this, cx| {
            let looked = work.await;
            let _ = this.update(cx, |this, cx| this.workflows_looked(looked, cx));
        })
        .detach();
    }

    fn workflows_looked(&mut self, looked: Looked, cx: &mut Context<Self>) {
        self.checks.checking = false;
        self.checks.swept |= looked.swept;
        for error in &looked.errors {
            eprintln!("{error}");
        }
        // The look marked what it claimed as running (so nothing could remove or double-run it
        // while its models were probed); the runs started here keep that mark, the others lose it.
        let claimed: Vec<String> = looked
            .found
            .iter()
            .flat_map(|found| &found.starts)
            .map(|start| match start {
                Start::Run { run, .. } | Start::Resume { run } => run.workflow_id.clone(),
                Start::Refused { workflow_id, .. } => workflow_id.clone(),
            })
            .collect();
        // A look that was going when the app began to quit finds nothing to start.
        if !self.runner.is_quitting()
            && let Some(found) = looked.found
        {
            self.apply_found(found, cx);
        }
        for workflow_id in claimed {
            if !self.runner.is_running(&workflow_id) {
                self.signals.set_running(&workflow_id, false);
            }
        }
        if !self.runner.is_quitting() && std::mem::take(&mut self.checks.again) {
            self.check_workflows(cx);
        }
    }

    fn apply_found(&mut self, found: Found, cx: &mut Context<Self>) {
        for workflow_id in &found.stops {
            self.stop_run(workflow_id);
        }
        for start in found.starts {
            match start {
                Start::Resume { run } => {
                    self.resume_run(&run, cx);
                }
                Start::Refused {
                    workflow_name,
                    failure,
                    ..
                } => {
                    // The run stays interrupted; a notification says why it did not continue.
                    let body = super::summary::notice_body(&failure.title, &failure.text);
                    if let Some(shell) = self.shell.upgrade() {
                        shell.update(cx, |shell, cx| {
                            shell.workflow_notice(None, &workflow_name, &body, cx)
                        });
                    }
                }
                Start::Run { mut run, failure } => {
                    // A run that never starts would otherwise only show in the run history.
                    let failed = failure.is_some();
                    if let Some(failure) = failure {
                        // Delivered here, so the runner must not deliver a second summary for the
                        // run it only writes.
                        run.report_posted_at = Some(run.ended_at.unwrap_or(run.started_at));
                        run.summary = Some(RunSummary {
                            title: failure.title.clone(),
                            text: failure.text.clone(),
                        });
                        self.deliver(
                            Summary {
                                run: run.clone(),
                                title: failure.title,
                                text: failure.text,
                                launches: Vec::new(),
                            },
                            cx,
                        );
                    }
                    // The runner writes every run it takes. When it refuses one that already failed
                    // (and was announced above), the record is written here so it is not lost.
                    let kept = failed.then(|| run.clone());
                    if !self.start_run(run, cx)
                        && let Some(run) = kept
                    {
                        let save = self.workflow_host(cx).save_run(run);
                        cx.spawn(async move |_, _| {
                            if let Err(error) = save.await {
                                eprintln!("Could not save a failed workflow run: {error}");
                            }
                        })
                        .detach();
                    }
                }
            }
        }
    }
}

/// What `quit` and `restart` call before they shut the processes down: the workflow runs let go
/// without ending (the next start interrupts them), nothing new starts, and their commands are
/// killed before the app kills every process. It never updates `Workflows`, so it is safe to call
/// from inside one of its updates.
pub(crate) fn begin_quit(cx: &mut gpui::App) {
    let runner = cx.try_global::<QuitTarget>().map(|t| t.0.clone());
    if let Some(runner) = runner {
        runner.quit();
    }
    monocode_core::mono::workflows::command::cancel_all();
}

/// The runner `begin_quit` reaches (there is one per app).
pub(super) struct QuitTarget(pub(super) WorkflowRunner);
impl gpui::Global for QuitTarget {}

#[cfg(test)]
mod tests;
