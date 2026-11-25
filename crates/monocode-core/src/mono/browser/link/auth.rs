//! Mutual HMAC handshake, per-frame MAC with a replay counter, and the pairing nonce (spec 5.3,
//! 12.3). All comparisons of secrets and MACs are constant time.
//!
//! Wire definitions (also in `shared/handshake-vectors.json`, which the extension's tests use):
//!
//! - `K`: 32 random bytes, the pairing key. Challenges are 32 random bytes. All binary values are
//!   standard base64 with padding on the wire.
//! - app MAC: `HMAC-SHA256(K, "zptah-browser/1|app|" || ext_chal || app_chal || instance_id)`
//! - ext MAC: `HMAC-SHA256(K, "zptah-browser/1|ext|" || app_chal || ext_chal || instance_id)`
//!   where challenges are the raw 32 bytes and `instance_id` is its 32 ASCII hex characters.
//! - session key `S`: HKDF-SHA256 with ikm `K`, salt `ext_chal || app_chal` (64 bytes) and info
//!   `"zptah-browser/1 session"`, 32 bytes of output.
//! - frame MAC: `HMAC-SHA256(S, direction || seq || canonical)` where `direction` is one byte
//!   (1 extension to app, 2 app to extension), `seq` is 8 bytes big endian, and `canonical` is the
//!   UTF-8 of the envelope without `mac`, ASCII keys sorted, no whitespace, plain integers only
//!   (`1.0` is refused). The MAC covers the parsed envelope, not the received bytes.

use super::protocol::{
    negotiate, AuthParams, Envelope, ErrorCode, HelloParams, HelloResult, PairBeginParams,
    PairBeginResult, ProtocolError,
};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;
use std::collections::VecDeque;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

type HmacSha256 = Hmac<Sha256>;

pub const KEY_LEN: usize = 32;
pub const CHALLENGE_LEN: usize = 32;
pub const NONCE_LEN: usize = 16;
/// A pairing window lasts 5 minutes (spec 12.3).
pub const PAIRING_TTL_MS: u64 = 5 * 60 * 1000;
/// Wrong nonces tolerated before the window closes.
pub const PAIRING_MAX_ATTEMPTS: u32 = 5;
const APP_DOMAIN: &[u8] = b"zptah-browser/1|app|";
const EXT_DOMAIN: &[u8] = b"zptah-browser/1|ext|";
const SESSION_INFO: &[u8] = b"zptah-browser/1 session";
/// Largest integer both Rust and JavaScript represent exactly.
const MAX_SAFE_INT: i64 = 9_007_199_254_740_991;

/// Why a handshake, pairing or frame check failed. `code()` is what goes on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthError {
    /// The peer's MAC did not verify (wrong key or tampered handshake).
    BadMac,
    /// A frame MAC did not verify.
    BadFrameMac,
    /// A frame carries a `seq` that is not above the last one, or no `seq` or `mac` at all.
    Replay,
    /// This extension challenge was already used in an earlier hello.
    ChallengeReplayed,
    /// The step arrived out of order (for example `auth` before `hello`).
    OutOfOrder,
    /// No key for this extension, or it is not paired.
    NotPaired,
    PairingClosed,
    PairingExpired,
    PairingNonceUsed,
    PairingBadNonce,
    Protocol(ProtocolError),
}

impl AuthError {
    pub fn code(&self) -> ErrorCode {
        match self {
            AuthError::NotPaired
            | AuthError::PairingClosed
            | AuthError::PairingExpired
            | AuthError::PairingNonceUsed
            | AuthError::PairingBadNonce => ErrorCode::NotPaired,
            AuthError::Protocol(e) => e.code,
            _ => ErrorCode::Auth,
        }
    }

