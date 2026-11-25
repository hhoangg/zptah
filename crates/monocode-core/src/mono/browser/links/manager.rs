//! The link manager: handshake, pairing, admission and live links (spec 5.3, 5.4, 12.3).
//!
//! [`LinkManager::serve`] takes one connection whose peer the server already verified (own
//! executable, started by a browser) and runs the protocol on the calling thread: `hello`, then
//! either `auth` (a paired extension) or `pair.begin` (a pairing window is open). After `auth` it
//! admits the link only when no live link has that `instance_id` and the browser and data dir match
//! what pairing recorded, then serves it until it closes. The auth response is the first sealed
//! frame (app `seq` 1). Errors before the session exists go out unsealed.
use super::super::link::auth::{
    AppHandshake, AuthError, ChallengeLog, HelloContext, PairingWindow, Session,
};
use super::super::link::frame::{
    read_frame, write_frame, FrameFormat, MAX_HOST_TO_EXTENSION_FRAME,
};
use super::super::link::protocol::{
    parse_params, AuthParams, AuthResult, Envelope, EnvelopeType, ErrorBody, ErrorCode,
    HelloParams, Method, PairBeginParams, ProtocolError, Roster, Sender,
};
use super::super::peer::{Flavor, PeerReport};
use super::keystore::LinkKeyStore;
use super::known_builds::{
    installed_extension_dir, BuildVerdict, KnownBuilds, UNKNOWN_BUILD_MESSAGE,
};
use super::pairing::{PairingState, PairingTicket, ProfileChoice};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// How long a fresh connection may take to finish the handshake.
/// Room for the `seq` and `mac` members sealing adds (a 43 character MAC and a counter).
const SEAL_OVERHEAD: usize = 128;

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

// ---- transport ----

/// What the manager needs to stop or time out a connection from another thread.
pub(crate) trait ConnControl: Send + Sync {
    fn set_read_timeout(&self, timeout: Option<Duration>);
    /// Closes both directions; a blocked read returns.
    fn close(&self);
}

/// One accepted connection, split so a reader thread and senders do not share a lock.
pub(crate) struct Conn {
    pub reader: Box<dyn Read + Send>,
    pub writer: Box<dyn Write + Send>,
    pub control: Box<dyn ConnControl>,
}

#[cfg(unix)]
struct UnixControl(std::os::unix::net::UnixStream);

#[cfg(unix)]
impl ConnControl for UnixControl {
    fn set_read_timeout(&self, timeout: Option<Duration>) {
        let _ = self.0.set_read_timeout(timeout);
    }
    fn close(&self) {
        let _ = self.0.shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(unix)]
impl Conn {
    pub(crate) fn from_unix(stream: std::os::unix::net::UnixStream) -> std::io::Result<Conn> {
        Ok(Conn {
            reader: Box::new(stream.try_clone()?),
            writer: Box::new(stream.try_clone()?),
            control: Box::new(UnixControl(stream)),
        })
    }
}

// ---- records ----

/// What pairing recorded about one extension install. Not secret: the key is in the key store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LinkRecord {
    pub instance_id: String,
    /// Shown on the card ("Work").
    pub name: String,
    /// `--profile-directory` value.
    pub profile_dir: String,
    /// `browser.brand` from the handshake.
    pub brand: String,
    /// The parent browser executable the host reported at pairing, when known.
    pub parent_exe: Option<PathBuf>,
    /// The parent's `--user-data-dir` at pairing, when it had one.
    pub user_data_dir: Option<PathBuf>,
    pub paired_at_ms: u64,
}

pub(crate) trait LinkRecordStore: Send + Sync {
    fn list(&self) -> Result<Vec<LinkRecord>, String>;
    fn get(&self, instance_id: &str) -> Result<Option<LinkRecord>, String> {
        Ok(self
            .list()?
            .into_iter()
            .find(|r| r.instance_id == instance_id))
    }
    fn save(&self, record: &LinkRecord) -> Result<(), String>;
    fn remove(&self, instance_id: &str) -> Result<(), String>;
}

#[derive(Default)]
pub(crate) struct MemoryRecordStore {
    records: Mutex<Vec<LinkRecord>>,
}

