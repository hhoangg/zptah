use super::decide::{decide, effective, Decision, DecisionInput};
use super::lexicon::{lexicon, Classification, ControlInput, ControlKind, Lexicon};
use super::normalize::{normalize, page_is_vietnamese};
use super::signals::*;
use super::Category;
use serde::Deserialize;
use std::collections::BTreeSet;

fn set(cats: &[Category]) -> BTreeSet<Category> {
    cats.iter().copied().collect()
}

// ---- normalization -------------------------------------------------------------------------

#[test]
fn normalization_folds_case_tone_and_marks() {
    let a = normalize("THANH TOÁN");
    let b = normalize("thanh-toán");
    let c = normalize("Thanh toan");
    assert_eq!(a.joined(false), b.joined(false));
    assert_eq!(a.joined(true), "thanh toan");
    assert_eq!(c.joined(true), "thanh toan");
    assert!(a.has_diacritics && b.has_diacritics && !c.has_diacritics);
    // Both tone styles are one spelling.
    assert_eq!(
        normalize("hoá").joined(false),
        normalize("hóa").joined(false)
    );
    assert_eq!(
        normalize("thuỷ").joined(false),
        normalize("thủy").joined(false)
    );
    assert_eq!(
        normalize("hoà").joined(false),
        normalize("hòa").joined(false)
    );
    // qu keeps its tone on y.
    assert_eq!(normalize("quý").joined(false), "quý");
    // đ is a diacritic letter and strips to d.
    assert!(normalize("Đăng").has_diacritics);
    assert_eq!(normalize("Đăng").joined(true), "dang");
}

#[test]
fn punctuation_is_a_hard_boundary_but_hyphen_between_letters_is_not() {
    let n = normalize("Sim số - Thẻ cào");
    assert_eq!(n.segments.len(), 2);
    assert_eq!(normalize("check-out").segments.len(), 1);
    assert_eq!(normalize("check-out").word_count(), 2);
    assert_eq!(normalize("don't").joined(false), "dont");
    assert_eq!(normalize("Mua, ngay").segments.len(), 2);
    assert!(normalize("   ").is_empty());
}

#[test]
fn page_language_uses_lang_then_diacritic_share() {
    assert!(page_is_vietnamese(Some("vi-VN"), "hello"));
    assert!(!page_is_vietnamese(Some("en"), "Thanh toán đơn hàng"));
    assert!(page_is_vietnamese(None, "Thanh toán đơn hàng của bạn"));
    assert!(!page_is_vietnamese(None, "Pay for your order now"));
}

// ---- the shared cases file ------------------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct Expect {
    categories: Vec<Category>,
    send: bool,
    post: bool,
    dismiss: bool,
    not_risky: bool,
    consent: bool,
    informational: bool,
}

#[derive(Deserialize)]
struct Case {
    #[serde(flatten)]
    input: ControlInput,
    expect: Expect,
}

#[derive(Deserialize)]
struct Cases {
    cases: Vec<Case>,
}

const CASES_JSON: &str =
    include_str!("../../../../../../extensions/zptah-browser/shared/lexicon-cases.json");

fn check(c: &ControlInput, want: &Expect) -> Result<(), String> {
    let got = lexicon().classify_control(c);
    let want_cats: BTreeSet<Category> = want.categories.iter().copied().collect();
    let ok = got.categories == want_cats
        && got.send == want.send
        && got.post == want.post
        && got.dismiss == want.dismiss
        && got.not_risky == want.not_risky
        && got.consent == want.consent
        && got.informational == want.informational;
    if ok {
        Ok(())
    } else {
        Err(format!("{:?} ({:?}) -> {:?}", c.name, c.kind, got))
    }
}

