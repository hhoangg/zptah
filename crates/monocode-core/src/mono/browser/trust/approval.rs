//! The queue of approval cards of the extension backend (spec 8.2): the API the card in the app
//! (and any other front end) uses.
//!
//! The gate asks with [`ApprovalQueue::ask`], which blocks the tool call until the user answers,
//! the card times out, or the run ends. A front end reads [`ApprovalQueue::pending`], shows each
//! [`PendingApproval`] (text from [`PendingApproval::text`], the picture at
//! [`PendingApproval::screenshot`]) and answers with [`ApprovalQueue::answer`]. It is told that
//! something changed through [`ApprovalQueue::subscribe`]; a front end that never subscribed
//! cannot answer, so an ask is denied at once (the gate fails closed).
//!
//! There is one queue per Mono, found with [`queue_for`]; [`all_pending`] lists every Mono's open
//! cards for the attention badge.

use super::super::copy::{self, CardFacts, CardText, Lang};
use super::super::risk::Category;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The user's answer to a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserAnswer {
    /// "Allow once".
    AllowOnce,
    /// "Allow in this conversation". Ignored (as "Allow once") when the card does not offer it.
    AllowConversation,
    /// "Don't allow".
    Deny,
}

/// How an ask ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Answered(UserAnswer),
    /// Nobody answered within the timeout.
    TimedOut,
    /// Nobody could answer (no front end, plan or read-only run), or the run ended or was
    /// stopped while the card was open.
    Withdrawn,
}

/// The picture of the element the card is about (saved in the Mono's output folder).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardScreenshot {
    /// The file name inside the output folder; the audit row keeps this name.
    pub file_name: String,
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// `visible` or `debugger`.
    pub method: String,
}

/// What the gate hands over to ask.
#[derive(Debug, Clone)]
pub struct Request {
    pub mono_id: String,
    pub session_id: i64,
    /// The tool the Mono called (`browser_click`, ...).
    pub tool: String,
    pub tab: Option<u64>,
    pub facts: CardFacts,
    pub screenshot: Option<CardScreenshot>,
    /// A conversation grant is offered (false for payment, credential entry and tier K tools).
    pub offer_conversation: bool,
}

/// An open card.
#[derive(Debug, Clone)]
pub struct PendingApproval {
    /// Unguessable; also the `approval_id` of the audit row.
    pub id: String,
    pub created_ms: u64,
    /// When the card gives up (the gate denies it then).
    pub expires_ms: u64,
    pub mono_id: String,
    pub session_id: i64,
    pub tool: String,
    pub tab: Option<u64>,
    pub facts: CardFacts,
    pub screenshot: Option<CardScreenshot>,
    pub offer_conversation: bool,
}

impl PendingApproval {
    /// The card text in `lang`.
    pub fn text_in(&self, lang: Lang) -> CardText {
        let mut text = copy::card(lang, &self.facts);
        if !self.offer_conversation {
            text.allow_conversation = None;
        }
        text
    }

    /// The card text in the app language ([`copy::app_language`]).
    pub fn text(&self) -> CardText {
        self.text_in(copy::app_language())
    }
}

type Notify = Arc<dyn Fn() + Send + Sync>;

struct Open {
    pending: PendingApproval,
    reply: mpsc::Sender<Outcome>,
}

#[derive(Default)]
struct Inner {
    open: Vec<Open>,
    listeners: Vec<(u64, Notify)>,
    notices: Vec<Notice>,
}

/// Something the user must know that is not a question.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    /// The Mono needs a password: zptah never types one (spec 8.6). The user signs in themselves.
    PasswordNeeded,
}

/// A card with no answer to give: its only button is "Show tab" (and dismiss).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub id: String,
    pub kind: NoticeKind,
    pub tab: Option<u64>,
    pub mono_label: String,
    pub site: String,
    pub created_ms: u64,
}

impl Notice {
    pub fn text_in(&self, lang: Lang) -> CardText {
        match self.kind {
            NoticeKind::PasswordNeeded => copy::password_stop(lang, &self.mono_label, &self.site),
        }
    }

    pub fn text(&self) -> CardText {
        self.text_in(copy::app_language())
    }
}

const MAX_NOTICES: usize = 20;

