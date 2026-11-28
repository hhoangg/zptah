//! The scheduling cases of `useMonoWorkflows.test.ts`: what one look at a Mono's workflow files
//! finds to start, resume and stop. Real files in temp dirs; the host side (running what was
//! found) is tested in `monocode-gpui`.
use super::engine::{mark_interrupted, start_run};
use super::model::*;
use super::models::Probe;
use super::ordered::OrderedMap;
use super::preflight::{Preflight, RequirementStatus};
use super::store::*;
use super::tick::*;
use super::validate::validate_workflow;
use crate::catalog::Model;
use crate::mono::files::AgentFile;
use crate::provider::Kind;
use chrono::{Local, TimeZone};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

const ID: &str = "wf-abcd1234";
const OTHER: &str = "wf-other000";

fn now() -> u64 {
    Local
        .with_ymd_and_hms(2026, 10, 22, 12, 0, 0)
        .single()
        .unwrap()
        .timestamp_millis() as u64
}

fn temp_root() -> PathBuf {
    let root = std::env::temp_dir().join(format!("wf-tick-{}", crate::mono::new_id()));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn dir(root: &std::path::Path) -> MonoDir {
    MonoDir::new(root, "mono-1")
}

const DAILY: &str = r#"{"id":"start","type":"schedule","schedule":{"scheduleKind":"daily","time":"09:00","minute":0,"dayOfWeek":1}}"#;

/// A valid scheduled workflow that is due (its time was a second ago). Its one step is a session
/// the runner has to start, so a started run is still going.
fn workflow_with(id: &str, extra: Value) -> Workflow {
    let mut v: Value = serde_json::from_str(&format!(
        r#"{{"id":"{id}","name":"Ship it","inputs":{{"repo":{{"type":"project"}}}},
            "inputValues":{{"repo":"/code/app"}},
            "nodes":[{DAILY},{{"id":"step","type":"session","project":"{{{{inputs.repo}}}}","prompt":"go","checkout":"root","output":{{}}}}],
            "edges":[{{"from":"start","to":"step"}}],
            "enabled":true,"createdAt":1,"nextRunAt":{}}}"#,
        now() - 1_000
    ))
    .unwrap();
    for (k, x) in extra.as_object().unwrap() {
        if x.is_null() {
            v.as_object_mut().unwrap().remove(k);
        } else {
            v[k] = x.clone();
        }
    }
    super::fixtures::workflow_from(&v.to_string())
}

fn workflow(extra: Value) -> Workflow {
    workflow_with(ID, extra)
}

/// A valid workflow whose `dev` session names `extra` (harness / model fields).
fn with_session(extra: &str) -> Workflow {
    let mut w = workflow(
        json!({"inputs": {"repo": {"type": "project"}}, "inputValues": {"repo": "/code/app"}}),
    );
    w.nodes.insert(
        1,
        super::fixtures::node(&format!(
            r#"{{"id":"dev","type":"session","project":"{{{{inputs.repo}}}}","prompt":"go","checkout":"root",{extra}}}"#
        )),
    );
    w.edges = vec![super::fixtures::edge(r#"{"from":"start","to":"dev"}"#)];
    assert_eq!(validate_workflow(&w), vec![]);
    w
}

fn save(dir: &MonoDir, workflows: &[Workflow]) {
    let list = workflows.to_vec();
    update_workflows(dir, |_| Ok((list.clone(), ()))).unwrap();
}

fn stored(dir: &MonoDir) -> Vec<Workflow> {
    load_workflows(dir).unwrap().workflows
}

fn stored_one(dir: &MonoDir) -> Workflow {
    stored(dir).remove(0)
}

fn probe_nothing(_: Kind) -> Probe {
    Probe {
        installed: true,
        models: None,
    }
}

fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        name: id.into(),
        alias_target: None,
        efforts: vec![],
        default_effort: None,
        fast: false,
        thinking: false,
        default: false,
        tiers: vec![],
        controls: vec![],
    }
}

fn probe_claude(_: Kind) -> Probe {
    Probe {
        installed: true,
        models: Some(vec![model("claude-sonnet-5")]),
    }
}

fn idle(_: &str) -> bool {
    false
}

fn look(files: &impl MonoFiles, running: &dyn Fn(&str) -> bool) -> Found {
    look_with(files, running, probe_nothing)
}

fn look_with(
    files: &impl MonoFiles,
    running: &dyn Fn(&str) -> bool,
    probe: fn(Kind) -> Probe,
) -> Found {
    check(
        files,
        &Tick {
            now: now(),
            live_runs: &|id| usize::from(running(id)),
            probe,
            on_claim: &|_, _| {},
            preflight: &|_| Preflight::default(),
        },
    )
    .unwrap()
}

fn look_pf(
    files: &impl MonoFiles,
    running: &dyn Fn(&str) -> bool,
    preflight: &dyn Fn(&Workflow) -> Preflight,
) -> Found {
    check(
        files,
        &Tick {
            now: now(),
            live_runs: &|id| usize::from(running(id)),
            probe: probe_nothing,
            on_claim: &|_, _| {},
            preflight,
        },
    )
    .unwrap()
}

/// Every requirement missing, with a reason.
fn all_missing(w: &Workflow) -> Preflight {
    Preflight {
        statuses: w
            .requires
            .iter()
            .map(|r| RequirementStatus {
                requirement: r.clone(),
                met: false,
                detail: Some("Not found on PATH".into()),
            })
            .collect(),
    }
}

fn all_found(w: &Workflow) -> Preflight {
    Preflight {
        statuses: w
            .requires
            .iter()
            .map(|r| RequirementStatus {
                requirement: r.clone(),
                met: true,
                detail: None,
            })
            .collect(),
    }
}

fn runs_of(found: &Found) -> Vec<(&WorkflowRun, Option<&Failure>)> {
    found
        .starts
        .iter()
        .filter_map(|s| match s {
            Start::Run { run, failure } => Some((run, failure.as_ref())),
            _ => None,
        })
        .collect()
}

fn resumes_of(found: &Found) -> Vec<&WorkflowRun> {
    found
        .starts
        .iter()
        .filter_map(|s| match s {
            Start::Resume { run } => Some(run),
            _ => None,
        })
        .collect()
}

/// Counts what is saved, to see when a look must leave the file alone.
struct Counting {
    disk: MonoDir,
    saves: Cell<usize>,
}

impl Counting {
    fn new(disk: MonoDir) -> Self {
        Self {
            disk,
            saves: Cell::new(0),
        }
    }
}

