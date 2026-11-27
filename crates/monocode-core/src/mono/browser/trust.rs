//! Two-level trust, task state and the decision for the extension backend (spec 7.4 to 7.6, 8.1,
//! 8.4).
//!
//! * [`BrowserLevel`]: `read-only` and `act`. The ids of the retired levels still read (both
//!   asking levels become `act`); an unknown stored value reads as `read-only`, never as `act`.
//! * [`TrustState`]: what the gate remembers for a conversation: task domains, domains reached
//!   by a link (read-only until approved), sensitive sites already asked about, conversation-long
//!   grants ([`grants::GrantBook`]) and the taint of the run.
//! * [`TrustState::evaluate`]: puts the pieces of 7.6 together (classifier categories, declared
//!   risk, extension risk, soul rules, first visit and taint) and calls [`decide`].
//!
//! Typed values are only read to look for a card number and are never stored: the audit keeps the
//! field kind and the length ([`audit::typed_summary`]).

pub mod approval;
pub mod audit;
pub mod grants;
pub mod rules;

use super::risk::decide::{decide, effective, never_granted, Decision, DecisionInput};
use super::risk::signals::{
    domains_in_text, element_classification, gate_categories, navigation_category,
    registrable_domain_of, site_category, ActionKind, ElementSignals, FormSignals, TaskContext,
};
use super::risk::Category;
use super::soul_rules::{ActionFacts, FactKind, Parsed};
use super::{ALWAYS_BLOCKED_TOOLS, KNOWN_TOOLS, READ_ONLY_TOOLS};
use grants::GrantBook;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

// ---- Levels ----

/// How far a Mono may act (spec 8.1). There is no "ask before every action" level.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BrowserLevel {
    /// Reads and navigates; never clicks, types or changes a page.
    ReadOnly,
    /// Acts for the user inside the task; asks only for the risky categories.
    #[default]
    Act,
}

impl BrowserLevel {
    pub const ALL: [BrowserLevel; 2] = [Self::ReadOnly, Self::Act];

    pub fn id(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Act => "act",
        }
    }

    /// The level with this id. The ids of the retired levels read as `act` (spec 15.3); an unknown
    /// id is `None`.
    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "read-only" | "readOnly" => Some(Self::ReadOnly),
            "act" | "ask-risky" | "askRisky" | "ask-every" | "askEvery" | "autonomous" => {
                Some(Self::Act)
            }
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ReadOnly => "Read only",
            Self::Act => "Act for me",
        }
    }
}

impl<'de> Deserialize<'de> for BrowserLevel {
    /// A retired asking level reads as `act`; anything else that is not known (also `null` or a
    /// value that is not text) reads as `read-only`: a value this build does not understand never
    /// gives a Mono more power than the least one.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(value
            .as_str()
            .and_then(Self::from_id)
            .unwrap_or(Self::ReadOnly))
    }
}

impl BrowserLevel {
    /// The level as a number, for recipes (`min_level`): 1 reads, 2 acts.
    pub fn level(self) -> u8 {
        match self {
            Self::ReadOnly => 1,
            Self::Act => 2,
        }
    }

