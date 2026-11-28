//! Removing the worktrees a run made, on the user's click ("Remove worktree" on the run page).
//! Nothing else ever removes them: a session's own removal skips a worktree a run owns, and the
//! app never cleans one up on its own.
//!
//! The removal asks four things, in this order: the run must be over for good (not going, not
//! stopping, not claimed or asked to resume, no other removal going); nothing may use the folder
//! (no terminal pane, no busy session: the same guard as the Settings worktree removal); the work
//! in each worktree must still be what the user saw when they confirmed (`remove_worktree`
//! looks again); and only then the folder goes, the sessions that worked in it move to the main
//! checkout and the run record says when it was removed.
use super::Workflows;
use crate::runtime::locked;
use gpui::{Context, Task};
use monocode_core::mono::workflows::{
    model::RunStatus,
    run_worktree::{AtRisk, CHANGED_SINCE_CONFIRM, remove_worktree},
    store::{load_runs, load_workflows, update_runs},
};
use std::path::Path;

impl Workflows {
    /// Removes the worktree of the run and the worktrees of its For each branches, marks them
    /// removed in the run record and drops their ownership mark. The branches and their commits
    /// stay. `confirmed` is what the user was shown for each worktree (path and what it held);
    /// a worktree whose work is different now is not removed. A run that is going, queued,
    /// stopping or about to resume keeps its worktrees.
    pub(crate) fn remove_run_worktrees(
        &mut self,
        workflow_id: String,
        run_id: String,
        confirmed: Vec<(String, AtRisk)>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let (dir, store) = (self.dir.clone(), self.store.clone());
        let guard = match self.runner.begin_removal(&workflow_id, &run_id) {
            Ok(guard) => guard,
            Err(error) => return cx.spawn(async move |_, _| Err(error)),
        };
        let shell = self.shell.clone();
        cx.spawn(async move |this, cx| {
            // Held until the removal is over: the run cannot start or resume meanwhile.
            let _guard = guard;
            let look = {
                let (dir, workflow_id, run_id) = (dir.clone(), workflow_id.clone(), run_id.clone());
                cx.background_executor().spawn(async move {
                    let run = load_runs(&dir, &workflow_id)?
                        .into_iter()
                        .find(|run| run.id == run_id)
                        .ok_or("This run is no longer kept")?;
                    if matches!(run.status, RunStatus::Running | RunStatus::Queued) {
                        return Err("Stop the run before removing its worktree".to_string());
                    }
                    // A resume asked for is waiting to be claimed: it would use the folder.
                    let resume_asked = load_workflows(&dir)?
                        .workflows
                        .iter()
                        .any(|w| w.id == workflow_id && w.resume_run_id.as_deref() == Some(&run_id));
                    if resume_asked {
                        return Err(
                            "This run was asked to resume. Wait until it ends, or cancel the resume, before removing its worktree"
                                .to_string(),
                        );
                    }
                    // The branches' worktrees first, then the run's own.
                    let mut paths: Vec<String> = run
                        .branch_worktrees
                        .values()
                        .filter(|tree| !tree.is_removed())
                        .map(|tree| tree.path.clone())
                        .collect();
                    paths.extend(
                        run.worktree
                            .iter()
                            .filter(|tree| !tree.is_removed())
                            .map(|tree| tree.path.clone()),
                    );
                    Ok(paths)
                })
            };
            let paths = look.await?;
            // Nothing may use a folder that is about to go: the Settings guard, on the app's side.
            if let Some(shell) = shell.upgrade() {
                cx.update(|cx| {
                    paths
                        .iter()
                        .try_for_each(|path| shell.read(cx).worktree_users(path, cx).map(|_| ()))
                })?;
            }
            let work = {
                let (dir, workflow_id, run_id, store) =
                    (dir.clone(), workflow_id.clone(), run_id.clone(), store.clone());
                cx.background_executor().spawn(async move {
                    let mut gone: Vec<String> = Vec::new();
                    let mut detached = Vec::new();
                    let mut failure = None;
                    for path in &paths {
                        let Some((_, shown)) = confirmed.iter().find(|(shown, _)| shown == path)
                        else {
                            failure = Some(CHANGED_SINCE_CONFIRM.to_string());
                            break;
                        };
                        // Unmark while the folder still exists, so both the real-path and the
                        // raw key resolve. A failed removal puts the mark back.
                        let mark = locked(&store).ok().and_then(|db| {
                            let owner =
                                monocode_core::session_worktree::workflow_worktree_owner(&db, path);
                            let _ = monocode_core::session_worktree::unmark_workflow_worktree(
                                &db, path,
                            );
                            owner
                        });
                        let removed = remove_worktree(Path::new(path), shown);
                        if removed.is_err()
                            && let Some(owner) = &mark
                            && let Ok(db) = locked(&store)
                        {
                            let _ = monocode_core::session_worktree::mark_workflow_worktree(
                                &db, path, owner,
                            );
                        }
                        match removed {
                            Ok(main) => {
                                gone.push(path.clone());
                                if let Ok(db) = locked(&store) {
                                    // The sessions that worked there keep their history.
                                    if let Some(main) = main
                                        && let Ok(moved) = db.detach_worktree(path, &main)
                                    {
                                        detached.push((path.clone(), moved));
                                    }
                                }
                            }
                            Err(error) => {
                                failure = Some(if error == CHANGED_SINCE_CONFIRM {
                                    error
                                } else {
                                    format!("Could not remove {path}: {error}")
                                });
                                break;
                            }
                        }
                    }
                    // What was removed is marked so even when a later one failed. The record
                    // keeps the branch name, since the branch and its commits stay. Saved over
                    // the newest file, never over a run that went on meanwhile.
                    let at = monocode_core::time::now_ms();
                    update_runs(&dir, &workflow_id, |mut runs| {
                        for stored in runs
                            .iter_mut()
                            .filter(|r| r.id == run_id && r.status != RunStatus::Running)
                        {
                            for tree in stored
                                .branch_worktrees
                                .values_mut()
                                .chain(stored.worktree.iter_mut())
                                .filter(|t| gone.contains(&t.path))
                            {
                                tree.removed_at = Some(at);
                            }
                        }
                        Ok((runs, ()))
                    })?;
                    Ok::<_, String>((failure, detached))
                })
            };
            let (failure, detached) = work.await?;
            if let Some(shell) = shell.upgrade() {
                cx.update(|cx| {
                    shell.update(cx, |shell, cx| {
                        for (path, moved) in detached {
                            shell.worktree_detached(moved, &path, cx);
                        }
                    })
                });
            }
            let _ = this.update(cx, |_, cx| cx.notify());
            failure.map_or(Ok(()), Err)
        })
    }
}

