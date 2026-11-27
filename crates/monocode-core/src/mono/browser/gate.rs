//! The Browser gate: a zptah MCP server that stands between the agent and Ardia. The agent is only
//! given the gate's URL, so every browser tool call passes through [`Policy::evaluate`] before it
//! is forwarded to the [`Upstream`] (the `ardia` command, see [`ardia`]). It works in every
//! permission mode (Auto and Full Access included) because it is zptah code, not a harness ask
//! mode.
//!
//! What is enforced here, and what is not:
//!
//! * Mechanical, whatever the agent does: tools outside the trust level are neither listed nor
//!   run; `browser_route`, `browser_unroute` and `browser_resize` never run. Anything the user
//!   does not answer, or that cannot be asked (no channel, plan or read-only session, timeout),
//!   is denied: the gate fails closed. The Ardia upstream classifies every act tool call itself
//!   and asks through the approval queue.
//! * By the agent's cooperation: the actions the classifier can not recognise from a click.
//!   The gate offers the agent `browser_request_approval` for them and the Mono's rules require
//!   it, but nothing stops an agent that skips it.
//! * One gate serves all sessions of a Mono and they share one browser. Cards, denials and the
//!   remembered page are kept per route (one per app session): a session's cards show only in
//!   that session, and one session ending does not deny another's.
//!
//! An approval is shown through the app's approval card: the gate sends an [`Ask`] to the run
//! that is attached ([`Gate::attach`]), the provider loop turns it into an `Event::Approval` and
//! answers it from the user's decision.
use super::ALWAYS_BLOCKED_TOOLS;
use super::{ardia, defense, output, AuditEntry, BrowserLevel, MonoBrowserStore};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

mod guard;
mod http;
pub mod origins;
pub use origins::Origins;

/// What the wiring tells the Ardia upstream about its Mono. Set before the first run and on every
/// launch (the level and rules are live settings).
#[derive(Clone)]
pub struct ActConfig {
    pub level: BrowserLevel,
    /// The soul rules in force (`soul_rules::reconcile`).
    pub rules: super::soul_rules::Parsed,
    /// Where grants and audit rows live. None (tests, no database): grants stay in memory and no
    /// audit row is written.
    pub store: Option<MonoBrowserStore>,
    /// The Mono's name as the card says it (`Mono::label()`).
    pub mono_label: String,
    /// The Ardia profile's name, for the audit row.
    pub profile: Option<String>,
    /// How long a card waits for the user.
    pub approval_timeout: Duration,
    /// Where the cards of this Mono go.
    pub queue: Arc<super::trust::approval::ApprovalQueue>,
}

/// A conversation (an app session) was closed, cleared or deleted: its grants and open cards end
/// and the Ardia tabs it opened close.
pub fn end_conversation_everywhere(session: i64) {
    ardia::end_conversation_everywhere(session);
}

/// A stand-in route name for code that builds a [`Runtime`](super::Runtime) without a gate.
pub const SHARED_ROUTE: &str = "shared";
/// The tool the gate adds for the actions it can not detect itself.
pub const APPROVAL_TOOL: &str = "browser_request_approval";
/// What a turn is told when its gate was shut down mid-turn.
pub(crate) const STOPPED_GATE: &str = "This browser was turned off or changed while you were working, so it is no longer available in this turn. Tell the user; a new message starts again with the current settings.";
/// How long the gate waits for the user. Codex is told to wait a minute longer.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(10 * 60);

// ---- What needs approval ----

/// What the user is asked to allow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    /// A payment, purchase, subscription, transfer, or card or bank details. Agent-chosen.
    Payment,
    /// Sending a message, email or content to someone. Agent-chosen.
    SendMessage,
    /// Posting, commenting, reviewing or publishing. Agent-chosen.
    PostPublish,
    /// Any other risky action. Agent-chosen.
    Other,
    /// An action that follows untrusted page content and was not declared before it was read.
    /// Chosen by the gate, asked at every trust level.
    Untrusted,
    /// A page held a prompt injection pattern: the user decides whether the Mono sees it.
    Injection,
}
impl Category {
    /// The values `browser_request_approval` accepts.
    pub const AGENT_CHOICES: [&'static str; 4] =
        ["payment", "send_message", "post_publish", "other"];
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "payment" => Self::Payment,
            "send_message" => Self::SendMessage,
            "post_publish" => Self::PostPublish,
            "other" => Self::Other,
            _ => return None,
        })
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Payment => "Money: payment, purchase, subscription or transfer",
            Self::SendMessage => "Sending a message",
            Self::PostPublish => "Posting, commenting or publishing",
            Self::Other => "Risky action",
            Self::Untrusted => "Action after reading untrusted content",
            Self::Injection => "Possible prompt injection in a page",
        }
    }
}

/// One approval the user is asked for. Everything the user needs to decide is in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovalRequest {
    pub category: Category,
    /// What the browser is about to do, in a sentence.
    pub action: String,
    pub url: String,
    pub details: String,
}
impl ApprovalRequest {
    /// The title of the approval card.
    pub fn title(&self) -> String {
        match self.category {
            Category::Payment => "Allow the browser to make a payment?",
            Category::SendMessage => "Allow the browser to send this?",
            Category::PostPublish => "Allow the browser to post this?",
            Category::Other => "Allow this risky browser action?",
            Category::Untrusted => "Allow this action? It follows untrusted web content",
            Category::Injection => "This page looks like a prompt injection. Let the Mono read it?",
        }
        .to_owned()
    }
    /// The body of the approval card: category, action, page and details.
    pub fn detail(&self) -> String {
        let mut out = format!("{}\nAction: {}", self.category.label(), self.action);
        if !self.url.is_empty() {
            out.push_str(&format!("\nPage: {}", self.url));
        }
        if !self.details.is_empty() {
            out.push_str(&format!("\nDetails: {}", self.details));
        }
        out
    }
}