#[test]
fn shared_lexicon_cases_all_pass() {
    let cases: Cases = serde_json::from_str(CASES_JSON).expect("lexicon-cases.json parses");
    assert!(cases.cases.len() > 500, "cases file looks truncated");
    let failures: Vec<String> = cases
        .cases
        .iter()
        .filter_map(|c| check(&c.input, &c.expect).err())
        .collect();
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

fn classify(name: &str) -> Classification {
    lexicon().classify_name(name, ControlKind::Button, false)
}

#[test]
fn every_risk_category_has_vietnamese_and_english_entries() {
    use super::lexicon::Class;
    for cat in [
        Category::Payment,
        Category::Destructive,
        Category::AccountSecurity,
        Category::Sharing,
        Category::Terms,
    ] {
        let words: Vec<&str> = lexicon()
            .entries()
            .iter()
            .filter(|e| e.class == Class::Category(cat))
            .map(|e| e.phrase.as_str())
            .collect();
        assert!(
            words.iter().any(|w| !w.is_ascii()),
            "{cat:?} has no diacritic entry"
        );
        assert!(
            words.iter().any(|w| w.is_ascii()),
            "{cat:?} has no ascii entry"
        );
    }
}

#[test]
fn longest_phrase_wins() {
    // "Xác nhận đặt hàng" is payment, not the context word "Xác nhận".
    assert_eq!(
        classify("Xác nhận đặt hàng").categories,
        set(&[Category::Payment])
    );
    // "Gửi đánh giá" is a post (review), not a plain send.
    let c = classify("Gửi đánh giá");
    assert!(c.post && !c.send);
    // "Xóa tài khoản" stays destructive and is not cut by "Tài khoản".
    assert_eq!(
        classify("Xóa tài khoản").categories,
        set(&[Category::Destructive])
    );
    // "Chia sẻ ngay" is sharing.
    assert_eq!(
        classify("Chia sẻ ngay").categories,
        set(&[Category::Sharing])
    );
}

#[test]
fn short_words_need_a_short_name() {
    assert!(classify("Mua").categories.contains(&Category::Payment));
    assert!(classify("Mua hàng").categories.contains(&Category::Payment));
    assert!(classify("Mua vé xem phim").categories.is_empty());
    assert!(classify("pay").categories.contains(&Category::Payment));
    assert!(classify("Pay now").categories.contains(&Category::Payment));
    assert!(classify("How to pay for your order today")
        .categories
        .is_empty());
    assert!(classify("Payment methods help").categories.is_empty());
}

#[test]
fn buttons_match_in_the_first_words_of_a_segment() {
    let long = "one two three four five six seven pay now please";
    assert!(classify(long).categories.is_empty());
    let l = lexicon().classify_name(long, ControlKind::Link, false);
    assert!(l.informational);
}

#[test]
fn lexicon_from_json_rejects_a_bad_entry() {
    let bad = r#"{"short_word_max_words":6,"button_window_words":4,"informational_max_words":8,
        "ambiguous_stripped":[],"informational_prefixes":[],"consent_container_words":[],
        "consent_managers":[],"public_visibility_words":[],"visibility_control_words":[],
        "entries":[{"phrase":"x","class":"nope","lang":"en"}]}"#;
    assert!(Lexicon::from_json(bad).is_err());
}

#[test]
fn consent_container_needs_cookie_words_or_a_known_manager() {
    let l = lexicon();
    assert!(l.looks_like_consent_container("We use cookies to improve", ""));
    assert!(l.looks_like_consent_container("Chúng tôi sử dụng cookie", ""));
    assert!(l.looks_like_consent_container("Welcome", "id=onetrust-banner-sdk"));
    assert!(!l.looks_like_consent_container("Confirm your order and pay", ""));
}

// ---- fields ---------------------------------------------------------------------------------

fn field(f: impl FnOnce(&mut FieldAttrs)) -> FieldKind {
    let mut a = FieldAttrs::default();
    f(&mut a);
    classify_field(&a)
}