    pub fn to_protocol_error(&self) -> ProtocolError {
        match self {
            AuthError::Protocol(e) => e.clone(),
            other => ProtocolError::new(other.code(), other.to_string()),
        }
    }
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            AuthError::BadMac => "authentication failed",
            AuthError::BadFrameMac => "frame authentication failed",
            AuthError::Replay => "frame replayed or out of order",
            AuthError::ChallengeReplayed => "challenge replayed",
            AuthError::OutOfOrder => "handshake step out of order",
            AuthError::NotPaired => "extension is not paired",
            AuthError::PairingClosed => "no pairing in progress",
            AuthError::PairingExpired => "pairing window expired",
            AuthError::PairingNonceUsed => "pairing nonce already used",
            AuthError::PairingBadNonce => "pairing nonce does not match",
            AuthError::Protocol(e) => return write!(f, "{e}"),
        };
        f.write_str(text)
    }
}
impl std::error::Error for AuthError {}

impl From<ProtocolError> for AuthError {
    fn from(e: ProtocolError) -> Self {
        AuthError::Protocol(e)
    }
}

/// The pairing key K. Zeroed on drop and never printed.
#[derive(Clone)]
pub struct PairingKey([u8; KEY_LEN]);

impl PairingKey {
    pub fn generate() -> Self {
        Self(rand::random())
    }
    pub fn from_bytes(bytes: [u8; KEY_LEN]) -> Self {
        Self(bytes)
    }
    pub fn from_b64(text: &str) -> Result<Self, ProtocolError> {
        Ok(Self(decode_fixed::<KEY_LEN>(text, "key")?))
    }
    pub fn to_b64(&self) -> String {
        B64.encode(self.0)
    }
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl Drop for PairingKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for PairingKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingKey(<redacted>)")
    }
}

/// The per-link session key S.
#[derive(Clone)]
pub struct SessionKey([u8; KEY_LEN]);

impl SessionKey {
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl Drop for SessionKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionKey(<redacted>)")
    }
}

pub type Challenge = [u8; CHALLENGE_LEN];

pub fn new_challenge() -> Challenge {
    rand::random()
}

pub fn encode_bytes(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

fn decode_fixed<const N: usize>(text: &str, what: &str) -> Result<[u8; N], ProtocolError> {
    let bad = || ProtocolError::invalid(format!("`{what}` is not {N} bytes of base64"));
    let bytes = B64.decode(text).map_err(|_| bad())?;
    bytes.try_into().map_err(|_| bad())
}

/// Decodes a 32-byte challenge from its base64 text.
pub fn decode_challenge(text: &str) -> Result<Challenge, ProtocolError> {
    decode_fixed::<CHALLENGE_LEN>(text, "challenge")
}

fn mac_of(key: &[u8], parts: &[&[u8]]) -> HmacSha256 {
    let mut mac = <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC takes any key length");
    for part in parts {
        mac.update(part);
    }
    mac
}

fn app_parts<'a>(ext: &'a Challenge, app: &'a Challenge, id: &'a str) -> [&'a [u8]; 4] {
    [APP_DOMAIN, ext, app, id.as_bytes()]
}

fn ext_parts<'a>(ext: &'a Challenge, app: &'a Challenge, id: &'a str) -> [&'a [u8]; 4] {
    [EXT_DOMAIN, app, ext, id.as_bytes()]
}

/// The app's proof of `K` in the `hello` response.
pub fn app_mac(key: &PairingKey, ext: &Challenge, app: &Challenge, instance_id: &str) -> [u8; 32] {
    mac_of(key.as_bytes(), &app_parts(ext, app, instance_id))
        .finalize()
        .into_bytes()
        .into()
}

/// The extension's proof of `K` in the `auth` request.
pub fn ext_mac(key: &PairingKey, ext: &Challenge, app: &Challenge, instance_id: &str) -> [u8; 32] {
    mac_of(key.as_bytes(), &ext_parts(ext, app, instance_id))
        .finalize()
        .into_bytes()
        .into()
}

