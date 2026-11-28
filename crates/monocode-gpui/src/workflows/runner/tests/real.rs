//! The real host over a test shell: the `workflowSessionStart.test.ts` cases without a Mono (a
//! session starts in an app project, filed in the workflow's folder), the summary in the chat of
//! the Mono that asked for it and the run history file. No provider is ever started.
use super::*;
use crate::monos::tests::{Setup, call, chat_rows, session_count, setup, started_link};
use crate::workflows::{Probes, Workflows};
use gpui::Entity;
use monocode_core::agent::AccessMode;
use monocode_core::{
    catalog::Model,
    mono::workflows::{global::WorkflowsDir, models::Probe, store::load_runs},
    provider::Kind,
};

mod cloud;
mod extraction;

pub(super) fn request(s: &Setup) -> SessionRequest {
    SessionRequest {
        project: s.project.clone(),
        prompt: "Do it".into(),
        harness: None,
        model: None,
        effort: None,
        new_worktree: false,
        worktree_cwd: None,
        parent_session_id: None,
        folder_name: None,
        target: None,
        terminal: false,
    }
}

pub(super) fn workflows_of(s: &Setup, cx: &mut TestAppContext) -> Entity<Workflows> {
    s.window
        .update(cx, |shell, _, _| shell.workflows().expect("workflows"))
        .unwrap()
}

/// Asks the entity to start a session, which answers on `tx` (or refuses at once).
pub(super) fn ask_session(
    s: &Setup,
    cx: &mut TestAppContext,
    req: SessionRequest,
    draft: bool,
    tx: async_channel::Sender<Result<StartedSession, String>>,
) -> Result<(), String> {
    let workflows = workflows_of(s, cx);
    cx.update(|cx| workflows.update(cx, |w, cx| w.workflow_session(req, draft, tx, cx)))
}

/// Starts a session through the host path, with the prompt left in the composer so no provider
/// runs.
fn begin(
    s: &Setup,
    cx: &mut TestAppContext,
    req: SessionRequest,
) -> Result<StartedSession, String> {
    let (tx, rx) = async_channel::bounded(1);
    ask_session(s, cx, req, true, tx)?;
    cx.run_until_parked();
    rx.try_recv().expect("the start answered")
}

pub(super) fn local(started: &StartedSession) -> i64 {
    started
        .session_id
        .parse()
        .expect("an unshared store keeps the plain id")
}

fn probe_with(s: &Setup, cx: &mut TestAppContext, probe: fn(Kind) -> Probe) {
    crate::monos::tests::set_probes(s, cx, |p| *p = Probes::new().with_probe(probe));
}

fn catalog_model(id: &str, efforts: &[&str]) -> Model {
    Model {
        id: id.into(),
        name: id.into(),
        alias_target: None,
        efforts: efforts.iter().map(|e| e.to_string()).collect(),
        default_effort: None,
        fast: false,
        thinking: false,
        default: false,
        tiers: vec![],
        controls: vec![],
    }
}

fn only_test_model(kind: Kind) -> Probe {
    Probe {
        installed: true,
        models: (kind == Kind::Codex).then(|| vec![catalog_model("test", &[])]),
    }
}

fn efforts_listed(kind: Kind) -> Probe {
    Probe {
        installed: true,
        models: match kind {
            Kind::Grok => Some(vec![catalog_model("grok-4.6", &["low", "high"])]),
            Kind::Codex => Some(vec![catalog_model("test", &[])]),
            _ => None,
        },
    }
}

pub(super) fn host_of(s: &Setup, cx: &mut TestAppContext) -> Rc<dyn WorkflowHost> {
    let workflows = workflows_of(s, cx);
    cx.update(|cx| workflows.update(cx, |w, cx| w.workflow_host(cx)))
}

/// Resolves `future` on the foreground executor and keeps its answer.
pub(super) fn collect<T: 'static>(
    cx: &mut TestAppContext,
    future: Fut<T>,
) -> Rc<RefCell<Option<T>>> {
    let slot = Rc::new(RefCell::new(None));
    let out = slot.clone();
    cx.spawn(async move |_| *out.borrow_mut() = Some(future.await))
        .detach();
    slot
}

