//! Envelope, method and event names, error codes and version negotiation (spec 5.2, 5.3, 5.5, 5.6).

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// The only protocol major this build speaks.
pub const PROTOCOL_VERSION: u32 = 1;
/// The protocol range the app supports (`BROWSER_PROTOCOL = 1..=1`, spec 13.5).
pub const BROWSER_PROTOCOL: (u32, u32) = (1, 1);
/// Longest allowed envelope id, e.g. `a-1234`.
pub const MAX_ID_LEN: usize = 32;

/// Every error code of the protocol, serialized exactly as the spec writes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    #[serde(rename = "E_PROTOCOL")]
    Protocol,
    #[serde(rename = "E_AUTH")]
    Auth,
    #[serde(rename = "E_NOT_PAIRED")]
    NotPaired,
    #[serde(rename = "E_UNKNOWN_MONO")]
    UnknownMono,
    #[serde(rename = "E_NOT_OWNER")]
    NotOwner,
    #[serde(rename = "E_BUSY")]
    Busy,
    #[serde(rename = "E_TAB_GONE")]
    TabGone,
    #[serde(rename = "E_RESTRICTED_URL")]
    RestrictedUrl,
    #[serde(rename = "E_ORIGIN_BLOCKED")]
    OriginBlocked,
    #[serde(rename = "E_STALE_REF")]
    StaleRef,
    #[serde(rename = "E_STALE_PREPARE")]
    StalePrepare,
    #[serde(rename = "E_STOPPED")]
    Stopped,
    #[serde(rename = "E_PASSWORD_FIELD")]
    PasswordField,
    #[serde(rename = "E_INCOGNITO")]
    Incognito,
    #[serde(rename = "E_DEBUGGER_DENIED")]
    DebuggerDenied,
    #[serde(rename = "E_DEBUGGER_BUSY")]
    DebuggerBusy,
    #[serde(rename = "E_NEEDS_APPROVAL")]
    NeedsApproval,
    #[serde(rename = "E_TOO_LARGE")]
    TooLarge,
    #[serde(rename = "E_TIMEOUT")]
    Timeout,
    #[serde(rename = "E_INVALID")]
    Invalid,
    #[serde(rename = "E_UNSUPPORTED")]
    Unsupported,
    #[serde(rename = "E_DIALOG_SUSPECTED")]
    DialogSuspected,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 22] = [
        ErrorCode::Protocol,
        ErrorCode::Auth,
        ErrorCode::NotPaired,
        ErrorCode::UnknownMono,
        ErrorCode::NotOwner,
        ErrorCode::Busy,
        ErrorCode::TabGone,
        ErrorCode::RestrictedUrl,
        ErrorCode::OriginBlocked,
        ErrorCode::StaleRef,
        ErrorCode::StalePrepare,
        ErrorCode::Stopped,
        ErrorCode::PasswordField,
        ErrorCode::Incognito,
        ErrorCode::DebuggerDenied,
        ErrorCode::DebuggerBusy,
        ErrorCode::NeedsApproval,
        ErrorCode::TooLarge,
        ErrorCode::Timeout,
        ErrorCode::Invalid,
        ErrorCode::Unsupported,
        ErrorCode::DialogSuspected,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Protocol => "E_PROTOCOL",
            ErrorCode::Auth => "E_AUTH",
            ErrorCode::NotPaired => "E_NOT_PAIRED",
            ErrorCode::UnknownMono => "E_UNKNOWN_MONO",
            ErrorCode::NotOwner => "E_NOT_OWNER",
            ErrorCode::Busy => "E_BUSY",
            ErrorCode::TabGone => "E_TAB_GONE",
            ErrorCode::RestrictedUrl => "E_RESTRICTED_URL",
            ErrorCode::OriginBlocked => "E_ORIGIN_BLOCKED",
            ErrorCode::StaleRef => "E_STALE_REF",
            ErrorCode::StalePrepare => "E_STALE_PREPARE",
            ErrorCode::Stopped => "E_STOPPED",
            ErrorCode::PasswordField => "E_PASSWORD_FIELD",
            ErrorCode::Incognito => "E_INCOGNITO",
            ErrorCode::DebuggerDenied => "E_DEBUGGER_DENIED",
            ErrorCode::DebuggerBusy => "E_DEBUGGER_BUSY",
            ErrorCode::NeedsApproval => "E_NEEDS_APPROVAL",
            ErrorCode::TooLarge => "E_TOO_LARGE",
            ErrorCode::Timeout => "E_TIMEOUT",
            ErrorCode::Invalid => "E_INVALID",
            ErrorCode::Unsupported => "E_UNSUPPORTED",
            ErrorCode::DialogSuspected => "E_DIALOG_SUSPECTED",
        }
    }
}