fn verify_b64(key: &[u8], parts: &[&[u8]], mac_b64: &str) -> bool {
    match B64.decode(mac_b64) {
        // `verify_slice` compares in constant time.
        Ok(bytes) => mac_of(key, parts).verify_slice(&bytes).is_ok(),
        Err(_) => false,
    }
}

pub fn session_key(key: &PairingKey, ext: &Challenge, app: &Challenge) -> SessionKey {
    let mut salt = [0u8; CHALLENGE_LEN * 2];
    salt[..CHALLENGE_LEN].copy_from_slice(ext);
    salt[CHALLENGE_LEN..].copy_from_slice(app);
    let hk = Hkdf::<Sha256>::new(Some(&salt), key.as_bytes());
    let mut out = [0u8; KEY_LEN];
    hk.expand(SESSION_INFO, &mut out)
        .expect("32 bytes is a valid HKDF-SHA256 length");
    SessionKey(out)
}

/// Direction byte mixed into every frame MAC so a frame cannot be reflected back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    ExtensionToApp = 1,
    AppToExtension = 2,
}

impl Direction {
    pub fn opposite(self) -> Direction {
        match self {
            Direction::ExtensionToApp => Direction::AppToExtension,
            Direction::AppToExtension => Direction::ExtensionToApp,
        }
    }
}

/// Canonical JSON for MACs: object keys sorted, no whitespace, plain integers in the safe range and
/// ASCII object keys, so the bytes match `JSON.stringify` with sorted keys in JavaScript. It runs on
/// untrusted frames before the MAC check, so it must never panic.
pub fn canonical_json(value: &Value) -> Result<String, ProtocolError> {
    let mut out = String::new();
    write_canonical(value, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &Value, out: &mut String) -> Result<(), ProtocolError> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            // Only values JSON writes as plain integers: `1.0` and `1e2` are refused, so the
            // MAC input never depends on how a float was spelled.
            let bad = || ProtocolError::invalid("numbers must be safe integers");
            let int = if let Some(i) = n.as_i64() {
                i
            } else if let Some(u) = n.as_u64() {
                i64::try_from(u).map_err(|_| bad())?
            } else {
                return Err(bad());
            };
            if int.unsigned_abs() > MAX_SAFE_INT as u64 {
                return Err(bad());
            }
            out.push_str(&int.to_string());
        }
        Value::String(s) => {
            out.push_str(&serde_json::to_string(s).expect("a string always serializes"))
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            // ASCII keys sort the same by byte (Rust) and by UTF-16 unit (JavaScript).
            if map.keys().any(|k| !k.is_ascii()) {
                return Err(ProtocolError::invalid("object keys must be ASCII"));
            }
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).expect("a string always serializes"));
                out.push(':');
                write_canonical(&map[key], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

/// The bytes a frame MAC covers, for tests and vectors.
pub fn frame_mac_input(
    direction: Direction,
    envelope: &Envelope,
) -> Result<Vec<u8>, ProtocolError> {
    let seq = envelope
        .seq
        .ok_or_else(|| ProtocolError::invalid("frame has no `seq`"))?;
    let mut unsigned = envelope.clone();
    unsigned.mac = None;
    let value = serde_json::to_value(&unsigned).expect("an envelope always serializes");
    let canonical = canonical_json(&value)?;
    let mut input = Vec::with_capacity(9 + canonical.len());
    input.push(direction as u8);
    input.extend_from_slice(&seq.to_be_bytes());
    input.extend_from_slice(canonical.as_bytes());
    Ok(input)
}

/// An authenticated link: signs outgoing frames and checks incoming ones. A failure of `open`
/// means the link must be closed.
#[derive(Debug)]
pub struct Session {
    key: SessionKey,
    local: Direction,
    sent: u64,
    received: u64,
}

impl Session {
    pub fn new(key: SessionKey, local: Direction) -> Self {
        Self {
            key,
            local,
            sent: 0,
            received: 0,
        }
    }

    /// Stamps `seq` (1, 2, ...) and `mac` onto an outgoing frame.
    pub fn seal(&mut self, envelope: &mut Envelope) -> Result<(), ProtocolError> {
        envelope.seq = Some(self.sent + 1);
        envelope.mac = None;
        let input = frame_mac_input(self.local, envelope)?;
        let tag = mac_of(self.key.as_bytes(), &[&input])
            .finalize()
            .into_bytes();
        envelope.mac = Some(B64.encode(tag));
        self.sent += 1;
        Ok(())
    }

    /// Verifies an incoming frame: MAC first, then the counter, which must be exactly the next one. State changes only on success.
    pub fn open(&mut self, envelope: &Envelope) -> Result<(), AuthError> {
        let (Some(seq), Some(mac)) = (envelope.seq, envelope.mac.as_deref()) else {
            return Err(AuthError::Replay);
        };
        let input = frame_mac_input(self.local.opposite(), envelope)?;
        if !verify_b64(self.key.as_bytes(), &[&input], mac) {
            return Err(AuthError::BadFrameMac);
        }
        if seq != self.received + 1 {
            return Err(AuthError::Replay);
        }
        self.received = seq;
        Ok(())
    }
}

/// Remembers recent extension challenges so a recorded `hello` cannot be replayed.
#[derive(Debug)]
pub struct ChallengeLog {
    seen: VecDeque<Challenge>,
    capacity: usize,
}

impl ChallengeLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            seen: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// Records the challenge, or fails when it was already seen.
    pub fn record(&mut self, challenge: &Challenge) -> Result<(), AuthError> {
        if self
            .seen
            .iter()
            .any(|c| bool::from(c.as_slice().ct_eq(challenge.as_slice())))
        {
            return Err(AuthError::ChallengeReplayed);
        }
        if self.seen.len() == self.capacity {
            self.seen.pop_front();
        }
        self.seen.push_back(*challenge);
        Ok(())
    }
}

