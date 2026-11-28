//! The worktree a workflow run works in (`workspace.mode = perRun`), and the one each For each
//! branch gets. They are made with the app's own worktree helpers (`git::create_project_worktree`,
//! in `<project>-worktrees`) on branches named `wf/<slug>-<run number>`.
//!
//! The run owns its worktree: no session owns it, so the per-session removal never takes it (see
//! `session_worktree::mark_workflow_worktree`), and the app never deletes it on its own. It stays
//! after the run, because it may hold unmerged work, until the user removes it from the run's page.
use super::model::RunWorktree;
use crate::git;
use std::path::Path;

/// How many branch names are tried before giving up on a collision.
const NAME_ATTEMPTS: u32 = 20;
/// The longest workflow slug used in a branch name.
const SLUG_MAX: usize = 30;

/// The part of a workflow's name that goes into branch names: lowercase letters and digits
/// joined by single hyphens.
pub fn slug_of(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug: String = slug.chars().take(SLUG_MAX).collect();
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() {
        "workflow".to_string()
    } else {
        slug.to_string()
    }
}

/// `wf/<slug>-<number>`, and `wf/<slug>-<number>-<attempt + 1>` for a retry after a collision.
pub fn run_branch(slug: &str, number: u64, attempt: u32) -> String {
    if attempt == 0 {
        format!("wf/{slug}-{number}")
    } else {
        format!("wf/{slug}-{number}-{}", attempt + 1)
    }
}

/// The branch of a For each branch's worktree: the run's branch, the group and the index.
pub fn branch_worktree_branch(run_branch: &str, group: &str, index: usize) -> String {
    format!("{run_branch}-{group}-{index}")
}

/// The key a For each branch's worktree is kept under in the run: the live branch key.
pub fn branch_key(group: &str, index: usize) -> String {
    format!("{group}#{index}")
}

fn head_of(cwd: &Path) -> Result<String, String> {
    git::git(cwd, &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"])
        .map(|sha| sha.trim().to_string())
}

/// The commit of `base_branch` in `project`: the local branch, else the one of the same name on
/// origin.
fn base_commit(project: &Path, base_branch: &str) -> Result<String, String> {
    if crate::files::remote::is_remote(project) {
        // The remote machine resolves it when it makes the worktree.
        return Ok(base_branch.to_string());
    }
    for reference in [base_branch.to_string(), format!("origin/{base_branch}")] {
        if let Ok(sha) = git::git(
            project,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{reference}^{{commit}}"),
            ],
        ) {
            return Ok(sha.trim().to_string());
        }
    }
    Err(format!(
        "The base branch \"{base_branch}\" does not exist in this project. Set workspace.baseBranch to a branch that does."
    ))
}

/// Whether Git refused only because of a name that is taken, so another name may work.
fn is_collision(error: &str) -> bool {
    error.contains("already has a working copy")
        || error.contains("already exists")
        || error.contains("already used by worktree")
        || error.contains("already checked out")
}

/// Makes the run's worktree: a new branch `wf/<slug>-<number>` (a number is added when the name is
/// taken) at `base_branch`, in the folder the app keeps worktrees in.
pub fn create_run_worktree(
    project: &Path,
    base_branch: &str,
    slug: &str,
    number: u64,
) -> Result<RunWorktree, String> {
    let base = base_commit(project, base_branch)?;
    let mut last = String::new();
    for attempt in 0..NAME_ATTEMPTS {
        let branch = run_branch(slug, number, attempt);
        match git::create_project_worktree(project, &branch, Some(&base), false) {
            Ok(tree) => {
                return Ok(RunWorktree {
                    path: tree.path,
                    branch,
                    base,
                })
            }
            Err(error) if is_collision(&error) => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(format!(
        "Could not find a free branch name for the run worktree: {last}"
    ))
}

/// Makes the worktree of one For each branch, forked from the run worktree's current HEAD (so it
/// sees what the steps before the loop did).
pub fn fork_branch_worktree(
    run: &RunWorktree,
    group: &str,
    index: usize,
) -> Result<RunWorktree, String> {
    let cwd = Path::new(&run.path);
    let base = head_of(cwd)?;
    let wanted = branch_worktree_branch(&run.branch, group, index);
    let mut last = String::new();
    for attempt in 0..NAME_ATTEMPTS {
        let branch = if attempt == 0 {
            wanted.clone()
        } else {
            format!("{wanted}-{}", attempt + 1)
        };
        match git::create_project_worktree(cwd, &branch, Some(&base), false) {
            Ok(tree) => {
                return Ok(RunWorktree {
                    path: tree.path,
                    branch,
                    base,
                })
            }
            Err(error) if is_collision(&error) => last = error,
            Err(error) => return Err(error),
        }
    }
    Err(format!(
        "Could not find a free branch name for the branch worktree: {last}"
    ))
}

/// What would be lost by removing a worktree, for the confirmation the user sees first.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AtRisk {
    /// Changed or untracked files (`git status --porcelain` lines), at most `LISTED`.
    pub uncommitted: Vec<String>,
    /// How many files there are beyond the listed ones.
    pub more_files: usize,
    /// Commits on no remote branch (`<short sha> <subject>`), at most `LISTED`.
    pub unpushed: Vec<String>,
    /// How many commits there are beyond the listed ones.
    pub more_commits: usize,
    /// The folder is gone or Git could not tell: nothing is known about the work in it.
    pub unknown: bool,
}

/// How many files and commits the confirmation lists.
pub const LISTED: usize = 12;

impl AtRisk {
    pub fn is_clear(&self) -> bool {
        !self.unknown && self.uncommitted.is_empty() && self.unpushed.is_empty()
    }
}

/// The uncommitted and unpushed work in `path`. Nothing is changed.
pub fn at_risk(path: &Path) -> AtRisk {
    let mut risk = AtRisk::default();
    if !path.is_dir() {
        risk.unknown = true;
        return risk;
    }
    match git::git(path, &["status", "--porcelain", "--untracked-files=normal"]) {
        Ok(out) => {
            let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
            risk.more_files = lines.len().saturating_sub(LISTED);
            risk.uncommitted = lines.iter().take(LISTED).map(|l| l.to_string()).collect();
        }
        Err(_) => risk.unknown = true,
    }
    match git::git(
        path,
        &["log", "--format=%h %s", "HEAD", "--not", "--remotes"],
    ) {
        Ok(out) => {
            let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
            risk.more_commits = lines.len().saturating_sub(LISTED);
            risk.unpushed = lines.iter().take(LISTED).map(|l| l.to_string()).collect();
        }
        Err(_) => risk.unknown = true,
    }
    risk
}

/// Removes a worktree the run made, after the user confirmed what `at_risk` listed. Its branch
/// and commits stay. Refuses the main checkout and anything that is not a linked worktree of the
/// repository.
pub fn remove_worktree(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let snapshot = git::snapshot(path)?;
    let main = snapshot
        .worktrees
        .first()
        .ok_or("No main working copy found")?;
    let main = Path::new(&main.path);
    crate::worktree_management::remove(main, path)?;
    let _ = git::git(main, &["worktree", "prune"]);
    Ok(())
}
