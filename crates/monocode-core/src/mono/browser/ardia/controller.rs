//! What is shared by every Mono's Ardia upstream: one Ardia runs per machine, so the readiness
//! checks, the on-demand launch, the idle quit, the "who started this Ardia" record and the list
//! of tabs zptah opened are process-wide state (design spec sections 2, 2a, 4).
use super::platform::{Platform, RealPlatform};
use super::runner::{session_params, Cli, Failure, LineStream, ProcessRunner, Runner};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

/// The first Ardia control protocol the backend speaks.
pub const MIN_PROTOCOL: u64 = 4;
/// Minutes without browser use before a zptah-started Ardia is quit (setting, 0 = never).
pub const DEFAULT_IDLE_MINUTES: u32 = 10;
/// The `native_settings` key of the idle quit setting.
pub const IDLE_SETTING_KEY: &str = "ardia-idle-quit-minutes";
/// How long a launched Ardia gets to answer `hello`.
pub const LAUNCH_WAIT: Duration = Duration::from_secs(20);
/// How long tabs stay held without a renewal (renewed on every call).
pub const HOLD_SECONDS: u64 = 120;
/// After a launch, `window.opened` events are ignored this long (Ardia restores its windows).
const RESTORE_GRACE_MS: u64 = 5_000;

// ---- readiness ----

/// The first readiness check that fails, with the step that fixes it (spec section 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    /// Static: no `ardia` command on the PATH.
    CliMissing,
    /// Static: the Ardia application is not installed.
    NotInstalled,
    /// Static: the Mono has no `ardiaProfile`.
    NoProfile,
    /// Live: Ardia is closed and zptah can not open it here (Linux, Windows).
    NotRunning,
    /// Live: zptah launched Ardia and it did not answer in time.
    DidntStart,
    /// Live: Ardia's Control setting is off.
    ControlOff,
    /// Live: Ardia speaks an older control protocol.
    ProtocolTooOld(u64),
    /// Live: the Mono's profile is not in `profiles.list`.
    ProfileMissing,
    /// Live: something else went wrong (a reply that could not be read, a refusal).
    Unexpected(String),
}
impl Readiness {
    /// The name of the check.
    pub fn check(&self) -> &'static str {
        match self {
            Self::CliMissing => "ardia on PATH",
            Self::NotInstalled => "Ardia installed",
            Self::NoProfile | Self::ProfileMissing => "Ardia profile",
            Self::NotRunning | Self::DidntStart => "Ardia reachable",
            Self::ControlOff => "Control on",
            Self::ProtocolTooOld(_) => "Protocol",
            Self::Unexpected(_) => "Ardia reply",
        }
    }
    /// The step that fixes it.
    pub fn fix(&self) -> &'static str {
        match self {
            Self::CliMissing => "Install Ardia, then turn on Settings > Plugins > Command line.",
            Self::NotInstalled => "Install Ardia.",
            Self::NoProfile | Self::ProfileMissing => "Pick an Ardia profile.",
            Self::NotRunning => "Open Ardia.",
            Self::DidntStart => "Open Ardia yourself, then try again.",
            Self::ControlOff => {
                "In Ardia open Settings > Advanced and turn on \"Let the ardia command control Ardia\"."
            }
            Self::ProtocolTooOld(_) => "Update Ardia.",
            Self::Unexpected(_) => "Update Ardia, then try again.",
        }
    }
    /// What went wrong, in a sentence.
    pub fn problem(&self) -> String {
        match self {
            Self::CliMissing => "The `ardia` command was not found.".into(),
            Self::NotInstalled => "Ardia is not installed.".into(),
            Self::NoProfile => "No Ardia profile is chosen for this Mono.".into(),
            Self::NotRunning => "Ardia is not running.".into(),
            Self::DidntStart => "Ardia didn't start.".into(),
            Self::ControlOff => "Ardia's Control is off.".into(),
            Self::ProtocolTooOld(found) => format!(
                "This Ardia speaks control protocol {found}; zptah needs {MIN_PROTOCOL} or newer."
            ),
            Self::ProfileMissing => "This Mono's Ardia profile no longer exists.".into(),
            Self::Unexpected(why) => format!("Ardia answered something unexpected ({why})."),
        }
    }
    /// The tool error and the readiness row: the check, the problem and the fix.
    pub fn message(&self) -> String {
        format!("{} [{}] {}", self.problem(), self.check(), self.fix())
    }
    /// Whether the check can be run with Ardia closed (spec section 2: static checks).
    pub fn is_static(&self) -> bool {
        matches!(
            self,
            Self::CliMissing | Self::NotInstalled | Self::NoProfile
        )
    }
}

