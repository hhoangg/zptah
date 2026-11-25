//! Signals (spec 7.2), site data (7.4) and the table that turns signals into categories (7.3,
//! "How signals become categories").
//!
//! The extension computes [`ElementSignals`] and [`FormSignals`] and never sends field values.
//! The gate adds what only it knows: the user's messages of the task, the task domains and the
//! page it is on ([`TaskContext`]).

use super::lexicon::{lexicon, Classification, ControlInput, ControlKind, LinkContext};
use super::normalize::normalize;
use super::Category;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

const SITES_JSON: &str =
    include_str!("../../../../../../extensions/zptah-browser/shared/risk-sites.json");

/// What a form field is for (spec 7.2). Field values are never read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    Password,
    Otp,
    CardNumber,
    CardCvc,
    CardExpiry,
    BankAccount,
    Amount,
    NationalId,
    Email,
    File,
    TermsCheckbox,
    Other,
}

impl FieldKind {
    pub fn parse(s: &str) -> Option<FieldKind> {
        Some(match s {
            "password" => FieldKind::Password,
            "otp" => FieldKind::Otp,
            "card_number" => FieldKind::CardNumber,
            "card_cvc" => FieldKind::CardCvc,
            "card_expiry" => FieldKind::CardExpiry,
            "bank_account" => FieldKind::BankAccount,
            "amount" => FieldKind::Amount,
            "national_id" => FieldKind::NationalId,
            "email" => FieldKind::Email,
            "file" => FieldKind::File,
            "terms_checkbox" => FieldKind::TermsCheckbox,
            "other" => FieldKind::Other,
            _ => return None,
        })
    }
    fn is_card(self) -> bool {
        matches!(
            self,
            FieldKind::CardNumber | FieldKind::CardCvc | FieldKind::CardExpiry
        )
    }
}

/// Signals of the element an action targets (spec 7.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ElementSignals {
    pub role: String,
    /// Accessible name, at most 200 chars.
    pub name: String,
    pub text: String,
    pub tag: String,
    #[serde(rename = "type")]
    pub input_type: Option<String>,
    pub autocomplete: Option<String>,
    pub input_name: Option<String>,
    pub input_id: Option<String>,
    pub placeholder: Option<String>,
    pub label: Option<String>,
    pub href: Option<String>,
    pub target: Option<String>,
    pub form_action: Option<String>,
    pub form_method: Option<String>,
    pub is_submit: bool,
    pub in_dialog: bool,
    pub dialog_text: Option<String>,
    /// The dialog has a primary button besides this element (the extension computes it).
    pub dialog_has_primary: bool,
    /// Names of the dialog's other primary buttons.
    pub dialog_primary_names: Vec<String>,
    /// Ids and classes of the dialog (consent-manager detection).
    pub dialog_markers: Option<String>,
    /// Inside `nav`, `footer` or `role=menu` (the extension computes it).
    pub in_landmark_menu: bool,
    pub nearby_text: String,
    pub amounts: Vec<String>,
}

/// One field of the form around the element.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormField {
    pub kind: FieldKind,
    pub label: String,
    pub filled: bool,
}

/// Signals of the form around the element (spec 7.2).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FormSignals {
    pub fields: Vec<FormField>,
    /// To, Cc, Bcc chips and email inputs, for send actions.
    pub recipients: Vec<String>,
    /// Heading of the open chat or thread.
    pub conversation_title: Option<String>,
    /// Selected value of a visibility or audience control.
    pub visibility: Option<String>,
}

/// What the action does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Click,
    /// Typing into a field of this kind.
    Type(FieldKind),
    Select,
    Upload,
}

/// What only the gate knows about the task and the page.
#[derive(Debug, Clone, Default)]
pub struct TaskContext {
    /// The user's messages of the task (and conversation).
    pub user_messages: Vec<String>,
    /// Registrable domains of the task (7.4).
    pub task_domains: BTreeSet<String>,
    /// Registrable domains reached by a link and not yet approved: any write asks.
    pub read_only_domains: BTreeSet<String>,
    pub page_url: String,
    pub page_title: String,
    pub page_heading: String,
    pub page_vi: bool,
}

/// The categories of an action and whether it is blocked outright.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GateOutcome {
    pub categories: BTreeSet<Category>,
    /// A hard stop with its reason (typing into a password field, spec 8.6).
    pub blocked: Option<&'static str>,
}

// ---------------------------------------------------------------------------------------------
// Sites (7.4)
// ---------------------------------------------------------------------------------------------

/// A site category of `risk-sites.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteCategory {
    Banking,
    Brokerage,
    Crypto,
    Wallet,
    Government,
}

