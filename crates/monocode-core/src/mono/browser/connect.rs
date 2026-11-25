//! The public face of the browser link layer for the app (spec 12.3). It owns the link manager and
//! the listener for the whole app lifetime, watches the link events, and answers the questions the
//! Connect card asks: which browsers and profiles exist, is the manifest written, is a pairing
//! open, is a browser connected, did a protocol mismatch happen.
//!
//! Everything that touches the outside world is injected through [`ConnectOptions`] (home folder,
//! app data folder, executable, page opener, storage), so tests never write into a real browser
//! directory, keychain or socket directory.
use super::link::auth::AuthError;
use super::link::protocol::ErrorCode;
use super::links::keystore::MonoSecretsKeyStore;
use super::links::keystore::{FallbackKeyStore, KeyringStore, LinkKeyStore, MemoryKeyStore};
use super::links::manager::{
    Deps, FileRecordStore, LinkEvent, LinkManager, LinkRecordStore, ManagerConfig,
    MemoryRecordStore, Refusal,
};
use super::links::manifest_install::{
    self, Browser, Change, InstallEnv, Registry, Target, V1_BROWSERS,
};
use super::links::pairing::{pair_page_url, PairingState, ProfileChoice};
use super::links::profiles::{read_profiles, valid_profile_dir};
use super::peer::{self, Flavor, PeerPolicy};
use super::MonoBrowserStore;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