/// What the last good live check saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveInfo {
    pub version: String,
    pub protocol: u64,
}

/// A profile of `profiles.list`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArdiaProfile {
    pub id: String,
    pub name: String,
}

/// Who started the running Ardia.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// Closed, or not seen yet.
    Nobody,
    /// zptah launched it (and may quit it).
    Zptah,
    /// It was running before zptah needed it, or the user took it over. Never quit by zptah.
    User,
}

/// What the Browser tab shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    /// The first failing check, static first. `None`: nothing failed (Ardia may be closed).
    pub failure: Option<Readiness>,
    /// Ardia answered `hello` just now.
    pub running: bool,
    /// The last good live result, kept while Ardia is closed.
    pub cached: Option<LiveInfo>,
    pub owner: Owner,
}
impl Report {
    /// "Ardia running (started by zptah)" with its Quit button applies.
    pub fn quittable(&self) -> bool {
        self.running && self.owner == Owner::Zptah
    }
}

// ---- the controller ----

struct Inner {
    owner: Owner,
    /// Bumped by every launch and every detected loss of Ardia; a conversation re-runs the live
    /// checks when its own copy is older.
    epoch: u64,
    live: Option<LiveInfo>,
    last_activity_ms: u64,
    in_flight: usize,
    /// `<mono>:<session>` of the turns that used the browser and have not ended.
    busy: HashSet<String>,
    idle_minutes: u32,
    takeover: bool,
    cleanup_due: bool,
    /// Own tabs by `ardia browse` session name, each with the profile it was opened in: a tab of
    /// an old profile is never used again after the Mono picks another.
    sessions: HashMap<String, Vec<(String, String)>>,
    /// Every tab id zptah opened and has not closed, kept in a file.
    persisted: BTreeSet<String>,
    state_file: Option<PathBuf>,
    watch_generation: u64,
    /// Stops the `ardia events` child of the current watcher.
    watcher_stop: Option<Box<dyn Fn() + Send + Sync>>,
    /// The event stream ended while Ardia may still run: nothing says the user is not there, so
    /// there is no idle quit until Ardia is seen closed.
    watch_lost: bool,
    /// Events before this time are the launch's own (Ardia restoring its session).
    events_from_ms: u64,
    /// `window.opened` counts only from here (a session restore opens windows).
    windows_from_ms: u64,
    /// An idle quit or a Quit is running: new calls wait for it.
    quitting: bool,
    /// The last `hello` was answered (cheap to read; the Browser tab and the conversation row use it).
    up: bool,
}

/// See the module docs. One per process in the app ([`controller`]); tests build their own.
pub struct Controller {
    cli: Cli,
    platform: Arc<dyn Platform>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    inner: Mutex<Inner>,
    /// Signalled when a quit ends (`begin_call` waits on it).
    quit_done: Condvar,
    /// One live check or launch at a time, so two Monos never start Ardia twice.
    launch_lock: Mutex<()>,
    launch_wait: Duration,
    poll_every: Duration,
    me: Weak<Controller>,
}

/// Counts a tool call as in flight for as long as it lives.
pub struct CallGuard {
    controller: Arc<Controller>,
}
impl Drop for CallGuard {
    fn drop(&mut self) {
        let mut inner = self.controller.lock();
        inner.in_flight = inner.in_flight.saturating_sub(1);
        inner.last_activity_ms = (self.controller.clock)();
    }
}