impl MonoFiles for Counting {
    fn read(&self, relative: &str) -> Result<AgentFile, String> {
        self.disk.read(relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        self.saves.set(self.saves.get() + 1);
        self.disk.save(relative, text, expected)
    }
}

// --- due and requested runs ---------------------------------------------------------------

#[test]
fn a_due_scheduled_workflow_is_claimed_and_started() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({}))]);
    let found = look(&disk, &idle);
    let runs = runs_of(&found);
    assert_eq!(runs.len(), 1);
    let (run, failure) = runs[0];
    assert!(failure.is_none());
    assert_eq!(run.workflow_id, ID);
    assert_eq!(run.caller, RunCaller::new(CallerKind::Schedule, "start"));
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(run.started_at, now());
    let after = stored_one(&disk);
    assert!(after.next_run_at.unwrap() > now());
    assert!(!after.run_requested);
    assert_eq!(after.last_run_id.as_deref(), Some(run.id.as_str()));
    std::fs::remove_dir_all(root).unwrap();
}

/// Two schedule triggers and a manual one, all leading to one session.
fn three_triggers(extra: Value) -> Workflow {
    let mut spec = json!({
        "inputs": {"repo": {"type": "project"}}, "inputValues": {"repo": "/code/app"},
        "nodes": [
            {"id": "start", "type": "schedule",
             "schedule": {"scheduleKind": "daily", "time": "09:00", "minute": 0, "dayOfWeek": 1}},
            {"id": "evening", "type": "schedule",
             "schedule": {"scheduleKind": "daily", "time": "18:00", "minute": 0, "dayOfWeek": 1}},
            {"id": "hand", "type": "manual"},
            {"id": "dev", "type": "session", "project": "{{inputs.repo}}", "prompt": "go",
             "checkout": "root", "output": {}}
        ],
        "edges": [
            {"from": "start", "to": "dev"},
            {"from": "evening", "to": "dev"},
            {"from": "hand", "to": "dev"}
        ]
    });
    for (k, x) in extra.as_object().unwrap() {
        spec[k] = x.clone();
    }
    workflow(spec)
}

#[test]
fn a_run_records_the_schedule_trigger_that_came_due() {
    let root = temp_root();
    let disk = dir(&root);
    let later = now() + 3_600_000;
    save(
        &disk,
        &[three_triggers(json!({
            "nextRunAt": now() - 1_000,
            "nextRunAtByNode": {"start": later, "evening": now() - 1_000}
        }))],
    );
    let found = look(&disk, &idle);
    let runs = runs_of(&found);
    assert_eq!(runs.len(), 1);
    let (run, failure) = runs[0];
    assert!(failure.is_none(), "{failure:?}");
    assert_eq!(run.caller, RunCaller::new(CallerKind::Schedule, "evening"));
    // Only the one that came due ran; the other schedule and the manual trigger were skipped.
    let status = |id: &str| run.nodes.iter().find(|n| n.node_id == id).unwrap().status;
    assert_eq!(status("evening"), NodeStatus::Done);
    assert_eq!(status("start"), NodeStatus::Skipped);
    assert_eq!(status("hand"), NodeStatus::Skipped);
    // Claiming moved the one that ran on and left the other's time alone.
    let after = stored_one(&disk);
    assert!(after.next_run_at_by_node["evening"] > now());
    assert_eq!(after.next_run_at_by_node["start"], later);
    assert_eq!(
        after.next_run_at,
        after.next_run_at_by_node.values().min().copied()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_requested_run_is_manual_through_the_manual_trigger_unless_a_caller_was_recorded() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[three_triggers(json!({"runRequested": true}))]);
    let found = look(&disk, &idle);
    assert_eq!(
        runs_of(&found)[0].0.caller,
        RunCaller::new(CallerKind::Manual, "hand")
    );

    // A Mono that asked for the run is recorded, and the record is cleared once claimed.
    let mut asked = three_triggers(json!({"runRequested": true}));
    asked.run_caller = Some(RunCaller::mono("mono-7", "hand"));
    save(&disk, &[asked]);
    let found = look(&disk, &idle);
    assert_eq!(
        runs_of(&found)[0].0.caller,
        RunCaller::mono("mono-7", "hand")
    );
    let after = stored_one(&disk);
    assert!(!after.run_requested);
    assert_eq!(after.run_caller, None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_requested_run_starts_even_when_the_schedule_is_not_due() {
    let root = temp_root();
    let disk = dir(&root);
    let later = now() + 3_600_000;
    save(
        &disk,
        &[workflow(json!({"runRequested": true, "nextRunAt": later}))],
    );
    let found = look(&disk, &idle);
    let runs = runs_of(&found);
    assert_eq!(
        runs[0].0.caller,
        RunCaller::new(CallerKind::Manual, "start")
    );
    let after = stored_one(&disk);
    assert!(!after.run_requested);
    // A requested run leaves the schedule where it was.
    assert_eq!(after.next_run_at, Some(later));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_manual_workflow_runs_on_request_and_stays_paused() {
    let root = temp_root();
    let disk = dir(&root);
    let manual = workflow(json!({"runRequested": true, "enabled": false, "nextRunAt": null}));
    let mut manual = manual;
    manual.nodes[0] = super::fixtures::node(r#"{"id":"start","type":"manual"}"#);
    save(&disk, &[manual]);
    let found = look(&disk, &idle);
    assert_eq!(
        runs_of(&found)[0].0.caller,
        RunCaller::new(CallerKind::Manual, "start")
    );
    let after = stored_one(&disk);
    assert!(!after.enabled && !after.run_requested);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_workflow_waiting_for_review_does_not_start() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(json!({"runRequested": true, "needsReview": true}))],
    );
    let found = look(&disk, &idle);
    assert!(found.starts.is_empty());
    assert!(stored_one(&disk).run_requested);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_tick_while_every_slot_is_taken_moves_the_schedule_and_queues_instead_of_starting() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({"concurrency": {"max": 1}}))]);
    let found = look(&disk, &|id| id == ID);
    assert!(found.starts.is_empty());
    let after = stored_one(&disk);
    assert!(after.next_run_at.unwrap() > now());
    // The occurrence waits in the queue instead of being dropped.
    assert!(after.last_run_id.is_some());
    assert_eq!(load_runs(&disk, ID).unwrap()[0].status, RunStatus::Queued);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_requested_run_waits_in_the_queue_for_the_current_one_instead_of_being_dropped() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"runRequested": true, "concurrency": {"max": 1}}),
        )],
    );
    let found = look(&disk, &|id| id == ID);
    assert!(found.starts.is_empty());
    assert!(!stored_one(&disk).run_requested);
    assert_eq!(load_runs(&disk, ID).unwrap()[0].status, RunStatus::Queued);
    // Once the current run ends the queued one starts, as the same run.
    let queued_id = load_runs(&disk, ID).unwrap()[0].id.clone();
    let found = look(&disk, &idle);
    assert_eq!(runs_of(&found).len(), 1);
    assert_eq!(runs_of(&found)[0].0.id, queued_id);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_one_run_is_started_per_workflow_and_other_workflows_go_side_by_side() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(json!({})), workflow_with(OTHER, json!({}))],
    );
    let found = look(&disk, &idle);
    let mut ids: Vec<_> = runs_of(&found)
        .iter()
        .map(|(run, _)| run.workflow_id.clone())
        .collect();
    ids.sort();
    assert_eq!(ids, vec![ID.to_string(), OTHER.to_string()]);
    // The next look, with both running, claims neither again.
    let found = look(&disk, &|_| true);
    assert!(found.starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_claim_is_checked_again_against_the_latest_file() {
    // The workflow is paused between the look and the claim: nothing is claimed or started.
    struct Pauses {
        disk: MonoDir,
        reads: Cell<usize>,
    }
    impl MonoFiles for Pauses {
        fn read(&self, relative: &str) -> Result<AgentFile, String> {
            if relative == "workflows.json" {
                self.reads.set(self.reads.get() + 1);
                // The second read of the file is the claim's: the user pauses it just before.
                if self.reads.get() == 2 {
                    let mut paused = stored_one(&self.disk);
                    paused.enabled = false;
                    save(&self.disk, &[paused]);
                }
            }
            self.disk.read(relative)
        }
        fn save(&self, r: &str, t: &str, e: Option<&str>) -> Result<String, String> {
            self.disk.save(r, t, e)
        }
    }
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({}))]);
    let pausing = Pauses {
        disk: disk.clone(),
        reads: Cell::new(0),
    };
    let found = look(&pausing, &idle);
    assert!(found.starts.is_empty(), "{:?}", runs_of(&found).len());
    assert_eq!(stored_one(&disk).last_run_id, None);
    std::fs::remove_dir_all(root).unwrap();
}

