//! The words of the Browser approval card (spec 8.2, 8.3), in English and Vietnamese.
//!
//! The copy follows the app UI language ([`app_language`]): Vietnamese when the UI is Vietnamese,
//! English otherwise. Everything here is plain language: no JSON, selectors or refs. What the
//! page said (an element name, a dialog text, a recipient) is quoted and clipped; what the Mono
//! said (`purpose`, the words of `browser_request_approval`) sits on its own line, labelled as the
//! Mono's words, so the user can tell what zptah found from what the agent claims.
//!
//! The card is described by [`CardFacts`] and rendered by [`card`]. The first sentence names the
//! action and the first (most serious) category; every further category adds one short line.

use super::risk::signals::{FieldKind, SiteCategory};
use super::risk::Category;
use std::sync::atomic::{AtomicU8, Ordering};

/// The language of the copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    En,
    Vi,
}

impl Lang {
    pub fn id(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Vi => "vi",
        }
    }

    /// The language of a locale tag such as `vi`, `vi_VN.UTF-8` or `en-US`: Vietnamese for `vi`,
    /// English for everything else.
    pub fn from_locale(tag: &str) -> Lang {
        let tag = tag.trim().to_ascii_lowercase();
        if tag == "vi" || tag.starts_with("vi-") || tag.starts_with("vi_") || tag.starts_with("vi.")
        {
            Lang::Vi
        } else {
            Lang::En
        }
    }

    /// The system language from the usual environment variables (the GUI passes the macOS
    /// language itself, see [`set_app_language`]).
    pub fn system() -> Lang {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .filter_map(|name| std::env::var(name).ok())
            .find(|value| !value.trim().is_empty())
            .map(|value| Lang::from_locale(&value))
            .unwrap_or(Lang::En)
    }
}

static APP_LANGUAGE: AtomicU8 = AtomicU8::new(0);

/// The app UI language the cards are written in. English until [`set_app_language`] is called.
pub fn app_language() -> Lang {
    match APP_LANGUAGE.load(Ordering::Relaxed) {
        1 => Lang::Vi,
        _ => Lang::En,
    }
}

/// Sets the app UI language (the settings screen, or [`Lang::system`] at start).
pub fn set_app_language(lang: Lang) {
    APP_LANGUAGE.store(
        match lang {
            Lang::En => 0,
            Lang::Vi => 1,
        },
        Ordering::Relaxed,
    );
}

/// What the Mono wants to do, as the card words it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionDesc {
    Click {
        label: String,
    },
    Type {
        label: String,
    },
    Select {
        label: String,
    },
    Press {
        key: String,
    },
    Drag,
    Hover,
    Scroll,
    /// Upload files from the user's computer.
    Upload {
        names: Vec<String>,
    },
    /// Tick (`on`) or untick a checkbox, radio or switch.
    Check {
        label: String,
        on: bool,
    },
    /// Open an address (navigation, a tab).
    Navigate,
    /// Answer a JavaScript dialog.
    Dialog {
        accept: bool,
    },
    /// Take over a tab another Mono owns (spec 9.3 "tab transfer").
    TabTransfer {
        title: String,
        from: String,
    },
    /// An action the Mono described itself with `browser_request_approval`.
    Other,
}

/// What the card needs to know. All text is what the page or the Mono said: it is clipped and
/// stripped of control characters when rendered.
#[derive(Debug, Clone)]
pub struct CardFacts {
    /// The Mono's name (`Mono::label()`).
    pub mono: String,
    /// The registrable domain the action happens on.
    pub site: String,
    /// The full address, shown for an unrelated domain.
    pub url: String,
    pub action: ActionDesc,
    /// Every category found, in any order.
    pub categories: Vec<Category>,
    /// The action is inside a dialog and this is what the dialog says.
    pub in_dialog: bool,
    pub dialog_text: Option<String>,
    pub recipients: Vec<String>,
    pub amounts: Vec<String>,
    /// The kind of field being typed into (credential entry).
    pub field: Option<FieldKind>,
    /// The kind of site (sensitive site).
    pub site_kind: Option<SiteCategory>,
    /// The rule of the Mono's soul that matched, if any.
    pub rule: Option<String>,
    /// The Mono's own words from `browser_request_approval`: what it wants to do, and details.
    pub agent_action: Option<String>,
    pub agent_details: Option<String>,
    /// `purpose` of the call: the Mono's claim, shown apart.
    pub purpose: Option<String>,
}

