use super::*;

fn facts(action: ActionDesc, categories: &[Category]) -> CardFacts {
    CardFacts::new("Mira", "shop.example", action, categories.to_vec())
}
fn click(label: &str, categories: &[Category]) -> CardFacts {
    facts(
        ActionDesc::Click {
            label: label.into(),
        },
        categories,
    )
}

fn all_cases() -> Vec<(&'static str, CardFacts)> {
    let mut payment = click("Pay 1,200,000 VND", &[Category::Payment]);
    payment.site = "shopee.vn".into();
    let mut destructive = click("Delete", &[Category::Destructive]);
    destructive.site = "mail.google.com".into();
    destructive.in_dialog = true;
    destructive.dialog_text = Some("Delete 12 conversations?".into());
    let mut security = click("Turn off 2-Step Verification", &[Category::AccountSecurity]);
    security.site = "myaccount.google.com".into();
    let mut sharing = click("Anyone with the link", &[Category::Sharing]);
    sharing.site = "docs.google.com".into();
    let mut unnamed = click("Send", &[Category::SendUnnamed]);
    unnamed.site = "mail.google.com".into();
    unnamed.recipients = vec!["an@example.com".into()];
    let mut post = click("Post", &[Category::PostPublic]);
    post.site = "facebook.com".into();
    let mut upload = facts(
        ActionDesc::Upload {
            names: vec!["invoice-2026-11.pdf".into()],
        },
        &[Category::Upload],
    );
    upload.site = "drive.google.com".into();
    let mut otp = facts(
        ActionDesc::Type {
            label: "Code".into(),
        },
        &[Category::CredentialEntry],
    );
    otp.site = "vietcombank.com.vn".into();
    otp.field = Some(FieldKind::Otp);
    let mut terms = click("I agree", &[Category::Terms]);
    terms.site = "example.com".into();
    let mut unrelated = facts(ActionDesc::Navigate, &[Category::UnrelatedDomain]);
    unrelated.site = "evil-example.net".into();
    unrelated.url = "https://evil-example.net/collect?q=...".into();
    let mut sensitive = facts(ActionDesc::Navigate, &[Category::SensitiveSite]);
    sensitive.site = "vietcombank.com.vn".into();
    sensitive.site_kind = Some(SiteCategory::Banking);
    let mut rule = click("Book", &[Category::SoulRule]);
    rule.site = "airbnb.com".into();
    rule.rule = Some("ask: click \"Book\" on airbnb.com".into());
    let tainted = click("Submit", &[Category::Tainted]);
    let injection = click("Submit", &[Category::Injection]);
    let unclear = click("Confirm", &[Category::Unclear]);
    let mut other = facts(ActionDesc::Other, &[Category::SoulRule]);
    other.agent_action = Some("Empty the trash".into());
    other.agent_details = Some("12 files".into());
    let mut flow_run = facts(
        ActionDesc::FlowRun {
            name: "order-coffee".into(),
            site: "evil-example.net".into(),
            risky: vec![
                "step 3: click \"Pay now\"".into(),
                "step 5: goes to evil-example.net".into(),
            ],
            params: vec![
                ("size".into(), "large".into()),
                ("note".into(), "no sugar please".into()),
            ],
        },
        &[Category::Payment, Category::UnrelatedDomain],
    );
    flow_run.url = "https://evil-example.net/x".into();
    let flow_save = facts(
        ActionDesc::FlowSave {
            name: "order-coffee".into(),
            site: "coffee.example".into(),
            steps: 4,
            listed: vec![
                "1: open https://coffee.example/".into(),
                "2: fill textbox \"Size\" <text, 5 characters>".into(),
            ],
            replaces: true,
        },
        &[],
    );
    vec![
        ("payment", payment),
        ("destructive", destructive),
        ("account_security", security),
        ("sharing", sharing),
        ("send_unnamed", unnamed),
        ("post_public", post),
        ("upload", upload),
        ("credential_entry", otp),
        ("terms", terms),
        ("unrelated_domain", unrelated),
        ("sensitive_site", sensitive),
        ("soul_rule", rule),
        ("tainted", tainted),
        ("injection", injection),
        ("unclear", unclear),
        ("other", other),
        ("flow_run", flow_run),
        ("flow_save", flow_save),
    ]
}