/// What the Connect card shows now, most specific first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No supported browser is installed.
    NoBrowser,
    /// A browser exists; the user has not pressed Connect (and no manifest is written).
    Ready,
    /// The host manifest is written but no pairing is running.
    ManifestWritten,
    /// A pairing was tried and ended without the extension answering: the extension is probably
    /// not installed in that profile yet.
    WaitingForInstall,
    /// A pairing window is open and the pair page was opened in the browser.
    Pairing,
    /// At least one paired browser is connected.
    Connected,
    /// An extension tried to connect with a protocol or build this app does not accept.
    ProtocolMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileView {
    pub dir: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrowserView {
    pub name: &'static str,
    pub profiles: Vec<ProfileView>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkView {
    pub instance_id: String,
    pub name: String,
    pub profile_dir: String,
    pub brand: String,
    pub connected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectStatus {
    pub phase: Phase,
    pub browsers: Vec<BrowserView>,
    pub links: Vec<LinkView>,
    /// The pairing key lives in the encrypted `mono_secrets` table, not the OS credential store.
    pub keys_in_fallback: bool,
    /// Why the listener is not running (a second app copy, an unsupported OS), when it is not.
    pub server_error: Option<String>,
    /// The last thing worth telling the user (a refused connection, a failed open).
    pub alert: Option<String>,
    /// When the open pairing window closes, in ms since the epoch.
    pub pairing_expires_ms: Option<u64>,
}

/// Opens a page in one browser profile. Production launches Chrome; tests record the call.
pub trait PageOpener: Send + Sync {
    fn open(&self, profile_dir: &str, url: &str) -> Result<(), String>;
}

/// Starts the installed Chrome with `--profile-directory`. Chrome forwards the URL to a running
/// instance of that profile, so no second Chrome appears. Nothing here asks the OS to activate
/// the app, so focus handling is whatever Chrome does for a forwarded URL.
pub struct SystemOpener;

impl PageOpener for SystemOpener {
    fn open(&self, profile_dir: &str, url: &str) -> Result<(), String> {
        super::open_in_profile(&super::PrereqEnv::detect(), profile_dir, url)
    }
}

/// Where the keys and link records live.
pub enum Storage {
    /// The OS credential store with `mono_secrets` in this database as the fallback, and the link
    /// records in `<app data dir>/browser-links.json`.
    Real { database: PathBuf },
    /// Nothing leaves memory (tests).
    Memory,
}

pub struct ConnectOptions {
    pub app_version: String,
    /// The browser dirs are looked up under these (tests pass a temp folder).
    pub home: PathBuf,
    pub local_app_data: Option<PathBuf>,
    pub app_data_dir: PathBuf,
    /// The executable the host manifest points at.
    pub exe: PathBuf,
    pub flavor_dev: bool,
    pub storage: Storage,
    pub opener: Arc<dyn PageOpener>,
    /// Where the listener's socket and lock go. `None` starts no listener (tests).
    pub socket_dir: Option<PathBuf>,
}

/// What the event watcher learned. Not security state: only what the card says.
#[derive(Default)]
struct Observed {
    /// Set by `connect`: whether the pair page was opened.
    attempt: Option<Attempt>,
    /// A refused connection the user should hear about.
    mismatch: bool,
    alert: Option<String>,
    manifest_written: bool,
}

struct Attempt {
    opened: bool,
}

struct NoRegistry;
impl Registry for NoRegistry {
    fn set_default(&self, _: &str, _: &str) -> std::io::Result<()> {
        Err(std::io::Error::other("no registry on this OS"))
    }
    fn delete_key(&self, _: &str) -> std::io::Result<()> {
        Ok(())
    }
}

fn registry() -> Box<dyn Registry> {
    #[cfg(windows)]
    {
        Box::new(manifest_install::WindowsRegistry)
    }
    #[cfg(not(windows))]
    {
        Box::new(NoRegistry)
    }
}

#[cfg(unix)]
type Server = super::links::server::LinkServer;
#[cfg(not(unix))]
type Server = ();

/// The running link layer. Create it once with [`start_app`], or per test with [`Self::new`].
pub struct BrowserConnect {
    manager: LinkManager,
    flavor: Flavor,
    env: InstallEnv,
    exe: PathBuf,
    opener: Arc<dyn PageOpener>,
    observed: Arc<Mutex<Observed>>,
    server: Mutex<Option<Server>>,
    server_error: Option<String>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

static GLOBAL: OnceLock<Arc<BrowserConnect>> = OnceLock::new();

/// Starts the link layer for the app and keeps it for the app lifetime. Call once at startup,
/// after the database path is known. The listener is unix only for now (the Windows named pipe is
/// a later ticket); the Debug-build relaxation of the peer policy applies to dev builds only.
pub fn start_app(
    database: &Path,
    app_data_dir: &Path,
    app_version: &str,
) -> Option<Arc<BrowserConnect>> {
    // Hash the running image before anything can connect: an updater may replace the file later.
    let _ = peer::own_exe_hash();
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())?;
    let exe = std::env::current_exe().ok()?;
    #[cfg(unix)]
    let socket_dir = peer::dir::socket_dir().ok();
    #[cfg(not(unix))]
    let socket_dir = None;
    let connect = Arc::new(BrowserConnect::new(ConnectOptions {
        app_version: app_version.to_owned(),
        home,
        local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
        app_data_dir: app_data_dir.to_owned(),
        exe,
        flavor_dev: cfg!(debug_assertions),
        storage: Storage::Real {
            database: database.to_owned(),
        },
        opener: Arc::new(SystemOpener),
        socket_dir,
    }));
    GLOBAL.set(connect.clone()).ok();
    GLOBAL.get().cloned()
}

/// The app's link layer, once [`start_app`] ran.
pub fn global() -> Option<Arc<BrowserConnect>> {
    GLOBAL.get().cloned()
}

/// Stops the listener and removes its socket. The app calls it on quit.
pub fn shutdown() {
    if let Some(connect) = GLOBAL.get() {
        connect.stop();
    }
}

impl BrowserConnect {
    pub fn new(options: ConnectOptions) -> Self {
        let flavor = if options.flavor_dev {
            Flavor::Dev
        } else {
            Flavor::Release
        };
        let (keys, records): (Arc<dyn LinkKeyStore>, Arc<dyn LinkRecordStore>) =
            match &options.storage {
                Storage::Real { database } => (
                    Arc::new(FallbackKeyStore::new(
                        Arc::new(KeyringStore::new(options.flavor_dev)),
                        Arc::new(MonoSecretsKeyStore::new(MonoBrowserStore::at(
                            database.clone(),
                        ))),
                    )),
                    Arc::new(FileRecordStore::new(
                        options.app_data_dir.join("browser-links.json"),
                    )),
                ),
                Storage::Memory => (
                    Arc::new(MemoryKeyStore::new()),
                    Arc::new(MemoryRecordStore::new()),
                ),
            };
        let (manager, events) = LinkManager::new(
            ManagerConfig::new(flavor, options.app_version.clone()),
            Deps::new(keys, records),
        );
        let observed = Arc::new(Mutex::new(Observed::default()));
        watch_events(events, observed.clone());
        let (server, server_error) = start_server(&manager, flavor, options.socket_dir.as_deref());
        Self {
            manager,
            flavor,
            env: InstallEnv {
                os: manifest_install::Os::current(),
                home: options.home,
                local_app_data: options.local_app_data,
                app_data_dir: options.app_data_dir,
            },
            exe: options.exe,
            opener: options.opener,
            observed,
            server: Mutex::new(server),
            server_error,
        }
    }

    /// Stops accepting and releases the single-instance lock.
    pub fn stop(&self) {
        drop(lock(&self.server).take());
    }

    fn extension_id(&self) -> &'static str {
        match self.flavor {
            Flavor::Dev => manifest_install::DEV_EXTENSION_ID,
            Flavor::Release => manifest_install::STORE_EXTENSION_IDS[0],
        }
    }

    fn detected(&self) -> Vec<Browser> {
        manifest_install::detected(&self.env, V1_BROWSERS)
    }

    fn browser_views(&self) -> Vec<BrowserView> {
        self.detected()
            .into_iter()
            .map(|browser| BrowserView {
                name: browser_name(browser),
                profiles: self
                    .env
                    .data_dir(browser)
                    .map(|dir| read_profiles(&dir))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|p| ProfileView {
                        dir: p.dir,
                        name: p.name,
                    })
                    .collect(),
            })
            .collect()
    }

    /// True when every detected browser already has a manifest identical to what we would write.
    /// Windows keeps the registry entry out of reach of the injected trait, so it relies on the
    /// flag `connect` sets.
    fn manifest_present(&self) -> bool {
        let detected = self.detected();
        if detected.is_empty() {
            return false;
        }
        let Ok(expected) = manifest_install::manifest_json(self.flavor, &self.exe) else {
            return false;
        };
        let on_disk =
            detected.iter().all(
                |b| match manifest_install::target(&self.env, *b, self.flavor) {
                    Some(Target::File(path)) => std::fs::read_to_string(path)
                        .map(|text| text == expected)
                        .unwrap_or(false),
                    Some(Target::Registry { file, .. }) => std::fs::read_to_string(file)
                        .map(|text| text == expected)
                        .unwrap_or(false),
                    None => true,
                },
            );
        on_disk || lock(&self.observed).manifest_written
    }

    pub fn status(&self) -> ConnectStatus {
        let browsers = self.browser_views();
        let links: Vec<LinkView> = self
            .manager
            .links()
            .unwrap_or_default()
            .into_iter()
            .map(|l| LinkView {
                instance_id: l.instance_id,
                name: l.name,
                profile_dir: l.profile_dir,
                brand: l.brand,
                connected: l.connected,
            })
            .collect();
        let pairing = self.manager.pairing_state();
        let (attempt_opened, mismatch, alert) = {
            let o = lock(&self.observed);
            (
                o.attempt.as_ref().map(|a| a.opened),
                o.mismatch,
                o.alert.clone(),
            )
        };
        let connected = links.iter().any(|l| l.connected);
        let phase = if browsers.is_empty() {
            Phase::NoBrowser
        } else if connected {
            Phase::Connected
        } else if mismatch {
            Phase::ProtocolMismatch
        } else {
            match (&pairing, attempt_opened) {
                (PairingState::Waiting { .. }, Some(true)) => Phase::Pairing,
                (PairingState::Waiting { .. }, _) => Phase::WaitingForInstall,
                (PairingState::Idle, Some(_)) => Phase::WaitingForInstall,
                (PairingState::Idle, None) if self.manifest_present() => Phase::ManifestWritten,
                (PairingState::Idle, None) => Phase::Ready,
            }
        };
        ConnectStatus {
            phase,
            browsers,
            links,
            keys_in_fallback: self.manager.keys_in_fallback(),
            server_error: self.server_error.clone(),
            alert,
            pairing_expires_ms: match pairing {
                PairingState::Waiting { expires_ms } => Some(expires_ms),
                PairingState::Idle => None,
            },
        }
    }

    /// The Connect button: writes the host manifest for every detected browser, opens a pairing
    /// window for `profile_dir` and opens the pair page in that profile. `profile_name` is the
    /// label shown on the card (the profile's own name when empty).
    pub fn connect(&self, profile_dir: &str, profile_name: &str) -> Result<(), String> {
        if !valid_profile_dir(profile_dir) {
            return Err("Choose a browser profile first.".into());
        }
        if self.detected().is_empty() {
            return Err("Install Google Chrome first.".into());
        }
        if let Some(reason) = &self.server_error {
            return Err(reason.clone());
        }
        let installed = manifest_install::install(
            &self.env,
            self.flavor,
            &self.exe,
            V1_BROWSERS,
            registry().as_ref(),
        )
        .map_err(|e| match e {
            manifest_install::InstallError::BadExecutable(_) => {
                "The app location is not usable for the browser host.".to_string()
            }
            manifest_install::InstallError::DevNotAllowed => {
                "The development extension is only available in development builds.".to_string()
            }
            manifest_install::InstallError::UnsafeDestination(_) => {
                "The browser's host folder is not safe to write to.".to_string()
            }
            manifest_install::InstallError::Io(_) => {
                "The browser host could not be installed.".to_string()
            }
        })?;
        let _changed = installed.iter().any(|i| i.change == Change::Written);
        lock(&self.observed).manifest_written = true;
        let profile = ProfileChoice::new(profile_dir, profile_name)?;
        let ticket = self.manager.start_pairing(profile);
        let url = pair_page_url(self.extension_id(), &ticket.nonce);
        let opened = self.opener.open(profile_dir, &url);
        {
            let mut o = lock(&self.observed);
            o.mismatch = false;
            o.alert = opened.as_ref().err().cloned();
            o.attempt = Some(Attempt {
                opened: opened.is_ok(),
            });
        }
        if opened.is_err() {
            self.manager.cancel_pairing();
        }
        opened
    }

    /// Stops a pairing in progress.
    pub fn cancel_pairing(&self) {
        self.manager.cancel_pairing();
        lock(&self.observed).attempt = None;
    }

    /// Forgets one browser: closes the link and deletes its key and record.
    pub fn disconnect(&self, instance_id: &str) -> Result<(), String> {
        self.manager.disconnect(instance_id)
    }

    /// Forgets every browser and removes the host manifests.
    pub fn disconnect_all(&self) -> Result<(), String> {
        let links = self.manager.disconnect_all();
        let manifests =
            manifest_install::remove(&self.env, self.flavor, V1_BROWSERS, registry().as_ref())
                .map_err(|_| "The browser host could not be removed.".to_string());
        {
            let mut o = lock(&self.observed);
            *o = Observed::default();
        }
        links.and(manifests)
    }
}

fn browser_name(browser: Browser) -> &'static str {
    match browser {
        Browser::Chrome => "Google Chrome",
        Browser::ChromeBeta => "Chrome Beta",
        Browser::ChromeCanary => "Chrome Canary",
        Browser::Chromium => "Chromium",
        Browser::Edge => "Microsoft Edge",
        Browser::Brave => "Brave",
        Browser::Arc => "Arc",
    }
}