impl CardFacts {
    pub fn new(mono: &str, site: &str, action: ActionDesc, categories: Vec<Category>) -> Self {
        Self {
            mono: mono.to_owned(),
            site: site.to_owned(),
            url: String::new(),
            action,
            categories,
            in_dialog: false,
            dialog_text: None,
            recipients: Vec::new(),
            amounts: Vec::new(),
            field: None,
            site_kind: None,
            rule: None,
            agent_action: None,
            agent_details: None,
            purpose: None,
        }
    }
}

/// A rendered card: the first sentence (shown bold), the lines under it, the Mono's words, and the
/// labels of the buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardText {
    pub headline: String,
    pub lines: Vec<String>,
    /// `Mira says: "..."`, apart from the facts.
    pub purpose: Option<String>,
    pub allow_once: &'static str,
    /// `None` when the answer is not offered (payment and credential entry are never granted).
    pub allow_conversation: Option<&'static str>,
    pub deny: &'static str,
    pub show_tab: &'static str,
}

const MAX_LABEL: usize = 80;
const MAX_QUOTE: usize = 200;

/// One line of plain text, clipped to `max` characters (with an ellipsis when cut).
pub fn clean(text: &str, max: usize) -> String {
    let flat: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= max {
        flat
    } else {
        let cut: String = flat.chars().take(max.saturating_sub(1)).collect();
        format!("{}\u{2026}", cut.trim_end())
    }
}

fn quoted(text: &str, max: usize) -> String {
    format!("\"{}\"", clean(text, max).replace('"', "'"))
}

/// The category the first sentence is about: the most serious one.
fn primary(categories: &[Category]) -> Option<Category> {
    ordered(categories).first().copied()
}

/// The categories present, most serious first.
fn ordered(categories: &[Category]) -> Vec<Category> {
    const ORDER: [Category; 15] = [
        Category::Payment,
        Category::Destructive,
        Category::AccountSecurity,
        Category::SendUnnamed,
        Category::PostPublic,
        Category::Sharing,
        Category::Upload,
        Category::CredentialEntry,
        Category::Terms,
        Category::Injection,
        Category::Tainted,
        Category::UnrelatedDomain,
        Category::SensitiveSite,
        Category::Unclear,
        Category::SoulRule,
    ];
    ORDER
        .into_iter()
        .filter(|c| categories.contains(c))
        .collect()
}

/// The rule as the user wrote it, without its `ask:` verb.
fn rule_body(rule: &str) -> String {
    let rule = rule.trim();
    let body = rule
        .split_once(':')
        .filter(|(verb, _)| verb.trim().eq_ignore_ascii_case("ask"))
        .map_or(rule, |(_, rest)| rest);
    clean(body, MAX_QUOTE)
}

fn join(items: &[String], max_items: usize) -> String {
    let shown: Vec<String> = items
        .iter()
        .take(max_items)
        .map(|i| clean(i, 80))
        .filter(|i| !i.is_empty())
        .collect();
    shown.join(", ")
}

