//! Runs started by an Inbox item. The Inbox wiring (a later step) matches an item to a workflow
//! trigger (`event::matching_trigger`) and calls `start_event_run`; the "Test run with this item"
//! button calls `start_test_run`. Both go through the queue like every other start (see
//! `monocode_core::mono::workflows::event_run`).
//!
//! The seams are called by the Inbox wiring, which is a later step, so they look unused until then.
#![allow(dead_code)]
use super::Workflows;
use crate::runtime::{Database, locked};
use gpui::{Context, Task};
use monocode_core::{
    inbox::Item,
    mono::workflows::{
        event_run::{self, Claims, EventClaim, EventOutcome, Held, TestRun},
        tick::{Start, Tick},
    },
};

/// The claims table of the app database, locked once per call so a start that reads files or
/// probes models never holds the database.
struct DbClaims(Database);

impl Claims for DbClaims {
    fn claim(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
    ) -> Result<bool, String> {
        locked(&self.0)?.claim_workflow_item(workflow_id, trigger_node, event_key)
    }
    fn record_run(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
        run_id: &str,
    ) -> Result<(), String> {
        locked(&self.0)?.set_workflow_claim_run(workflow_id, trigger_node, event_key, run_id)
    }
    fn release(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
    ) -> Result<(), String> {
        locked(&self.0)?.release_workflow_event(workflow_id, trigger_node, event_key)
    }
}

/// What an event start did, once the run is with the runner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum EventStarted {
    /// The run is going (or it failed before it began and was written).
    Started { run_id: String },
    /// No free slot: the run waits in the queue (1 starts next).
    Queued { run_id: String, position: usize },
    /// The queue is full: the item is recorded as a `skipped` run.
    Skipped { run_id: String },
    /// A run for this item exists already.
    Duplicate,
    /// Left alone with no claim: the workflow is paused or needs a review. Look again later.
    Held(Held),
    /// The workflow or the trigger is gone.
    Ignored,
    /// No run was made and the claim was given back: the same item may be tried again.
    Failed(String),
}

impl Workflows {
    /// Starts a run for `item` through the event trigger `trigger_node` of `workflow_id`. Claims
    /// the item first (one run per workflow per item), then starts or queues the run. A paused
    /// workflow, or one that needs a review, takes no claim.
    pub(crate) fn start_event_run(
        &mut self,
        workflow_id: &str,
        trigger_node: &str,
        item: Item,
        cx: &mut Context<Self>,
    ) -> Task<EventStarted> {
        let dir = self.dir.clone();
        let signals = self.signals.clone();
        let probes = self.probes();
        let claims = DbClaims(self.store.clone());
        let (workflow, trigger) = (workflow_id.to_owned(), trigger_node.to_owned());
        let work = cx.background_executor().spawn(async move {
            let live = |id: &str| signals.live_runs(id);
            let tick = Tick {
                now: monocode_core::time::now_ms(),
                live_runs: &live,
                probe: probes.models,
                preflight: &probes.requirements,
                on_claim: &|workflow: &str, run: &str| signals.claim_run(workflow, run),
            };
            let outcome =
                event_run::start_event_run(&dir, &claims, &tick, &workflow, &trigger, &item);
            (outcome, DbClaims(claims.0))
        });
        cx.spawn(async move |this, cx| {
            let (outcome, claims) = work.await;
            this.update(cx, |this, cx| this.event_started(outcome, claims, cx))
                .unwrap_or_else(|_| EventStarted::Failed("The app is closing".into()))
        })
    }

    fn event_started(
        &mut self,
        outcome: EventOutcome,
        claims: DbClaims,
        cx: &mut Context<Self>,
    ) -> EventStarted {
        match outcome {
            EventOutcome::Ignored => EventStarted::Ignored,
            EventOutcome::Held(held) => EventStarted::Held(held),
            EventOutcome::Duplicate => EventStarted::Duplicate,
            EventOutcome::Failed(error) => EventStarted::Failed(error),
            EventOutcome::Queued { run_id, position } => {
                // A queued run starts when a slot frees up: ask for a look now in case it is free.
                self.signals.request_check_now();
                EventStarted::Queued { run_id, position }
            }
            EventOutcome::Skipped { run } => EventStarted::Skipped { run_id: run.id },
            EventOutcome::Started { start, claim } => {
                self.apply_event_start(start, claim, &claims, cx)
            }
        }
    }