fn render(lang: Lang, f: &CardFacts) -> String {
    let c = card(lang, f);
    let mut out = c.headline.clone();
    for line in &c.lines {
        out.push_str("\n  ");
        out.push_str(line);
    }
    if let Some(p) = &c.purpose {
        out.push_str(&format!("\n  [{p}]"));
    }
    out
}

const SNAPSHOTS: &[(&str, &str, &str)] = &[
    ("en", "payment", "Mira wants to click \"Pay 1,200,000 VND\" on shopee.vn.\n  This looks like a payment."),
    ("en", "destructive", "Mira wants to click \"Delete\" in a dialog on mail.google.com.\n  The dialog says: \"Delete 12 conversations?\""),
    ("en", "account_security", "Mira wants to click \"Turn off 2-Step Verification\" on myaccount.google.com.\n  This changes how your account is protected."),
    ("en", "sharing", "Mira wants to click \"Anyone with the link\" on docs.google.com.\n  This changes who can see or open it."),
    ("en", "send_unnamed", "Mira wants to send a message to an@example.com on mail.google.com.\n  You did not mention this person."),
    ("en", "post_public", "Mira wants to click \"Post\" on facebook.com.\n  It will be visible to the public."),
    ("en", "upload", "Mira wants to upload \"invoice-2026-11.pdf\" from your computer to drive.google.com."),
    ("en", "credential_entry", "Mira wants to type a one-time code on vietcombank.com.vn.\n  This is sensitive information and it is asked every time."),
    ("en", "terms", "Mira wants to click \"I agree\" on example.com.\n  This accepts terms on your behalf."),
    ("en", "unrelated_domain", "Mira wants to open evil-example.net.\n  This site is not part of what you asked for. Full address: https://evil-example.net/collect?q=..."),
    ("en", "sensitive_site", "Mira wants to open vietcombank.com.vn, a banking site."),
    ("en", "soul_rule", "Mira wants to click \"Book\" on airbnb.com.\n  Your rule for Mira says to ask first: click \"Book\" on airbnb.com."),
    ("en", "tainted", "Mira wants to click \"Submit\" on shop.example.\n  It carries something read on another site to this one."),
    ("en", "injection", "Mira wants to click \"Submit\" on shop.example.\n  A page tried to give instructions to the Mono, so every change asks."),
    ("en", "unclear", "Mira wants to click \"Confirm\" on shop.example.\n  zptah could not read enough of this page to tell what this does."),
    ("en", "other", "Mira asks your OK on shop.example: \"Empty the trash\"\n  Details from Mira: \"12 files\""),
    ("en", "flow_run", "Mira wants to run the flow \"order-coffee\" on shop.example.\n  This looks like a payment.\n  This site is not part of what you asked for.\n  Steps that need your OK:\n  - step 3: click \"Pay now\"\n  - step 5: goes to evil-example.net\n  Parameters:\n  - size: \"large\"\n  - note: \"no sugar please\""),
    ("en", "flow_save", "Mira wants to save the flow \"order-coffee\" in Ardia.\n  This writes a flow into Ardia's flow store, which you and every profile share.\n  Site: coffee.example.\n  It saves the 4 step(s) recorded in this conversation.\n  Steps (typed values are hidden here):\n  - 1: open https://coffee.example/\n  - 2: fill textbox \"Size\" <text, 5 characters>\n  Values typed in these steps are stored in Ardia's shared flow store exactly as they were typed, unless Ardia masks them (it masks passwords only).\n  A flow with this name already exists on this site and may be replaced."),
    ("vi", "flow_run", "Mira muốn chạy luồng \"order-coffee\" trên shop.example.\n  Đây có vẻ là một khoản thanh toán.\n  Trang này không thuộc việc bạn yêu cầu.\n  Các bước cần bạn đồng ý:\n  - step 3: click \"Pay now\"\n  - step 5: goes to evil-example.net\n  Tham số:\n  - size: \"large\"\n  - note: \"no sugar please\""),
    ("vi", "flow_save", "Mira muốn lưu luồng \"order-coffee\" vào Ardia.\n  Việc này ghi một luồng vào kho luồng của Ardia, nơi bạn và mọi hồ sơ dùng chung.\n  Trang: coffee.example.\n  Nó lưu 4 bước đã ghi trong cuộc trò chuyện này.\n  Các bước (giá trị đã nhập được ẩn ở đây):\n  - 1: open https://coffee.example/\n  - 2: fill textbox \"Size\" <text, 5 characters>\n  Các giá trị đã nhập trong những bước này được lưu trong kho luồng dùng chung của Ardia đúng như đã nhập, trừ khi Ardia che chúng (Ardia chỉ che mật khẩu).\n  Đã có một luồng cùng tên trên trang này và nó có thể bị thay thế."),
    ("vi", "payment", "Mira muốn bấm \"Pay 1,200,000 VND\" trên shopee.vn.\n  Đây có vẻ là một khoản thanh toán."),
    ("vi", "destructive", "Mira muốn bấm \"Delete\" trong hộp thoại trên mail.google.com.\n  Hộp thoại ghi: \"Delete 12 conversations?\""),
    ("vi", "account_security", "Mira muốn bấm \"Turn off 2-Step Verification\" trên myaccount.google.com.\n  Thao tác này thay đổi cách bảo vệ tài khoản của bạn."),
    ("vi", "sharing", "Mira muốn bấm \"Anyone with the link\" trên docs.google.com.\n  Thao tác này thay đổi ai có thể xem hoặc mở nó."),
    ("vi", "send_unnamed", "Mira muốn gửi tin nhắn tới an@example.com trên mail.google.com.\n  Bạn chưa nhắc tới người này."),
    ("vi", "post_public", "Mira muốn bấm \"Post\" trên facebook.com.\n  Bài đăng sẽ hiển thị công khai."),
    ("vi", "upload", "Mira muốn tải \"invoice-2026-11.pdf\" từ máy bạn lên drive.google.com."),
    ("vi", "credential_entry", "Mira muốn nhập mã OTP trên vietcombank.com.vn.\n  Đây là thông tin nhạy cảm và lần nào cũng được hỏi."),
    ("vi", "terms", "Mira muốn bấm \"I agree\" trên example.com.\n  Thao tác này chấp nhận điều khoản thay bạn."),
    ("vi", "unrelated_domain", "Mira muốn mở evil-example.net.\n  Trang này không thuộc việc bạn yêu cầu. Địa chỉ đầy đủ: https://evil-example.net/collect?q=..."),
    ("vi", "sensitive_site", "Mira muốn mở vietcombank.com.vn, một trang ngân hàng."),
    ("vi", "soul_rule", "Mira muốn bấm \"Book\" trên airbnb.com.\n  Quy tắc bạn đặt cho Mira yêu cầu hỏi trước: click \"Book\" on airbnb.com."),
    ("vi", "tainted", "Mira muốn bấm \"Submit\" trên shop.example.\n  Việc này mang thông tin đọc được ở trang khác sang trang này."),
    ("vi", "injection", "Mira muốn bấm \"Submit\" trên shop.example.\n  Một trang web đã thử ra lệnh cho Mono, nên mọi thay đổi đều được hỏi."),
    ("vi", "unclear", "Mira muốn bấm \"Confirm\" trên shop.example.\n  zptah không đọc đủ nội dung trang này để biết thao tác này làm gì."),
    ("vi", "other", "Mira xin bạn đồng ý trên shop.example: \"Empty the trash\"\n  Chi tiết từ Mira: \"12 files\""),
];

