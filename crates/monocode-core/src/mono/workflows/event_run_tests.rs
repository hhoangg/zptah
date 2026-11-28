//! Starting a run for an Inbox item: the claim comes first, one run per workflow per item, the
//! queue and its cap, a paused or unreviewed workflow, and the manual test run. Real files and
//! a real database in temp dirs; the runner side is tested in `monocode-gpui`.
use super::engine::start_run;
use super::event_run::*;
use super::fixtures::workflow_from;
use super::model::*;
use super::models::Probe;
use super::preflight::Preflight;
use super::queue::{enqueue, queued_run};
use super::store::*;
use super::tick::{Start, Tick};
use crate::inbox::Item;
use crate::mono::files::AgentFile;
use crate::provider::Kind;
use crate::store::Store;
use std::path::PathBuf;

const ID: &str = "wf-event001";
const NOW: u64 = 1_800_000_000_000;

struct Env {
    files: MonoDir,
    root: PathBuf,
    store: Store,
    _guard: tempfile::TempDir,
}

impl Env {
    fn files_root(&self) -> PathBuf {
        self.root.clone()
    }
}

fn env() -> Env {
    let guard = tempfile::tempdir().unwrap();
    let root: PathBuf = guard.path().join("files");
    std::fs::create_dir_all(&root).unwrap();
    let store = Store::open_monocode(&guard.path().join("monocode.db")).unwrap();
    Env {
        files: MonoDir::new(root.clone(), "mono-1"),
        root,
        store,
        _guard: guard,
    }
}

/// Two event triggers (`first` and `second`) lead to one session.
fn workflow(extra: &str) -> Workflow {
    workflow_from(&format!(
        r#"{{"id":"{ID}","name":"On new PR","inputs":{{}},"inputValues":{{}},
            "nodes":[
              {{"id":"first","type":"event","source":"github","event":"pull_request_opened","project":"/code/app"}},
              {{"id":"second","type":"event","source":"github","event":"pull_request_opened","project":"/code/app","repos":["acme/app"]}},
              {{"id":"work","type":"session","project":"/code/app","prompt":"Review {{{{trigger.item.title}}}}","checkout":"root","outputs":{{}}}}],
            "edges":[{{"from":"first","to":"work"}},{{"from":"second","to":"work"}}],
            "enabled":true,"createdAt":1{extra}}}"#
    ))
}

fn save(env: &Env, w: Workflow) {
    update_workflows(&env.files, |_| Ok((vec![w.clone()], ()))).unwrap();
}

fn pr(number: i64) -> Item {
    Item {
        provider: "github".into(),
        kind: "pr".into(),
        number,
        repo: "acme/app".into(),
        project_path: "/code/app".into(),
        title: format!("Fix {number}"),
        url: format!("https://example.test/{number}"),
        ..Item::default()
    }
}

fn probe(_: Kind) -> Probe {
    Probe {
        installed: true,
        models: None,
    }
}

fn tick<'a>(live: &'a dyn Fn(&str) -> usize, claimed: &'a dyn Fn(&str, &str)) -> Tick<'a> {
    Tick {
        now: NOW,
        live_runs: live,
        probe,
        on_claim: claimed,
        preflight: &|_| Preflight::default(),
    }
}

fn idle(_: &str) -> usize {
    0
}

fn start(env: &Env, trigger: &str, item: &Item) -> EventOutcome {
    start_with(env, trigger, item, &idle)
}

fn start_with(env: &Env, trigger: &str, item: &Item, live: &dyn Fn(&str) -> usize) -> EventOutcome {
    start_event_run(
        &env.files,
        &env.store,
        &tick(live, &|_, _| {}),
        ID,
        trigger,
        item,
    )
}

fn run_of(outcome: &EventOutcome) -> &WorkflowRun {
    match outcome {
        EventOutcome::Started {
            start: Start::Run { run, .. },
            ..
        } => run,
        other => panic!("expected a started run, got {other:?}"),
    }
}

/// The runner took the run: the claim learns its id, as `Workflows::start_event_run` does.
fn taken(env: &Env, outcome: &EventOutcome) {
    let EventOutcome::Started { claim, .. } = outcome else {
        panic!("not started")
    };
    claim.record_run(&env.store, &run_of(outcome).id).unwrap();
}

