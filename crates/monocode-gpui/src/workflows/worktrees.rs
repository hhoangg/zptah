//! Removing the worktrees a run made, on the user's click ("Remove worktree" on the run page).
//! Nothing else ever removes them: a session's own removal skips a worktree a run owns, and the
//! app never cleans one up on its own.
use super::Workflows;
use crate::runtime::locked;
use gpui::{Context, Task};
use monocode_core::mono::workflows::{
    model::RunStatus,
    run_worktree::remove_worktree,
    store::{load_runs, update_runs},
};
use std::path::Path;

impl Workflows {
    /// Removes the worktree of the run and the worktrees of its For each branches, forgets them in
    /// the run record and drops their ownership mark. The branches and their commits stay. The
    /// caller showed the user what would be lost and got the click. A run that is going or
    /// queued keeps its worktrees.
    pub(crate) fn remove_run_worktrees(
        &mut self,
        workflow_id: String,
        run_id: String,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let (dir, store) = (self.dir.clone(), self.store.clone());
        cx.background_executor().spawn(async move {
            let run = load_runs(&dir, &workflow_id)?
                .into_iter()
                .find(|run| run.id == run_id)
                .ok_or("This run is no longer kept")?;
            if matches!(run.status, RunStatus::Running | RunStatus::Queued) {
                return Err("Stop the run before removing its worktree".into());
            }
            // The branches' worktrees first, then the run's own.
            let mut paths: Vec<String> = run
                .branch_worktrees
                .values()
                .map(|tree| tree.path.clone())
                .collect();
            paths.extend(run.worktree.iter().map(|tree| tree.path.clone()));
            let mut gone: Vec<String> = Vec::new();
            let mut failure = None;
            for path in &paths {
                match remove_worktree(Path::new(path)) {
                    Ok(()) => {
                        gone.push(path.clone());
                        if let Ok(db) = locked(&store) {
                            let _ = monocode_core::session_worktree::unmark_workflow_worktree(
                                &db, path,
                            );
                        }
                    }
                    Err(error) => {
                        failure = Some(format!("Could not remove {path}: {error}"));
                        break;
                    }
                }
            }
            // What was removed is forgotten even when a later one failed.
            update_runs(&dir, &workflow_id, |mut runs| {
                for stored in runs.iter_mut().filter(|r| r.id == run_id) {
                    stored
                        .branch_worktrees
                        .retain(|_, tree| !gone.contains(&tree.path));
                    if stored
                        .worktree
                        .as_ref()
                        .is_some_and(|t| gone.contains(&t.path))
                    {
                        stored.worktree = None;
                    }
                }
                Ok((runs, ()))
            })?;
            failure.map_or(Ok(()), Err)
        })
    }
}
