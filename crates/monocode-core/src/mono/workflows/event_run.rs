//! Starting a run for an Inbox item. An event run goes through the same queue as every other
//! start: the item is claimed first (one run per workflow per item, D28), then the run is made,
//! started or queued, and the claim is told which run it started.
//!
//! The order matters for a crash or a failure in between:
//! 1. `claim_workflow_item`, BEFORE any run record exists, so two windows or two polls cannot
//!    both start a run for the item.
//! 2. The run is claimed from the workflow file, then started or put in the queue.
//! 3. `set_workflow_claim_run` names the run. If making the run failed the claim is released so
//!    a retry can take it; a claim that never got a run is also released at startup.
//!
//! A paused workflow (`enabled` false) and one that needs a review never claim: the item is left
//! for a later look (D45). A run that waits in the queue for a trigger is held again while the
//! workflow is paused (see `tick::promote`).
use super::event::{event_key, trigger_item};
use super::model::{CallerKind, NodeKind, RunCaller, Workflow, WorkflowRun, QUEUED_RUNS_MAX};
use super::ordered::Json;
use super::queue::{enqueue, queue_full_reason, queue_len, queued_run, record_skipped, Enqueued};
use super::store::{load_runs, load_workflows, update_workflows, MonoFiles};
use super::tick::{begin, Origin, Start, Tick};
use crate::inbox::Item;
use crate::store::{Store, WorkflowClaim};

/// Why an item was left alone without taking a claim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// The workflow is paused: its triggers do not start runs.
    Paused,
    /// The workflow waits for the user to approve its definition.
    NeedsReview,
}

/// Where claims are kept. The app's database implements it by locking per call, so the slow
/// part of a start (reading files, probing models) never holds the database.
pub trait Claims {
    /// True only when this call took the claim (one per workflow per item).
    fn claim(&self, workflow_id: &str, trigger_node: &str, event_key: &str)
        -> Result<bool, String>;
    fn record_run(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
        run_id: &str,
    ) -> Result<(), String>;
    fn release(&self, workflow_id: &str, trigger_node: &str, event_key: &str)
        -> Result<(), String>;
}

impl Claims for Store {
    fn claim(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
    ) -> Result<bool, String> {
        self.claim_workflow_item(workflow_id, trigger_node, event_key)
    }
    fn record_run(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
        run_id: &str,
    ) -> Result<(), String> {
        self.set_workflow_claim_run(workflow_id, trigger_node, event_key, run_id)
    }
    fn release(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
    ) -> Result<(), String> {
        self.release_workflow_event(workflow_id, trigger_node, event_key)
    }
}

/// The claim an event run holds until the run is known to exist.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventClaim {
    pub workflow_id: String,
    pub trigger_node: String,
    pub event_key: String,
}

impl EventClaim {
    /// Names the run the claim started. Call it once the run record is written or the runner
    /// took the run.
    pub fn record_run(&self, claims: &dyn Claims, run_id: &str) -> Result<(), String> {
        claims.record_run(
            &self.workflow_id,
            &self.trigger_node,
            &self.event_key,
            run_id,
        )
    }

    /// Gives the claim back because the run was not made or was refused, so a retry can take it.
    pub fn release(&self, claims: &dyn Claims) -> Result<(), String> {
        claims.release(&self.workflow_id, &self.trigger_node, &self.event_key)
    }
}

/// What starting a run for an item did.
#[derive(Clone, Debug, PartialEq)]
pub enum EventOutcome {
    /// Nothing to do: the workflow or the trigger is gone, or the trigger is not an event.
    Ignored,
    /// Left alone, no claim taken: look again later.
    Held(Held),
    /// A run for this item exists already (or is being made by another look).
    Duplicate,
    /// A run to start. The caller starts it (as for `tick::Found::starts`), then calls
    /// `claim.record_run` when the runner took it or `claim.release` when it refused it.
    Started { start: Start, claim: EventClaim },
    /// The workflow has no free slot: the run waits in the queue (1 starts next). The claim
    /// already names it.
    Queued { run_id: String, position: usize },
    /// The queue is full: the item is recorded as a `skipped` run with the reason.
    Skipped { run: WorkflowRun },
    /// The run could not be made. The claim was released: the caller may retry.
    Failed(String),
}

/// What making a run did.
// `Start` holds a whole run; this lives for one call, so boxing it would only add noise.
#[allow(clippy::large_enum_variant)]
enum Created {
    Run(Start),
    Queued(usize),
    Full,
    /// The workflow was removed, paused or changed so it may not start this run any more.
    Declined,
}