/// What the gate does with one tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allow,
    /// Refused, with the sentence the agent is told.
    Deny(String),
    Ask(ApprovalRequest),
}

/// The rules of one Mono's gate. Plain data: tests build it directly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Policy {
    pub trust: BrowserLevel,
    /// The Mono's blocked origins (a finance page is judged by the upstream's classifier).
    pub blocked_origins: Vec<String>,
    /// When not empty, only these origins may be used (the Mono's allowed list).
    pub allowed_origins: Vec<String>,
    /// `browser_upload` can run: there is a private folder for the copies of uploaded files
    /// ([`ardia::upstream::private_staging_dir`]). Without one the tool is not offered.
    pub uploads: bool,
}
impl Policy {
    /// The rules of a gate in front of Ardia.
    pub fn ardia(trust: BrowserLevel, blocked_origins: Vec<String>) -> Self {
        Self {
            trust,
            blocked_origins,
            allowed_origins: Vec::new(),
            uploads: true,
        }
    }
    /// The same rules for a system with (`true`) or without a private upload folder.
    pub fn with_uploads(mut self, uploads: bool) -> Self {
        self.uploads = uploads;
        self
    }
    /// The same rules with the Mono's allowed origins.
    pub fn with_allowed_origins(mut self, allowed: Vec<String>) -> Self {
        self.allowed_origins = allowed;
        self
    }
    /// The decision for one `tools/call`: the read tools are allowed at every level, the act tools
    /// and the approval tool only at the act level (the upstream then decides what needs the
    /// user's answer, spec 6); addresses are limited to http and https.
    pub fn evaluate(&self, tool: &str, args: &Value) -> Verdict {
        if ALWAYS_BLOCKED_TOOLS.contains(&tool) {
            return Verdict::Deny(format!("{tool} is never available in this browser."));
        }
        let acts = ardia::upstream::ACT_TOOLS.contains(&tool) || tool == APPROVAL_TOOL;
        if !acts && !ardia::upstream::TOOLS.contains(&tool) {
            return Verdict::Deny(format!("{tool} is not an available browser tool."));
        }
        if tool == "browser_upload" && !self.uploads {
            return Verdict::Deny("browser_upload is not available on this system: there is no private folder for the copies of uploaded files.".into());
        }
        let adopts = tool == "browser_tabs" && args["action"] == "adopt_active";
        if (acts || adopts) && self.trust == BrowserLevel::ReadOnly {
            return Verdict::Deny(format!(
                "{tool} is not available: this Mono can only read the browser. Tell the user what to do, or that the browser level must be raised."
            ));
        }
        match unsafe_address(tool, args) {
            Some(reason) => Verdict::Deny(reason),
            None => Verdict::Allow,
        }
    }

    /// Whether the gate lists `tool` to the agent.
    pub fn offers(&self, tool: &str) -> bool {
        ardia::upstream::TOOLS.contains(&tool)
            || (self.trust != BrowserLevel::ReadOnly
                && ardia::upstream::ACT_TOOLS.contains(&tool)
                && (self.uploads || tool != "browser_upload"))
    }
}

/// `browser_navigate` and a new tab open http and https addresses only, at every level:
/// `javascript:`, `data:`, `file:` and the rest are refused.
fn unsafe_address(tool: &str, args: &Value) -> Option<String> {
    let target = match tool {
        "browser_navigate" | "browser_open" => args["url"].as_str().unwrap_or(""),
        "browser_tabs" if args["action"] == "new" => args["url"].as_str().unwrap_or(""),
        _ => return None,
    };
    if target.is_empty() && tool == "browser_tabs" {
        return None;
    }
    let ok = url::Url::parse(target.trim())
        .is_ok_and(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some());
    (!ok).then(|| {
        "Only http and https addresses can be opened (not javascript:, data:, file: or others)."
            .to_owned()
    })
}

/// The address a call is about to open, if it opens one.
fn opened_address(tool: &str, args: &Value) -> Option<String> {
    let target = match tool {
        "browser_navigate" | "browser_open" => args["url"].as_str(),
        "browser_tabs" if args["action"] == "new" => args["url"].as_str(),
        _ => None,
    }?;
    Some(target.trim().to_owned()).filter(|t| !t.is_empty())
}

/// Words in a host that mark a bank, wallet or payment site. A match only asks for approval, so
/// a false positive costs one card.
const FINANCE_HOST_PARTS: &[&str] = &[
    "paypal",
    "stripe",
    "venmo",
    "revolut",
    "wise.com",
    "payoneer",
    "skrill",
    "klarna",
    "affirm",
    "coinbase",
    "binance",
    "kraken",
    "bank",
    "wallet",
    "momo.vn",
    "zalopay",
    "vnpay",
    "shopeepay",
    "vietcombank",
    "vietinbank",
    "techcombank",
    "bidv",
    "mbbank",
    "vpbank",
    "tpbank",
    "sacombank",
    "agribank",
    "chase.com",
    "wellsfargo",
    "citi.com",
    "americanexpress",
    "capitalone",
    "schwab",
    "robinhood",
    "cash.app",
    "pay.",
];
const FINANCE_PATH_PARTS: &[&str] = &[
    "checkout",
    "payment",
    "payments",
    "pay",
    "billing",
    "subscribe",
    "wallet",
    "transfer",
];