/// The folders of the project, as the sidebar lists them.
fn folders_of(s: &Setup, cx: &mut TestAppContext) -> monocode_core::session_groups::Groups {
    s.window
        .update(cx, |shell, _, _| shell.mono_groups(&s.project))
        .unwrap()
}

pub(super) fn view_of(
    s: &Setup,
    cx: &mut TestAppContext,
    id: i64,
) -> Entity<crate::conversation::Conversation> {
    s.window
        .update(cx, |shell, _, _| {
            shell.mono_session_entity(id).expect("a view")
        })
        .unwrap()
}

/// The global workflow files.
pub(super) fn global_dir(s: &Setup, cx: &mut TestAppContext) -> WorkflowsDir {
    let workflows = workflows_of(s, cx);
    cx.update(|cx| workflows.read(cx).dir().clone())
}

// --- workflowSessionStart.test.ts -------------------------------------------------------------

#[gpui::test]
fn it_starts_a_full_access_session_without_a_mono_and_files_it_in_the_workflows_folder(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let mut req = request(&s);
    req.folder_name = Some("Ship it".into());
    let first = begin(&s, cx, req.clone()).unwrap();
    let id = local(&first);
    let view = view_of(&s, cx, id);
    cx.update(|cx| {
        assert_eq!(view.read(cx).settings.access, AccessMode::FullAccess);
    });
    // The session is an ordinary one: no Mono's roster knows it.
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(monos.roster.get(&s.id).unwrap().launches.is_empty());
    });
    let groups = folders_of(&s, cx);
    assert_eq!(groups.folders.len(), 1);
    assert_eq!(groups.folders[0].name, "Ship it");
    assert_eq!(groups.folders[0].session_ids, [id]);

    // The next session, of this run or of a later one, reuses that folder.
    let second = begin(&s, cx, req.clone()).unwrap();
    let third = begin(&s, cx, req.clone()).unwrap();
    let groups = folders_of(&s, cx);
    assert_eq!(groups.folders.len(), 1);
    assert_eq!(
        groups.folders[0].session_ids,
        [id, local(&second), local(&third)]
    );

    // Another workflow has its own folder.
    req.folder_name = Some("Nightly".into());
    let other = begin(&s, cx, req).unwrap();
    let groups = folders_of(&s, cx);
    assert_eq!(groups.folders.len(), 2);
    assert_eq!(
        groups
            .folders
            .iter()
            .find(|f| f.name == "Nightly")
            .unwrap()
            .session_ids,
        [local(&other)]
    );
}

#[gpui::test]
fn a_session_without_a_folder_name_stays_unfiled(cx: &mut TestAppContext) {
    let s = setup(cx);
    begin(&s, cx, request(&s)).unwrap();
    assert!(folders_of(&s, cx).folders.is_empty());
}

#[gpui::test]
fn letting_go_of_a_session_drops_its_watch_and_cached_id(cx: &mut TestAppContext) {
    let s = setup(cx);
    let workflows = workflows_of(&s, cx);
    let kept = |cx: &mut TestAppContext| cx.update(|cx| workflows.read(cx).live.kept());
    assert_eq!(kept(cx), 0);
    let started = begin(&s, cx, request(&s)).unwrap();
    assert!(
        kept(cx) >= 2,
        "a watch and a wake-up are kept while the run uses it"
    );
    let host = host_of(&s, cx);
    host.release_session(&started.session_id);
    assert_eq!(kept(cx), 0);
    // A session can be watched again afterwards.
    let again = host.send(&started.session_id, "Again".into());
    drop(again);
    host.release_session(&started.session_id);
    assert_eq!(kept(cx), 0);
}

#[gpui::test]
fn it_nests_under_a_session_a_workflow_started_and_drops_a_parent_it_does_not_know(
    cx: &mut TestAppContext,
) {
    let s = setup(cx);
    let parent = begin(&s, cx, request(&s)).unwrap();
    let mut req = request(&s);
    req.parent_session_id = Some(parent.session_id.clone());
    let child = begin(&s, cx, req.clone()).unwrap();
    assert_eq!(started_link(&s, local(&child)), Some(local(&parent)));
    s.window
        .update(cx, |shell, _, _| {
            assert_eq!(shell.mono_started_by(local(&child)), Some(local(&parent)));
        })
        .unwrap();

    // A child cannot be a parent and a session the app forgot is no parent: the session just
    // starts at the top level.
    for parent in [child.session_id.clone(), "app-gone-dev".into()] {
        req.parent_session_id = Some(parent.clone());
        let flat = begin(&s, cx, req.clone()).unwrap_or_else(|e| panic!("{parent}: {e}"));
        assert_eq!(started_link(&s, local(&flat)), None, "{parent}");
    }
}