/// The parsed `risk-sites.json`.
#[derive(Debug, Deserialize)]
pub struct Sites {
    categories: BTreeMap<SiteCategory, Vec<String>>,
    pub checkout_segments: Vec<String>,
    pub action_segments: Vec<String>,
    registrable_suffixes: Vec<String>,
}

pub fn sites() -> &'static Sites {
    static CELL: OnceLock<Sites> = OnceLock::new();
    CELL.get_or_init(|| serde_json::from_str(SITES_JSON).expect("shared/risk-sites.json is valid"))
}

pub(crate) fn host_of(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()?
        .host_str()
        .map(|h| h.trim_end_matches('.').to_lowercase())
}

impl Sites {
    /// The category of a host: a listed domain covers its subdomains.
    pub fn category_of_host(&self, host: &str) -> Option<SiteCategory> {
        let host = host.trim_end_matches('.').to_lowercase();
        self.categories.iter().find_map(|(cat, domains)| {
            domains
                .iter()
                .any(|d| host == *d || host.ends_with(&format!(".{d}")))
                .then_some(*cat)
        })
    }

    /// The registrable domain of a host (`www.shop.example.com.vn` is `example.com.vn`).
    pub fn registrable_domain(&self, host: &str) -> String {
        let host = host.trim_end_matches('.').to_lowercase();
        let labels: Vec<&str> = host.split('.').collect();
        if labels.len() <= 2 || host.parse::<std::net::IpAddr>().is_ok() {
            return host;
        }
        let last_two = labels[labels.len() - 2..].join(".");
        let take = if self.registrable_suffixes.contains(&last_two) {
            3
        } else {
            2
        };
        labels[labels.len() - take..].join(".")
    }
}

/// The site category of a URL, if it is a sensitive site.
pub fn site_category(url: &str) -> Option<SiteCategory> {
    sites().category_of_host(&host_of(url)?)
}

/// Whether a URL path has a checkout segment (`/checkout`, `/cart/checkout`, `/thanh-toan`).
pub fn is_checkout_path(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let s = sites();
    parsed.path_segments().is_some_and(|mut segs| {
        segs.any(|seg| {
            let seg = seg.to_lowercase();
            s.checkout_segments.contains(&seg)
        })
    })
}

/// The registrable domain of a URL.
pub fn registrable_domain_of(url: &str) -> Option<String> {
    Some(sites().registrable_domain(&host_of(url)?))
}

fn domain_regex() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(r"(?i)\b(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+[a-z]{2,}\b")
            .expect("domain regex is valid")
    })
}

/// The registrable domains the user named in a text (a URL or a bare host).
pub fn domains_in_text(text: &str) -> BTreeSet<String> {
    domain_regex()
        .find_iter(text)
        .map(|m| sites().registrable_domain(m.as_str()))
        .collect()
}

/// Navigating to a domain outside the task domains asks (`unrelated_domain`, 7.4).
pub fn navigation_category(task: &TaskContext, target_url: &str) -> Option<Category> {
    let domain = registrable_domain_of(target_url)?;
    (!task.task_domains.contains(&domain)).then_some(Category::UnrelatedDomain)
}

// ---------------------------------------------------------------------------------------------
// Amounts and fields
// ---------------------------------------------------------------------------------------------

fn amount_regex() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(r"(?i)[$€£₫]\s?\d[\d.,]*|\d[\d.,]*\s?(?:₫|[$€£]|đ|vnd|vnđ|usd|eur|gbp)")
            .expect("amount regex is valid")
    })
}

/// Money patterns in a text (`₫189,000`, `500.000đ`, `$20`, `12 USD`).
pub fn extract_amounts(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for m in amount_regex().find_iter(text) {
        let ends_with_letter = m.as_str().chars().last().is_some_and(char::is_alphabetic);
        let next_is_word = text[m.end()..]
            .chars()
            .next()
            .is_some_and(char::is_alphanumeric);
        if ends_with_letter && next_is_word {
            continue;
        }
        out.push(m.as_str().trim().to_owned());
    }
    out
}

/// The attributes the field classifier reads. Values are never read.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FieldAttrs {
    #[serde(rename = "type")]
    pub input_type: String,
    pub autocomplete: String,
    pub name: String,
    pub id: String,
    pub label: String,
    pub placeholder: String,
    pub inputmode: String,
    pub maxlength: Option<u32>,
    /// Text around the field, for the numeric OTP heuristic.
    pub nearby_text: String,
}

const FIELD_PRIORITY: [FieldKind; 8] = [
    FieldKind::Password,
    FieldKind::Otp,
    FieldKind::CardNumber,
    FieldKind::CardCvc,
    FieldKind::CardExpiry,
    FieldKind::BankAccount,
    FieldKind::NationalId,
    FieldKind::Amount,
];