#[test]
fn field_kinds_from_attributes() {
    assert_eq!(
        field(|a| a.input_type = "password".into()),
        FieldKind::Password
    );
    assert_eq!(
        field(|a| a.autocomplete = "current-password".into()),
        FieldKind::Password
    );
    // A text input that is masked with CSS is still found by its label.
    assert_eq!(field(|a| a.label = "Mật khẩu".into()), FieldKind::Password);
    assert_eq!(
        field(|a| a.placeholder = "Nhap mat khau".into()),
        FieldKind::Password
    );
    assert_eq!(field(|a| a.name = "user_pin".into()), FieldKind::Password);
    assert_eq!(
        field(|a| a.autocomplete = "one-time-code".into()),
        FieldKind::Otp
    );
    assert_eq!(field(|a| a.label = "Mã OTP".into()), FieldKind::Otp);
    assert_eq!(field(|a| a.label = "Mã xác thực".into()), FieldKind::Otp);
    assert_eq!(
        field(|a| a.label = "Verification code".into()),
        FieldKind::Otp
    );
    assert_eq!(
        field(|a| {
            a.inputmode = "numeric".into();
            a.maxlength = Some(6);
            a.nearby_text = "Nhập mã xác minh".into();
        }),
        FieldKind::Otp
    );
    assert_eq!(
        field(|a| a.autocomplete = "cc-number".into()),
        FieldKind::CardNumber
    );
    assert_eq!(field(|a| a.id = "cardnumber".into()), FieldKind::CardNumber);
    assert_eq!(field(|a| a.label = "Số thẻ".into()), FieldKind::CardNumber);
    assert_eq!(
        field(|a| a.name = "card-number".into()),
        FieldKind::CardNumber
    );
    assert_eq!(
        field(|a| a.autocomplete = "cc-csc".into()),
        FieldKind::CardCvc
    );
    assert_eq!(field(|a| a.name = "cvv".into()), FieldKind::CardCvc);
    assert_eq!(
        field(|a| a.label = "Mã bảo mật thẻ".into()),
        FieldKind::CardCvc
    );
    assert_eq!(
        field(|a| a.label = "Security code".into()),
        FieldKind::CardCvc
    );
    assert_eq!(
        field(|a| a.autocomplete = "cc-exp-month".into()),
        FieldKind::CardExpiry
    );
    assert_eq!(
        field(|a| a.label = "Ngày hết hạn".into()),
        FieldKind::CardExpiry
    );
    assert_eq!(
        field(|a| a.label = "Số tài khoản".into()),
        FieldKind::BankAccount
    );
    assert_eq!(field(|a| a.label = "IBAN".into()), FieldKind::BankAccount);
    assert_eq!(field(|a| a.label = "Số tiền".into()), FieldKind::Amount);
    assert_eq!(field(|a| a.label = "CCCD".into()), FieldKind::NationalId);
    assert_eq!(
        field(|a| a.label = "Passport number".into()),
        FieldKind::NationalId
    );
    assert_eq!(field(|a| a.input_type = "file".into()), FieldKind::File);
    assert_eq!(field(|a| a.input_type = "email".into()), FieldKind::Email);
    assert_eq!(
        field(|a| {
            a.input_type = "checkbox".into();
            a.label = "Tôi đồng ý với điều khoản".into();
        }),
        FieldKind::TermsCheckbox
    );
    assert_eq!(field(|a| a.label = "Họ và tên".into()), FieldKind::Other);
    // F8: punctuation breaks a phrase.
    assert_eq!(
        field(|a| a.label = "Sim số - Thẻ cào".into()),
        FieldKind::Other
    );
}

#[test]
fn otp_wins_over_the_ambiguous_security_code() {
    assert_eq!(
        field(|a| {
            a.label = "Security code".into();
            a.name = "otp".into();
        }),
        FieldKind::Otp
    );
}

// ---- amounts, recipients, sites ---------------------------------------------------------------

#[test]
fn amounts_are_found() {
    assert_eq!(extract_amounts("Tổng ₫189,000 (109)"), vec!["₫189,000"]);
    assert_eq!(extract_amounts("Tặng 500.000đ cho bạn"), vec!["500.000đ"]);
    assert_eq!(extract_amounts("Pay $20 or 12 USD"), vec!["$20", "12 USD"]);
    assert!(extract_amounts("Mua 5 đơn hàng").is_empty());
    assert!(extract_amounts("no money here").is_empty());
}

#[test]
fn recipients_must_be_named_by_the_user() {
    let msgs = vec![
        "Email Nguyễn Văn An about the report".to_owned(),
        "cc bob@example.com".to_owned(),
    ];
    assert!(recipient_named("Nguyễn Văn An", &msgs));
    assert!(recipient_named("Nguyen Van An", &msgs));
    assert!(recipient_named("bob@example.com", &msgs));
    assert!(recipient_named("Bob <BOB@example.com>", &msgs));
    assert!(!recipient_named("mallory@evil.example", &msgs));
    assert!(!recipient_named("Trần Thị Bình", &msgs));
    assert!(!recipient_named("", &msgs));
}

