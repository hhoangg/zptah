//! The driver of the conversation switch benchmark (`crate::perf_switch`):
//! it opens a long chat and a short one in turn, the way a reader switching
//! tabs would, and times each return to the long chat.
use super::*;
use crate::perf_switch::{self as probe, Outcome, Reading, Scenario};
use gpui::{AsyncWindowContext, WeakEntity};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(10);
/// How long one switch may take before it counts as a timeout.
const LIMIT: Duration = Duration::from_secs(10);
/// Time for the previous step's work to finish before the next begins.
const SETTLE: Duration = Duration::from_millis(500);
const STARTUP: Duration = Duration::from_secs(60);

impl Shell {
    pub(crate) fn start_perf_switch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| run(this, cx).await)
            .detach();
    }
    /// The newest session with this title.
    fn perf_session(&self, title: &str) -> Option<i64> {
        self.session_summaries
            .values()
            .filter(|session| session.title == title)
            .max_by_key(|session| session.updated_at)
            .map(|session| session.id)
    }
    fn perf_transcript(
        &self,
        id: i64,
        cx: &gpui::App,
    ) -> Option<Entity<crate::transcript::TranscriptView>> {
        Some(self.sessions.get(&id)?.read(cx).transcript.clone())
    }
}

async fn pause(cx: &mut AsyncWindowContext, duration: Duration) {
    cx.background_executor().timer(duration).await;
}

/// Polls until `done`, for at most `limit`.
async fn until(
    cx: &mut AsyncWindowContext,
    limit: Duration,
    mut done: impl FnMut(&mut AsyncWindowContext) -> bool,
) -> bool {
    let started = Instant::now();
    while !done(cx) {
        if started.elapsed() >= limit {
            return false;
        }
        pause(cx, POLL).await;
    }
    true
}

async fn run(this: WeakEntity<Shell>, cx: &mut AsyncWindowContext) {
    let config = probe::config();
    let mut ids = None;
    until(cx, STARTUP, |cx| {
        ids = this
            .read_with(cx, |shell, _| {
                Some((
                    shell.perf_session(&config.target)?,
                    shell.perf_session(&config.filler)?,
                ))
            })
            .ok()
            .flatten();
        ids.is_some()
    })
    .await;
    let Some((target, filler)) = ids else {
        probe::log(&format!(
            "perf-switch v=1 label={} error=missing-session target={:?} filler={:?}",
            config.label, config.target, config.filler
        ));
        quit(&this, cx);
        return;
    };
    // Let the window settle on its first chat.
    pause(cx, SETTLE).await;
    painting(&this, cx).await;
    for &scenario in &config.scenarios {
        let mut outcomes = Vec::new();
        for trial in 0..config.warmup + config.trials {
            let warmup = trial < config.warmup;
            if let Some(outcome) = switch(&this, cx, scenario, target, filler, trial, warmup).await
            {
                probe::log(&outcome.line(&config.label));
                outcomes.push(outcome);
            }
        }
        probe::log(&probe::summary(&config.label, scenario, &outcomes));
    }
    quit(&this, cx);
}

/// Waits until the window draws: asks for a redraw of everything and waits
/// for a frame callback, which runs only when the window is presented. A
/// window the system is not presenting (hidden, covered, a locked screen)
/// draws nothing, and timing a switch there only measures when it is shown
/// again. This does not depend on any transcript being on screen.
async fn painting(this: &WeakEntity<Shell>, cx: &mut AsyncWindowContext) {
    let started = Instant::now();
    loop {
        let drawn = Arc::new(AtomicBool::new(false));
        let flag = drawn.clone();
        let _ = this.update_in(cx, |_, window, _| {
            window.on_next_frame(move |_, _| flag.store(true, Ordering::Relaxed));
            window.refresh();
        });
        pause(cx, Duration::from_millis(250)).await;
        if drawn.load(Ordering::Relaxed) {
            return;
        }
        if started.elapsed() >= STARTUP {
            probe::log(&format!(
                "perf-switch v=1 label={} warning=window-not-painting",
                probe::config().label
            ));
            return;
        }
    }
}