/// The sentence after "wants to" for the action, without the site.
fn verb_phrase(lang: Lang, facts: &CardFacts) -> String {
    let label = |l: &str| quoted(l, MAX_LABEL);
    let dialog = facts.in_dialog && facts.dialog_text.is_some();
    if matches!(facts.action, ActionDesc::Click { .. })
        && facts.categories.contains(&Category::SendUnnamed)
        && !facts.recipients.is_empty()
    {
        let to = join(&facts.recipients, 3);
        return match lang {
            Lang::En => format!("send a message to {to}"),
            Lang::Vi => format!("gửi tin nhắn tới {to}"),
        };
    }
    match (&facts.action, lang) {
        (ActionDesc::Click { label: l }, Lang::En) => {
            if dialog {
                format!("click {} in a dialog", label(l))
            } else {
                format!("click {}", label(l))
            }
        }
        (ActionDesc::Click { label: l }, Lang::Vi) => {
            if dialog {
                format!("bấm {} trong hộp thoại", label(l))
            } else {
                format!("bấm {}", label(l))
            }
        }
        (ActionDesc::Type { label: l }, Lang::En) => match field_noun(lang, facts.field) {
            Some(noun) => format!("type {noun}"),
            None => format!("type into {}", label(l)),
        },
        (ActionDesc::Type { label: l }, Lang::Vi) => match field_noun(lang, facts.field) {
            Some(noun) => format!("nhập {noun}"),
            None => format!("nhập vào {}", label(l)),
        },
        (ActionDesc::Check { label: l, on }, Lang::En) => {
            format!("{} {}", if *on { "check" } else { "uncheck" }, label(l))
        }
        (ActionDesc::Check { label: l, on }, Lang::Vi) => {
            format!("{} {}", if *on { "chọn" } else { "bỏ chọn" }, label(l))
        }
        (ActionDesc::Select { label: l }, Lang::En) => format!("change {}", label(l)),
        (ActionDesc::Select { label: l }, Lang::Vi) => format!("chọn ở {}", label(l)),
        (ActionDesc::Press { key }, Lang::En) => format!("press the {} key", clean(key, 32)),
        (ActionDesc::Press { key }, Lang::Vi) => format!("nhấn phím {}", clean(key, 32)),
        (ActionDesc::Drag, Lang::En) => "drag an item".into(),
        (ActionDesc::Drag, Lang::Vi) => "kéo một mục".into(),
        (ActionDesc::Hover, Lang::En) => "point at an item".into(),
        (ActionDesc::Hover, Lang::Vi) => "trỏ vào một mục".into(),
        (ActionDesc::Scroll, Lang::En) => "scroll the page".into(),
        (ActionDesc::Scroll, Lang::Vi) => "cuộn trang".into(),
        (ActionDesc::Upload { names }, Lang::En) => {
            format!("upload {} from your computer", file_list(names))
        }
        (ActionDesc::Upload { names }, Lang::Vi) => {
            format!("tải {} từ máy bạn lên", file_list(names))
        }
        (ActionDesc::Navigate, Lang::En) => "open".into(),
        (ActionDesc::Navigate, Lang::Vi) => "mở".into(),
        (ActionDesc::Dialog { accept }, Lang::En) => {
            if *accept {
                "accept a dialog".into()
            } else {
                "dismiss a dialog".into()
            }
        }
        (ActionDesc::Dialog { accept }, Lang::Vi) => {
            if *accept {
                "bấm Đồng ý trong hộp thoại".into()
            } else {
                "bấm Hủy trong hộp thoại".into()
            }
        }
        (ActionDesc::TabTransfer { title, from }, Lang::En) => format!(
            "take over the tab {} from {}",
            quoted(title, MAX_LABEL),
            clean(from, MAX_LABEL)
        ),
        (ActionDesc::TabTransfer { title, from }, Lang::Vi) => format!(
            "tiếp quản thẻ {} từ {}",
            quoted(title, MAX_LABEL),
            clean(from, MAX_LABEL)
        ),
        (ActionDesc::Other, Lang::En) => "do something that needs your OK".into(),
        (ActionDesc::Other, Lang::Vi) => "làm một việc cần bạn đồng ý".into(),
    }
}

fn file_list(names: &[String]) -> String {
    match names {
        [] => "a file".into(),
        [one] => quoted(one, MAX_LABEL),
        many => {
            let shown: Vec<String> = many.iter().take(3).map(|n| quoted(n, MAX_LABEL)).collect();
            let more = many.len().saturating_sub(3);
            if more > 0 {
                format!("{} and {more} more", shown.join(", "))
            } else {
                shown.join(", ")
            }
        }
    }
}

fn field_noun(lang: Lang, field: Option<FieldKind>) -> Option<&'static str> {
    Some(match (field?, lang) {
        (FieldKind::Otp, Lang::En) => "a one-time code",
        (FieldKind::Otp, Lang::Vi) => "mã OTP",
        (FieldKind::CardNumber | FieldKind::CardCvc | FieldKind::CardExpiry, Lang::En) => {
            "your card details"
        }
        (FieldKind::CardNumber | FieldKind::CardCvc | FieldKind::CardExpiry, Lang::Vi) => {
            "thông tin thẻ của bạn"
        }
        (FieldKind::NationalId, Lang::En) => "your ID number",
        (FieldKind::NationalId, Lang::Vi) => "số căn cước của bạn",
        (FieldKind::BankAccount, Lang::En) => "a bank account number",
        (FieldKind::BankAccount, Lang::Vi) => "số tài khoản ngân hàng",
        _ => return None,
    })
}

