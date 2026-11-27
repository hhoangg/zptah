//! The risk lexicon and the name classifier (spec 7.3).
//!
//! The data is `extensions/zptah-browser/shared/risk-lexicon.json`, embedded with `include_str!`
//! so the extension and the gate match the same words. Matching rules, in the order they apply:
//!
//! 1. Text and phrases are normalized by [`normalize`](super::normalize). A segment with
//!    Vietnamese diacritics is compared with diacritics; one without any is compared stripped.
//! 2. A phrase matches as a whole-word sequence inside one punctuation segment.
//! 3. A single-word phrase (`pay`, `xoa`, `gui`) matches only as the whole name or as the first
//!    word of a short name (6 words, `Mua`, `Gửi` and `Đăng` 3 words).
//! 4. Ambiguous stripped single words (`go`, `moi`, `dang`, `dong`, `co`) only count on a
//!    Vietnamese page.
//! 5. The longest phrase wins. Not-risky and dismiss entries match only as the whole name (a few
//!    sign-in and sign-up entries also win when they start the name).
//! 6. Buttons are matched in the first words of each segment; links and landmark text that are
//!    long or start with `Chính sách`, `Hướng dẫn`, ... are informational; some links are exempt.
//!
//! Residual risks (known, accepted):
//!
//! * Chinese, Japanese, Korean and Thai have no spaces, so their entries match only a whole
//!   name, not a phrase inside a longer name.
//! * An ambiguous stripped word (`go`, `moi`, `dang`, `dong`, `co`) in an unaccented name on a
//!   Vietnamese page counts as the Vietnamese word, so an English "Go" there is read as `Gỡ`.
//! * Buttons are matched only in the first 4 words of each punctuation segment, so a risky
//!   phrase placed after four words of a long button name is not seen (links and landmark text
//!   have no window).
//! * Matching is by words; a button whose meaning comes only from an icon or from script
//!   behavior is judged by its accessible name and visible text, so a misleading name can hide
//!   the action. The decision keeps the declared, extension, soul and taint sources for that.

use super::normalize::{normalize, Norm, Word};
use super::signals::{sites, FieldKind};
use super::Category;
use serde::Deserialize;
use std::collections::{BTreeSet, HashSet};
use std::sync::OnceLock;

const LEXICON_JSON: &str =
    include_str!("../../../../../../extensions/zptah-browser/shared/risk-lexicon.json");

/// What an entry stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Category(Category),
    /// A send phrase; becomes `send_unnamed` when the recipient is not the user's.
    Send,
    /// A post phrase; becomes `post_public`.
    Post,
    Consent,
    /// A bare confirm-like word that takes the category of its page or dialog.
    Context,
    /// A dismiss word (cancel, close, skip).
    Dismiss,
    NotRisky,
    Field(FieldKind),
    /// `security code`: a card CVC only when no OTP signal is present.
    CvcWeak,
}

impl Class {
    fn parse(s: &str) -> Option<Class> {
        Some(match s {
            "payment" => Class::Category(Category::Payment),
            "destructive" => Class::Category(Category::Destructive),
            "account_security" => Class::Category(Category::AccountSecurity),
            "sharing" => Class::Category(Category::Sharing),
            "terms" => Class::Category(Category::Terms),
            "send" => Class::Send,
            "post" => Class::Post,
            "consent" => Class::Consent,
            "context" => Class::Context,
            "dismiss" => Class::Dismiss,
            "not_risky" => Class::NotRisky,
            "field_card_cvc_weak" => Class::CvcWeak,
            other => Class::Field(FieldKind::parse(other.strip_prefix("field_")?)?),
        })
    }
    fn is_field(self) -> bool {
        matches!(self, Class::Field(_) | Class::CvcWeak)
    }
}

#[derive(Deserialize)]
struct RawEntry {
    phrase: String,
    class: String,
    #[serde(default)]
    max_words: Option<usize>,
    #[serde(default)]
    prefix: bool,
    #[serde(default)]
    ctx: bool,
}