#[gpui::test]
fn it_starts_a_model_in_its_own_harness_when_the_node_sets_only_a_model(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut req = request(&s);
    // The app's default harness is codex; grok:grok-4.6 belongs to grok.
    req.model = Some("grok:grok-4.6".into());
    let started = begin(&s, cx, req).unwrap();
    let view = view_of(&s, cx, local(&started));
    cx.update(|cx| {
        assert_eq!(view.read(cx).summary.provider, "grok");
        assert_eq!(view.read(cx).settings.model.as_deref(), Some("grok-4.6"));
    });
}

#[gpui::test]
fn a_node_that_sets_nothing_starts_on_the_apps_default_harness(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let view = view_of(&s, cx, local(&started));
    cx.update(|cx| assert_eq!(view.read(cx).summary.provider, "codex"));
}

#[gpui::test]
fn it_names_the_model_when_the_catalog_no_longer_has_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    probe_with(&s, cx, only_test_model);
    let before = session_count(&s);
    let mut req = request(&s);
    req.harness = Some("codex".into());
    req.model = Some("codex:gone".into());
    assert_eq!(
        begin(&s, cx, req.clone()).err().as_deref(),
        Some("Model \"codex:gone\" is not available. Pick one in the step's model list.")
    );
    assert_eq!(session_count(&s), before, "no session row is left behind");
    // A model the catalog lists starts.
    req.model = Some("codex:test".into());
    assert!(begin(&s, cx, req).is_ok());
}

#[gpui::test]
fn it_names_the_effort_when_the_model_does_not_offer_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    probe_with(&s, cx, efforts_listed);
    let before = session_count(&s);
    let mut req = request(&s);
    req.harness = Some("grok".into());
    req.model = Some("grok:grok-4.6".into());
    req.effort = Some("turbo".into());
    assert_eq!(
        begin(&s, cx, req.clone()).err().as_deref(),
        Some(
            "Effort \"turbo\" is not available for this model. Pick one in the step's model list."
        )
    );
    // A model with no effort setting at all reads the same.
    let mut flat = request(&s);
    flat.harness = Some("codex".into());
    flat.model = Some("codex:test".into());
    flat.effort = Some("high".into());
    assert!(
        begin(&s, cx, flat)
            .err()
            .unwrap()
            .starts_with("Effort \"high\" is not available")
    );
    assert_eq!(session_count(&s), before);

    req.effort = Some("high".into());
    let started = begin(&s, cx, req).unwrap();
    let view = view_of(&s, cx, local(&started));
    cx.update(|cx| assert_eq!(view.read(cx).settings.effort.as_deref(), Some("high")));
}

#[gpui::test]
fn it_rejects_a_project_that_is_not_an_app_project(cx: &mut TestAppContext) {
    let s = setup(cx);
    let before = session_count(&s);
    let mut req = request(&s);
    req.project = "/code/other".into();
    let (tx, _rx) = async_channel::bounded(1);
    assert_eq!(
        ask_session(&s, cx, req, true, tx),
        Err("Not one of the app's projects".into())
    );
    assert_eq!(session_count(&s), before);
}

#[gpui::test]
fn a_followup_into_a_session_with_an_unsent_draft_is_refused(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let host = host_of(&s, cx);
    let answer = collect(cx, host.send(&started.session_id, "Again".into()));
    cx.run_until_parked();
    let outcome = answer.borrow().clone().unwrap();
    assert_eq!(outcome.status, Status::Failed);
    assert_eq!(
        outcome.error.as_deref(),
        Some("Session is busy or has an unsent draft")
    );
    let gone = collect(cx, host.send("424242", "Again".into()));
    cx.run_until_parked();
    assert_eq!(
        gone.borrow().clone().unwrap().error.as_deref(),
        Some("The session is no longer available.")
    );
}

