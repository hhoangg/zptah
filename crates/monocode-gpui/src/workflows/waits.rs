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
            },
        );
        #[cfg(test)]
        self.live.needs_you.push((local, body.clone()));
        if let Some(shell) = self.shell.upgrade() {
            shell.update(cx, |shell, cx| shell.workflow_needs_you(local, &body, cx));
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

    /// The user pressed a button of the banner on session `local`. The banner goes at once; the
    /// step settles once whatever is pressed afterwards.
    pub(crate) fn answer_wait(&mut self, local: i64, choice: WaitChoice) -> Result<(), String> {
        let ids = self
            .live
            .waits
            .get(&local)
            .map(|wait| vec![wait.session_id.clone(), local.to_string()])
            .ok_or("This step is no longer waiting for you.")?;
        let answered = self.runner.answer_wait(&ids, choice);
        self.live.waits.remove(&local);
        answered
    }
}