#[test]
fn sites_and_domains() {
    assert_eq!(
        site_category("https://www.vietcombank.com.vn/x"),
        Some(SiteCategory::Banking)
    );
    assert_eq!(
        site_category("https://dichvucong.gov.vn/"),
        Some(SiteCategory::Government)
    );
    assert_eq!(
        site_category("https://a.b.gov.vn/"),
        Some(SiteCategory::Government)
    );
    assert_eq!(
        site_category("https://app.momo.vn/"),
        Some(SiteCategory::Wallet)
    );
    assert_eq!(
        site_category("https://www.coinbase.com/"),
        Some(SiteCategory::Crypto)
    );
    assert_eq!(site_category("https://notvietcombank.com.vn/"), None);
    assert_eq!(site_category("https://example.com/"), None);
    assert!(is_checkout_path("https://shop.example/cart/checkout"));
    assert!(is_checkout_path("https://shop.example/vi/thanh-toan"));
    assert!(!is_checkout_path("https://shop.example/products/paying"));
    let s = sites();
    assert_eq!(
        s.registrable_domain("www.shop.example.com.vn"),
        "example.com.vn"
    );
    assert_eq!(s.registrable_domain("a.b.example.com"), "example.com");
    assert_eq!(s.registrable_domain("localhost"), "localhost");
    assert_eq!(s.registrable_domain("127.0.0.1"), "127.0.0.1");
    let d = domains_in_text("open https://www.tiki.vn/a and check shopee.vn");
    assert!(d.contains("tiki.vn") && d.contains("shopee.vn"));
}

#[test]
fn navigation_outside_task_domains_is_unrelated() {
    let task = TaskContext {
        task_domains: ["tiki.vn".to_owned()].into(),
        ..TaskContext::default()
    };
    assert_eq!(
        navigation_category(&task, "https://evil.example/?q=1"),
        Some(Category::UnrelatedDomain)
    );
    assert_eq!(navigation_category(&task, "https://www.tiki.vn/p"), None);
}

// ---- the signals table ------------------------------------------------------------------------

fn task(url: &str) -> TaskContext {
    TaskContext {
        user_messages: vec!["Send the report to Bob".to_owned()],
        page_url: url.to_owned(),
        ..TaskContext::default()
    }
}

fn button(name: &str) -> ElementSignals {
    ElementSignals {
        role: "button".into(),
        name: name.into(),
        tag: "button".into(),
        ..ElementSignals::default()
    }
}

fn cats(
    action: ActionKind,
    el: &ElementSignals,
    form: &FormSignals,
    t: &TaskContext,
) -> BTreeSet<Category> {
    gate_categories(action, el, form, t).categories
}

#[test]
fn name_phrases_map_to_their_category() {
    let t = task("https://example.com/");
    let f = FormSignals::default();
    assert_eq!(
        cats(ActionKind::Click, &button("Xóa vĩnh viễn"), &f, &t),
        set(&[Category::Destructive])
    );
    assert_eq!(
        cats(ActionKind::Click, &button("Đổi mật khẩu"), &f, &t),
        set(&[Category::AccountSecurity])
    );
    assert!(cats(ActionKind::Click, &button("Add to cart"), &f, &t).is_empty());
    assert!(cats(ActionKind::Click, &button("Đăng nhập"), &f, &t).is_empty());
}

#[test]
fn money_form_or_checkout_amount_is_payment() {
    let t = task("https://example.com/cart");
    let mut submit = button("Continue");
    submit.is_submit = true;
    let form = FormSignals {
        fields: vec![FormField {
            kind: FieldKind::CardNumber,
            label: "Card".into(),
            filled: true,
        }],
        ..FormSignals::default()
    };
    assert!(cats(ActionKind::Click, &submit, &form, &t).contains(&Category::Payment));
    let mut priced = button("Next");
    priced.amounts = vec!["$20".into()];
    assert!(cats(ActionKind::Click, &priced, &FormSignals::default(), &t).is_empty());
    let on_checkout = task("https://example.com/checkout");
    assert!(cats(
        ActionKind::Click,
        &priced,
        &FormSignals::default(),
        &on_checkout
    )
    .contains(&Category::Payment));
}