// --- claimed but not started yet -------------------------------------------------------------

#[test]
fn a_workflow_is_reported_claimed_before_its_models_are_probed() {
    static CLAIMED_WHEN_PROBED: AtomicUsize = AtomicUsize::new(0);
    static CLAIMS: AtomicUsize = AtomicUsize::new(0);
    fn probing(kind: Kind) -> Probe {
        CLAIMED_WHEN_PROBED.store(CLAIMS.load(Ordering::SeqCst), Ordering::SeqCst);
        probe_nothing(kind)
    }
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[
            with_session(r#""harness":"claude""#),
            // Not claimed: waits for review.
            workflow_with(OTHER, json!({"runRequested": true, "needsReview": true})),
        ],
    );
    let claimed = RefCell::new(Vec::new());
    let on_claim = |id: &str, _run: &str| {
        CLAIMS.fetch_add(1, Ordering::SeqCst);
        claimed.borrow_mut().push(id.to_string());
    };
    let found = check(
        &disk,
        &Tick {
            now: now(),
            live_runs: &|_| 0,
            probe: probing,
            on_claim: &on_claim,
            preflight: &|_| Preflight::default(),
        },
    )
    .unwrap();
    assert_eq!(runs_of(&found).len(), 1);
    assert_eq!(*claimed.borrow(), vec![ID.to_string()]);
    // The probe (slow on a cold cache) ran after the claim was announced.
    assert_eq!(CLAIMED_WHEN_PROBED.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(root).unwrap();
}

// --- a workflow that cannot start -----------------------------------------------------------

#[test]
fn an_invalid_workflow_gets_a_failed_run_and_a_message_for_the_chat() {
    let root = temp_root();
    let disk = dir(&root);
    let mut broken = workflow(json!({}));
    broken.edges = vec![super::fixtures::edge(r#"{"from":"start","to":"ghost"}"#)];
    assert!(!validate_workflow(&broken).is_empty());
    save(&disk, &[broken]);
    let found = look(&disk, &idle);
    let (run, failure) = runs_of(&found)[0];
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.ended_at, Some(now()));
    assert!(
        run.error
            .as_deref()
            .unwrap()
            .starts_with("The workflow is not valid: "),
        "{:?}",
        run.error
    );
    assert!(run.error.as_deref().unwrap().contains("ghost"));
    let failure = failure.unwrap();
    assert_eq!(failure.title, "Ship it · failed");
    assert!(failure
        .text
        .starts_with("## Ship it · failed\n\nThe workflow could not start. "));
    assert!(failure.text.contains(run.error.as_deref().unwrap()));
    // It was claimed all the same: the schedule moved on and does not fail again every look.
    assert!(stored_one(&disk).next_run_at.unwrap() > now());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_session_naming_a_model_the_catalog_lacks_fails_the_run_naming_it() {
    let root = temp_root();
    let disk = dir(&root);
    let w = with_session(r#""model":"claude:sonnet-6""#);
    save(&disk, &[w]);
    let found = look_with(&disk, &idle, probe_claude);
    let (run, failure) = runs_of(&found)[0];
    assert_eq!(run.status, RunStatus::Failed);
    assert!(
        run.error.as_deref().unwrap().contains(
            "Model \"claude:sonnet-6\" is not available. Pick one in the step's model list."
        ),
        "{:?}",
        run.error
    );
    assert!(failure.unwrap().text.contains("Model \"claude:sonnet-6\""));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_catalog_is_only_probed_for_the_harnesses_the_workflow_uses() {
    static ASKED: AtomicUsize = AtomicUsize::new(0);
    fn counting(kind: Kind) -> Probe {
        ASKED.fetch_add(1, Ordering::SeqCst);
        assert_eq!(kind, Kind::Claude);
        probe_nothing(kind)
    }
    let root = temp_root();
    let disk = dir(&root);
    // No session node: nothing to probe.
    save(&disk, &[workflow(json!({}))]);
    look_with(&disk, &idle, counting);
    assert_eq!(ASKED.load(Ordering::SeqCst), 0);
    let w = with_session(r#""harness":"claude""#);
    save(&disk, &[w]);
    look_with(&disk, &idle, counting);
    assert_eq!(ASKED.load(Ordering::SeqCst), 1);
    std::fs::remove_dir_all(root).unwrap();
}

// --- requests in the file -------------------------------------------------------------------

fn interrupted(id: &str) -> WorkflowRun {
    mark_interrupted(&start_run(
        &workflow(json!({})),
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        id,
    ))
}

fn put_runs(dir: &MonoDir, workflow_id: &str, runs: Vec<WorkflowRun>) {
    update_runs(dir, workflow_id, |_| Ok((runs.clone(), ()))).unwrap();
}

#[test]
fn a_resume_asked_for_in_the_file_resumes_an_interrupted_run() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old"}),
        )],
    );
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    let found = look(&disk, &idle);
    let resumes = resumes_of(&found);
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0].id, "run-old");
    assert_eq!(resumes[0].status, RunStatus::Interrupted);
    // The request is answered, so it is not repeated.
    assert_eq!(stored_one(&disk).resume_run_id, None);
    assert!(look(&disk, &idle).starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_for_a_run_that_is_not_interrupted_is_answered_and_ignored() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old"}),
        )],
    );
    let mut done = interrupted("run-old");
    done.status = RunStatus::Done;
    put_runs(&disk, ID, vec![done]);
    assert!(look(&disk, &idle).starts.is_empty());
    assert_eq!(stored_one(&disk).resume_run_id, None);
    // And one for a run that is not there.
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-none"}),
        )],
    );
    assert!(look(&disk, &idle).starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_is_refused_while_the_workflow_has_no_free_slot() {
    let root = temp_root();
    let disk = dir(&root);
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old", "concurrency": {"max": 1}}),
        )],
    );
    assert!(look(&disk, &|id| id == ID).starts.is_empty());
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old"}),
        )],
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_keeps_the_workflow_from_also_starting_a_scheduled_run() {
    let root = temp_root();
    let disk = dir(&root);
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    // Due, and a resume is asked for: the resume takes the only slot, so the occurrence waits.
    save(
        &disk,
        &[workflow(
            json!({"resumeRunId": "run-old", "concurrency": {"max": 1}}),
        )],
    );
    let found = look(&disk, &idle);
    assert_eq!(resumes_of(&found).len(), 1);
    assert!(runs_of(&found).is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_stop_nobody_runs_is_cleared_and_one_for_a_run_that_is_going_is_handed_over() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[
            workflow(json!({"enabled": false, "stopRequested": true})),
            workflow_with(OTHER, json!({"enabled": false, "stopRequested": true})),
        ],
    );
    let found = look(&disk, &|id| id == OTHER);
    assert_eq!(found.stops, vec![OTHER.to_string()]);
    assert!(stored(&disk).iter().all(|w| !w.stop_requested));
    // Asked for again, nothing is going: nothing to stop.
    save(
        &disk,
        &[workflow(json!({"enabled": false, "stopRequested": true}))],
    );
    assert!(look(&disk, &idle).stops.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_stop_asked_for_before_a_run_starts_is_dropped_at_the_claim() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"runRequested": true, "stopRequested": true}),
        )],
    );
    let found = look(&disk, &idle);
    assert_eq!(runs_of(&found).len(), 1);
    assert!(found.stops.is_empty());
    assert!(!stored_one(&disk).stop_requested);
    std::fs::remove_dir_all(root).unwrap();
}