impl Default for ChallengeLog {
    fn default() -> Self {
        Self::new(1024)
    }
}

/// What the app knows when a `hello` arrives.
pub struct HelloContext<'a> {
    /// The stored key for the hello's `instance_id`, if paired.
    pub key: Option<&'a PairingKey>,
    /// A pairing window is open (spec 12.3).
    pub pairing_open: bool,
    pub app_version: &'a str,
}

#[derive(Debug)]
enum AppState {
    Fresh,
    AwaitAuth {
        key: PairingKey,
        ext: Challenge,
        app: Challenge,
        instance_id: String,
    },
    AwaitPair {
        instance_id: String,
    },
    Done,
}

/// App side of one connection's handshake. Single use: after `auth` succeeds or any step fails,
/// a new connection needs a new handshake.
#[derive(Debug)]
pub struct AppHandshake {
    state: AppState,
}

impl Default for AppHandshake {
    fn default() -> Self {
        Self::new()
    }
}

impl AppHandshake {
    pub fn new() -> Self {
        Self {
            state: AppState::Fresh,
        }
    }

    pub fn on_hello(
        &mut self,
        hello: &HelloParams,
        ctx: &HelloContext<'_>,
        log: &mut ChallengeLog,
    ) -> Result<HelloResult, AuthError> {
        self.on_hello_with(hello, ctx, log, new_challenge())
    }

