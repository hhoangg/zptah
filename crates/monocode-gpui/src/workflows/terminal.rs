//! Terminal workflow steps: the session is the provider's own CLI in a background tab, so its
//! turn is told by the pane's events (Claude's hooks say when a turn starts and ends) instead of
//! by a conversation. The pane's state is kept here while the step runs.
use super::{Workflows, runner::host::Seen};
use crate::conversation::{TurnEnd, WorkflowTurn};
use crate::tui_session::TuiEvent;
use monocode_core::{provider::Kind, tui_launch::run_error};

const CLOSED: &str = "The terminal session closed before the step finished.";

/// What the pane of a terminal step has reported.
#[derive(Default)]
pub(in crate::workflows) struct TuiTurn {
    /// A turn has been seen going: the CLI took the prompt.
    seen_busy: bool,
    /// How the step's turn ended, once it has.
    over: Option<Result<(), String>>,
}

impl TuiTurn {
    pub(in crate::workflows) fn seen(&self) -> Seen {
        match &self.over {
            Some(Ok(())) => Seen::Over(WorkflowTurn {
                end: TurnEnd::Completed,
                reply: String::new(),
            }),
            Some(Err(why)) => Seen::Over(WorkflowTurn {
                end: TurnEnd::Failed(why.clone()),
                reply: String::new(),
            }),
            None if self.seen_busy => Seen::Working,
            None => Seen::Starting,
        }
    }

    /// Folds in what the pane said; true when the step's wait should look again.
    fn hear(&mut self, event: &TuiEvent) -> bool {
        if self.over.is_some() {
            return false;
        }
        match event {
            TuiEvent::Busy(true) => self.seen_busy = true,
            // A turn that went and stopped is the step's turn.
            TuiEvent::Busy(false) if self.seen_busy => self.over = Some(Ok(())),
            // The CLI left: after a turn that is the end of it, before one it is a failed start.
            TuiEvent::Exited(exit) => {
                self.over = Some(if self.seen_busy {
                    Ok(())
                } else {
                    Err(run_error(Kind::Claude, exit))
                })
            }
            _ => return false,
        }
        true
    }
}

impl Workflows {
    /// The shell heard from the pane of session `local`.
    pub(crate) fn tui_event(
        &mut self,
        local: i64,
        event: &TuiEvent,
        _cx: &mut gpui::Context<Self>,
    ) {
        let heard = self
            .live
            .tui
            .get_mut(&local)
            .is_some_and(|turn| turn.hear(event));
        if heard {
            let _ = self.live.wake(local).0.try_send(());
        }
    }

    /// The pane of session `local` is gone (tab closed, session removed): it can never report.
    pub(crate) fn tui_gone(&mut self, local: i64, _cx: &mut gpui::Context<Self>) {
        let Some(turn) = self.live.tui.get_mut(&local) else {
            return;
        };
        if turn.over.is_none() {
            turn.over = Some(Err(CLOSED.into()));
            let _ = self.live.wake(local).0.try_send(());
        }
    }
}