#[test]
fn typing_credentials() {
    let t = task("https://example.com/");
    let (e, f) = (ElementSignals::default(), FormSignals::default());
    for kind in [
        FieldKind::Otp,
        FieldKind::CardNumber,
        FieldKind::CardCvc,
        FieldKind::CardExpiry,
        FieldKind::NationalId,
    ] {
        assert_eq!(
            cats(ActionKind::Type(kind), &e, &f, &t),
            set(&[Category::CredentialEntry]),
            "{kind:?}"
        );
    }
    assert!(cats(ActionKind::Type(FieldKind::Other), &e, &f, &t).is_empty());
    let out = gate_categories(ActionKind::Type(FieldKind::Password), &e, &f, &t);
    assert!(out.blocked.is_some());
}

#[test]
fn terms_checkbox_and_upload() {
    let t = task("https://example.com/");
    let cb = ElementSignals {
        input_type: Some("checkbox".into()),
        label: Some("Tôi đã đọc và đồng ý".into()),
        ..ElementSignals::default()
    };
    assert!(cats(ActionKind::Click, &cb, &FormSignals::default(), &t).contains(&Category::Terms));
    assert_eq!(
        cats(
            ActionKind::Upload,
            &ElementSignals::default(),
            &FormSignals::default(),
            &t
        ),
        set(&[Category::Upload])
    );
}

#[test]
fn send_is_unnamed_unless_the_recipient_is_known() {
    let t = task("https://mail.example/");
    let send = button("Send");
    let mut form = FormSignals {
        recipients: vec!["Bob <bob@example.com>".into()],
        ..FormSignals::default()
    };
    assert!(cats(ActionKind::Click, &send, &form, &t).is_empty());
    form.recipients = vec!["Bob".into(), "mallory@evil.example".into()];
    assert!(cats(ActionKind::Click, &send, &form, &t).contains(&Category::SendUnnamed));
    // No recipient and no conversation: fail closed.
    assert!(cats(ActionKind::Click, &send, &FormSignals::default(), &t)
        .contains(&Category::SendUnnamed));
    // A chat whose title is a named person.
    let chat = FormSignals {
        conversation_title: Some("Bob".into()),
        ..FormSignals::default()
    };
    assert!(cats(ActionKind::Click, &send, &chat, &t).is_empty());
    let other = FormSignals {
        conversation_title: Some("Alice".into()),
        ..FormSignals::default()
    };
    assert!(cats(ActionKind::Click, &send, &other, &t).contains(&Category::SendUnnamed));
}

#[test]
fn post_and_public_visibility() {
    let t = task("https://social.example/");
    assert!(cats(
        ActionKind::Click,
        &button("Đăng bài"),
        &FormSignals::default(),
        &t
    )
    .contains(&Category::PostPublic));
    let public = FormSignals {
        visibility: Some("Công khai".into()),
        ..FormSignals::default()
    };
    let mut submit = button("Save");
    submit.is_submit = true;
    assert!(cats(ActionKind::Click, &submit, &public, &t).contains(&Category::PostPublic));
    let private = FormSignals {
        visibility: Some("Only me".into()),
        ..FormSignals::default()
    };
    assert!(cats(ActionKind::Click, &submit, &private, &t).is_empty());
}

#[test]
fn changing_a_visibility_control_is_sharing() {
    let t = task("https://drive.example/");
    let sel = ElementSignals {
        role: "combobox".into(),
        name: "Chế độ hiển thị".into(),
        ..ElementSignals::default()
    };
    assert!(
        cats(ActionKind::Select, &sel, &FormSignals::default(), &t).contains(&Category::Sharing)
    );
}

#[test]
fn unnamed_control_on_a_money_page_is_payment() {
    let unnamed = ElementSignals::default();
    let f = FormSignals::default();
    assert!(cats(
        ActionKind::Click,
        &unnamed,
        &f,
        &task("https://shop.example/checkout")
    )
    .contains(&Category::Payment));
    assert!(cats(
        ActionKind::Click,
        &unnamed,
        &f,
        &task("https://www.vietcombank.com.vn/")
    )
    .contains(&Category::Payment));
    assert!(!cats(
        ActionKind::Click,
        &unnamed,
        &f,
        &task("https://example.com/")
    )
    .contains(&Category::Payment));
}