#[gpui::test]
fn a_turn_that_was_never_taken_is_given_up_on_and_an_idle_one_has_ended(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let id = local(&started);
    let workflows = workflows_of(&s, cx);
    // Idle and armed: the turn is over.
    let over = cx.update(|cx| workflows.update(cx, |w, cx| w.workflow_settled(id, true, cx)));
    let over = collect(cx, over);
    cx.run_until_parked();
    assert_eq!(over.borrow().clone().unwrap().status, Status::Completed);
    // Idle and never seen working: the prompt was not taken, so the wait ends in a failure.
    let never = cx.update(|cx| workflows.update(cx, |w, cx| w.workflow_settled(id, false, cx)));
    let never = collect(cx, never);
    cx.run_until_parked();
    assert!(never.borrow().is_none());
    cx.executor().advance_clock(Duration::from_secs(15));
    cx.run_until_parked();
    assert_eq!(
        never.borrow().clone().unwrap().error.as_deref(),
        Some("The session did not start a turn.")
    );
}

#[gpui::test]
fn it_reads_a_sessions_directory_reply_and_waiting_state(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let host = host_of(&s, cx);
    assert_eq!(
        host.worktree_of(&started.session_id),
        Some(s.project.clone())
    );
    assert_eq!(host.final_reply(&started.session_id), "");
    assert!(!host.waiting_approval(&started.session_id));
    let info = host.session_info(&started.session_id).unwrap();
    assert_eq!(
        (info.cwd.as_str(), info.harness.as_str()),
        (s.project.as_str(), "codex")
    );
    assert_eq!(info.local_id, Some(local(&started)));
    assert!(host.session_info("424242").is_none());
    assert_eq!(host.worktree_of("424242"), None);
}

#[gpui::test]
fn a_workflow_stops_its_own_sessions(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let host = host_of(&s, cx);
    let stopped = collect(cx, host.stop(&started.session_id));
    cx.run_until_parked();
    assert_eq!(stopped.borrow().clone().unwrap(), Ok(()));
}

#[gpui::test]
fn a_project_input_resolves_to_one_of_the_apps_projects(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    assert_eq!(host.project_path(&s.project), Some(s.project.clone()));
    assert_eq!(
        host.project_path(&format!("  {}  ", s.project)),
        Some(s.project.clone())
    );
    let name = std::path::Path::new(&s.project)
        .file_name()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(host.project_path(&name), Some(s.project.clone()));
    assert_eq!(host.project_path(""), None);
    assert_eq!(host.project_path("  "), None);
    assert_eq!(host.project_path("/code/other"), None);
}

#[gpui::test]
fn the_summary_lands_in_the_monos_chat_with_a_card_for_each_session(cx: &mut TestAppContext) {
    let s = setup(cx);
    let started = begin(&s, cx, request(&s)).unwrap();
    let host = host_of(&s, cx);
    let info = host.session_info(&started.session_id).unwrap();
    let run = run_started_by(&s, RunCaller::mono(&s.id, "start"), None);
    host.post(Summary {
        run,
        title: "Ship it · completed".into(),
        text: "## Ship it · completed\n\nall good".into(),
        launches: vec![info],
    });
    cx.run_until_parked();
    let rows = chat_rows(&s);
    assert!(
        rows.iter()
            .any(|r| r == "## Ship it · completed\n\nall good"),
        "{rows:?}"
    );
    let card = rows
        .iter()
        .find(|r| r.contains("\"type\":\"session\""))
        .expect("a session card");
    assert!(card.contains(&format!("\"sessionId\":{}", local(&started))));
}

