//! One look at the workflow files (the tick of `useMonoWorkflows.ts`): which runs to start,
//! which interrupted runs to resume, which runs to stop. It does the file work and the model
//! catalog probing, so it blocks and belongs off the UI thread; starting what it found is the
//! runner's job (`monocode-gpui`).
//!
//! Order within a look, as in TS: requests left in the file (`resumeRunId`, `stopRequested`), then
//! the workflows whose schedule only has to move on, then each due workflow is claimed (re-checked
//! against the newest file inside the save) and turned into a run. A workflow whose definition or
//! models are unusable gets a run that is already failed, with the message to show the user.
//!
//! A workflow lets `concurrency.max` runs go at once. A claim that finds no free slot (or a
//! queue that is not empty) is written as a `queued` run instead, and after the claims every
//! workflow with free slots starts the oldest queued runs, first in first out. The promotion is
//! committed in a hash-checked save of the runs file that sees the newest state, so a queued run
//! starts exactly once even when two looks race.
use std::collections::{HashMap, HashSet};

use super::engine::{mark_interrupted, start_run};
use super::model::{with_run_values, CallerKind, RunCaller, RunStatus, Workflow, WorkflowRun};
use super::models::{harnesses_used, workflow_issues, ModelCatalog, Probe};
use super::preflight::{union_requires, Preflight};
use super::queue::{
    cancel_queued, enqueue, queue_full_reason, queue_len, queued_fifo, queued_run, skipped_run,
    Enqueued,
};
use super::schedule::{after_claim, due_schedule_trigger, due_workflows};
use super::store::{
    load_runs, load_workflows, take_requests, update_runs, update_workflows, MonoFiles,
};
use super::yaml::input_value_problems;
use crate::provider::Kind;

/// What a look needs to know.
pub struct Tick<'a> {
    pub now: u64,
    /// How many runs of the workflow are going (the runner's answer, claims included).
    pub live_runs: &'a dyn Fn(&str) -> usize,
    /// How a harness is probed for the model catalog (tests must not reach real CLIs).
    pub probe: fn(Kind) -> Probe,
    /// Called with the workflow id and the run id as soon as a run is claimed (or a resume or a
    /// promotion of it is taken), before anyone starts it: the caller counts the run as live, so
    /// the workflow cannot be removed, over-filled or reported "not running" in between.
    pub on_claim: &'a dyn Fn(&str, &str),
    /// Whether the machine has what a workflow declares in `requires`. Called only for a workflow
    /// that declares something, after its definition and models passed (tests must not reach real
    /// CLIs or shells).
    pub preflight: &'a dyn Fn(&Workflow) -> Preflight,
}

/// The message to show when a run could not even start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    pub title: String,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Start {
    /// A new run, claimed from the file. A run that is not `Running` (failed validation) only has
    /// to be written, with `failure` posted.
    Run {
        run: WorkflowRun,
        failure: Option<Failure>,
    },
    /// An interrupted run asked to continue, as stored.
    Resume { run: WorkflowRun },
    /// An interrupted run asked to continue that may not resume yet (a requirement is missing).
    /// The run stays interrupted; `failure` says why.
    Refused {
        workflow_id: String,
        workflow_name: String,
        run_id: String,
        failure: Failure,
    },
}

#[derive(Debug, Default, PartialEq)]
pub struct Found {
    pub starts: Vec<Start>,
    /// Workflows with a run going whose stop was asked for (every live run of it stops).
    pub stops: Vec<String>,
    /// `(workflow id, run id)` of runs asked to stop by id. A queued one was cancelled already.
    pub stop_runs: Vec<(String, String)>,
}

fn warn(what: &str, error: &str) {
    eprintln!("{what}: {error}");
}

/// Claims `workflow_id` from the newest file if it is still due. Returns the workflow as it was
/// before the claim (its `run_requested` says why it ran).
fn claim(
    files: &impl MonoFiles,
    now: u64,
    workflow_id: &str,
    run_id: &str,
    busy: &dyn Fn(&str) -> bool,
) -> Result<Option<Workflow>, String> {
    update_workflows(files, |list| {
        let only: Vec<Workflow> = list
            .iter()
            .filter(|w| w.id == workflow_id)
            .cloned()
            .collect();
        let due = due_workflows(&only, now, busy)
            .run
            .first()
            .map(|w| (*w).clone());
        let list = if due.is_some() {
            list.into_iter()
                .map(|w| {
                    if w.id == workflow_id {
                        after_claim(&w, now, Some(run_id))
                    } else {
                        w
                    }
                })
                .collect()
        } else {
            list
        };
        Ok((list, due))
    })
}