/// Whether a page is a bank, wallet or payment page, or one of the Mono's blocked origins.
pub fn is_finance_url(url: &str, blocked_origins: &[String]) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let origin = parsed.origin().ascii_serialization();
    if blocked_origins.iter().any(|o| o == &origin) {
        return true;
    }
    let host = parsed.host_str().unwrap_or("").to_lowercase();
    if FINANCE_HOST_PARTS.iter().any(|part| host.contains(part)) {
        return true;
    }
    parsed
        .path_segments()
        .into_iter()
        .flatten()
        .any(|segment| FINANCE_PATH_PARTS.contains(&segment.to_lowercase().as_str()))
}

/// Whether any text in the arguments is a payment card number: 13 to 19 digits (spaces and
/// dashes allowed) that pass the Luhn check.
pub fn contains_card_number(args: &Value) -> bool {
    fn strings<'a>(value: &'a Value, out: &mut Vec<&'a str>) {
        match value {
            Value::String(text) => out.push(text),
            Value::Array(items) => items.iter().for_each(|v| strings(v, out)),
            Value::Object(map) => map.values().for_each(|v| strings(v, out)),
            _ => {}
        }
    }
    let mut texts = Vec::new();
    strings(args, &mut texts);
    texts.into_iter().any(digits_hold_card)
}

/// The value of a decimal digit written in ASCII, full width or another common script.
fn digit_value(c: char) -> Option<char> {
    let base = match c as u32 {
        0x30..=0x39 => 0x30,
        0xFF10..=0xFF19 => 0xFF10,
        0x0660..=0x0669 => 0x0660,
        0x06F0..=0x06F9 => 0x06F0,
        0x0966..=0x096F => 0x0966,
        _ => return None,
    };
    char::from_digit(c as u32 - base, 10)
}

/// Whether a text has a run of 13 to 19 digits that passes Luhn. Digits are normalized (full
/// width and other scripts count), and any character that is not a letter or a digit (spaces of
/// every kind, dots, slashes, dashes) is a separator inside a number.
pub fn digits_hold_card(text: &str) -> bool {
    let mut run = String::new();
    let mut found = false;
    for c in text.chars() {
        if let Some(digit) = digit_value(c) {
            run.push(digit);
        } else if !c.is_alphanumeric() && !run.is_empty() {
            // A separator inside a number does not end the run.
        } else {
            found |= window_is_card(&run);
            run.clear();
        }
    }
    found | window_is_card(&run)
}

/// Whether any stretch of 13 to 19 digits inside a run is a card number: it passes Luhn and has
/// the length and the leading digits of a real card brand. A card number can sit inside a longer
/// run of digits (an order number glued to it), so every stretch is looked at; the brand check
/// keeps the order and tracking numbers that pass Luhn by chance (about one in ten) from looking
/// like cards.
fn window_is_card(run: &str) -> bool {
    (13..=19).any(|len| {
        run.len() >= len
            && (0..=run.len() - len).any(|start| {
                let digits = &run[start..start + len];
                luhn(digits) && card_shape(digits)
            })
    })
}