#[derive(Deserialize)]
struct RawLexicon {
    short_word_max_words: usize,
    button_window_words: usize,
    informational_max_words: usize,
    signup_prefixes: Vec<String>,
    period_words: Vec<String>,
    ambiguous_stripped: Vec<String>,
    informational_prefixes: Vec<String>,
    consent_container_words: Vec<String>,
    consent_managers: Vec<String>,
    public_visibility_words: Vec<String>,
    visibility_control_words: Vec<String>,
    entries: Vec<RawEntry>,
}

/// One prepared phrase.
#[derive(Debug)]
pub struct Entry {
    pub phrase: String,
    words: Vec<Word>,
    pub class: Class,
    max_words: usize,
    prefix: bool,
    ctx: bool,
    has_diacritics: bool,
}

impl Entry {
    fn whole_name_only(&self) -> bool {
        matches!(self.class, Class::NotRisky | Class::Dismiss) && !self.prefix
    }
}

/// How a text is searched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A control name: short-word and whole-name rules apply.
    Name,
    /// Dialog, heading and page text: multi-word phrases anywhere, single words by the name rule
    /// per segment.
    Context,
    /// Dialog text, any phrase anywhere (a destructive question may be a sentence).
    Anywhere,
    /// Form-field attributes: field classes only, anywhere.
    Field,
}

#[derive(Debug, Clone, Copy)]
struct Hit {
    seg: usize,
    start: usize,
    len: usize,
    entry: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Informational {
    Long,
    Prefix,
}

/// Whether a normalized text starts with the words of a phrase (compared stripped when the
/// first segment has no diacritics).
fn starts_with_phrase(norm: &Norm, phrase: &Norm) -> bool {
    let (Some(first), Some(pw)) = (norm.segments.first(), phrase.segments.first()) else {
        return false;
    };
    let plain = first.iter().all(|w| w.acc == w.plain);
    first.len() >= pw.len()
        && pw.iter().zip(first).all(|(a, b)| {
            if plain {
                a.plain == b.plain
            } else {
                a.acc == b.acc
            }
        })
}

/// What a control name means to the gate before signals are added.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Classification {
    pub categories: BTreeSet<Category>,
    /// A send phrase matched; the recipient decides whether it is `send_unnamed`.
    pub send: bool,
    /// A post phrase matched; visibility decides whether it is `post_public`.
    pub post: bool,
    /// A dismiss control (Cancel in a dialog with a primary button, Close, Not now).
    pub dismiss: bool,
    /// The whole name is a known harmless action (Add to cart, Đăng nhập). It never removes a
    /// category that comes from another signal.
    pub not_risky: bool,
    /// The name is a cookie-consent accept phrase.
    pub consent: bool,
    /// A long link or policy text: it only feeds nearby text.
    pub informational: bool,
}

impl Classification {
    pub fn is_risky(&self) -> bool {
        !self.categories.is_empty() || self.send || self.post
    }
}

/// The kind of control a name belongs to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlKind {
    #[default]
    Button,
    Submit,
    Link,
    /// Landmark or other text.
    Text,
}

/// What the extension knows about a link (spec 7.3, F8).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LinkContext {
    pub href: String,
    pub page_url: String,
    /// Inside `nav`, `footer` or `role=menu`.
    pub in_nav_footer_menu: bool,
}

/// Everything the name classifier looks at.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ControlInput {
    pub name: String,
    pub kind: ControlKind,
    /// The page is Vietnamese (see [`page_is_vietnamese`](super::normalize::page_is_vietnamese)).
    pub page_vi: bool,
    pub in_dialog: bool,
    pub dialog_text: String,
    /// The dialog has a primary (confirm) button besides the control.
    pub dialog_has_primary: bool,
    /// Names of the dialog's other primary buttons (empty when unknown).
    pub dialog_primary_names: Vec<String>,
    /// Ids and classes of the dialog, for consent-manager detection.
    pub dialog_markers: String,
    /// ARIA role of the control (`button` on an `<a>` makes it a button).
    pub role: String,
    pub nearby_text: String,
    pub page_heading: String,
    pub page_title: String,
    pub link: Option<LinkContext>,
}

/// The prepared lexicon.
pub struct Lexicon {
    entries: Vec<Entry>,
    window: usize,
    info_max: usize,
    signup_prefixes: Vec<Norm>,
    period_words: Vec<Norm>,
    ambiguous: HashSet<String>,
    info_prefixes: Vec<Norm>,
    consent_words: Vec<Norm>,
    consent_managers: Vec<String>,
    public_words: Vec<Norm>,
    visibility_words: Vec<Norm>,
}