#[test]
fn every_category_reads_well_in_english_and_vietnamese() {
    let cases = all_cases();
    assert_eq!(SNAPSHOTS.len(), cases.len() * 2);
    for (lang_id, name, expected) in SNAPSHOTS {
        let lang = if *lang_id == "vi" { Lang::Vi } else { Lang::En };
        let (_, facts) = cases.iter().find(|(n, _)| n == name).expect(name);
        assert_eq!(&render(lang, facts), expected, "{lang_id} {name}");
    }
}

#[test]
fn all_fifteen_categories_and_other_have_a_card() {
    let names: Vec<&str> = all_cases().iter().map(|(n, _)| *n).collect();
    for category in [
        Category::Payment,
        Category::Destructive,
        Category::AccountSecurity,
        Category::Sharing,
        Category::SendUnnamed,
        Category::PostPublic,
        Category::Upload,
        Category::CredentialEntry,
        Category::Terms,
        Category::UnrelatedDomain,
        Category::SensitiveSite,
        Category::SoulRule,
        Category::Tainted,
        Category::Injection,
        Category::Unclear,
    ] {
        assert!(names.contains(&category.as_str()), "{}", category.as_str());
    }
    assert!(names.contains(&"other"));
    // The flow cards have their own lines.
    assert!(names.contains(&"flow_run") && names.contains(&"flow_save"));
}

