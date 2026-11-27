//! Shared app updater: one check/install across windows; no I/O on the UI thread.
use crate::{
    runtime::{Database, locked},
    theme::*,
};
use gpui::{App, Context, Entity, EventEmitter, Global, Role, div, prelude::*, px};
use monocode_core::app_update::{
    self, Config, Installation, Installed, Update, remote::RunningSession,
};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Clone)]
pub(crate) struct Shared(pub Entity<Updater>, Arc<Mutex<InstallGate>>);
impl Global for Shared {}
#[derive(Default)]
struct InstallGate {
    installing: bool,
    quitting: bool,
}

/// A normal quit can cancel a download, but must not interrupt a file swap or
/// a privileged package install. Keep this gate outside the entity borrow so
/// successful installation can call the regular quit path from its callback.
pub(crate) fn defer_quit(cx: &App) -> bool {
    let Some(shared) = cx.try_global::<Shared>() else {
        return false;
    };
    let mut gate = shared.1.lock().unwrap_or_else(|e| e.into_inner());
    gate.quitting = true;
    gate.installing
}
pub(crate) enum Event {
    Changed,
    Error(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Disabled,
    Idle,
    Checking,
    Current,
    Available,
    Downloading,
    Installing,
}
enum Progress {
    /// The re-check is over and the download is starting.
    Started,
    Download(u64, Option<u64>),
    Installing,
    /// The download is done; the UI thread decides whether work that started meanwhile may be
    /// interrupted. Answer on the channel.
    Confirm(async_channel::Sender<bool>),
}

/// The error of an install the user declined at the last moment. Not shown as a failure.
pub(crate) const DECLINED: &str = "The update was not installed: work is running";

pub(crate) fn version() -> &'static str {
    option_env!("MONOCODE_NATIVE_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}
/// The feed, its verification key and the running version, from the build.
pub(crate) fn feed_config() -> Result<Config, String> {
    let endpoint = option_env!("MONOCODE_NATIVE_UPDATER_ENDPOINT")
        .filter(|v| !v.is_empty())
        .ok_or("Automatic updates aren’t configured for this build")?;
    let key = option_env!("MONOCODE_NATIVE_UPDATER_PUBKEY")
        .filter(|v| !v.is_empty())
        .ok_or("This build has no update verification key")?;
    Config::new(endpoint, key, version())
}
/// Asks the signed feed whether a newer package exists. Blocking: run it off the UI thread.
pub(crate) fn check_feed() -> Result<Option<(Update, Installation)>, String> {
    let config = feed_config()?;
    let installation = Installation::detect()?;
    Ok(app_update::check(&config, &installation)?.map(|update| (update, installation)))
}
/// Called with the reason when a remote install fails before the app restarts.
pub(crate) type InstallFailed = Box<dyn FnOnce(String, &mut App)>;
/// Starts the install of an update found by [`check_feed`], through the one path the Update
/// and Restart button uses (download, signature check, install, relaunch).
pub(crate) fn install_remote(
    update: Update,
    installation: Installation,
    failed: InstallFailed,
    acknowledged: Vec<RunningSession>,
    cx: &mut App,
) -> Result<(), String> {
    let entity = cx
        .try_global::<Shared>()
        .map(|shared| shared.0.clone())
        .ok_or("The updater is not running")?;
    entity.update(cx, |updater, cx| {
        updater.install_remote(update, installation, failed, acknowledged, cx)
    })
}
/// An update is downloading or installing now.
pub(crate) fn is_busy(cx: &App) -> bool {
    cx.try_global::<Shared>()
        .is_some_and(|shared| shared.0.read(cx).busy())
}
pub(crate) fn shared(store: Database, cx: &mut App) -> Entity<Updater> {
    if let Some(shared) = cx.try_global::<Shared>() {
        return shared.0.clone();
    }
    let entity = cx.new(|cx| Updater::new(store, cx));
    let gate = entity.read(cx).gate.clone();
    cx.set_global(Shared(entity.clone(), gate));
    entity
}

pub(crate) struct Updater {
    config: Option<Config>,
    installation: Option<Installation>,
    store: Database,
    phase: Phase,
    pending: Option<Update>,
    progress: Option<u64>,
    /// Re-reading the feed before the download starts.
    rechecking: bool,
    message: Option<String>,
    poll: Option<gpui::Task<()>>,
    gate: Arc<Mutex<InstallGate>>,
    /// A remote install in progress: told why it failed, once.
    remote_failed: Option<InstallFailed>,
    /// Work the user already accepted for this install; only newer work asks again.
    acknowledged: Vec<RunningSession>,
    /// UI installs that got past the busy check, for tests.
    #[cfg(test)]
    installs: usize,
}
impl EventEmitter<Event> for Updater {}
impl Updater {
    fn new(store: Database, cx: &mut Context<Self>) -> Self {
        let (config, message) = match feed_config() {
            Ok(config) => (Some(config), None),
            Err(error) => (None, Some(error)),
        };
        let mut this = Self {
            phase: if config.is_some() {
                Phase::Idle
            } else {
                Phase::Disabled
            },
            config,
            installation: None,
            store,
            pending: None,
            progress: None,
            rechecking: false,
            message,
            poll: None,
            gate: Arc::new(Mutex::new(InstallGate::default())),
            remote_failed: None,
            acknowledged: Vec::new(),
            #[cfg(test)]
            installs: 0,
        };
        if this.config.is_some() {
            this.check(false, cx);
            this.poll = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_secs(4 * 60 * 60))
                        .await;
                    if this.update(cx, |this, cx| this.check(false, cx)).is_err() {
                        break;
                    }
                }
            }));
        }
        this
    }
    fn busy(&self) -> bool {
        matches!(
            self.phase,
            Phase::Checking | Phase::Downloading | Phase::Installing
        )
    }
    pub(crate) fn actionable(&self) -> bool {
        matches!(
            self.phase,
            Phase::Available | Phase::Downloading | Phase::Installing
        )
    }
    fn changed(&self, cx: &mut Context<Self>) {
        cx.emit(Event::Changed);
        cx.notify();
    }
    pub(crate) fn check(&mut self, manual: bool, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let Some(config) = self.config.clone() else {
            self.changed(cx);
            return;
        };
        self.phase = Phase::Checking;
        self.message = None;
        self.changed(cx);
        let installation = self.installation.clone();
        let work = cx.background_executor().spawn(async move {
            let installation = match installation {
                Some(installation) => installation,
                None => Installation::detect()?,
            };
            let update = app_update::check(&config, &installation)?;
            Ok::<_, String>((installation, update))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((installation, update)) => {
                        this.installation = Some(installation);
                        this.phase = if update.is_some() {
                            Phase::Available
                        } else {
                            Phase::Current
                        };
                        this.pending = update;
                    }
                    Err(error) => {
                        this.phase = if this.pending.is_some() {
                            Phase::Available
                        } else {
                            Phase::Idle
                        };
                        this.message = Some(error.clone());
                        if manual {
                            cx.emit(Event::Error(format!("Couldn’t check for updates: {error}")));
                        }
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
    }
    fn install_remote(
        &mut self,
        update: Update,
        installation: Installation,
        failed: InstallFailed,
        acknowledged: Vec<RunningSession>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if self.busy() {
            return Err("An update is already in progress".into());
        }
        self.pending = Some(update);
        self.installation = Some(installation);
        self.remote_failed = Some(failed);
        self.acknowledged = acknowledged;
        // Pinned: the requester was told this version and judges the outcome by it.
        self.start_install(false, cx);
        Ok(())
    }
    /// Tells a remote requester why the install ended before the restart.
    fn tell_remote_failure(&mut self, error: &str, cx: &mut Context<Self>) {
        if let Some(failed) = self.remote_failed.take() {
            failed(error.to_owned(), cx);
        }
    }
    /// The UI install: re-reads the feed first so a release newer than `pending` wins.
    fn install(&mut self, acknowledged: Vec<RunningSession>, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        self.acknowledged = acknowledged;
        #[cfg(test)]
        {
            self.installs += 1;
        }
        self.start_install(true, cx);
    }
    /// Asked after the download, right before the point of no return: work may have started
    /// meanwhile. Answers `reply` with whether to go on.
    fn confirm_install(
        acknowledged: Vec<RunningSession>,
        reply: async_channel::Sender<bool>,
        cx: &mut App,
    ) {
        super::update_gate::ask(cx, None, acknowledged, move |accepted, _| {
            let _ = reply.try_send(accepted.is_some());
        });
    }
    fn apply_progress(&mut self, progress: Progress, cx: &mut Context<Self>) {
        match progress {
            Progress::Started => self.rechecking = false,
            Progress::Download(n, total) => {
                self.progress = total
                    .filter(|n| *n > 0)
                    .map(|total| (n.saturating_mul(100) / total).min(100))
            }
            Progress::Installing => self.phase = Phase::Installing,
            Progress::Confirm(reply) => {
                Updater::confirm_install(self.acknowledged.clone(), reply, cx)
            }
        }
        self.changed(cx);
    }
    /// The install ended before the restart. A declined one stays available and is no error.
    fn install_failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.phase = Phase::Available;
        if error != DECLINED {
            self.message = Some(error.clone());
            cx.emit(Event::Error(format!(
                "Couldn’t install the update: {error}"
            )));
        }
        self.changed(cx);
        self.tell_remote_failure(&error, cx);
        if self.gate.lock().unwrap_or_else(|e| e.into_inner()).quitting {
            crate::quit(cx);
        }
    }
    fn start_install(&mut self, recheck: bool, cx: &mut Context<Self>) {
        if self.busy() {
            return;
        }
        let (Some(pending), Some(installation)) = (self.pending.clone(), self.installation.clone())
        else {
            return;
        };
        let config = self.config.clone().filter(|_| recheck);
        self.phase = Phase::Downloading;
        self.progress = Some(0);
        self.rechecking = config.is_some();
        self.message = None;
        self.changed(cx); // Close the double-click window before starting I/O.
        let store = self.store.clone();
        let gate = self.gate.clone();
        let (tx, rx) = async_channel::bounded(8);
        let work = cx.background_executor().spawn(async move {
            let update = choose(pending, config.as_ref(), |pending, config| {
                app_update::latest(pending, config, &installation)
            });
            let _ = tx.send_blocking(Progress::Started);
            let result = (|| {
                let mut last_bytes = 0;
                let mut last_percent = None;
                let download = update.download(|n, total| {
                    let percent = total.filter(|n| *n > 0).map(|total| n.saturating_mul(100) / total);
                    if (percent.is_some() && percent != last_percent) || (percent.is_none() && n - last_bytes >= 1024 * 1024) {
                        let _ = tx.try_send(Progress::Download(n, total));
                        last_percent = percent;
                        last_bytes = n;
                    }
                })?;
                confirm_before_install(&tx)?;
                {
                    let mut gate = gate.lock().unwrap_or_else(|e| e.into_inner());
                    if gate.quitting { return Err("The app is closing; the update was not installed".into()); }
                    gate.installing = true;
                }
                let _ = tx.send_blocking(Progress::Installing);
                locked(&store)?.save_setting(
                    "monocode.installedUpdate",
                    &serde_json::json!({"version": update.version.to_string()}).to_string(),
                )?;
                locked(&store)?.save_setting("monocode.nativeReleaseNotes", &serde_json::json!({"version": update.version.to_string(), "body": update.notes}).to_string())?;
                update.install(download, &installation)
            })();
            if result.is_err() {
                let _ = locked(&store)
                    .and_then(|store| store.save_setting("monocode.installedUpdate", ""));
            }
            gate.lock().unwrap_or_else(|e| e.into_inner()).installing = false;
            (update, result)
        });
        cx.spawn(async move |this, cx| {
            while let Ok(progress) = rx.recv().await {
                if this
                    .update(cx, |this, cx| this.apply_progress(progress, cx))
                    .is_err()
                {
                    return;
                }
            }
            let (update, result) = work.await;
            let _ = this.update(cx, |this, cx| {
                // Offer what was actually attempted, so a failed install retries the latest.
                this.rechecking = false;
                this.pending = Some(update);
                match result {
                    Ok(Installed::Relaunch(path)) => {
                        if let Err(error) = app_update::schedule_relaunch(&path) {
                            this.phase = Phase::Available;
                            this.message = Some(error.clone());
                            cx.emit(Event::Error(format!(
                                "The update was installed. Reopen zptah to finish: {error}"
                            )));
                            this.changed(cx);
                            this.tell_remote_failure(
                                &format!(
                                    "installed, but the app could not restart itself: {error}"
                                ),
                                cx,
                            );
                        } else {
                            crate::quit(cx);
                        }
                    }
                    Ok(Installed::InstallerStarted) => crate::quit(cx),
                    Err(error) => this.install_failed(error, cx),
                }
            });
        })
        .detach();
    }
}
/// Runs on the install thread once the download is done: asks the UI thread whether work that
/// started meanwhile may be interrupted, and waits for the answer. `Err(DECLINED)` stops the
/// install before anything is written.
fn confirm_before_install(tx: &async_channel::Sender<Progress>) -> Result<(), String> {
    let (reply, answer) = async_channel::bounded(1);
    let _ = tx.send_blocking(Progress::Confirm(reply));
    if answer.recv_blocking().unwrap_or(false) {
        Ok(())
    } else {
        Err(DECLINED.into())
    }
}
/// The update to install: `pending`, or what `latest` finds when the install re-checks the
/// feed first (the UI paths do; a remote install stays pinned to the version it reported).
fn choose<T, C>(pending: T, recheck: Option<&C>, latest: impl FnOnce(T, &C) -> T) -> T {
    match recheck {
        Some(config) => latest(pending, config),
        None => pending,
    }
}
/// What the updater offers right now, for Settings' About button.
pub(crate) enum Action {
    Check,
    Install(String),
    Busy(String),
}
pub(crate) fn action(entity: &Entity<Updater>, cx: &App) -> Action {
    let state = entity.read(cx);
    match state.phase {
        Phase::Available => Action::Install(state.pending_version()),
        Phase::Checking => Action::Busy("Checking…".into()),
        Phase::Downloading | Phase::Installing => Action::Busy(state.progress_label()),
        Phase::Disabled | Phase::Idle | Phase::Current => Action::Check,
    }
}
/// A line for Settings' About description: the last check's outcome, if any.
pub(crate) fn status(entity: &Entity<Updater>, cx: &App) -> Option<String> {
    let state = entity.read(cx);
    match state.phase {
        Phase::Current => Some("You’re on the latest version.".into()),
        Phase::Checking => None,
        _ => state.message.clone(),
    }
}
/// The UI install (sidebar, Settings): asks first when work is running, and does nothing when
/// the user cancels.
pub(crate) fn install(entity: &Entity<Updater>, cx: &mut App) {
    let entity = entity.clone();
    super::update_gate::ask(cx, None, Vec::new(), move |accepted, cx| {
        if let Some(accepted) = accepted {
            entity.update(cx, |updater, cx| updater.install(accepted, cx));
        }
    });
}
impl Updater {
    fn pending_version(&self) -> String {
        self.pending
            .as_ref()
            .map(|u| u.version.to_string())
            .unwrap_or_default()
    }
    fn progress_label(&self) -> String {
        match self.phase {
            Phase::Installing => "Installing…".into(),
            _ if self.rechecking => "Checking for the latest version…".into(),
            _ => self
                .progress
                .map(|p| format!("Downloading {p}%"))
                .unwrap_or_else(|| "Downloading…".into()),
        }
    }
}
/// The sidebar's update card, shown while an update is available or installing.
pub(crate) fn row(entity: Entity<Updater>, cx: &App) -> gpui::AnyElement {
    let action = entity.clone();
    let state = entity.read(cx);
    let available = state.phase == Phase::Available;
    // Matches the desktop footer: one line, accent-tinted until it starts.
    let label = if available {
        format!("Update to v{}", state.pending_version())
    } else {
        state.progress_label()
    };
    let glyph = if state.busy() {
        crate::pulse::looping(
            "native-update-spin",
            Duration::from_secs(1),
            crate::pulse::Rate::Smooth,
            |turn| {
                icon("loader", 16.)
                    .text_color(content(0.7))
                    .with_transformation(gpui::Transformation::rotate(gpui::percentage(turn)))
            },
        )
        .into_any_element()
    } else {
        icon("arrow-down-circle", 16.)
            .text_color(accent_color())
            .into_any_element()
    };
    let mut button = div()
        .id("install-native-update")
        .role(Role::Button)
        .rounded(px(8.))
        .px_2()
        .py_2()
        .flex()
        .items_center()
        .gap_2()
        .bg(if available {
            accent_color().opacity(0.15)
        } else {
            content(0.05)
        })
        .text_color(content(if available { 1. } else { 0.75 }))
        .child(
            div()
                .size(px(18.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .child(glyph),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(12.))
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child(label),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.))
                        .text_color(content(0.4))
                        .child(format!("v{}", version())),
                ),
        );
    if available {
        button = button
            .aria_label("Install the available zptah update")
            .cursor_pointer()
            .hover(|s| s.bg(accent_color().opacity(0.2)))
            .on_click(move |_, _, cx| install(&action, cx));
    }
    div()
        .id("native-app-update")
        .role(Role::Status)
        .w_full()
        .min_w_0()
        .child(button)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::{
        cell::RefCell,
        rc::Rc,
        sync::{Arc, Mutex},
    };
    #[test]
    fn ui_installs_recheck_the_feed_and_remote_installs_stay_pinned() {
        let mut calls = 0;
        // Pending v3, the fresh feed says v4.
        let chosen = choose(3, Some(&()), |_, _| {
            calls += 1;
            4
        });
        assert_eq!((chosen, calls), (4, 1));
        let chosen = choose(3, None::<&()>, |_, _| {
            calls += 1;
            4
        });
        assert_eq!((chosen, calls), (3, 1));
    }
    #[gpui::test]
    fn quit_waits_for_installation_to_finish(cx: &mut TestAppContext) {
        let store = Arc::new(Mutex::new(
            monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let updater = cx.update(|cx| shared(store, cx));
        let gate = updater.read_with(cx, |updater, _| updater.gate.clone());
        gate.lock().unwrap().installing = true;
        assert!(cx.update(|cx| defer_quit(cx)));
        assert!(gate.lock().unwrap().quitting);
        gate.lock().unwrap().installing = false;
        assert!(!cx.update(|cx| defer_quit(cx)));
    }
    #[gpui::test]
    fn windows_share_one_updater_and_busy_operations_cannot_start_again(cx: &mut TestAppContext) {
        let store = Arc::new(Mutex::new(
            monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let first = cx.update(|cx| shared(store.clone(), cx));
        let second = cx.update(|cx| shared(store, cx));
        assert_eq!(first.entity_id(), second.entity_id());
        first.update(cx, |view, cx| {
            assert!(!view.actionable());
            for phase in [Phase::Downloading, Phase::Installing] {
                view.phase = phase;
                view.progress = Some(42);
                view.install(Vec::new(), cx);
                view.check(true, cx);
                assert_eq!(view.phase, phase);
                assert_eq!(view.progress, Some(42));
                assert!(view.actionable());
            }
            view.phase = Phase::Current;
            assert!(!view.actionable());
        });
    }
    fn gated(
        cx: &mut TestAppContext,
        running: Vec<RunningSession>,
        builder: bool,
    ) -> (Entity<Updater>, gpui::VisualTestContext) {
        // Without the app's builder the dialog is a platform prompt, answered by its button
        // label; with it, the dialog is the real modal and takes keys.
        if builder {
            cx.update(|cx| cx.set_prompt_builder(crate::modal::prompt_builder));
        }
        let over = super::super::update_gate::RunningOverride::default();
        *over.0.borrow_mut() = running;
        cx.update(|cx| cx.set_global(over));
        let store = Arc::new(Mutex::new(
            monocode_core::store::Store::open(std::path::Path::new(":memory:")).unwrap(),
        ));
        let updater = cx.update(|cx| shared(store, cx));
        let window = cx.add_window(|_, _| gpui::Empty);
        (
            updater,
            gpui::VisualTestContext::from_window(window.into(), cx),
        )
    }
    fn running(id: &str) -> RunningSession {
        RunningSession {
            id: id.into(),
            title: id.into(),
        }
    }
    fn dialog_open(visual: &mut gpui::VisualTestContext) -> bool {
        visual.update(|window, _| window.has_active_prompt())
    }
    fn installs(updater: &Entity<Updater>, visual: &gpui::VisualTestContext) -> usize {
        updater.read_with(visual, |u, _| u.installs)
    }
    #[gpui::test]
    fn the_ui_install_waits_for_the_user_and_cancel_does_not_start_it(cx: &mut TestAppContext) {
        let (updater, mut visual) = gated(cx, vec![running("1")], false);
        visual.update(|_, cx| install(&updater, cx));
        visual.run_until_parked();
        assert!(visual.has_pending_prompt(), "the dialog is up");
        assert_eq!(
            installs(&updater, &visual),
            0,
            "nothing starts before the answer"
        );
        visual.simulate_prompt_answer("Cancel");
        visual.run_until_parked();
        assert_eq!(installs(&updater, &visual), 0);
    }
    #[gpui::test]
    fn enter_on_the_update_dialog_does_not_install(cx: &mut TestAppContext) {
        let (updater, mut visual) = gated(cx, vec![running("1")], true);
        visual.update(|_, cx| install(&updater, cx));
        visual.run_until_parked();
        assert!(dialog_open(&mut visual));
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        assert!(!dialog_open(&mut visual), "enter answered the dialog");
        assert_eq!(
            installs(&updater, &visual),
            0,
            "Cancel is the default answer"
        );
    }
    #[gpui::test]
    fn update_anyway_starts_the_ui_install_and_remembers_what_was_accepted(
        cx: &mut TestAppContext,
    ) {
        let (updater, mut visual) = gated(cx, vec![running("1")], false);
        visual.update(|_, cx| install(&updater, cx));
        visual.run_until_parked();
        assert_eq!(installs(&updater, &visual), 0);
        visual.simulate_prompt_answer("Update anyway");
        visual.run_until_parked();
        assert_eq!(installs(&updater, &visual), 1);
        assert_eq!(
            updater.read_with(&visual, |u, _| u.acknowledged.clone()),
            vec![running("1")]
        );
    }
    #[gpui::test]
    fn no_running_work_installs_without_a_dialog(cx: &mut TestAppContext) {
        let (updater, mut visual) = gated(cx, Vec::new(), false);
        visual.update(|_, cx| install(&updater, cx));
        visual.run_until_parked();
        assert!(!visual.has_pending_prompt());
        assert_eq!(installs(&updater, &visual), 1);
    }
    #[gpui::test]
    fn work_started_during_the_download_asks_again_before_the_install(cx: &mut TestAppContext) {
        let (updater, mut visual) = gated(cx, vec![running("1"), running("2")], false);
        updater.update(&mut visual, |u, _| u.acknowledged = vec![running("1")]);
        let (tx, rx) = async_channel::bounded(8);
        // The install thread asks through the same function start_install uses.
        let thread = std::thread::spawn(move || confirm_before_install(&tx));
        let reply = loop {
            match rx.try_recv() {
                Ok(Progress::Confirm(reply)) => break reply,
                Ok(_) => {}
                Err(_) => std::thread::yield_now(),
            }
        };
        updater.update(&mut visual, |u, cx| {
            u.apply_progress(Progress::Confirm(reply), cx)
        });
        visual.run_until_parked();
        assert!(
            visual.has_pending_prompt(),
            "work 2 started after the first answer"
        );
        visual.simulate_prompt_answer("Cancel");
        visual.run_until_parked();
        assert_eq!(thread.join().unwrap(), Err(DECLINED.to_string()));
    }
    #[gpui::test]
    fn nothing_new_installs_without_asking_again(cx: &mut TestAppContext) {
        let (updater, mut visual) = gated(cx, vec![running("1")], false);
        updater.update(&mut visual, |u, _| u.acknowledged = vec![running("1")]);
        let (tx, rx) = async_channel::bounded(8);
        let thread = std::thread::spawn(move || confirm_before_install(&tx));
        let reply = loop {
            match rx.try_recv() {
                Ok(Progress::Confirm(reply)) => break reply,
                Ok(_) => {}
                Err(_) => std::thread::yield_now(),
            }
        };
        updater.update(&mut visual, |u, cx| {
            u.apply_progress(Progress::Confirm(reply), cx)
        });
        visual.run_until_parked();
        assert!(!visual.has_pending_prompt());
        assert_eq!(thread.join().unwrap(), Ok(()));
    }
    #[gpui::test]
    fn a_declined_install_stays_available_without_an_error(cx: &mut TestAppContext) {
        let (updater, mut visual) = gated(cx, Vec::new(), false);
        let errors = Rc::new(RefCell::new(0));
        let seen = errors.clone();
        let _subscription = visual.update(|_, cx| {
            cx.subscribe(&updater, move |_, event: &Event, _| {
                if matches!(event, Event::Error(_)) {
                    *seen.borrow_mut() += 1;
                }
            })
        });
        let told = Rc::new(RefCell::new(Vec::new()));
        let sink = told.clone();
        updater.update(&mut visual, |u, cx| {
            u.phase = Phase::Downloading;
            u.message = None;
            u.remote_failed = Some(Box::new(move |why, _| sink.borrow_mut().push(why)));
            u.install_failed(DECLINED.into(), cx);
        });
        assert_eq!(
            updater.read_with(&visual, |u, _| (u.phase, u.message.clone())),
            (Phase::Available, None)
        );
        assert_eq!(*errors.borrow(), 0, "a decline is not an error");
        assert_eq!(
            *told.borrow(),
            vec![DECLINED.to_string()],
            "a remote requester hears it"
        );
        updater.update(&mut visual, |u, cx| {
            u.install_failed("disk full".into(), cx)
        });
        assert_eq!(*errors.borrow(), 1);
    }
}