/// The run for a claimed workflow: running, or already failed with the message to post.
/// Who started a claimed run: the caller recorded with the request, else the user (manual) when
/// a run was requested, else the schedule trigger that came due.
fn caller_of(claimed: &Workflow, now: u64) -> RunCaller {
    if claimed.run_requested {
        if let Some(caller) = &claimed.run_caller {
            return caller.clone();
        }
        let node = claimed
            .nodes
            .iter()
            .find(|n| n.parent.is_none() && matches!(n.kind, super::model::NodeKind::Manual))
            .or_else(|| claimed.first_trigger());
        return RunCaller::new(CallerKind::Manual, node.map_or("", |n| n.id.as_str()));
    }
    let node = due_schedule_trigger(claimed, now).or_else(|| claimed.first_trigger());
    RunCaller::new(CallerKind::Schedule, node.map_or("", |n| n.id.as_str()))
}

/// What starts a run: who asked, the number and queue time it already has.
struct Origin {
    caller: RunCaller,
    number: u64,
    queued_at: Option<u64>,
}

fn begin(claimed: &Workflow, origin: Origin, tick: &Tick, run_id: &str) -> Start {
    // The run sees the values asked for this run laid over the stored ones, and keeps only that.
    let effective = with_run_values(claimed);
    let mut run = start_run(&effective, origin.caller, tick.now, run_id);
    run.number = Some(origin.number);
    run.queued_at = origin.queued_at;
    let catalog = ModelCatalog::for_harnesses(&harnesses_used(&effective), tick.probe);
    let mut messages: Vec<String> = workflow_issues(&effective, &catalog)
        .into_iter()
        .map(|issue| issue.message)
        .collect();
    // The definition may have changed since the values were asked for.
    if claimed.run_values.is_some() {
        messages.extend(input_value_problems(
            &effective.inputs,
            &effective.input_values,
        ));
    }
    let error = if !messages.is_empty() {
        format!("The workflow is not valid: {}", messages.join("; "))
    } else if effective.requires.is_empty() {
        return Start::Run { run, failure: None };
    } else {
        let check = (tick.preflight)(&effective);
        if check.all_met() {
            return Start::Run { run, failure: None };
        }
        format!("Missing requirements: {}", check.summary())
    };
    run.status = RunStatus::Failed;
    run.ended_at = Some(tick.now);
    run.error = Some(error.clone());
    // A run that never starts would otherwise only show in the run history.
    let failure = Failure {
        title: format!("{} \u{b7} failed", claimed.name),
        text: format!(
            "## {} \u{b7} failed\n\nThe workflow could not start. {error}",
            claimed.name
        ),
    };
    Start::Run {
        run,
        failure: Some(failure),
    }
}

/// An interrupted run asked to continue: resumed as stored when what it declared and what the
/// workflow declares now are present, otherwise refused (the run stays interrupted). The stored
/// definition is checked rather than the live one so the run keeps its own steps; the live
/// `requires` are added because they may have grown since.
fn resume_or_refuse(run: WorkflowRun, live: Option<&Workflow>, tick: &Tick) -> Start {
    // A worktree the user removed comes back from its branch; without the branch the run stays
    // interrupted.
    if let Some(problem) = super::run_worktree::resume_problem(&run) {
        let name = run.definition.name.clone();
        return Start::Refused {
            workflow_id: run.workflow_id,
            workflow_name: name.clone(),
            run_id: run.id,
            failure: Failure {
                title: format!("{name} \u{b7} not resumed"),
                text: format!(
                    "## {name} \u{b7} not resumed\n\nThe run could not continue and stays interrupted. {problem}"
                ),
            },
        };
    }
    let requires = union_requires(
        &run.definition.requires,
        live.map_or(&[][..], |w| &w.requires),
    );
    if requires.is_empty() {
        return Start::Resume { run };
    }
    let mut checked = run.definition.clone();
    checked.requires = requires;
    let check = (tick.preflight)(&checked);
    if check.all_met() {
        return Start::Resume { run };
    }
    let name = run.definition.name.clone();
    let failure = Failure {
        title: format!("{name} \u{b7} not resumed"),
        text: format!(
            "## {name} \u{b7} not resumed\n\nThe run could not continue yet and stays interrupted. Missing requirements: {}",
            check.summary()
        ),
    };
    Start::Refused {
        workflow_id: run.workflow_id,
        workflow_name: name,
        run_id: run.id,
        failure,
    }
}

/// How many runs of `workflow_id` a look has started so far (claimed, promoted or resumed), on
/// top of what `live_runs` already counts.
type Started = HashMap<String, usize>;

fn live(tick: &Tick, started: &Started, workflow_id: &str) -> usize {
    (tick.live_runs)(workflow_id) + started.get(workflow_id).copied().unwrap_or(0)
}