    /// The short explanation shown under the level.
    pub fn explanation(self) -> &'static str {
        match self {
            Self::ReadOnly => "Looks only: opens pages, reads them, takes screenshots, reads the console and scrolls. It cannot click, type or submit.",
            Self::Act => "Reads, clicks, types, fills forms and uploads files on its own, and asks you first only before risky steps such as payments, deletions, sending, posting and account changes. It never types passwords or one-time codes, and an upload always asks.",
        }
    }

    /// What the app guarantees, shown once under the levels.
    pub const ENFORCEMENT_NOTE: &'static str = "zptah checks every step itself, before it runs and whatever the Mono was told. A payment always asks you, every time.";

    /// The tools the harness must not offer at this level.
    pub fn disabled_tools(self) -> Vec<&'static str> {
        match self {
            Self::ReadOnly => KNOWN_TOOLS
                .into_iter()
                .filter(|tool| !READ_ONLY_TOOLS.contains(tool))
                .collect(),
            Self::Act => ALWAYS_BLOCKED_TOOLS.to_vec(),
        }
    }

    /// The tools a harness must not offer on an Ardia gate at this level: at read only the act
    /// tools and the two gate-only tools (approval request, declared intent), always the tools no
    /// level allows.
    pub fn disabled_tools_ardia(self) -> Vec<&'static str> {
        self.disabled_tools_ardia_for(true)
    }

    /// [`Self::disabled_tools_ardia`] for a system that has (`uploads`) or has not a private
    /// folder for the copies of uploaded files: without one `browser_upload` is disabled at every
    /// level.
    pub fn disabled_tools_ardia_for(self, uploads: bool) -> Vec<&'static str> {
        let mut tools = ALWAYS_BLOCKED_TOOLS.to_vec();
        if !uploads {
            tools.push("browser_upload");
        }
        if self == Self::ReadOnly {
            tools.extend(super::ardia::upstream::ACT_TOOLS);
            tools.push(super::gate::APPROVAL_TOOL);
            tools.push(super::defense::DECLARE_TOOL);
        }
        tools
    }

    /// Whether the gate lists `tool` to the agent at this level.
    pub fn offers(self, tool: &str) -> bool {
        KNOWN_TOOLS.contains(&tool) && !self.disabled_tools().contains(&tool)
    }
}

/// Whether a risk category needs the user's answer at `level`. At `read-only` nothing that acts is
/// allowed at all (the decision denies it first), so there is nothing to approve; at `act` every
/// category of the classifier asks, unless a live grant covers it (see [`decide`]).
pub fn approval_required(level: BrowserLevel, _category: Category) -> bool {
    level == BrowserLevel::Act
}

// ---- Tiers (spec 7.1) ----

/// What kind of tool a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// Reads; never asks by itself (the sensitive-site rule can).
    Read,
    /// Acts inside the task unless the classifier finds a risk.
    Act,
    /// Always asks.
    Always,
    /// Not offered and refused by name.
    Refused,
}

impl Tier {
    pub fn letter(self) -> &'static str {
        match self {
            Self::Read => "R",
            Self::Act => "A",
            Self::Always => "K",
            Self::Refused => "X",
        }
    }
}

fn tabs_action(args: &Value) -> &str {
    args["action"].as_str().unwrap_or("list")
}

/// The tier of a tool call (the table of 7.1).
pub fn tier_of(tool: &str, args: &Value) -> Tier {
    match tool {
        "browser_tabs" => match tabs_action(args) {
            "list" | "select" => Tier::Read,
            "new" | "close" | "adopt" | "release" => Tier::Act,
            _ => Tier::Refused,
        },
        "browser_snapshot"
        | "browser_find"
        | "browser_read"
        | "browser_take_screenshot"
        | "browser_wait_for"
        | "browser_mouse_wheel"
        | "browser_declare_intent" => Tier::Read,
        "browser_press_key" => {
            if args["key"].as_str().is_some_and(super::is_scroll_key) {
                Tier::Read
            } else {
                Tier::Act
            }
        }
        "browser_navigate"
        | "browser_navigate_back"
        | "browser_navigate_forward"
        | "browser_click"
        | "browser_type"
        | "browser_fill_form"
        | "browser_select_option"
        | "browser_hover"
        | "browser_drag"
        | "browser_handle_dialog" => Tier::Act,
        "browser_file_upload" | super::gate::APPROVAL_TOOL => Tier::Always,
        _ => Tier::Refused,
    }
}

/// Whether the tool is marked "yes" in the "Read only level" column of 7.1.
pub fn read_only_ok(tool: &str, args: &Value) -> bool {
    match tool {
        "browser_tabs" => matches!(tabs_action(args), "list" | "new" | "select"),
        "browser_navigate" | "browser_navigate_back" | "browser_navigate_forward" => true,
        _ => tier_of(tool, args) == Tier::Read,
    }
}

/// Tools whose result carries page content, so a first read of a sensitive site asks (7.4).
fn reads_page(tool: &str) -> bool {
    matches!(
        tool,
        "browser_snapshot" | "browser_find" | "browser_read" | "browser_take_screenshot"
    )
}