impl MemoryRecordStore {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

impl LinkRecordStore for MemoryRecordStore {
    fn list(&self) -> Result<Vec<LinkRecord>, String> {
        Ok(self.records.lock().unwrap().clone())
    }
    fn save(&self, record: &LinkRecord) -> Result<(), String> {
        let mut all = self.records.lock().unwrap();
        all.retain(|r| r.instance_id != record.instance_id);
        all.push(record.clone());
        Ok(())
    }
    fn remove(&self, instance_id: &str) -> Result<(), String> {
        self.records
            .lock()
            .unwrap()
            .retain(|r| r.instance_id != instance_id);
        Ok(())
    }
}

/// Records in one JSON file in the app data dir, replaced atomically.
pub(crate) struct FileRecordStore {
    path: PathBuf,
    lock: Mutex<()>,
}

impl FileRecordStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            path,
            lock: Mutex::new(()),
        }
    }

    fn read(&self) -> Result<Vec<LinkRecord>, String> {
        match std::fs::read(&self.path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|_| "The saved browser connections are damaged.".to_string()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(_) => Err("The saved browser connections could not be read.".to_string()),
        }
    }

    fn write(&self, records: &[LinkRecord]) -> Result<(), String> {
        let fail = || "The browser connections could not be saved.".to_string();
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|_| fail())?;
        }
        let tmp = self
            .path
            .with_extension(format!("json.{:016x}.tmp", rand::random::<u64>()));
        let bytes = serde_json::to_vec_pretty(records).map_err(|_| fail())?;
        // `create_new` (O_EXCL) never follows a planted file or link at the temp name.
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .and_then(|mut f| f.write_all(&bytes));
        if written.is_err() {
            let _ = std::fs::remove_file(&tmp);
            return Err(fail());
        }
        std::fs::rename(&tmp, &self.path).map_err(|_| {
            let _ = std::fs::remove_file(&tmp);
            fail()
        })
    }
}

impl LinkRecordStore for FileRecordStore {
    fn list(&self) -> Result<Vec<LinkRecord>, String> {
        let _guard = self.lock.lock().unwrap();
        self.read()
    }
    fn save(&self, record: &LinkRecord) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        let mut all = self.read()?;
        all.retain(|r| r.instance_id != record.instance_id);
        all.push(record.clone());
        self.write(&all)
    }
    fn remove(&self, instance_id: &str) -> Result<(), String> {
        let _guard = self.lock.lock().unwrap();
        let mut all = self.read()?;
        all.retain(|r| r.instance_id != instance_id);
        self.write(&all)
    }
}

/// The Monos' roster and the stop switch, sent in the `auth` result.
pub(crate) trait RosterSource: Send + Sync {
    fn snapshot(&self) -> AuthResult;
}

pub(crate) struct EmptyRoster;

impl RosterSource for EmptyRoster {
    fn snapshot(&self) -> AuthResult {
        AuthResult {
            roster: Roster::new(),
            stopped: false,
        }
    }
}

// ---- outcomes ----

/// Why a connection ended without a link. Each variant maps to a line the user may see.
#[derive(Debug, Clone)]
pub(crate) enum Refusal {
    /// The peer hung up before finishing.
    Closed,
    Timeout,
    Frame(String),
    Protocol(ProtocolError),
    Auth(AuthError),
    UnknownBuild {
        version: String,
    },
    NotPaired,
    BrowserMismatch {
        name: String,
    },
    DataDirMismatch {
        name: String,
    },
    Duplicate {
        name: String,
    },
    Storage(String),
    /// Peer verification failed; the connection was dropped before any protocol byte.
    Peer(String),
}

impl Refusal {
    /// Text for an alert (spec 5.3, 5.4).
    pub(crate) fn message(&self) -> String {
        match self {
            Refusal::Duplicate { name } => {
                format!("A second browser tried to connect as '{name}'.")
            }
            Refusal::BrowserMismatch { name } | Refusal::DataDirMismatch { name } => format!(
                "A browser tried to connect as '{name}' from a different browser or profile folder."
            ),
            Refusal::UnknownBuild { .. } => UNKNOWN_BUILD_MESSAGE.to_string(),
            Refusal::Auth(_) => "A browser connection failed to authenticate.".to_string(),
            Refusal::NotPaired => "A browser that is not paired tried to connect.".to_string(),
            Refusal::Protocol(e) if e.code == ErrorCode::Protocol => {
                "A browser extension speaks a different protocol version.".to_string()
            }
            Refusal::Storage(_) => "The browser key could not be saved.".to_string(),
            Refusal::Peer(_) => {
                "A program that is not the zptah browser host tried to connect.".to_string()
            }
            _ => "A browser connection was refused.".to_string(),
        }
    }