/// Fails every save of workflows.json after the first `ok` ones.
struct Failing {
    disk: MonoDir,
    ok: Cell<usize>,
}

impl MonoFiles for Failing {
    fn read(&self, relative: &str) -> Result<AgentFile, String> {
        self.disk.read(relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        if relative == "workflows.json" {
            if self.ok.get() == 0 {
                return Err("disk full".into());
            }
            self.ok.set(self.ok.get() - 1);
        }
        self.disk.save(relative, text, expected)
    }
}

#[test]
fn a_resume_already_taken_from_the_file_is_still_acted_on_when_a_later_save_fails() {
    let root = temp_root();
    let disk = dir(&root);
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    save(
        &disk,
        &[
            workflow(json!({"enabled": false, "resumeRunId": "run-old"})),
            // Came due long ago: only moved on, which needs a save that fails.
            workflow_with(OTHER, json!({"nextRunAt": now() - 5 * 3_600_000})),
        ],
    );
    // The take is saved; the advance and the claims are not.
    let failing = Failing {
        disk: disk.clone(),
        ok: Cell::new(1),
    };
    let found = look(&failing, &idle);
    assert_eq!(resumes_of(&found).len(), 1);
    assert_eq!(stored(&disk)[0].resume_run_id, None);
    std::fs::remove_dir_all(root).unwrap();
}

// --- not touching the file for nothing --------------------------------------------------------

#[test]
fn a_look_with_nothing_to_do_does_not_write() {
    let root = temp_root();
    let counting = Counting::new(dir(&root));
    save(
        &counting.disk,
        &[workflow(json!({"nextRunAt": now() + 3_600_000}))],
    );
    assert!(look(&counting, &idle).starts.is_empty());
    assert_eq!(counting.saves.get(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_schedule_that_cannot_move_on_is_not_rewritten_every_look() {
    let root = temp_root();
    let counting = Counting::new(dir(&root));
    // An enabled schedule with a time that is no time and no next run: "advanced" on every look,
    // but advancing it changes nothing.
    let mut broken = workflow(json!({"nextRunAt": null}));
    broken.nodes[0] = super::fixtures::node(
        r#"{"id":"start","type":"schedule","schedule":{"scheduleKind":"daily","time":"99:99","minute":0,"dayOfWeek":1}}"#,
    );
    save(&counting.disk, &[broken]);
    for _ in 0..3 {
        assert!(look(&counting, &idle).starts.is_empty());
    }
    assert_eq!(counting.saves.get(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

// --- the sweep ------------------------------------------------------------------------------

fn running(id: &str) -> WorkflowRun {
    start_run(
        &workflow(json!({})),
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        id,
    )
}

fn statuses(dir: &MonoDir, workflow_id: &str) -> Vec<(String, RunStatus)> {
    load_runs(dir, workflow_id)
        .unwrap()
        .into_iter()
        .map(|r| (r.id, r.status))
        .collect()
}

#[test]
fn runs_left_running_are_marked_interrupted() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({"enabled": false}))]);
    let mut done = running("run-done");
    done.status = RunStatus::Done;
    put_runs(&disk, ID, vec![running("run-old"), done]);
    sweep_interrupted(&disk, &[]).unwrap();
    assert_eq!(
        statuses(&disk, ID),
        vec![
            ("run-old".to_string(), RunStatus::Interrupted),
            ("run-done".to_string(), RunStatus::Done)
        ]
    );
    // Nothing else of the run is invented: running nodes went back to pending.
    let run = load_runs(&disk, ID).unwrap().remove(0);
    assert_eq!(run, mark_interrupted(&running("run-old")));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_run_the_runner_is_driving_is_left_alone() {
    let root = temp_root();
    let counting = Counting::new(dir(&root));
    save(&counting.disk, &[workflow(json!({"enabled": false}))]);
    put_runs(&counting.disk, ID, vec![running("run-live")]);
    let before = counting.saves.get();
    sweep_interrupted(&counting, &[(ID.to_string(), "run-live".to_string())]).unwrap();
    assert_eq!(counting.saves.get(), before);
    assert_eq!(statuses(&counting.disk, ID)[0].1, RunStatus::Running);
    // Another run of the same workflow that nobody drives is still marked.
    put_runs(
        &counting.disk,
        ID,
        vec![running("run-live"), running("run-gone")],
    );
    sweep_interrupted(&counting, &[(ID.to_string(), "run-live".to_string())]).unwrap();
    let after = statuses(&counting.disk, ID);
    assert_eq!(after[0], ("run-live".to_string(), RunStatus::Running));
    assert_eq!(after[1], ("run-gone".to_string(), RunStatus::Interrupted));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_sweep_of_runs_that_were_not_running_writes_nothing() {
    let root = temp_root();
    let counting = Counting::new(dir(&root));
    save(&counting.disk, &[workflow(json!({"enabled": false}))]);
    put_runs(&counting.disk, ID, vec![interrupted("run-1")]);
    let before = counting.saves.get();
    sweep_interrupted(&counting, &[]).unwrap();
    assert_eq!(counting.saves.get(), before);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_run_history_that_cannot_be_read_does_not_stop_the_sweep_of_the_others() {
    struct Broken {
        disk: MonoDir,
    }
    impl MonoFiles for Broken {
        fn read(&self, relative: &str) -> Result<AgentFile, String> {
            if relative.contains(ID) {
                return Err("disk error".into());
            }
            self.disk.read(relative)
        }
        fn save(&self, r: &str, t: &str, e: Option<&str>) -> Result<String, String> {
            self.disk.save(r, t, e)
        }
    }
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[
            workflow(json!({"enabled": false})),
            workflow_with(OTHER, json!({"enabled": false})),
        ],
    );
    put_runs(&disk, OTHER, vec![running("run-x")]);
    let broken = Broken { disk: disk.clone() };
    // The workflow file itself reads through `workflows.json`; only the first history fails.
    assert_eq!(sweep_interrupted(&broken, &[]).unwrap_err(), "disk error");
    assert_eq!(statuses(&disk, OTHER)[0].1, RunStatus::Interrupted);
    std::fs::remove_dir_all(root).unwrap();
}

// --- run values and requirements -------------------------------------------------------------

fn counted(extra: Value) -> Workflow {
    let mut base = json!({
        "inputs": {"repo": {"type": "project"}, "count": {"type": "number"}},
        "inputValues": {"repo": "/code/app", "count": 2}
    });
    for (k, v) in extra.as_object().unwrap() {
        base[k] = v.clone();
    }
    workflow(base)
}

fn count_of(w: &Workflow) -> Option<InputValue> {
    w.input_values.get("count").cloned()
}

#[test]
fn a_requested_run_uses_its_run_values_and_keeps_the_stored_ones() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[counted(
            json!({"runRequested": true, "runValues": {"count": 9}}),
        )],
    );
    let found = look(&disk, &idle);
    let (run, failure) = runs_of(&found)[0];
    assert!(failure.is_none());
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(count_of(&run.definition), Some(InputValue::Number(9.0)));
    // Kept on the run's snapshot only so a run put back in the queue knows its own values.
    assert!(run.definition.run_values.is_some());
    let after = stored_one(&disk);
    assert_eq!(count_of(&after), Some(InputValue::Number(2.0)));
    assert!(after.run_values.is_none() && !after.run_requested);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn run_values_that_no_longer_fit_the_definition_fail_the_run() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[counted(
            json!({"runRequested": true, "runValues": {"count": "many"}}),
        )],
    );
    let found = look(&disk, &idle);
    let (run, failure) = runs_of(&found)[0];
    assert_eq!(run.status, RunStatus::Failed);
    assert!(run
        .error
        .as_deref()
        .unwrap()
        .contains("The workflow is not valid"));
    assert!(failure.unwrap().text.contains("count"));
    assert!(stored_one(&disk).run_values.is_none());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_scheduled_run_with_a_missing_requirement_fails_with_a_chat_message_and_the_schedule_moves_on()
{
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"requires": [{"cli": "gh", "hint": "brew install gh"}]}),
        )],
    );
    let found = look_pf(&disk, &idle, &all_missing);
    let (run, failure) = runs_of(&found)[0];
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.caller, RunCaller::new(CallerKind::Schedule, "start"));
    assert_eq!(run.ended_at, Some(now()));
    let error = run.error.as_deref().unwrap();
    assert_eq!(
        error,
        "Missing requirements: cli gh: not found on PATH (brew install gh)"
    );
    let failure = failure.unwrap();
    assert_eq!(failure.title, "Ship it \u{b7} failed");
    assert!(failure
        .text
        .contains("The workflow could not start. Missing requirements:"));
    let after = stored_one(&disk);
    assert!(after.next_run_at.unwrap() > now());
    assert_eq!(after.last_run_id.as_deref(), Some(run.id.as_str()));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_requested_run_with_a_missing_requirement_fails_the_same_way_and_is_consumed() {
    let root = temp_root();
    let disk = dir(&root);
    let later = now() + 3_600_000;
    save(
        &disk,
        &[workflow(
            json!({"runRequested": true, "nextRunAt": later, "requires": [{"env": "TOKEN"}]}),
        )],
    );
    let found = look_pf(&disk, &idle, &all_missing);
    let (run, failure) = runs_of(&found)[0];
    assert_eq!(run.caller, RunCaller::new(CallerKind::Manual, "start"));
    assert_eq!(run.status, RunStatus::Failed);
    assert!(run
        .error
        .as_deref()
        .unwrap()
        .starts_with("Missing requirements: env TOKEN"));
    assert!(failure.is_some());
    let after = stored_one(&disk);
    assert!(!after.run_requested);
    assert_eq!(after.next_run_at, Some(later));
    assert!(look(&disk, &idle).starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn met_requirements_start_the_run_and_a_workflow_without_any_is_never_preflighted() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({"requires": [{"cli": "gh"}]}))]);
    let found = look_pf(&disk, &idle, &all_found);
    assert_eq!(runs_of(&found)[0].0.status, RunStatus::Running);
    save(&disk, &[workflow(json!({}))]);
    let asked = Cell::new(0);
    let counting = |w: &Workflow| {
        asked.set(asked.get() + 1);
        all_missing(w)
    };
    let found = look_pf(&disk, &idle, &counting);
    assert_eq!(runs_of(&found)[0].0.status, RunStatus::Running);
    assert_eq!(asked.get(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_is_refused_when_the_removed_worktree_has_no_repository_to_come_back_from() {
    let root = temp_root();
    let disk = dir(&root);
    let mut run = mark_interrupted(&start_run(
        &workflow(json!({})),
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-old",
    ));
    run.worktree = Some(RunWorktree {
        path: "/nowhere/app-worktrees/wf-x-1".into(),
        branch: "wf/x-1".into(),
        base: "abc".into(),
        removed_at: Some(5),
    });
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old"}),
        )],
    );
    put_runs(&disk, ID, vec![run]);
    let found = look(&disk, &idle);
    let Start::Refused {
        run_id, failure, ..
    } = &found.starts[0]
    else {
        panic!("not refused: {:?}", found.starts);
    };
    assert_eq!(run_id, "run-old");
    assert!(failure.text.contains("cannot continue"), "{}", failure.text);
    // The run stays interrupted and the request is consumed.
    assert_eq!(
        load_runs(&disk, ID).unwrap()[0].status,
        RunStatus::Interrupted
    );
    assert_eq!(stored_one(&disk).resume_run_id, None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_is_refused_while_a_requirement_is_missing_and_the_run_stays_interrupted() {
    let root = temp_root();
    let disk = dir(&root);
    // The run was made when the workflow required gh; the live workflow now adds TOKEN.
    let mut old = workflow(json!({}));
    old.requires = vec![Requirement {
        kind: RequirementKind::Cli,
        name: "gh".into(),
        hint: None,
    }];
    let run = mark_interrupted(&start_run(
        &old,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-old",
    ));
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old", "requires": [{"env": "TOKEN"}]}),
        )],
    );
    put_runs(&disk, ID, vec![run]);
    let seen = RefCell::new(Vec::new());
    let refusing = |w: &Workflow| {
        seen.borrow_mut().push(
            w.requires
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>(),
        );
        all_missing(w)
    };
    let claimed = RefCell::new(Vec::new());
    let on_claim = |id: &str, _run: &str| claimed.borrow_mut().push(id.to_string());
    let found = check(
        &disk,
        &Tick {
            now: now(),
            live_runs: &|_| 0,
            probe: probe_nothing,
            on_claim: &on_claim,
            preflight: &refusing,
        },
    )
    .unwrap();
    assert_eq!(found.starts.len(), 1);
    let Start::Refused {
        workflow_id,
        workflow_name,
        run_id,
        failure,
    } = &found.starts[0]
    else {
        panic!("not refused: {:?}", found.starts);
    };
    assert_eq!(
        (
            workflow_id.as_str(),
            workflow_name.as_str(),
            run_id.as_str()
        ),
        (ID, "Ship it", "run-old")
    );
    assert!(failure.text.contains("Missing requirements: cli gh"));
    assert!(failure.text.contains("env TOKEN"));
    assert_eq!(
        *seen.borrow(),
        vec![vec!["gh".to_string(), "TOKEN".to_string()]]
    );
    assert_eq!(*claimed.borrow(), vec![ID.to_string()]);
    // The run is untouched and the request is consumed.
    assert_eq!(
        load_runs(&disk, ID).unwrap()[0].status,
        RunStatus::Interrupted
    );
    assert_eq!(stored_one(&disk).resume_run_id, None);
    assert!(look(&disk, &idle).starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_goes_ahead_when_the_requirements_are_met_with_the_values_it_started_with() {
    let root = temp_root();
    let disk = dir(&root);
    // The run started with count 9 (a run value); the stored value is 2.
    let started = counted(json!({"requires": [{"cli": "gh"}], "runValues": {"count": 9}}));
    let run = mark_interrupted(&start_run(
        &super::model::with_run_values(&started),
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-old",
    ));
    save(
        &disk,
        &[counted(
            json!({"enabled": false, "resumeRunId": "run-old", "requires": [{"cli": "gh"}]}),
        )],
    );
    put_runs(&disk, ID, vec![run]);
    let found = look_pf(&disk, &idle, &all_found);
    let resumes = resumes_of(&found);
    assert_eq!(resumes.len(), 1);
    assert_eq!(
        count_of(&resumes[0].definition),
        Some(InputValue::Number(9.0))
    );
    assert_eq!(count_of(&stored_one(&disk)), Some(InputValue::Number(2.0)));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_of_a_run_that_requires_nothing_is_not_preflighted() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"enabled": false, "resumeRunId": "run-old"}),
        )],
    );
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    let asked = Cell::new(0);
    let counting = |w: &Workflow| {
        asked.set(asked.get() + 1);
        all_missing(w)
    };
    let found = look_pf(&disk, &idle, &counting);
    assert_eq!(resumes_of(&found).len(), 1);
    assert_eq!(asked.get(), 0);
    std::fs::remove_dir_all(root).unwrap();
}

