//! The queue of a workflow's runs. A workflow lets `concurrency.max` of its runs go at the same
//! time; a start over that becomes a `queued` run in the runs file and starts, first in first
//! out, when a run ends (see `tick::check`, which promotes inside the hash-checked save so a
//! queued run starts exactly once).
//!
//! A queued record holds who asked (`caller`, with the event item), the values asked for the run
//! (`runValues` of a stub definition) and its number. The definition the run executes is taken
//! when it starts, not when it was queued (D22).
use super::model::{RunCaller, RunStatus, Workflow, WorkflowRun, QUEUED_RUNS_MAX};
use super::store::{update_runs, MonoFiles};

/// The runs of `runs` that wait, the one to start first at the front.
pub fn queued_fifo(runs: &[WorkflowRun]) -> Vec<&WorkflowRun> {
    // The file lists newest first, so the oldest queued run is the last one.
    let mut waiting: Vec<&WorkflowRun> = runs
        .iter()
        .filter(|r| r.status == RunStatus::Queued)
        .collect();
    waiting.reverse();
    // Runs queued at the same moment keep file order; otherwise the earlier queue time goes first.
    waiting.sort_by_key(|r| r.queued_at.unwrap_or(r.started_at));
    waiting
}

/// How many runs wait.
pub fn queued_count(runs: &[WorkflowRun]) -> usize {
    runs.iter()
        .filter(|r| r.status == RunStatus::Queued)
        .count()
}

/// The place of a queued run in the queue, 1 for the next to start. `None` when it is not queued.
pub fn queue_position(runs: &[WorkflowRun], run_id: &str) -> Option<usize> {
    queued_fifo(runs)
        .iter()
        .position(|r| r.id == run_id)
        .map(|at| at + 1)
}

/// The record of a run that waits: the caller, the values and the number, with a definition that
/// is only a stub.
pub fn queued_run(
    claimed: &Workflow,
    caller: RunCaller,
    number: u64,
    run_id: &str,
    now: u64,
) -> WorkflowRun {
    let mut stub = Workflow::new(claimed.id.clone(), claimed.name.clone());
    stub.run_values = claimed.run_values.clone();
    WorkflowRun {
        id: run_id.to_string(),
        workflow_id: claimed.id.clone(),
        definition: stub,
        status: RunStatus::Queued,
        started_at: now,
        ended_at: None,
        caller,
        items: Default::default(),
        nodes: Vec::new(),
        report_posted_at: None,
        error: None,
        cloud_instances: Vec::new(),
        summary: None,
        number: Some(number),
        queued_at: Some(now),
        worktree: None,
        branch_worktrees: Default::default(),
    }
}

/// The record of a start that was refused because the queue is full. `reason` is shown to the
/// user as the run's error.
pub fn skipped_run(
    workflow: &Workflow,
    caller: RunCaller,
    run_id: &str,
    now: u64,
    reason: &str,
) -> WorkflowRun {
    let mut run = queued_run(workflow, caller, 0, run_id, now);
    run.status = RunStatus::Skipped;
    run.number = None;
    run.queued_at = None;
    run.ended_at = Some(now);
    run.error = Some(reason.to_string());
    run
}

/// The text for a start the full queue refused.
pub fn queue_full_reason() -> String {
    format!(
        "The queue is full: {QUEUED_RUNS_MAX} runs of this workflow are already waiting for a free slot"
    )
}

/// What `enqueue` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Enqueued {
    /// Written as a queued run, at this place in the queue (1 starts next).
    Queued(usize),
    /// Not written: the queue already holds the most it may.
    Full,
    /// Not written (or taken back): the workflow was removed while the run was being queued.
    Gone,
}

fn workflow_exists(files: &impl MonoFiles, id: &str) -> Result<bool, String> {
    Ok(super::store::load_workflows(files)?
        .workflows
        .iter()
        .any(|w| w.id == id))
}