/// The embedded lexicon, parsed once.
pub fn lexicon() -> &'static Lexicon {
    static CELL: OnceLock<Lexicon> = OnceLock::new();
    CELL.get_or_init(|| {
        Lexicon::from_json(LEXICON_JSON).expect("shared/risk-lexicon.json is a valid lexicon")
    })
}

impl Lexicon {
    /// Parses a lexicon file. Errors name the entry that is wrong.
    pub fn from_json(json: &str) -> Result<Lexicon, String> {
        let raw: RawLexicon = serde_json::from_str(json).map_err(|e| e.to_string())?;
        let mut entries: Vec<Entry> = Vec::with_capacity(raw.entries.len());
        let mut seen: HashSet<(String, String)> = HashSet::new();
        for e in raw.entries {
            let class =
                Class::parse(&e.class).ok_or_else(|| format!("unknown class {:?}", e.class))?;
            let norm = normalize(&e.phrase);
            if norm.segments.len() != 1 {
                return Err(format!("phrase {:?} must be one run of words", e.phrase));
            }
            let words = norm.segments.into_iter().next().unwrap_or_default();
            let key = (
                words
                    .iter()
                    .map(|w| w.acc.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
                e.class.clone(),
            );
            if !seen.insert(key) {
                continue;
            }
            entries.push(Entry {
                phrase: e.phrase,
                has_diacritics: norm.has_diacritics,
                words,
                class,
                max_words: e.max_words.unwrap_or(raw.short_word_max_words),
                prefix: e.prefix,
                ctx: e.ctx,
            });
        }
        let norms = |list: &[String]| list.iter().map(|s| normalize(s)).collect::<Vec<_>>();
        Ok(Lexicon {
            entries,
            window: raw.button_window_words,
            info_max: raw.informational_max_words,
            signup_prefixes: norms(&raw.signup_prefixes),
            period_words: norms(&raw.period_words),
            ambiguous: raw.ambiguous_stripped.into_iter().collect(),
            info_prefixes: norms(&raw.informational_prefixes),
            consent_words: norms(&raw.consent_container_words),
            consent_managers: raw
                .consent_managers
                .iter()
                .map(|s| s.to_lowercase())
                .collect(),
            public_words: norms(&raw.public_visibility_words),
            visibility_words: norms(&raw.visibility_control_words),
        })
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    fn entry_matches_at(&self, e: &Entry, seg: &[Word], start: usize, plain: bool) -> bool {
        let end = start + e.words.len();
        if end > seg.len() {
            return false;
        }
        e.words.iter().zip(&seg[start..end]).all(|(ew, tw)| {
            if plain {
                ew.plain == tw.plain
            } else {
                ew.acc == tw.acc
            }
        })
    }

    fn find(&self, norm: &Norm, page_vi: bool, mode: Mode) -> Vec<Hit> {
        let total = norm.word_count();
        let mut hits = Vec::new();
        for (si, seg) in norm.segments.iter().enumerate() {
            // A segment without Vietnamese diacritics is compared stripped, even when another
            // segment of the same name has them.
            let plain = seg.iter().all(|w| w.acc == w.plain);
            for start in 0..seg.len() {
                for (ei, e) in self.entries.iter().enumerate() {
                    if (mode == Mode::Field) != e.class.is_field() {
                        continue;
                    }
                    if !self.entry_matches_at(e, seg, start, plain) {
                        continue;
                    }
                    let len = e.words.len();
                    if plain
                        && len == 1
                        && e.has_diacritics
                        && !page_vi
                        && self.ambiguous.contains(&e.words[0].plain)
                    {
                        continue;
                    }
                    let whole = norm.segments.len() == 1 && start == 0 && len == total;
                    let allowed = match mode {
                        Mode::Field | Mode::Anywhere => true,
                        Mode::Name | Mode::Context => {
                            if e.whole_name_only() {
                                whole && mode == Mode::Name
                            } else if e.prefix {
                                si == 0 && start == 0
                            } else if len == 1 {
                                if mode == Mode::Context {
                                    start == 0 && seg.len() <= e.max_words
                                } else {
                                    whole || (si == 0 && start == 0 && total <= e.max_words)
                                }
                            } else {
                                true
                            }
                        }
                    };
                    if allowed {
                        hits.push(Hit {
                            seg: si,
                            start,
                            len,
                            entry: ei,
                        });
                    }
                }
            }
        }
        hits
    }

    /// Longest phrase wins: a hit that overlaps a longer one is dropped. Hits on the exact same
    /// span (a phrase that is both `terms` and `payment`) stay together.
    fn longest(&self, mut hits: Vec<Hit>) -> Vec<Hit> {
        hits.sort_by(|a, b| {
            b.len
                .cmp(&a.len)
                .then(a.seg.cmp(&b.seg))
                .then(a.start.cmp(&b.start))
        });
        let mut chosen: Vec<Hit> = Vec::new();
        for h in hits {
            let clash = chosen.iter().any(|c| {
                c.seg == h.seg
                    && h.start < c.start + c.len
                    && c.start < h.start + h.len
                    && !(c.start == h.start && c.len == h.len)
            });
            if !clash {
                chosen.push(h);
            }
        }
        chosen
    }

    /// Classifies a name alone (no dialog or page context).
    pub fn classify_name(&self, name: &str, kind: ControlKind, page_vi: bool) -> Classification {
        self.classify_norm(&normalize(name), kind, page_vi).0
    }

    /// Whether the name is a bare word that takes its meaning from the dialog or the page around
    /// it (Confirm, OK, Yes, Continue, Xác nhận ...): what it does can not be told from the name.
    pub fn takes_context(&self, name: &str, kind: ControlKind, page_vi: bool) -> bool {
        let (_, context, ctx_dismiss) = self.classify_norm(&normalize(name), kind, page_vi);
        context || ctx_dismiss
    }

    /// Returns the classification and whether a context-taking word matched.
    fn classify_norm(
        &self,
        norm: &Norm,
        kind: ControlKind,
        page_vi: bool,
    ) -> (Classification, bool, bool) {
        let mut out = Classification::default();
        if norm.is_empty() {
            return (out, false, false);
        }
        let mut hits = self.find(norm, page_vi, Mode::Name);
        if matches!(kind, ControlKind::Link | ControlKind::Text) {
            if let Some(reason) = self.informational_reason(norm) {
                // A long or policy-like link only feeds nearby text, but a multi-word destructive
                // or account-security phrase near its start still counts, and so does a payment
                // phrase in a long name ("Help: delete account" must not hide). A payment phrase
                // after a policy prefix ("Chính sách thanh toán") stays informational (F8).
                let kept: Vec<Hit> = self
                    .longest(hits)
                    .into_iter()
                    .filter(|h| {
                        let e = &self.entries[h.entry];
                        // A payment phrase after a policy prefix is informational only in the
                        // first segment ("Chính sách thanh toán"); after a colon or dash
                        // ("Điều khoản: Thanh toán ngay") it is its own phrase.
                        let payment_ok = reason == Informational::Long || h.seg > 0;
                        h.start < self.window
                            && e.words.len() >= 2
                            && match e.class {
                                Class::Category(
                                    Category::Destructive | Category::AccountSecurity,
                                ) => true,
                                Class::Category(Category::Payment) => payment_ok,
                                _ => false,
                            }
                    })
                    .collect();
                if kept.is_empty() {
                    out.informational = true;
                    return (out, false, false);
                }
                hits = kept;
            }
        }
        if matches!(kind, ControlKind::Button | ControlKind::Submit) {
            hits.retain(|h| h.start < self.window);
        }
        let (mut context, mut ctx_dismiss) = (false, false);
        for h in self.longest(hits) {
            let e = &self.entries[h.entry];
            match e.class {
                Class::Category(c) => {
                    out.categories.insert(c);
                }
                Class::Send => out.send = true,
                Class::Post => out.post = true,
                Class::Consent => out.consent = true,
                Class::Context => context = true,
                Class::Dismiss => {
                    out.dismiss = true;
                    ctx_dismiss |= e.ctx;
                }
                Class::NotRisky => out.not_risky = true,
                Class::Field(_) | Class::CvcWeak => {}
            }
        }
        (out, context, ctx_dismiss)
    }

    fn informational_reason(&self, norm: &Norm) -> Option<Informational> {
        if norm.word_count() > self.info_max {
            return Some(Informational::Long);
        }
        self.info_prefixes
            .iter()
            .any(|p| starts_with_phrase(norm, p))
            .then_some(Informational::Prefix)
    }

    /// The categories a text implies (page heading, nearby text, dialog text).
    pub fn text_categories(&self, text: &str, page_vi: bool, anywhere: bool) -> BTreeSet<Category> {
        let norm = normalize(text);
        let mut out = BTreeSet::new();
        let mode = if anywhere {
            Mode::Anywhere
        } else {
            Mode::Context
        };
        for h in self.longest(self.find(&norm, page_vi, mode)) {
            if let Class::Category(c) = self.entries[h.entry].class {
                if anywhere && self.entries[h.entry].words.len() == 1 && c != Category::Destructive
                {
                    continue;
                }
                out.insert(c);
            }
        }
        out
    }

    fn context_categories(&self, input: &ControlInput) -> BTreeSet<Category> {
        let mut out = BTreeSet::new();
        if input.in_dialog {
            out.extend(self.text_categories(&input.dialog_text, input.page_vi, true));
        }
        for text in [&input.page_heading, &input.nearby_text, &input.page_title] {
            out.extend(self.text_categories(text, input.page_vi, false));
        }
        out
    }

    /// Classifies a control: its name, the dialog it is in and the page around it.
    pub fn classify_control(&self, input: &ControlInput) -> Classification {
        let norm = normalize(&input.name);
        let (mut out, context, ctx_dismiss) = self.classify_norm(&norm, input.kind, input.page_vi);
        if out.informational {
            return out;
        }
        if out.dismiss {
            if ctx_dismiss && !(input.in_dialog && input.dialog_has_primary) {
                // A bare Cancel outside a dialog (or in one with no primary button) takes the
                // category of the page.
                out.dismiss = false;
                out.categories.extend(self.context_categories(input));
            } else if ctx_dismiss && self.dialog_cancel_is_the_risky_one(input) {
                // The dialog asks a destructive question and no other primary button carries it,
                // so this Cancel is the control that does the destructive thing.
                out.dismiss = false;
                out.categories.insert(Category::Destructive);
                return out;
            } else {
                // Dismissing is never risky, whatever the dialog says.
                out.categories.clear();
                return out;
            }
        }
        if context {
            out.categories.extend(self.context_categories(input));
        }
        if out.not_risky && self.signup_with_price(input, &norm) {
            // A sign-up with a price or a billing period creates a paid subscription (7b).
            out.not_risky = false;
            out.categories.insert(Category::Payment);
        }
        if input.in_dialog {
            // A consent-looking name skips the dialog text only inside a confirmed cookie banner.
            // Any payment, account, sharing or terms signal in the dialog text disqualifies it
            // (spec 7.3), whatever cookie words or markers it has.
            let in_banner = out.consent
                && self.looks_like_consent_container(&input.dialog_text, &input.dialog_markers)
                && self
                    .text_categories(&input.dialog_text, input.page_vi, true)
                    .is_empty();
            if in_banner {
                // keep the consent flag
            } else {
                out.consent = false;
                out.categories.extend(self.text_categories(
                    &input.dialog_text,
                    input.page_vi,
                    true,
                ));
            }
        }
        if input.kind == ControlKind::Link && self.link_exempt(input, &out) {
            out.categories.clear();
            out.send = false;
            out.post = false;
        }
        out
    }

    /// M2: Cancel in a destructive dialog whose other primary buttons do not carry the
    /// destructive meaning (their names are known and none is destructive).
    fn dialog_cancel_is_the_risky_one(&self, input: &ControlInput) -> bool {
        if input.dialog_primary_names.is_empty()
            || !self
                .text_categories(&input.dialog_text, input.page_vi, true)
                .contains(&Category::Destructive)
        {
            return false;
        }
        // A primary button carries the meaning when its own name is destructive or is a bare
        // confirm word (Yes, OK, Confirm) that takes the dialog's meaning.
        !input.dialog_primary_names.iter().any(|name| {
            let (class, context, _) =
                self.classify_norm(&normalize(name), ControlKind::Button, input.page_vi);
            context || class.categories.contains(&Category::Destructive)
        })
    }

    /// I5: a sign-up phrase next to an amount or a billing period.
    fn signup_with_price(&self, input: &ControlInput, name: &Norm) -> bool {
        if !self
            .signup_prefixes
            .iter()
            .any(|p| starts_with_phrase(name, p))
        {
            return false;
        }
        let text = format!("{} {}", input.name, input.nearby_text);
        !super::signals::extract_amounts(&text).is_empty()
            || contains_any(&normalize(&text), &self.period_words)
    }

    /// The footer and menu link exemption of F8 (spec 7.3).
    fn link_exempt(&self, input: &ControlInput, class: &Classification) -> bool {
        let Some(link) = &input.link else {
            return false;
        };
        let href = link.href.trim();
        if !link.in_nav_footer_menu
            || href.is_empty()
            || href.starts_with('#')
            || input.role.eq_ignore_ascii_case("button")
            || input.in_dialog
            || class.categories.contains(&Category::Destructive)
            || class.categories.contains(&Category::AccountSecurity)
        {
            return false;
        }
        let Ok(page) = url::Url::parse(&link.page_url) else {
            return false;
        };
        let Ok(target) = page.join(&link.href) else {
            return false;
        };
        if target.origin() != page.origin() || target.query().is_some() {
            return false;
        }
        let s = sites();
        !target.path_segments().is_some_and(|mut segs| {
            segs.any(|seg| {
                let seg = seg.to_lowercase();
                s.checkout_segments.contains(&seg) || s.action_segments.contains(&seg)
            })
        })
    }

    /// The field kind named by label-like text (name, id, label, placeholder), if any.
    pub fn field_kind_of(&self, text: &str) -> Option<FieldKind> {
        let norm = normalize(text);
        let hits = self.longest(self.find(&norm, true, Mode::Field));
        let classes: Vec<Class> = hits.iter().map(|h| self.entries[h.entry].class).collect();
        let has = |k: FieldKind| classes.contains(&Class::Field(k));
        for kind in [
            FieldKind::Password,
            FieldKind::Otp,
            FieldKind::CardNumber,
            FieldKind::CardCvc,
            FieldKind::CardExpiry,
            FieldKind::BankAccount,
            FieldKind::NationalId,
            FieldKind::Amount,
        ] {
            if has(kind) {
                return Some(kind);
            }
        }
        if classes.contains(&Class::CvcWeak) {
            return Some(FieldKind::CardCvc);
        }
        None
    }

    /// Whether `text` classifies as the given risk category (used for checkbox labels).
    pub fn text_has_phrase_class(&self, text: &str, page_vi: bool, wanted: Category) -> bool {
        self.classify_name(text, ControlKind::Text, page_vi)
            .categories
            .contains(&wanted)
            || self.text_categories(text, page_vi, false).contains(&wanted)
    }

    /// A public audience word (public, everyone, công khai, mọi người).
    pub fn is_public_visibility(&self, text: &str) -> bool {
        contains_any(&normalize(text), &self.public_words)
    }

    /// A label of a visibility, audience or permission control.
    pub fn is_visibility_control(&self, text: &str) -> bool {
        contains_any(&normalize(text), &self.visibility_words)
    }

    /// Whether a container looks like a cookie banner by its text or a known consent manager.
    pub fn looks_like_consent_container(&self, text: &str, markers: &str) -> bool {
        let m = markers.to_lowercase();
        self.consent_managers.iter().any(|c| m.contains(c))
            || contains_any(&normalize(text), &self.consent_words)
    }
}

/// Whether any of `needles` occurs in `hay` as a whole-word sequence in either form.
fn contains_any(hay: &Norm, needles: &[Norm]) -> bool {
    needles.iter().any(|n| {
        let Some(nw) = n.segments.first() else {
            return false;
        };
        hay.segments.iter().any(|seg| {
            seg.windows(nw.len()).any(|win| {
                let acc = win.iter().zip(nw).all(|(a, b)| a.acc == b.acc);
                let plain = seg.iter().all(|w| w.acc == w.plain)
                    && win.iter().zip(nw).all(|(a, b)| a.plain == b.plain);
                acc || plain
            })
        })
    })
}