/// The kind of a form field from its attributes (spec 7.3, form-field signals).
pub fn classify_field(a: &FieldAttrs) -> FieldKind {
    let lex = lexicon();
    let ty = a.input_type.to_lowercase();
    let ac = a.autocomplete.to_lowercase();
    let ac_has = |t: &str| ac.split_whitespace().any(|x| x == t || x.starts_with(t));
    if ac_has("one-time-code") {
        return FieldKind::Otp;
    }
    if ac_has("current-password") || ac_has("new-password") || ty == "password" {
        return FieldKind::Password;
    }
    if ac_has("cc-number") {
        return FieldKind::CardNumber;
    }
    if ac_has("cc-csc") {
        return FieldKind::CardCvc;
    }
    if ac_has("cc-exp") {
        return FieldKind::CardExpiry;
    }
    if ty == "file" {
        return FieldKind::File;
    }
    let found: BTreeSet<FieldKind> = [&a.name, &a.id, &a.label, &a.placeholder]
        .into_iter()
        .filter_map(|t| lex.field_kind_of(t))
        .collect();
    if let Some(kind) = FIELD_PRIORITY.iter().find(|k| found.contains(k)) {
        return *kind;
    }
    let numeric_short = a.inputmode.eq_ignore_ascii_case("numeric")
        && a.maxlength.is_some_and(|n| (4..=8).contains(&n));
    if numeric_short && lex.field_kind_of(&a.nearby_text) == Some(FieldKind::Otp) {
        return FieldKind::Otp;
    }
    if ty == "checkbox"
        && [&a.label, &a.name]
            .into_iter()
            .any(|t| lex.text_has_phrase_class(t, false, Category::Terms))
    {
        return FieldKind::TermsCheckbox;
    }
    if ty == "email" || ac_has("email") {
        return FieldKind::Email;
    }
    FieldKind::Other
}

// ---------------------------------------------------------------------------------------------
// Recipients
// ---------------------------------------------------------------------------------------------

fn email_regex() -> &'static Regex {
    static CELL: OnceLock<Regex> = OnceLock::new();
    CELL.get_or_init(|| {
        Regex::new(r"(?i)[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}").expect("email regex is valid")
    })
}

/// Whether a recipient (a name, an address or `Name <address>`) is one the user named.
pub fn recipient_named(recipient: &str, user_messages: &[String]) -> bool {
    let joined = user_messages.join("\n");
    let lower = joined.to_lowercase();
    let mut name_part = recipient.to_owned();
    if let Some(m) = email_regex().find(recipient) {
        if lower.contains(&m.as_str().to_lowercase()) {
            return true;
        }
        name_part = recipient.replace(m.as_str(), " ");
    }
    let wanted = normalize(&name_part.replace(['<', '>', '"'], " "));
    if wanted.is_empty() {
        return false;
    }
    let hay = normalize(&joined);
    let plain = !(wanted.has_diacritics && hay.has_diacritics) || !wanted.has_diacritics;
    let words = |n: &super::normalize::Norm| -> Vec<String> {
        n.segments
            .iter()
            .flatten()
            .map(|w| {
                if plain {
                    w.plain.clone()
                } else {
                    w.acc.clone()
                }
            })
            .collect()
    };
    let (w, h) = (words(&wanted), words(&hay));
    !w.is_empty() && h.windows(w.len()).any(|win| win == w.as_slice())
}

// ---------------------------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------------------------

fn is_write_click(el: &ElementSignals) -> bool {
    !is_real_link(el)
}

/// A real link: it has a non-empty `href` and is not exposed as a button.
fn is_real_link(el: &ElementSignals) -> bool {
    el.href.as_deref().is_some_and(|h| !h.trim().is_empty())
        && !el.is_submit
        && !el.role.eq_ignore_ascii_case("button")
}

fn control_input(el: &ElementSignals, name: &str, task: &TaskContext) -> ControlInput {
    let kind = if is_real_link(el) {
        ControlKind::Link
    } else if el.is_submit {
        ControlKind::Submit
    } else {
        ControlKind::Button
    };
    ControlInput {
        name: name.to_owned(),
        kind,
        page_vi: task.page_vi,
        in_dialog: el.in_dialog,
        dialog_text: el.dialog_text.clone().unwrap_or_default(),
        dialog_has_primary: el.dialog_has_primary,
        dialog_primary_names: el.dialog_primary_names.clone(),
        dialog_markers: el.dialog_markers.clone().unwrap_or_default(),
        role: el.role.clone(),
        nearby_text: el.nearby_text.clone(),
        page_heading: task.page_heading.clone(),
        page_title: task.page_title.clone(),
        link: el.href.as_ref().map(|href| LinkContext {
            href: href.clone(),
            page_url: task.page_url.clone(),
            in_nav_footer_menu: el.in_landmark_menu,
        }),
    }
}

