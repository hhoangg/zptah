//! The sweep of hidden sessions a Mono's workflow once asked its questions in. Workflows no
//! longer ask a Mono, but a session of that kind left by an older version, or by a quit in the
//! middle of one, is still deleted once, at startup.
use super::*;

/// Deletes the sessions of Ask-Mono questions. Only ids the roster recorded when the question
/// started are given, so no other session is ever touched. Returns the ids that could not be
/// deleted; one already gone counts as deleted, and a failure does not hold up the others.
pub(super) fn delete_asks(store: &Database, ids: &[i64]) -> Result<Vec<i64>, String> {
    let mut db = locked(store)?;
    let existing: HashSet<i64> = db.sessions()?.into_iter().map(|s| s.id).collect();
    let mut failed = Vec::new();
    for &id in ids {
        if !existing.contains(&id) {
            continue;
        }
        match db.delete(id) {
            Ok(()) => {}
            // A shared store reports an id it no longer knows this way.
            Err(error) if error == "Session no longer exists" => {}
            Err(error) => {
                eprintln!("Could not remove workflow question session {id}: {error}");
                failed.push(id);
            }
        }
    }
    Ok(failed)
}

impl Monos {
    /// Ends the sessions of Ask-Mono questions for good. The roster forgets them first and the
    /// queued roster write has to be on disk before any row is deleted: row ids can be reused, so
    /// a quit between the two must never leave an id behind that a later start would delete. The
    /// roster is only written through the manager's ordered writes, so an older snapshot can never
    /// overwrite a newer one. A row that cannot be deleted goes back into the roster, still
    /// hidden, for the next start to try again.
    pub(in crate::monos) fn forget_asks(&mut self, ids: Vec<i64>, cx: &mut Context<Self>) {
        self.roster.hidden_sessions.retain(|id| !ids.contains(id));
        self.roster.ask_sessions.retain(|id| !ids.contains(id));
        let written = self.save_reporting(cx);
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move {
            let written = written
                .recv()
                .await
                .unwrap_or_else(|_| Err("The roster was not written".into()));
            let failed = match written {
                Ok(()) => delete_asks(&store, &ids),
                Err(error) => Err(error),
            };
            // Whatever could not be deleted (everything, if the roster was not written) is kept.
            failed.unwrap_or_else(|error| {
                eprintln!("Could not remove workflow question sessions: {error}");
                ids
            })
        });
        cx.spawn(async move |this, cx| {
            let keep = work.await;
            if keep.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                for id in keep {
                    if !this.roster.hidden_sessions.contains(&id) {
                        this.roster.hidden_sessions.push(id);
                    }
                    if !this.roster.ask_sessions.contains(&id) {
                        this.roster.ask_sessions.push(id);
                    }
                }
                this.save(cx);
            });
        })
        .detach();
    }

    /// Deletes the Ask-Mono sessions a previous run of the app left behind (once, at startup).
    pub(super) fn sweep_asks(&mut self, cx: &mut Context<Self>) {
        let candidates: Vec<i64> = self.roster.ask_sessions.clone();
        if candidates.is_empty() {
            return;
        }
        self.forget_asks(candidates, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monos::tests::setup;
    use gpui::TestAppContext;

    #[gpui::test]
    fn the_sweep_removes_question_sessions_a_quit_left_behind(cx: &mut TestAppContext) {
        let s = setup(cx);
        let (leftover, lookalike) = {
            let db = locked(&s.store).unwrap();
            let leftover = db.create("codex", &s.project).unwrap();
            db.title(leftover.id, "Mono \u{b7} workflow question")
                .unwrap();
            // A hidden session whose title only looks like one: the roster never recorded it.
            let lookalike = db.create("codex", &s.project).unwrap();
            db.title(lookalike.id, "Mono \u{b7} workflow question")
                .unwrap();
            (leftover.id, lookalike.id)
        };
        cx.update(|cx| {
            s.manager.update(cx, |m, cx| {
                m.roster.hidden_sessions.extend([leftover, lookalike]);
                m.roster.ask_sessions.push(leftover);
                m.sweep_asks(cx);
            })
        });
        cx.run_until_parked();
        let (hidden, asks, sessions): (Vec<i64>, Vec<i64>, Vec<i64>) = (
            cx.update(|cx| s.manager.read(cx).roster.hidden_sessions.clone()),
            cx.update(|cx| s.manager.read(cx).roster.ask_sessions.clone()),
            locked(&s.store)
                .unwrap()
                .sessions()
                .unwrap()
                .into_iter()
                .map(|session| session.id)
                .collect(),
        );
        assert!(!hidden.contains(&leftover) && !sessions.contains(&leftover));
        assert!(!asks.contains(&leftover));
        assert!(hidden.contains(&lookalike) && sessions.contains(&lookalike));
        // The Mono's own chat is never taken for one.
        assert!(hidden.contains(&s.session) && sessions.contains(&s.session));
    }

    #[gpui::test]
    fn a_leftover_question_that_is_already_gone_does_not_hold_up_the_others(
        cx: &mut TestAppContext,
    ) {
        let s = setup(cx);
        let real = locked(&s.store)
            .unwrap()
            .create("codex", &s.project)
            .unwrap()
            .id;
        let gone = real + 1000;
        // Nothing failed: the missing one counts as deleted, the real one is deleted.
        assert_eq!(
            delete_asks(&s.store, &[gone, real]).unwrap(),
            Vec::<i64>::new()
        );
        let remaining: Vec<i64> = locked(&s.store)
            .unwrap()
            .sessions()
            .unwrap()
            .into_iter()
            .map(|session| session.id)
            .collect();
        assert!(!remaining.contains(&real));
        // And the same ids again: nothing left, nothing fails.
        assert_eq!(
            delete_asks(&s.store, &[gone, real]).unwrap(),
            Vec::<i64>::new()
        );
    }

    #[gpui::test]
    fn forgetting_a_question_session_never_writes_over_a_newer_roster(cx: &mut TestAppContext) {
        let s = setup(cx);
        let id = locked(&s.store)
            .unwrap()
            .create("codex", &s.project)
            .unwrap()
            .id;
        cx.update(|cx| {
            s.manager.update(cx, |m, cx| {
                m.roster.ask_sessions.push(id);
                m.forget_asks(vec![id], cx);
                // A change made right after, saved through the manager's ordered writes.
                m.roster.hidden_sessions.push(424_242);
                m.save(cx);
            })
        });
        cx.run_until_parked();
        let roster = monocode_core::mono::Roster::load(&locked(&s.store).unwrap()).unwrap();
        assert!(
            roster.hidden_sessions.contains(&424_242),
            "the newer write is the last"
        );
        assert!(!roster.ask_sessions.contains(&id));
    }
}