/// Tools that move through history or only hover: they change no page content, so a broken rules
/// block does not make them ask.
fn is_non_writing(tool: &str) -> bool {
    matches!(
        tool,
        "browser_navigate" | "browser_navigate_back" | "browser_navigate_forward" | "browser_hover"
    )
}

// ---- Typed values ----

/// Whether a typed value is a payment card number. The value is looked at and dropped.
pub fn typed_value_is_card(value: &str) -> bool {
    super::gate::digits_hold_card(value)
}

// ---- The decision ----

/// One action as the gate sees it. Field values are passed only so a card number can be spotted;
/// nothing here is stored.
#[derive(Debug, Clone)]
pub struct Action<'a> {
    pub tool: &'a str,
    /// The tool's arguments (for the tab action, the key and the declared risk).
    pub args: &'a Value,
    /// The element action, for the tools that target one.
    pub kind: Option<ActionKind>,
    pub element: ElementSignals,
    pub form: FormSignals,
    pub page_url: &'a str,
    pub page_title: &'a str,
    pub page_heading: &'a str,
    pub page_vi: bool,
    /// Where a navigation or a new tab goes.
    pub target_url: Option<&'a str>,
    /// The value being typed, if any. Looked at for a card number, never stored.
    pub typed: Option<&'a str>,
    /// `declared_risk` of the call: can only add categories.
    pub declared: BTreeSet<Category>,
    /// Categories the extension found while preparing the action.
    pub ext: BTreeSet<Category>,
}

impl<'a> Action<'a> {
    pub fn new(tool: &'a str, args: &'a Value, page_url: &'a str) -> Self {
        Self {
            tool,
            args,
            kind: None,
            element: ElementSignals::default(),
            form: FormSignals::default(),
            page_url,
            page_title: "",
            page_heading: "",
            page_vi: false,
            target_url: None,
            typed: None,
            declared: declared_risk(tool, args),
            ext: BTreeSet::new(),
        }
    }
}

/// The categories the agent declared in a call. Unknown ids are ignored. For the agent's own
/// `browser_request_approval` the old category names map to the new ones; `other` has no match
/// and becomes `soul_rule` so the call still asks (the card copy is T15's).
pub fn declared_risk(tool: &str, args: &Value) -> BTreeSet<Category> {
    let mut out: BTreeSet<Category> = args["declared_risk"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().and_then(Category::parse))
        .collect();
    if tool == super::gate::APPROVAL_TOOL {
        out.insert(match args["category"].as_str().unwrap_or("") {
            "payment" => Category::Payment,
            "send_message" => Category::SendUnnamed,
            "post_publish" => Category::PostPublic,
            _ => Category::SoulRule,
        });
    }
    out
}

/// The Mono's settings as the gate reads them now.
#[derive(Debug, Clone)]
pub struct Config<'a> {
    pub level: BrowserLevel,
    /// The soul rules in force ([`super::soul_rules::reconcile`]).
    pub rules: &'a Parsed,
}

/// What the gate decided, and what it needs to write the audit row and to apply the answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    pub decision: Decision,
    pub tier: Tier,
    /// Every category found, whether or not a grant covers it.
    pub categories: BTreeSet<Category>,
    /// The registrable domain the action happens on (the key of grants).
    pub site: String,
    /// A short plain reason for the audit row.
    pub reason: String,
}

/// The user's answer to a card.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    AllowedOnce,
    /// "Allow in this conversation".
    AllowedTask,
    Denied,
    Timeout,
}

const MAX_TAINT_TOKENS: usize = 4000;
const MIN_TAINT_TOKEN: usize = 8;

/// What the gate remembers for one conversation of one Mono.
#[derive(Debug)]
pub struct TrustState {
    grants: GrantBook,
    user_messages: Vec<String>,
    /// When each of `user_messages` was written (ms), in step with it.
    user_message_ms: Vec<u64>,
    task_domains: BTreeSet<String>,
    /// Reached by a link and not approved: reading is free, any write asks (7.4).
    read_only_domains: BTreeSet<String>,
    /// Sensitive sites the user already answered a first-visit card for.
    seen_sensitive: BTreeSet<String>,
    /// Words read from pages in this run, with the domain they came from. Memory only.
    read_tokens: BTreeMap<String, String>,
    injection: bool,
}

