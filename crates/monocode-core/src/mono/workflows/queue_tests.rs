//! The queue of a workflow's runs: order, the cap, cancelling, and the skipped record of an event
//! the full queue refused. Real files in temp dirs.
use super::fixtures::workflow_from;
use super::model::*;
use super::queue::*;
use super::store::*;
use serde_json::json;
use std::path::PathBuf;

const ID: &str = "wf-queue001";

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("wf-queue-{}", crate::mono::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn workflow() -> Workflow {
    workflow_from(
        &json!({
            "id": ID, "name": "Queue me", "inputs": {}, "inputValues": {},
            "nodes": [{"id": "start", "type": "manual"}], "edges": [],
            "enabled": true, "createdAt": 1
        })
        .to_string(),
    )
}

fn stored(disk: &MonoDir) {
    update_workflows(disk, |_| Ok((vec![workflow()], ()))).unwrap();
}

fn waiting(id: &str, at: u64) -> WorkflowRun {
    queued_run(
        &workflow(),
        RunCaller::new(CallerKind::Manual, "start"),
        at,
        id,
        at,
    )
}

#[test]
fn a_queued_record_keeps_who_asked_the_values_and_the_number_with_a_stub_definition() {
    let mut w = workflow();
    let mut values = super::ordered::OrderedMap::new();
    values.insert("count".to_string(), InputValue::Number(3.0));
    w.run_values = Some(values.clone());
    let mut caller = RunCaller::new(CallerKind::Event, "start");
    caller.item = Some(super::ordered::Json::parse(r#"{"ref":"CU-1"}"#).unwrap());
    let run = queued_run(&w, caller.clone(), 7, "run-a", 50);
    assert_eq!(run.status, RunStatus::Queued);
    assert_eq!(run.number, Some(7));
    assert_eq!(run.queued_at, Some(50));
    assert_eq!(run.caller, caller);
    assert_eq!(run.definition.run_values, Some(values));
    // The definition is taken when the run starts: the stub holds no steps.
    assert!(run.definition.nodes.is_empty() && run.nodes.is_empty());
}

#[test]
fn the_queue_is_first_in_first_out_and_positions_count_from_one() {
    let runs = vec![waiting("c", 30), waiting("b", 20), waiting("a", 10)];
    let order: Vec<&str> = queued_fifo(&runs).iter().map(|r| r.id.as_str()).collect();
    assert_eq!(order, ["a", "b", "c"]);
    assert_eq!(queue_position(&runs, "a"), Some(1));
    assert_eq!(queue_position(&runs, "c"), Some(3));
    assert_eq!(queue_position(&runs, "nobody"), None);
    assert_eq!(queued_count(&runs), 3);
}

#[test]
fn a_run_that_is_not_waiting_is_not_in_the_queue() {
    let mut done = waiting("done", 5);
    done.status = RunStatus::Done;
    let runs = vec![waiting("a", 10), done];
    assert_eq!(queued_count(&runs), 1);
    assert_eq!(queue_position(&runs, "done"), None);
}

#[test]
fn at_most_fifty_runs_wait_and_the_next_one_is_refused() {
    let root = temp_root();
    let disk = MonoDir::new(&root, "mono-1");
    stored(&disk);
    for n in 0..QUEUED_RUNS_MAX {
        let run = waiting(&format!("run-{n}"), n as u64 + 1);
        assert_eq!(
            enqueue(&disk, run).unwrap(),
            Enqueued::Queued(n + 1),
            "run {n}"
        );
    }
    assert_eq!(
        enqueue(&disk, waiting("one-too-many", 999)).unwrap(),
        Enqueued::Full
    );
    let runs = load_runs(&disk, ID).unwrap();
    assert_eq!(queued_count(&runs), QUEUED_RUNS_MAX);
    assert!(runs.iter().all(|r| r.id != "one-too-many"));
    assert_eq!(queue_len(&disk, ID).unwrap(), QUEUED_RUNS_MAX);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn an_event_over_the_cap_is_recorded_as_a_skipped_run_with_the_reason() {
    let root = temp_root();
    let disk = MonoDir::new(&root, "mono-1");
    let mut caller = RunCaller::new(CallerKind::Event, "start");
    caller.item = Some(super::ordered::Json::parse(r#"{"ref":"CU-9"}"#).unwrap());
    let skipped = record_skipped(
        &disk,
        &workflow(),
        caller.clone(),
        123,
        &queue_full_reason(),
    )
    .unwrap();
    assert_eq!(skipped.status, RunStatus::Skipped);
    assert!(skipped.error.as_deref().unwrap().contains("queue is full"));
    let stored = load_runs(&disk, ID).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].status, RunStatus::Skipped);
    assert_eq!(stored[0].caller, caller);
    // A skipped run never counts as waiting.
    assert_eq!(queue_len(&disk, ID).unwrap(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cancelling_a_queued_run_stops_it_without_starting_and_leaves_the_others() {
    let root = temp_root();
    let disk = MonoDir::new(&root, "mono-1");
    stored(&disk);
    enqueue(&disk, waiting("a", 1)).unwrap();
    enqueue(&disk, waiting("b", 2)).unwrap();
    assert!(cancel_queued(&disk, ID, "a", 100).unwrap());
    let runs = load_runs(&disk, ID).unwrap();
    let a = runs.iter().find(|r| r.id == "a").unwrap();
    assert_eq!(a.status, RunStatus::Stopped);
    assert_eq!(a.ended_at, Some(100));
    assert_eq!(
        runs.iter().find(|r| r.id == "b").unwrap().status,
        RunStatus::Queued
    );
    // Cancelling again, or a run that never queued, changes nothing.
    assert!(!cancel_queued(&disk, ID, "a", 101).unwrap());
    assert!(!cancel_queued(&disk, ID, "nobody", 101).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_run_is_not_queued_for_a_workflow_that_was_removed() {
    let root = temp_root();
    let disk = MonoDir::new(&root, "mono-1");
    // The workflow is not stored: nothing is written, and no runs file is left behind.
    assert_eq!(enqueue(&disk, waiting("late", 1)).unwrap(), Enqueued::Gone);
    assert!(load_runs(&disk, ID).unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_promoted_run_the_app_refused_goes_back_in_the_queue_with_its_values() {
    use super::engine::start_run;
    let root = temp_root();
    let disk = MonoDir::new(&root, "mono-1");
    stored(&disk);
    // Promoted: the queued record was replaced by the run that was about to start.
    let mut asked = workflow();
    let mut values = super::ordered::OrderedMap::new();
    values.insert("count".to_string(), InputValue::Number(3.0));
    asked.run_values = Some(values);
    let mut effective = with_run_values(&asked);
    effective.run_values = None;
    let mut run = start_run(
        &effective,
        RunCaller::new(CallerKind::Manual, "start"),
        500,
        "run-p",
    );
    run.number = Some(4);
    run.queued_at = Some(100);
    // The workflow has only a trigger, which finishes at once: this one is held as going.
    run.status = RunStatus::Running;
    run.ended_at = None;
    update_runs(&disk, ID, |_| Ok((vec![run.clone()], ()))).unwrap();
    assert_eq!(load_runs(&disk, ID).unwrap()[0].status, RunStatus::Running);

    assert!(requeue(&disk, &run).unwrap());
    let back = &load_runs(&disk, ID).unwrap()[0];
    assert_eq!(back.status, RunStatus::Queued);
    assert_eq!(back.number, Some(4));
    assert_eq!(back.queued_at, Some(100));
    assert_eq!(back.caller, run.caller);
    // The values the run had are what the next promotion starts it with.
    assert_eq!(
        back.definition.run_values,
        Some(run.definition.input_values.clone())
    );
    assert_eq!(queue_len(&disk, ID).unwrap(), 1);
    // Asked again, there is no running record left to put back.
    assert!(!requeue(&disk, &run).unwrap());
    // A run that was never queued is left alone.
    let mut fresh = run.clone();
    fresh.queued_at = None;
    assert!(!requeue(&disk, &fresh).unwrap());
    std::fs::remove_dir_all(root).unwrap();
}