// --- concurrency and the queue ---------------------------------------------------------------

use super::queue::{enqueue, queued_count, queued_run};

/// A look where `live(id)` runs of a workflow are going.
fn look_live(files: &impl MonoFiles, live: &dyn Fn(&str) -> usize) -> Found {
    check(
        files,
        &Tick {
            now: now(),
            live_runs: live,
            probe: probe_nothing,
            on_claim: &|_, _| {},
            preflight: &|_| Preflight::default(),
        },
    )
    .unwrap()
}

/// A workflow that is not due and has run before, so only the queue has something to do.
fn quiet(extra: Value) -> Workflow {
    let mut spec = json!({"lastRunId": "earlier", "nextRunAt": now() + 3_600_000});
    for (k, x) in extra.as_object().unwrap() {
        spec[k] = x.clone();
    }
    workflow(spec)
}

fn waiting(id: &str, n: u64) -> WorkflowRun {
    queued_run(
        &workflow(json!({})),
        RunCaller::new(CallerKind::Manual, "start"),
        n,
        id,
        n * 10,
    )
}

fn queue_of(dir: &MonoDir) -> Vec<(String, RunStatus)> {
    let mut runs = statuses(dir, ID);
    runs.reverse();
    runs
}

#[test]
fn a_workflow_goes_up_to_its_concurrency_and_queues_the_start_over_it() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"concurrency": {"max": 2}, "runRequested": true}),
        )],
    );
    // One of the two slots is free: the request starts, as run number 1.
    let found = look_live(&disk, &|_| 1);
    let runs = runs_of(&found);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].0.number, Some(1));
    // Both are taken: the next request waits as a queued run, number 2, and nothing starts.
    let mut again = stored_one(&disk);
    again.run_requested = true;
    save(&disk, &[again]);
    let found = look_live(&disk, &|_| 2);
    assert!(found.starts.is_empty());
    let stored = load_runs(&disk, ID).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].status, RunStatus::Queued);
    assert_eq!(stored[0].number, Some(2));
    assert_eq!(stored[0].queued_at, Some(now()));
    assert_eq!(stored[0].caller.kind, CallerKind::Manual);
    assert_eq!(stored_one(&disk).run_counter, 2);
    assert!(!stored_one(&disk).run_requested);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn the_default_concurrency_is_three() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({"runRequested": true}))]);
    assert_eq!(runs_of(&look_live(&disk, &|_| 2)).len(), 1);
    let mut again = stored_one(&disk);
    again.run_requested = true;
    save(&disk, &[again]);
    let found = look_live(&disk, &|_| 3);
    assert!(found.starts.is_empty());
    assert_eq!(queued_count(&load_runs(&disk, ID).unwrap()), 1);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queued_runs_start_first_in_first_out_as_slots_free_up() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[quiet(json!({"concurrency": {"max": 2}}))]);
    put_runs(
        &disk,
        ID,
        vec![waiting("q3", 3), waiting("q2", 2), waiting("q1", 1)],
    );
    let claimed = RefCell::new(Vec::new());
    let on_claim = |_: &str, run: &str| claimed.borrow_mut().push(run.to_string());
    let found = check(
        &disk,
        &Tick {
            now: now(),
            live_runs: &|_| 0,
            probe: probe_nothing,
            on_claim: &on_claim,
            preflight: &|_| Preflight::default(),
        },
    )
    .unwrap();
    // The two oldest start, oldest first, with the number, queue time and caller they had.
    let started: Vec<(&str, Option<u64>, Option<u64>)> = runs_of(&found)
        .iter()
        .map(|(r, _)| (r.id.as_str(), r.number, r.queued_at))
        .collect();
    assert_eq!(
        started,
        vec![("q1", Some(1), Some(10)), ("q2", Some(2), Some(20))]
    );
    assert_eq!(*claimed.borrow(), vec!["q1".to_string(), "q2".to_string()]);
    assert_eq!(
        queue_of(&disk),
        vec![
            ("q1".into(), RunStatus::Running),
            ("q2".into(), RunStatus::Running),
            ("q3".into(), RunStatus::Queued)
        ]
    );
    // One run ends and a slot frees up: the third goes, and no run is started twice.
    update_runs(&disk, ID, |runs| {
        let list = runs
            .into_iter()
            .map(|mut r| {
                if r.id == "q1" {
                    r.status = RunStatus::Done;
                }
                r
            })
            .collect();
        Ok((list, ()))
    })
    .unwrap();
    let found = look_live(&disk, &|_| 1);
    assert_eq!(
        runs_of(&found)
            .iter()
            .map(|(r, _)| r.id.as_str())
            .collect::<Vec<_>>(),
        vec!["q3"]
    );
    assert!(look_live(&disk, &|_| 0).starts.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_full_workflow_starts_nothing_from_its_queue() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[quiet(json!({"concurrency": {"max": 2}}))]);
    put_runs(&disk, ID, vec![waiting("q2", 2), waiting("q1", 1)]);
    assert!(look_live(&disk, &|_| 2).starts.is_empty());
    assert_eq!(queued_count(&load_runs(&disk, ID).unwrap()), 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_queued_run_starts_on_the_definition_of_that_moment_with_its_own_values() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[quiet(json!({"name": "Renamed since"}))]);
    let mut asked = workflow(json!({}));
    let mut values = OrderedMap::new();
    values.insert("repo".to_string(), InputValue::String("/code/lib".into()));
    asked.run_values = Some(values);
    put_runs(
        &disk,
        ID,
        vec![queued_run(
            &asked,
            RunCaller::new(CallerKind::Manual, "start"),
            1,
            "q1",
            10,
        )],
    );
    let found = look_live(&disk, &|_| 0);
    let runs = runs_of(&found);
    assert_eq!(runs.len(), 1);
    let run = runs[0].0;
    // The stored workflow is as it is now; the values are the ones asked for this run.
    assert_eq!(run.definition.name, "Renamed since");
    assert_eq!(
        run.definition.input_values.get("repo"),
        Some(&InputValue::String("/code/lib".into()))
    );
    // Kept on the run's snapshot only so a run put back in the queue knows its own values.
    assert!(run.definition.run_values.is_some());
    assert_eq!(
        stored_one(&disk).input_values.get("repo"),
        Some(&InputValue::String("/code/app".into()))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_workflow_waiting_for_review_keeps_its_queue_and_a_paused_one_keeps_its_scheduled_runs() {
    let root = temp_root();
    let disk = dir(&root);
    // Waiting for a review: nothing queued starts, whoever asked.
    save(&disk, &[quiet(json!({"needsReview": true}))]);
    put_runs(&disk, ID, vec![waiting("q1", 1)]);
    assert!(look_live(&disk, &|_| 0).starts.is_empty());
    assert_eq!(queued_count(&load_runs(&disk, ID).unwrap()), 1);
    // Paused: a run a trigger asked for waits, a run asked for by hand goes.
    save(&disk, &[quiet(json!({"enabled": false}))]);
    let mut scheduled = waiting("q2", 2);
    scheduled.caller = RunCaller::new(CallerKind::Schedule, "start");
    put_runs(&disk, ID, vec![scheduled, waiting("q1", 1)]);
    let found = look_live(&disk, &|_| 0);
    assert_eq!(
        runs_of(&found)
            .iter()
            .map(|(r, _)| r.id.as_str())
            .collect::<Vec<_>>(),
        vec!["q1"]
    );
    assert_eq!(queue_of(&disk)[1], ("q2".to_string(), RunStatus::Queued));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn queued_runs_stay_queued_over_a_restart_and_start_by_themselves_as_capacity_allows() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[quiet(json!({"concurrency": {"max": 1}}))]);
    // The last session left one run going and one waiting.
    put_runs(&disk, ID, vec![waiting("q1", 2), running("was-going")]);
    // Until the sweep, the file still says a run goes, so the queue waits.
    assert!(look_live(&disk, &|_| 0).starts.is_empty());
    sweep_interrupted(&disk, &[]).unwrap();
    assert_eq!(
        statuses(&disk, ID),
        vec![
            ("q1".to_string(), RunStatus::Queued),
            ("was-going".to_string(), RunStatus::Interrupted)
        ]
    );
    // The sweep leaves the queued run alone, and the next look starts it. The interrupted one is
    // not resumed on its own.
    let found = look_live(&disk, &|_| 0);
    assert_eq!(
        runs_of(&found)
            .iter()
            .map(|(r, _)| r.id.as_str())
            .collect::<Vec<_>>(),
        vec!["q1"]
    );
    assert!(resumes_of(&found).is_empty());
    assert_eq!(
        statuses(&disk, ID),
        vec![
            ("q1".to_string(), RunStatus::Running),
            ("was-going".to_string(), RunStatus::Interrupted)
        ]
    );
    std::fs::remove_dir_all(root).unwrap();
}

