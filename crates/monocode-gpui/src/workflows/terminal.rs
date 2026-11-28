//! Terminal workflow steps: the session is the provider's own CLI in a background tab, so its
//! turn is told by the pane's events (Claude's hooks say when a turn starts and ends) instead of
//! by a conversation. The pane's state is kept here while the step runs.
use super::{Workflows, runner::host::Seen};
use crate::conversation::{TurnEnd, WorkflowTurn};
use crate::tui_session::TuiEvent;
use monocode_core::{
    provider::Kind,
    tui_launch::{Exit, run_error},
};

const CLOSED: &str = "The terminal session closed before the step finished.";

/// What the pane of a terminal step has reported.
#[derive(Default)]
pub(in crate::workflows) struct TuiTurn {
    /// A turn has been seen going: the CLI took the prompt.
    seen_busy: bool,
    /// Whether the CLI is working right now, kept after the step's own turn ended too (the user
    /// may type on while the step waits for them).
    busy: bool,
    /// How the step's turn ended, once it has.
    over: Option<Result<(), String>>,
    /// The step's run has looked at `over`. An exit that comes right behind the end of the turn
    /// (the pane ends its turn when the CLI leaves) can still change what the turn's end means,
    /// until then.
    taken: std::cell::Cell<bool>,
}

impl TuiTurn {
    pub(in crate::workflows) fn seen(&self) -> Seen {
        if self.over.is_some() {
            self.taken.set(true);
        }
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

    /// Whether the CLI is working on a turn now.
    pub(in crate::workflows) fn busy(&self) -> bool {
        self.busy
    }

    /// Folds in what the pane said; true when a wait should look again.
    fn hear(&mut self, event: &TuiEvent) -> bool {
        match event {
            TuiEvent::Busy(busy) => {
                let changed = self.busy != *busy;
                self.busy = *busy;
                if self.over.is_some() {
                    return changed && !*busy;
                }
                if *busy {
                    self.seen_busy = true;
                } else if self.seen_busy {
                    // A turn that went and stopped is the step's turn.
                    self.over = Some(Ok(()));
                } else {
                    return false;
                }
                true
            }
            TuiEvent::Exited(exit) => {
                self.busy = false;
                let clean = exit.error.is_none() && exit.code.is_none_or(|code| code == 0);
                match &self.over {
                    // The turn was over long before: the CLI leaving changes nothing.
                    Some(_) if self.taken.get() => return true,
                    // The pane ends the turn as it exits, so an exit that is not clean ended it
                    // badly.
                    Some(Ok(())) if !clean => {
                        self.over = Some(Err(left_mid_turn(exit)));
                    }
                    Some(_) => return true,
                    // The CLI left before any turn went, or while one was going.
                    None if self.seen_busy => self.over = Some(Err(left_mid_turn(exit))),
                    None => self.over = Some(Err(run_error(Kind::Claude, exit))),
                }
                true
            }
            _ => false,
        }
    }
}

/// Why a step fails when its CLI left before the turn ended.
fn left_mid_turn(exit: &Exit) -> String {
    format!(
        "The terminal session exited before its turn ended. {}",
        run_error(Kind::Claude, exit)
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    fn exit(code: Option<i32>) -> TuiEvent {
        TuiEvent::Exited(Exit {
            code,
            error: None,
            early: false,
            no_conversation: false,
        })
    }

    fn outcome(turn: &TuiTurn) -> Option<Result<(), String>> {
        match turn.seen() {
            Seen::Over(WorkflowTurn {
                end: TurnEnd::Completed,
                ..
            }) => Some(Ok(())),
            Seen::Over(WorkflowTurn {
                end: TurnEnd::Failed(why),
                ..
            }) => Some(Err(why)),
            _ => None,
        }
    }

    #[test]
    fn an_exit_while_a_turn_goes_fails_the_step() {
        let mut turn = TuiTurn::default();
        turn.hear(&TuiEvent::Busy(true));
        assert!(turn.hear(&exit(Some(0))));
        let why = outcome(&turn).unwrap().unwrap_err();
        assert!(why.contains("exited before its turn ended"), "{why}");
    }

    #[test]
    fn an_error_exit_right_behind_the_end_of_the_turn_fails_the_step() {
        let mut turn = TuiTurn::default();
        turn.hear(&TuiEvent::Busy(true));
        turn.hear(&TuiEvent::Busy(false));
        turn.hear(&exit(Some(2)));
        assert!(outcome(&turn).unwrap().is_err());
    }

    #[test]
    fn a_clean_exit_after_a_turn_the_run_has_seen_changes_nothing() {
        let mut turn = TuiTurn::default();
        turn.hear(&TuiEvent::Busy(true));
        turn.hear(&TuiEvent::Busy(false));
        assert_eq!(outcome(&turn), Some(Ok(())));
        turn.hear(&exit(Some(1)));
        assert_eq!(outcome(&turn), Some(Ok(())));
    }

    #[test]
    fn busy_is_kept_after_the_turn_ended() {
        let mut turn = TuiTurn::default();
        turn.hear(&TuiEvent::Busy(true));
        turn.hear(&TuiEvent::Busy(false));
        assert!(!turn.busy());
        turn.hear(&TuiEvent::Busy(true));
        assert!(turn.busy());
        assert!(turn.hear(&TuiEvent::Busy(false)), "the waiter looks again");
        assert!(!turn.busy());
    }
}