    fn wire(&self) -> ProtocolError {
        match self {
            Refusal::Protocol(e) => e.clone(),
            Refusal::Auth(e) => e.to_protocol_error(),
            Refusal::UnknownBuild { version } => {
                ProtocolError::new(ErrorCode::Protocol, UNKNOWN_BUILD_MESSAGE)
                    .with_detail(json!({ "reason": "unknown_build", "version": version }))
            }
            Refusal::NotPaired => {
                ProtocolError::new(ErrorCode::NotPaired, "extension is not paired")
            }
            Refusal::BrowserMismatch { .. } | Refusal::DataDirMismatch { .. } => {
                ProtocolError::new(
                    ErrorCode::Auth,
                    "this link does not match the paired browser",
                )
            }
            Refusal::Duplicate { .. } => {
                ProtocolError::new(ErrorCode::Auth, "this extension already has a live link")
            }
            Refusal::Storage(_) => ProtocolError::new(ErrorCode::Invalid, "could not save the key"),
            Refusal::Closed | Refusal::Timeout | Refusal::Frame(_) | Refusal::Peer(_) => {
                ProtocolError::invalid("bad frame")
            }
        }
    }
}

impl From<AuthError> for Refusal {
    fn from(e: AuthError) -> Self {
        Refusal::Auth(e)
    }
}
impl From<ProtocolError> for Refusal {
    fn from(e: ProtocolError) -> Self {
        Refusal::Protocol(e)
    }
}