#[cfg(unix)]
fn start_server(
    manager: &LinkManager,
    flavor: Flavor,
    dir: Option<&Path>,
) -> (Option<Server>, Option<String>) {
    let Some(dir) = dir else {
        return (None, None);
    };
    // Only a debug build may relax the peer policy (the policy ignores the relaxation otherwise).
    let policy = if cfg!(debug_assertions) && flavor == Flavor::Dev {
        #[cfg(debug_assertions)]
        {
            PeerPolicy::native_dev()
        }
        #[cfg(not(debug_assertions))]
        {
            PeerPolicy::release()
        }
    } else {
        PeerPolicy::release()
    };
    match Server::start(manager.clone(), dir, flavor, policy) {
        Ok(server) => (Some(server), None),
        Err(e) => (
            None,
            Some(format!("The browser link could not start: {e}.")),
        ),
    }
}

#[cfg(not(unix))]
fn start_server(
    _manager: &LinkManager,
    _flavor: Flavor,
    dir: Option<&Path>,
) -> (Option<Server>, Option<String>) {
    // The Windows named pipe listener is a later ticket (T7b).
    (
        None,
        dir.map(|_| "Browser connections are not available on this system yet.".to_string()),
    )
}

fn watch_events(events: std::sync::mpsc::Receiver<LinkEvent>, observed: Arc<Mutex<Observed>>) {
    let spawned = std::thread::Builder::new()
        .name("zptah-browser-events".into())
        .spawn(move || {
            // Ends when the manager (the only sender) is dropped.
            while let Ok(event) = events.recv() {
                let mut o = lock(&observed);
                match event {
                    LinkEvent::Paired { .. } => o.attempt = None,
                    LinkEvent::Connected { .. } => {
                        o.attempt = None;
                        o.mismatch = false;
                        o.alert = None;
                    }
                    LinkEvent::Refused { refusal, .. } => {
                        let mismatch = match &refusal {
                            Refusal::UnknownBuild { .. } => true,
                            Refusal::Protocol(e) => e.code == ErrorCode::Protocol,
                            Refusal::Auth(AuthError::Protocol(e)) => e.code == ErrorCode::Protocol,
                            _ => false,
                        };
                        if mismatch {
                            o.mismatch = true;
                        }
                        o.alert = Some(refusal.message());
                    }
                    LinkEvent::Disconnected { .. } | LinkEvent::Extension { .. } => {}
                }
            }
        });
    drop(spawned);
}