/// Whether 13 to 19 digits have the length and the leading digits (issuer range) of a payment
/// card brand: Visa, Mastercard (51-55 and 2221-2720), Maestro, American Express, Discover,
/// Diners Club, JCB, UnionPay, Mir, RuPay, Verve, Troy, UATP, Napas, and the local brands that
/// start with 5, 6, 8 or 9. Digits that no brand issues (a leading 0 or 7, a Visa of 14 digits,
/// an American Express of 16) are not a card. The ranges are deliberately generous where brands
/// overlap (every number that starts with 6 and is 13 to 19 digits long counts), because missing
/// a real card is worse than a false alarm.
fn card_shape(digits: &str) -> bool {
    let len = digits.len();
    let prefix = |n: usize| digits[..n].parse::<u32>().unwrap_or(0);
    match digits.as_bytes()[0] {
        // Visa (13 to 19 digits: 13, 16 and 19 are issued, the rest are Visa-based local brands).
        b'4' => true,
        // Mastercard 51-55 (16), Maestro 50 and 56-59 (12-19), Dankort 5019, Verve 5061.
        b'5' => len == 16 || (matches!(prefix(2), 50 | 56..=59) && len >= 12),
        // Every brand that starts with 6 (Discover, UnionPay, Maestro, RuPay, Elo, Hipercard,
        // Troy 65): 13 to 19 digits.
        b'6' => true,
        // American Express 34 and 37 (15); Diners Club 30, 36, 38, 39 (14, 16-19); JCB 3528-3589
        // (16-19); JCB 15-digit (3088, 3096, 3112, 3158, 3337); InstaPayment 637-639 is a 6.
        b'3' => match prefix(2) {
            34 | 37 => len == 15,
            30 | 36 | 38 | 39 => len == 14 || len >= 16,
            35 => len >= 16,
            31 | 33 => len == 15 || len >= 16,
            _ => false,
        },
        // Mastercard 2221-2720 and Mir 2200-2204 (16-19).
        b'2' => {
            // JCB 2131 (15 digits) too.
            ((2221..=2720).contains(&prefix(4)) && len == 16)
                || (prefix(4) == 2131 && len == 15)
                || ((2200..=2204).contains(&prefix(4)) && len >= 16)
        }
        // UATP (15) and the old JCB 1800 (15).
        b'1' => len == 15,
        // RuPay 81 and 82, Uzcard 8600 (16).
        b'8' => matches!(prefix(2), 81 | 82 | 86) && len == 16,
        // Napas 9704 (16-19), Troy 9792, Humo 9860, BC Card 94 (16).
        b'9' => {
            (prefix(4) == 9704 && len >= 16)
                || (matches!(prefix(4), 9792 | 9860) || prefix(2) == 94) && len == 16
        }
        _ => false,
    }
}
fn luhn(digits: &str) -> bool {
    let sum: u32 = digits
        .bytes()
        .rev()
        .enumerate()
        .map(|(i, b)| {
            let d = u32::from(b - b'0');
            if i % 2 == 1 {
                let d = d * 2;
                if d > 9 {
                    d - 9
                } else {
                    d
                }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

// ---- Asking the user ----

/// One approval request on its way to the run that shows the card.
pub struct Ask {
    pub request: ApprovalRequest,
    /// `true` is Allow. A dropped sender is a deny.
    pub reply: mpsc::Sender<bool>,
    /// Set by the gate when it stopped waiting (timeout): the card should be withdrawn.
    pub abandoned: Arc<AtomicBool>,
}

/// A run's end of the approval channel. Dropping it detaches the run from the gate.
pub struct Attached {
    pub receiver: mpsc::Receiver<Ask>,
    gate: Arc<Inner>,
    route: String,
    generation: u64,
}
impl Attached {
    pub fn try_recv(&self) -> Result<Ask, mpsc::TryRecvError> {
        self.receiver.try_recv()
    }
}
impl Drop for Attached {
    fn drop(&mut self) {
        let mut map = self
            .gate
            .approvers
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mine = map
            .get(&self.route)
            .is_some_and(|a| a.generation == self.generation);
        if mine {
            map.remove(&self.route);
        }
        drop(map);
        if mine {
            self.gate.run_ended(&self.route);
        }
    }
}
/// The route's end of the approval channel. Nothing on an Ardia gate asks through the channel
/// (the Ardia upstream shows its cards through the approval queue); it stays for [`Gate::attach`].
struct Approver {
    #[cfg_attr(not(test), allow(dead_code))]
    sender: mpsc::Sender<Ask>,
    generation: u64,
}

// ---- The upstream server ----

/// Posts one JSON-RPC message to the browser backend (the Ardia upstream, or a fake in tests).
pub trait Upstream: Send + Sync {
    /// Returns the reply (for a request) and the `Mcp-Session-Id` the server named, if any.
    /// A server that refused the request answers `Err("HTTP <status>")`.
    fn post(
        &self,
        url: &str,
        session: Option<&str>,
        message: &Value,
    ) -> Result<(Option<Value>, Option<String>), String>;
    /// Whether the MCP session is still the one to use after `error`. A backend that keeps
    /// one session for its life says so here; one that drops its session on an error does not, and
    /// the gate starts a new one.
    fn session_survives(&self, _error: &str) -> bool {
        false
    }
    /// A run of app session `session` starts with the user's own message (never page content).
    /// Only the Ardia upstream uses it: task domains and named recipients come from it.
    fn run_started(&self, _session: i64, _user_message: &str) {}
    /// [`Upstream::run_started`] that also says whether `user_message` was typed by the user.
    /// An automation's stored prompt is the user's own text (it counts as the words of the task)
    /// but was not typed in this turn, so it does not lift a Stop (design spec section 9). The
    /// default ignores the difference.
    fn run_started_by(&self, session: i64, user_message: &str, _typed_by_user: bool) {
        self.run_started(session, user_message);
    }
    /// The run of app session `session` ended (its approval channel was dropped).
    fn run_ended(&self, _session: i64) {}
    /// A page read in app session `session` carried an injection pattern.
    fn injection_found(&self, _session: i64) {}
    /// The Mono declared an intent (`browser_declare_intent`) in app session `session`; `approved`
    /// is false for one declared after a read that the user did not approve.
    fn intent_declared(&self, _session: i64, _intent: &defense::Intent, _approved: bool) {}
    /// The gate's policy was set or changed: its trust level and its blocked and allowed origins
    /// are the ones enforced now. An upstream that judges actions itself follows them on its next
    /// call, and an open card checks the level again after the user answered.
    fn policy_changed(&self, _policy: &Policy) {}
}

thread_local! {
    static RUN: std::cell::RefCell<Option<RunContext>> = const { std::cell::RefCell::new(None) };
}

/// Which app session the tool call running on this thread belongs to. The gate sets it for the
/// length of a call, so an upstream that needs the conversation (grants, task domains) can read
/// it without every `post` carrying it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunContext {
    pub route: String,
    /// The app session id (0 when the route has none).
    pub session_id: i64,
}

impl RunContext {
    /// The context of the tool call on this thread, if the gate set one.
    pub fn current() -> Option<RunContext> {
        RUN.with(|slot| slot.borrow().clone())
    }
}

struct RunScope(Option<RunContext>);
impl RunScope {
    fn enter(context: RunContext) -> Self {
        Self(RUN.with(|slot| slot.borrow_mut().replace(context)))
    }
}
impl Drop for RunScope {
    fn drop(&mut self) {
        RUN.with(|slot| *slot.borrow_mut() = self.0.take());
    }
}

#[derive(Clone)]
struct Session {
    /// Identifies the server instance the session was opened on.
    key: String,
    id: Option<String>,
}
struct State {
    policy: Policy,
    upstream: Arc<dyn Upstream>,
    /// Names the running backend (a new one gets a new key).
    upstream_key: String,
    session: Option<Session>,
    /// The page the browser is on, as the last tool result or opened address said. The browser
    /// (and its current tab) is shared by every session of the Mono, so the checks read this one
    /// value whichever session writes.
    page: String,
    /// Issued routes: an unguessable token per app session (`None`: no session).
    routes: std::collections::HashMap<Option<i64>, String>,
}

/// One gate serves every session of a Mono. The backend side is one shared session (and one
/// browser connection) per Mono. Approval cards and denials are kept per route (one per app
/// session), so a card of one session never shows in another.
struct Inner {
    state: Mutex<State>,
    init: Mutex<()>,
    approvers: Mutex<std::collections::HashMap<String, Approver>>,
    /// Held by an agent call while it is forwarded, so calls are never interleaved in the same tab.
    run: Mutex<()>,
    /// Where this Mono keeps its browser files; none until the app sets it.
    workspace: Mutex<Option<Workspace>>,
    /// What each route (app session) read and declared this turn (see [`defense`]).
    turns: Mutex<std::collections::HashMap<String, defense::Turn>>,
    generation: AtomicU64,
    next_id: AtomicU64,
    /// The Mono's browser was turned off or changed ([`Gate::shut_down`]): the turn that still
    /// holds this gate's address gets a refusal for every further call.
    stopped: AtomicBool,
    /// How long [`Inner::ask`] waits for the user.
    #[cfg_attr(not(test), allow(dead_code))]
    timeout: Duration,
}

/// What the gate needs to confine the files the browser writes and to audit. Without it (some
/// tests) the gate leaves `filename` alone.
#[derive(Clone, Debug)]
pub struct Workspace {
    pub mono_id: String,
    /// The browser's output folder: private, swept, the only place `filename` may point to.
    pub output_dir: PathBuf,
    /// The store database (approved hashes, audit log).
    pub database: PathBuf,
}

impl Workspace {
    /// Removes old files from the output folder, keeping the screenshots that audit rows still
    /// point to (spec 8.5: they live as long as their row).
    pub fn sweep(&self, keep_audit_screenshots: bool) -> usize {
        if !keep_audit_screenshots {
            return output::sweep(&self.output_dir);
        }
        let keep = MonoBrowserStore::at(self.database.clone()).audit_screenshots(&self.mono_id);
        output::sweep_keeping(&self.output_dir, &keep)
    }
}

/// One Mono's gate: its loopback server and its policy.
pub struct Gate {
    inner: Arc<Inner>,
    port: u16,
    secret: String,
    _server: http::HttpServer,
}
impl std::fmt::Debug for Gate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gate(port {})", self.port)
    }
}

impl Gate {
    /// Starts the gate's server in front of the backend named `upstream_key`.
    pub fn start(
        upstream: Arc<dyn Upstream>,
        policy: Policy,
        upstream_key: &str,
    ) -> Result<Arc<Self>, String> {
        Self::start_with_timeout(upstream, policy, upstream_key, APPROVAL_TIMEOUT)
    }
    pub fn start_with_timeout(
        upstream: Arc<dyn Upstream>,
        policy: Policy,
        upstream_key: &str,
        timeout: Duration,
    ) -> Result<Arc<Self>, String> {
        let inner = Arc::new(Inner {
            turns: Mutex::new(Default::default()),
            state: Mutex::new(State {
                policy,
                upstream,
                upstream_key: upstream_key.to_owned(),
                session: None,
                page: String::new(),
                routes: Default::default(),
            }),
            init: Mutex::new(()),
            approvers: Mutex::new(Default::default()),
            run: Mutex::new(()),
            workspace: Mutex::new(None),
            generation: AtomicU64::new(0),
            next_id: AtomicU64::new(100),
            stopped: AtomicBool::new(false),
            timeout,
        });
        let secret = format!("{:032x}", rand::random::<u128>());
        let server = http::HttpServer::start(inner.clone(), secret.clone())?;
        Ok(Arc::new(Self {
            port: server.port,
            inner,
            secret,
            _server: server,
        }))
    }

    /// The URL a session connects to: the path holds the session's route only. The gate's secret
    /// travels as `Authorization: Bearer` (see [`Self::token`]), never in a URL, so it stays off
    /// argv. Cards, denials and the remembered page are kept per route.
    pub fn url_for(&self, route: &str) -> String {
        format!("http://127.0.0.1:{}/{route}/mcp", self.port)
    }
    /// The bearer token every request to the gate must carry.
    pub fn token(&self) -> &str {
        &self.secret
    }
    /// The route token of an app session (`None`: a caller without one). It is random, issued on
    /// first use and stable for the gate's life, so a warm agent process keeps a working URL; the
    /// server answers only routes it issued.
    pub fn route_for(&self, session: Option<i64>) -> String {
        self.inner
            .lock_state()
            .routes
            .entry(session)
            .or_insert_with(|| format!("{:032x}", rand::random::<u128>()))
            .clone()
    }
    /// The URL of the route used by callers that have no session.
    pub fn url(&self) -> String {
        self.url_for(&self.route_for(None))
    }

    /// Tells the gate where this Mono keeps its browser files and confines `filename` arguments to
    /// the output folder.
    pub fn set_workspace(&self, workspace: Workspace) {
        let _ = output::prepare(&workspace.output_dir);
        self.inner.sweep_output(&workspace);
        *self
            .inner
            .workspace
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(workspace);
    }

    /// Removes the browser files of this Mono (its gate stopped): they are page content.
    pub fn clear_output(&self) {
        if let Some(workspace) = self.inner.workspace() {
            output::clear(&workspace.output_dir);
        }
    }

    /// The Mono's browser was turned off or changed: this gate refuses every further call, so a
    /// turn that still holds its address can not keep using the browser. The server keeps
    /// answering (with the refusal) until the last holder lets go of the gate.
    pub fn shut_down(&self) {
        self.inner.stopped.store(true, Ordering::SeqCst);
    }

    /// Whether [`Gate::shut_down`] was called.
    pub fn is_shut_down(&self) -> bool {
        self.inner.stopped.load(Ordering::SeqCst)
    }

    /// Applies a new policy to the calls that follow, keeping the server.
    pub fn set_policy(&self, policy: Policy) {
        let upstream = {
            let mut state = self.inner.lock_state();
            state.policy = policy.clone();
            state.upstream.clone()
        };
        // An upstream that judges actions itself follows the level and the origin lists at once,
        // also while a card is open.
        upstream.policy_changed(&policy);
    }

    /// Applies the Mono's current trust level and blocked origins, and which backend is behind the
    /// gate (a new one gives a new key). Takes effect on the next call.
    pub fn configure(&self, policy: Policy, upstream_key: &str, upstream: Arc<dyn Upstream>) {
        let current = {
            let mut state = self.inner.lock_state();
            state.policy = policy.clone();
            if state.upstream_key != upstream_key {
                state.upstream_key = upstream_key.to_owned();
                state.upstream = upstream;
                state.session = None;
                state.page.clear();
            }
            state.upstream.clone()
        };
        current.policy_changed(&policy);
    }

    /// Routes the approval requests of `route` to the calling run until the result is dropped.
    /// A later attach to the same route replaces this one; other routes are untouched.
    pub fn attach(&self, route: &str) -> Attached {
        let (sender, receiver) = mpsc::channel();
        let generation = self.inner.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner
            .approvers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(route.to_owned(), Approver { sender, generation });
        Attached {
            receiver,
            gate: Arc::clone(&self.inner),
            route: route.to_owned(),
            generation,
        }
    }

    /// A new agent turn starts on `route` with the user's own request text: what was read,
    /// declared and suggested in the turn before is forgotten (see [`defense`]).
    pub fn begin_turn(&self, route: &str, request: &str) {
        self.inner.begin_turn(route, request, true);
    }

    /// [`Gate::begin_turn`] that says whether `request` was typed by the user in this turn (false
    /// for an automation's stored prompt: its words count for the task but do not lift a Stop).
    pub fn begin_turn_as(&self, route: &str, request: &str, typed_by_user: bool) {
        self.inner.begin_turn(route, request, typed_by_user);
    }

    /// Tests of other features: the route's turn is never judged by the taint rule.
    #[cfg(test)]
    pub fn exempt_from_taint(&self, route: &str) {
        self.inner.exempt_from_taint(route);
    }

    /// Answers one JSON-RPC message from the agent on `route` (what the HTTP server calls).
    pub fn handle(&self, route: &str, message: Value) -> Option<Value> {
        self.inner.respond(route, message)
    }
}

impl http::Handler for Inner {
    fn handle(&self, route: &str, message: Value) -> Option<Value> {
        self.respond(route, message)
    }
    fn knows(&self, route: &str) -> bool {
        self.lock_state().routes.values().any(|r| r == route)
    }
}

fn reply(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}
fn failure(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}
fn tool_text(text: impl Into<String>, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text.into()}], "isError": is_error})
}