fn site_kind_noun(lang: Lang, kind: Option<SiteCategory>) -> &'static str {
    match (kind, lang) {
        (Some(SiteCategory::Banking), Lang::En) => "a banking site",
        (Some(SiteCategory::Banking), Lang::Vi) => "một trang ngân hàng",
        (Some(SiteCategory::Brokerage), Lang::En) => "a brokerage site",
        (Some(SiteCategory::Brokerage), Lang::Vi) => "một trang chứng khoán",
        (Some(SiteCategory::Crypto), Lang::En) => "a crypto site",
        (Some(SiteCategory::Crypto), Lang::Vi) => "một trang tiền mã hóa",
        (Some(SiteCategory::Wallet), Lang::En) => "a payment wallet site",
        (Some(SiteCategory::Wallet), Lang::Vi) => "một trang ví thanh toán",
        (Some(SiteCategory::Government), Lang::En) => "a government site",
        (Some(SiteCategory::Government), Lang::Vi) => "một trang của cơ quan nhà nước",
        (None, Lang::En) => "a sensitive site",
        (None, Lang::Vi) => "một trang nhạy cảm",
    }
}

/// The sentence that says why this category asks, written for the first category of a card.
fn tail(lang: Lang, category: Category, facts: &CardFacts) -> Option<String> {
    let site = clean(&facts.site, MAX_LABEL);
    Some(match (category, lang) {
        (Category::Payment, Lang::En) => "This looks like a payment.".into(),
        (Category::Payment, Lang::Vi) => "Đây có vẻ là một khoản thanh toán.".into(),
        (Category::Destructive, Lang::En) => match &facts.dialog_text {
            Some(text) if facts.in_dialog => {
                format!("The dialog says: {}", quoted(text, MAX_QUOTE))
            }
            _ => "This may delete or cancel something.".into(),
        },
        (Category::Destructive, Lang::Vi) => match &facts.dialog_text {
            Some(text) if facts.in_dialog => format!("Hộp thoại ghi: {}", quoted(text, MAX_QUOTE)),
            _ => "Thao tác này có thể xóa hoặc hủy một thứ gì đó.".into(),
        },
        (Category::AccountSecurity, Lang::En) => {
            "This changes how your account is protected.".into()
        }
        (Category::AccountSecurity, Lang::Vi) => {
            "Thao tác này thay đổi cách bảo vệ tài khoản của bạn.".into()
        }
        (Category::Sharing, Lang::En) => "This changes who can see or open it.".into(),
        (Category::Sharing, Lang::Vi) => "Thao tác này thay đổi ai có thể xem hoặc mở nó.".into(),
        (Category::SendUnnamed, Lang::En) => {
            if facts.recipients.is_empty() {
                "You did not mention who receives it.".into()
            } else {
                "You did not mention this person.".into()
            }
        }
        (Category::SendUnnamed, Lang::Vi) => {
            if facts.recipients.is_empty() {
                "Bạn chưa nhắc tới người nhận.".into()
            } else {
                "Bạn chưa nhắc tới người này.".into()
            }
        }
        (Category::PostPublic, Lang::En) => "It will be visible to the public.".into(),
        (Category::PostPublic, Lang::Vi) => "Bài đăng sẽ hiển thị công khai.".into(),
        // The headline already says which files leave the computer.
        (Category::Upload, _) if matches!(facts.action, ActionDesc::Upload { .. }) => return None,
        (Category::Upload, Lang::En) => format!("Files from your computer go to {site}."),
        (Category::Upload, Lang::Vi) => format!("Các tệp từ máy bạn sẽ được gửi lên {site}."),
        (Category::CredentialEntry, Lang::En) => {
            "This is sensitive information and it is asked every time.".into()
        }
        (Category::CredentialEntry, Lang::Vi) => {
            "Đây là thông tin nhạy cảm và lần nào cũng được hỏi.".into()
        }
        (Category::Terms, Lang::En) => "This accepts terms on your behalf.".into(),
        (Category::Terms, Lang::Vi) => "Thao tác này chấp nhận điều khoản thay bạn.".into(),
        (Category::UnrelatedDomain, Lang::En) => {
            let mut text = "This site is not part of what you asked for.".to_owned();
            if !facts.url.is_empty() {
                text.push_str(&format!(" Full address: {}", clean(&facts.url, 300)));
            }
            text
        }
        (Category::UnrelatedDomain, Lang::Vi) => {
            let mut text = "Trang này không thuộc việc bạn yêu cầu.".to_owned();
            if !facts.url.is_empty() {
                text.push_str(&format!(" Địa chỉ đầy đủ: {}", clean(&facts.url, 300)));
            }
            text
        }
        // A navigation card already names the kind of site in its headline.
        (Category::SensitiveSite, _) if matches!(facts.action, ActionDesc::Navigate) => {
            return None
        }
        (Category::SensitiveSite, Lang::En) => {
            format!("This is {}.", site_kind_noun(lang, facts.site_kind))
        }
        (Category::SensitiveSite, Lang::Vi) => {
            format!("Đây là {}.", site_kind_noun(lang, facts.site_kind))
        }
        (Category::SoulRule, Lang::En) => match (&facts.rule, &facts.agent_action) {
            (Some(rule), _) => format!(
                "Your rule for {} says to ask first: {}.",
                clean(&facts.mono, MAX_LABEL),
                rule_body(rule)
            ),
            (None, Some(action)) => format!(
                "{} asked for your OK. In its words: {}",
                clean(&facts.mono, MAX_LABEL),
                quoted(action, MAX_QUOTE)
            ),
            (None, None) => format!(
                "This needs your OK because of the rules for {}.",
                clean(&facts.mono, MAX_LABEL)
            ),
        },
        (Category::SoulRule, Lang::Vi) => match (&facts.rule, &facts.agent_action) {
            (Some(rule), _) => format!(
                "Quy tắc bạn đặt cho {} yêu cầu hỏi trước: {}.",
                clean(&facts.mono, MAX_LABEL),
                rule_body(rule)
            ),
            (None, Some(action)) => format!(
                "{} xin bạn đồng ý. Theo lời {}: {}",
                clean(&facts.mono, MAX_LABEL),
                clean(&facts.mono, MAX_LABEL),
                quoted(action, MAX_QUOTE)
            ),
            (None, None) => format!(
                "Việc này cần bạn đồng ý theo quy tắc của {}.",
                clean(&facts.mono, MAX_LABEL)
            ),
        },
        (Category::Unclear, Lang::En) => {
            "zptah could not read enough of this page to tell what this does.".into()
        }
        (Category::Unclear, Lang::Vi) => {
            "zptah không đọc đủ nội dung trang này để biết thao tác này làm gì.".into()
        }
        (Category::Tainted, Lang::En) => {
            "It carries something read on another site to this one.".into()
        }
        (Category::Tainted, Lang::Vi) => {
            "Việc này mang thông tin đọc được ở trang khác sang trang này.".into()
        }
        (Category::Injection, Lang::En) => {
            "A page tried to give instructions to the Mono, so every change asks.".into()
        }
        (Category::Injection, Lang::Vi) => {
            "Một trang web đã thử ra lệnh cho Mono, nên mọi thay đổi đều được hỏi.".into()
        }
    })
}