/// Lets a rival look start the queued run just before the save of the runs file, so the save
/// meets a changed file.
struct Rival {
    disk: MonoDir,
    armed: Cell<bool>,
}

impl MonoFiles for Rival {
    fn read(&self, relative: &str) -> Result<AgentFile, String> {
        self.disk.read(relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        if relative.contains("wf-abcd1234") && self.armed.replace(false) {
            update_runs(&self.disk, ID, |runs| {
                let list = runs
                    .into_iter()
                    .map(|r| {
                        if r.id == "q1" {
                            let mut started = running("q1");
                            started.number = r.number;
                            started
                        } else {
                            r
                        }
                    })
                    .collect();
                Ok((list, ()))
            })
            .unwrap();
        }
        self.disk.save(relative, text, expected)
    }
}

#[test]
fn a_queued_run_is_never_started_twice_when_another_look_got_there_first() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[quiet(json!({"concurrency": {"max": 1}}))]);
    put_runs(&disk, ID, vec![waiting("q1", 1)]);
    let rival = Rival {
        disk: disk.clone(),
        armed: Cell::new(true),
    };
    let claimed = Cell::new(0);
    let on_claim = |_: &str, _: &str| claimed.set(claimed.get() + 1);
    let found = check(
        &rival,
        &Tick {
            now: now(),
            live_runs: &|_| 0,
            probe: probe_nothing,
            on_claim: &on_claim,
            preflight: &|_| Preflight::default(),
        },
    )
    .unwrap();
    // The rival started it; this look saw the newest file, found it going and started nothing.
    assert!(found.starts.is_empty());
    assert_eq!(claimed.get(), 0);
    assert_eq!(
        statuses(&disk, ID),
        vec![("q1".to_string(), RunStatus::Running)]
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_schedule_that_comes_due_while_one_run_waits_is_skipped_and_moves_on() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({"concurrency": {"max": 1}}))]);
    put_runs(&disk, ID, vec![waiting("q1", 1)]);
    let found = look_live(&disk, &|_| 1);
    assert!(found.starts.is_empty());
    // Still the one that was waiting; nothing was added for the occurrence.
    assert_eq!(queue_of(&disk), vec![("q1".to_string(), RunStatus::Queued)]);
    assert!(stored_one(&disk).next_run_at.unwrap() > now());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_schedule_that_comes_due_with_every_slot_taken_waits_in_the_queue_once() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({"concurrency": {"max": 1}}))]);
    assert!(look_live(&disk, &|_| 1).starts.is_empty());
    let stored = load_runs(&disk, ID).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].status, RunStatus::Queued);
    assert_eq!(stored[0].caller.kind, CallerKind::Schedule);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_manual_start_over_the_cap_is_recorded_as_skipped_not_queued() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[workflow(
            json!({"concurrency": {"max": 1}, "runRequested": true}),
        )],
    );
    for n in 0..QUEUED_RUNS_MAX {
        enqueue(&disk, waiting(&format!("q{n}"), n as u64 + 1)).unwrap();
    }
    assert!(look_live(&disk, &|_| 1).starts.is_empty());
    let runs = load_runs(&disk, ID).unwrap();
    assert_eq!(queued_count(&runs), QUEUED_RUNS_MAX);
    let skipped: Vec<&WorkflowRun> = runs
        .iter()
        .filter(|r| r.status == RunStatus::Skipped)
        .collect();
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0]
        .error
        .as_deref()
        .unwrap()
        .contains("queue is full"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_stop_asked_for_a_queued_run_cancels_it_and_one_for_a_going_run_is_handed_over() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[quiet(
            json!({"stopRunIds": ["q1", "going"], "concurrency": {"max": 1}}),
        )],
    );
    put_runs(&disk, ID, vec![waiting("q2", 2), waiting("q1", 1)]);
    let found = look_live(&disk, &|_| 1);
    assert_eq!(
        found.stop_runs,
        vec![
            (ID.to_string(), "q1".to_string()),
            (ID.to_string(), "going".to_string())
        ]
    );
    assert_eq!(
        queue_of(&disk),
        vec![
            ("q1".to_string(), RunStatus::Stopped),
            ("q2".to_string(), RunStatus::Queued)
        ]
    );
    // Taken: the request does not come back.
    assert!(stored_one(&disk).stop_run_ids.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_resume_waits_for_nothing_but_needs_a_free_slot() {
    let root = temp_root();
    let disk = dir(&root);
    save(
        &disk,
        &[quiet(
            json!({"concurrency": {"max": 2}, "resumeRunId": "run-old"}),
        )],
    );
    put_runs(&disk, ID, vec![interrupted("run-old")]);
    // One of two slots is taken: the resume goes next to the run that is going.
    let found = look_live(&disk, &|_| 1);
    assert_eq!(resumes_of(&found).len(), 1);
    // With both taken a resume is not started over them.
    let mut again = stored_one(&disk);
    again.resume_run_id = Some("run-old".into());
    save(&disk, &[again]);
    let found = look_live(&disk, &|_| 2);
    assert!(resumes_of(&found).is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

/// Removes the workflow (and clears its runs) just before the first save of its runs file, as
/// `workflows.remove` does between a tick's claim and its write.
struct Remover {
    disk: MonoDir,
    armed: Cell<bool>,
}

impl MonoFiles for Remover {
    fn read(&self, relative: &str) -> Result<AgentFile, String> {
        self.disk.read(relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        if relative.contains("wf-abcd1234") && self.armed.replace(false) {
            update_workflows(&self.disk, |_| Ok((Vec::new(), ()))).unwrap();
            clear_runs(&self.disk, ID);
        }
        self.disk.save(relative, text, expected)
    }
}

#[test]
fn a_run_claimed_just_before_the_workflow_is_removed_is_not_queued_after_it() {
    let root = temp_root();
    let disk = dir(&root);
    save(&disk, &[workflow(json!({"concurrency": {"max": 1}}))]);
    let remover = Remover {
        disk: disk.clone(),
        armed: Cell::new(true),
    };
    let found = check(
        &remover,
        &Tick {
            now: now(),
            live_runs: &|_| 1,
            probe: probe_nothing,
            on_claim: &|_, _| {},
            preflight: &|_| Preflight::default(),
        },
    )
    .unwrap();
    assert!(found.starts.is_empty());
    assert!(load_workflows(&disk).unwrap().workflows.is_empty());
    // Neither a lost run in a live file nor an orphan run of the removed workflow.
    assert!(load_runs(&disk, ID).unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn a_claim_that_leaves_more_requests_behind_asks_for_another_look() {
    let root = temp_root();
    let disk = dir(&root);
    let mut w = workflow(json!({"runRequested": true}));
    w.run_backlog = vec![RunRequest {
        caller: None,
        values: None,
    }];
    save(&disk, &[w]);
    let found = look(&disk, &idle);
    assert!(found.more_requests);
    // The next request is the pending one now, so the next look claims it.
    assert!(stored_one(&disk).run_requested);
    let found = look(&disk, &idle);
    assert!(!found.more_requests);
    assert!(!stored_one(&disk).run_requested);
    std::fs::remove_dir_all(root).unwrap();
}
