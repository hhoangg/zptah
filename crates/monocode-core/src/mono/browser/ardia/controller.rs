//! What is shared by every Mono's Ardia upstream: one Ardia runs per machine, so the readiness
//! checks, the on-demand launch, the idle quit, the "who started this Ardia" record and the list
//! of tabs zptah opened are process-wide state (design spec sections 2, 2a, 4).
use super::platform::{Platform, RealPlatform};
use super::runner::{session_params, Cli, Failure, LineStream, ProcessRunner, Runner};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
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
    /// Own tabs by `ardia browse` session name.
    sessions: HashMap<String, Vec<String>>,
    /// Every tab id zptah opened and has not closed, kept in a file.
    persisted: BTreeSet<String>,
    state_file: Option<PathBuf>,
    watch_generation: u64,
}

/// See the module docs. One per process in the app ([`controller`]); tests build their own.
pub struct Controller {
    cli: Cli,
    platform: Arc<dyn Platform>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    inner: Mutex<Inner>,
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
            }),
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
    /// A call found Ardia gone: the next one checks again, and launches it if it can.
    pub fn note_unreachable(&self) {
        let mut inner = self.lock();
        inner.epoch += 1;
        inner.owner = Owner::Nobody;
        inner.watch_generation += 1;
    }

    fn hello(&self) -> Result<LiveInfo, Failure> {
        let params = json!({"protocol": MIN_PROTOCOL, "caller": {"name": "zptah", "app": "zptah"}});
        let reply = self.cli.call("hello", &params)?;
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
        let mut launched = false;
        let info = match self.hello() {
            Ok(info) => info,
            Err(Failure::Unreachable(_)) => {
                self.lose_ardia();
                if !launch {
                    return Err(Readiness::NotRunning);
                }
                if !self.platform.can_launch() {
                    return Err(Readiness::NotRunning);
                }
                launched = true;
                self.launch_and_wait()?
            }
            Err(other) => {
                // Control off or a refusal: Ardia is up, so it is not ours to have started.
                self.mark_running(false);
                return Err(Self::readiness_of(other));
            }
        };
        self.mark_running(launched);
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

    fn lose_ardia(&self) {
        let mut inner = self.lock();
        if inner.owner != Owner::Nobody {
            inner.epoch += 1;
        }
        inner.owner = Owner::Nobody;
        inner.watch_generation += 1;
    }

    /// Ardia answered. When zptah did not just launch it, it is the user's.
    fn mark_running(&self, launched: bool) {
        let mut inner = self.lock();
        if launched {
            inner.owner = Owner::Zptah;
            inner.takeover = false;
            inner.cleanup_due = true;
            inner.epoch += 1;
        } else if inner.owner == Owner::Nobody {
            inner.owner = Owner::User;
            inner.epoch += 1;
        }
    }

    fn launch_and_wait(&self) -> Result<LiveInfo, Readiness> {
        if !self.platform.installed() {
            return Err(Readiness::NotInstalled);
        }
        self.platform.launch().map_err(|_| Readiness::DidntStart)?;
        let started = std::time::Instant::now();
        loop {
            match self.hello() {
                Ok(info) => {
                    self.start_watcher();
                    return Ok(info);
                }
                // Up, but refusing: the caller reports it.
                Err(Failure::ControlOff) => {
                    self.start_watcher();
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

    /// Starts Ardia in the background when it is closed (the Browser tab's button). `Ok(true)`:
    /// zptah started it now.
    pub fn start_now(&self) -> Result<bool, Readiness> {
        let _one = self.launch_lock.lock().unwrap_or_else(|e| e.into_inner());
        match self.hello() {
            Ok(_) | Err(Failure::ControlOff) => {
                self.mark_running(false);
                Ok(false)
            }
            Err(Failure::Unreachable(_)) => {
                if !self.platform.can_launch() {
                    return Err(Readiness::NotRunning);
                }
                self.launch_and_wait().map(|_| {
                    self.mark_running(true);
                    true
                })
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

    pub fn owner(&self) -> Owner {
        self.lock().owner
    }
    pub fn last_live(&self) -> Option<LiveInfo> {
        self.lock().live.clone()
    }

    // ---- the user takes the browser over ----

    fn start_watcher(&self) {
        let Some(me) = self.me.upgrade() else {
            return;
        };
        let generation = {
            let mut inner = self.lock();
            inner.watch_generation += 1;
            inner.watch_generation
        };
        let args: Vec<String> = ["events", "app.activated", "window.opened", "--json"]
            .into_iter()
            .map(String::from)
            .collect();
        let Ok(mut stream) = self.cli.runner().watch(&args) else {
            return;
        };
        std::thread::spawn(move || {
            while let Some(line) = LineStream::next_line(&mut *stream) {
                if me.lock().watch_generation != generation {
                    return;
                }
                if is_takeover_event(&line) {
                    me.note_user_event();
                }
            }
        });
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

    /// A tool call starts: Ardia is in use until the guard drops.
    pub fn begin_call(self: &Arc<Self>, turn: &str) -> CallGuard {
        let mut inner = self.lock();
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
        let (minutes, idle_ms) = {
            let inner = self.lock();
            if inner.owner != Owner::Zptah {
                return IdleOutcome::NotOurs;
            }
            if inner.takeover {
                return IdleOutcome::NotOurs;
            }
            if inner.idle_minutes == 0 {
                return IdleOutcome::Disabled;
            }
            if inner.in_flight > 0 || !inner.busy.is_empty() {
                return IdleOutcome::Busy;
            }
            (
                inner.idle_minutes,
                self.now().saturating_sub(inner.last_activity_ms),
            )
        };
        if idle_ms < u64::from(minutes) * 60_000 {
            return IdleOutcome::Waiting;
        }
        // Ardia shown by the user, or one that can not be read, is the user's.
        if self.platform.is_hidden() != Some(true) {
            self.lock().owner = Owner::User;
            return IdleOutcome::NotOurs;
        }
        // Ardia asks before quitting: the user chose that, so it is theirs.
        if let Ok(settings) = self
            .cli
            .call("settings.get", &json!({"keys": ["warnBeforeQuitting"]}))
        {
            if settings["warnBeforeQuitting"] == true {
                self.lock().owner = Owner::User;
                return IdleOutcome::NotOurs;
            }
        }
        match self.quit_now() {
            Ok(()) => IdleOutcome::Quit,
            Err(_) => IdleOutcome::Waiting,
        }
    }

    /// Closes the session tabs and quits Ardia, at once. Only an Ardia zptah started.
    pub fn quit_now(&self) -> Result<(), String> {
        let _one = self.launch_lock.lock().unwrap_or_else(|e| e.into_inner());
        if self.lock().owner != Owner::Zptah {
            return Err("Ardia was not started by zptah, so zptah does not quit it.".into());
        }
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

    // ---- tabs zptah opened ----

    pub fn add_tab(&self, session: &str, id: &str) {
        let mut inner = self.lock();
        let tabs = inner.sessions.entry(session.to_owned()).or_default();
        if !tabs.iter().any(|t| t == id) {
            tabs.push(id.to_owned());
        }
        inner.persisted.insert(id.to_owned());
        save_tabs(&inner);
    }
    pub fn remove_tab(&self, session: &str, id: &str) {
        let mut inner = self.lock();
        if let Some(tabs) = inner.sessions.get_mut(session) {
            tabs.retain(|t| t != id);
            if tabs.is_empty() {
                inner.sessions.remove(session);
            }
        }
        inner.persisted.remove(id);
        save_tabs(&inner);
    }
    /// The tabs zptah opened for `session`, oldest first.
    pub fn session_tabs(&self, session: &str) -> Vec<String> {
        self.lock()
            .sessions
            .get(session)
            .cloned()
            .unwrap_or_default()
    }
    pub fn owns_tab(&self, session: &str, id: &str) -> bool {
        self.lock()
            .sessions
            .get(session)
            .is_some_and(|tabs| tabs.iter().any(|t| t == id))
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
        let sessions: Vec<String> = self.lock().sessions.keys().cloned().collect();
        for session in sessions {
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
            let tracked: HashSet<&String> = inner.sessions.values().flatten().collect();
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

/// An `ardia events` line that means the user is at the browser.
pub fn is_takeover_event(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v["event"].as_str().map(str::to_owned))
        .is_some_and(|e| matches!(e.as_str(), "app.activated" | "window.opened"))
}

fn save_tabs(inner: &Inner) {
    let Some(path) = &inner.state_file else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let tabs: Vec<&String> = inner.persisted.iter().collect();
    let _ = std::fs::write(path, json!({"tabs": tabs}).to_string());
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

/// The process-wide controller: the real `ardia` command, the real platform, and a thread that
/// applies the idle quit.
pub fn controller() -> &'static Arc<Controller> {
    static CONTROLLER: OnceLock<Arc<Controller>> = OnceLock::new();
    CONTROLLER.get_or_init(|| {
        let controller = Controller::new(
            Arc::new(ProcessRunner::real()),
            Arc::new(RealPlatform),
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