fn caller_for(kind: CallerKind, trigger_node: &str, item: &Item) -> RunCaller {
    let mut caller = RunCaller::new(kind, trigger_node);
    caller.item = Some(Json::from(&trigger_item(item)));
    caller
}

fn event_trigger_exists(w: &Workflow, trigger_node: &str) -> bool {
    w.nodes
        .iter()
        .any(|n| n.id == trigger_node && n.parent.is_none() && matches!(n.kind, NodeKind::Event(_)))
}

/// Claims the workflow for one run (the next number, `last_run_id`) and starts it or queues it.
/// `needs_enabled` is false for a test run, which is allowed while the workflow is paused.
fn create_run(
    files: &impl MonoFiles,
    tick: &Tick,
    workflow_id: &str,
    caller: &RunCaller,
    needs_enabled: bool,
) -> Result<(Created, String), String> {
    let run_id = uuid::Uuid::new_v4().to_string();
    let workflows = load_workflows(files)?.workflows;
    let Some(current) = workflows.iter().find(|w| w.id == workflow_id) else {
        return Ok((Created::Declined, run_id));
    };
    let waiting = queue_len(files, workflow_id).unwrap_or(0);
    let busy = (tick.live_runs)(workflow_id) >= current.concurrency.limit() || waiting > 0;
    // A full queue takes no run and uses no number.
    if busy && waiting >= QUEUED_RUNS_MAX {
        return Ok((Created::Full, run_id));
    }
    let claimed = update_workflows(files, |list| {
        let allowed = |w: &Workflow| !w.needs_review && (w.enabled || !needs_enabled);
        if !list.iter().any(|w| w.id == workflow_id && allowed(w)) {
            return Ok((list, None));
        }
        let mut before = None;
        let list = list
            .into_iter()
            .map(|mut w| {
                if w.id == workflow_id {
                    before = Some(w.clone());
                    w.last_run_id = Some(run_id.clone());
                    w.run_counter += 1;
                }
                w
            })
            .collect();
        Ok((list, before))
    })?;
    let Some(claimed) = claimed else {
        return Ok((Created::Declined, run_id));
    };
    let origin = Origin {
        caller: caller.clone(),
        number: claimed.run_counter + 1,
        queued_at: None,
    };
    let free = (tick.live_runs)(workflow_id) < claimed.concurrency.limit()
        && queue_len(files, workflow_id).unwrap_or(0) == 0;
    if free {
        (tick.on_claim)(workflow_id, &run_id);
        return Ok((Created::Run(begin(&claimed, origin, tick, &run_id)), run_id));
    }
    let record = queued_run(&claimed, origin.caller, origin.number, &run_id, tick.now);
    let created = match enqueue(files, record)? {
        Enqueued::Queued(position) => Created::Queued(position),
        Enqueued::Full => Created::Full,
        Enqueued::Gone => Created::Declined,
    };
    Ok((created, run_id))
}

fn warn(what: &str, error: &str) {
    eprintln!("{what}: {error}");
}