/// A short extra line for a category that is not the first one.
fn extra(lang: Lang, category: Category, facts: &CardFacts) -> Option<String> {
    Some(match (category, lang) {
        (Category::Payment, Lang::En) => "It also looks like a payment.".into(),
        (Category::Payment, Lang::Vi) => "Thao tác này cũng có vẻ là một khoản thanh toán.".into(),
        (Category::Destructive, Lang::En) => "It may also delete or cancel something.".into(),
        (Category::Destructive, Lang::Vi) => {
            "Thao tác này cũng có thể xóa hoặc hủy một thứ gì đó.".into()
        }
        (Category::AccountSecurity, Lang::En) => "It also changes your account security.".into(),
        (Category::AccountSecurity, Lang::Vi) => {
            "Thao tác này cũng thay đổi bảo mật tài khoản của bạn.".into()
        }
        (Category::Sharing, Lang::En) => "It also changes who can see or open it.".into(),
        (Category::Sharing, Lang::Vi) => {
            "Thao tác này cũng thay đổi ai có thể xem hoặc mở nó.".into()
        }
        (Category::SendUnnamed, Lang::En) => "You did not mention who receives it.".into(),
        (Category::SendUnnamed, Lang::Vi) => "Bạn chưa nhắc tới người nhận.".into(),
        (Category::PostPublic, Lang::En) => "It will also be visible to the public.".into(),
        (Category::PostPublic, Lang::Vi) => "Thao tác này cũng sẽ hiển thị công khai.".into(),
        (Category::Upload, Lang::En) => "It also uploads files from your computer.".into(),
        (Category::Upload, Lang::Vi) => "Thao tác này cũng tải tệp từ máy bạn lên.".into(),
        (Category::CredentialEntry, Lang::En) => "It also enters sensitive information.".into(),
        (Category::CredentialEntry, Lang::Vi) => {
            "Thao tác này cũng nhập thông tin nhạy cảm.".into()
        }
        (Category::Terms, Lang::En) => "It also accepts terms on your behalf.".into(),
        (Category::Terms, Lang::Vi) => "Thao tác này cũng chấp nhận điều khoản thay bạn.".into(),
        (Category::UnrelatedDomain, Lang::En) => {
            "This site is not part of what you asked for.".into()
        }
        (Category::UnrelatedDomain, Lang::Vi) => "Trang này không thuộc việc bạn yêu cầu.".into(),
        (Category::SensitiveSite, Lang::En) => {
            format!("It is also on {}.", site_kind_noun(lang, facts.site_kind))
        }
        (Category::SensitiveSite, Lang::Vi) => {
            format!(
                "Thao tác này cũng ở trên {}.",
                site_kind_noun(lang, facts.site_kind)
            )
        }
        (Category::SoulRule, _) => return tail(lang, category, facts),
        (Category::Tainted, _) | (Category::Injection, _) | (Category::Unclear, _) => {
            return tail(lang, category, facts)
        }
    })
}