#[test]
fn a_flow_card_lists_at_most_six_steps_and_says_when_a_flow_is_unclear_or_has_no_parameters() {
    let risky: Vec<String> = (1..=9)
        .map(|n| format!("step {n}: click \"X{n}\""))
        .collect();
    let f = facts(
        ActionDesc::FlowRun {
            name: "big".into(),
            site: "big.example".into(),
            risky,
            params: vec![],
        },
        &[Category::Unclear],
    );
    let en = render(Lang::En, &f);
    assert!(
        en.contains("step 6: click \"X6\"") && !en.contains("X7"),
        "{en}"
    );
    assert!(
        en.contains("- and 3 more") && en.contains("Parameters: none."),
        "{en}"
    );
    assert!(
        en.contains("could not tell what some steps of this flow do"),
        "{en}"
    );
    let vi = render(Lang::Vi, &f);
    assert!(
        vi.contains("- và 3 bước nữa") && vi.contains("Tham số: không có."),
        "{vi}"
    );
    // A text the page made is one clipped line, and a quote can not close the name.
    let g = facts(
        ActionDesc::FlowRun {
            name: "a\"b".into(),
            site: "a.example".into(),
            risky: vec![format!("step 1:\n{}", "Z".repeat(300))],
            params: vec![],
        },
        &[Category::Unclear],
    );
    let text = render(Lang::En, &g);
    assert!(
        text.contains("flow \"a'b\"") && text.matches('\n').count() < 8,
        "{text}"
    );
    assert!(!text.contains(&"Z".repeat(200)), "{text}");
    // A flow with a payment is not grantable.
    assert!(card(
        Lang::En,
        &facts(
            ActionDesc::FlowRun {
                name: "p".into(),
                site: "p.example".into(),
                risky: vec![],
                params: vec![]
            },
            &[Category::Payment]
        )
    )
    .allow_conversation
    .is_none());
}

#[test]
fn several_categories_share_one_card_with_a_line_each() {
    let mut f = click(
        "Pay",
        &[
            Category::Payment,
            Category::SensitiveSite,
            Category::Destructive,
        ],
    );
    f.site_kind = Some(SiteCategory::Banking);
    f.amounts = vec!["1,200,000 VND".into()];
    f.purpose = Some("Paying for the order you asked me to place.".into());
    assert_eq!(
        render(Lang::En, &f),
        "Mira wants to click \"Pay\" on shop.example.\n  This looks like a payment.\n  It may also delete or cancel something.\n  It is also on a banking site.\n  Amount on the page: 1,200,000 VND.\n  [Mira says: \"Paying for the order you asked me to place.\"]"
    );
    f.purpose = Some("Thanh toán đơn hàng bạn nhờ đặt.".into());
    assert_eq!(
        render(Lang::Vi, &f),
        "Mira muốn bấm \"Pay\" trên shop.example.\n  Đây có vẻ là một khoản thanh toán.\n  Thao tác này cũng có thể xóa hoặc hủy một thứ gì đó.\n  Thao tác này cũng ở trên một trang ngân hàng.\n  Số tiền trên trang: 1,200,000 VND.\n  [Mira nói: \"Thanh toán đơn hàng bạn nhờ đặt.\"]"
    );
}