/// A worktree some other path deleted (the Settings removal): any run that owned it says when it
/// went, and the ownership mark goes. Blocks (reads the run files): use it off the UI thread.
pub(crate) fn mark_removed_everywhere(
    dir: &monocode_core::mono::workflows::global::WorkflowsDir,
    store: &monocode_core::store::Store,
    path: &str,
) {
    let key = monocode_core::files::path_key(path);
    let _ = monocode_core::session_worktree::unmark_workflow_worktree(store, path);
    let Ok(loaded) = load_workflows(dir) else {
        return;
    };
    let at = monocode_core::time::now_ms();
    for workflow in loaded.workflows {
        let owns = load_runs(dir, &workflow.id).is_ok_and(|runs| {
            runs.iter().any(|run| {
                run.worktree
                    .iter()
                    .chain(run.branch_worktrees.values())
                    .any(|t| monocode_core::files::path_key(&t.path) == key && !t.is_removed())
            })
        });
        if !owns {
            continue;
        }
        let _ = update_runs(dir, &workflow.id, |mut runs| {
            for run in runs.iter_mut().filter(|r| r.status != RunStatus::Running) {
                for tree in run
                    .worktree
                    .iter_mut()
                    .chain(run.branch_worktrees.values_mut())
                    .filter(|t| monocode_core::files::path_key(&t.path) == key)
                {
                    tree.removed_at.get_or_insert(at);
                }
            }
            Ok((runs, ()))
        });
    }
}