#[test]
fn writes_on_sensitive_or_read_only_sites() {
    let f = FormSignals::default();
    let bank = task("https://www.techcombank.com.vn/");
    assert!(cats(ActionKind::Click, &button("Mở"), &f, &bank).contains(&Category::SensitiveSite));
    // A plain link click is a read.
    let mut link = button("Tin tức");
    link.href = Some("/news".into());
    link.tag = "a".into();
    link.role = "link".into();
    assert!(!cats(ActionKind::Click, &link, &f, &bank).contains(&Category::SensitiveSite));
    let mut linked = task("https://other.example/page");
    linked.read_only_domains = ["other.example".to_owned()].into();
    assert!(
        cats(ActionKind::Click, &button("Save"), &f, &linked).contains(&Category::UnrelatedDomain)
    );
    assert!(!cats(ActionKind::Click, &link, &f, &linked).contains(&Category::UnrelatedDomain));
}

#[test]
fn cookie_accept_is_not_terms() {
    let t = task("https://example.com/");
    let c = lexicon().classify_name("Accept all cookies", ControlKind::Button, false);
    assert!(c.consent && c.categories.is_empty());
    assert!(cats(
        ActionKind::Click,
        &button("Chấp nhận tất cả"),
        &FormSignals::default(),
        &t
    )
    .is_empty());
    assert!(cats(
        ActionKind::Click,
        &button("I accept"),
        &FormSignals::default(),
        &t
    )
    .contains(&Category::Terms));
}

// ---- the decision -----------------------------------------------------------------------------

#[test]
fn decision_order() {
    let none = BTreeSet::new();
    let pay = set(&[Category::Payment]);
    let sharing = set(&[Category::Sharing, Category::PostPublic]);
    let mk = |gate: &'static BTreeSet<Category>| DecisionInput {
        blocked: None,
        read_only_level: false,
        tool_read_ok: false,
        gate,
        declared: Box::leak(Box::new(BTreeSet::new())),
        ext: Box::leak(Box::new(BTreeSet::new())),
        soul: Box::leak(Box::new(BTreeSet::new())),
        grants: Box::leak(Box::new(BTreeSet::new())),
    };
    let none: &'static _ = Box::leak(Box::new(none));
    let pay: &'static _ = Box::leak(Box::new(pay));
    let sharing: &'static _ = Box::leak(Box::new(sharing));

    assert_eq!(decide(&mk(none)), Decision::Allow);
    assert_eq!(decide(&mk(pay)), Decision::Ask(vec![Category::Payment]));

    let mut blocked = mk(none);
    blocked.blocked = Some("blocked origin");
    assert_eq!(decide(&blocked), Decision::Deny("blocked origin".into()));

    let mut ro = mk(none);
    ro.read_only_level = true;
    assert_eq!(decide(&ro), Decision::Deny("read only".into()));
    ro.tool_read_ok = true;
    assert_eq!(decide(&ro), Decision::Allow);

    // A grant covers sharing and post_public, but never payment.
    let grants: &'static _ = Box::leak(Box::new(set(&[
        Category::Sharing,
        Category::PostPublic,
        Category::Payment,
    ])));
    let mut g = mk(sharing);
    g.grants = grants;
    assert_eq!(
        decide(&g),
        Decision::AllowByGrant(vec![Category::Sharing, Category::PostPublic])
    );
    let mut gp = mk(pay);
    gp.grants = grants;
    assert_eq!(decide(&gp), Decision::Ask(vec![Category::Payment]));
}

#[test]
fn a_purpose_cannot_downgrade_and_credentials_are_never_granted() {
    let gate = set(&[Category::Destructive]);
    let declared = set(&[]);
    let ext = set(&[Category::CredentialEntry]);
    let soul = set(&[Category::SoulRule]);
    let grants = set(&[
        Category::Destructive,
        Category::CredentialEntry,
        Category::SoulRule,
    ]);
    let input = DecisionInput {
        blocked: None,
        read_only_level: false,
        tool_read_ok: true,
        gate: &gate,
        declared: &declared,
        ext: &ext,
        soul: &soul,
        grants: &grants,
    };
    // An empty declared risk does not remove the gate's category.
    assert_eq!(
        effective(&input),
        set(&[
            Category::Destructive,
            Category::CredentialEntry,
            Category::SoulRule
        ])
    );
    // One category is never covered, so the whole prompt is an ask with every category.
    assert_eq!(
        decide(&input),
        Decision::Ask(vec![
            Category::Destructive,
            Category::CredentialEntry,
            Category::SoulRule
        ])
    );
}