impl Controller {
    pub fn new(
        runner: Arc<dyn Runner>,
        platform: Arc<dyn Platform>,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
        launch_wait: Duration,
        poll_every: Duration,
    ) -> Arc<Self> {
        let now = clock();
        Arc::new_cyclic(|me| Self {
            cli: Cli::new(runner),
            platform,
            clock,
            inner: Mutex::new(Inner {
                owner: Owner::Nobody,
                epoch: 1,
                live: None,
                last_activity_ms: now,
                in_flight: 0,
                busy: HashSet::new(),
                idle_minutes: DEFAULT_IDLE_MINUTES,
                takeover: false,
                cleanup_due: true,
                sessions: HashMap::new(),
                persisted: BTreeSet::new(),
                state_file: None,
                watch_generation: 0,
                watcher_stop: None,
                watch_lost: false,
                events_from_ms: 0,
                windows_from_ms: 0,
                quitting: false,
                up: false,
            }),
            quit_done: Condvar::new(),
            launch_lock: Mutex::new(()),
            launch_wait,
            poll_every,
            me: me.clone(),
        })
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn cli(&self) -> &Cli {
        &self.cli
    }
    pub fn platform(&self) -> &Arc<dyn Platform> {
        &self.platform
    }
    fn now(&self) -> u64 {
        (self.clock)()
    }

    // ---- settings ----

    /// The idle quit timeout in minutes (0: never quit on idle).
    pub fn set_idle_minutes(&self, minutes: u32) {
        self.lock().idle_minutes = minutes;
    }
    pub fn idle_minutes(&self) -> u32 {
        self.lock().idle_minutes
    }
    /// Where the opened tab ids are kept. Reads what an earlier run left, so the next live check
    /// closes those tabs.
    pub fn set_state_file(&self, path: PathBuf) {
        let mut inner = self.lock();
        if inner.state_file.as_ref() == Some(&path) {
            return;
        }
        let saved: Vec<String> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok())
            .and_then(|v| serde_json::from_value(v["tabs"].clone()).ok())
            .unwrap_or_default();
        inner.persisted.extend(saved);
        inner.state_file = Some(path);
        inner.cleanup_due = true;
        save_tabs(&inner);
    }

    // ---- static readiness ----

    /// The checks that work with Ardia closed (spec section 2): the command, the application and
    /// a saved profile.
    pub fn static_check(&self, profile: &str) -> Result<(), Readiness> {
        if self.cli.runner().executable().is_none() {
            return Err(Readiness::CliMissing);
        }
        if !self.platform.installed() {
            return Err(Readiness::NotInstalled);
        }
        if profile.trim().is_empty() {
            return Err(Readiness::NoProfile);
        }
        Ok(())
    }

    // ---- live readiness and launch ----

    pub fn epoch(&self) -> u64 {
        self.lock().epoch
    }
    /// A call could not reach Ardia: the next one checks again (and launches it if it is
    /// closed). It does not change who started Ardia: a browse daemon that went away says nothing
    /// about the application, and only `hello` seeing it closed does (`lose_ardia`).
    pub fn note_unreachable(&self) {
        self.lock().epoch += 1;
    }

    fn hello(&self) -> Result<LiveInfo, Failure> {
        let params = json!({"protocol": MIN_PROTOCOL, "caller": {"name": "zptah", "app": "zptah"}});
        let reply = self.cli.call("hello", &params);
        self.lock().up = !matches!(reply, Err(Failure::Unreachable(_) | Failure::NotInstalled));
        let reply = reply?;
        Ok(LiveInfo {
            version: reply["version"].as_str().unwrap_or("").to_owned(),
            protocol: reply["protocol"].as_u64().unwrap_or(0),
        })
    }

    fn readiness_of(failure: Failure) -> Readiness {
        match failure {
            Failure::NotInstalled => Readiness::CliMissing,
            Failure::Unreachable(_) => Readiness::NotRunning,
            Failure::ControlOff => Readiness::ControlOff,
            other => Readiness::Unexpected(other.text()),
        }
    }

    /// The live checks, launching Ardia first when it is closed and the platform allows it
    /// (spec section 2a). Call it from a tool call, never at Mono launch.
    pub fn ensure_live(&self, profile: &str) -> Result<LiveInfo, Readiness> {
        self.check_live(profile, true)
    }

    /// The live checks only when Ardia is already up (the Browser tab's Try again).
    pub fn check_live(&self, profile: &str, launch: bool) -> Result<LiveInfo, Readiness> {
        let _one = self.launch_lock.lock().unwrap_or_else(|e| e.into_inner());
        self.static_check(profile)?;
        let info = match self.hello() {
            Ok(info) => info,
            Err(Failure::Unreachable(_)) => {
                self.lose_ardia();
                if !launch || !self.platform.can_launch() {
                    return Err(Readiness::NotRunning);
                }
                self.launch_and_wait()?
            }
            Err(other) => {
                // Control off or a refusal: Ardia is up (and stays whoever's it was).
                self.mark_running();
                return Err(Self::readiness_of(other));
            }
        };
        self.mark_running();
        if info.protocol < MIN_PROTOCOL {
            return Err(Readiness::ProtocolTooOld(info.protocol));
        }
        let profiles = self.profiles_unlocked()?;
        if !profiles.iter().any(|p| p.id == profile) {
            return Err(Readiness::ProfileMissing);
        }
        self.lock().live = Some(info.clone());
        self.close_leftovers();
        Ok(info)
    }