/// The `error` member of a failed response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: ErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
}

/// A protocol violation, carrying the code to answer with.
#[derive(Debug, Clone, PartialEq)]
pub struct ProtocolError {
    pub code: ErrorCode,
    pub message: String,
    pub detail: Option<Value>,
}

impl ProtocolError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            detail: None,
        }
    }
    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::Invalid, message)
    }
    pub fn with_detail(mut self, detail: Value) -> Self {
        self.detail = Some(detail);
        self
    }
    pub fn to_body(&self) -> ErrorBody {
        ErrorBody {
            code: self.code,
            message: self.message.clone(),
            detail: self.detail.clone(),
        }
    }
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.message)
    }
}
impl std::error::Error for ProtocolError {}

/// Who sent a message. Decides which requests and events are legal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sender {
    App,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EnvelopeType {
    Req,
    Res,
    Evt,
}

/// Methods in the spec's tables (5.3, 5.5). `Hello`, `Auth` and `PairBegin` are the only
/// extension-originated requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Hello,
    Auth,
    PairBegin,
    RosterSet,
    StatusGet,
    StopResume,
    TabsList,
    TabOpen,
    TabClose,
    TabAdopt,
    TabRelease,
    PageSnapshot,
    PageRead,
    PageFind,
    PageScreenshot,
    PageWait,
    NavGo,
    NavHistory,
    ActPrepare,
    ActCommit,
    ActExtend,
    ActCancel,
    DialogRespond,
    UploadPut,
    /// A run of the Mono ended (the debugger detaches 2 minutes later, decision 4).
    TaskEnd,
    CursorPrefs,
}

impl Method {
    pub const ALL: [Method; 26] = [
        Method::Hello,
        Method::Auth,
        Method::PairBegin,
        Method::RosterSet,
        Method::StatusGet,
        Method::StopResume,
        Method::TabsList,
        Method::TabOpen,
        Method::TabClose,
        Method::TabAdopt,
        Method::TabRelease,
        Method::PageSnapshot,
        Method::PageRead,
        Method::PageFind,
        Method::PageScreenshot,
        Method::PageWait,
        Method::NavGo,
        Method::NavHistory,
        Method::ActPrepare,
        Method::ActCommit,
        Method::ActExtend,
        Method::ActCancel,
        Method::DialogRespond,
        Method::UploadPut,
        Method::TaskEnd,
        Method::CursorPrefs,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Method::Hello => "hello",
            Method::Auth => "auth",
            Method::PairBegin => "pair.begin",
            Method::RosterSet => "roster.set",
            Method::StatusGet => "status.get",
            Method::StopResume => "stop.resume",
            Method::TabsList => "tabs.list",
            Method::TabOpen => "tab.open",
            Method::TabClose => "tab.close",
            Method::TabAdopt => "tab.adopt",
            Method::TabRelease => "tab.release",
            Method::PageSnapshot => "page.snapshot",
            Method::PageRead => "page.read",
            Method::PageFind => "page.find",
            Method::PageScreenshot => "page.screenshot",
            Method::PageWait => "page.wait",
            Method::NavGo => "nav.go",
            Method::NavHistory => "nav.history",
            Method::ActPrepare => "act.prepare",
            Method::ActCommit => "act.commit",
            Method::ActExtend => "act.extend",
            Method::ActCancel => "act.cancel",
            Method::DialogRespond => "dialog.respond",
            Method::UploadPut => "upload.put",
            Method::TaskEnd => "task.end",
            Method::CursorPrefs => "cursor.prefs",
        }
    }

    /// Exact match only: names are case sensitive and never trimmed.
    pub fn parse(name: &str) -> Result<Method, ProtocolError> {
        Method::ALL
            .into_iter()
            .find(|m| m.as_str() == name)
            .ok_or_else(|| ProtocolError::new(ErrorCode::Unsupported, "unknown method"))
    }

    /// True for the fixed set the extension may request (spec 5.2).
    pub fn extension_originated(self) -> bool {
        matches!(self, Method::Hello | Method::Auth | Method::PairBegin)
    }

    /// Mono-scoped methods must name a Mono (spec 5.5).
    pub fn requires_mono(self) -> bool {
        matches!(
            self,
            Method::TabsList
                | Method::TabOpen
                | Method::TabClose
                | Method::TabAdopt
                | Method::TabRelease
                | Method::PageSnapshot
                | Method::PageRead
                | Method::PageFind
                | Method::PageScreenshot
                | Method::PageWait
                | Method::NavGo
                | Method::NavHistory
                | Method::ActPrepare
                | Method::ActCommit
                | Method::ActExtend
                | Method::ActCancel
                | Method::DialogRespond
                | Method::UploadPut
                | Method::TaskEnd
        )
    }
}