    /// Hands a run the core made to the runner, then tells the claim what happened: the run is
    /// recorded on it, or the claim is given back when the runner refused (the app is quitting).
    fn apply_event_start(
        &mut self,
        start: Start,
        claim: EventClaim,
        claims: &DbClaims,
        cx: &mut Context<Self>,
    ) -> EventStarted {
        let Start::Run { run, failure } = start else {
            let _ = claim.release(claims);
            return EventStarted::Failed("The run could not start".into());
        };
        let (workflow_id, run_id) = (run.workflow_id.clone(), run.id.clone());
        let taken = self.start_found_run(run, failure, cx);
        // The core marked the run live when it claimed it; a run the runner did not take is not.
        if !self.runner.is_run_live(&run_id) {
            self.signals.release_run(&workflow_id, &run_id);
        }
        if taken {
            if let Err(error) = claim.record_run(claims, &run_id) {
                eprintln!("Could not record the run of a workflow event claim: {error}");
            }
            EventStarted::Started { run_id }
        } else {
            let _ = claim.release(claims);
            EventStarted::Failed("The workflow could not start the run now".into())
        }
    }

    /// "Test run with this item" (D30): runs the saved workflow once for `item`, started by hand.
    /// No claim is taken, so the item can still fire the trigger later. Allowed while the
    /// workflow is paused; refused while it needs a review. Resolves to the run's id.
    pub(crate) fn start_test_run(
        &mut self,
        workflow_id: &str,
        trigger_node: &str,
        item: Item,
        cx: &mut Context<Self>,
    ) -> Task<Result<EventStarted, String>> {
        let dir = self.dir.clone();
        let signals = self.signals.clone();
        let probes = self.probes();
        let (workflow, trigger) = (workflow_id.to_owned(), trigger_node.to_owned());
        let work = cx.background_executor().spawn(async move {
            let live = |id: &str| signals.live_runs(id);
            let tick = Tick {
                now: monocode_core::time::now_ms(),
                live_runs: &live,
                probe: probes.models,
                preflight: &probes.requirements,
                on_claim: &|workflow: &str, run: &str| signals.claim_run(workflow, run),
            };
            event_run::start_test_run(&dir, &tick, &workflow, &trigger, &item)
        });
        cx.spawn(async move |this, cx| {
            let made = work.await?;
            this.update(cx, |this, cx| match made {
                TestRun::Queued { run_id, position } => {
                    this.signals.request_check_now();
                    Ok(EventStarted::Queued { run_id, position })
                }
                TestRun::Started(Start::Run { run, failure }) => {
                    let (workflow_id, run_id) = (run.workflow_id.clone(), run.id.clone());
                    this.start_found_run(run, failure, cx);
                    if !this.runner.is_run_live(&run_id) {
                        this.signals.release_run(&workflow_id, &run_id);
                    }
                    Ok(EventStarted::Started { run_id })
                }
                TestRun::Started(_) => Err("The run could not start".to_string()),
            })
            .unwrap_or_else(|_| Err("The app is closing".into()))
        })
    }

    /// The startup check: claims that never got a run (the app stopped between the claim and the
    /// run) are given back, so the item can be taken again. Runs once, before any event start.
    pub(super) fn release_unclaimed_events(&self) {
        let released =
            locked(&self.store).and_then(|store| event_run::release_unclaimed(&store, &self.dir));
        match released {
            Ok(claims) if !claims.is_empty() => eprintln!(
                "Released {} workflow event claim(s) that never started a run",
                claims.len()
            ),
            Ok(_) => {}
            Err(error) => eprintln!("Could not check workflow event claims: {error}"),
        }
    }

    /// Wires the removal of a workflow to the claims of the session database, so removing one
    /// forgets which items it saw (see `Signals::set_forget_claims`).
    pub(super) fn forget_claims_on_removal(&self) {
        let store = self.store.clone();
        self.signals.set_forget_claims(move |workflow_id| {
            if let Err(error) =
                locked(&store).and_then(|store| store.forget_workflow_claims(workflow_id))
            {
                eprintln!("Could not forget the event claims of a removed workflow: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests;