/// Renders the card of an ask.
pub fn card(lang: Lang, facts: &CardFacts) -> CardText {
    let mono = clean(&facts.mono, MAX_LABEL);
    let site = clean(&facts.site, MAX_LABEL);
    let main = primary(&facts.categories);
    let navigates = matches!(facts.action, ActionDesc::Navigate);
    let phrase = verb_phrase(lang, facts);
    let headline = match (lang, navigates, main) {
        // "Mira wants to open vietcombank.com.vn, a banking site."
        (Lang::En, true, Some(Category::SensitiveSite)) => format!(
            "{mono} wants to open {site}, {}.",
            site_kind_noun(lang, facts.site_kind)
        ),
        (Lang::Vi, true, Some(Category::SensitiveSite)) => format!(
            "{mono} muốn mở {site}, {}.",
            site_kind_noun(lang, facts.site_kind)
        ),
        (Lang::En, true, _) => format!("{mono} wants to open {site}."),
        (Lang::Vi, true, _) => format!("{mono} muốn mở {site}."),
        (Lang::En, _, _) if matches!(facts.action, ActionDesc::Other) => {
            match facts.agent_action.as_deref() {
                Some(said) => format!("{mono} asks your OK on {site}: {}", quoted(said, MAX_QUOTE)),
                None => format!("{mono} asks your OK on {site}."),
            }
        }
        (Lang::Vi, _, _) if matches!(facts.action, ActionDesc::Other) => {
            match facts.agent_action.as_deref() {
                Some(said) => format!(
                    "{mono} xin bạn đồng ý trên {site}: {}",
                    quoted(said, MAX_QUOTE)
                ),
                None => format!("{mono} xin bạn đồng ý trên {site}."),
            }
        }
        (Lang::En, _, _) => match &facts.action {
            ActionDesc::Upload { .. } => format!("{mono} wants to {phrase} to {site}."),
            _ => format!("{mono} wants to {phrase} on {site}."),
        },
        (Lang::Vi, _, _) => match &facts.action {
            ActionDesc::Upload { .. } => format!("{mono} muốn {phrase} {site}."),
            _ => format!("{mono} muốn {phrase} trên {site}."),
        },
    };

    let mut lines = Vec::new();
    if let Some(main) = main {
        // "Other" (a request the Mono described itself) already put its words in the headline.
        let skip_tail = matches!(facts.action, ActionDesc::Other)
            && main == Category::SoulRule
            && facts.rule.is_none();
        if !skip_tail {
            if let Some(line) = tail(lang, main, facts) {
                lines.push(line);
            }
        }
        for category in ordered(&facts.categories)
            .into_iter()
            .filter(|c| *c != main)
        {
            if let Some(line) = extra(lang, category, facts) {
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
        }
    }
    // The user's own tab: the card says which page is being handed over.
    if matches!(facts.action, ActionDesc::TabTransfer { .. }) && !facts.url.is_empty() {
        let url = clean(&facts.url, 300);
        lines.push(match lang {
            Lang::En => format!("Address: {url}"),
            Lang::Vi => format!("Địa chỉ: {url}"),
        });
    }
    if facts.categories.contains(&Category::Payment) && !facts.amounts.is_empty() {
        let amounts = join(&facts.amounts, 3);
        lines.push(match lang {
            Lang::En => format!("Amount on the page: {amounts}."),
            Lang::Vi => format!("Số tiền trên trang: {amounts}."),
        });
    }
    if let Some(details) = facts
        .agent_details
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        lines.push(match lang {
            Lang::En => format!("Details from {mono}: {}", quoted(details, MAX_QUOTE)),
            Lang::Vi => format!("Chi tiết từ {mono}: {}", quoted(details, MAX_QUOTE)),
        });
    }
    let purpose = facts
        .purpose
        .as_deref()
        .map(|p| clean(p, MAX_QUOTE))
        .filter(|p| !p.is_empty())
        .map(|p| match lang {
            Lang::En => format!("{mono} says: \"{}\"", p.replace('"', "'")),
            Lang::Vi => format!("{mono} nói: \"{}\"", p.replace('"', "'")),
        });
    let grantable = !facts
        .categories
        .iter()
        .any(|c| super::risk::decide::never_granted(*c));
    let b = Buttons::of(lang);
    CardText {
        headline,
        lines,
        purpose,
        allow_once: b.allow_once,
        allow_conversation: grantable.then_some(b.allow_conversation),
        deny: b.deny,
        show_tab: b.show_tab,
    }
}