/// Starts the oldest queued runs of `workflow` while it has free slots. The runs are built (and
/// judged) first, which can take a while; then one hash-checked save of the runs file puts each
/// over the queued record it came from, only when that record is still queued and a slot is
/// still free in the newest file. Whatever the save does not take is not started.
fn promote(
    files: &impl MonoFiles,
    tick: &Tick,
    workflow: &Workflow,
    started: &mut Started,
    found: &mut Found,
) {
    // A workflow waiting for a review keeps its queue: what it would run is not approved. A
    // paused one keeps the runs its triggers asked for; a run the user or a Mono asked for by
    // hand still goes.
    if workflow.needs_review || workflow.last_run_id.is_none() {
        return;
    }
    let max = workflow.concurrency.limit();
    let going = live(tick, started, &workflow.id);
    if going >= max {
        return;
    }
    let runs = match load_runs(files, &workflow.id) {
        Ok(runs) => runs,
        Err(error) => return warn("Could not read the queue of a workflow", &error),
    };
    let candidates: Vec<(String, Start)> = queued_fifo(&runs)
        .into_iter()
        .filter(|queued| {
            workflow.enabled || matches!(queued.caller.kind, CallerKind::Manual | CallerKind::Mono)
        })
        .take(max - going)
        .map(|queued| {
            let mut asked = workflow.clone();
            asked.run_values = queued.definition.run_values.clone();
            let origin = Origin {
                caller: queued.caller.clone(),
                number: queued.number.unwrap_or(0),
                queued_at: queued.queued_at,
            };
            (queued.id.clone(), begin(&asked, origin, tick, &queued.id))
        })
        .collect();
    if candidates.is_empty() {
        return;
    }
    let committed = update_runs(files, &workflow.id, |mut list| {
        let in_file = list
            .iter()
            .filter(|r| r.status == RunStatus::Running)
            .count();
        let mut room = max.saturating_sub(going.max(in_file));
        let mut taken: Vec<Start> = Vec::new();
        for (queued_id, start) in &candidates {
            let Start::Run { run, .. } = start else {
                continue;
            };
            let Some(slot) = list
                .iter()
                .position(|r| r.id == *queued_id && r.status == RunStatus::Queued)
            else {
                continue;
            };
            // A run that failed to start holds no slot.
            let takes_slot = run.status == RunStatus::Running;
            if takes_slot && room == 0 {
                continue;
            }
            list[slot] = run.clone();
            room -= usize::from(takes_slot);
            taken.push(start.clone());
        }
        Ok((list, taken))
    });
    match committed {
        Ok(taken) => {
            for start in taken {
                if let Start::Run { run, .. } = &start {
                    if run.status == RunStatus::Running {
                        (tick.on_claim)(&workflow.id, &run.id);
                        *started.entry(workflow.id.clone()).or_default() += 1;
                    }
                }
                found.starts.push(start);
            }
        }
        Err(error) => warn("Could not start a queued workflow run", &error),
    }
}