#[test]
fn an_event_start_claims_the_item_and_makes_a_run_for_it() {
    let env = env();
    save(&env, workflow(""));
    let outcome = start(&env, "first", &pr(7));
    let run = run_of(&outcome);
    assert_eq!(run.status, RunStatus::Running);
    assert_eq!(run.caller.kind, CallerKind::Event);
    assert_eq!(run.caller.trigger_node, "first");
    let item = run.caller.item.as_ref().expect("the item is recorded");
    assert_eq!(item.get("title").and_then(|t| t.as_str()), Some("Fix 7"));
    // The firing trigger finished with the item as its output; the other one is skipped.
    let fired = run.nodes.iter().find(|n| n.node_id == "first").unwrap();
    assert_eq!(fired.status, NodeStatus::Done);
    let output = fired.output.as_ref().unwrap();
    assert_eq!(output.get("ref").and_then(|v| v.as_str()), Some("#7"));
    assert_eq!(
        output.get("repo").and_then(|v| v.as_str()),
        Some("acme/app")
    );
    let other = run.nodes.iter().find(|n| n.node_id == "second").unwrap();
    assert_eq!(other.status, NodeStatus::Skipped);
    // The run took the next number, and the workflow remembers it.
    assert_eq!(run.number, Some(1));
    let stored = load_workflows(&env.files).unwrap().workflows.remove(0);
    assert_eq!(stored.run_counter, 1);
    assert_eq!(stored.last_run_id.as_deref(), Some(run.id.as_str()));
}

#[test]
fn the_run_scope_has_the_trigger_item() {
    let env = env();
    save(&env, workflow(""));
    let outcome = start(&env, "first", &pr(9));
    let run = run_of(&outcome);
    let scope = super::engine::run_scope(run, "", Some("work"));
    let trigger = scope.get("trigger").expect("trigger in scope");
    assert_eq!(trigger.get("kind").and_then(|v| v.as_str()), Some("event"));
    let item = trigger.get("item").expect("item in scope");
    assert_eq!(
        item.get("url").and_then(|v| v.as_str()),
        Some("https://example.test/9")
    );
    assert_eq!(item.get("source").and_then(|v| v.as_str()), Some("github"));
}

#[test]
fn the_same_item_twice_makes_one_run() {
    let env = env();
    save(&env, workflow(""));
    let first = start(&env, "first", &pr(7));
    taken(&env, &first);
    assert_eq!(start(&env, "first", &pr(7)), EventOutcome::Duplicate);
    // Another item is another run.
    assert!(matches!(
        start(&env, "first", &pr(8)),
        EventOutcome::Started { .. }
    ));
}

#[test]
fn two_triggers_for_the_same_item_make_one_run() {
    let env = env();
    save(&env, workflow(""));
    assert!(matches!(
        start(&env, "first", &pr(7)),
        EventOutcome::Started { .. }
    ));
    assert_eq!(start(&env, "second", &pr(7)), EventOutcome::Duplicate);
}

#[test]
fn the_claim_is_taken_before_the_run_exists() {
    let env = env();
    save(&env, workflow(""));
    // Someone else holds the claim: nothing is made, not even a queued record or a number.
    assert!(env
        .store
        .claim_workflow_item(
            ID,
            "second",
            &crate::mono::workflows::event::event_key(&pr(7))
        )
        .unwrap());
    assert_eq!(start(&env, "first", &pr(7)), EventOutcome::Duplicate);
    let stored = load_workflows(&env.files).unwrap().workflows.remove(0);
    assert_eq!(stored.run_counter, 0);
    assert!(load_runs(&env.files, ID).unwrap().is_empty());
}

/// Files that read fine and refuse to save the workflow list: making the run fails.
struct SaveFails(MonoDir);

impl MonoFiles for SaveFails {
    fn read(&self, relative: &str) -> Result<AgentFile, String> {
        self.0.read(relative)
    }
    fn save(&self, relative: &str, text: &str, expected: Option<&str>) -> Result<String, String> {
        if relative == crate::mono::files::WORKFLOWS_FILE {
            return Err("disk full".into());
        }
        self.0.save(relative, text, expected)
    }
}

#[test]
fn a_run_that_could_not_be_made_gives_the_claim_back() {
    let env = env();
    save(&env, workflow(""));
    let broken = SaveFails(MonoDir::new(env.files_root(), "mono-1"));
    let outcome = start_event_run(
        &broken,
        &env.store,
        &tick(&idle, &|_, _| {}),
        ID,
        "first",
        &pr(7),
    );
    assert!(
        matches!(outcome, EventOutcome::Failed(ref e) if e.contains("disk full")),
        "{outcome:?}"
    );
    assert!(env.store.unclaimed_workflow_claims().unwrap().is_empty());
    // A retry can take the item again.
    assert!(matches!(
        start(&env, "first", &pr(7)),
        EventOutcome::Started { .. }
    ));
}

