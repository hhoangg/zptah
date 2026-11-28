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

/// Why a run worktree is not made for a project on another machine (D49).
pub const REMOTE_WORKTREE: &str = "A run worktree is made on this computer only, so a project on a remote machine cannot have one: use workspace mode none for it";

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
    git::git(
        cwd,
        &["rev-parse", "--verify", "--end-of-options", "HEAD^{commit}"],
    )
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
    if crate::files::remote::is_remote(project) {
        return Err(REMOTE_WORKTREE.into());
    }
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
                    removed_at: None,
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
    // Git runs on this computer: a path on another machine is never handed to it.
    if crate::files::remote::is_remote(cwd) {
        return Err(REMOTE_WORKTREE.into());
    }
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
                    removed_at: None,
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
    /// Commits no remote branch and no other local branch has (`<short sha> <subject>`), at
    /// most `LISTED`.
    pub unpushed: Vec<String>,
    /// How many commits there are beyond the listed ones.
    pub more_commits: usize,
    /// The folder is gone or Git could not tell: nothing is known about the work in it.
    pub unknown: bool,
    /// A fingerprint of everything above, listed or not, and of the files' sizes and times: two
    /// looks that differ in it saw different work, even when the lists read the same.
    pub digest: u64,
}

/// How many files and commits the confirmation lists.
pub const LISTED: usize = 12;

/// Why a removal was refused because the work changed after the user looked.
pub const CHANGED_SINCE_CONFIRM: &str =
    "This worktree changed since the list was shown. Review what it holds and confirm again.";

impl AtRisk {
    pub fn is_clear(&self) -> bool {
        !self.unknown && self.uncommitted.is_empty() && self.unpushed.is_empty()
    }
}

/// The uncommitted and unpushed work in `path`. Nothing is changed.
pub fn at_risk(path: &Path) -> AtRisk {
    use std::hash::{Hash, Hasher};
    let mut risk = AtRisk::default();
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if crate::files::remote::is_remote(path) || !path.is_dir() {
        risk.unknown = true;
        return risk;
    }
    match git::git(path, &["status", "--porcelain", "--untracked-files=normal"]) {
        Ok(out) => {
            let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
            risk.more_files = lines.len().saturating_sub(LISTED);
            risk.uncommitted = lines.iter().take(LISTED).map(|l| l.to_string()).collect();
            for line in &lines {
                line.hash(&mut hasher);
                // A file edited again after the list was shown keeps its status line.
                let name = line.get(3..).unwrap_or("").trim().trim_matches('"');
                if let Ok(meta) = std::fs::metadata(path.join(name)) {
                    meta.len().hash(&mut hasher);
                    if let Ok(time) = meta.modified() {
                        time.hash(&mut hasher);
                    }
                }
            }
        }
        Err(_) => risk.unknown = true,
    }
    // Work counts as unpushed when no remote branch has it and no other local branch has it
    // either: the history the run started from is on its base branch, not at risk.
    let branch = git::git(path, &["branch", "--show-current"])
        .map(|b| b.trim().to_string())
        .unwrap_or_default();
    let exclude = format!("--exclude={branch}");
    let mut args = vec!["log", "--format=%h %s", "HEAD", "--not", "--remotes"];
    if !branch.is_empty() {
        args.push(&exclude);
    }
    args.push("--branches");
    match git::git(path, &args) {
        Ok(out) => {
            let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
            risk.more_commits = lines.len().saturating_sub(LISTED);
            risk.unpushed = lines.iter().take(LISTED).map(|l| l.to_string()).collect();
            lines.hash(&mut hasher);
        }
        Err(_) => risk.unknown = true,
    }
    risk.digest = hasher.finish();
    risk
}

