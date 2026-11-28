//! What a workflow step in "Wait for me" and a workflow terminal step ask of the shell: the
//! banner in the waiting session, the "needs you" notice, and the terminal session a step opens
//! in the background. The state itself lives in `Workflows`; the shell only draws and delivers.
use super::*;
use crate::modal::{ghost_button, primary_button};
use crate::workflows::WaitBanner;
use monocode_core::agent::Settings;

#[cfg(test)]
mod tests;

impl Shell {
    /// The strip at the top of a session a workflow step waits on: "Workflow <name> - step
    /// <node> - [Continue] [Fail step]". Chat and terminal sessions share it.
    pub(super) fn workflow_wait_banner(
        &self,
        id: i64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let workflows = self.workflows.clone()?;
        let WaitBanner { workflow, step, .. } = workflows.read(cx).wait_banner(id)?;
        let label: SharedString = format!("Workflow {workflow} - step {step}").into();
        let answer = |choice: crate::workflows::WaitChoice| {
            cx.listener(move |this, _, _, cx| {
                this.workflow_answer_wait(id, choice, cx);
            })
        };
        Some(
            div()
                .id(("workflow-wait-banner", id as usize))
                .debug_selector(|| "workflow-wait-banner".into())
                .role(Role::Group)
                .aria_label(label.clone())
                .flex_shrink_0()
                .h(px(40.))
                .px(px(12.))
                .flex()
                .items_center()
                .gap(px(10.))
                .border_b_1()
                .border_color(content(0.1))
                .bg(content(0.05))
                .child(icon("zap", 14.).text_color(content(0.7)))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_size(px(13.))
                        .text_color(content(0.85))
                        .child(label),
                )
                .child(
                    primary_button(("workflow-wait-continue", id as usize), "Continue")
                        .debug_selector(|| "workflow-wait-continue".into())
                        .on_click(answer(crate::workflows::WaitChoice::Continue)),
                )
                .child(
                    ghost_button(("workflow-wait-fail", id as usize), "Fail step")
                        .debug_selector(|| "workflow-wait-fail".into())
                        .on_click(answer(crate::workflows::WaitChoice::Fail)),
                )
                .into_any_element(),
        )
    }

    /// The user pressed a banner button. The step settles once; a press that finds nothing
    /// waiting (already answered, stopped) only redraws.
    pub(crate) fn workflow_answer_wait(
        &mut self,
        id: i64,
        choice: crate::workflows::WaitChoice,
        cx: &mut Context<Self>,
    ) {
        if let Some(workflows) = self.workflows.clone() {
            // The answer does not reach back into the shell, so this is safe from a click.
            let _ = workflows.update(cx, |w, _| w.answer_wait(id, choice));
        }
        cx.notify();
    }

    /// A step needs the user: the usual "needs you" notice, on this computer and the phone.
    pub(crate) fn workflow_needs_you(&mut self, id: i64, body: &str, cx: &mut Context<Self>) {
        if let Some(summary) = self.session_summaries.get(&id).cloned() {
            self.deliver_session_notice(
                &summary,
                "agentInput",
                body,
                Some(crate::sound::Cue::Input),
                cx,
            );
        }
        cx.notify();
    }

    /// Where a terminal workflow session runs: its worktree, else its project.
    pub(crate) fn workflow_tui_dir(&self, id: i64) -> Option<String> {
        self.session_worktrees
            .get(&id)
            .cloned()
            .flatten()
            .or_else(|| self.session_summaries.get(&id).map(|s| s.cwd.clone()))
    }

    /// The summary of a terminal workflow session, for the run's session list.
    pub(crate) fn workflow_tui_summary(&self, id: i64) -> Option<Session> {
        self.session_summaries.get(&id).cloned()
    }

    /// A terminal step is stopped: its CLI ends, and nothing relaunches it.
    pub(crate) fn workflow_stop_tui(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(tui) = self.tui_sessions.get(&id).cloned() {
            tui.update(cx, |tui, cx| tui.kill(cx));
        }
    }

    /// Puts a terminal session a workflow step made into the app as a background tab (the
    /// session is kept alive without being shown, and nobody's focus moves) and starts the CLI
    /// with `prompt` as its first prompt. False when the terminal could not be opened.
    pub(crate) fn workflow_install_tui(
        &mut self,
        summary: Session,
        settings: &Settings,
        prompt: &str,
        cx: &mut Context<Self>,
    ) -> bool {
        let id = summary.id;
        // The pane reads where it runs from here and from the saved settings.
        if let Some(tree) = &settings.worktree_cwd {
            self.session_worktrees.insert(id, Some(tree.clone()));
        }
        self.add(summary, cx);
        self.place_background_tab(id);
        let started = self.start_tui_with_prompt(id, prompt, cx);
        self.persist(cx);
        started
    }
}
