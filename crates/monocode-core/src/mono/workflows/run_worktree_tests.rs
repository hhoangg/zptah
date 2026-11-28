//! The run worktree on temporary git repositories: the branch name, a collision, For each
//! branches forked from the run's HEAD, and what the removal would lose.
use super::model::RunWorktree;
use super::run_worktree::*;
use crate::git::git;
use std::path::Path;

/// A repository with one commit on `main`, inside its own temp dir so its `-worktrees` sibling
/// is cleaned up with it.
struct Repo {
    root: tempfile::TempDir,
}

impl Repo {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("app");
        std::fs::create_dir_all(&repo).unwrap();
        let this = Self { root };
        this.git(&["init", "-b", "main"]);
        this.git(&["config", "user.email", "fixture@example.invalid"]);
        this.git(&["config", "user.name", "Fixture"]);
        this.git(&["config", "commit.gpgsign", "false"]);
        std::fs::write(repo.join("file"), "one").unwrap();
        this.git(&["add", "file"]);
        this.git(&["commit", "-m", "Initial"]);
        this
    }
    fn path(&self) -> std::path::PathBuf {
        self.root.path().join("app")
    }
    fn git(&self, args: &[&str]) -> String {
        git(&self.path(), args).unwrap()
    }
}

fn commit_in(path: &Path, name: &str) {
    std::fs::write(path.join(name), name).unwrap();
    git(path, &["add", name]).unwrap();
    git(path, &["commit", "-m", name]).unwrap();
}

#[test]
fn the_slug_is_lowercase_words_joined_by_hyphens() {
    assert_eq!(slug_of("Ticket Review!"), "ticket-review");
    assert_eq!(slug_of("  Dev / QA  loop "), "dev-qa-loop");
    assert_eq!(slug_of("!!!"), "workflow");
    assert!(slug_of(&"word ".repeat(20)).len() <= 30);
    assert!(!slug_of(&"word ".repeat(20)).ends_with('-'));
}

#[test]
fn the_branch_is_wf_slug_and_run_number_and_a_retry_adds_a_counter() {
    assert_eq!(run_branch("ticket-review", 4, 0), "wf/ticket-review-4");
    assert_eq!(run_branch("ticket-review", 4, 1), "wf/ticket-review-4-2");
    assert_eq!(run_branch("ticket-review", 4, 2), "wf/ticket-review-4-3");
    assert_eq!(branch_worktree_branch("wf/x-1", "each", 2), "wf/x-1-each-2");
    assert_eq!(branch_key("each", 2), "each#2");
}

#[test]
fn a_run_worktree_is_made_on_its_branch_at_the_base_branch() {
    let repo = Repo::new();
    let main_head = repo.git(&["rev-parse", "HEAD"]).trim().to_string();
    // A later commit on another branch must not leak into a worktree based on main.
    repo.git(&["switch", "-c", "other"]);
    commit_in(&repo.path(), "later");
    let tree = create_run_worktree(&repo.path(), "main", "nightly", 3).unwrap();
    assert_eq!(tree.branch, "wf/nightly-3");
    assert_eq!(tree.base, main_head);
    assert!(Path::new(&tree.path).join("file").is_file());
    assert!(!Path::new(&tree.path).join("later").exists());
    let branch = git(Path::new(&tree.path), &["branch", "--show-current"]).unwrap();
    assert_eq!(branch.trim(), "wf/nightly-3");
    // It sits where the app keeps worktrees.
    assert!(tree.path.contains("app-worktrees"));
}

#[test]
fn a_taken_branch_name_is_retried_with_a_counter() {
    let repo = Repo::new();
    let first = create_run_worktree(&repo.path(), "main", "nightly", 1).unwrap();
    // The same run number again (a re-created workflow): the branch and the folder are taken.
    let second = create_run_worktree(&repo.path(), "main", "nightly", 1).unwrap();
    assert_eq!(first.branch, "wf/nightly-1");
    assert_eq!(second.branch, "wf/nightly-1-2");
    assert_ne!(first.path, second.path);
    let third = create_run_worktree(&repo.path(), "main", "nightly", 1).unwrap();
    assert_eq!(third.branch, "wf/nightly-1-3");
}

#[test]
fn a_missing_base_branch_says_which_one() {
    let repo = Repo::new();
    let error = create_run_worktree(&repo.path(), "release", "nightly", 1).unwrap_err();
    assert!(error.contains("\"release\""), "{error}");
    assert!(error.contains("baseBranch"), "{error}");
}

#[test]
fn a_for_each_branch_is_forked_from_the_run_worktrees_current_head() {
    let repo = Repo::new();
    let run = create_run_worktree(&repo.path(), "main", "nightly", 1).unwrap();
    // A step before the loop committed in the run worktree.
    commit_in(Path::new(&run.path), "before-loop");
    let run_head = git(Path::new(&run.path), &["rev-parse", "HEAD"])
        .unwrap()
        .trim()
        .to_string();
    let branch = fork_branch_worktree(&run, "each", 0).unwrap();
    assert_eq!(branch.branch, "wf/nightly-1-each-0");
    assert_eq!(branch.base, run_head);
    assert!(Path::new(&branch.path).join("before-loop").is_file());
    assert_ne!(branch.path, run.path);
    // Another index has its own.
    let second = fork_branch_worktree(&run, "each", 1).unwrap();
    assert_ne!(second.path, branch.path);
    assert_eq!(second.branch, "wf/nightly-1-each-1");
}

#[test]
fn what_a_removal_would_lose_is_listed() {
    let repo = Repo::new();
    let tree = create_run_worktree(&repo.path(), "main", "nightly", 1).unwrap();
    let path = Path::new(&tree.path);
    assert!(at_risk(path).is_clear());
    std::fs::write(path.join("scratch"), "wip").unwrap();
    commit_in(path, "unpushed");
    std::fs::write(path.join("dirty"), "more").unwrap();
    let risk = at_risk(path);
    assert!(!risk.is_clear());
    assert!(
        risk.uncommitted.iter().any(|l| l.contains("dirty")),
        "{risk:?}"
    );
    assert!(
        risk.unpushed.iter().any(|l| l.contains("unpushed")),
        "{risk:?}"
    );
    // A folder that is gone is not known to be clean.
    let gone = at_risk(&repo.root.path().join("nowhere"));
    assert!(gone.unknown && !gone.is_clear());
}

#[test]
fn removing_a_run_worktree_keeps_its_branch_and_never_touches_the_main_checkout() {
    let repo = Repo::new();
    let tree = create_run_worktree(&repo.path(), "main", "nightly", 1).unwrap();
    remove_worktree(Path::new(&tree.path)).unwrap();
    assert!(!Path::new(&tree.path).exists());
    let branches = repo.git(&["branch", "--list", "wf/nightly-1"]);
    assert!(branches.contains("wf/nightly-1"));
    // The main checkout is refused.
    assert!(remove_worktree(&repo.path()).is_err());
    assert!(repo.path().join("file").is_file());
    // A worktree that is already gone is not an error.
    remove_worktree(Path::new(&tree.path)).unwrap();
}

#[test]
fn the_record_of_a_worktree_round_trips() {
    let tree = RunWorktree {
        path: "/p".into(),
        branch: "wf/x-1".into(),
        base: "abc".into(),
        removed_at: None,
    };
    let json = serde_json::to_string(&tree).unwrap();
    assert_eq!(json, r#"{"path":"/p","branch":"wf/x-1","base":"abc"}"#);
    assert_eq!(serde_json::from_str::<RunWorktree>(&json).unwrap(), tree);
}