/// Starts a run of `workflow_id` for `item`, fired by the event trigger `trigger_node`. The
/// caller has matched the item to the trigger (`event::matching_trigger`).
pub fn start_event_run(
    files: &impl MonoFiles,
    claims: &dyn Claims,
    tick: &Tick,
    workflow_id: &str,
    trigger_node: &str,
    item: &Item,
) -> EventOutcome {
    let workflow = match load_workflows(files) {
        Ok(loaded) => loaded.workflows.into_iter().find(|w| w.id == workflow_id),
        Err(error) => return EventOutcome::Failed(error),
    };
    let Some(workflow) = workflow else {
        return EventOutcome::Ignored;
    };
    if !event_trigger_exists(&workflow, trigger_node) {
        return EventOutcome::Ignored;
    }
    // Nothing is claimed for a workflow that may not start a run: the item stays unclaimed.
    if workflow.needs_review {
        return EventOutcome::Held(Held::NeedsReview);
    }
    if !workflow.enabled {
        return EventOutcome::Held(Held::Paused);
    }
    let claim = EventClaim {
        workflow_id: workflow_id.to_string(),
        trigger_node: trigger_node.to_string(),
        event_key: event_key(item),
    };
    match claims.claim(workflow_id, trigger_node, &claim.event_key) {
        Ok(true) => {}
        Ok(false) => return EventOutcome::Duplicate,
        Err(error) => return EventOutcome::Failed(error),
    }
    let caller = caller_for(CallerKind::Event, trigger_node, item);
    let fail = |error: String| {
        if let Err(release) = claim.release(claims) {
            warn("Could not release a workflow event claim", &release);
        }
        EventOutcome::Failed(error)
    };
    let (created, run_id) = match create_run(files, tick, workflow_id, &caller, true) {
        Ok(made) => made,
        Err(error) => return fail(error),
    };
    let named = |run_id: &str| {
        if let Err(error) = claim.record_run(claims, run_id) {
            warn("Could not record the run of a workflow event claim", &error);
        }
    };
    match created {
        Created::Run(start) => EventOutcome::Started {
            start,
            claim: claim.clone(),
        },
        Created::Queued(position) => {
            named(&run_id);
            EventOutcome::Queued { run_id, position }
        }
        Created::Full => {
            match record_skipped(files, &workflow, caller, tick.now, &queue_full_reason()) {
                Ok(run) => {
                    named(&run.id);
                    EventOutcome::Skipped { run }
                }
                Err(error) => fail(error),
            }
        }
        // Paused, reviewed or removed between the look and the claim: leave the item for later.
        Created::Declined => {
            let _ = claim.release(claims);
            match load_workflows(files)
                .ok()
                .and_then(|l| l.workflows.into_iter().find(|w| w.id == workflow_id))
            {
                None => EventOutcome::Ignored,
                Some(w) if w.needs_review => EventOutcome::Held(Held::NeedsReview),
                Some(_) => EventOutcome::Held(Held::Paused),
            }
        }
    }
}

/// What a test run did.
#[derive(Clone, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // `Start` is as large in `tick::Found::starts`.
pub enum TestRun {
    /// A run to start, as for `tick::Found::starts`.
    Started(Start),
    /// No free slot: it waits in the queue (1 starts next).
    Queued { run_id: String, position: usize },
}

/// "Test run with this item" (D30): the saved definition runs once for `item`, started by hand
/// (a manual caller that carries the item). It takes no claim, so the item can still fire the
/// trigger later, and it is allowed while the workflow is paused. A workflow that needs a review
/// is refused.
pub fn start_test_run(
    files: &impl MonoFiles,
    tick: &Tick,
    workflow_id: &str,
    trigger_node: &str,
    item: &Item,
) -> Result<TestRun, String> {
    let workflow = load_workflows(files)?
        .workflows
        .into_iter()
        .find(|w| w.id == workflow_id)
        .ok_or("No workflow with that id")?;
    if !event_trigger_exists(&workflow, trigger_node) {
        return Err("That trigger is not an event trigger of the workflow".into());
    }
    if workflow.needs_review {
        return Err("Approve the workflow before running it".into());
    }
    let caller = caller_for(CallerKind::Manual, trigger_node, item);
    match create_run(files, tick, workflow_id, &caller, false)? {
        (Created::Run(start), _) => Ok(TestRun::Started(start)),
        (Created::Queued(position), run_id) => Ok(TestRun::Queued { run_id, position }),
        (Created::Full, _) => Err(queue_full_reason()),
        (Created::Declined, _) => Err("The workflow changed; try again".into()),
    }
}

/// The startup check: a claim that never got a run id (the app stopped after the claim) is
/// repaired. When the workflow's runs file holds a run of that trigger for the same item (the
/// app stopped between making the run and recording it) the claim records that run; only a claim
/// with no run is released so the item can be taken again. Returns the claims it released.
pub fn release_unclaimed(
    store: &Store,
    files: &impl MonoFiles,
) -> Result<Vec<WorkflowClaim>, String> {
    let mut released = Vec::new();
    for claim in store.unclaimed_workflow_claims()? {
        let found = load_runs(files, &claim.workflow_id)
            .unwrap_or_default()
            .into_iter()
            .find(|run| {
                run.caller.kind == CallerKind::Event
                    && run.caller.trigger_node == claim.trigger_node
                    && run
                        .caller
                        .item
                        .as_ref()
                        .and_then(|item| item.get("eventKey"))
                        .and_then(|key| key.as_str())
                        == Some(claim.event_key.as_str())
            });
        match found {
            Some(run) => store.set_workflow_claim_run(
                &claim.workflow_id,
                &claim.trigger_node,
                &claim.event_key,
                &run.id,
            )?,
            None => {
                store.release_workflow_event(
                    &claim.workflow_id,
                    &claim.trigger_node,
                    &claim.event_key,
                )?;
                released.push(claim);
            }
        }
    }
    Ok(released)
}