impl TrustState {
    pub fn new(grants: GrantBook) -> Self {
        Self {
            grants,
            user_messages: Vec::new(),
            user_message_ms: Vec::new(),
            task_domains: BTreeSet::new(),
            read_only_domains: BTreeSet::new(),
            seen_sensitive: BTreeSet::new(),
            read_tokens: BTreeMap::new(),
            injection: false,
        }
    }

    pub fn grants(&self) -> &GrantBook {
        &self.grants
    }
    pub fn grants_mut(&mut self) -> &mut GrantBook {
        &mut self.grants
    }

    /// A new run of the conversation starts with the user's message. Task domains, user
    /// references and named recipients last for the whole conversation (decision 3); the taint and
    /// injection flag are per run. Grants are not touched: they last across runs.
    pub fn begin_run(&mut self, user_message: &str) {
        self.read_tokens.clear();
        self.injection = false;
        // A run that was not started by a message the user typed (an automation, another Mono)
        // brings no words of the user's own.
        if !user_message.trim().is_empty() {
            self.note_user_message(user_message);
        }
    }

    /// A message the user typed: its domains become task domains.
    pub fn note_user_message(&mut self, text: &str) {
        self.task_domains.extend(domains_in_text(text));
        self.user_messages.push(text.to_owned());
        self.user_message_ms.push(crate::time::now_ms());
    }

    /// What the user typed in this conversation, for the adopt reference (spec 9.3). With `after`
    /// only messages written after that time (ms) count: a tab the user took back needs words
    /// written after the stop (spec 10a).
    pub fn user_texts(&self, after: Option<u64>) -> Vec<(String, u64)> {
        self.user_messages
            .iter()
            .zip(&self.user_message_ms)
            .filter(|(_, ms)| after.is_none_or(|a| **ms > a))
            .map(|(text, ms)| (text.clone(), *ms))
            .collect()
    }

    /// A domain the user referenced (a tab they dragged into the group, an allowed origin).
    pub fn note_user_domain(&mut self, url_or_domain: &str) {
        let domain = registrable_domain_of(url_or_domain)
            .or_else(|| registrable_domain_of(&format!("https://{url_or_domain}")));
        if let Some(domain) = domain {
            self.read_only_domains.remove(&domain);
            self.task_domains.insert(domain);
        }
    }

    /// The Mono followed a real link to `url`: a new domain is read-only until the user approves.
    pub fn note_link_followed(&mut self, url: &str) {
        if let Some(domain) = registrable_domain_of(url) {
            if !self.task_domains.contains(&domain) {
                self.read_only_domains.insert(domain);
            }
        }
    }

    /// An injection pattern was found in a page of this run: every write asks.
    pub fn flag_injection(&mut self) {
        self.injection = true;
    }

    /// Remembers words of page text read at `url`, to spot them being sent to another site.
    pub fn note_read(&mut self, url: &str, text: &str) {
        let Some(domain) = registrable_domain_of(url) else {
            return;
        };
        for word in text.split(|c: char| c.is_whitespace() || "\"'<>()[],;".contains(c)) {
            if self.read_tokens.len() >= MAX_TAINT_TOKENS {
                break;
            }
            let word = word
                .trim_matches(|c: char| ".:!?".contains(c))
                .to_lowercase();
            let digits = word.chars().filter(char::is_ascii_digit).count();
            if word.chars().count() >= MIN_TAINT_TOKEN || digits >= 6 {
                self.read_tokens
                    .entry(word)
                    .or_insert_with(|| domain.clone());
            }
        }
    }

    /// Whether `text` (typed, or in a URL) repeats something read on a site other than `dest`.
    fn carries_read_data(&self, text: &str, dest: &str) -> bool {
        let text = text.to_lowercase();
        let digits: String = text.chars().filter(char::is_ascii_digit).collect();
        self.read_tokens.iter().any(|(token, from)| {
            if from == dest {
                return false;
            }
            if text.contains(token.as_str()) {
                return true;
            }
            let token_digits: String = token.chars().filter(char::is_ascii_digit).collect();
            token_digits.len() >= 6 && digits.contains(&token_digits)
        })
    }