/// Events sent by the extension (spec 5.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    TabUpdated,
    TabRemoved,
    TabOwnership,
    Stop,
    TabStopped,
    DialogOpened,
    DebuggerDetached,
    CursorChanged,
    /// The late result of a commit that ended as `E_DIALOG_SUSPECTED`.
    ActLate,
}

impl Event {
    pub const ALL: [Event; 9] = [
        Event::TabUpdated,
        Event::TabRemoved,
        Event::TabOwnership,
        Event::Stop,
        Event::TabStopped,
        Event::DialogOpened,
        Event::DebuggerDetached,
        Event::CursorChanged,
        Event::ActLate,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Event::TabUpdated => "tab.updated",
            Event::TabRemoved => "tab.removed",
            Event::TabOwnership => "tab.ownership",
            Event::Stop => "stop",
            Event::TabStopped => "tab.stopped",
            Event::DialogOpened => "dialog.opened",
            Event::DebuggerDetached => "debugger.detached",
            Event::CursorChanged => "cursor.changed",
            Event::ActLate => "act.late",
        }
    }

    pub fn parse(name: &str) -> Result<Event, ProtocolError> {
        Event::ALL
            .into_iter()
            .find(|e| e.as_str() == name)
            .ok_or_else(|| ProtocolError::new(ErrorCode::Unsupported, "unknown event"))
    }
}

/// Every message. Unknown fields are ignored on read (spec 5.2), so a newer peer may add fields.
/// `seq` and `mac` are present on every frame after `auth` (spec 5.3).
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope {
    pub v: u32,
    #[serde(rename = "type")]
    pub kind: EnvelopeType,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ok: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
}

/// `params` and `result` can carry the pairing key (`pair.begin`), so Debug never prints them.
impl std::fmt::Debug for Envelope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Envelope")
            .field("v", &self.v)
            .field("type", &self.kind)
            .field("id", &self.id)
            .field("method", &self.method)
            .field("mono", &self.mono)
            .field("params", &self.params.as_ref().map(|_| "<redacted>"))
            .field("ok", &self.ok)
            .field("result", &self.result.as_ref().map(|_| "<redacted>"))
            .field("error", &self.error.as_ref().map(|e| e.code))
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