/// A fake extension kept connected, for UI tests in other crates.
#[cfg(all(any(test, feature = "test-support"), unix))]
pub struct FakeLinkGuard {
    _extension: super::links::fake_extension::FakeExtension,
    _thread: std::thread::JoinHandle<()>,
}

#[cfg(all(any(test, feature = "test-support"), unix))]
impl BrowserConnect {
    fn serve_fake(
        &self,
        instance_id: &str,
        key: super::link::auth::PairingKey,
    ) -> (
        super::links::fake_extension::FakeExtension,
        std::thread::JoinHandle<()>,
    ) {
        let (conn, fake) = super::links::fake_extension::socket_pair(instance_id, key);
        let manager = self.manager.clone();
        let report = peer::PeerReport {
            pid: 4242,
            parent_pid: Some(100),
            parent_exe: None,
            user_data_dir: None,
        };
        let thread = std::thread::spawn(move || manager.serve(conn, &report));
        (fake, thread)
    }

    /// Pairs a fake extension with the pair page `nonce` the opener received, reconnects it and
    /// keeps the link open until the guard drops.
    pub fn connect_fake_extension(
        &self,
        instance_id: &str,
        nonce: &str,
    ) -> Result<FakeLinkGuard, String> {
        let (mut fake, thread) =
            self.serve_fake(instance_id, super::link::auth::PairingKey::generate());
        let key = fake.pair(nonce).map_err(|e| e.message)?;
        let _ = thread.join();
        let (mut fake, thread) = self.serve_fake(instance_id, key);
        fake.connect().map_err(|e| e.message)?;
        Ok(FakeLinkGuard {
            _extension: fake,
            _thread: thread,
        })
    }

    /// A fake extension whose protocol range this app does not support, refused at `hello`.
    pub fn refuse_fake_protocol(&self, instance_id: &str) {
        let (mut fake, thread) =
            self.serve_fake(instance_id, super::link::auth::PairingKey::generate());
        fake.tweak_hello(|p| p.protocol = [2, 3]);
        let _ = fake.hello_only(true);
        let _ = thread.join();
    }
}

#[cfg(test)]
mod tests;
