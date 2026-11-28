//! "Remove worktree" on the run page: the confirmation that lists what the removal would lose.
//! A run owns its worktree and the app never deletes it on its own; only this dialog, after the
//! user pressed Remove, takes it (and the worktrees of its For each branches) away. The branch
//! and its commits stay. Nothing is removed while the run is going.
use super::super::*;
use gpui::{Div, FontWeight};
use monocode_core::mono::workflows::{
    model::{RunStatus, WorkflowRun},
    run_worktree::{AtRisk, at_risk},
};

/// One worktree the removal would take.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct TreeEntry {
    pub(super) path: String,
    pub(super) branch: String,
}

/// The open dialog.
pub(super) struct TreeDialog {
    pub(super) workflow: String,
    pub(super) run: String,
    /// The worktrees of the run's For each branches first, the run's own last.
    pub(super) trees: Vec<TreeEntry>,
    /// What each holds that nothing else has; `None` while Git is asked.
    pub(super) risks: Option<Vec<AtRisk>>,
    pub(super) removing: bool,
    pub(super) notice: Option<String>,
    generation: u64,
}

/// The worktrees a run made, branch worktrees first.
pub(super) fn trees_of(run: &WorkflowRun) -> Vec<TreeEntry> {
    let mut trees: Vec<TreeEntry> = run
        .branch_worktrees
        .values()
        .map(|t| TreeEntry {
            path: t.path.clone(),
            branch: t.branch.clone(),
        })
        .collect();
    if let Some(tree) = &run.worktree {
        trees.push(TreeEntry {
            path: tree.path.clone(),
            branch: tree.branch.clone(),
        });
    }
    trees
}

/// Whether the run page offers to remove the run's worktree: it has one and nothing is using it.
pub(super) fn can_remove(run: &WorkflowRun) -> bool {
    !trees_of(run).is_empty() && !matches!(run.status, RunStatus::Running | RunStatus::Queued)
}

/// The lines the dialog lists for one worktree.
pub(super) fn risk_lines(risk: &AtRisk) -> Vec<String> {
    let mut lines = Vec::new();
    if risk.unknown {
        lines.push("Could not tell what this worktree holds (the folder is gone or Git did not answer).".to_string());
    }
    if !risk.uncommitted.is_empty() {
        lines.push(format!(
            "Uncommitted changes ({}):",
            risk.uncommitted.len() + risk.more_files
        ));
        lines.extend(risk.uncommitted.iter().map(|l| format!("  {l}")));
        if risk.more_files > 0 {
            lines.push(format!("  and {} more", risk.more_files));
        }
    }
    if !risk.unpushed.is_empty() {
        lines.push(format!(
            "Commits on no remote branch ({}):",
            risk.unpushed.len() + risk.more_commits
        ));
        lines.extend(risk.unpushed.iter().map(|l| format!("  {l}")));
        if risk.more_commits > 0 {
            lines.push(format!("  and {} more", risk.more_commits));
        }
    }
    lines
}

fn text_line(text: impl Into<SharedString>, color: f32) -> Div {
    div()
        .text_size(px(12.))
        .line_height(px(17.))
        .text_color(content(color))
        .child(text.into())
}