impl Envelope {
    pub fn request(id: impl Into<String>, method: Method, params: Value) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: EnvelopeType::Req,
            id: id.into(),
            method: Some(method.as_str().to_string()),
            mono: None,
            params: Some(params),
            ok: None,
            result: None,
            error: None,
            seq: None,
            mac: None,
        }
    }

    pub fn event(id: impl Into<String>, event: Event, params: Value) -> Self {
        Self {
            kind: EnvelopeType::Evt,
            method: Some(event.as_str().to_string()),
            ..Self::request(id, Method::Hello, params)
        }
    }

    pub fn response_ok(id: impl Into<String>, result: Value) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: EnvelopeType::Res,
            id: id.into(),
            method: None,
            mono: None,
            params: None,
            ok: Some(true),
            result: Some(result),
            error: None,
            seq: None,
            mac: None,
        }
    }

    pub fn response_err(id: impl Into<String>, error: &ProtocolError) -> Self {
        Self {
            ok: Some(false),
            result: None,
            error: Some(error.to_body()),
            ..Self::response_ok(id, Value::Null)
        }
    }

    pub fn with_mono(mut self, mono: impl Into<String>) -> Self {
        self.mono = Some(mono.into());
        self
    }

    /// Parses one frame payload: UTF-8 JSON, one object, valid per [`Envelope::validate`] shape
    /// rules. Missing or mistyped required members give `E_INVALID`; an unsupported `v` gives
    /// `E_PROTOCOL`. Unknown members are ignored.
    pub fn from_slice(bytes: &[u8]) -> Result<Envelope, ProtocolError> {
        let value: Value = serde_json::from_slice(bytes)
            .map_err(|_| ProtocolError::invalid("frame is not valid JSON"))?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Envelope, ProtocolError> {
        let Value::Object(map) = &value else {
            return Err(ProtocolError::invalid("frame is not a JSON object"));
        };
        for key in ["v", "type", "id"] {
            if !map.contains_key(key) {
                return Err(ProtocolError::invalid(format!("missing member `{key}`")));
            }
        }
        match map.get("v") {
            Some(Value::Number(n)) if n.as_u64() == Some(PROTOCOL_VERSION as u64) => {}
            Some(Value::Number(_)) => {
                return Err(
                    ProtocolError::new(ErrorCode::Protocol, "unsupported protocol version")
                        .with_detail(supported_detail()),
                )
            }
            _ => return Err(ProtocolError::invalid("`v` must be an integer")),
        }
        let envelope: Envelope = serde_json::from_value(value)
            .map_err(|_| ProtocolError::invalid("malformed envelope"))?;
        envelope.validate()?;
        Ok(envelope)
    }

    pub fn to_vec(&self) -> Vec<u8> {
        serde_json::to_vec(self).expect("an envelope always serializes")
    }

    /// Structural rules that hold for every message, independent of who sent it.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.v != PROTOCOL_VERSION {
            return Err(
                ProtocolError::new(ErrorCode::Protocol, "unsupported protocol version")
                    .with_detail(supported_detail()),
            );
        }
        validate_id(&self.id)?;
        if let Some(mono) = &self.mono {
            if !is_hex32(mono) {
                return Err(ProtocolError::invalid("`mono` is not a 32 hex id"));
            }
        }
        if let Some(params) = &self.params {
            if !params.is_object() {
                return Err(ProtocolError::invalid("`params` must be an object"));
            }
        }
        if let Some(mac) = &self.mac {
            if mac.len() > 64 {
                return Err(ProtocolError::invalid("`mac` is too long"));
            }
        }
        match self.kind {
            EnvelopeType::Req | EnvelopeType::Evt => {
                let name = self
                    .method
                    .as_deref()
                    .filter(|m| !m.is_empty() && m.len() <= 64)
                    .ok_or_else(|| ProtocolError::invalid("`method` is required"))?;
                let _ = name;
                if self.ok.is_some() || self.result.is_some() || self.error.is_some() {
                    return Err(ProtocolError::invalid("a request carries no result"));
                }
            }
            EnvelopeType::Res => {
                match self.ok {
                    Some(true) => {
                        if !matches!(self.result, Some(Value::Object(_))) {
                            return Err(ProtocolError::invalid("ok response needs `result`"));
                        }
                    }
                    Some(false) => {
                        if self.error.is_none() {
                            return Err(ProtocolError::invalid("failed response needs `error`"));
                        }
                    }
                    None => return Err(ProtocolError::invalid("response needs `ok`")),
                }
                if self.method.is_some() || self.params.is_some() {
                    return Err(ProtocolError::invalid("a response carries no method"));
                }
            }
        }
        Ok(())
    }

    /// Checks that `sender` may send this message: `req` ids carry the sender's prefix and name a
    /// method that sender may call, `evt` comes only from the extension, `res` ids carry the
    /// other side's prefix. Mono-scoped requests must name a Mono. Unknown names give
    /// `E_UNSUPPORTED`.
    pub fn check_from(&self, sender: Sender) -> Result<(), ProtocolError> {
        self.validate()?;
        let own = match sender {
            Sender::App => "a-",
            Sender::Extension => "x-",
        };
        let other = if sender == Sender::App { "x-" } else { "a-" };
        let prefix_ok = |want: &str| self.id.starts_with(want);
        match self.kind {
            EnvelopeType::Req => {
                if !prefix_ok(own) {
                    return Err(ProtocolError::invalid("request id has the wrong prefix"));
                }
                let method = Method::parse(self.method.as_deref().unwrap_or_default())?;
                match sender {
                    Sender::Extension if !method.extension_originated() => {
                        return Err(ProtocolError::new(
                            ErrorCode::Unsupported,
                            "the extension may not request this method",
                        ))
                    }
                    Sender::App if method.extension_originated() => {
                        return Err(ProtocolError::new(
                            ErrorCode::Unsupported,
                            "the app may not request this method",
                        ))
                    }
                    _ => {}
                }
                if sender == Sender::App && method.requires_mono() && self.mono.is_none() {
                    return Err(ProtocolError::invalid("`mono` is required for this method"));
                }
            }
            EnvelopeType::Evt => {
                if sender != Sender::Extension {
                    return Err(ProtocolError::new(
                        ErrorCode::Unsupported,
                        "only the extension sends events",
                    ));
                }
                if !prefix_ok(own) {
                    return Err(ProtocolError::invalid("event id has the wrong prefix"));
                }
                Event::parse(self.method.as_deref().unwrap_or_default())?;
            }
            EnvelopeType::Res => {
                if !prefix_ok(other) {
                    return Err(ProtocolError::invalid("response id has the wrong prefix"));
                }
            }
        }
        Ok(())
    }
}

