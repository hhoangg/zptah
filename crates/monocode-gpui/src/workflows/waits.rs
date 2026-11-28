//! "Wait for me": the step's session shows a banner, the user is told, and the runner waits for
//! Continue or Fail step. The banner state is kept here and drawn by the shell.
use super::{
    Workflows,
    runner::{WaitChoice, WaitNotice},
};
use gpui::Context;

/// What the banner in a waiting session says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct WaitBanner {
    /// The session by the id run files use.
    pub session_id: String,
    pub workflow: String,
    pub step: String,
    /// Continue was pressed while the session's turn still runs: the step takes its outputs when
    /// the turn ends, and Continue is not offered again.
    pub turn_running: bool,
}

impl Workflows {
    /// The banner session `local` shows, when a step waits on it.
    pub(crate) fn wait_banner(&self, local: i64) -> Option<WaitBanner> {
        self.live.waits.get(&local).cloned()
    }

    /// The "needs you" notices raised so far, for tests.
    #[cfg(test)]
    pub(crate) fn needs_you(&self) -> Vec<(i64, String)> {
        self.live.needs_you.clone()
    }

    /// The step of `notice` waits: banner up and a "needs you" notice. Called from the run.
    pub(in crate::workflows) fn workflow_wait(
        &mut self,
        notice: WaitNotice,
        cx: &mut Context<Self>,
    ) {
        let Some(local) = self.workflow_local_id(&notice.session_id) else {
            return;
        };
        let body = format!(
            "Workflow {} - step {} needs you",
            notice.workflow, notice.step
        );
        self.live.waits.insert(
            local,
            WaitBanner {
                session_id: notice.session_id,
                workflow: notice.workflow,
                step: notice.step,
                turn_running: false,
            },
        );
        #[cfg(test)]
        self.live.needs_you.push((local, body.clone()));
        if let Some(shell) = self.shell.upgrade() {
            shell.update(cx, |shell, cx| shell.workflow_needs_you(local, &body, cx));
        }
    }

    /// Continue was pressed while the session's turn still runs: the banner says the step waits
    /// for the turn to end.
    /// True when it changed. It does not redraw: it is called from a click on the shell too, which
    /// redraws itself.
    pub(in crate::workflows) fn wait_for_turn(&mut self, local: i64) -> bool {
        match self.live.waits.get_mut(&local) {
            Some(wait) if !wait.turn_running => {
                wait.turn_running = true;
                true
            }
            _ => false,
        }
    }

    /// The wait ended (answered, timed out, stopped): the banner goes.
    pub(in crate::workflows) fn workflow_wait_over(
        &mut self,
        persisted: &str,
        cx: &mut Context<Self>,
    ) {
        let Some(local) = self.workflow_local_id(persisted) else {
            return;
        };
        if self.live.waits.remove(&local).is_some()
            && let Some(shell) = self.shell.upgrade()
        {
            shell.update(cx, |_, cx| cx.notify());
        }
    }

    /// The user pressed a button of the banner on session `local`. The step settles once
    /// whatever is pressed afterwards. The banner goes at once, except for a Continue while the
    /// session's turn still runs: it stays, saying the step waits for the turn, until the step
    /// has taken its outputs.
    pub(crate) fn answer_wait(
        &mut self,
        local: i64,
        choice: WaitChoice,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let ids = self
            .live
            .waits
            .get(&local)
            .filter(|wait| !wait.turn_running)
            .map(|wait| vec![wait.session_id.clone(), local.to_string()])
            .ok_or("This step is no longer waiting for you.")?;
        let answered = self.runner.answer_wait(&ids, choice);
        if answered.is_ok()
            && choice == WaitChoice::Continue
            && self.workflow_busy(local, cx) == Some(true)
        {
            self.wait_for_turn(local);
        } else {
            self.live.waits.remove(&local);
        }
        answered
    }
}