#[gpui::test]
fn the_run_history_is_written_to_the_global_file_and_replaced_in_place(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    let def = simple(json!({}));
    let mut run = start_run(
        &def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-1",
    );
    let saved = collect(cx, host.save_run(run.clone()));
    cx.run_until_parked();
    assert_eq!(saved.borrow().clone().unwrap(), Ok(()));
    run.status = RunStatus::Done;
    let again = collect(cx, host.save_run(run));
    let other = collect(
        cx,
        host.save_run(start_run(
            &def,
            RunCaller::new(CallerKind::Manual, "start"),
            2_000,
            "run-2",
        )),
    );
    cx.run_until_parked();
    assert!(again.borrow().clone().unwrap().is_ok() && other.borrow().clone().unwrap().is_ok());
    let dir = global_dir(&s, cx);
    let runs = load_runs(&dir, WF).unwrap();
    assert_eq!(
        runs.iter()
            .map(|r| (r.id.as_str(), r.status))
            .collect::<Vec<_>>(),
        [("run-2", RunStatus::Running), ("run-1", RunStatus::Done)]
    );
    // The old per-Mono folder of the Mono is not where runs go.
    assert!(!s.root.join(&s.id).join("workflow-runs").exists());
}

#[gpui::test]
fn a_command_nobody_started_cannot_be_cancelled(cx: &mut TestAppContext) {
    let s = setup(cx);
    let host = host_of(&s, cx);
    let answer = collect(cx, host.cancel_command("run-1:nonce:cmd::1"));
    cx.run_until_parked();
    assert_eq!(answer.borrow().clone().unwrap(), Ok(false));
}

// --- the app side of start/resume/stop --------------------------------------------------------

/// trigger -> dev: a session step that never gets a turn in a test (the prompt is only left in
/// the composer), so the run stays going.
fn waiting_workflow(s: &Setup) -> Workflow {
    workflow_with(
        vec![trigger(), session("dev", json!({}), json!({}))],
        vec![edge("start", "dev")],
        json!({"repo": s.project, "lib": s.project}),
    )
}

#[gpui::test]
fn the_app_starts_a_run_through_the_real_host_and_stops_it(cx: &mut TestAppContext) {
    let s = setup(cx);
    let def = waiting_workflow(&s);
    let run = start_run(
        &def,
        RunCaller::new(CallerKind::Manual, "start"),
        1_000,
        "run-1",
    );
    let workflows = workflows_of(&s, cx);
    let started = cx.update(|cx| {
        workflows.update(cx, |w, cx| {
            let first = w.start_run(run.clone(), cx);
            let second = w.start_run(run.clone(), cx);
            (first, second)
        })
    });
    assert_eq!(started, (true, false), "one run per workflow");
    cx.run_until_parked();
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(monos.workflow_signals.is_running(WF));
        assert!(monos.workflows.is_running(WF));
        assert_eq!(
            workflows.read(cx).running_workflows(),
            [("run-1".to_string(), "Ship it".to_string())]
        );
    });
    // The step's session is a plain one in the project, in the workflow's folder.
    assert_eq!(folders_of(&s, cx).folders[0].name, "Ship it");
    let stopped = cx.update(|cx| workflows.update(cx, |w, _| w.stop_run(WF)));
    assert!(stopped);
    cx.run_until_parked();
    cx.update(|cx| {
        let monos = s.manager.read(cx);
        assert!(!monos.workflows.is_running(WF));
        assert!(!monos.workflow_signals.is_running(WF));
    });
    let dir = global_dir(&s, cx);
    let runs = load_runs(&dir, WF).unwrap();
    assert_eq!(runs[0].status, RunStatus::Stopped);
    // The summary is kept on the run for its page, and no Mono was told: none asked for it.
    let summary = runs[0].summary.clone().expect("a summary on the run");
    assert_eq!(summary.title, "Ship it · stopped");
    assert_eq!(summaries_in_chat(&s), 0, "{:?}", chat_rows(&s));
}

fn run_started_by(s: &Setup, caller: RunCaller, report_to: Option<&str>) -> WorkflowRun {
    let mut def = waiting_workflow(s);
    def.report_to = report_to.map(|mono| monocode_core::mono::workflows::model::ReportTo {
        mono: Some(mono.into()),
    });
    start_run(&def, caller, 1_000, "run-1")
}

/// Runs the workflow and stops it, so its summary is delivered once.
fn run_and_stop(s: &Setup, cx: &mut TestAppContext, run: WorkflowRun) {
    let workflows = workflows_of(s, cx);
    assert!(cx.update(|cx| workflows.update(cx, |w, cx| w.start_run(run, cx))));
    cx.run_until_parked();
    assert!(cx.update(|cx| workflows.update(cx, |w, _| w.stop_run(WF))));
    cx.run_until_parked();
}