/// Removes a worktree the run made, after the user confirmed what `confirmed` listed. The work
/// is looked at again right before: when it is not what the user saw, nothing is removed. A
/// worktree that held nothing at risk is removed without force, so Git itself refuses one that
/// gained work meanwhile. Its branch and commits stay. Refuses the main checkout, a remote path
/// and anything that is not a linked worktree of the repository. Returns the main working copy
/// of the repository (so the sessions that used the worktree can be moved there), or `None` when
/// the folder was gone already.
pub fn remove_worktree(path: &Path, confirmed: &AtRisk) -> Result<Option<String>, String> {
    if crate::files::remote::is_remote(path) {
        return Err("A worktree on a remote machine is not removed from here".into());
    }
    if !path.exists() {
        return Ok(None);
    }
    if at_risk(path) != *confirmed {
        return Err(CHANGED_SINCE_CONFIRM.into());
    }
    let snapshot = git::snapshot(path)?;
    let main_path = snapshot
        .worktrees
        .first()
        .ok_or("No main working copy found")?
        .path
        .clone();
    let main = Path::new(&main_path);
    if confirmed.is_clear() {
        crate::worktree_management::remove_clean(path)?;
    } else {
        crate::worktree_management::remove(main, path)?;
    }
    let _ = git::git(main, &["worktree", "prune"]);
    Ok(Some(main_path))
}

/// The repository a run worktree was made from. The app keeps worktrees in `<repo>-worktrees`,
/// so the repository is the sibling without that suffix.
fn repo_of(tree_path: &str) -> Option<std::path::PathBuf> {
    let folder = Path::new(tree_path).parent()?;
    let name = folder.file_name()?.to_str()?.strip_suffix("-worktrees")?;
    let repo = folder.with_file_name(name);
    repo.is_dir().then_some(repo)
}

/// Whether `branch` exists in the repository `repo`.
fn branch_exists(repo: &Path, branch: &str) -> bool {
    git::git(
        repo,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{branch}"),
        ],
    )
    .is_ok()
}

/// Why a worktree the user removed cannot be made again, or `None` when it can: its branch is
/// what the run's work is kept on, so a branch that is gone ends the run's workspace.
pub fn restore_problem(tree: &RunWorktree) -> Option<String> {
    if !tree.is_removed() {
        return None;
    }
    let Some(repo) = repo_of(&tree.path) else {
        return Some(format!(
            "The repository of the removed worktree {} cannot be found, so the run cannot continue.",
            tree.path
        ));
    };
    if !branch_exists(&repo, &tree.branch) {
        return Some(format!(
            "The branch {} of the removed worktree no longer exists, so the run cannot continue. Its work cannot be recovered.",
            tree.branch
        ));
    }
    None
}

/// Why a stored run cannot be resumed because of the worktrees the user removed from it.
pub fn resume_problem(run: &super::model::WorkflowRun) -> Option<String> {
    run.worktree
        .iter()
        .chain(run.branch_worktrees.values())
        .find_map(restore_problem)
}

/// Makes a removed worktree again from the branch the removal kept: the folder returns where it
/// was, with the committed work of the run. Uncommitted work and ignored files were deleted with
/// it. Err says why it cannot.
pub fn restore_worktree(tree: &RunWorktree) -> Result<RunWorktree, String> {
    if let Some(problem) = restore_problem(tree) {
        return Err(problem);
    }
    let repo = repo_of(&tree.path).ok_or("The repository of the worktree cannot be found")?;
    let made = git::create_project_worktree(&repo, &tree.branch, None, true)?;
    if crate::files::path_key(&made.path) != crate::files::path_key(&tree.path) {
        // Not where the run's steps expect it: undo rather than leave a second copy around.
        let _ = remove_worktree(Path::new(&made.path), &at_risk(Path::new(&made.path)));
        return Err(format!(
            "The worktree could not be made again at {}",
            tree.path
        ));
    }
    Ok(RunWorktree {
        removed_at: None,
        ..tree.clone()
    })
}

/// The worktrees of a run that are still on disk (not removed): its own and its For each
/// branches', as `(path, branch)`.
pub fn kept_worktrees(run: &super::model::WorkflowRun) -> Vec<(String, String)> {
    run.worktree
        .iter()
        .chain(run.branch_worktrees.values())
        .filter(|t| !t.is_removed())
        .map(|t| (t.path.clone(), t.branch.clone()))
        .collect()
}