impl Inner {
    fn lock_state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn respond(&self, route: &str, message: Value) -> Option<Value> {
        let id = message.get("id").filter(|id| !id.is_null())?.clone();
        let params = &message["params"];
        Some(match message["method"].as_str().unwrap_or("") {
            "initialize" => reply(
                &id,
                json!({
                    "protocolVersion": params["protocolVersion"].as_str().unwrap_or("2025-03-26"),
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "monoptah-browser-gate", "version": env!("CARGO_PKG_VERSION")},
                }),
            ),
            "ping" => reply(&id, json!({})),
            "tools/list" => match self.list_tools() {
                Ok(tools) => reply(&id, json!({"tools": tools})),
                Err(error) => failure(&id, -32603, &error),
            },
            "tools/call" => reply(
                &id,
                self.call_tool(
                    route,
                    params["name"].as_str().unwrap_or(""),
                    params
                        .get("arguments")
                        .cloned()
                        .unwrap_or_else(|| json!({})),
                ),
            ),
            _ => failure(&id, -32601, "Method not found"),
        })
    }

    /// The upstream tools this level offers, plus the approval tool and the declared intent where
    /// there is something to approve.
    fn list_tools(&self) -> Result<Vec<Value>, String> {
        let policy = self.lock_state().policy.clone();
        let upstream = self.upstream_rpc("tools/list", json!({}))?;
        let mut tools: Vec<Value> = upstream["tools"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|tool| tool["name"].as_str().is_some_and(|n| policy.offers(n)))
            .cloned()
            .collect();
        if policy.trust != BrowserLevel::ReadOnly {
            tools.push(approval_tool_definition());
            tools.push(defense::declare_tool_definition());
        }
        Ok(tools)
    }

    fn workspace(&self) -> Option<Workspace> {
        self.workspace
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The policy's decision for one call. `Err` is the reply to give the caller instead of
    /// running the call. The Ardia upstream decides what needs the user's answer itself and asks
    /// through the approval queue, so the gate only denies here.
    fn authorize(&self, name: &str, args: &Value) -> Result<(), Value> {
        let policy = self.lock_state().policy.clone();
        match policy.evaluate(name, args) {
            Verdict::Deny(reason) => return Err(tool_text(reason, true)),
            // The policy never asks any more; a verdict that would is a refusal, not a prompt.
            Verdict::Ask(_) => {
                return Err(tool_text(
                    "This action needs an approval the gate can not ask for.",
                    true,
                ))
            }
            Verdict::Allow => {}
        }
        Ok(())
    }

    /// Sweeps the output folder; the pictures of audit rows stay.
    fn sweep_output(&self, workspace: &Workspace) -> usize {
        workspace.sweep(true)
    }

    /// The app session id a route was issued for (0: none).
    fn session_of(&self, route: &str) -> i64 {
        self.lock_state()
            .routes
            .iter()
            .find(|(_, r)| r.as_str() == route)
            .and_then(|(session, _)| *session)
            .unwrap_or(0)
    }

    /// The run on `route` ended: the upstream may start its end-of-task clocks.
    fn run_ended(&self, route: &str) {
        let (session, upstream) = {
            let state = self.lock_state();
            (
                state
                    .routes
                    .iter()
                    .find(|(_, r)| r.as_str() == route)
                    .and_then(|(session, _)| *session)
                    .unwrap_or(0),
                state.upstream.clone(),
            )
        };
        upstream.run_ended(session);
    }

    fn call_tool(&self, route: &str, name: &str, args: Value) -> Value {
        let _scope = RunScope::enter(RunContext {
            route: route.to_owned(),
            session_id: self.session_of(route),
        });
        if self.stopped.load(Ordering::SeqCst) {
            return tool_text(STOPPED_GATE, true);
        }
        if name == defense::DECLARE_TOOL {
            return self.declare_intent(route, &args);
        }
        self.call_ardia_tool(route, name, args)
    }

    /// A call on an Ardia gate: authorized by the policy, run by the upstream (which keeps its own
    /// tabs and checks origins), then fenced, scanned and tainted like any page read.
    fn call_ardia_tool(&self, route: &str, name: &str, args: Value) -> Value {
        if let Err(reply) = self.authorize(name, &args) {
            return reply;
        }
        let result = {
            let _one_at_a_time = self.run.lock().unwrap_or_else(|e| e.into_inner());
            self.forward(name, &args)
        };
        self.defend(route, name, result)
    }

    /// One call to the backend, after the policy allowed it. Remembers the page.
    fn forward(&self, name: &str, forwarded: &Value) -> Value {
        // Opening a page makes it this session's page at once, so the next write is checked
        // against it even when the result does not repeat the address.
        if let Some(target) = opened_address(name, forwarded) {
            self.lock_state().page = target;
        }
        match self.upstream_rpc("tools/call", json!({"name": name, "arguments": forwarded})) {
            Ok(result) => {
                if let Some(url) = page_url_of(&result) {
                    self.lock_state().page = url;
                }
                result
            }
            Err(error) => tool_text(format!("{} {error}", guard::UNAVAILABLE_PREFIX), true),
        }
    }

    fn audit(&self, route: &str, workspace: &Workspace, tool: &str, url: &str, summary: &str) {
        let session_id = self
            .lock_state()
            .routes
            .iter()
            .find(|(_, r)| r.as_str() == route)
            .and_then(|(session, _)| *session)
            .unwrap_or(0);
        let entry = AuditEntry {
            ts_ms: crate::time::now_ms(),
            session_id,
            tool: tool.to_owned(),
            url: audit_url(url),
            summary: summary.chars().take(200).collect(),
        };
        let _ = MonoBrowserStore::at(workspace.database.clone()).record(&workspace.mono_id, &entry);
    }

    /// Shows the card and waits. Everything but an explicit Allow is an error text for the agent.
    #[cfg_attr(not(test), allow(dead_code))]
    fn ask(&self, route: &str, request: ApprovalRequest) -> Result<(), String> {
        let sender = self
            .approvers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(route)
            .map(|a| a.sender.clone());
        let Some(sender) = sender else {
            return Err(guard::NO_CHANNEL.into());
        };
        let (reply, answer) = mpsc::channel();
        let abandoned = Arc::new(AtomicBool::new(false));
        let ask = Ask {
            request,
            reply,
            abandoned: abandoned.clone(),
        };
        if sender.send(ask).is_err() {
            return Err(guard::NO_CHANNEL.into());
        }
        match answer.recv_timeout(self.timeout) {
            Ok(true) => Ok(()),
            Ok(false) => Err(
                "Denied by the user. Do not do this action, and do not try another way to do it."
                    .into(),
            ),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                abandoned.store(true, Ordering::SeqCst);
                Err("Denied: the user did not answer in time.".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err("Denied: the request was not answered (the run ended or was stopped).".into())
            }
        }
    }

    /// One request to the backend, starting a session first when there is none. A
    /// refused request (HTTP 400 or 404: a stale session) is repeated once on a new session;
    /// any other failure is not, because the call may have run.
    fn upstream_rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        let mut last = String::new();
        for _ in 0..2 {
            let session = self.ensure_session()?;
            let message = json!({
                "jsonrpc": "2.0",
                "id": self.next_id.fetch_add(1, Ordering::SeqCst),
                "method": method,
                "params": params,
            });
            let upstream = self.lock_state().upstream.clone();
            match upstream.post(&session.key, session.id.as_deref(), &message) {
                Ok((Some(reply), _)) => {
                    return match reply.get("error") {
                        Some(error) => Err(error["message"]
                            .as_str()
                            .unwrap_or("The browser backend refused the request.")
                            .to_owned()),
                        None => Ok(reply["result"].clone()),
                    }
                }
                Ok((None, _)) => return Err("The browser server sent no reply.".into()),
                Err(error) => {
                    // A server that keeps its one MCP session for its whole life (stdio) must not
                    // be asked to initialize again: it would refuse, for ever.
                    if !upstream.session_survives(&error) {
                        self.lock_state().session = None;
                    }
                    let stale = error == "HTTP 400" || error == "HTTP 404";
                    last = error;
                    if !stale {
                        break;
                    }
                }
            }
        }
        Err(last)
    }

    fn ensure_session(&self) -> Result<Session, String> {
        let _one_at_a_time = self.init.lock().unwrap_or_else(|e| e.into_inner());
        let (key, upstream) = {
            let state = self.lock_state();
            if let Some(session) = state
                .session
                .as_ref()
                .filter(|s| s.key == state.upstream_key)
            {
                return Ok(session.clone());
            }
            (state.upstream_key.clone(), state.upstream.clone())
        };
        let hello = json!({
            "jsonrpc": "2.0",
            "id": self.next_id.fetch_add(1, Ordering::SeqCst),
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {"name": "monoptah-gate", "version": env!("CARGO_PKG_VERSION")},
            },
        });
        let (reply, id) = upstream.post(&key, None, &hello)?;
        if reply.as_ref().is_none_or(|r| r.get("error").is_some()) {
            return Err("The browser server refused to start a session.".into());
        }
        upstream.post(
            &key,
            id.as_deref(),
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        )?;
        let session = Session { key, id };
        self.lock_state().session = Some(session.clone());
        Ok(session)
    }
}