/// Classifies the accessible name and the visible text separately (an `aria-label` can say
/// something harmless over a button that shows "Pay now") and unions the results.
fn classify_element(el: &ElementSignals, task: &TaskContext) -> (ControlInput, Classification) {
    let lex = lexicon();
    let first = if el.name.trim().is_empty() {
        &el.text
    } else {
        &el.name
    };
    let input = control_input(el, first, task);
    let mut class = lex.classify_control(&input);
    let text = el.text.trim();
    if !el.name.trim().is_empty() && !text.is_empty() && text != el.name.trim() {
        let second = lex.classify_control(&control_input(el, text, task));
        class.categories.extend(second.categories);
        class.send |= second.send;
        class.post |= second.post;
        class.consent &= second.consent;
        class.dismiss &= second.dismiss;
        class.not_risky &= second.not_risky;
        class.informational &= second.informational;
    }
    (input, class)
}

/// What the lexicon says about the element alone (send, post and the categories of its name), for
/// the soul rules that match on "send" and "post".
pub fn element_classification(el: &ElementSignals, task: &TaskContext) -> Classification {
    classify_element(el, task).1
}

fn send_is_unnamed(form: &FormSignals, task: &TaskContext) -> bool {
    if !form.recipients.is_empty() {
        return form
            .recipients
            .iter()
            .any(|r| !recipient_named(r, &task.user_messages));
    }
    match form
        .conversation_title
        .as_deref()
        .filter(|t| !t.trim().is_empty())
    {
        Some(title) => !recipient_named(title, &task.user_messages),
        // No recipient and no conversation: fail closed.
        None => true,
    }
}

/// Turns signals into categories (spec 7.3, "How signals become categories") plus the site
/// signals of 7.4 for writes. The gate unions the result with declared, extension, soul and taint
/// categories (7.6).
pub fn gate_categories(
    action: ActionKind,
    el: &ElementSignals,
    form: &FormSignals,
    task: &TaskContext,
) -> GateOutcome {
    let lex = lexicon();
    let mut out = GateOutcome::default();
    let cats = &mut out.categories;
    let on_checkout = is_checkout_path(&task.page_url);
    let sensitive = site_category(&task.page_url).is_some();
    let mut write = true;
    match action {
        ActionKind::Type(FieldKind::Password) => {
            out.blocked = Some("typing into a password field is never allowed");
        }
        ActionKind::Type(k) => {
            if matches!(k, FieldKind::Otp | FieldKind::NationalId) || k.is_card() {
                cats.insert(Category::CredentialEntry);
            }
        }
        ActionKind::Upload => {
            cats.insert(Category::Upload);
        }
        ActionKind::Click | ActionKind::Select => {
            let (input, class) = classify_element(el, task);
            cats.extend(class.categories.iter().copied());
            let submit_like = el.is_submit || input.kind == ControlKind::Button;
            let has_money_field = form.fields.iter().any(|f| {
                f.kind.is_card() || matches!(f.kind, FieldKind::BankAccount | FieldKind::Amount)
            });
            if (el.is_submit && has_money_field)
                || (submit_like && !el.amounts.is_empty() && on_checkout)
            {
                cats.insert(Category::Payment);
            }
            let label = el.label.as_deref().unwrap_or(&el.name);
            if el.input_type.as_deref() == Some("checkbox")
                && lex.text_has_phrase_class(label, task.page_vi, Category::Terms)
            {
                cats.insert(Category::Terms);
            }
            if class.send && send_is_unnamed(form, task) {
                cats.insert(Category::SendUnnamed);
            }
            let public = form
                .visibility
                .as_deref()
                .is_some_and(|v| lex.is_public_visibility(v));
            if class.post || (public && (el.is_submit || class.send)) {
                cats.insert(Category::PostPublic);
            }
            let changes_control = action == ActionKind::Select
                || matches!(
                    el.role.as_str(),
                    "combobox" | "radio" | "switch" | "menuitemradio" | "option" | "listbox"
                );
            let control_label = [&el.name, &el.label.clone().unwrap_or_default()]
                .into_iter()
                .any(|t| lex.is_visibility_control(t));
            if changes_control && control_label {
                cats.insert(Category::Sharing);
            }
            if el.name.trim().is_empty()
                && el.text.trim().is_empty()
                && (sensitive || on_checkout)
                && !class.consent
            {
                cats.insert(Category::Payment);
            }
            write = is_write_click(el) || action == ActionKind::Select;
        }
    }
    if write {
        if sensitive {
            cats.insert(Category::SensitiveSite);
        }
        if registrable_domain_of(&task.page_url)
            .is_some_and(|d| task.read_only_domains.contains(&d))
        {
            cats.insert(Category::UnrelatedDomain);
        }
    }
    out
}