/// Looks at the workflows once. A request or workflow that cannot be handled is reported
/// on stderr and skipped, so one bad entry does not hold up the others; an error is returned
/// only when the workflow file itself cannot be read.
pub fn check(files: &impl MonoFiles, tick: &Tick) -> Result<Found, String> {
    let mut found = Found::default();
    let mut started = Started::new();
    let mut workflows = load_workflows(files)?.workflows;
    if workflows
        .iter()
        .any(|w| w.resume_run_id.is_some() || w.stop_requested || !w.stop_run_ids.is_empty())
    {
        match take_requests(files, |id| (tick.live_runs)(id) > 0) {
            Ok(taken) => {
                found.stops = taken.stops;
                for (workflow_id, run_id) in taken.stop_runs {
                    // A run that still waits is cancelled here; one that goes is stopped by the
                    // runner, which is told below.
                    if let Err(error) = cancel_queued(files, &workflow_id, &run_id, tick.now) {
                        warn("Could not cancel a queued workflow run", &error);
                    }
                    found.stop_runs.push((workflow_id, run_id));
                }
                for request in taken.resumes {
                    let live_workflow = workflows.iter().find(|w| w.id == request.workflow_id);
                    // A workflow with every slot taken does not resume over its own runs.
                    let max = live_workflow.map_or(1, |w| w.concurrency.limit());
                    if live(tick, &started, &request.workflow_id) >= max {
                        continue;
                    }
                    match load_runs(files, &request.workflow_id) {
                        Ok(runs) => {
                            if let Some(run) = runs.into_iter().find(|r| r.id == request.run_id) {
                                if run.status == RunStatus::Interrupted {
                                    (tick.on_claim)(&request.workflow_id, &run.id);
                                    *started.entry(request.workflow_id.clone()).or_default() += 1;
                                    found
                                        .starts
                                        .push(resume_or_refuse(run, live_workflow, tick));
                                }
                            }
                        }
                        Err(error) => warn("Could not resume the workflow run", &error),
                    }
                }
            }
            Err(error) => warn("Could not read the workflow requests", &error),
        }
        // What was taken is acted on even if the file cannot be read again.
        match load_workflows(files) {
            Ok(loaded) => workflows = loaded.workflows,
            Err(error) => {
                warn("Could not read the workflows again", &error);
                return Ok(found);
            }
        }
    }
    // A scheduled occurrence is skipped while a run of the workflow already waits for a slot.
    let waiting = |id: &str| queue_len(files, id).unwrap_or(0) > 0;
    let due = due_workflows(&workflows, tick.now, waiting);
    // An invalid schedule has no next time to move to, so it is "advanced" on every look: only
    // write when something would change.
    if due
        .advance
        .iter()
        .any(|w| after_claim(w, tick.now, None) != **w)
    {
        if let Err(error) = update_workflows(files, |list| {
            let stale: HashSet<String> = due_workflows(&list, tick.now, waiting)
                .advance
                .iter()
                .map(|w| w.id.clone())
                .collect();
            let list = list
                .into_iter()
                .map(|w| {
                    if stale.contains(&w.id) {
                        after_claim(&w, tick.now, None)
                    } else {
                        w
                    }
                })
                .collect();
            Ok((list, ()))
        }) {
            warn("Could not move workflow schedules on", &error);
        }
    }
    let to_run: Vec<String> = due.run.iter().map(|w| w.id.clone()).collect();
    for id in to_run {
        let run_id = uuid::Uuid::new_v4().to_string();
        match claim(files, tick.now, &id, &run_id, &waiting) {
            Ok(Some(claimed)) => {
                let origin = Origin {
                    caller: caller_of(&claimed, tick.now),
                    number: claimed.run_counter + 1,
                    queued_at: None,
                };
                let free = live(tick, &started, &id) < claimed.concurrency.limit() && !waiting(&id);
                if free {
                    (tick.on_claim)(&id, &run_id);
                    *started.entry(id.clone()).or_default() += 1;
                    found.starts.push(begin(&claimed, origin, tick, &run_id));
                    continue;
                }
                // No slot, or runs ahead in the queue: this one waits its turn.
                let record = queued_run(
                    &claimed,
                    origin.caller.clone(),
                    origin.number,
                    &run_id,
                    tick.now,
                );
                match enqueue(files, record) {
                    Ok(Enqueued::Queued(_)) => {}
                    // The workflow was removed after the claim: nothing to record.
                    Ok(Enqueued::Gone) => {}
                    Ok(Enqueued::Full) => {
                        let reason = queue_full_reason();
                        let skipped =
                            skipped_run(&claimed, origin.caller, &run_id, tick.now, &reason);
                        if let Err(error) = update_runs(files, &id, |runs| {
                            Ok((std::iter::once(skipped.clone()).chain(runs).collect(), ()))
                        }) {
                            warn(
                                &format!("Workflow {id} could not record a skipped run"),
                                &error,
                            );
                        }
                    }
                    Err(error) => warn(&format!("Workflow {id} could not queue a run"), &error),
                }
            }
            Ok(None) => {}
            Err(error) => warn(&format!("Workflow {id} could not start"), &error),
        }
    }
    // Free slots go to the runs that wait, oldest first.
    match load_workflows(files) {
        Ok(loaded) => {
            for workflow in &loaded.workflows {
                promote(files, tick, workflow, &mut started, &mut found);
            }
        }
        Err(error) => warn("Could not read the workflows for the queue", &error),
    }
    Ok(found)
}

/// Marks the runs a previous session left `running` as interrupted, so they can be resumed.
/// `driven` lists the `(workflow id, run id)` the runner is driving right now, which are left
/// alone. Tries every workflow; the first error is returned after the others were done.
pub fn sweep_interrupted(
    files: &impl MonoFiles,
    driven: &[(String, String)],
) -> Result<(), String> {
    let mut first_error = None;
    let mut note = |error: String| {
        first_error.get_or_insert(error);
    };
    for workflow in load_workflows(files)?.workflows {
        let mine = |run_id: &str| {
            driven
                .iter()
                .any(|(wf, run)| *wf == workflow.id && run == run_id)
        };
        let leftover = match load_runs(files, &workflow.id) {
            Ok(runs) => runs
                .iter()
                .any(|r| r.status == RunStatus::Running && !mine(&r.id)),
            Err(error) => {
                note(error);
                continue;
            }
        };
        if !leftover {
            continue;
        }
        let swept = update_runs(files, &workflow.id, |runs| {
            let runs = runs
                .into_iter()
                .map(|r| {
                    if r.status == RunStatus::Running && !mine(&r.id) {
                        mark_interrupted(&r)
                    } else {
                        r
                    }
                })
                .collect();
            Ok((runs, ()))
        });
        if let Err(error) = swept {
            note(error);
        }
    }
    first_error.map_or(Ok(()), Err)
}