/// What the Connect card and the extension upstream listen to.
#[derive(Debug, Clone)]
pub(crate) enum LinkEvent {
    /// A pairing finished; the extension reconnects with the normal handshake next.
    Paired {
        instance_id: String,
        name: String,
    },
    Connected {
        instance_id: String,
        name: String,
    },
    Disconnected {
        instance_id: String,
    },
    Refused {
        instance_id: Option<String>,
        refusal: Refusal,
    },
    /// An extension event (spec 5.6), already MAC checked.
    Extension {
        instance_id: String,
        envelope: Envelope,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinkInfo {
    pub instance_id: String,
    pub name: String,
    pub profile_dir: String,
    pub brand: String,
    pub connected: bool,
    pub paired_at_ms: u64,
}

// ---- configuration ----

pub(crate) struct ManagerConfig {
    pub flavor: Flavor,
    pub app_version: String,
    /// Extension ids whose installed folder may be hashed (spec 13.5).
    pub extension_ids: Vec<String>,
    /// Compare the parent's `--user-data-dir` with the recorded one. Off on Windows, where the
    /// host cannot read it yet: there the browser brand and executable are all that is compared.
    pub check_data_dir: bool,
    pub handshake_timeout: Duration,
}

impl ManagerConfig {
    pub(crate) fn new(flavor: Flavor, app_version: impl Into<String>) -> Self {
        let extension_ids = match flavor {
            Flavor::Release => super::manifest_install::STORE_EXTENSION_IDS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            Flavor::Dev => vec![super::manifest_install::DEV_EXTENSION_ID.to_string()],
        };
        Self {
            flavor,
            app_version: app_version.into(),
            extension_ids,
            check_data_dir: !cfg!(windows),
            handshake_timeout: HANDSHAKE_TIMEOUT,
        }
    }
}

pub(crate) struct Deps {
    pub keys: Arc<dyn LinkKeyStore>,
    pub records: Arc<dyn LinkRecordStore>,
    pub roster: Arc<dyn RosterSource>,
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl Deps {
    pub(crate) fn new(keys: Arc<dyn LinkKeyStore>, records: Arc<dyn LinkRecordStore>) -> Self {
        Self {
            keys,
            records,
            roster: Arc::new(EmptyRoster),
            clock: Arc::new(crate::time::now_ms),
        }
    }
}

// ---- live links ----

#[derive(Debug)]
pub(crate) enum RequestError {
    /// The link closed before an answer.
    Closed,
    Timeout,
    /// The request does not fit Chrome's 1 MiB message limit; split it into chunks.
    TooLarge,
    Invalid(ProtocolError),
    /// The extension answered with an error.
    Remote(ErrorBody),
}

struct Shared {
    instance_id: String,
    /// Held for a whole send (seal and write) so frames leave in `seq` order. A slow write blocks
    /// other senders but never the reader.
    writer: Mutex<Box<dyn Write + Send>>,
    /// Held only for the instant of a `seal` or an `open`.
    session: Mutex<Session>,
    pending: Mutex<HashMap<String, mpsc::Sender<Envelope>>>,
    next_id: AtomicU64,
    closed: AtomicBool,
    control: Box<dyn ConnControl>,
}

impl Shared {
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.control.close();
    }
}

/// Send requests to one live link. Cheap to clone; stops working when the link closes.
#[derive(Clone)]
pub(crate) struct LinkHandle {
    shared: Arc<Shared>,
}

impl LinkHandle {
    pub(crate) fn instance_id(&self) -> &str {
        &self.shared.instance_id
    }

    pub(crate) fn is_open(&self) -> bool {
        !self.shared.closed.load(Ordering::SeqCst)
    }

    /// Sends a request and waits for its result. `mono` is required by Mono-scoped methods.
    pub(crate) fn request(
        &self,
        method: Method,
        mono: Option<&str>,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RequestError> {
        let shared = &self.shared;
        if shared.closed.load(Ordering::SeqCst) {
            return Err(RequestError::Closed);
        }
        let id = format!("a-{}", shared.next_id.fetch_add(1, Ordering::SeqCst));
        let mut env = Envelope::request(id.clone(), method, params);
        if let Some(mono) = mono {
            env = env.with_mono(mono);
        }
        env.check_from(Sender::App).map_err(RequestError::Invalid)?;
        let (tx, rx) = mpsc::channel();
        shared.pending.lock().unwrap().insert(id.clone(), tx);
        let sent = self.send(&mut env);
        if let Err(e) = sent {
            shared.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match rx.recv_timeout(timeout) {
            Ok(res) => match (res.ok, res.error) {
                (Some(true), _) => Ok(res.result.unwrap_or(Value::Null)),
                (_, Some(body)) => Err(RequestError::Remote(body)),
                _ => Err(RequestError::Invalid(ProtocolError::invalid(
                    "response without result or error",
                ))),
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {
                shared.pending.lock().unwrap().remove(&id);
                Err(RequestError::Timeout)
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(RequestError::Closed),
        }
    }

    fn send(&self, env: &mut Envelope) -> Result<(), RequestError> {
        // Chrome takes at most 1 MiB per message to the extension (spec 5.1); the app chunks
        // anything bigger. Check before sealing: sealing consumes a `seq`, and a frame that is
        // sealed but never sent would make the extension see a gap and close the link.
        if env.to_vec().len() + SEAL_OVERHEAD > MAX_HOST_TO_EXTENSION_FRAME {
            return Err(RequestError::TooLarge);
        }
        let mut writer = lock(&self.shared.writer);
        lock(&self.shared.session)
            .seal(env)
            .map_err(RequestError::Invalid)?;
        let bytes = env.to_vec();
        if bytes.len() > MAX_HOST_TO_EXTENSION_FRAME {
            // Safety net: the pre-check leaves room, so this should not happen. The seq is spent,
            // so the link cannot continue.
            drop(writer);
            self.shared.close();
            return Err(RequestError::TooLarge);
        }
        if write_frame(&mut *writer, FrameFormat::SOCKET, &bytes).is_err() {
            drop(writer);
            self.shared.close();
            return Err(RequestError::Closed);
        }
        Ok(())
    }

    pub(crate) fn close(&self) {
        self.shared.close();
    }
}

enum Slot {
    /// Authenticated and admitted, auth response not yet sent.
    Reserved,
    Live(Arc<Shared>),
}

struct Pairing {
    window: PairingWindow,
    profile: ProfileChoice,
    expires_ms: u64,
}

enum Outcome {
    Paired,
    Linked(Linked),
}

struct Linked {
    session: Session,
    instance_id: String,
    name: String,
}

struct Inner {
    config: ManagerConfig,
    keys: Arc<dyn LinkKeyStore>,
    records: Arc<dyn LinkRecordStore>,
    roster: Arc<dyn RosterSource>,
    clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    known: KnownBuilds,
    challenges: Mutex<ChallengeLog>,
    pairing: Mutex<Option<Pairing>>,
    links: Mutex<HashMap<String, Slot>>,
    events: Mutex<mpsc::Sender<LinkEvent>>,
}

/// Cheap to clone; all clones share state.
#[derive(Clone)]
pub(crate) struct LinkManager {
    inner: Arc<Inner>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl LinkManager {
    pub(crate) fn new(
        config: ManagerConfig,
        deps: Deps,
    ) -> (LinkManager, mpsc::Receiver<LinkEvent>) {
        let (tx, rx) = mpsc::channel();
        let known = KnownBuilds::compiled(config.flavor);
        let inner = Inner {
            config,
            keys: deps.keys,
            records: deps.records,
            roster: deps.roster,
            clock: deps.clock,
            known,
            challenges: Mutex::new(ChallengeLog::default()),
            pairing: Mutex::new(None),
            links: Mutex::new(HashMap::new()),
            events: Mutex::new(tx),
        };
        (
            LinkManager {
                inner: Arc::new(inner),
            },
            rx,
        )
    }

    /// Replaces the known-build list (tests and the release tooling).
    pub(crate) fn with_known_builds(mut self, known: KnownBuilds) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("set known builds before sharing the manager")
            .known = known;
        self
    }

    fn now(&self) -> u64 {
        (self.inner.clock)()
    }

    fn emit(&self, event: LinkEvent) {
        let _ = lock(&self.inner.events).send(event);
    }

    // ---- pairing ----

    /// Opens a 5 minute window for one profile and returns the nonce for the pair page. A window
    /// already open is replaced: only the newest nonce works.
    pub(crate) fn start_pairing(&self, profile: ProfileChoice) -> PairingTicket {
        let now = self.now();
        let (window, nonce) = PairingWindow::open(now);
        self.install_pairing(window, nonce, profile, now)
    }

    /// Like `start_pairing` with a chosen nonce (tests).
    pub(crate) fn start_pairing_with(
        &self,
        profile: ProfileChoice,
        nonce: [u8; super::super::link::auth::NONCE_LEN],
    ) -> PairingTicket {
        let now = self.now();
        let (window, text) = PairingWindow::open_with(now, nonce);
        self.install_pairing(window, text, profile, now)
    }

    fn install_pairing(
        &self,
        window: PairingWindow,
        nonce: String,
        profile: ProfileChoice,
        now: u64,
    ) -> PairingTicket {
        let expires_ms = now.saturating_add(super::super::link::auth::PAIRING_TTL_MS);
        *lock(&self.inner.pairing) = Some(Pairing {
            window,
            profile,
            expires_ms,
        });
        PairingTicket { nonce, expires_ms }
    }

    pub(crate) fn cancel_pairing(&self) {
        *lock(&self.inner.pairing) = None;
    }

    pub(crate) fn pairing_state(&self) -> PairingState {
        let now = self.now();
        match &*lock(&self.inner.pairing) {
            Some(p) if p.window.is_open(now) => PairingState::Waiting {
                expires_ms: p.expires_ms,
            },
            _ => PairingState::Idle,
        }
    }

    // ---- queries and control ----

    /// True when a pairing key lives in the `mono_secrets` fallback instead of the OS credential
    /// store, so the Connect card can say so.
    pub(crate) fn keys_in_fallback(&self) -> bool {
        self.inner.keys.uses_fallback()
    }

    pub(crate) fn links(&self) -> Result<Vec<LinkInfo>, String> {
        let live: Vec<String> = lock(&self.inner.links)
            .iter()
            .filter(|(_, s)| matches!(s, Slot::Live(_)))
            .map(|(id, _)| id.clone())
            .collect();
        Ok(self
            .inner
            .records
            .list()?
            .into_iter()
            .map(|r| LinkInfo {
                connected: live.contains(&r.instance_id),
                instance_id: r.instance_id,
                name: r.name,
                profile_dir: r.profile_dir,
                brand: r.brand,
                paired_at_ms: r.paired_at_ms,
            })
            .collect())
    }

    pub(crate) fn handle(&self, instance_id: &str) -> Option<LinkHandle> {
        match lock(&self.inner.links).get(instance_id) {
            Some(Slot::Live(shared)) => Some(LinkHandle {
                shared: shared.clone(),
            }),
            _ => None,
        }
    }

    /// Forgets one browser: closes its link, deletes its key and record.
    pub(crate) fn disconnect(&self, instance_id: &str) -> Result<(), String> {
        if let Some(handle) = self.handle(instance_id) {
            handle.close();
        }
        {
            // A link that authenticated but is not live yet loses its reservation, so `run`
            // refuses to start it.
            let mut links = lock(&self.inner.links);
            if matches!(links.get(instance_id), Some(Slot::Reserved)) {
                links.remove(instance_id);
            }
        }
        let keys = self.inner.keys.delete(instance_id);
        let records = self.inner.records.remove(instance_id);
        keys.and(records)
    }

    /// "Disconnect all browsers": every link, key and record. Keeps going after an error and
    /// reports the first one.
    pub(crate) fn disconnect_all(&self) -> Result<(), String> {
        self.cancel_pairing();
        let mut ids: Vec<String> = self
            .inner
            .records
            .list()?
            .into_iter()
            .map(|r| r.instance_id)
            .collect();
        ids.extend(lock(&self.inner.links).keys().cloned());
        ids.sort();
        ids.dedup();
        let mut first = Ok(());
        for id in ids {
            let res = self.disconnect(&id);
            if first.is_ok() {
                first = res;
            }
        }
        first
    }

    /// Reports a connection the peer check refused (nothing was read from it).
    pub(crate) fn report_peer_refused(&self, why: &dyn std::fmt::Display) {
        self.emit(LinkEvent::Refused {
            instance_id: None,
            refusal: Refusal::Peer(why.to_string()),
        });
    }

    // ---- serving ----

    /// Runs one verified connection to its end on the calling thread.
    pub(crate) fn serve(&self, conn: Conn, report: &PeerReport) {
        self.serve_notify(conn, report, || {});
    }

    /// Like `serve`, calling `handshake_done` once the handshake ended (success or not), before
    /// a long-lived link starts serving. The server uses it to count only unauthenticated work.
    pub(crate) fn serve_notify(
        &self,
        conn: Conn,
        report: &PeerReport,
        handshake_done: impl FnOnce(),
    ) {
        let Conn {
            mut reader,
            mut writer,
            control,
        } = conn;
        control.set_read_timeout(Some(self.inner.config.handshake_timeout));
        let mut instance = None;
        let outcome = self.handshake(&mut reader, &mut writer, report, &mut instance);
        handshake_done();
        match outcome {
            Ok(Outcome::Paired) => control.close(),
            Ok(Outcome::Linked(linked)) => {
                control.set_read_timeout(None);
                let shared = Arc::new(Shared {
                    instance_id: linked.instance_id,
                    writer: Mutex::new(writer),
                    session: Mutex::new(linked.session),
                    pending: Mutex::new(HashMap::new()),
                    next_id: AtomicU64::new(1),
                    closed: AtomicBool::new(false),
                    control,
                });
                self.run(shared, linked.name, reader);
            }
            Err(refusal) => {
                if !matches!(refusal, Refusal::Closed) {
                    self.emit(LinkEvent::Refused {
                        instance_id: instance,
                        refusal,
                    });
                }
                control.close();
            }
        }
    }

    fn read_env(reader: &mut Box<dyn Read + Send>) -> Result<Envelope, Refusal> {
        use super::super::link::frame::FrameError;
        match read_frame(reader, FrameFormat::SOCKET) {
            Ok(Some(payload)) => Envelope::from_slice(&payload).map_err(Refusal::Protocol),
            Ok(None) => Err(Refusal::Closed),
            Err(FrameError::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                Err(Refusal::Timeout)
            }
            Err(e) => Err(Refusal::Frame(e.to_string())),
        }
    }

    fn write_env(writer: &mut Box<dyn Write + Send>, env: &Envelope) -> Result<(), Refusal> {
        write_frame(writer, FrameFormat::SOCKET, &env.to_vec())
            .map_err(|e| Refusal::Frame(e.to_string()))
    }

    /// Answers the request `id` with `refusal` (unsealed) and hands the refusal back.
    fn reject(
        writer: &mut Box<dyn Write + Send>,
        id: &str,
        refusal: Refusal,
    ) -> Result<Outcome, Refusal> {
        let id = if id.len() >= 3 && id.starts_with("x-") {
            id
        } else {
            "x-0"
        };
        let _ = Self::write_env(writer, &Envelope::response_err(id, &refusal.wire()));
        Err(refusal)
    }

    fn handshake(
        &self,
        reader: &mut Box<dyn Read + Send>,
        writer: &mut Box<dyn Write + Send>,
        report: &PeerReport,
        instance: &mut Option<String>,
    ) -> Result<Outcome, Refusal> {
        let req = match Self::read_env(reader) {
            Ok(env) => env,
            Err(Refusal::Protocol(e)) => {
                return Self::reject(writer, "x-0", Refusal::Protocol(e));
            }
            Err(other) => return Err(other),
        };
        if let Err(e) = req.check_from(Sender::Extension) {
            return Self::reject(writer, &req.id, Refusal::Protocol(e));
        }
        if req.method.as_deref() != Some(Method::Hello.as_str()) {
            return Self::reject(writer, &req.id, Refusal::Auth(AuthError::OutOfOrder));
        }
        let hello = match HelloParams::parse(&req.params) {
            Ok(h) => h,
            Err(e) => return Self::reject(writer, &req.id, Refusal::Protocol(e)),
        };
        *instance = Some(hello.instance_id.clone());

        let record = self
            .inner
            .records
            .get(&hello.instance_id)
            .map_err(Refusal::Storage)?;
        let installed = record
            .as_ref()
            .and_then(|r| self.installed_dir(r, report, &hello));
        if let BuildVerdict::UnknownVersion(version) | BuildVerdict::HashMismatch(version) = self
            .inner
            .known
            .check(&hello.ext_version, installed.as_deref())
        {
            return Self::reject(writer, &req.id, Refusal::UnknownBuild { version });
        }

        let key = if hello.paired {
            self.inner
                .keys
                .get(&hello.instance_id)
                .map_err(Refusal::Storage)?
        } else {
            None
        };
        let now = self.now();
        let pairing_open = lock(&self.inner.pairing)
            .as_ref()
            .map(|p| p.window.is_open(now))
            .unwrap_or(false);
        let ctx = HelloContext {
            key: key.as_ref(),
            pairing_open,
            app_version: &self.inner.config.app_version,
        };
        let mut hs = AppHandshake::new();
        let result = {
            let mut log = lock(&self.inner.challenges);
            hs.on_hello(&hello, &ctx, &mut log)
        };
        let result = match result {
            Ok(r) => r,
            Err(e) => return Self::reject(writer, &req.id, Refusal::Auth(e)),
        };
        let reply = Envelope::response_ok(
            req.id.clone(),
            serde_json::to_value(&result).expect("a result serializes"),
        );
        Self::write_env(writer, &reply)?;

        let next = Self::read_env(reader)?;
        if let Err(e) = next.check_from(Sender::Extension) {
            return Self::reject(writer, &next.id, Refusal::Protocol(e));
        }
        let method = next.method.as_deref().unwrap_or_default();
        if hello.paired && method == Method::Auth.as_str() {
            self.finish_auth(&mut hs, &hello, &next, report, record, writer)
        } else if !hello.paired && method == Method::PairBegin.as_str() {
            self.finish_pairing(&mut hs, &hello, &next, report, writer)
        } else {
            Self::reject(writer, &next.id, Refusal::Auth(AuthError::OutOfOrder))
        }
    }

    fn installed_dir(
        &self,
        record: &LinkRecord,
        report: &PeerReport,
        hello: &HelloParams,
    ) -> Option<PathBuf> {
        let user_data_dir = record
            .user_data_dir
            .as_ref()
            .or(report.user_data_dir.as_ref())?;
        self.inner.config.extension_ids.iter().find_map(|id| {
            installed_extension_dir(user_data_dir, &record.profile_dir, id, &hello.ext_version)
        })
    }

    fn finish_auth(
        &self,
        hs: &mut AppHandshake,
        hello: &HelloParams,
        req: &Envelope,
        report: &PeerReport,
        record: Option<LinkRecord>,
        writer: &mut Box<dyn Write + Send>,
    ) -> Result<Outcome, Refusal> {
        let auth: AuthParams = match parse_params(&req.params) {
            Ok(a) => a,
            Err(e) => return Self::reject(writer, &req.id, Refusal::Protocol(e)),
        };
        let mut session = match hs.on_auth(&auth) {
            Ok(s) => s,
            Err(e) => return Self::reject(writer, &req.id, Refusal::Auth(e)),
        };
        // Only now is the extension authenticated: checks that tell the caller about our state
        // come after the proof, so an unauthenticated peer learns nothing from them.
        let record = match self.admit(hello, report, record) {
            Ok(r) => r,
            Err(refusal) => return Self::reject(writer, &req.id, refusal),
        };
        let result = self.inner.roster.snapshot();
        let mut reply = Envelope::response_ok(
            req.id.clone(),
            serde_json::to_value(&result).expect("a result serializes"),
        );
        if let Err(e) = session.seal(&mut reply) {
            self.release(&hello.instance_id);
            return Err(Refusal::Protocol(e));
        }
        if let Err(e) = Self::write_env(writer, &reply) {
            self.release(&hello.instance_id);
            return Err(e);
        }
        Ok(Outcome::Linked(Linked {
            session,
            instance_id: hello.instance_id.clone(),
            name: record.name,
        }))
    }

    fn release(&self, instance_id: &str) {
        let mut links = lock(&self.inner.links);
        if matches!(links.get(instance_id), Some(Slot::Reserved)) {
            links.remove(instance_id);
        }
    }

    /// Browser, data dir and duplicate checks. Reserves the slot, so two racing links with one
    /// `instance_id` cannot both pass.
    fn admit(
        &self,
        hello: &HelloParams,
        report: &PeerReport,
        record: Option<LinkRecord>,
    ) -> Result<LinkRecord, Refusal> {
        let record = record.ok_or(Refusal::NotPaired)?;
        let name = record.name.clone();
        if !record.brand.eq_ignore_ascii_case(&hello.browser.brand) {
            return Err(Refusal::BrowserMismatch { name });
        }
        if let (Some(want), Some(got)) = (&record.parent_exe, &report.parent_exe) {
            if want != got {
                return Err(Refusal::BrowserMismatch { name });
            }
        }
        if self.inner.config.check_data_dir && record.user_data_dir != report.user_data_dir {
            return Err(Refusal::DataDirMismatch { name });
        }
        let mut links = lock(&self.inner.links);
        if links.contains_key(&record.instance_id) {
            return Err(Refusal::Duplicate { name });
        }
        links.insert(record.instance_id.clone(), Slot::Reserved);
        Ok(record)
    }

    fn finish_pairing(
        &self,
        hs: &mut AppHandshake,
        hello: &HelloParams,
        req: &Envelope,
        report: &PeerReport,
        writer: &mut Box<dyn Write + Send>,
    ) -> Result<Outcome, Refusal> {
        let params: PairBeginParams = match parse_params(&req.params) {
            Ok(p) => p,
            Err(e) => return Self::reject(writer, &req.id, Refusal::Protocol(e)),
        };
        let now = self.now();
        let (result, key, profile) = {
            let mut pairing = lock(&self.inner.pairing);
            let Some(p) = pairing.as_mut() else {
                return Self::reject(writer, &req.id, Refusal::Auth(AuthError::PairingClosed));
            };
            match hs.on_pair_begin(&params, &mut p.window, now) {
                Ok((result, key)) => {
                    let profile = p.profile.clone();
                    *pairing = None;
                    (result, key, profile)
                }
                Err(e) => return Self::reject(writer, &req.id, Refusal::Auth(e)),
            }
        };
        let record = LinkRecord {
            instance_id: hello.instance_id.clone(),
            name: profile.name,
            profile_dir: profile.dir,
            brand: hello.browser.brand.clone(),
            parent_exe: report.parent_exe.clone(),
            user_data_dir: report.user_data_dir.clone(),
            paired_at_ms: now,
        };
        if let Err(e) = self.store_pairing(&key, &record) {
            return Self::reject(writer, &req.id, Refusal::Storage(e));
        }
        let reply = Envelope::response_ok(
            req.id.clone(),
            serde_json::to_value(&result).expect("a result serializes"),
        );
        Self::write_env(writer, &reply)?;
        self.emit(LinkEvent::Paired {
            instance_id: record.instance_id,
            name: record.name,
        });
        Ok(Outcome::Paired)
    }

    /// Saves the key first, then the record, and drops an older install of the same profile.
    fn store_pairing(
        &self,
        key: &super::super::link::auth::PairingKey,
        record: &LinkRecord,
    ) -> Result<(), String> {
        self.inner.keys.set(&record.instance_id, key)?;
        if let Err(e) = self.inner.records.save(record) {
            let _ = self.inner.keys.delete(&record.instance_id);
            return Err(e);
        }
        for old in self.inner.records.list()? {
            if old.instance_id != record.instance_id
                && old.profile_dir == record.profile_dir
                && old.user_data_dir == record.user_data_dir
            {
                self.disconnect(&old.instance_id)?;
            }
        }
        Ok(())
    }

    /// Serves an admitted link until it closes.
    fn run(&self, shared: Arc<Shared>, name: String, mut reader: Box<dyn Read + Send>) {
        {
            let mut links = lock(&self.inner.links);
            if !matches!(links.get(&shared.instance_id), Some(Slot::Reserved)) {
                // Disconnected between `auth` and now: do not start a link for a forgotten pairing.
                drop(links);
                shared.close();
                return;
            }
            links.insert(shared.instance_id.clone(), Slot::Live(shared.clone()));
        }
        self.emit(LinkEvent::Connected {
            instance_id: shared.instance_id.clone(),
            name,
        });
        loop {
            if shared.closed.load(Ordering::SeqCst) {
                break;
            }
            let payload = match read_frame(&mut reader, FrameFormat::SOCKET) {
                Ok(Some(p)) => p,
                _ => break,
            };
            let Ok(env) = Envelope::from_slice(&payload) else {
                break;
            };
            if env.check_from(Sender::Extension).is_err() {
                break;
            }
            // A bad MAC or an old seq closes the link (spec 5.3).
            if lock(&shared.session).open(&env).is_err() {
                break;
            }
            match env.kind {
                EnvelopeType::Res => {
                    let tx = lock(&shared.pending).remove(&env.id);
                    if let Some(tx) = tx {
                        let _ = tx.send(env);
                    }
                }
                EnvelopeType::Evt => self.emit(LinkEvent::Extension {
                    instance_id: shared.instance_id.clone(),
                    envelope: env,
                }),
                // Extension requests after `auth` do not exist.
                EnvelopeType::Req => break,
            }
        }
        shared.close();
        lock(&shared.pending).clear();
        {
            let mut links = lock(&self.inner.links);
            if matches!(links.get(&shared.instance_id), Some(Slot::Live(s)) if Arc::ptr_eq(s, &shared))
            {
                links.remove(&shared.instance_id);
            }
        }
        self.emit(LinkEvent::Disconnected {
            instance_id: shared.instance_id.clone(),
        });
    }
}