#[test]
fn the_buttons_follow_the_language_and_the_conversation_grant_is_offered_only_when_allowed() {
    let plain = card(Lang::En, &click("Delete", &[Category::Destructive]));
    assert_eq!(plain.allow_once, "Allow once");
    assert_eq!(plain.allow_conversation, Some("Allow in this conversation"));
    assert_eq!(plain.deny, "Don't allow");
    assert_eq!(plain.show_tab, "Show tab");
    let vi = card(Lang::Vi, &click("Xóa", &[Category::Destructive]));
    assert_eq!(vi.allow_once, "Cho phép lần này");
    assert_eq!(
        vi.allow_conversation,
        Some("Cho phép trong cuộc trò chuyện này")
    );
    assert_eq!(vi.deny, "Không cho phép");
    assert_eq!(vi.show_tab, "Xem tab");
    for never in [
        Category::Payment,
        Category::CredentialEntry,
        Category::SoulRule,
    ] {
        for lang in [Lang::En, Lang::Vi] {
            assert_eq!(
                card(lang, &click("x", &[Category::Destructive, never])).allow_conversation,
                None
            );
        }
    }
}

#[test]
fn what_the_page_said_is_one_clipped_line() {
    let long = "A".repeat(500);
    let f = click(&format!("Pay\nnow\u{7}{long}"), &[Category::Payment]);
    let c = card(Lang::En, &f);
    assert!(!c.headline.contains('\n') && !c.headline.contains('\u{7}'));
    assert!(c.headline.chars().count() < 160, "{}", c.headline);
    assert!(c.headline.contains('\u{2026}'));
    assert_eq!(clean("  a \t b  ", 10), "a b");
    // A quote mark in page text cannot close the quote around it.
    assert!(quoted("say \"hi\"", 20).matches('"').count() == 2);
}

#[test]
fn the_language_comes_from_the_locale() {
    for (tag, lang) in [
        ("vi", Lang::Vi),
        ("vi_VN.UTF-8", Lang::Vi),
        ("vi-VN", Lang::Vi),
        ("VI", Lang::Vi),
        ("en_US.UTF-8", Lang::En),
        ("fr", Lang::En),
        ("", Lang::En),
        ("vietnam", Lang::En),
    ] {
        assert_eq!(Lang::from_locale(tag), lang, "{tag}");
    }
    set_app_language(Lang::Vi);
    assert_eq!(app_language(), Lang::Vi);
    set_app_language(Lang::En);
    assert_eq!(app_language(), Lang::En);
}

#[test]
fn the_password_stop_the_banner_and_the_rule_review_have_both_languages() {
    let en = password_stop(Lang::En, "Mira", "accounts.google.com");
    assert_eq!(
        en.headline,
        "Mira needs your password on accounts.google.com."
    );
    assert_eq!(en.lines[0], "zptah never lets a Mono type passwords. Open the tab, sign in yourself, then tell the Mono to continue.");
    let vi = password_stop(Lang::Vi, "Mira", "accounts.google.com");
    assert_eq!(
        vi.headline,
        "Mira cần mật khẩu của bạn trên accounts.google.com."
    );
    assert_eq!(vi.show_tab, "Xem tab");
    let review = rule_review(
        Lang::En,
        "Mira",
        &["ask: click \"Book\" on airbnb.com".into()],
    );
    assert_eq!(review.headline, "The browser rules of Mira changed.");
    assert!(review.line.contains("airbnb.com") && review.confirm == "Confirm the new rules");
    assert_eq!(rule_review(Lang::Vi, "Mira", &[]).keep, "Giữ quy tắc cũ");
}