/// Puts a run in the queue, unless it is full or its workflow was removed. Hash-checked with the
/// rest of the runs file, and the workflow is looked for again on every attempt, so a removal
/// that lands between a claim and this write leaves no runs file behind.
pub fn enqueue(files: &impl MonoFiles, run: WorkflowRun) -> Result<Enqueued, String> {
    let id = run.workflow_id.clone();
    let result = update_runs(files, &id, |runs| {
        if !workflow_exists(files, &id)? {
            return Ok((runs, Enqueued::Gone));
        }
        if queued_count(&runs) >= QUEUED_RUNS_MAX {
            return Ok((runs, Enqueued::Full));
        }
        let place = queued_count(&runs) + 1;
        let list = std::iter::once(run.clone()).chain(runs).collect();
        Ok((list, Enqueued::Queued(place)))
    })?;
    // A removal that cleared the file to the very text this write started from does not move the
    // hash, so look once more after the write.
    if matches!(result, Enqueued::Queued(_)) && !workflow_exists(files, &id)? {
        super::store::clear_runs(files, &id);
        return Ok(Enqueued::Gone);
    }
    Ok(result)
}

/// Records a start the queue refused (an event over the cap) as a `skipped` run with the reason,
/// so the user can see it was not lost silently. Returns the record. For event triggers.
pub fn record_skipped(
    files: &impl MonoFiles,
    workflow: &Workflow,
    caller: RunCaller,
    now: u64,
    reason: &str,
) -> Result<WorkflowRun, String> {
    let run = skipped_run(
        workflow,
        caller,
        &uuid::Uuid::new_v4().to_string(),
        now,
        reason,
    );
    update_runs(files, &workflow.id, |runs| {
        let list = std::iter::once(run.clone()).chain(runs).collect();
        Ok((list, ()))
    })?;
    Ok(run)
}

/// Cancels a queued run: it becomes `stopped` without ever starting. False when `run_id` is not
/// queued (it started, ended or never existed).
pub fn cancel_queued(
    files: &impl MonoFiles,
    workflow_id: &str,
    run_id: &str,
    now: u64,
) -> Result<bool, String> {
    update_runs(files, workflow_id, |runs| {
        let mut cancelled = false;
        let list = runs
            .into_iter()
            .map(|mut run| {
                if run.id == run_id && run.status == RunStatus::Queued {
                    run.status = RunStatus::Stopped;
                    run.ended_at = Some(now);
                    run.error = Some("Cancelled while it waited in the queue.".into());
                    cancelled = true;
                }
                run
            })
            .collect();
        Ok((list, cancelled))
    })
}

/// Puts a promoted run back in the queue when the app refused to start it (it is quitting, or no
/// slot was free after all), so its record does not say `running` for a run that never ran. The
/// run keeps its place by number and its queue time; its values are kept for the next promotion.
/// False when the record is not a running one that was promoted.
pub fn requeue(files: &impl MonoFiles, run: &WorkflowRun) -> Result<bool, String> {
    if run.status != RunStatus::Running || run.queued_at.is_none() {
        return Ok(false);
    }
    update_runs(files, &run.workflow_id, |runs| {
        let mut back = false;
        let list = runs
            .into_iter()
            .map(|stored| {
                // Only the record this call is about, and only while it has not begun any step.
                if stored.id == run.id
                    && stored.status == RunStatus::Running
                    && stored.worktree.is_none()
                {
                    // Its own values only when it had some; otherwise the stored inputs apply.
                    let claimed = run.definition.clone();
                    let mut queued = queued_run(
                        &claimed,
                        run.caller.clone(),
                        run.number.unwrap_or(0),
                        &run.id,
                        run.queued_at.unwrap_or(run.started_at),
                    );
                    queued.queued_at = run.queued_at;
                    back = true;
                    return queued;
                }
                stored
            })
            .collect();
        Ok((list, back))
    })
}

/// How many runs of the workflow wait in the queue.
pub fn queue_len(files: &impl MonoFiles, workflow_id: &str) -> Result<usize, String> {
    super::store::queued_count_of(files, workflow_id)
}