/// The page a tool result reports: a `- Page URL: ...` line, or for a tab list the address of
/// the tab marked `(current)`, so a tab switch or a new tab moves the remembered page too.
fn page_url_of(result: &Value) -> Option<String> {
    let lines: Vec<&str> = result["content"]
        .as_array()?
        .iter()
        .filter_map(|part| part["text"].as_str())
        .flat_map(str::lines)
        .collect();
    let direct = lines.iter().find_map(|line| {
        line.trim()
            .trim_start_matches("- ")
            .strip_prefix("Page URL:")
            .map(|url| url.trim().to_owned())
    });
    direct
        .or_else(|| {
            lines
                .iter()
                .filter(|line| line.contains("(current)"))
                .find_map(|line| {
                    let tail = line.rsplit_once("](")?.1;
                    Some(tail.split(')').next()?.trim().to_owned())
                })
        })
        .filter(|url| !url.is_empty())
}

fn approval_tool_definition() -> Value {
    json!({
        "name": APPROVAL_TOOL,
        "title": "Ask the user for approval",
        "description": "Ask the user for approval BEFORE a risky action, and wait for the answer. Call it before: a payment, purchase, subscription, transfer, or entering card or bank details (payment); sending a message, email or content to someone (send_message); posting, commenting, reviewing or publishing (post_publish); anything else irreversible or outward-facing (other). Do the action only if the result says Approved. If it says Denied, do not do the action and do not try another way.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "category": {"type": "string", "enum": Category::AGENT_CHOICES, "description": "payment, send_message, post_publish or other"},
                "action": {"type": "string", "description": "What you are about to do, in one sentence."},
                "url": {"type": "string", "description": "The page the action happens on."},
                "details": {"type": "string", "description": "Exactly what will be sent, posted or paid: the text, the recipient, the amount."},
            },
            "required": ["category", "action"],
        },
    })
}

/// What the audit log keeps of a page address: origin and path, never the query or fragment
/// (they can carry tokens). An address that does not parse is not logged.
pub fn audit_url(url: &str) -> String {
    url::Url::parse(url)
        .map(|u| format!("{}{}", u.origin().ascii_serialization(), u.path()))
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect()
}

/// The text parts of a tool result, joined.
fn result_text(result: &Value) -> String {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|part| part["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests;