struct Buttons {
    allow_once: &'static str,
    allow_conversation: &'static str,
    deny: &'static str,
    show_tab: &'static str,
}

impl Buttons {
    fn of(lang: Lang) -> Self {
        match lang {
            Lang::En => Self {
                allow_once: "Allow once",
                allow_conversation: "Allow in this conversation",
                deny: "Don't allow",
                show_tab: "Show tab",
            },
            Lang::Vi => Self {
                allow_once: "Cho phép lần này",
                allow_conversation: "Cho phép trong cuộc trò chuyện này",
                deny: "Không cho phép",
                show_tab: "Xem tab",
            },
        }
    }
}

/// Not a prompt, a stop: the Mono needs a password (spec 8.3, 8.6). The only button is Show tab.
pub fn password_stop(lang: Lang, mono: &str, site: &str) -> CardText {
    let mono = clean(mono, MAX_LABEL);
    let site = clean(site, MAX_LABEL);
    let b = Buttons::of(lang);
    let (headline, line) = match lang {
        Lang::En => (
            format!("{mono} needs your password on {site}."),
            "zptah never lets a Mono type passwords. Open the tab, sign in yourself, then tell the Mono to continue.",
        ),
        Lang::Vi => (
            format!("{mono} cần mật khẩu của bạn trên {site}."),
            "zptah không bao giờ cho Mono nhập mật khẩu. Hãy mở tab, tự đăng nhập, rồi bảo Mono tiếp tục.",
        ),
    };
    CardText {
        headline,
        lines: vec![line.to_owned()],
        purpose: None,
        allow_once: b.allow_once,
        allow_conversation: None,
        deny: b.deny,
        show_tab: b.show_tab,
    }
}

/// The prompt when the Mono's browser rules changed in a way the gate will not apply on its own
/// (spec 7.5): the buttons are "Review" and "Keep the old rules".
pub struct RuleReview {
    pub headline: String,
    pub line: String,
    pub confirm: &'static str,
    pub keep: &'static str,
}

pub fn rule_review(lang: Lang, mono: &str, changed_lines: &[String]) -> RuleReview {
    let mono = clean(mono, MAX_LABEL);
    let shown = join(changed_lines, 3);
    match lang {
        Lang::En => RuleReview {
            headline: format!("The browser rules of {mono} changed."),
            line: format!(
                "Until you confirm, the old rules stay in force and the new lines only add asks. Changed: {shown}"
            ),
            confirm: "Confirm the new rules",
            keep: "Keep the old rules",
        },
        Lang::Vi => RuleReview {
            headline: format!("Quy tắc trình duyệt của {mono} đã thay đổi."),
            line: format!(
                "Cho đến khi bạn xác nhận, quy tắc cũ vẫn có hiệu lực và các dòng mới chỉ thêm yêu cầu hỏi. Thay đổi: {shown}"
            ),
            confirm: "Xác nhận quy tắc mới",
            keep: "Giữ quy tắc cũ",
        },
    }
}

#[cfg(test)]
mod tests;
