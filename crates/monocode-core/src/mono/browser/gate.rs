//! The Browser gate: a zptah MCP server that stands between the agent and the zptah Browser
//! extension. The agent is only given the gate's URL, so every browser tool call passes through
//! [`Policy::evaluate`] before it is forwarded to the [`Upstream`] (the extension link, see
//! [`extension`]). It works in every permission mode (Auto and Full Access included) because it
//! is zptah code, not a harness ask mode.
//!
//! What is enforced here, and what is not:
//!
//! * Mechanical, whatever the agent does: tools outside the trust level are neither listed nor
//!   run; `browser_route`, `browser_unroute` and `browser_resize` never run. Anything the user
//!   does not answer, or that cannot be asked (no channel, plan or read-only session, timeout),
//!   is denied: the gate fails closed. The extension upstream classifies every act tool call
//!   itself (`trust::TrustState::evaluate`) and asks through the approval queue.
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
use super::recipe::{self, record::Trace, runner};
use super::{defense, output, AuditEntry, BrowserLevel, MonoBrowserStore};
use super::{is_scroll_key, ALWAYS_BLOCKED_TOOLS, KNOWN_TOOLS, READ_ONLY_TOOLS, SCROLL_KEYS};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

pub(crate) mod extension;
mod guard;
mod http;
mod refs;
pub mod tabs;
pub use extension::act::{end_conversation_everywhere, upstream_for, ActConfig, ExtensionStatus};
pub use extension::{ExtensionUpstream, Origins};

/// A stand-in route name for code that builds a [`Runtime`](super::Runtime) without a gate.
pub const SHARED_ROUTE: &str = "shared";
/// The prefix of the id of the card that offers to save a task as a recipe. Its three answers
/// are Dismiss (deny), Save as recipe (allow once) and Never for this site (the session answer,
/// desktop only).
pub const SUGGEST_ID_PREFIX: &str = "browser-gate-suggest-";
/// The tool the gate adds for the actions it can not detect itself.
pub const APPROVAL_TOOL: &str = "browser_request_approval";
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
    /// Saving a browser recipe the Mono recorded. Chosen by the gate.
    Recipe,
    /// An action that follows untrusted page content and was not declared before it was read.
    /// Chosen by the gate, asked at every trust level.
    Untrusted,
    /// A page held a prompt injection pattern: the user decides whether the Mono sees it.
    Injection,
    /// "Save as recipe?" for a task the app found repeatable. Chosen by the app.
    Suggestion,
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
            Self::Recipe => "Save a browser recipe",
            Self::Untrusted => "Action after reading untrusted content",
            Self::Injection => "Possible prompt injection in a page",
            Self::Suggestion => "Save this task as a browser recipe",
        }
    }
}

