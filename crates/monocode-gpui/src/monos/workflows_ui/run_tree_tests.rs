//! The run page's "Remove worktree" and the queue on the run page and in the list: the
//! confirmation lists what the removal would lose, nothing is removed without the click, and a
//! queued run shows its place and can be cancelled.
use super::list::slots_subtitle;
use super::run_model_tests::{linear, nr, run_of};
use super::run_tree_dialog::{can_remove, risk_lines, trees_of};
use super::tests::{A, click, history, open_tab, put, settle};
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::workflows::{
    model::{CallerKind, RunCaller, RunStatus, RunWorktree, WorkflowRun},
    queue::queued_run,
    run_worktree::{AtRisk, create_run_worktree},
    store::{update_runs, update_workflows},
};
use serde_json::json;
use std::path::{Path, PathBuf};

/// A temp repository on `main` with one commit, removed with its worktrees folder when dropped.
struct Repo {
    path: PathBuf,
}

impl Repo {
    fn new() -> Self {
        let path = std::env::temp_dir()
            .join(format!("wf-run-tree-{}", monocode_core::mono::new_id()))
            .join("app");
        std::fs::create_dir_all(&path).unwrap();
        let repo = Self { path };
        repo.git(&["init", "-b", "main"]);
        repo.git(&["config", "user.email", "fixture@example.invalid"]);
        repo.git(&["config", "user.name", "Fixture"]);
        repo.git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.path.join("file"), "x").unwrap();
        repo.git(&["add", "file"]);
        repo.git(&["commit", "-m", "Initial"]);
        repo
    }
    fn git(&self, args: &[&str]) {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&self.path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success(), "{args:?}");
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        if let Some(root) = self.path.parent() {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

fn put_run(s: &Setup, run: &WorkflowRun) {
    update_runs(&s.workflows_dir(), &run.workflow_id, |runs| {
        let mut all: Vec<WorkflowRun> = runs.into_iter().filter(|r| r.id != run.id).collect();
        all.insert(0, run.clone());
        Ok((all, ()))
    })
    .unwrap();
}

fn done_run(w: &Workflow, tree: Option<RunWorktree>) -> WorkflowRun {
    let mut run = run_of(
        w,
        "done",
        json!({}),
        json!([
            nr("start", "", "done", json!({})),
            nr("dev", "", "done", json!({})),
            nr("report", "", "done", json!({}))
        ]),
    );
    run.worktree = tree;
    run
}

fn open_run(s: &Setup, visual: &mut VisualTestContext) {
    s.manager.update(visual, |m, cx| {
        m.open_workflow_route(WfRoute::Run(A.into(), "run-1".into()), cx)
    });
    visual.run_until_parked();
    settle(s, visual);
}

/// The owner found through the real-path spelling, the key a mark is written under.
fn owner_of(s: &Setup, path: &str) -> Option<String> {
    let real = Path::new(path)
        .canonicalize()
        .map_or_else(|_| path.to_string(), |p| p.to_string_lossy().into_owned());
    monocode_core::session_worktree::workflow_worktree_owner_checked(
        &s.store.lock().unwrap(),
        &real,
    )
    .unwrap()
}

fn marked(s: &Setup, path: &str) -> bool {
    monocode_core::session_worktree::is_workflow_worktree(&s.store.lock().unwrap(), path)
}

/// A repository, a run worktree of it with some work in it, and a finished run that owns it.
fn run_with_work(s: &Setup, cx: &mut TestAppContext) -> (Repo, RunWorktree) {
    let repo = Repo::new();
    let tree = create_run_worktree(&repo.path, "main", "ship-it", 1).unwrap();
    std::fs::write(Path::new(&tree.path).join("notes.txt"), "wip").unwrap();
    monocode_core::session_worktree::mark_workflow_worktree(
        &s.store.lock().unwrap(),
        &tree.path,
        "run-1",
    )
    .unwrap();
    let w = linear();
    put(s, vec![w.clone()]);
    put_run(s, &done_run(&w, Some(tree.clone())));
    open_tab(s, cx);
    (repo, tree)
}

#[test]
fn the_offer_is_only_for_a_run_that_has_a_worktree_and_is_not_using_it() {
    let w = linear();
    let tree = RunWorktree {
        path: "/wt".into(),
        branch: "wf/x-1".into(),
        base: "abc".into(),
        removed_at: None,
    };
    assert!(!can_remove(&done_run(&w, None)));
    assert!(can_remove(&done_run(&w, Some(tree.clone()))));
    for status in ["running", "queued"] {
        let mut run = done_run(&w, Some(tree.clone()));
        run.status = serde_json::from_value(json!(status)).unwrap();
        assert!(!can_remove(&run), "{status}");
    }
    // Interrupted and failed runs may be cleaned up too.
    for status in [
        RunStatus::Interrupted,
        RunStatus::Failed,
        RunStatus::Stopped,
    ] {
        let mut run = done_run(&w, Some(tree.clone()));
        run.status = status;
        assert!(can_remove(&run), "{status:?}");
    }
}

#[test]
fn the_worktrees_listed_are_the_branches_first_and_the_runs_own_last() {
    let w = linear();
    let tree = |n: &str| RunWorktree {
        path: format!("/wt/{n}"),
        branch: format!("wf/{n}"),
        base: "abc".into(),
        removed_at: None,
    };
    let mut run = done_run(&w, Some(tree("run")));
    run.branch_worktrees.insert("each#0".into(), tree("a"));
    run.branch_worktrees.insert("each#1".into(), tree("b"));
    let paths: Vec<String> = trees_of(&run).into_iter().map(|t| t.path).collect();
    assert_eq!(paths, ["/wt/a", "/wt/b", "/wt/run"]);
}

#[test]
fn the_confirmation_lists_files_and_commits_and_says_when_nothing_would_be_lost() {
    let risk = AtRisk {
        uncommitted: vec![" M src/a.rs".into(), "?? notes.txt".into()],
        more_files: 3,
        unpushed: vec!["abc1234 Add the thing".into()],
        more_commits: 0,
        unknown: false,
        digest: 0,
    };
    let lines = risk_lines(&risk);
    assert_eq!(lines[0], "Uncommitted changes (5):");
    assert!(lines.contains(&"   M src/a.rs".to_string()));
    assert!(lines.contains(&"  and 3 more".to_string()));
    assert!(lines.contains(&"Commits on no remote branch (1):".to_string()));
    assert!(lines.contains(&"  abc1234 Add the thing".to_string()));
    assert!(risk_lines(&AtRisk::default()).is_empty());
    let unknown = risk_lines(&AtRisk {
        unknown: true,
        ..Default::default()
    });
    assert!(unknown[0].contains("Could not tell"));
}

#[test]
fn the_list_says_how_many_runs_go_and_wait() {
    assert_eq!(
        slots_subtitle((2, 1)).as_deref(),
        Some("2 running / 1 queued")
    );
    assert_eq!(
        slots_subtitle((0, 3)).as_deref(),
        Some("0 running / 3 queued")
    );
    assert_eq!(
        slots_subtitle((3, 0)).as_deref(),
        Some("3 running / 0 queued")
    );
    // One run and an empty queue is what the ordinary subtitle already says.
    assert_eq!(slots_subtitle((1, 0)), None);
    assert_eq!(slots_subtitle((0, 0)), None);
}

#[gpui::test]
fn remove_worktree_asks_first_lists_the_work_and_removes_only_on_the_click(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let s = setup(cx);
    let (repo, tree) = run_with_work(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    assert!(visual.debug_bounds("wf-run-worktree").is_some());
    click(visual, "wf-run-remove-tree");
    // Git is asked what the removal would lose, off the UI thread.
    visual.run_until_parked();
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-run-tree").is_some());
    let listed = s.manager.read_with(visual, |m, _| {
        let dialog = m.wf.tree.as_ref().expect("the confirmation");
        assert_eq!(dialog.trees.len(), 1);
        dialog
            .risks
            .as_ref()
            .map(|risks| risk_lines(&risks[0]))
            .expect("checked")
    });
    assert!(listed.iter().any(|l| l.contains("notes.txt")), "{listed:?}");
    // Nothing is removed by looking, or by cancelling.
    assert!(Path::new(&tree.path).join("notes.txt").is_file());
    click(visual, "wf-run-tree-cancel");
    assert!(visual.debug_bounds("wf-run-tree").is_none());
    assert!(Path::new(&tree.path).is_dir());
    assert!(marked(&s, &tree.path));
    assert!(history(&s, A)[0].worktree.is_some());

    assert!(
        owner_of(&s, &tree.path).is_some(),
        "marked before the removal"
    );
    // The click on Remove worktree takes the folder, keeps the branch, and forgets the record.
    click(visual, "wf-run-remove-tree");
    visual.run_until_parked();
    settle(&s, visual);
    click(visual, "wf-run-tree-remove");
    visual.run_until_parked();
    assert!(!Path::new(&tree.path).exists());
    repo.git(&["show-ref", "--verify", "refs/heads/wf/ship-it-1"]);
    // The record keeps the branch name and says when the folder went (D47).
    let kept = history(&s, A)[0]
        .worktree
        .clone()
        .expect("the record stays");
    assert_eq!(kept.branch, "wf/ship-it-1");
    assert!(kept.removed_at.is_some());
    assert!(!marked(&s, &tree.path), "the ownership mark goes with it");
    assert_eq!(owner_of(&s, &tree.path), None);
    assert!(s.manager.read_with(visual, |m, _| m.wf.tree.is_none()));
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-run-remove-tree").is_none());
    // The page still names the worktree: removed, with its branch.
    assert!(visual.debug_bounds("wf-run-worktree").is_some());
    assert_eq!(
        s.manager.read_with(visual, |m, _| m
            .wf
            .run
            .run
            .as_ref()
            .and_then(|r| r.worktree.as_ref())
            .map(|t| t.removed_at.is_some())),
        Some(true)
    );
}

#[gpui::test]
fn work_added_after_the_dialog_opened_stops_the_removal_and_the_list_is_looked_at_again(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let s = setup(cx);
    let (_repo, tree) = run_with_work(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    click(visual, "wf-run-remove-tree");
    visual.run_until_parked();
    settle(&s, visual);
    // The list was made; then more work lands in the worktree.
    std::fs::write(Path::new(&tree.path).join("later.txt"), "more").unwrap();
    click(visual, "wf-run-tree-remove");
    visual.run_until_parked();
    settle(&s, visual);
    // Nothing was removed, the dialog says why and lists the new file.
    assert!(Path::new(&tree.path).join("later.txt").is_file());
    assert!(
        owner_of(&s, &tree.path).is_some(),
        "a refused removal keeps the mark"
    );
    assert!(
        history(&s, A)[0]
            .worktree
            .as_ref()
            .unwrap()
            .removed_at
            .is_none()
    );
    let (notice, listed) = s.manager.read_with(visual, |m, _| {
        let dialog = m.wf.tree.as_ref().expect("still open");
        (
            dialog.notice.clone().unwrap_or_default(),
            dialog.risks.as_ref().map(|r| risk_lines(&r[0])),
        )
    });
    assert!(notice.contains("changed since"), "{notice}");
    assert!(
        listed
            .expect("looked at again")
            .iter()
            .any(|l| l.contains("later.txt")),
        "the new file is on the list the user confirms next"
    );
    // Confirming what is listed now removes it.
    click(visual, "wf-run-tree-remove");
    visual.run_until_parked();
    assert!(!Path::new(&tree.path).exists());
}

#[gpui::test]
fn a_run_the_runner_still_holds_or_claimed_keeps_its_worktree_on_remove(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let s = setup(cx);
    let (_repo, tree) = run_with_work(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    click(visual, "wf-run-remove-tree");
    visual.run_until_parked();
    settle(&s, visual);
    // A start or a resume was claimed for the run meanwhile.
    let signals = s
        .manager
        .read_with(visual, |m, _| m.workflow_signals.clone());
    signals.claim_run(A, "run-1");
    click(visual, "wf-run-tree-remove");
    visual.run_until_parked();
    let notice = s
        .manager
        .read_with(visual, |m, _| {
            m.wf.tree.as_ref().and_then(|d| d.notice.clone())
        })
        .unwrap_or_default();
    assert!(notice.contains("about to start or resume"), "{notice}");
    assert!(Path::new(&tree.path).join("notes.txt").is_file());
    signals.release_run(A, "run-1");
    // A resume asked for, and not yet claimed, holds it too.
    update_workflows(&s.workflows_dir(), |mut list| {
        list[0].resume_run_id = Some("run-1".into());
        Ok((list, ()))
    })
    .unwrap();
    click(visual, "wf-run-tree-remove");
    visual.run_until_parked();
    let notice = s
        .manager
        .read_with(visual, |m, _| {
            m.wf.tree.as_ref().and_then(|d| d.notice.clone())
        })
        .unwrap_or_default();
    assert!(notice.contains("asked to resume"), "{notice}");
    assert!(Path::new(&tree.path).join("notes.txt").is_file());
    assert!(
        history(&s, A)[0]
            .worktree
            .as_ref()
            .unwrap()
            .removed_at
            .is_none()
    );
}

#[gpui::test]
fn the_confirmation_says_ignored_files_go_too_and_uses_the_shared_modal(cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let s = setup(cx);
    let (_repo, _tree) = run_with_work(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    click(visual, "wf-run-remove-tree");
    visual.run_until_parked();
    settle(&s, visual);
    // A card of the shared modal: a dialog with the modal's own buttons.
    assert!(visual.debug_bounds("wf-run-tree").is_some());
    assert!(visual.debug_bounds("wf-run-tree-cancel").is_some());
    assert!(super::run_tree_dialog::TREE_NOTE.contains("ignores"));
    assert!(super::run_tree_dialog::TREE_NOTE.contains(".env"));
    // A press on the scrim, outside the card, dismisses it.
    visual.simulate_click(gpui::point(px(4.), px(4.)), Default::default());
    visual.run_until_parked();
    assert!(visual.debug_bounds("wf-run-tree").is_none());
}

#[gpui::test]
fn the_sessions_that_worked_in_a_removed_worktree_keep_their_history_and_move_to_the_main_checkout(
    cx: &mut TestAppContext,
) {
    cx.executor().allow_parking();
    let s = setup(cx);
    let (repo, tree) = run_with_work(&s, cx);
    // A conversation of the run that worked in its worktree.
    let id = {
        let store = s.store.lock().unwrap();
        let session = store
            .create("claude", &tree.path)
            .expect("a session in the worktree");
        store
            .save_agent_settings(
                session.id,
                &monocode_core::agent::Settings {
                    worktree_cwd: Some(tree.path.clone()),
                    ..Default::default()
                },
            )
            .unwrap();
        session.id
    };
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    click(visual, "wf-run-remove-tree");
    visual.run_until_parked();
    settle(&s, visual);
    click(visual, "wf-run-tree-remove");
    visual.run_until_parked();
    assert!(!Path::new(&tree.path).exists());
    let store = s.store.lock().unwrap();
    let kept = store
        .sessions()
        .unwrap()
        .into_iter()
        .find(|session| session.id == id)
        .expect("the session stays");
    let settings = store.agent_settings(id).unwrap().unwrap();
    assert!(settings.worktree_removed, "it knows its folder is gone");
    assert_eq!(
        Path::new(&kept.cwd).canonicalize().unwrap(),
        repo.path.canonicalize().unwrap(),
        "it is attached to the main checkout again"
    );
}

#[gpui::test]
fn a_run_that_is_still_going_has_no_remove_button(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = linear();
    put(&s, vec![w.clone()]);
    let mut run = done_run(
        &w,
        Some(RunWorktree {
            path: "/wt".into(),
            branch: "wf/x-1".into(),
            base: "abc".into(),
            removed_at: None,
        }),
    );
    run.status = RunStatus::Running;
    put_run(&s, &run);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    open_run(&s, visual);
    // The worktree is named, but only a run that is over offers to remove it.
    assert!(visual.debug_bounds("wf-run-worktree").is_some());
    assert!(visual.debug_bounds("wf-run-remove-tree").is_none());
}

#[gpui::test]
fn a_queued_run_shows_its_place_and_cancel_ends_it_without_starting(cx: &mut TestAppContext) {
    let s = setup(cx);
    let mut w = linear();
    // What queued the runs claimed the workflow.
    w.last_run_id = Some("run-0".into());
    put(&s, vec![w.clone()]);
    let caller = RunCaller::new(CallerKind::Manual, "start");
    let mut first = queued_run(&w, caller.clone(), 1, "run-0", 5);
    first.workflow_id = A.into();
    let mut second = queued_run(&w, caller, 2, "run-1", 6);
    second.workflow_id = A.into();
    put_run(&s, &first);
    put_run(&s, &second);
    open_tab(&s, cx);
    let visual = &mut VisualTestContext::from_window(s.window.into(), cx);
    visual.run_until_parked();
    // The list counts what waits.
    assert_eq!(
        s.manager.read_with(visual, |m, _| m.workflow_slots(A)),
        (0, 2)
    );
    open_run(&s, visual);
    assert!(visual.debug_bounds("wf-run-queue").is_some());
    assert!(visual.debug_bounds("wf-run-stop").is_none());
    click(visual, "wf-run-cancel");
    visual.run_until_parked();
    let runs = history(&s, A);
    let status = |id: &str| runs.iter().find(|r| r.id == id).unwrap().status;
    assert_eq!(status("run-1"), RunStatus::Stopped);
    assert_eq!(status("run-0"), RunStatus::Queued);
    assert!(
        !s.manager
            .read_with(visual, |m, _| m.workflows.is_running(A))
    );
}

#[gpui::test]
fn removing_a_workflow_is_refused_while_runs_wait(cx: &mut TestAppContext) {
    use monocode_core::mono::workflows::cli::{Signals, StoreHost};
    let s = setup(cx);
    let mut w = linear();
    w.last_run_id = Some("run-0".into());
    put(&s, vec![w.clone()]);
    let mut waiting = queued_run(
        &w,
        RunCaller::new(CallerKind::Manual, "start"),
        1,
        "run-1",
        5,
    );
    waiting.workflow_id = A.into();
    put_run(&s, &waiting);
    let mut host = StoreHost::new(s.workflows_dir(), std::sync::Arc::new(Signals::default()));
    let notice = super::actions::remove(&mut host, A).expect("refused");
    assert!(notice.contains("1 queued run"), "{notice}");
    assert_eq!(super::tests::saved(&s).len(), 1);
    assert_eq!(history(&s, A).len(), 1);
}