impl Monos {
    /// Opens the confirmation for the run on the page, and asks Git what it would lose.
    pub(super) fn run_tree_open(&mut self, cx: &mut Context<Self>) {
        let Some(run) = self.wf.run.run.clone().filter(can_remove) else {
            return;
        };
        let trees = trees_of(&run);
        let generation = self.wf.tree.as_ref().map_or(0, |d| d.generation) + 1;
        self.wf.tree = Some(TreeDialog {
            workflow: run.workflow_id.clone(),
            run: run.id.clone(),
            trees: trees.clone(),
            risks: None,
            removing: false,
            notice: None,
            generation,
        });
        let paths: Vec<String> = trees.into_iter().map(|t| t.path).collect();
        let work = cx.background_executor().spawn(async move {
            paths
                .iter()
                .map(|path| at_risk(std::path::Path::new(path)))
                .collect::<Vec<_>>()
        });
        cx.spawn(async move |this, cx| {
            let risks = work.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(dialog) = this.wf.tree.as_mut()
                    && dialog.generation == generation
                {
                    dialog.risks = Some(risks);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn run_tree_close(&mut self, cx: &mut Context<Self>) {
        // A removal that is going cannot be taken back, so the dialog stays until it answers.
        if self.wf.tree.as_ref().is_some_and(|d| d.removing) {
            return;
        }
        self.wf.tree = None;
        cx.notify();
    }

    /// The user pressed Remove: the worktrees go, then the dialog.
    pub(super) fn run_tree_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self
            .wf
            .tree
            .as_mut()
            .filter(|d| !d.removing && d.risks.is_some())
        else {
            return;
        };
        let (workflow, run) = (dialog.workflow.clone(), dialog.run.clone());
        let Some(workflows) = self
            .workflows_entity
            .as_ref()
            .and_then(|workflows| workflows.upgrade())
        else {
            dialog.notice = Some("The workflows are not available.".into());
            cx.notify();
            return;
        };
        dialog.removing = true;
        dialog.notice = None;
        let task = workflows.update(cx, |workflows, cx| {
            workflows.remove_run_worktrees(workflow, run, cx)
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.wf.tree = None,
                    Err(error) => {
                        if let Some(dialog) = this.wf.tree.as_mut() {
                            dialog.removing = false;
                            dialog.notice = Some(error);
                        }
                    }
                }
                this.refresh_workflows(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The dialog, when it was opened for the run on screen.
    pub(super) fn run_tree_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let dialog = self.wf.tree.as_ref()?;
        if self.wf.run.key.as_ref() != Some(&(dialog.workflow.clone(), dialog.run.clone())) {
            return None;
        }
        let removing = dialog.removing;
        let checking = dialog.risks.is_none();
        let mut body = div().flex().flex_col().gap_2();
        body = body.child(text_line(
            "The run keeps its worktree until you remove it. Removing it deletes the folder; the branch and its commits stay.",
            0.6,
        ));
        for (at, tree) in dialog.trees.iter().enumerate() {
            let mut block = div().flex().flex_col().gap_0p5().child(
                div()
                    .text_size(px(12.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(content(0.85))
                    .child(format!("{} ({})", tree.path, tree.branch)),
            );
            match dialog.risks.as_ref().and_then(|risks| risks.get(at)) {
                None => block = block.child(text_line("Checking\u{2026}", 0.45)),
                Some(risk) => {
                    let lines = risk_lines(risk);
                    if lines.is_empty() {
                        block = block.child(text_line("Nothing would be lost.", 0.45));
                    }
                    for line in lines {
                        block = block.child(text_line(line, 0.7));
                    }
                }
            }
            body = body.child(block);
        }
        if let Some(notice) = dialog.notice.clone() {
            body = body.child(text_line(notice, 0.85));
        }
        let size = window.viewport_size();
        let weak = cx.weak_entity();
        let width = 460f32.min(size.width.as_f32() - 24.);
        let tall = (size.height.as_f32() - 48.).max(160.);
        Some(
            div()
                .absolute()
                .size_full()
                .child(
                    gpui::deferred(
                        gpui::anchored()
                            .position(gpui::point(px(0.), px(0.)))
                            .child(
                                div()
                                    .w(size.width)
                                    .h(size.height)
                                    .bg(crate::modal::scrim_color())
                                    .occlude(),
                            ),
                    )
                    .with_priority(79),
                )
                .child(
                    popover("wf-run-tree-modal", Anchor::Center)
                        .modal_surface(true)
                        .width(width)
                        .gap(0.)
                        .priority(80)
                        .max_height(tall)
                        .on_dismiss(move |_, cx| {
                            let _ = weak.update(cx, |this, cx| this.run_tree_close(cx));
                        })
                        .child(
                            div()
                                .id("wf-run-tree")
                                .debug_selector(|| "wf-run-tree".into())
                                .role(Role::Dialog)
                                .aria_label("Remove the run's worktree")
                                .max_h(px(tall))
                                .flex()
                                .flex_col()
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .px_4()
                                        .pt_4()
                                        .text_size(px(14.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(content(1.))
                                        .child("Remove the run's worktree?"),
                                )
                                .child(
                                    div()
                                        .id("wf-run-tree-body")
                                        .debug_selector(|| "wf-run-tree-body".into())
                                        .flex_1()
                                        .min_h_0()
                                        .overflow_y_scroll()
                                        .px_4()
                                        .py_3()
                                        .child(body),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .px_4()
                                        .pb_4()
                                        .flex()
                                        .items_center()
                                        .justify_end()
                                        .gap_2()
                                        .child(
                                            crate::modal::ghost_button("wf-run-tree-cancel", "Cancel")
                                                .debug_selector(|| "wf-run-tree-cancel".into())
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.run_tree_close(cx)
                                                })),
                                        )
                                        .child(
                                            crate::modal::disabled(
                                                crate::modal::primary_button(
                                                    "wf-run-tree-remove",
                                                    if removing {
                                                        "Removing\u{2026}"
                                                    } else {
                                                        "Remove worktree"
                                                    },
                                                )
                                                .debug_selector(|| "wf-run-tree-remove".into()),
                                                removing || checking,
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.run_tree_confirm(cx)
                                            })),
                                        ),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