    /// Like `on_hello` with a chosen app challenge (vectors and tests).
    pub fn on_hello_with(
        &mut self,
        hello: &HelloParams,
        ctx: &HelloContext<'_>,
        log: &mut ChallengeLog,
        app_challenge: Challenge,
    ) -> Result<HelloResult, AuthError> {
        if !matches!(self.state, AppState::Fresh) {
            self.state = AppState::Done;
            return Err(AuthError::OutOfOrder);
        }
        // Whatever happens next, this connection does not get a second hello.
        self.state = AppState::Done;
        if !super::protocol::is_hex32(&hello.instance_id) {
            return Err(ProtocolError::invalid("`instance_id` is not 32 hex").into());
        }
        let protocol = negotiate(hello.protocol)?;
        let ext = decode_challenge(&hello.challenge)?;
        if !hello.paired {
            if !ctx.pairing_open {
                return Err(AuthError::NotPaired);
            }
            log.record(&ext)?;
            self.state = AppState::AwaitPair {
                instance_id: hello.instance_id.clone(),
            };
            return Ok(HelloResult {
                protocol,
                app_version: ctx.app_version.to_string(),
                challenge: encode_bytes(&app_challenge),
                mac: None,
            });
        }
        let key = ctx.key.ok_or(AuthError::NotPaired)?;
        log.record(&ext)?;
        let mac = app_mac(key, &ext, &app_challenge, &hello.instance_id);
        self.state = AppState::AwaitAuth {
            key: key.clone(),
            ext,
            app: app_challenge,
            instance_id: hello.instance_id.clone(),
        };
        Ok(HelloResult {
            protocol,
            app_version: ctx.app_version.to_string(),
            challenge: encode_bytes(&app_challenge),
            mac: Some(encode_bytes(&mac)),
        })
    }

    /// Verifies the extension's proof and returns the app side of the session.
    pub fn on_auth(&mut self, auth: &AuthParams) -> Result<Session, AuthError> {
        let state = std::mem::replace(&mut self.state, AppState::Done);
        let AppState::AwaitAuth {
            key,
            ext,
            app,
            instance_id,
        } = state
        else {
            return Err(AuthError::OutOfOrder);
        };
        if !verify_b64(
            key.as_bytes(),
            &ext_parts(&ext, &app, &instance_id),
            &auth.mac,
        ) {
            return Err(AuthError::BadMac);
        }
        Ok(Session::new(
            session_key(&key, &ext, &app),
            Direction::AppToExtension,
        ))
    }

    /// Handles `pair.begin` on a connection that said `paired: false` while a window is open.
    /// Returns the result to send and the new key to store. The connection is finished; the
    /// extension reconnects and runs the normal handshake.
    pub fn on_pair_begin(
        &mut self,
        params: &PairBeginParams,
        window: &mut PairingWindow,
        now_ms: u64,
    ) -> Result<(PairBeginResult, PairingKey), AuthError> {
        let state = std::mem::replace(&mut self.state, AppState::Done);
        let AppState::AwaitPair { instance_id } = state else {
            return Err(AuthError::OutOfOrder);
        };
        if !super::protocol::is_hex32(&params.instance_id) {
            return Err(ProtocolError::invalid("`instance_id` is not 32 hex").into());
        }
        if instance_id != params.instance_id {
            return Err(AuthError::PairingBadNonce);
        }
        window.consume(&params.nonce, now_ms)?;
        let key = PairingKey::generate();
        let result = PairBeginResult {
            key: super::protocol::SecretString::new(key.to_b64()),
        };
        Ok((result, key))
    }
}

#[derive(Debug)]
enum ExtState {
    Fresh,
    Sent,
    AwaitAuthResult {
        key: PairingKey,
        ext: Challenge,
        app: Challenge,
    },
    Done,
}

/// Extension side of the handshake, for the in-process FakeExtension and tests. The real
/// extension implements the same steps in JavaScript against the shared vectors.
#[derive(Debug)]
pub struct ExtHandshake {
    key: PairingKey,
    instance_id: String,
    challenge: Challenge,
    state: ExtState,
}

impl ExtHandshake {
    pub fn new(key: PairingKey, instance_id: impl Into<String>) -> Self {
        Self::with_challenge(key, instance_id, new_challenge())
    }

    pub fn with_challenge(
        key: PairingKey,
        instance_id: impl Into<String>,
        challenge: Challenge,
    ) -> Self {
        Self {
            key,
            instance_id: instance_id.into(),
            challenge,
            state: ExtState::Fresh,
        }
    }