fn summaries_in_chat(s: &Setup) -> usize {
    chat_rows(s)
        .iter()
        .filter(|r| r.starts_with("## Ship it · stopped"))
        .count()
}

#[gpui::test]
fn the_summary_goes_to_the_chat_of_the_mono_that_started_the_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = run_started_by(&s, RunCaller::mono(&s.id, "start"), None);
    run_and_stop(&s, cx, run);
    assert_eq!(summaries_in_chat(&s), 1, "{:?}", chat_rows(&s));
}

#[gpui::test]
fn the_summary_goes_to_the_mono_named_in_report_to(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = run_started_by(
        &s,
        RunCaller::new(CallerKind::Schedule, "start"),
        Some(&s.id),
    );
    run_and_stop(&s, cx, run);
    assert_eq!(summaries_in_chat(&s), 1, "{:?}", chat_rows(&s));
}

#[gpui::test]
fn a_mono_that_is_both_named_and_the_caller_gets_the_summary_once(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = run_started_by(&s, RunCaller::mono(&s.id, "start"), Some(&s.id));
    run_and_stop(&s, cx, run);
    assert_eq!(summaries_in_chat(&s), 1, "{:?}", chat_rows(&s));
}

#[gpui::test]
fn a_run_nobody_asked_about_posts_to_no_chat(cx: &mut TestAppContext) {
    let s = setup(cx);
    let run = run_started_by(&s, RunCaller::new(CallerKind::Manual, "start"), None);
    run_and_stop(&s, cx, run);
    assert_eq!(summaries_in_chat(&s), 0, "{:?}", chat_rows(&s));
}

// --- commands -----------------------------------------------------------------------------------

#[cfg(unix)]
#[gpui::test]
fn a_command_waits_on_a_thread_of_its_own_so_the_executor_stays_free(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::command;
    let s = setup(cx);
    cx.executor().allow_parking();
    // `-c` with `/bin/sh`, never the user's login shell.
    let shell = std::env::var_os("SHELL");
    // SAFETY: each test runs alone: locally with `--test-threads=1`, on CI as its own
    // process under cargo-nextest, so no other test thread touches the environment.
    unsafe { std::env::set_var("SHELL", "/nonexistent/monoptah-test-shell") };
    let host = host_of(&s, cx);
    let slot = collect(
        cx,
        host.run_command(
            s.project.clone(),
            "sleep 30".into(),
            120_000,
            "cmd-own-thread".into(),
        ),
    );
    // Were the call made on the executor, this would block until the sleep ended.
    let began = std::time::Instant::now();
    cx.run_until_parked();
    assert!(began.elapsed() < Duration::from_secs(10));
    assert!(slot.borrow().is_none(), "still running");
    // Cancelled before or after the thread registered it, it ends at once either way.
    command::cancel("cmd-own-thread");
    while slot.borrow().is_none() {
        assert!(began.elapsed() < Duration::from_secs(30), "no result");
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(20));
    }
    let result = slot.borrow_mut().take().unwrap().unwrap();
    assert!(result.cancelled);
    // SAFETY: as above.
    match shell {
        Some(old) => unsafe { std::env::set_var("SHELL", old) },
        None => unsafe { std::env::remove_var("SHELL") },
    }
}