#[test]
fn a_busy_workflow_queues_the_run_and_the_claim_names_it() {
    let env = env();
    save(&env, workflow(r#","concurrency":{"max":1}"#));
    let outcome = start_with(&env, "first", &pr(7), &|_| 1);
    let EventOutcome::Queued { run_id, position } = outcome else {
        panic!("expected a queued run, got {outcome:?}")
    };
    assert_eq!(position, 1);
    let runs = load_runs(&env.files, ID).unwrap();
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].id, run_id);
    assert_eq!(runs[0].status, RunStatus::Queued);
    assert_eq!(runs[0].caller.kind, CallerKind::Event);
    assert!(runs[0].caller.item.is_some());
    // The claim has its run: the startup check leaves it alone.
    assert!(env.store.unclaimed_workflow_claims().unwrap().is_empty());
    assert_eq!(
        start_with(&env, "first", &pr(7), &|_| 1),
        EventOutcome::Duplicate
    );
}

#[test]
fn an_event_over_the_queue_cap_is_recorded_as_a_skipped_run() {
    let env = env();
    save(&env, workflow(r#","concurrency":{"max":1}"#));
    let w = workflow(r#","concurrency":{"max":1}"#);
    for n in 0..QUEUED_RUNS_MAX {
        let run = queued_run(
            &w,
            RunCaller::new(CallerKind::Manual, "first"),
            n as u64 + 1,
            &format!("queued-{n}"),
            n as u64,
        );
        enqueue(&env.files, run).unwrap();
    }
    let outcome = start_with(&env, "first", &pr(7), &|_| 1);
    let EventOutcome::Skipped { run } = outcome else {
        panic!("expected a skipped run, got {outcome:?}")
    };
    assert_eq!(run.status, RunStatus::Skipped);
    assert!(run.error.as_deref().unwrap().contains("queue is full"));
    assert_eq!(run.caller.kind, CallerKind::Event);
    let runs = load_runs(&env.files, ID).unwrap();
    assert_eq!(
        runs.iter()
            .filter(|r| r.status == RunStatus::Queued)
            .count(),
        QUEUED_RUNS_MAX
    );
    assert!(runs
        .iter()
        .any(|r| r.id == run.id && r.status == RunStatus::Skipped));
    // It takes no number, and the item is not tried again.
    let stored = load_workflows(&env.files).unwrap().workflows.remove(0);
    assert_eq!(stored.run_counter, 0);
    assert_eq!(
        start_with(&env, "first", &pr(7), &|_| 1),
        EventOutcome::Duplicate
    );
}

#[test]
fn a_paused_workflow_never_claims() {
    let env = env();
    let mut w = workflow("");
    w.enabled = false;
    save(&env, w);
    assert_eq!(
        start(&env, "first", &pr(7)),
        EventOutcome::Held(Held::Paused)
    );
    assert!(load_runs(&env.files, ID).unwrap().is_empty());
    // Nothing was claimed: once resumed the same item starts a run.
    update_workflows(&env.files, |mut list| {
        list[0].enabled = true;
        Ok((list, ()))
    })
    .unwrap();
    assert!(matches!(
        start(&env, "first", &pr(7)),
        EventOutcome::Started { .. }
    ));
}

#[test]
fn a_workflow_that_needs_review_never_claims() {
    let env = env();
    let mut w = workflow("");
    w.needs_review = true;
    save(&env, w);
    assert_eq!(
        start(&env, "first", &pr(7)),
        EventOutcome::Held(Held::NeedsReview)
    );
    assert!(env
        .store
        .claim_workflow_item(
            ID,
            "first",
            &crate::mono::workflows::event::event_key(&pr(7))
        )
        .unwrap());
}

#[test]
fn a_missing_workflow_or_a_trigger_that_is_not_an_event_is_ignored() {
    let env = env();
    assert_eq!(start(&env, "first", &pr(7)), EventOutcome::Ignored);
    save(&env, workflow(""));
    assert_eq!(start(&env, "work", &pr(7)), EventOutcome::Ignored);
    assert_eq!(start(&env, "nope", &pr(7)), EventOutcome::Ignored);
}

#[test]
fn a_claim_the_runner_refused_is_released() {
    let env = env();
    save(&env, workflow(""));
    let outcome = start(&env, "first", &pr(7));
    let EventOutcome::Started { claim, .. } = &outcome else {
        panic!("not started")
    };
    // The claim waits for its run until the runner takes it.
    assert_eq!(env.store.unclaimed_workflow_claims().unwrap().len(), 1);
    claim.release(&env.store).unwrap();
    assert!(env.store.unclaimed_workflow_claims().unwrap().is_empty());
    assert!(matches!(
        start(&env, "first", &pr(7)),
        EventOutcome::Started { .. }
    ));
}

#[test]
fn the_startup_check_releases_claims_without_a_run_only() {
    let env = env();
    save(&env, workflow(""));
    // One claim that got its run, one that did not (the app stopped in between).
    let done = start(&env, "first", &pr(7));
    taken(&env, &done);
    assert!(matches!(
        start(&env, "first", &pr(8)),
        EventOutcome::Started { .. }
    ));
    let released = release_unclaimed(&env.store, &env.files).unwrap();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].workflow_id, ID);
    assert!(env.store.unclaimed_workflow_claims().unwrap().is_empty());
    // The released item can be taken again; the other one cannot.
    assert!(matches!(
        start(&env, "first", &pr(8)),
        EventOutcome::Started { .. }
    ));
    assert_eq!(start(&env, "first", &pr(7)), EventOutcome::Duplicate);
}

#[test]
fn removing_a_workflow_forgets_its_claims() {
    let env = env();
    save(&env, workflow(""));
    let done = start(&env, "first", &pr(7));
    taken(&env, &done);
    env.store.forget_workflow_claims(ID).unwrap();
    assert!(env
        .store
        .claim_workflow_item(
            ID,
            "first",
            &crate::mono::workflows::event::event_key(&pr(7))
        )
        .unwrap());
}

#[test]
fn the_cli_removal_forgets_the_claims_through_the_signals() {
    use super::cli::{Signals, StoreHost, WorkflowHost};
    use super::global::WorkflowsDir;
    use std::sync::{Arc, Mutex};
    let dir = tempfile::tempdir().unwrap();
    let signals = Arc::new(Signals::default());
    let forgotten = Arc::new(Mutex::new(Vec::<String>::new()));
    let seen = forgotten.clone();
    signals.set_forget_claims(move |id| seen.lock().unwrap().push(id.to_string()));
    let mut host = StoreHost::new(WorkflowsDir::new(dir.path()), signals);
    host.clear_runs(ID);
    assert_eq!(*forgotten.lock().unwrap(), [ID]);
}

#[test]
fn a_test_run_is_manual_carries_the_item_and_takes_no_claim() {
    let env = env();
    save(&env, workflow(""));
    let made = start_test_run(&env.files, &tick(&idle, &|_, _| {}), ID, "first", &pr(7)).unwrap();
    let TestRun::Started(Start::Run { run, .. }) = made else {
        panic!("expected a started run")
    };
    assert_eq!(run.caller.kind, CallerKind::Manual);
    assert_eq!(run.caller.trigger_node, "first");
    assert!(run.caller.item.is_some());
    // The event node still finished with the item.
    let fired = run.nodes.iter().find(|n| n.node_id == "first").unwrap();
    assert!(fired
        .output
        .as_ref()
        .is_some_and(|o| o.get("title").is_some()));
    // No claim: the same item still fires the trigger.
    assert!(env.store.unclaimed_workflow_claims().unwrap().is_empty());
    assert!(matches!(
        start(&env, "first", &pr(7)),
        EventOutcome::Started { .. }
    ));
}

#[test]
fn a_test_run_is_allowed_while_paused_and_refused_while_it_needs_review() {
    let env = env();
    let mut w = workflow("");
    w.enabled = false;
    save(&env, w.clone());
    assert!(start_test_run(&env.files, &tick(&idle, &|_, _| {}), ID, "first", &pr(7)).is_ok());
    w.needs_review = true;
    save(&env, w);
    let refused =
        start_test_run(&env.files, &tick(&idle, &|_, _| {}), ID, "first", &pr(7)).unwrap_err();
    assert!(refused.contains("Approve"), "{refused}");
}

#[test]
fn a_test_run_over_a_busy_workflow_queues_and_a_full_queue_refuses() {
    let env = env();
    save(&env, workflow(r#","concurrency":{"max":1}"#));
    let made = start_test_run(&env.files, &tick(&|_| 1, &|_, _| {}), ID, "first", &pr(7)).unwrap();
    assert!(matches!(made, TestRun::Queued { position: 1, .. }));
    let w = workflow(r#","concurrency":{"max":1}"#);
    for n in 1..QUEUED_RUNS_MAX {
        enqueue(
            &env.files,
            queued_run(
                &w,
                RunCaller::new(CallerKind::Manual, "first"),
                n as u64 + 1,
                &format!("q-{n}"),
                n as u64,
            ),
        )
        .unwrap();
    }
    let refused =
        start_test_run(&env.files, &tick(&|_| 1, &|_, _| {}), ID, "first", &pr(8)).unwrap_err();
    assert!(refused.contains("queue is full"), "{refused}");
}

#[test]
fn the_engine_hands_the_item_to_the_firing_event_trigger_only() {
    let mut caller = RunCaller::new(CallerKind::Event, "second");
    caller.item = Some(super::ordered::Json::parse(r##"{"title":"T","ref":"#1"}"##).unwrap());
    let run = start_run(&workflow(""), caller, NOW, "run-1");
    let first = run.nodes.iter().find(|n| n.node_id == "first").unwrap();
    let second = run.nodes.iter().find(|n| n.node_id == "second").unwrap();
    assert_eq!(first.status, NodeStatus::Skipped);
    assert_eq!(second.status, NodeStatus::Done);
    assert_eq!(
        second
            .output
            .as_ref()
            .and_then(|o| o.get("title"))
            .and_then(|v| v.as_str()),
        Some("T")
    );
}

#[test]
fn matching_picks_the_first_trigger_that_fits_with_the_project_resolved() {
    use super::event::matching_trigger;
    let projects = ["/code/app".to_string()];
    let w = workflow("");
    // `first` has no repo filter, so it wins over `second`.
    assert_eq!(
        matching_trigger(&w, &pr(7), &projects).map(|n| n.id.as_str()),
        Some("first")
    );
    let mut other_repo = pr(7);
    other_repo.repo = "acme/other".into();
    assert_eq!(
        matching_trigger(&w, &other_repo, &projects).map(|n| n.id.as_str()),
        Some("first")
    );
    let mut elsewhere = pr(7);
    elsewhere.project_path = "/code/elsewhere".into();
    assert!(matching_trigger(&w, &elsewhere, &projects).is_none());
    // `{{inputs.repo}}` is read from the stored input values (D29).
    let by_input = workflow_from(
        r#"{"id":"wf-input0001","name":"n","inputs":{"repo":{"type":"project"}},"inputValues":{"repo":"/code/app"},
            "nodes":[{"id":"start","type":"event","source":"github","event":"pull_request_opened","project":"{{inputs.repo}}"}],
            "edges":[],"enabled":true,"createdAt":1}"#,
    );
    assert!(matching_trigger(&by_input, &pr(7), &projects).is_some());
    assert!(matching_trigger(&by_input, &elsewhere, &projects).is_none());
    // The workflow's own project stands in when the trigger names none.
    let mut own = workflow_from(
        r#"{"id":"wf-own000001","name":"n","inputs":{"repo":{"type":"project"}},"inputValues":{"repo":"/code/app"},
            "workspace":{"mode":"perRun","project":"{{inputs.repo}}"},
            "nodes":[{"id":"start","type":"event","source":"github","event":"pull_request_opened"}],
            "edges":[],"enabled":true,"createdAt":1}"#,
    );
    assert!(matching_trigger(&own, &pr(7), &projects).is_some());
    own.input_values = Default::default();
    assert!(matching_trigger(&own, &pr(7), &projects).is_none());
}

#[test]
fn the_trigger_item_carries_the_work_item_to_link() {
    let item = super::event::trigger_item(&pr(7));
    let linked = super::event::work_item_of(&item).unwrap();
    assert_eq!(linked.kind, "pr");
    assert_eq!(linked.repo, "acme/app");
    assert_eq!(linked.number, 7);
    assert_eq!(linked.url, "https://example.test/7");
    assert!(super::event::work_item_of(&serde_json::json!({"title": "x"})).is_none());
}

#[test]
fn the_startup_check_records_a_run_that_was_made_but_not_recorded() {
    let env = env();
    save(&env, workflow(""));
    // The run was written, then the app stopped before the claim learned its id.
    let outcome = start(&env, "first", &pr(7));
    let run = run_of(&outcome).clone();
    update_runs(&env.files, ID, |runs| {
        Ok((std::iter::once(run.clone()).chain(runs).collect(), ()))
    })
    .unwrap();
    // A claim of another trigger for another item has no run at all.
    assert!(matches!(
        start(&env, "second", &pr(8)),
        EventOutcome::Started { .. }
    ));
    let released = release_unclaimed(&env.store, &env.files).unwrap();
    assert_eq!(released.len(), 1);
    assert_eq!(released[0].trigger_node, "second");
    assert!(env.store.unclaimed_workflow_claims().unwrap().is_empty());
    // The first item stays claimed (no duplicate run); the second can be taken again.
    assert_eq!(start(&env, "first", &pr(7)), EventOutcome::Duplicate);
    assert!(matches!(
        start(&env, "second", &pr(8)),
        EventOutcome::Started { .. }
    ));
}