    /// `hello` found Ardia closed: whoever started it, it is gone, and the next one is new.
    fn lose_ardia(&self) {
        let stop = {
            let mut inner = self.lock();
            if inner.owner != Owner::Nobody {
                inner.epoch += 1;
            }
            inner.owner = Owner::Nobody;
            inner.watch_lost = false;
            inner.watch_generation += 1;
            inner.watcher_stop.take()
        };
        if let Some(stop) = stop {
            stop();
        }
    }

    /// Ardia answered. When zptah has not started it, it is the user's.
    fn mark_running(&self) {
        let mut inner = self.lock();
        if inner.owner == Owner::Nobody {
            inner.owner = Owner::User;
            inner.epoch += 1;
        }
    }

    /// zptah asked the platform to start Ardia: from here it is zptah's, also when it answers
    /// late (after "didn't start") or refuses control.
    fn set_launched(&self) {
        let mut inner = self.lock();
        inner.owner = Owner::Zptah;
        inner.takeover = false;
        inner.watch_lost = false;
        inner.cleanup_due = true;
        inner.events_from_ms = u64::MAX;
        inner.windows_from_ms = u64::MAX;
        inner.epoch += 1;
    }

    fn launch_and_wait(&self) -> Result<LiveInfo, Readiness> {
        if !self.platform.installed() {
            return Err(Readiness::NotInstalled);
        }
        self.platform.launch().map_err(|_| Readiness::DidntStart)?;
        self.set_launched();
        self.start_watcher();
        let started = std::time::Instant::now();
        loop {
            match self.hello() {
                Ok(info) => {
                    self.session_restored();
                    return Ok(info);
                }
                // Up, but refusing: the caller reports it.
                Err(Failure::ControlOff) => {
                    self.session_restored();
                    return Err(Readiness::ControlOff);
                }
                Err(_) => {}
            }
            if started.elapsed() >= self.launch_wait {
                return Err(Readiness::DidntStart);
            }
            std::thread::sleep(self.poll_every);
        }
    }

    /// Ardia answered after the launch: events count from now, windows after a short grace.
    fn session_restored(&self) {
        let now = self.now();
        let mut inner = self.lock();
        inner.events_from_ms = now;
        inner.windows_from_ms = now + RESTORE_GRACE_MS;
    }

    /// Starts Ardia in the background when it is closed (the Browser tab's button). `Ok(true)`:
    /// zptah started it now.
    pub fn start_now(&self) -> Result<bool, Readiness> {
        let _one = self.launch_lock.lock().unwrap_or_else(|e| e.into_inner());
        match self.hello() {
            Ok(_) | Err(Failure::ControlOff) => {
                self.mark_running();
                Ok(false)
            }
            Err(Failure::Unreachable(_)) => {
                if !self.platform.can_launch() {
                    return Err(Readiness::NotRunning);
                }
                self.launch_and_wait().map(|_| true)
            }
            Err(other) => Err(Self::readiness_of(other)),
        }
    }