/// Whether an action of `category` needs the user's approval at `level` (used by recipes).
/// `read-only` can not act at all (the policy denies the tool, so there is nothing to approve);
/// `act` asks for every category the agent or a recipe names. Untrusted content and injection
/// warnings ask at every level; a suggestion never blocks, and a recipe that saves itself never
/// asks through this function.
pub fn approval_required(level: BrowserLevel, category: Category) -> bool {
    if matches!(category, Category::Untrusted | Category::Injection) {
        return true;
    }
    if category == Category::Suggestion {
        return false;
    }
    match level {
        BrowserLevel::ReadOnly => false,
        BrowserLevel::Act => category != Category::Recipe,
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
            Category::Recipe => "Save this browser recipe?",
            Category::Untrusted => "Allow this action? It follows untrusted web content",
            Category::Injection => "This page looks like a prompt injection. Let the Mono read it?",
            Category::Suggestion => "Save this task as a recipe?",
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
    /// The Mono's blocked origins (recipes ask before a step on one, as on a finance page).
    pub blocked_origins: Vec<String>,
}
impl Policy {
    pub fn new(trust: BrowserLevel, blocked_origins: Vec<String>) -> Self {
        Self {
            trust,
            blocked_origins,
        }
    }

    /// The decision for one `tools/call`. The extension upstream decides every act tool call
    /// itself (finance pages, card numbers and the rest are categories of its classifier and it
    /// asks through the approval queue), so this is the mechanical part: the level's tool list,
    /// the address scheme and the approval tool.
    pub fn evaluate(&self, tool: &str, args: &Value) -> Verdict {
        if ALWAYS_BLOCKED_TOOLS.contains(&tool) {
            return Verdict::Deny(format!("{tool} is never available in this browser."));
        }
        if tool == APPROVAL_TOOL {
            // The upstream words the card and asks (tier K: always).
            return if self.trust == BrowserLevel::ReadOnly {
                Verdict::Deny(
                    "At Level 1 the browser can not act, so there is nothing to approve.".into(),
                )
            } else {
                Verdict::Allow
            };
        }
        if !KNOWN_TOOLS.contains(&tool) {
            return Verdict::Deny(format!("{tool} is not an available browser tool."));
        }
        if let Some(reason) = unsafe_address(tool, args) {
            return Verdict::Deny(reason);
        }
        if self.trust == BrowserLevel::ReadOnly {
            let action = args["action"].as_str().unwrap_or("");
            // Closing a tab ends something the user may still need.
            let allowed = READ_ONLY_TOOLS.contains(&tool)
                && (tool != "browser_tabs" || matches!(action, "list" | "new" | "select"));
            // `browser_press_key` is the Level 1 way to scroll, for the scroll keys only: Enter or
            // Space could submit a form or press a focused button.
            if allowed
                && tool == "browser_press_key"
                && !args["key"].as_str().is_some_and(is_scroll_key)
            {
                return Verdict::Deny(format!(
                    "browser_press_key at Level 1 (read only) only accepts the scrolling keys {}. Other keys can press a button or submit a form; tell the user what to do, or that the browser level must be raised.",
                    SCROLL_KEYS.join(", ")
                ));
            }
            return if allowed {
                Verdict::Allow
            } else {
                Verdict::Deny(format!(
                    "{tool} is not available: this browser is at Level 1 (read only). Tell the user what to do, or that the browser level must be raised."
                ))
            };
        }
        Verdict::Allow
    }

    /// Whether the gate lists `tool` to the agent.
    pub fn offers(&self, tool: &str) -> bool {
        self.trust.offers(tool)
    }
}

/// `browser_navigate` and a new tab open http and https addresses only, at every level:
/// `javascript:`, `data:`, `file:` and the rest are refused.
fn unsafe_address(tool: &str, args: &Value) -> Option<String> {
    let target = match tool {
        "browser_navigate" => args["url"].as_str().unwrap_or(""),
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
        "browser_navigate" => args["url"].as_str(),
        "browser_tabs" if args["action"] == "new" => args["url"].as_str(),
        _ => None,
    }?;
    Some(target.trim().to_owned()).filter(|t| !t.is_empty())
}

fn clip(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}
fn short_args(args: &Value) -> String {
    let text = args
        .get("element")
        .or_else(|| args.get("target"))
        .and_then(Value::as_str)
        .unwrap_or("");
    clip(text, 120)
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
    /// Set by the run when the user chose "Never for this site" on a suggestion card (then the
    /// reply is `true`, and the gate reads this first).
    pub never: Arc<AtomicBool>,
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
struct Approver {
    sender: mpsc::Sender<Ask>,
    generation: u64,
}

// ---- The upstream server ----

/// Posts one JSON-RPC message to the browser backend (the extension link, or a fake in tests).
pub trait Upstream: Send + Sync {
    /// Returns the reply (for a request) and the `Mcp-Session-Id` the server named, if any.
    /// A server that refused the request answers `Err("HTTP <status>")`.
    fn post(
        &self,
        url: &str,
        session: Option<&str>,
        message: &Value,
    ) -> Result<(Option<Value>, Option<String>), String>;
    /// Whether the MCP session is still the one to use after `error`. The extension link has
    /// exactly one session for its life; a backend that drops its session on an error says so
    /// here and the gate starts a new one.
    fn session_survives(&self, _error: &str) -> bool {
        false
    }
    /// A run of app session `session` starts with the user's own message (never page content).
    /// Only the extension upstream uses it: task domains and named recipients come from it.
    fn run_started(&self, _session: i64, _user_message: &str) {}
    /// The run of app session `session` ended (its approval channel was dropped).
    fn run_ended(&self, _session: i64) {}
    /// A page read in app session `session` carried an injection pattern.
    fn injection_found(&self, _session: i64) {}
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
    /// (and its current tab) is shared by every session of the Mono, so the recipe checks read
    /// this one value whichever session writes.
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
    /// Held by a recipe run for all of it, and by an agent call while it is forwarded, so a run is
    /// never interleaved with agent clicks in the same tab.
    run: Mutex<()>,
    /// Where this Mono keeps its recipes and browser files; none until the app sets it.
    workspace: Mutex<Option<Workspace>>,
    /// The browser calls of each route (app session) since its last saved recipe, in memory only.
    traces: Mutex<std::collections::HashMap<String, Trace>>,
    /// Failed recipe runs waiting for the Mono's repair patch (in memory, with an expiry).
    repairs: recipe::repair::Book,
    /// Which tabs the agent opened and when each was used (see [`tabs`]).
    tabs: Mutex<tabs::TabBook>,
    /// What each route (app session) read and declared this turn (see [`defense`]).
    turns: Mutex<std::collections::HashMap<String, defense::Turn>>,
    /// This gate, for the suggestion card that waits on its own thread.
    me: std::sync::Weak<Inner>,
    calls: AtomicU64,
    generation: AtomicU64,
    next_id: AtomicU64,
    /// Tests: selectors are not turned into refs (see [`Gate::pass_selectors_through`]).
    #[cfg(test)]
    selectors_through: AtomicBool,
    timeout: Duration,
}

/// What the gate needs to keep recipes and to confine the files the browser writes. Without it
/// (some tests) the gate offers no recipe tools and leaves `filename` alone.
#[derive(Clone, Debug)]
pub struct Workspace {
    pub mono_id: String,
    /// The browser's output folder: private, swept, the only place `filename` may point to.
    pub output_dir: PathBuf,
    /// Recipe files. Outside the output folder, so a sweep never touches them.
    pub recipes_dir: PathBuf,
    /// The store database (approved hashes, audit log).
    pub database: PathBuf,
    pub recipe_options: runner::Options,
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
        let inner = Arc::new_cyclic(|me| Inner {
            me: me.clone(),
            turns: Mutex::new(Default::default()),
            tabs: Mutex::new(Default::default()),
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
            traces: Mutex::new(Default::default()),
            repairs: Default::default(),
            calls: AtomicU64::new(0),
            generation: AtomicU64::new(0),
            next_id: AtomicU64::new(100),
            #[cfg(test)]
            selectors_through: AtomicBool::new(false),
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

    /// Tells the gate where this Mono keeps its recipes and browser files. Turns the recipe tools
    /// on and confines `filename` arguments to the output folder.
    pub fn set_workspace(&self, workspace: Workspace) {
        let _ = output::prepare(&workspace.output_dir);
        self.inner.sweep_output(&workspace);
        *self
            .inner
            .workspace
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(workspace);
    }

    /// The Browser tab's dry run of a recipe (see `recipe::manage::dry_run`), through this gate:
    /// the Mono's trust level, blocked sites and approval rules apply to every step. It uses a
    /// route of its own that no run is attached to, so a step that would need a card is denied
    /// instead of asking in some chat. It waits for a running recipe to finish, so call it off
    /// the UI thread.
    pub fn dry_run_recipe(&self, recipe_id: &str) -> Result<recipe::manage::DryRunReport, String> {
        let workspace = self
            .inner
            .workspace()
            .ok_or("The Mono's browser is not ready.")?;
        let _one_at_a_time = self.inner.run.lock().unwrap_or_else(|e| e.into_inner());
        let route = "dry-run";
        let host = GateHost {
            inner: &self.inner,
            route,
            workspace: &workspace,
        };
        let store = recipe::store::RecipeStore::new(workspace.recipes_dir.clone());
        let approvals = MonoBrowserStore::at(workspace.database.clone());
        let result = recipe::manage::dry_run(
            &host,
            &store,
            &approvals,
            &workspace.mono_id,
            recipe_id,
            workspace.recipe_options.clone(),
        );
        self.inner.sweep_output(&workspace);
        result
    }

    /// Removes the browser files of this Mono (its gate stopped): they are page content.
    pub fn clear_output(&self) {
        if let Some(workspace) = self.inner.workspace() {
            output::clear(&workspace.output_dir);
        }
    }

    /// Applies a new policy to the calls that follow, keeping the server.
    pub fn set_policy(&self, policy: Policy) {
        self.inner.lock_state().policy = policy;
    }

    /// Applies the Mono's current trust level and blocked origins, and which backend is behind the
    /// gate (a new one gives a new key). Takes effect on the next call.
    pub fn configure(&self, policy: Policy, upstream_key: &str, upstream: Arc<dyn Upstream>) {
        let mut state = self.inner.lock_state();
        state.policy = policy;
        if state.upstream_key != upstream_key {
            state.upstream_key = upstream_key.to_owned();
            state.upstream = upstream;
            state.session = None;
            state.page.clear();
        }
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
        self.inner.begin_turn(route, request);
    }

    /// Tests that stand in for the extension upstream (which asks through the approval queue):
    /// shows a card on `route` through the gate's own channel and waits for the answer.
    #[cfg(test)]
    pub(crate) fn ask_for_test(&self, route: &str, request: ApprovalRequest) -> Result<(), String> {
        self.inner.ask(route, request)
    }

    /// Tests whose scripted page resolves selectors itself: recipe steps keep their selectors.
    #[cfg(test)]
    pub(crate) fn pass_selectors_through(&self) {
        self.inner.selectors_through.store(true, Ordering::SeqCst);
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

    /// The upstream tools this level offers, plus the approval tool where there is something to
    /// approve.
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
        for tool in &mut tools {
            if tool["name"] == "browser_tabs" {
                tool["inputSchema"]["properties"][tabs::DUPLICATE_FLAG] = json!({
                    "type": "boolean",
                    "description": "Only with action new: open a second tab even when that site is already open. Use it only when the user asked for another tab; otherwise the existing tab is reused.",
                });
                let description = tool["description"].as_str().unwrap_or("").to_owned();
                tool["description"] = format!(
                    "{description} Call action list first and reuse an open tab for the same site: a new tab to a site that is already open switches to that tab instead."
                )
                .into();
            }
        }
        if policy.trust != BrowserLevel::ReadOnly {
            tools.push(approval_tool_definition());
            tools.push(defense::declare_tool_definition());
        }
        if self.workspace().is_some() {
            tools.extend(recipe::tools::definitions(policy.trust));
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
    /// running the call. The extension upstream decides what needs the user's answer itself
    /// (spec 7.6) and asks through the approval queue, so the gate only denies here.
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
        if name == "browser_press_key" && policy.trust == BrowserLevel::ReadOnly {
            self.focus_guard()
                .map_err(|reason| tool_text(reason, true))?;
        }
        Ok(())
    }

    /// Level 1 scrolls with keys, and a key moves the value of a focused select, slider, radio,
    /// listbox or menu. So before the key goes out, the page is probed for the focused element
    /// (`[active]` in the snapshot): a form control or widget refuses the key, and so
    /// does a page whose focus can not be read. A page with nothing focused is fine to scroll.
    fn focus_guard(&self) -> Result<(), String> {
        const UNKNOWN: &str = "browser_press_key is refused at Level 1: the focused element of the page could not be read, and a key could change a control. Use browser_mouse_wheel if it is listed, or tell the user.";
        let probe = self.forward("browser_snapshot", &json!({}));
        if probe["isError"] == true {
            return Err(UNKNOWN.into());
        }
        let text = result_text(&probe);
        if !text.contains("```yaml") {
            return Err(UNKNOWN.into());
        }
        let forest = recipe::snapshot::parse(recipe::snapshot::snapshot_body(&text));
        let focused = recipe::snapshot::all(&forest).into_iter().find(|node| {
            node.attrs.iter().any(|a| a == "active") && FOCUS_WIDGETS.contains(&node.role.as_str())
        });
        match focused {
            Some(node) => Err(format!(
                "browser_press_key is refused at Level 1: a {} (\"{}\") has the focus, and a key could change its value or press it. Tell the user, or raise the browser level.",
                node.role,
                node.name.chars().take(40).collect::<String>()
            )),
            None => Ok(()),
        }
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
        if name == defense::DECLARE_TOOL {
            return self.declare_intent(route, &args);
        }
        if recipe::tools::is_recipe_tool(name) {
            return self.recipe_tool(route, name, &args);
        }
        if let Err(reply) = self.authorize(name, &args) {
            return reply;
        }
        let mut forwarded = args;
        // Never sent on: the backend does not know it.
        let duplicate_wanted = forwarded
            .as_object_mut()
            .and_then(|map| map.remove(tabs::DUPLICATE_FLAG))
            .is_some_and(|flag| flag == true);
        let workspace = self.workspace();
        if let Some(workspace) = &workspace {
            // A file the Mono names goes into the private output folder, never elsewhere.
            if let Err(reason) =
                output::confine_filename(name, &mut forwarded, &workspace.output_dir)
            {
                return tool_text(reason, true);
            }
            if self.calls.fetch_add(1, Ordering::Relaxed) % 25 == 24 {
                self.sweep_output(workspace);
            }
        }
        let page_before = self.lock_state().page.clone();
        let TabOutcome {
            result,
            note,
            performed,
        } = {
            let _one_at_a_time = self.run.lock().unwrap_or_else(|e| e.into_inner());
            self.act_tracking_tabs(route, name, &forwarded, duplicate_wanted)
        };
        if let Some(workspace) = &workspace {
            // A recording keeps what was done in the browser: a call the gate made instead of the
            // agent's (a tab switch) is what a replay must make too.
            if performed.is_empty() {
                self.trace(route, workspace, name, &forwarded, &page_before, &result);
            }
            for (done, args) in &performed {
                self.trace(route, workspace, done, args, &page_before, &result);
            }
            if result["isError"] != true {
                self.maybe_suggest(route, workspace);
            }
        }
        let mut result = self.defend(route, name, result);
        // The gate's own note sits outside the page-content fence.
        if let (Some(note), Some(parts)) = (note, result["content"].as_array_mut()) {
            parts.push(json!({"type": "text", "text": note}));
        }
        result
    }

    /// The tabs the browser has now, or none when the list could not be read.
    fn list_tabs(&self) -> Vec<tabs::Tab> {
        let args = json!({"action": "list"});
        match self.upstream_rpc(
            "tools/call",
            json!({"name": "browser_tabs", "arguments": args}),
        ) {
            Ok(result) if result["isError"] != true => tabs::parse_tabs(&result_text(&result)),
            _ => Vec::new(),
        }
    }
    fn lock_tabs(&self) -> std::sync::MutexGuard<'_, tabs::TabBook> {
        self.tabs.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Whether the policy lets a call the gate derives from the agent's (a tab switch, a close of
    /// a tab the agent made, a navigation to the very address the agent asked for) run without
    /// asking anyone. The agent's own call was authorized once, before; a derived call never
    /// prompts, and one that would is not made.
    fn derived_allowed(&self, route: &str, name: &str, args: &Value) -> bool {
        let (policy, page_url) = {
            let state = self.lock_state();
            (state.policy.clone(), state.page.clone())
        };
        // The same untrusted-content check as the agent's own call gets, but never asked.
        let verdict = policy.evaluate(name, args);
        let verdict = self.apply_taint(route, name, args, &page_url, verdict);
        verdict == Verdict::Allow
    }

    /// [`Inner::act`] for an authorized call that may open a tab or change which one is in use.
    ///
    /// * A new tab (or a navigation) to an address whose origin is already open in a tab the
    ///   agent made switches to that tab instead, and loads the address there. A tab the user
    ///   owns is only switched to, and only when it already shows that page: it is never
    ///   navigated away.
    /// * The tabs the agent made are closed least recently used first when a ninth tab would open
    ///   ([`tabs::MAX_TABS`]), only where the policy allows a close without asking.
    /// * The switch is checked on the result (the current tab must be the one expected), and any
    ///   doubt or failure falls back to the call exactly as the agent made it.
    ///
    /// Everything the gate does itself is reported in `performed`, so a recording replays it.
    fn act_tracking_tabs(
        &self,
        route: &str,
        name: &str,
        forwarded: &Value,
        duplicate_wanted: bool,
    ) -> TabOutcome {
        let action = forwarded["action"].as_str().unwrap_or("");
        let opens_tab = name == "browser_tabs" && action == "new";
        let navigates = name == "browser_navigate";
        let url = forwarded["url"].as_str().unwrap_or("").trim().to_owned();
        if !(opens_tab || navigates) {
            return self.as_asked(name, forwarded, &url);
        }
        let list = self.list_tabs();
        if list.is_empty() {
            // The list could not be read: do what the agent asked, as before.
            return self.as_asked(name, forwarded, &url);
        }
        self.lock_tabs().sync(&list);
        let reuse = if url.is_empty() || (opens_tab && duplicate_wanted) {
            None
        } else {
            self.lock_tabs().reusable(&list, &url)
        };
        if let Some(reuse) = reuse {
            if let Some(outcome) = self.reuse_tab(route, name, forwarded, &list, reuse, &url) {
                return outcome;
            }
        }
        let note = if opens_tab && list.len() >= tabs::MAX_TABS {
            self.close_least_used_agent_tab(route, &list)
        } else {
            None
        };
        let mut outcome = self.as_asked(name, forwarded, &url);
        outcome.note = note;
        outcome
    }

    /// The call as the agent made it, with the tab book kept in step.
    fn as_asked(&self, name: &str, forwarded: &Value, url: &str) -> TabOutcome {
        let result = self.forward(name, forwarded);
        if result["isError"] != true {
            let mut book = self.lock_tabs();
            let list = tabs::parse_tabs(&result_text(&result));
            let opened = name == "browser_tabs" && forwarded["action"] == "new";
            if opened && !list.is_empty() {
                book.opened(&list, url);
            } else if name == "browser_tabs" {
                if forwarded["action"] == "close" {
                    if let Some(index) = tabs::close_index(forwarded, &book) {
                        book.closed(index);
                    }
                }
                if !list.is_empty() {
                    book.sync(&list);
                }
            } else {
                book.touch_current();
            }
        }
        TabOutcome {
            result,
            note: None,
            performed: Vec::new(),
        }
    }

    /// Shows `url` in the open tab `reuse`, or `None` when that is not safe or did not work (the
    /// caller then makes the agent's own call).
    fn reuse_tab(
        &self,
        route: &str,
        name: &str,
        forwarded: &Value,
        list: &[tabs::Tab],
        reuse: tabs::Reuse,
        url: &str,
    ) -> Option<TabOutcome> {
        let target = &list[reuse.index];
        let opens_tab = name == "browser_tabs";
        let page_same = tabs::same_page(url, &target.url);
        // Never navigate a tab the user owns: it is reusable only as it is.
        if !reuse.agent && !page_same {
            return None;
        }
        let mut performed = Vec::new();
        let mut shown = None;
        if !target.current {
            let back = list.iter().position(|t| t.current);
            let select = json!({"action": "select", "index": reuse.index});
            if !self.derived_allowed(route, "browser_tabs", &select) {
                return None;
            }
            let result = self.forward("browser_tabs", &select);
            // The indexes may have moved since the list: the tab now in use must be the one
            // that was meant (same address), or the switch is undone.
            let now = tabs::parse_tabs(&result_text(&result));
            let landed = result["isError"] != true
                && now.iter().filter(|t| t.current).count() == 1
                && now
                    .get(reuse.index)
                    .is_some_and(|t| t.current && t.url == target.url);
            if !landed {
                // Undo the switch once, then read the list again (no loop): the book follows
                // whatever the browser shows now.
                if let Some(back) = back {
                    let _ =
                        self.forward("browser_tabs", &json!({"action": "select", "index": back}));
                }
                let after = self.list_tabs();
                if !after.is_empty() {
                    self.lock_tabs().sync(&after);
                }
                return None;
            }
            self.lock_tabs().sync(&now);
            performed.push(("browser_tabs".to_owned(), select));
            shown = Some(result);
        }
        let note = Some(tabs::reuse_note(reuse.index, &target.url));
        if opens_tab {
            let result = if page_same {
                match shown {
                    Some(result) => result,
                    None => self.forward("browser_tabs", &json!({"action": "list"})),
                }
            } else {
                // The agent's authorized address, exactly, in the tab the agent made.
                let navigate = json!({"url": url});
                if !self.derived_allowed(route, "browser_navigate", &navigate) {
                    return None;
                }
                let result = self.forward("browser_navigate", &navigate);
                if result["isError"] == true {
                    return None;
                }
                performed.push(("browser_navigate".to_owned(), navigate));
                self.lock_tabs().touch_current();
                result
            };
            return Some(TabOutcome {
                result,
                note,
                performed,
            });
        }
        // `browser_navigate`: a user's tab we switched to already shows the page; an agent tab or
        // the current tab gets the agent's own call.
        if !reuse.agent && !target.current {
            let result = shown?;
            return Some(TabOutcome {
                result,
                note,
                performed,
            });
        }
        let result = self.forward(name, forwarded);
        self.lock_tabs().touch_current();
        performed.push((name.to_owned(), forwarded.clone()));
        Some(TabOutcome {
            result,
            note,
            performed,
        })
    }

    /// Closes the agent-made tab used longest ago, to stay within [`tabs::MAX_TABS`]. Only when
    /// the policy allows a close without asking (Level 1 does not), the tab is the agent's with
    /// certainty, and a fresh list is exactly the one the choice was made on. Returns a note for
    /// the agent when a tab was closed; otherwise the agent just opens its tab.
    fn close_least_used_agent_tab(&self, route: &str, list: &[tabs::Tab]) -> Option<String> {
        let (index, url) = {
            let book = self.lock_tabs();
            let index = book.victim()?;
            (index, book.url_at(index)?.to_owned())
        };
        let close = json!({"action": "close", "index": index});
        if !self.derived_allowed(route, "browser_tabs", &close) {
            return None;
        }
        let fresh = self.list_tabs();
        let same = fresh.len() == list.len()
            && fresh.iter().zip(list).all(|(a, b)| a.url == b.url)
            && fresh.get(index).is_some_and(|t| t.url == url);
        if !same {
            return None;
        }
        let result = self.forward("browser_tabs", &close);
        if result["isError"] == true {
            return None;
        }
        let mut book = self.lock_tabs();
        book.closed(index);
        let after = tabs::parse_tabs(&result_text(&result));
        if !after.is_empty() {
            book.sync(&after);
        }
        Some(format!(
            "zptah closed the tab {url} that you opened earlier and used least, because this browser keeps at most {} tabs. Tabs the user opened are never closed.",
            tabs::MAX_TABS
        ))
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

    /// Adds an agent call to its route's trace (in memory; see `recipe::record`).
    fn trace(
        &self,
        route: &str,
        workspace: &Workspace,
        name: &str,
        args: &Value,
        page_before: &str,
        result: &Value,
    ) {
        let text = result_text(result);
        let ok = result["isError"] != true;
        let page_after = self.lock_state().page.clone();
        let mut traces = self.traces.lock().unwrap_or_else(|e| e.into_inner());
        let trace = traces.entry(route.to_owned()).or_default();
        if ok {
            // A saved snapshot (`- [Snapshot](page-1.yml)`) is read from the private output folder.
            let saved = output::linked_file(&text, &workspace.output_dir)
                .filter(|path| path.metadata().is_ok_and(|m| m.len() < 4_000_000))
                .and_then(|path| std::fs::read_to_string(path).ok());
            trace.learn(saved.as_deref().unwrap_or(&text));
        }
        trace.note(name, args, page_before, &page_after, ok);
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

    /// Turns the selector a recipe step puts in `target` into the ref of the element it names
    /// (spec 15.5): the page is snapshotted and the selector matched against it. `Err` is what
    /// the step gets instead of running: the answer of a locator that found nothing, or several.
    fn with_refs(&self, tool: &str, mut args: Value) -> Result<Value, runner::ToolOutput> {
        #[cfg(test)]
        if self.selectors_through.load(Ordering::SeqCst) {
            return Ok(args);
        }
        // Where the call names elements: its `target`, and the `target` of each form field.
        let mut slots: Vec<(Option<usize>, String)> = Vec::new();
        if let Some(selector) = refs::selector_of(&args) {
            slots.push((None, selector.to_owned()));
        }
        for (at, field) in args["fields"].as_array().into_iter().flatten().enumerate() {
            if let Some(selector) = refs::selector_of(field) {
                slots.push((Some(at), selector.to_owned()));
            }
        }
        if slots.is_empty() {
            return Ok(args);
        }
        let failed = |text: String| runner::ToolOutput {
            text,
            is_error: true,
        };
        let snapshot = self.forward("browser_snapshot", &json!({}));
        let text = result_text(&snapshot);
        if snapshot["isError"] == true {
            return Err(failed(text));
        }
        let forest = recipe::snapshot::parse(recipe::snapshot::snapshot_body(&text));
        // Every element must resolve to exactly one: otherwise the whole step fails.
        let mut found = Vec::new();
        for (at, selector) in &slots {
            match refs::locate(&forest, selector) {
                refs::Located::Ref(reference) => found.push((*at, reference)),
                refs::Located::Multiple(count) => {
                    return Err(failed(format!(
                        "Error: strict mode violation: locator resolved to {count} elements for {selector}"
                    )))
                }
                refs::Located::Missing | refs::Located::Unsupported => {
                    return Err(failed(format!(
                        "Error: \"{selector}\" does not match any elements ({tool})."
                    )))
                }
            }
        }
        for (at, reference) in found {
            match at {
                None => args["target"] = json!(reference),
                Some(at) => args["fields"][at]["target"] = json!(reference),
            }
        }
        Ok(args)
    }

    /// A `recipe_*` tool call. The browser-using ones (`run`, `record`) hold the run lock.
    fn recipe_tool(&self, route: &str, name: &str, args: &Value) -> Value {
        let Some(workspace) = self.workspace() else {
            return tool_text("Recipes are not available here.", true);
        };
        let trust = self.lock_state().policy.trust;
        if !recipe::tools::offered(name, trust) {
            return tool_text(
                format!("{name} is not available at this browser level (read only)."),
                true,
            );
        }
        if matches!(name, recipe::tools::RUN | recipe::tools::REPAIR) {
            if let Err(reason) = self.check_recipe_params(route, name, args) {
                return tool_text(reason, true);
            }
        }
        let uses_browser = matches!(
            name,
            recipe::tools::RUN | recipe::tools::RECORD | recipe::tools::REPAIR
        );
        let _one_at_a_time =
            uses_browser.then(|| self.run.lock().unwrap_or_else(|e| e.into_inner()));
        let store = recipe::store::RecipeStore::new(workspace.recipes_dir.clone());
        let approvals = MonoBrowserStore::at(workspace.database.clone());
        let trace = self
            .traces
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(route)
            .cloned()
            .unwrap_or_default();
        let host = GateHost {
            inner: self,
            route,
            workspace: &workspace,
        };
        // The user accepted "Save as recipe?": the recorder keeps its checks and its dry run,
        // but does not ask a second time. Never after untrusted content or an injection signal.
        let accepted = name == recipe::tools::RECORD
            && self.with_turn(route, |t| t.save_ok && !t.tainted && !t.flagged());
        let ask = |request: ApprovalRequest| {
            if accepted && request.category == Category::Recipe {
                Ok(())
            } else {
                self.ask(route, request)
            }
        };
        let context = recipe::tools::Context {
            mono_id: &workspace.mono_id,
            store: &store,
            approvals: Some(&approvals),
            host: &host,
            ask: &ask,
            trace: &trace,
            options: workspace.recipe_options.clone(),
            repairs: &self.repairs,
        };
        let reply = recipe::tools::call(name, args, &context);
        if uses_browser {
            // A run reads many snapshots: sweep at the end of each one.
            self.sweep_output(&workspace);
        }
        if name == recipe::tools::RECORD && !reply.is_error {
            if let Some(trace) = self
                .traces
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get_mut(route)
            {
                trace.clear();
            }
        }
        if name == recipe::tools::RECORD && !reply.is_error {
            self.with_turn(route, |t| t.save_ok = false);
        }
        // What a run or a repair returns is page content (extracted fields, an observation).
        if reply.page {
            return match self.after_read(route, name, &reply.text, true) {
                Err(reason) => tool_text(
                    format!("{reason} The result was withheld: it looked like a prompt injection."),
                    true,
                ),
                Ok(hits) => {
                    let mut out = tool_text(
                        defense::fence(&reply.text, &defense::new_nonce()),
                        reply.is_error,
                    );
                    if !hits.is_empty() {
                        out["content"].as_array_mut().into_iter().for_each(|parts| {
                            parts.push(json!({"type": "text", "text": guard::INJECTION_NOTE}))
                        });
                    }
                    out
                }
            };
        }
        tool_text(reply.text, reply.is_error)
    }

    /// Shows the card and waits. Everything but an explicit Allow is an error text for the agent.
    fn ask(&self, route: &str, request: ApprovalRequest) -> Result<(), String> {
        self.ask_full(route, request).map(|_| ())
    }

    /// [`Inner::ask`] that also tells whether the user chose "Never for this site" (only a
    /// suggestion card offers it).
    fn ask_full(&self, route: &str, request: ApprovalRequest) -> Result<bool, String> {
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
        let never = Arc::new(AtomicBool::new(false));
        let ask = Ask {
            request,
            reply,
            abandoned: abandoned.clone(),
            never: never.clone(),
        };
        if sender.send(ask).is_err() {
            return Err(guard::NO_CHANNEL.into());
        }
        match answer.recv_timeout(self.timeout) {
            Ok(true) => Ok(never.load(Ordering::SeqCst)),
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
        // A new connection owns none of the tabs of the old one: nothing is the agent's to close.
        self.lock_tabs().reset();
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

/// Roles that react to the scroll keys (or to a press) when they have the focus.
const FOCUS_WIDGETS: [&str; 22] = [
    "textbox",
    "searchbox",
    "combobox",
    "listbox",
    "option",
    "slider",
    "spinbutton",
    "radio",
    "radiogroup",
    "checkbox",
    "switch",
    "menu",
    "menubar",
    "menuitem",
    "menuitemcheckbox",
    "menuitemradio",
    "tab",
    "tablist",
    "button",
    "tree",
    "treeitem",
    "grid",
];

/// What [`Inner::act_tracking_tabs`] did.
struct TabOutcome {
    result: Value,
    /// For the agent, outside the page-content fence.
    note: Option<String>,
    /// The calls the gate made in place of or besides the agent's, with their arguments; empty
    /// when it made the agent's call as it was.
    performed: Vec<(String, Value)>,
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

/// A recipe run's way to the browser: every step passes the same policy as an agent call, and
/// asks the user where the policy says so.
struct GateHost<'a> {
    inner: &'a Inner,
    route: &'a str,
    workspace: &'a Workspace,
}
impl runner::Host for GateHost<'_> {
    fn call(&self, tool: &str, args: Value) -> Result<runner::ToolOutput, String> {
        if let Err(reply) = self.inner.authorize(tool, &args) {
            return Err(result_text(&reply));
        }
        // A recipe names elements by selector; the browser takes refs (see `refs`).
        let args = match self.inner.with_refs(tool, args) {
            Ok(args) => args,
            Err(output) => return Ok(output),
        };
        let result = self.inner.forward(tool, &args);
        // A page a recipe step reads is scanned like one the agent reads, but it does not taint
        // the turn until the run's output is read (the user approved the steps).
        self.inner
            .after_read(self.route, tool, &result_text(&result), false)?;
        Ok(runner::ToolOutput {
            text: result_text(&result),
            is_error: result["isError"] == true,
        })
    }
    fn page_url(&self) -> String {
        self.inner.lock_state().page.clone()
    }
    fn trust(&self) -> BrowserLevel {
        self.inner.lock_state().policy.trust
    }
    fn blocked_origins(&self) -> Vec<String> {
        self.inner.lock_state().policy.blocked_origins.clone()
    }
    fn pause(&self, duration: Duration) {
        if !duration.is_zero() {
            std::thread::sleep(duration);
        }
    }
    fn audit(&self, tool: &str, url: &str, summary: &str) {
        self.inner
            .audit(self.route, self.workspace, tool, url, summary);
    }
}

#[cfg(all(test, unix))]
mod extension_tests;
#[cfg(test)]
mod tests;
