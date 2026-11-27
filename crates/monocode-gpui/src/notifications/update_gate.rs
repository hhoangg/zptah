//! The one gate every update goes through: before an update installs and restarts the app,
//! ask the user when work is running. The wording and the "is it needed" rule are
//! `monocode_core::app_update::confirm`; this file gathers the running work from every window
//! and shows the dialog (the shared modal, through `window.prompt`).
use crate::shell::Shell;
use gpui::{AnyWindowHandle, App, PromptLevel};
use monocode_core::app_update::{
    confirm::{self, CANCEL, CONFIRM},
    remote::RunningSession,
};

/// Replaces what [`running`] gathers (tests, which have no `Shell` window to read).
#[cfg(test)]
#[derive(Clone, Default)]
pub(crate) struct RunningOverride(pub std::rc::Rc<std::cell::RefCell<Vec<RunningSession>>>);
#[cfg(test)]
impl gpui::Global for RunningOverride {}

/// Local work an update would interrupt, across all windows: chats, Mono chats and habit runs
/// with a turn in progress, and terminal agents mid-turn. Cloud sessions are not listed: a local
/// update does not stop them. Needs the windows readable, so call it from a deferred callback.
pub(crate) fn running(cx: &App) -> Vec<RunningSession> {
    #[cfg(test)]
    if let Some(over) = cx.try_global::<RunningOverride>() {
        return over.0.borrow().clone();
    }
    let mut all: Vec<RunningSession> = Vec::new();
    for window in cx.windows() {
        let Some(shell) = window.downcast::<Shell>() else {
            continue;
        };
        let Ok(found) = shell
            .read(cx)
            .map(|shell| shell.remote_update_running(None, cx))
        else {
            continue;
        };
        for work in found {
            if !all.iter().any(|seen| seen.id == work.id) {
                all.push(work);
            }
        }
    }
    all
}

/// `done(Some(running))` when the update may go on: nothing new is running, or the user chose
/// "Update anyway" (the list is what they accepted). `done(None)` when they cancelled, or when
/// there is no window to ask in. `acknowledged` is work the user already accepted for this
/// update, so only work that started since asks again. Never runs `done` before returning.
pub(crate) fn ask(
    cx: &mut App,
    requester: Option<String>,
    acknowledged: Vec<RunningSession>,
    done: impl FnOnce(Option<Vec<RunningSession>>, &mut App) + 'static,
) {
    // Deferred: a click handler runs inside its window's update, which blocks both reading
    // that window's `Shell` and opening a dialog in it.
    cx.defer(move |cx| {
        let found = running(cx);
        decide(cx, found, true, requester, acknowledged, done);
    });
}

/// [`ask`] for a caller that already gathered the running work.
pub(crate) fn ask_about(
    cx: &mut App,
    found: Vec<RunningSession>,
    requester: Option<String>,
    done: impl FnOnce(Option<Vec<RunningSession>>, &mut App) + 'static,
) {
    cx.defer(move |cx| decide(cx, found, false, requester, Vec::new(), done));
}

/// `regather`: look at the running work again on each retry (the caller did not fix a list).
fn decide(
    cx: &mut App,
    found: Vec<RunningSession>,
    regather: bool,
    requester: Option<String>,
    acknowledged: Vec<RunningSession>,
    done: impl FnOnce(Option<Vec<RunningSession>>, &mut App) + 'static,
) {
    let Some(prompt) = confirm::prompt(&found, &acknowledged, requester.as_deref()) else {
        done(Some(found), cx);
        return;
    };
    let Some(window) = window(cx) else {
        done(None, cx);
        return;
    };
    let shown = window.update(cx, |_, window, cx| {
        // Never replace a dialog the user is looking at: wait until it is gone.
        if window.has_active_prompt() {
            return None;
        }
        // The window may be hidden (closed to the tray): bring it back so the question is seen.
        cx.activate(true);
        window.activate_window();
        Some(crate::modal::cancel_default(|| {
            window.prompt(
                PromptLevel::Warning,
                prompt.title,
                Some(&prompt.detail),
                &[CONFIRM, CANCEL],
                cx,
            )
        }))
    });
    let answer = match shown {
        Err(_) => {
            done(None, cx);
            return;
        }
        Ok(None) => {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(RETRY).await;
                cx.update(|cx| {
                    // Work may have started or ended while another dialog was open.
                    let found = if regather { running(cx) } else { found };
                    decide(cx, found, regather, requester, acknowledged, done)
                });
            })
            .detach();
            return;
        }
        Ok(Some(answer)) => answer,
    };
    cx.spawn(async move |cx| {
        let accepted = matches!(answer.await, Ok(0));
        cx.update(|cx| done(accepted.then_some(found), cx));
    })
    .detach();
}

/// How long to wait before asking again while another dialog is open.
const RETRY: std::time::Duration = std::time::Duration::from_millis(500);