/// The cards of one Mono. Cheap to share; all methods take `&self`.
pub struct ApprovalQueue {
    mono_id: String,
    inner: Mutex<Inner>,
    next: AtomicU64,
    /// Plan, read-only and control-less runs cannot answer a card.
    can_answer: AtomicBool,
}

impl std::fmt::Debug for ApprovalQueue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalQueue")
            .field("mono_id", &self.mono_id)
            .finish_non_exhaustive()
    }
}

/// Keeps a front end subscribed; dropping it unsubscribes.
pub struct Subscription {
    queue: Arc<ApprovalQueue>,
    id: u64,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let last = {
            let mut inner = self.queue.lock();
            inner.listeners.retain(|(id, _)| *id != self.id);
            inner.listeners.is_empty()
        };
        // Nobody can answer any more: open cards are denied now, not after the timeout.
        if last {
            self.queue.withdraw_all();
        }
    }
}

impl ApprovalQueue {
    pub fn new(mono_id: &str) -> Arc<Self> {
        Arc::new(Self {
            mono_id: mono_id.to_owned(),
            inner: Mutex::new(Inner::default()),
            next: AtomicU64::new(1),
            can_answer: AtomicBool::new(true),
        })
    }

    pub fn mono_id(&self) -> &str {
        &self.mono_id
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Calls `notify` (on any thread) whenever a card opens or closes. A front end must keep the
    /// result alive for as long as it can show cards.
    pub fn subscribe(self: &Arc<Self>, notify: impl Fn() + Send + Sync + 'static) -> Subscription {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.lock().listeners.push((id, Arc::new(notify)));
        Subscription {
            queue: self.clone(),
            id,
        }
    }

    /// Shows a notice (a stop, not a question). The oldest go when there are many.
    pub fn post_notice(&self, kind: NoticeKind, tab: Option<u64>, mono_label: &str, site: &str) {
        {
            let mut inner = self.lock();
            if inner
                .notices
                .iter()
                .any(|n| n.kind == kind && n.tab == tab && n.site == site)
            {
                return;
            }
            inner.notices.push(Notice {
                id: format!("{:032x}", rand::random::<u128>()),
                kind,
                tab,
                mono_label: mono_label.to_owned(),
                site: site.to_owned(),
                created_ms: crate::time::now_ms(),
            });
            let excess = inner.notices.len().saturating_sub(MAX_NOTICES);
            inner.notices.drain(..excess);
        }
        self.notify();
    }

    /// The notices the user has not dismissed, oldest first.
    pub fn notices(&self) -> Vec<Notice> {
        self.lock().notices.clone()
    }

    /// Dismisses a notice. False when it is already gone.
    pub fn dismiss_notice(&self, id: &str) -> bool {
        let removed = {
            let mut inner = self.lock();
            let before = inner.notices.len();
            inner.notices.retain(|n| n.id != id);
            before != inner.notices.len()
        };
        if removed {
            self.notify();
        }
        removed
    }

    /// Whether a front end is subscribed.
    pub fn has_listener(&self) -> bool {
        !self.lock().listeners.is_empty()
    }

    /// The run sets this to `false` when nobody can answer a card (plan mode, read-only access,
    /// no controls), `true` again for a run that can.
    pub fn set_can_answer(&self, can: bool) {
        self.can_answer.store(can, Ordering::SeqCst);
    }

    /// The open cards, oldest first.
    pub fn pending(&self) -> Vec<PendingApproval> {
        self.lock().open.iter().map(|o| o.pending.clone()).collect()
    }

    fn notify(&self) {
        let listeners: Vec<Notify> = self
            .lock()
            .listeners
            .iter()
            .map(|(_, n)| n.clone())
            .collect();
        for listener in listeners {
            listener();
        }
    }

    /// Delivers the user's answer. False when the card is not open any more (answered, timed out
    /// or withdrawn).
    pub fn answer(&self, id: &str, answer: UserAnswer) -> bool {
        let found = {
            let mut inner = self.lock();
            inner
                .open
                .iter()
                .position(|o| o.pending.id == id)
                .map(|at| inner.open.remove(at))
        };
        let Some(open) = found else {
            return false;
        };
        let answer = if answer == UserAnswer::AllowConversation && !open.pending.offer_conversation
        {
            UserAnswer::AllowOnce
        } else {
            answer
        };
        let _ = open.reply.send(Outcome::Answered(answer));
        self.notify();
        true
    }

    /// Closes every open card of the Mono (the run ended, Stop was pressed): each ask ends
    /// [`Outcome::Withdrawn`].
    pub fn withdraw_all(&self) {
        let drained: Vec<Open> = std::mem::take(&mut self.lock().open);
        if drained.is_empty() {
            return;
        }
        for open in drained {
            let _ = open.reply.send(Outcome::Withdrawn);
        }
        self.notify();
    }

    /// Closes the open cards of one app session.
    pub fn withdraw_session(&self, session_id: i64) {
        let drained: Vec<Open> = {
            let mut inner = self.lock();
            let (gone, kept): (Vec<Open>, Vec<Open>) = std::mem::take(&mut inner.open)
                .into_iter()
                .partition(|o| o.pending.session_id == session_id);
            inner.open = kept;
            gone
        };
        if drained.is_empty() {
            return;
        }
        for open in drained {
            let _ = open.reply.send(Outcome::Withdrawn);
        }
        self.notify();
    }

    /// Opens a card and waits for the answer for at most `timeout`. `tick` runs about every
    /// `tick_every` while waiting (the gate keeps the prepared action alive with it). Returns
    /// the card id too, which the audit row keeps.
    pub fn ask(
        &self,
        request: Request,
        timeout: Duration,
        tick_every: Duration,
        mut tick: impl FnMut(),
    ) -> (String, Outcome) {
        let id = format!("{:032x}", rand::random::<u128>());
        if !self.can_answer.load(Ordering::SeqCst) || !self.has_listener() {
            return (id, Outcome::Withdrawn);
        }
        let now = crate::time::now_ms();
        let (reply, answer) = mpsc::channel();
        let pending = PendingApproval {
            id: id.clone(),
            created_ms: now,
            expires_ms: now + timeout.as_millis() as u64,
            mono_id: request.mono_id,
            session_id: request.session_id,
            tool: request.tool,
            tab: request.tab,
            facts: request.facts,
            screenshot: request.screenshot,
            offer_conversation: request.offer_conversation,
        };
        self.lock().open.push(Open { pending, reply });
        self.notify();
        let started = Instant::now();
        let outcome = loop {
            let left = timeout.saturating_sub(started.elapsed());
            if left.is_zero() {
                break Outcome::TimedOut;
            }
            match answer.recv_timeout(left.min(tick_every)) {
                Ok(outcome) => break outcome,
                Err(mpsc::RecvTimeoutError::Timeout) => tick(),
                Err(mpsc::RecvTimeoutError::Disconnected) => break Outcome::Withdrawn,
            }
        };
        if matches!(outcome, Outcome::TimedOut | Outcome::Withdrawn) {
            let removed = {
                let mut inner = self.lock();
                let before = inner.open.len();
                inner.open.retain(|o| o.pending.id != id);
                before != inner.open.len()
            };
            if removed {
                self.notify();
            }
        }
        (id, outcome)
    }
}

fn registry() -> &'static Mutex<HashMap<String, Arc<ApprovalQueue>>> {
    static QUEUES: OnceLock<Mutex<HashMap<String, Arc<ApprovalQueue>>>> = OnceLock::new();
    QUEUES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The queue of a Mono, created on first use.
pub fn queue_for(mono_id: &str) -> Arc<ApprovalQueue> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(mono_id.to_owned())
        .or_insert_with(|| ApprovalQueue::new(mono_id))
        .clone()
}

/// Every open card of every Mono, oldest first (for the attention badge and the notification).
pub fn all_pending() -> Vec<PendingApproval> {
    let queues: Vec<Arc<ApprovalQueue>> = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
        .cloned()
        .collect();
    let mut all: Vec<PendingApproval> = queues.iter().flat_map(|q| q.pending()).collect();
    all.sort_by_key(|p| p.created_ms);
    all
}

/// The categories a conversation grant may cover in `categories` (none: the button is not
/// offered).
pub fn offers_conversation(categories: &[Category], always_asks_tool: bool) -> bool {
    !always_asks_tool
        && !categories.is_empty()
        && !categories
            .iter()
            .any(|c| super::super::risk::decide::never_granted(*c))
}