    /// What a value the Mono supplies (a flow parameter) would be on `site`, whatever field it
    /// ends up in: a card number is credential entry, and something read on another site is
    /// tainted. The value is looked at and dropped.
    pub fn value_categories(&self, value: &str, site: &str) -> BTreeSet<Category> {
        let mut out = BTreeSet::new();
        if typed_value_is_card(value) {
            out.insert(Category::CredentialEntry);
        }
        if self.carries_read_data(value, site) {
            out.insert(Category::Tainted);
        }
        out
    }

    fn url_carries_read_data(&self, url: &str, dest: &str) -> bool {
        let Ok(parsed) = url::Url::parse(url) else {
            return false;
        };
        let mut carried = String::new();
        carried.push_str(parsed.path());
        for part in [parsed.query(), parsed.fragment()].into_iter().flatten() {
            carried.push(' ');
            carried.push_str(part);
        }
        let decoded: String = url::form_urlencoded::parse(carried.as_bytes())
            .map(|(k, v)| format!("{k} {v}"))
            .collect::<Vec<_>>()
            .join(" ");
        self.carries_read_data(&carried, dest) || self.carries_read_data(&decoded, dest)
    }

    fn task_context(&self, a: &Action<'_>) -> TaskContext {
        TaskContext {
            user_messages: self.user_messages.clone(),
            task_domains: self.task_domains.clone(),
            read_only_domains: self.read_only_domains.clone(),
            page_url: a.page_url.to_owned(),
            page_title: a.page_title.to_owned(),
            page_heading: a.page_heading.to_owned(),
            page_vi: a.page_vi,
        }
    }

    /// The decision for one call (spec 7.6).
    pub fn evaluate(&self, cfg: &Config<'_>, a: &Action<'_>) -> Evaluation {
        let tier = tier_of(a.tool, a.args);
        let task = self.task_context(a);
        let on_url = a.target_url.unwrap_or(a.page_url);
        let site = registrable_domain_of(on_url).unwrap_or_default();
        if tier == Tier::Refused {
            return Evaluation {
                decision: Decision::Deny(format!("{} is not available in this browser.", a.tool)),
                tier,
                categories: BTreeSet::new(),
                site,
                reason: "tool refused".into(),
            };
        }

        let mut gate: BTreeSet<Category> = BTreeSet::new();
        let mut blocked: Option<String> = None;
        let mut kind = FactKind::Other;
        let mut classification = None;
        let first_visit =
            |url: &str| site_category(url).is_some() && !self.seen_sensitive.contains(&site);

        if let Some(target) = a.target_url {
            kind = FactKind::Navigate;
            gate.extend(navigation_category(&task, target));
            if first_visit(target) {
                gate.insert(Category::SensitiveSite);
            }
            // Data read from a page and put in an address goes out to another site.
            if self.url_carries_read_data(target, &site) {
                gate.insert(Category::Tainted);
            }
        } else if reads_page(a.tool) && first_visit(a.page_url) {
            gate.insert(Category::SensitiveSite);
        }

        if let Some(action) = a.kind {
            kind = match action {
                ActionKind::Click => FactKind::Click,
                ActionKind::Type(_) => FactKind::Type,
                ActionKind::Select => FactKind::Select,
                ActionKind::Upload => FactKind::Upload,
            };
            let outcome = gate_categories(action, &a.element, &a.form, &task);
            gate.extend(outcome.categories);
            if let Some(reason) = outcome.blocked {
                blocked = Some(reason.to_owned());
            }
            if matches!(action, ActionKind::Click | ActionKind::Select) {
                classification = Some(element_classification(&a.element, &task));
            }
            if let Some(value) = a.typed {
                if typed_value_is_card(value) {
                    gate.insert(Category::CredentialEntry);
                }
                if self.carries_read_data(value, &site) {
                    gate.insert(Category::Tainted);
                }
            }
        }
        if a.tool == "browser_file_upload" || a.kind == Some(ActionKind::Upload) {
            kind = FactKind::Upload;
            gate.insert(Category::Upload);
        }

        let writes = tier != Tier::Read && !is_non_writing(a.tool);
        if self.injection && tier != Tier::Read {
            gate.insert(Category::Injection);
        }

        // Soul rules (7.5).
        let (send, post) = classification
            .as_ref()
            .map_or((false, false), |c| (c.send, c.post));
        let element_text = format!(
            "{} {} {}",
            a.element.name,
            a.element.text,
            a.element.label.as_deref().unwrap_or("")
        );
        let mut soul: BTreeSet<Category> = BTreeSet::new();
        let mut reason = String::new();
        if tier != Tier::Read {
            let facts = ActionFacts {
                kind,
                element_text: &element_text,
                url: on_url,
                send,
                post,
                categories: &gate,
            };
            let matched = cfg.rules.matching(&facts);
            if let Some(line) = matched.blocked {
                blocked.get_or_insert(format!("blocked by your browser rules: {line}"));
            }
            if matched.ask {
                soul.insert(Category::SoulRule);
                reason = format!("soul rule: {}", matched.lines.join("; "));
            }
            if cfg.rules.fail_closed() && writes {
                soul.insert(Category::SoulRule);
                if reason.is_empty() {
                    reason = "the browser rules have an error".into();
                }
            }
        }

        // A tier K action always asks: a grant never covers it.
        let covered = if tier == Tier::Always {
            BTreeSet::new()
        } else {
            self.grants.covered(&site)
        };
        let input = DecisionInput {
            blocked: blocked.as_deref(),
            read_only_level: cfg.level == BrowserLevel::ReadOnly,
            tool_read_ok: read_only_ok(a.tool, a.args),
            gate: &gate,
            declared: &a.declared,
            ext: &a.ext,
            soul: &soul,
            grants: &covered,
        };
        let decision = decide(&input);
        let categories = effective(&input);
        if reason.is_empty() {
            reason = match &decision {
                Decision::Allow => "no risk found".into(),
                Decision::AllowByGrant(_) => "covered by a grant".into(),
                Decision::Ask(_) => "risk found".into(),
                Decision::Deny(why) => why.clone(),
            };
        }
        Evaluation {
            decision,
            tier,
            categories,
            site,
            reason,
        }
    }