fn supported_detail() -> Value {
    json!({ "supported": [BROWSER_PROTOCOL.0, BROWSER_PROTOCOL.1] })
}

fn validate_id(id: &str) -> Result<(), ProtocolError> {
    let ok = id.len() >= 3
        && id.len() <= MAX_ID_LEN
        && (id.starts_with("a-") || id.starts_with("x-"))
        && id[2..].bytes().all(|b| b.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(ProtocolError::invalid("`id` must look like a-<n> or x-<n>"))
    }
}

/// 32 lowercase hex characters (a Mono id or an extension instance id).
pub fn is_hex32(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// The highest protocol both sides speak, or `E_PROTOCOL` listing what the app supports (spec 5.3).
pub fn negotiate(ext_range: [u32; 2]) -> Result<u32, ProtocolError> {
    let (lo, hi) = (ext_range[0], ext_range[1]);
    let best = hi.min(BROWSER_PROTOCOL.1);
    if lo > hi || best < lo.max(BROWSER_PROTOCOL.0) {
        return Err(
            ProtocolError::new(ErrorCode::Protocol, "no common protocol version")
                .with_detail(supported_detail()),
        );
    }
    Ok(best)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BrowserInfo {
    pub brand: String,
    pub version: String,
}

/// `hello` params (extension to app).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloParams {
    pub protocol: [u32; 2],
    pub ext_version: String,
    pub browser: BrowserInfo,
    pub instance_id: String,
    pub paired: bool,
    /// 32 random bytes, standard base64.
    pub challenge: String,
}

impl HelloParams {
    pub fn parse(params: &Option<Value>) -> Result<Self, ProtocolError> {
        let hello: HelloParams = parse_params(params)?;
        if !is_hex32(&hello.instance_id) {
            return Err(ProtocolError::invalid("`instance_id` is not 32 hex"));
        }
        if hello.ext_version.is_empty() || hello.ext_version.len() > 32 {
            return Err(ProtocolError::invalid("`ext_version` has a bad length"));
        }
        if hello.browser.brand.len() > 64 || hello.browser.version.len() > 64 {
            return Err(ProtocolError::invalid("`browser` is too long"));
        }
        super::auth::decode_challenge(&hello.challenge)?;
        Ok(hello)
    }
}

/// `hello` result (app to extension). `mac` is absent while a pairing is in progress, because no
/// key exists yet.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HelloResult {
    pub protocol: u32,
    pub app_version: String,
    pub challenge: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthParams {
    pub mac: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChromeGroupColor {
    Grey,
    Blue,
    Red,
    Yellow,
    Green,
    Pink,
    Purple,
    Cyan,
    Orange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccessLevel {
    Read,
    Act,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RosterEntry {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub color: String,
    pub group_color: ChromeGroupColor,
    pub level: AccessLevel,
    pub allowed_origins: Vec<String>,
    pub blocked_origins: Vec<String>,
    pub cursor: bool,
}

pub type Roster = Vec<RosterEntry>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AuthResult {
    pub roster: Roster,
    pub stopped: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairBeginParams {
    /// 128-bit pairing nonce, standard base64.
    pub nonce: String,
    pub instance_id: String,
}

/// A secret string (the pairing key on the wire). Serializes as a plain string; Debug redacts it.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SecretString(String);

impl SecretString {
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for SecretString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PairBeginResult {
    /// The pairing key K (32 bytes, standard base64), sent exactly once.
    pub key: SecretString,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TabInfo {
    pub tab: u64,
    pub window: u64,
    pub url: String,
    pub title: String,
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_title: Option<String>,
    pub active_in_window: bool,
    pub audible: bool,
    pub loading: bool,
}

/// What the Mono asks to do (spec 5.5). Unknown `kind` values fail to parse.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    Click {
        #[serde(rename = "ref")]
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        button: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        double: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        modifiers: Option<Vec<String>>,
    },
    Type {
        #[serde(rename = "ref")]
        target: String,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        submit: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slowly: Option<bool>,
    },
    Fill {
        fields: Vec<FillField>,
    },
    Select {
        #[serde(rename = "ref")]
        target: String,
        values: Vec<String>,
    },
    Press {
        key: String,
        #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    Hover {
        #[serde(rename = "ref")]
        target: String,
    },
    Drag {
        from_ref: String,
        to_ref: String,
    },
    Upload {
        #[serde(rename = "ref")]
        target: String,
        upload_id: String,
        names: Vec<String>,
    },
    Scroll {
        #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
        target: Option<String>,
        dx: i64,
        dy: i64,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FillField {
    #[serde(rename = "ref")]
    pub target: String,
    pub value: String,
    #[serde(rename = "type")]
    pub field_type: String,
}

/// Parses a `params` or `result` object into its typed form. A missing object or a missing
/// required member gives `E_INVALID`.
pub fn parse_params<T: serde::de::DeserializeOwned>(
    params: &Option<Value>,
) -> Result<T, ProtocolError> {
    let value = params
        .clone()
        .ok_or_else(|| ProtocolError::invalid("`params` is required"))?;
    serde_json::from_value(value).map_err(|e| ProtocolError::invalid(format!("bad params: {e}")))
}

/// A `Map` helper for building params in tests and callers.
pub fn object(pairs: impl IntoIterator<Item = (&'static str, Value)>) -> Value {
    let mut map = Map::new();
    for (k, v) in pairs {
        map.insert(k.to_string(), v);
    }
    Value::Object(map)
}