    /// `profiles.list`, for the picker.
    pub fn profiles(&self) -> Result<Vec<ArdiaProfile>, Readiness> {
        self.profiles_unlocked()
    }
    fn profiles_unlocked(&self) -> Result<Vec<ArdiaProfile>, Readiness> {
        let reply = self
            .cli
            .call("profiles.list", &json!({}))
            .map_err(Self::readiness_of)?;
        Ok(reply
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| {
                Some(ArdiaProfile {
                    id: p["id"].as_str()?.to_owned(),
                    name: p["name"].as_str().unwrap_or("").to_owned(),
                })
            })
            .collect())
    }

    /// What the Browser tab shows for a Mono's profile. Never launches Ardia.
    pub fn report(&self, profile: &str) -> Report {
        let (running, failure) = match self.static_check(profile) {
            Err(why) => (false, Some(why)),
            Ok(()) => match self.check_live(profile, false) {
                Ok(_) => (true, None),
                Err(Readiness::NotRunning) => (false, None),
                Err(why) => (true, Some(why)),
            },
        };
        let inner = self.lock();
        Report {
            failure,
            running,
            cached: inner.live.clone(),
            owner: inner.owner,
        }
    }

    /// Ardia answered the last `hello` and zptah started it: what the conversation row shows.
    /// A lookup only, it never runs `ardia`.
    pub fn owned_and_up(&self) -> bool {
        let inner = self.lock();
        inner.owner == Owner::Zptah && inner.up && !inner.quitting
    }

    /// Whether the event stream is still being read (it ending stops the idle quit).
    pub fn watching(&self) -> bool {
        !self.lock().watch_lost
    }

    pub fn owner(&self) -> Owner {
        self.lock().owner
    }
    pub fn last_live(&self) -> Option<LiveInfo> {
        self.lock().live.clone()
    }

    // ---- the user takes the browser over ----

    /// Starts reading `ardia events`, after the launch set who owns Ardia. The previous watcher,
    /// if any, is stopped.
    fn start_watcher(&self) {
        let (generation, previous) = {
            let mut inner = self.lock();
            inner.watch_generation += 1;
            (inner.watch_generation, inner.watcher_stop.take())
        };
        if let Some(stop) = previous {
            stop();
        }
        let args: Vec<String> = ["events", "app.activated", "window.opened", "--json"]
            .into_iter()
            .map(String::from)
            .collect();
        let Ok(mut stream) = self.cli.runner().watch(&args) else {
            // No way to see the user: lean to the user's.
            self.lock().watch_lost = true;
            return;
        };
        self.lock().watcher_stop = Some(stream.stopper());
        let me = self.me.clone();
        std::thread::spawn(move || {
            while let Some(line) = LineStream::next_line(&mut *stream) {
                let Some(me) = me.upgrade() else {
                    return;
                };
                if me.lock().watch_generation != generation {
                    return;
                }
                me.note_event(&line);
            }
            // The stream ended. If nothing stopped it on purpose, Ardia may still be running
            // with nobody watching for the user: no idle quit until it is seen closed.
            if let Some(me) = me.upgrade() {
                let mut inner = me.lock();
                if inner.watch_generation == generation {
                    inner.watch_lost = true;
                }
            }
        });
    }

    /// One line of the event stream.
    pub(super) fn note_event(&self, line: &str) {
        let Some(kind) = takeover_kind(line) else {
            return;
        };
        let now = self.now();
        let counts = {
            let inner = self.lock();
            match kind {
                "app.activated" => now >= inner.events_from_ms,
                _ => now >= inner.windows_from_ms,
            }
        };
        if counts {
            self.note_user_event();
        }
    }

    /// The user brought Ardia forward or opened a window in it: it is theirs now.
    pub fn note_user_event(&self) {
        let mut inner = self.lock();
        inner.takeover = true;
        if inner.owner == Owner::Zptah {
            inner.owner = Owner::User;
        }
    }

    // ---- activity and idle quit ----

    /// A tool call starts: Ardia is in use until the guard drops. Waits while an idle quit or a
    /// Quit is running (the call then finds Ardia closed and starts it again).
    pub fn begin_call(self: &Arc<Self>, turn: &str) -> CallGuard {
        let mut inner = self.lock();
        let mut waited = 0;
        while inner.quitting && waited < 40 {
            inner = self
                .quit_done
                .wait_timeout(inner, Duration::from_millis(500))
                .unwrap_or_else(|e| e.into_inner())
                .0;
            waited += 1;
        }
        inner.in_flight += 1;
        inner.busy.insert(turn.to_owned());
        inner.last_activity_ms = self.now();
        CallGuard {
            controller: self.clone(),
        }
    }
    /// A Mono turn starts or ends: it no longer counts as using the browser.
    pub fn turn_ended(&self, turn: &str) {
        let mut inner = self.lock();
        inner.busy.remove(turn);
        inner.last_activity_ms = self.now();
    }

    /// One look at the idle rule. Returns what it did.
    pub fn idle_tick(&self) -> IdleOutcome {
        let minutes = {
            let inner = self.lock();
            if inner.owner != Owner::Zptah || inner.takeover || inner.watch_lost {
                return IdleOutcome::NotOurs;
            }
            if inner.idle_minutes == 0 {
                return IdleOutcome::Disabled;
            }
            inner.idle_minutes
        };
        {
            let inner = self.lock();
            if inner.in_flight > 0 || !inner.busy.is_empty() {
                return IdleOutcome::Busy;
            }
        }
        if !self.is_idle(minutes) {
            return IdleOutcome::Waiting;
        }
        // Ardia shown by the user, or one that can not be read, is the user's.
        if self.platform.is_hidden() != Some(true) {
            self.lock().owner = Owner::User;
            return IdleOutcome::NotOurs;
        }
        // Ardia asks before quitting: the user chose that, so it is theirs. When the setting can
        // not be read, nothing is quit.
        match self
            .cli
            .call("settings.get", &json!({"keys": ["warnBeforeQuitting"]}))
        {
            Ok(settings) if settings["warnBeforeQuitting"] == true => {
                self.lock().owner = Owner::User;
                return IdleOutcome::NotOurs;
            }
            Ok(_) => {}
            Err(_) => return IdleOutcome::Waiting,
        }
        match self.quit(Some(minutes)) {
            Ok(()) => IdleOutcome::Quit,
            Err(QuitRefused::Busy) => IdleOutcome::Busy,
            Err(_) => IdleOutcome::Waiting,
        }
    }

    /// No call in flight, no turn that used the browser, and quiet for `minutes`.
    fn is_idle(&self, minutes: u32) -> bool {
        let inner = self.lock();
        Self::idle_in(&inner, self.now(), minutes)
    }
    fn idle_in(inner: &Inner, now: u64, minutes: u32) -> bool {
        inner.in_flight == 0
            && inner.busy.is_empty()
            && now.saturating_sub(inner.last_activity_ms) >= u64::from(minutes) * 60_000
    }

    /// Closes the session tabs and quits Ardia, at once. Only an Ardia zptah started.
    pub fn quit_now(&self) -> Result<(), String> {
        self.quit(None).map_err(|why| match why {
            QuitRefused::NotOurs => {
                "Ardia was not started by zptah, so zptah does not quit it.".to_owned()
            }
            QuitRefused::Busy => "Ardia is in use right now.".to_owned(),
            QuitRefused::Failed(why) => why,
        })
    }

    /// The one quit. With `idle: Some(minutes)` the idle rule is checked again under the lock,
    /// right before `quitting` is set, so a call that started meanwhile is never cut off; a call
    /// that starts later waits for the quit to end.
    fn quit(&self, idle: Option<u32>) -> Result<(), QuitRefused> {
        let _one = self.launch_lock.lock().unwrap_or_else(|e| e.into_inner());
        {
            let mut inner = self.lock();
            if inner.owner != Owner::Zptah {
                return Err(QuitRefused::NotOurs);
            }
            if let Some(minutes) = idle {
                if inner.takeover || !Self::idle_in(&inner, self.now(), minutes) {
                    return Err(QuitRefused::Busy);
                }
            }
            inner.quitting = true;
        }
        let result = self.quit_locked();
        self.lock().quitting = false;
        self.quit_done.notify_all();
        result.map_err(QuitRefused::Failed)
    }
    fn quit_locked(&self) -> Result<(), String> {
        self.close_all_tabs();
        self.platform.quit()?;
        // Ardia needs a few seconds to go (about 7 s measured): wait, so a late `hello` is not
        // mistaken for a user's Ardia.
        let started = std::time::Instant::now();
        while started.elapsed() < self.launch_wait {
            if matches!(self.hello(), Err(Failure::Unreachable(_))) {
                break;
            }
            std::thread::sleep(self.poll_every);
        }
        self.lose_ardia();
        Ok(())
    }

    /// The app quits: closes every tab zptah opened and, when zptah started Ardia, quits it.
    /// Bounded by the caller (a hung Ardia must not hold the app).
    pub fn shutdown(&self) {
        let _one = self.launch_lock.lock().unwrap_or_else(|e| e.into_inner());
        let (stop, ours) = {
            let mut inner = self.lock();
            inner.watch_generation += 1;
            (
                inner.watcher_stop.take(),
                inner.owner == Owner::Zptah && !inner.takeover,
            )
        };
        if let Some(stop) = stop {
            stop();
        }
        self.close_all_tabs();
        if ours && self.platform.is_hidden() == Some(true) {
            let _ = self.platform.quit();
        }
    }

    // ---- tabs zptah opened ----

    pub fn add_tab(&self, session: &str, profile: &str, id: &str) {
        let mut inner = self.lock();
        let tabs = inner.sessions.entry(session.to_owned()).or_default();
        if !tabs.iter().any(|(t, _)| t == id) {
            tabs.push((id.to_owned(), profile.to_owned()));
        }
        inner.persisted.insert(id.to_owned());
        save_tabs(&inner);
    }
    pub fn remove_tab(&self, session: &str, id: &str) {
        let mut inner = self.lock();
        if let Some(tabs) = inner.sessions.get_mut(session) {
            tabs.retain(|(t, _)| t != id);
            if tabs.is_empty() {
                inner.sessions.remove(session);
            }
        }
        inner.persisted.remove(id);
        save_tabs(&inner);
    }
    /// Every tab zptah opened for `session`, in any profile, oldest first (what ending the
    /// session closes).
    pub fn session_tabs(&self, session: &str) -> Vec<String> {
        self.lock()
            .sessions
            .get(session)
            .map(|tabs| tabs.iter().map(|(t, _)| t.clone()).collect())
            .unwrap_or_default()
    }
    /// The tabs zptah opened for `session` in `profile`: the only ones it may use.
    pub fn session_tabs_in(&self, session: &str, profile: &str) -> Vec<String> {
        self.lock()
            .sessions
            .get(session)
            .map(|tabs| {
                tabs.iter()
                    .filter(|(_, p)| p == profile)
                    .map(|(t, _)| t.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn owns_tab(&self, session: &str, profile: &str, id: &str) -> bool {
        self.lock()
            .sessions
            .get(session)
            .is_some_and(|tabs| tabs.iter().any(|(t, p)| t == id && p == profile))
    }
    /// The sessions that have tabs.
    pub fn sessions(&self) -> Vec<String> {
        self.lock().sessions.keys().cloned().collect()
    }
    /// The ids of every tab Ardia has (`tabs.list`), or `None` when that can not be read.
    pub fn tab_ids(&self) -> Option<HashSet<String>> {
        let reply = self.cli.call("tabs.list", &json!({})).ok()?;
        Some(
            reply
                .as_array()?
                .iter()
                .filter_map(|t| t["id"].as_str().map(str::to_owned))
                .collect(),
        )
    }
    /// A `tabs open` that did not answer may have made a tab: closes any tab of `profile` that was
    /// not in `before`. Without a list from before there is nothing to compare, so nothing is closed.
    pub fn close_new_tabs(&self, session: &str, profile: &str, before: Option<HashSet<String>>) {
        let Some(before) = before else {
            return;
        };
        let Ok(reply) = self.cli.call("tabs.list", &json!({})) else {
            return;
        };
        for tab in reply.as_array().into_iter().flatten() {
            let (Some(id), Some(p)) = (tab["id"].as_str(), tab["profile"].as_str()) else {
                continue;
            };
            if p == profile && !before.contains(id) {
                self.note_possible_tab(id);
                let _ = self.close_tab(session, id);
            }
        }
    }
    /// Remembers a tab id that may exist (an open that timed out) so the next live check closes it.
    pub fn note_possible_tab(&self, id: &str) {
        let mut inner = self.lock();
        inner.persisted.insert(id.to_owned());
        inner.cleanup_due = true;
        save_tabs(&inner);
    }

    /// Closes one tab zptah opened. A tab Ardia no longer knows counts as closed.
    pub fn close_tab(&self, session: &str, id: &str) -> Result<(), Failure> {
        match self.cli.tabs_close(id) {
            Ok(()) | Err(Failure::Refused { .. }) => {
                self.remove_tab(session, id);
                Ok(())
            }
            Err(other) => Err(other),
        }
    }

    /// Ends one browse session: releases its holds and closes the tabs zptah opened for it.
    pub fn end_session(&self, session: &str) {
        let tabs = self.session_tabs(session);
        if tabs.is_empty() {
            return;
        }
        let _ = self.cli.browse(session, &["close"]);
        let _ = self
            .cli
            .call("tabs.release", &session_params(session, json!({})));
        for id in tabs {
            if self.close_tab(session, &id).is_err() {
                // Ardia is gone: the ids stay in the file for the next live check.
                break;
            }
        }
        let mut inner = self.lock();
        if inner.sessions.get(session).is_some_and(|t| t.is_empty()) {
            inner.sessions.remove(session);
        }
    }

    /// Closes every tab zptah opened, in every session.
    pub fn close_all_tabs(&self) {
        for session in self.sessions() {
            self.end_session(&session);
        }
    }

    /// Tabs an earlier run left, or Ardia restored after a relaunch: closed once per launch.
    fn close_leftovers(&self) {
        let leftovers: Vec<String> = {
            let mut inner = self.lock();
            if !inner.cleanup_due {
                return;
            }
            inner.cleanup_due = false;
            let tracked: HashSet<&String> =
                inner.sessions.values().flatten().map(|(t, _)| t).collect();
            inner
                .persisted
                .iter()
                .filter(|id| !tracked.contains(id))
                .cloned()
                .collect()
        };
        for id in leftovers {
            match self.cli.tabs_close(&id) {
                Ok(()) | Err(Failure::Refused { .. }) => {
                    let mut inner = self.lock();
                    inner.persisted.remove(&id);
                    save_tabs(&inner);
                }
                Err(_) => {
                    self.lock().cleanup_due = true;
                    return;
                }
            }
        }
    }
}

/// What one [`Controller::idle_tick`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleOutcome {
    /// Ardia is closed, or it is the user's.
    NotOurs,
    Disabled,
    /// A tool call or a Mono turn that used the browser is running.
    Busy,
    /// Not idle long enough yet.
    Waiting,
    Quit,
}

/// The name of an `ardia events` line that means the user is at the browser.
fn takeover_kind(line: &str) -> Option<&'static str> {
    let event = serde_json::from_str::<Value>(line).ok()?;
    match event["event"].as_str()? {
        "app.activated" => Some("app.activated"),
        "window.opened" => Some("window.opened"),
        _ => None,
    }
}
/// An `ardia events` line that means the user is at the browser.
pub fn is_takeover_event(line: &str) -> bool {
    takeover_kind(line).is_some()
}

/// Why a quit did not run.
enum QuitRefused {
    NotOurs,
    Busy,
    Failed(String),
}

/// Writes the tab ids through a temporary file, so a crash never leaves half a file.
fn save_tabs(inner: &Inner) {
    let Some(path) = &inner.state_file else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tabs: Vec<&String> = inner.persisted.iter().collect();
    let temp = path.with_extension("json.tmp");
    if std::fs::write(&temp, json!({"tabs": tabs}).to_string()).is_ok() {
        let _ = std::fs::rename(&temp, path);
    }
}

/// The idle quit minutes saved in the store (`native_settings`), or the default.
pub fn idle_minutes_in(database: &std::path::Path) -> u32 {
    rusqlite::Connection::open_with_flags(database, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT value FROM native_settings WHERE key=?1",
                [IDLE_SETTING_KEY],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(DEFAULT_IDLE_MINUTES)
}

// ---- the app's controller ----

static PLATFORM: OnceLock<Arc<dyn Platform>> = OnceLock::new();
static CONTROLLER: OnceLock<Arc<Controller>> = OnceLock::new();

/// The app gives the platform that can quit and hide Ardia (macOS: through AppKit, which needs
/// no Apple Events permission). Call it before the first use of [`controller`].
pub fn set_platform(platform: Arc<dyn Platform>) {
    let _ = PLATFORM.set(platform);
}

/// The process-wide controller: the real `ardia` command, the platform, and a thread that applies
/// the idle quit.
pub fn controller() -> &'static Arc<Controller> {
    CONTROLLER.get_or_init(|| {
        let platform = PLATFORM
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(RealPlatform));
        let controller = Controller::new(
            Arc::new(ProcessRunner::real()),
            platform,
            Arc::new(crate::time::now_ms),
            LAUNCH_WAIT,
            Duration::from_millis(400),
        );
        let weak = Arc::downgrade(&controller);
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(15));
            let Some(controller) = weak.upgrade() else {
                return;
            };
            controller.idle_tick();
        });
        controller
    })
}

/// The app is quitting: closes the tabs zptah opened and quits an Ardia zptah started, waiting at
/// most `limit`. Does nothing when Ardia was never used in this run.
pub fn shutdown(limit: Duration) {
    let Some(controller) = CONTROLLER.get().cloned() else {
        return;
    };
    let (done, wait) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        controller.shutdown();
        let _ = done.send(());
    });
    let _ = wait.recv_timeout(limit);
}