#[test]
fn categories_serialize_in_snake_case() {
    assert_eq!(
        serde_json::to_string(&Category::SendUnnamed).unwrap(),
        "\"send_unnamed\""
    );
    for c in [
        Category::Payment,
        Category::AccountSecurity,
        Category::Injection,
    ] {
        assert_eq!(
            serde_json::to_string(&c).unwrap(),
            format!("\"{}\"", c.as_str())
        );
    }
}

// ---- review fixes ---------------------------------------------------------------------------

#[test]
fn aria_name_and_visible_text_are_both_classified() {
    // I1: an aria-label that sounds harmless over a button that says "Pay now".
    let t = task("https://example.com/");
    let mut el = button("Continue");
    el.text = "Pay now".into();
    assert!(cats(ActionKind::Click, &el, &FormSignals::default(), &t).contains(&Category::Payment));
    let mut el = button("Delete account");
    el.text = "OK".into();
    assert!(
        cats(ActionKind::Click, &el, &FormSignals::default(), &t).contains(&Category::Destructive)
    );
    // The same text twice is not double counted, and harmless on both sides stays harmless.
    let mut el = button("Add to cart");
    el.text = "Add to cart".into();
    assert!(cats(ActionKind::Click, &el, &FormSignals::default(), &t).is_empty());
}

#[test]
fn a_link_exposed_as_a_button_or_without_href_is_not_exempt() {
    // I4 at the element level.
    let t = task("https://shop.example.vn/");
    let mut el = button("Thanh toán");
    el.tag = "a".into();
    el.href = Some("/info".into());
    el.in_landmark_menu = true;
    // A real nav link is exempt.
    el.role = "link".into();
    assert!(cats(ActionKind::Click, &el, &FormSignals::default(), &t).is_empty());
    el.role = "button".into();
    assert!(cats(ActionKind::Click, &el, &FormSignals::default(), &t).contains(&Category::Payment));
    el.role = "link".into();
    el.href = Some("#".into());
    assert!(cats(ActionKind::Click, &el, &FormSignals::default(), &t).contains(&Category::Payment));
}

#[test]
fn consent_in_a_dialog_needs_a_confirmed_banner() {
    // I3 at the element level.
    let t = task("https://example.com/");
    let mut el = button("Accept all");
    el.in_dialog = true;
    el.dialog_text = Some("Delete your account? This cannot be undone.".into());
    assert!(
        cats(ActionKind::Click, &el, &FormSignals::default(), &t).contains(&Category::Destructive)
    );
    el.dialog_text = Some("We use cookies".into());
    assert!(cats(ActionKind::Click, &el, &FormSignals::default(), &t).is_empty());
}

#[test]
fn new_payment_labels_are_payment() {
    for name in [
        "Book now",
        "Withdraw",
        "Submit payment",
        "Complete payment",
        "Make payment",
        "Confirm purchase",
        "Submit order",
        "Reserve",
        "Confirm and pay",
        "Đặt ngay",
        "Hoàn tất thanh toán",
        "Giữ chỗ",
    ] {
        assert!(
            classify(name).categories.contains(&Category::Payment),
            "{name}"
        );
    }
    assert!(classify("Trở về trang chủ").not_risky);
    assert!(classify("Tìm hiểu ngay").not_risky);
}

#[test]
fn invisible_characters_and_lookalike_letters_do_not_hide_a_phrase() {
    let pay = |t: &str| classify(t).categories.contains(&Category::Payment);
    assert!(pay("pay"));
    assert!(pay("p\u{200B}ay"));
    assert!(pay("pa\u{FEFF}y"));
    assert!(pay("p\u{00AD}ay"));
    // Cyrillic a, Greek rho: look like Latin.
    assert!(pay("p\u{0430}y"));
    assert!(pay("\u{03C1}ay"));
    assert_eq!(normalize("p\u{200B}ay").joined(true), "pay");
}