fn quit(this: &WeakEntity<Shell>, cx: &mut AsyncWindowContext) {
    if probe::config().exit {
        let _ = this.update(cx, |_, cx| crate::quit(cx));
    }
}

fn open(this: &WeakEntity<Shell>, cx: &mut AsyncWindowContext, id: i64) -> bool {
    this.update_in(cx, |shell, window, cx| shell.open(id, window, cx))
        .is_ok()
}

fn loaded(this: &WeakEntity<Shell>, cx: &mut AsyncWindowContext, id: i64) -> bool {
    this.read_with(cx, |shell, cx| {
        shell
            .sessions
            .get(&id)
            .is_some_and(|chat| chat.read(cx).history_loaded())
    })
    .unwrap_or(false)
}

fn reading(this: &WeakEntity<Shell>, cx: &mut AsyncWindowContext, id: i64) -> Option<Reading> {
    this.read_with(cx, |shell, cx| {
        shell.perf_transcript(id, cx)?.read(cx).perf_reading()
    })
    .ok()
    .flatten()
}

/// One trial: leave the long chat for the short one the way `scenario`
/// describes, then time the way back.
async fn switch(
    this: &WeakEntity<Shell>,
    cx: &mut AsyncWindowContext,
    scenario: Scenario,
    target: i64,
    filler: i64,
    trial: usize,
    warmup: bool,
) -> Option<Outcome> {
    let config = probe::config();
    let mut expected = None;
    let mut evicted = false;
    if !open(this, cx, filler) {
        return None;
    }
    pause(cx, SETTLE).await;
    if scenario == Scenario::Uncached {
        this.update(cx, |shell, _| shell.sessions.remove(&target))
            .ok()?;
    } else {
        // Load the long chat at its end, as a reader who just left it would have.
        if !open(this, cx, target) {
            return None;
        }
        until(cx, LIMIT, |cx| loaded(this, cx, target)).await;
        pause(cx, SETTLE).await;
        if scenario == Scenario::Anchor {
            this.update(cx, |shell, cx| {
                let transcript = shell.perf_transcript(target, cx)?;
                transcript.update(cx, |view, cx| {
                    view.perf_scroll_into_history(config.anchor_turns, cx)
                });
                Some(())
            })
            .ok()?;
            pause(cx, SETTLE).await;
            expected = reading(this, cx, target);
        }
        if scenario == Scenario::Hit {
            probe::pin(target);
        }
        if !open(this, cx, filler) {
            probe::unpin();
            return None;
        }
        pause(cx, SETTLE).await;
        if scenario != Scenario::Hit {
            // The shell may have trimmed it already when the filler took over.
            let _ = this.update(cx, |shell, cx| {
                if let Some(chat) = shell.sessions.get(&target).cloned() {
                    chat.update(cx, |chat, cx| chat.unload_history(cx));
                }
            });
        }
        // For a hit this reports a history that was trimmed despite the pin.
        evicted = !loaded(this, cx, target);
    }
    let transcript = this
        .read_with(cx, |shell, cx| {
            shell
                .perf_transcript(target, cx)
                .map(|transcript| transcript.entity_id())
        })
        .ok()
        .flatten();
    painting(this, cx).await;
    probe::arm(target, transcript);
    if !open(this, cx, target) {
        probe::unpin();
        return probe::finish(scenario, trial, warmup, evicted, expected, true);
    }
    let done = until(cx, LIMIT, |_| {
        let (painted, ready) = probe::progress();
        painted && (ready || scenario == Scenario::Hit)
    })
    .await;
    // Late hooks (the install after a first paint) finish within this.
    pause(cx, SETTLE).await;
    probe::unpin();
    probe::finish(scenario, trial, warmup, evicted, expected, !done)
}