#[cfg(unix)]
#[gpui::test]
fn a_command_that_cannot_start_reports_why(cx: &mut TestAppContext) {
    let s = setup(cx);
    cx.executor().allow_parking();
    let host = host_of(&s, cx);
    let slot = collect(
        cx,
        host.run_command(
            "relative/dir".into(),
            "true".into(),
            1_000,
            "cmd-bad-cwd".into(),
        ),
    );
    let began = std::time::Instant::now();
    while slot.borrow().is_none() {
        assert!(began.elapsed() < Duration::from_secs(10));
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    let error = slot.borrow_mut().take().unwrap().unwrap_err();
    assert!(error.contains("absolute"), "{error}");
}

// --- a Mono calling `workflows.*` -------------------------------------------------------------

const OLD_ID: &str = "wf-oldmono1";

fn call_ok(
    s: &Setup,
    request: &str,
    action: &str,
    input: Value,
    cx: &mut TestAppContext,
) -> Result<Value, String> {
    call(s, request, action, input, cx).try_recv().unwrap()
}

#[gpui::test]
fn a_monos_workflow_actions_use_the_global_store_and_what_it_adds_waits_for_review(
    cx: &mut TestAppContext,
) {
    use monocode_core::mono::workflows::store::{MonoDir, load_workflows};
    let s = setup(cx);
    let definition = json!({
        "name": "Mine",
        "inputs": {"project": {"type": "project"}},
        "inputValues": {"project": s.project},
        "nodes": [
            {"id": "start", "type": "manual"},
            {"id": "dev", "type": "session", "project": "{{inputs.project}}", "prompt": "go", "checkout": "root"}
        ],
        "edges": [{"from": "start", "to": "dev"}]
    });
    let added = call_ok(
        &s,
        "add-1",
        "workflows.add",
        json!({"workflow": definition}),
        cx,
    )
    .unwrap();
    let id = added["id"].as_str().unwrap().to_string();
    let global = load_workflows(&global_dir(&s, cx)).unwrap().workflows;
    assert_eq!(global.len(), 1);
    assert!(
        !global[0].enabled && global[0].needs_review,
        "paused and awaiting review"
    );
    // The Mono's old folder is not where it went.
    assert!(
        load_workflows(&MonoDir::new(&s.root, &s.id))
            .unwrap()
            .workflows
            .is_empty()
    );
    // Another Mono's project is refused.
    let elsewhere = json!({
        "name": "Theirs",
        "inputs": {"project": {"type": "project"}},
        "inputValues": {"project": "/code/other"},
        "nodes": [
            {"id": "start", "type": "manual"},
            {"id": "dev", "type": "session", "project": "{{inputs.project}}", "prompt": "go", "checkout": "root"}
        ],
        "edges": [{"from": "start", "to": "dev"}]
    });
    let refused = call_ok(
        &s,
        "add-2",
        "workflows.add",
        json!({"workflow": elsewhere}),
        cx,
    )
    .unwrap_err();
    assert!(refused.contains("not this Mono's"), "{refused}");
    // It reads every workflow, its own included.
    let listed = call_ok(&s, "list-1", "workflows.list", json!({}), cx).unwrap();
    assert_eq!(listed["workflows"][0]["id"], json!(id));
}

#[gpui::test]
fn an_old_per_mono_workflow_id_gets_the_clear_error_and_the_mono_is_told_once(
    cx: &mut TestAppContext,
) {
    use monocode_core::mono::workflows::{
        parse::parse_workflow,
        store::{MonoDir, update_workflows},
    };
    let s = setup(cx);
    let old = parse_workflow(&Json::from(&json!({
        "id": OLD_ID, "name": "Old", "inputs": {}, "inputValues": {},
        "nodes": [{"id": "start", "type": "manual"}], "edges": [],
        "enabled": true, "createdAt": 1,
    })))
    .expect("a workflow");
    update_workflows(&MonoDir::new(&s.root, &s.id), |_| {
        Ok((vec![old.clone()], ()))
    })
    .unwrap();
    const CLEAR: &str =
        "This workflow was per Mono and is no longer used; recreate it in Workflows";
    let error = call_ok(&s, "old-1", "workflows.run", json!({"id": OLD_ID}), cx).unwrap_err();
    assert_eq!(error, CLEAR);
    // An id nobody ever had keeps the usual answer.
    let unknown = call_ok(
        &s,
        "old-2",
        "workflows.run",
        json!({"id": "wf-nothing00"}),
        cx,
    )
    .unwrap_err();
    assert!(unknown.contains("No workflow with that id"), "{unknown}");
    // The second use of the old id is refused the same way, and the notice is not repeated.
    assert_eq!(
        call_ok(&s, "old-3", "workflows.get", json!({"id": OLD_ID}), cx).unwrap_err(),
        CLEAR
    );
    cx.run_until_parked();
    let notices = chat_rows(&s)
        .iter()
        .filter(|row| row.starts_with(CLEAR))
        .count();
    assert_eq!(notices, 1, "{:?}", chat_rows(&s));
    // Nothing was written to the old file or run from it.
    assert!(!global_dir(&s, cx).root().join("workflows.json").exists());
}