/// The window the dialog opens in: the active one, else an app window.
fn window(cx: &App) -> Option<AnyWindowHandle> {
    cx.active_window()
        .or_else(|| {
            cx.windows()
                .into_iter()
                .find(|window| window.downcast::<Shell>().is_some())
        })
        .or_else(|| cx.windows().into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use std::{cell::RefCell, rc::Rc};

    fn work(id: &str, title: &str) -> RunningSession {
        RunningSession {
            id: id.into(),
            title: title.into(),
        }
    }
    /// Asks and returns where the answer lands: `None` until answered.
    type Answer = Rc<RefCell<Option<Option<Vec<RunningSession>>>>>;
    fn ask_now(cx: &mut VisualTestContext, acknowledged: Vec<RunningSession>) -> Answer {
        let answer: Answer = Rc::default();
        let sink = answer.clone();
        cx.update(|_, cx| {
            ask(cx, None, acknowledged, move |accepted, _| {
                *sink.borrow_mut() = Some(accepted)
            })
        });
        cx.run_until_parked();
        answer
    }
    /// `builder`: draw the real modal (keys work) instead of a platform prompt.
    fn setup(
        cx: &mut TestAppContext,
        running: Vec<RunningSession>,
        builder: bool,
    ) -> (VisualTestContext, RunningOverride) {
        if builder {
            cx.update(|cx| cx.set_prompt_builder(crate::modal::prompt_builder));
        }
        let over = RunningOverride(Rc::new(RefCell::new(running)));
        cx.update(|cx| cx.set_global(over.clone()));
        let window = cx.add_window(|_, _| gpui::Empty);
        (VisualTestContext::from_window(window.into(), cx), over)
    }

    #[gpui::test]
    fn no_running_work_goes_ahead_without_a_dialog(cx: &mut TestAppContext) {
        let (mut visual, _) = setup(cx, Vec::new(), false);
        let answer = ask_now(&mut visual, Vec::new());
        assert_eq!(*answer.borrow(), Some(Some(Vec::new())));
        assert!(!visual.has_pending_prompt());
    }

    #[gpui::test]
    fn running_work_shows_the_dialog_and_cancel_declines(cx: &mut TestAppContext) {
        let (mut visual, _) = setup(cx, vec![work("1", "Fix login")], false);
        let answer = ask_now(&mut visual, Vec::new());
        assert!(visual.has_pending_prompt());
        assert_eq!(*answer.borrow(), None, "waits for the user");
        visual.simulate_prompt_answer("Cancel");
        visual.run_until_parked();
        assert_eq!(*answer.borrow(), Some(None));
    }

    fn decline_with(key: &str, cx: &mut TestAppContext) {
        let (mut visual, _) = setup(cx, vec![work("1", "Fix login")], true);
        let answer = ask_now(&mut visual, Vec::new());
        assert_eq!(*answer.borrow(), None);
        visual.simulate_keystrokes(key);
        visual.run_until_parked();
        assert_eq!(*answer.borrow(), Some(None), "{key}");
    }
    #[gpui::test]
    fn escape_declines_the_real_dialog(cx: &mut TestAppContext) {
        decline_with("escape", cx);
    }
    #[gpui::test]
    fn enter_declines_the_real_dialog(cx: &mut TestAppContext) {
        decline_with("enter", cx);
    }

    #[gpui::test]
    fn update_anyway_goes_ahead(cx: &mut TestAppContext) {
        let (mut visual, _) = setup(cx, vec![work("1", "Fix login")], false);
        let answer = ask_now(&mut visual, Vec::new());
        visual.simulate_prompt_answer("Update anyway");
        visual.run_until_parked();
        assert_eq!(*answer.borrow(), Some(Some(vec![work("1", "Fix login")])));
    }

    #[gpui::test]
    fn work_started_after_the_first_answer_asks_again(cx: &mut TestAppContext) {
        let first = vec![work("1", "A")];
        let (mut visual, over) = setup(cx, first.clone(), false);
        // Accepted earlier and still the only work: no second dialog.
        let answer = ask_now(&mut visual, first.clone());
        assert_eq!(*answer.borrow(), Some(Some(first.clone())));
        assert!(!visual.has_pending_prompt());
        // Something else started during the download.
        over.0.borrow_mut().push(work("2", "B"));
        let answer = ask_now(&mut visual, first);
        assert!(visual.has_pending_prompt());
        visual.simulate_prompt_answer("Cancel");
        visual.run_until_parked();
        assert_eq!(*answer.borrow(), Some(None));
    }

    #[gpui::test]
    fn another_open_dialog_is_not_replaced(cx: &mut TestAppContext) {
        let (mut visual, _) = setup(cx, vec![work("1", "A")], true);
        let mut other = visual.update(|window, cx| {
            window.prompt(PromptLevel::Info, "Something else", None, &["OK"], cx)
        });
        visual.run_until_parked();
        let answer = ask_now(&mut visual, Vec::new());
        assert_eq!(*answer.borrow(), None);
        assert_eq!(
            other.try_recv(),
            Ok(None),
            "the user's dialog is still open"
        );
        // The user closes it; the update question follows.
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert_eq!(other.try_recv(), Ok(Some(0)));
        visual.executor().advance_clock(RETRY * 2);
        visual.run_until_parked();
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert_eq!(
            *answer.borrow(),
            Some(None),
            "the update dialog was shown and answered"
        );
    }

    #[gpui::test]
    fn work_that_ends_while_another_dialog_is_open_is_not_asked_about(cx: &mut TestAppContext) {
        let (mut visual, over) = setup(cx, vec![work("1", "A")], true);
        let _other = visual.update(|window, cx| {
            window.prompt(PromptLevel::Info, "Something else", None, &["OK"], cx)
        });
        visual.run_until_parked();
        let answer = ask_now(&mut visual, Vec::new());
        assert_eq!(*answer.borrow(), None);
        over.0.borrow_mut().clear();
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        visual.executor().advance_clock(RETRY * 2);
        visual.run_until_parked();
        assert_eq!(
            *answer.borrow(),
            Some(Some(Vec::new())),
            "nothing runs: no dialog"
        );
    }
}