    pub fn hello(&mut self, paired: bool, ext_version: &str, brand: &str) -> HelloParams {
        self.state = ExtState::Sent;
        HelloParams {
            protocol: [
                super::protocol::BROWSER_PROTOCOL.0,
                super::protocol::BROWSER_PROTOCOL.1,
            ],
            ext_version: ext_version.to_string(),
            browser: super::protocol::BrowserInfo {
                brand: brand.to_string(),
                version: "0".to_string(),
            },
            instance_id: self.instance_id.clone(),
            paired,
            challenge: encode_bytes(&self.challenge),
        }
    }

    /// Checks the app's MAC and answers with the extension's.
    pub fn on_hello_result(&mut self, result: &HelloResult) -> Result<AuthParams, AuthError> {
        if !matches!(self.state, ExtState::Sent) {
            self.state = ExtState::Done;
            return Err(AuthError::OutOfOrder);
        }
        self.state = ExtState::Done;
        let app = decode_challenge(&result.challenge)?;
        let mac = result.mac.as_deref().ok_or(AuthError::BadMac)?;
        if !verify_b64(
            self.key.as_bytes(),
            &app_parts(&self.challenge, &app, &self.instance_id),
            mac,
        ) {
            return Err(AuthError::BadMac);
        }
        let ext = self.challenge;
        let proof = ext_mac(&self.key, &ext, &app, &self.instance_id);
        self.state = ExtState::AwaitAuthResult {
            key: self.key.clone(),
            ext,
            app,
        };
        Ok(AuthParams {
            mac: encode_bytes(&proof),
        })
    }

    /// Call after the app accepted `auth`.
    pub fn into_session(self) -> Result<Session, AuthError> {
        match self.state {
            ExtState::AwaitAuthResult { key, ext, app } => Ok(Session::new(
                session_key(&key, &ext, &app),
                Direction::ExtensionToApp,
            )),
            _ => Err(AuthError::OutOfOrder),
        }
    }
}

/// A pairing window: 5 minutes, one 128-bit nonce, usable once.
#[derive(Debug)]
pub struct PairingWindow {
    nonce: [u8; NONCE_LEN],
    expires_ms: u64,
    used: bool,
    attempts: u32,
}

impl PairingWindow {
    /// Opens a window at `now_ms` and returns it with the nonce to show the extension page.
    pub fn open(now_ms: u64) -> (PairingWindow, String) {
        Self::open_with(now_ms, rand::random())
    }

    pub fn open_with(now_ms: u64, nonce: [u8; NONCE_LEN]) -> (PairingWindow, String) {
        let text = encode_bytes(&nonce);
        (
            PairingWindow {
                nonce,
                expires_ms: now_ms.saturating_add(PAIRING_TTL_MS),
                used: false,
                attempts: 0,
            },
            text,
        )
    }

    pub fn is_open(&self, now_ms: u64) -> bool {
        !self.used && now_ms < self.expires_ms && self.attempts < PAIRING_MAX_ATTEMPTS
    }

    /// Consumes the nonce. A correct nonce works once; a wrong one counts against the window.
    pub fn consume(&mut self, nonce_b64: &str, now_ms: u64) -> Result<(), AuthError> {
        if self.used {
            return Err(AuthError::PairingNonceUsed);
        }
        if now_ms >= self.expires_ms {
            return Err(AuthError::PairingExpired);
        }
        if self.attempts >= PAIRING_MAX_ATTEMPTS {
            return Err(AuthError::PairingClosed);
        }
        let given = decode_fixed::<NONCE_LEN>(nonce_b64, "nonce").ok();
        let matches = given
            .map(|g| bool::from(g.as_slice().ct_eq(self.nonce.as_slice())))
            .unwrap_or(false);
        if !matches {
            self.attempts += 1;
            return Err(AuthError::PairingBadNonce);
        }
        self.used = true;
        Ok(())
    }
}