    /// Applies the user's answer to an evaluation: a first visit to a sensitive site is not asked
    /// again, an approved unrelated domain becomes a task domain, and "Allow in this conversation"
    /// records a grant (never for payment or credential entry, never for a tier K tool). Returns
    /// the categories granted.
    pub fn commit(&mut self, evaluation: &Evaluation, answer: Answer) -> BTreeSet<Category> {
        if !matches!(answer, Answer::AllowedOnce | Answer::AllowedTask) {
            return BTreeSet::new();
        }
        if evaluation.categories.contains(&Category::SensitiveSite) {
            self.seen_sensitive.insert(evaluation.site.clone());
        }
        if evaluation.categories.contains(&Category::UnrelatedDomain) {
            self.read_only_domains.remove(&evaluation.site);
            self.task_domains.insert(evaluation.site.clone());
        }
        if answer == Answer::AllowedTask && evaluation.tier != Tier::Always {
            let grantable: BTreeSet<Category> = evaluation
                .categories
                .iter()
                .copied()
                .filter(|c| !never_granted(*c))
                .collect();
            return self.grants.grant(&evaluation.site, &grantable);
        }
        BTreeSet::new()
    }

    /// The overlay's Stop, or the user taking the task back, for one site: the Mono's grants for
    /// it end in every conversation.
    pub fn take_back(&mut self, site: &str) {
        self.grants.end_site_everywhere(site);
    }

    /// The conversation closed or was cleared: every grant of it ends, and so does the memory of
    /// what the user approved.
    pub fn end_conversation(&mut self) {
        self.grants.end_conversation();
        self.seen_sensitive.clear();
        self.task_domains.clear();
        self.read_only_domains.clear();
        self.user_messages.clear();
        self.user_message_ms.clear();
        self.read_tokens.clear();
        self.injection = false;
    }
}

#[cfg(test)]
mod tests;
