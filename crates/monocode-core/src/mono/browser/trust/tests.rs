use super::audit::{typed_summary, AuditDecision, DecisionAudit, AUDIT_MAX_AGE_MS};
use super::grants::GrantBook;
use super::*;
use crate::mono::browser::risk::signals::FieldKind;
use crate::mono::browser::soul_rules;
use crate::mono::browser::MonoBrowserStore;
use serde_json::json;

fn set(cats: &[Category]) -> BTreeSet<Category> {
    cats.iter().copied().collect()
}

fn state() -> TrustState {
    TrustState::new(GrantBook::in_memory("mono-a", 1))
}

fn act() -> Config<'static> {
    static EMPTY: std::sync::OnceLock<Parsed> = std::sync::OnceLock::new();
    Config {
        level: BrowserLevel::Act,
        rules: EMPTY.get_or_init(Parsed::default),
    }
}

fn read_only() -> Config<'static> {
    Config {
        level: BrowserLevel::ReadOnly,
        ..act()
    }
}

fn button(name: &str) -> ElementSignals {
    ElementSignals {
        role: "button".into(),
        name: name.into(),
        text: name.into(),
        tag: "button".into(),
        ..Default::default()
    }
}

fn click<'a>(args: &'a Value, url: &'a str, name: &str) -> Action<'a> {
    let mut a = Action::new("browser_click", args, url);
    a.kind = Some(ActionKind::Click);
    a.element = button(name);
    a
}

// ---- levels and tiers -------------------------------------------------------------------------

#[test]
fn legacy_levels_become_act() {
    assert_eq!(
        BrowserLevel::from(BrowserTrust::AskRisky),
        BrowserLevel::Act
    );
    assert_eq!(
        BrowserLevel::from(BrowserTrust::Autonomous),
        BrowserLevel::Act
    );
    assert_eq!(
        BrowserLevel::from(BrowserTrust::ReadOnly),
        BrowserLevel::ReadOnly
    );
    for (id, level) in [
        ("read-only", Some(BrowserLevel::ReadOnly)),
        ("act", Some(BrowserLevel::Act)),
        ("ask-risky", Some(BrowserLevel::Act)),
        ("ask-every", Some(BrowserLevel::Act)),
        ("autonomous", Some(BrowserLevel::Act)),
        ("nope", None),
    ] {
        assert_eq!(BrowserLevel::from_id(id), level, "{id}");
    }
    assert!(approval_required(BrowserLevel::Act, Category::Payment));
    assert!(!approval_required(
        BrowserLevel::ReadOnly,
        Category::Payment
    ));
    let policy = crate::mono::browser::gate::Policy::new(BrowserTrust::Autonomous, Vec::new());
    assert_eq!(policy.level(), BrowserLevel::Act);
}

#[test]
fn the_tier_table_matches_the_spec() {
    let none = json!({});
    for (tool, args, tier, read_ok) in [
        ("browser_tabs", json!({"action": "list"}), Tier::Read, true),
        (
            "browser_tabs",
            json!({"action": "select"}),
            Tier::Read,
            true,
        ),
        ("browser_tabs", json!({"action": "new"}), Tier::Act, true),
        ("browser_tabs", json!({"action": "close"}), Tier::Act, false),
        ("browser_tabs", json!({"action": "adopt"}), Tier::Act, false),
        ("browser_snapshot", none.clone(), Tier::Read, true),
        ("browser_read", none.clone(), Tier::Read, true),
        (
            "browser_press_key",
            json!({"key": "PageDown"}),
            Tier::Read,
            true,
        ),
        (
            "browser_press_key",
            json!({"key": "Enter"}),
            Tier::Act,
            false,
        ),
        ("browser_navigate", none.clone(), Tier::Act, true),
        ("browser_navigate_forward", none.clone(), Tier::Act, true),
        ("browser_click", none.clone(), Tier::Act, false),
        ("browser_hover", none.clone(), Tier::Act, false),
        ("browser_file_upload", none.clone(), Tier::Always, false),
        (
            "browser_request_approval",
            none.clone(),
            Tier::Always,
            false,
        ),
        ("browser_evaluate", none.clone(), Tier::Refused, false),
        ("browser_mouse_click_xy", none.clone(), Tier::Refused, false),
    ] {
        assert_eq!(tier_of(tool, &args), tier, "{tool} {args}");
        assert_eq!(read_only_ok(tool, &args), read_ok, "{tool} {args}");
    }
}

// ---- decision table ---------------------------------------------------------------------------

#[test]
fn decision_table() {
    let s = state();
    let page = "https://shop.example/cart";
    let args = json!({});
    // (action, config, expected)
    let ok = |a: &Action<'_>, c: &Config<'_>| s.evaluate(c, a).decision;

    // An ordinary click acts without asking at Act.
    assert_eq!(
        ok(&click(&args, page, "Add to cart"), &act()),
        Decision::Allow
    );
    // A payment button asks.
    assert!(matches!(
        ok(&click(&args, page, "Pay now"), &act()),
        Decision::Ask(c) if c.contains(&Category::Payment)
    ));
    // Read only denies a click, even a harmless one.
    assert_eq!(
        ok(&click(&args, page, "Add to cart"), &read_only()),
        Decision::Deny("read only".into())
    );
    // Reads are free at both levels.
    let snap = Action::new("browser_snapshot", &args, page);
    assert_eq!(ok(&snap, &act()), Decision::Allow);
    assert_eq!(ok(&snap, &read_only()), Decision::Allow);
    // A refused tool is denied at every level.
    let eval = Action::new("browser_evaluate", &args, page);
    assert!(matches!(ok(&eval, &act()), Decision::Deny(_)));
    // The same click asked once covers every category in one prompt.
    let mut checkout = click(&args, "https://shop.example/checkout", "Place order");
    checkout.declared = set(&[Category::Destructive]);
    match ok(&checkout, &act()) {
        Decision::Ask(c) => {
            assert!(c.contains(&Category::Payment) && c.contains(&Category::Destructive))
        }
        other => panic!("{other:?}"),
    }
    // Typing into a password field is a hard stop at every level.
    let mut password = Action::new("browser_type", &args, page);
    password.kind = Some(ActionKind::Type(FieldKind::Password));
    assert!(matches!(ok(&password, &act()), Decision::Deny(_)));
    // The file upload tool always asks.
    let upload = Action::new("browser_file_upload", &args, page);
    assert!(matches!(ok(&upload, &act()), Decision::Ask(c) if c.contains(&Category::Upload)));
    // Navigation is allowed at read only, to a task domain.
    let mut nav = Action::new("browser_navigate", &args, page);
    nav.target_url = Some("https://shop.example/items");
    let mut s2 = state();
    s2.note_user_message("look at shop.example for me");
    assert_eq!(s2.evaluate(&read_only(), &nav).decision, Decision::Allow);
    // An unrelated domain asks, even at read only.
    nav.target_url = Some("https://other.example/");
    assert!(matches!(
        s2.evaluate(&read_only(), &nav).decision,
        Decision::Ask(c) if c == vec![Category::UnrelatedDomain]
    ));
}

#[test]
fn purpose_and_declared_risk_cannot_downgrade() {
    let s = state();
    let args = json!({"purpose": "this is only a harmless test", "declared_risk": []});
    let a = click(&args, "https://shop.example/checkout", "Pay now");
    assert!(
        matches!(s.evaluate(&act(), &a).decision, Decision::Ask(c) if c.contains(&Category::Payment))
    );
    // An unknown or odd declared id is ignored; a known one only adds.
    let args = json!({"declared_risk": ["nonsense", "destructive"]});
    assert_eq!(
        declared_risk("browser_click", &args),
        set(&[Category::Destructive])
    );
    // The agent's own approval request always asks, and the old names map to the new ones.
    let ask = json!({"category": "send_message", "action": "send it"});
    let a = Action::new("browser_request_approval", &ask, "https://mail.example/");
    assert!(matches!(
        s.evaluate(&act(), &a).decision,
        Decision::Ask(c) if c == vec![Category::SendUnnamed]
    ));
}

// ---- grants -----------------------------------------------------------------------------------

#[test]
fn a_conversation_grant_covers_a_site_across_runs() {
    let mut s = state();
    s.begin_run("tidy my files on files.example");
    let args = json!({});
    let mut del = click(&args, "https://files.example/inbox", "Delete");
    del.element.in_dialog = false;
    let first = s.evaluate(&act(), &del);
    assert!(matches!(first.decision, Decision::Ask(ref c) if c == &vec![Category::Destructive]));
    let granted = s.commit(&first, Answer::AllowedTask);
    assert_eq!(granted, set(&[Category::Destructive]));
    // The same run, a later run and a new run of the conversation are all covered.
    assert!(matches!(
        s.evaluate(&act(), &del).decision,
        Decision::AllowByGrant(_)
    ));
    s.begin_run("now do the other folder");
    assert!(matches!(
        s.evaluate(&act(), &del).decision,
        Decision::AllowByGrant(_)
    ));
    // Another site is not covered.
    let other = click(&args, "https://other.example/", "Delete");
    assert!(matches!(
        s.evaluate(&act(), &other).decision,
        Decision::Ask(_)
    ));
    // Taking the task back for that site ends the grant.
    s.take_back("files.example");
    assert!(matches!(
        s.evaluate(&act(), &del).decision,
        Decision::Ask(_)
    ));
}

#[test]
fn payment_and_credential_entry_are_never_granted() {
    let mut s = state();
    let args = json!({});
    let pay = click(&args, "https://shop.example/checkout", "Pay now");
    let first = s.evaluate(&act(), &pay);
    assert!(first.categories.contains(&Category::Payment));
    let granted = s.commit(&first, Answer::AllowedTask);
    assert!(granted.is_empty());
    assert!(s.grants().covered("shop.example").is_empty());
    assert!(matches!(
        s.evaluate(&act(), &pay).decision,
        Decision::Ask(_)
    ));

    // A grant for a mixed prompt keeps the other categories, never the payment.
    let mut mixed = click(&args, "https://shop.example/checkout", "Pay now");
    mixed.declared = set(&[Category::Sharing]);
    let eval = s.evaluate(&act(), &mixed);
    let granted = s.commit(&eval, Answer::AllowedTask);
    assert_eq!(granted, set(&[Category::Sharing]));
    assert!(matches!(
        s.evaluate(&act(), &mixed).decision,
        Decision::Ask(_)
    ));

    // Even a forged grant row for payment is ignored.
    let mut book = GrantBook::in_memory("mono-a", 1);
    let forced = book.grant(
        "x.example",
        &set(&[Category::Payment, Category::CredentialEntry]),
    );
    assert!(forced.is_empty() && book.covered("x.example").is_empty());
}

#[test]
fn a_card_number_in_a_typed_value_asks_and_is_never_kept() {
    let s = state();
    let args = json!({});
    let mut a = Action::new("browser_type", &args, "https://shop.example/");
    a.kind = Some(ActionKind::Type(FieldKind::Other));
    a.typed = Some("4111 1111 1111 1111");
    let eval = s.evaluate(&act(), &a);
    assert!(
        matches!(eval.decision, Decision::Ask(ref c) if c.contains(&Category::CredentialEntry))
    );
    let dump = format!("{eval:?}");
    assert!(
        !dump.contains("4111"),
        "the value must not appear in the evaluation"
    );
    a.typed = Some("hello world 12345");
    assert_eq!(s.evaluate(&act(), &a).decision, Decision::Allow);
    assert_eq!(typed_summary("Email", "a@b.co"), "typed 6 chars into Email");
    assert!(!typed_summary("Card", "4111 1111 1111 1111").contains("4111"));
}

#[test]
fn a_k_tier_tool_ignores_grants() {
    let mut s = state();
    s.grants_mut()
        .grant("files.example", &set(&[Category::Upload]));
    let args = json!({});
    let a = Action::new("browser_file_upload", &args, "https://files.example/");
    let eval = s.evaluate(&act(), &a);
    assert!(matches!(eval.decision, Decision::Ask(_)));
    // Answering "in this conversation" on a K tool grants nothing.
    assert!(s.commit(&eval, Answer::AllowedTask).is_empty());
}

#[test]
fn closing_the_conversation_ends_every_grant() {
    let mut s = state();
    s.grants_mut()
        .grant("a.example", &set(&[Category::Sharing]));
    s.grants_mut()
        .grant("b.example", &set(&[Category::Destructive]));
    assert_eq!(s.grants().list().len(), 2);
    s.end_conversation();
    assert!(s.grants().list().is_empty());
}

// ---- first visit and taint --------------------------------------------------------------------

#[test]
fn the_first_visit_to_a_sensitive_site_asks_once() {
    let mut s = state();
    s.begin_run("check my balance at vietcombank.com.vn");
    let args = json!({});
    let mut nav = Action::new("browser_navigate", &args, "about:blank");
    nav.target_url = Some("https://vietcombank.com.vn/");
    let first = s.evaluate(&act(), &nav);
    assert!(matches!(first.decision, Decision::Ask(ref c) if c == &vec![Category::SensitiveSite]));
    s.commit(&first, Answer::AllowedOnce);
    assert_eq!(s.evaluate(&act(), &nav).decision, Decision::Allow);
    // Reading a banking tab the user already had open asks the first time too.
    let mut fresh = state();
    let snap = Action::new(
        "browser_snapshot",
        &args,
        "https://www.techcombank.com.vn/home",
    );
    let eval = fresh.evaluate(&act(), &snap);
    assert!(matches!(eval.decision, Decision::Ask(ref c) if c == &vec![Category::SensitiveSite]));
    fresh.commit(&eval, Answer::AllowedOnce);
    assert_eq!(fresh.evaluate(&act(), &snap).decision, Decision::Allow);
    // A denied answer asks again.
    let mut denied = state();
    let eval = denied.evaluate(&act(), &snap);
    denied.commit(&eval, Answer::Denied);
    assert!(matches!(
        denied.evaluate(&act(), &snap).decision,
        Decision::Ask(_)
    ));
    // Every write there is at least sensitive_site, even after the first visit.
    let write = click(&args, "https://vietcombank.com.vn/transfer", "Next");
    assert!(matches!(
        s.evaluate(&act(), &write).decision,
        Decision::Ask(ref c) if c.contains(&Category::SensitiveSite)
    ));
}

#[test]
fn a_link_followed_to_a_new_domain_is_read_only_until_approved() {
    let mut s = state();
    s.begin_run("search news.example for rust");
    s.note_link_followed("https://blog.example/post");
    let args = json!({});
    let snap = Action::new("browser_snapshot", &args, "https://blog.example/post");
    assert_eq!(s.evaluate(&act(), &snap).decision, Decision::Allow);
    let write = click(
        &args,
        "https://blog.example/post",
        "Subscribe to newsletter",
    );
    let eval = s.evaluate(&act(), &write);
    assert!(
        matches!(eval.decision, Decision::Ask(ref c) if c.contains(&Category::UnrelatedDomain))
    );
    s.commit(&eval, Answer::AllowedOnce);
    assert!(!s
        .evaluate(&act(), &write)
        .categories
        .contains(&Category::UnrelatedDomain));
}

#[test]
fn data_read_on_one_site_typed_or_put_in_a_url_on_another_asks() {
    let mut s = state();
    s.begin_run("compare prices on a.example and b.example");
    s.note_read(
        "https://a.example/account",
        "Your reference is ZX-9981-4471-AB and your phone is 0912345678.",
    );
    let args = json!({});
    let mut typed = Action::new("browser_type", &args, "https://b.example/form");
    typed.kind = Some(ActionKind::Type(FieldKind::Other));
    typed.typed = Some("zx-9981-4471-ab");
    assert!(matches!(
        s.evaluate(&act(), &typed).decision,
        Decision::Ask(ref c) if c.contains(&Category::Tainted)
    ));
    // The same value typed back on the site it came from is fine.
    let mut same = Action::new("browser_type", &args, "https://a.example/form");
    same.kind = Some(ActionKind::Type(FieldKind::Other));
    same.typed = Some("zx-9981-4471-ab");
    assert_eq!(s.evaluate(&act(), &same).decision, Decision::Allow);
    // A digit run is matched through separators.
    typed.typed = Some("091 234 5678");
    assert!(s
        .evaluate(&act(), &typed)
        .categories
        .contains(&Category::Tainted));
    // In an address.
    let mut nav = Action::new("browser_navigate", &args, "https://a.example/");
    nav.target_url = Some("https://b.example/q?ref=ZX-9981-4471-AB");
    assert!(s
        .evaluate(&act(), &nav)
        .categories
        .contains(&Category::Tainted));
    // A new run forgets what was read.
    s.begin_run("next task on b.example");
    assert!(!s
        .evaluate(&act(), &typed)
        .categories
        .contains(&Category::Tainted));
}

#[test]
fn an_injection_flag_makes_every_write_ask_but_not_reads() {
    let mut s = state();
    s.flag_injection();
    let args = json!({});
    let w = click(&args, "https://a.example/", "Next");
    assert!(matches!(
        s.evaluate(&act(), &w).decision,
        Decision::Ask(ref c) if c.contains(&Category::Injection)
    ));
    let r = Action::new("browser_snapshot", &args, "https://a.example/");
    assert_eq!(s.evaluate(&act(), &r).decision, Decision::Allow);
    s.begin_run("go on");
    assert_eq!(s.evaluate(&act(), &w).decision, Decision::Allow);
}

// ---- soul rules in the decision ---------------------------------------------------------------

#[test]
fn soul_rules_add_asks_and_blocks_and_a_broken_block_asks_every_write() {
    let s = state();
    let args = json!({});
    let rules = soul_rules::parse(
        "## Browser rules\n```browser-rules\nask: click \"details\" on airbnb.com\nblock: navigate to facebook.com\n```\n",
    );
    let cfg = Config {
        level: BrowserLevel::Act,
        rules: &rules,
    };
    let book = click(&args, "https://airbnb.com/rooms/1", "View details");
    assert!(matches!(
        s.evaluate(&cfg, &book).decision,
        Decision::Ask(ref c) if c == &vec![Category::SoulRule]
    ));
    let other = click(&args, "https://airbnb.com/rooms/1", "Save");
    assert_eq!(s.evaluate(&cfg, &other).decision, Decision::Allow);
    let mut nav = Action::new("browser_navigate", &args, "https://a.example/");
    nav.target_url = Some("https://facebook.com/");
    assert!(
        matches!(s.evaluate(&cfg, &nav).decision, Decision::Deny(r) if r.contains("browser rules"))
    );

    let broken = soul_rules::parse("## Browser rules\n```browser-rules\nask: nonsense\n```\n");
    let cfg = Config {
        level: BrowserLevel::Act,
        rules: &broken,
    };
    assert!(matches!(
        s.evaluate(&cfg, &other).decision,
        Decision::Ask(ref c) if c == &vec![Category::SoulRule]
    ));
    // Reads and hovers do not ask because of it.
    let r = Action::new("browser_snapshot", &args, "https://a.example/");
    assert_eq!(s.evaluate(&cfg, &r).decision, Decision::Allow);
    let h = Action::new("browser_hover", &args, "https://a.example/");
    assert_eq!(s.evaluate(&cfg, &h).decision, Decision::Allow);
}

// ---- storage: grants across restarts, rules, audit retention ------------------------------------

struct Fixture {
    _root: tempfile::TempDir,
    _store: crate::store::Store,
    secrets: MonoBrowserStore,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let store = crate::store::Store::open(&root.path().join("monocode.db")).unwrap();
    let secrets = MonoBrowserStore::for_store(&store).unwrap();
    Fixture {
        _root: root,
        _store: store,
        secrets,
    }
}

#[test]
fn stored_grants_survive_a_restart_and_end_on_stop_and_settings_change() {
    let f = fixture();
    let mut book = GrantBook::stored(f.secrets.clone(), "mono-a", 7);
    book.grant(
        "files.example",
        &set(&[Category::Destructive, Category::Payment]),
    );
    book.grant("mail.example", &set(&[Category::SendUnnamed]));
    // A new handle on the same database (an app restart) sees them; payment was never written.
    let again = GrantBook::stored(f.secrets.clone(), "mono-a", 7);
    assert_eq!(
        again.covered("files.example"),
        set(&[Category::Destructive])
    );
    // Another conversation of the same Mono has none.
    assert!(GrantBook::stored(f.secrets.clone(), "mono-a", 8)
        .covered("files.example")
        .is_empty());
    // The overlay's Stop: one site of the Mono.
    f.secrets
        .revoke_grants("mono-a", None, Some("files.example"))
        .unwrap();
    assert!(again.covered("files.example").is_empty());
    assert_eq!(again.covered("mail.example"), set(&[Category::SendUnnamed]));
    // The toolbar Stop or a settings change: every grant of the Mono.
    f.secrets.revoke_grants("mono-a", None, None).unwrap();
    assert!(again.list().is_empty());
    // Deleting the Mono leaves nothing.
    book.grant("x.example", &set(&[Category::Sharing]));
    f.secrets.delete_mono("mono-a").unwrap();
    assert!(f.secrets.list_grants("mono-a", None).is_empty());
}

#[test]
fn confirmed_rules_are_stored_per_mono() {
    let f = fixture();
    assert!(f.secrets.confirmed_rules("mono-a").is_none());
    let lines = vec!["block: post on *".to_owned()];
    f.secrets.confirm_rules("mono-a", &lines).unwrap();
    assert_eq!(f.secrets.confirmed_rules("mono-a"), Some(lines));
    assert!(f.secrets.confirmed_rules("mono-b").is_none());
}

fn row(ts_ms: u64, shot: Option<&str>) -> DecisionAudit {
    DecisionAudit {
        ts_ms,
        session_id: 1,
        tool: "browser_click".into(),
        url: "https://a.example/x".into(),
        summary: "click Next".into(),
        tab_id: Some(12),
        tab_owner: Some("mono".into()),
        tier: "A".into(),
        categories: vec!["payment".into(), "sensitive_site".into()],
        decision: AuditDecision::AskedAllowedOnce,
        reason: "risk found".into(),
        approval_id: Some("ap-1".into()),
        screenshot: shot.map(str::to_owned),
        actor: "mono".into(),
        input: Some("content".into()),
        profile: Some("Work".into()),
    }
}

#[test]
fn audit_rows_carry_the_new_columns_and_never_a_typed_value() {
    let f = fixture();
    f.secrets
        .record_decision("mono-a", &row(1_000, None))
        .unwrap();
    let rows = f.secrets.recent_decisions("mono-a", 10);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].tier.as_deref(), Some("A"));
    assert_eq!(rows[0].categories, ["payment", "sensitive_site"]);
    assert_eq!(rows[0].decision.as_deref(), Some("asked_allowed_once"));
    assert_eq!(rows[0].actor.as_deref(), Some("mono"));
    // The old reader still works on the same rows.
    assert_eq!(f.secrets.recent("mono-a", 10).len(), 1);
}

#[test]
fn an_old_audit_table_gets_the_new_columns() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("old.db");
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch(
        "CREATE TABLE mono_browser_audit(id INTEGER PRIMARY KEY AUTOINCREMENT, mono_id TEXT NOT NULL, session_id INTEGER NOT NULL, ts INTEGER NOT NULL, tool TEXT NOT NULL, url TEXT NOT NULL, summary TEXT NOT NULL);
         INSERT INTO mono_browser_audit(mono_id,session_id,ts,tool,url,summary) VALUES('mono-a',1,5,'browser_click','u','s');",
    )
    .unwrap();
    drop(conn);
    let store = MonoBrowserStore::at(path);
    store.record_decision("mono-a", &row(10, None)).unwrap();
    let rows = store.recent_decisions("mono-a", 10);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[1].tier, None);
    assert_eq!(rows[0].tier.as_deref(), Some("A"));
}

#[test]
fn retention_prunes_rows_older_than_90_days_and_returns_their_screenshots() {
    let f = fixture();
    let now = 200 * 24 * 60 * 60 * 1000_u64;
    f.secrets
        .record_decision("mono-a", &row(now - AUDIT_MAX_AGE_MS - 1, Some("old.png")))
        .unwrap();
    f.secrets
        .record_decision(
            "mono-a",
            &row(now - AUDIT_MAX_AGE_MS + 1000, Some("edge.png")),
        )
        .unwrap();
    // The write of a new row applies retention: the old row is gone, the edge row stays.
    let pruned = f
        .secrets
        .record_decision("mono-a", &row(now, None))
        .unwrap();
    assert_eq!(pruned, ["old.png"]);
    assert_eq!(f.secrets.recent_decisions("mono-a", 10).len(), 2);
    // Another Mono's rows are untouched.
    f.secrets.record_decision("mono-b", &row(1, None)).unwrap();
    f.secrets.prune_audit("mono-a", now).unwrap();
    assert_eq!(f.secrets.recent_decisions("mono-b", 10).len(), 1);
}

#[test]
fn retention_keeps_at_most_the_newest_rows_per_mono() {
    let f = fixture();
    let conn = rusqlite::Connection::open(f.secrets.database()).unwrap();
    let now = crate::time::now_ms();
    for i in 0..12 {
        conn.execute(
            "INSERT INTO mono_browser_audit(mono_id,session_id,ts,tool,url,summary,screenshot) VALUES('mono-a',1,?1,'t','u',?2,?3)",
            rusqlite::params![now as i64, format!("row {i}"), format!("s{i}.png")],
        )
        .unwrap();
    }
    conn.execute(
        "INSERT INTO mono_browser_audit(mono_id,session_id,ts,tool,url,summary) VALUES('mono-b',1,?1,'t','u','other')",
        [now as i64],
    )
    .unwrap();
    let pruned = super::audit::prune_audit_with(&conn, "mono-a", now, AUDIT_MAX_AGE_MS, 5).unwrap();
    assert_eq!(pruned.len(), 7);
    let left: Vec<String> = conn
        .prepare("SELECT summary FROM mono_browser_audit WHERE mono_id='mono-a' ORDER BY id")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(left, ["row 7", "row 8", "row 9", "row 10", "row 11"]);
    let other: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM mono_browser_audit WHERE mono_id='mono-b'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(other, 1);
    assert_eq!(super::audit::AUDIT_MAX_ROWS, 20_000);
    assert_eq!(AUDIT_MAX_AGE_MS, 90 * 24 * 3600 * 1000);
}

#[test]
fn the_server_key_is_zptah_browser_and_the_old_prefix_still_parses() {
    assert_eq!(crate::mono::browser::MCP_SERVER, "zptah_browser");
    for name in [
        "mcp__zptah_browser__browser_click",
        "zptah_browser:browser_click",
        "mcp__playwright_browser__browser_click",
        "playwright_browser:browser_click",
    ] {
        let entry = crate::mono::browser::audit_entry(name, "{}", 1).unwrap();
        assert_eq!(entry.tool, "browser_click", "{name}");
    }
    assert!(crate::mono::browser::audit_entry("mcp__other__browser_click", "{}", 1).is_none());
}

#[test]
fn injection_tainted_and_soul_rule_are_never_covered_by_a_grant() {
    let mut book = GrantBook::in_memory("mono-a", 1);
    let all = set(&[
        Category::Injection,
        Category::Tainted,
        Category::SoulRule,
        Category::Sharing,
    ]);
    assert_eq!(book.grant("x.example", &all), set(&[Category::Sharing]));
    // A stored row for one of them is ignored on read-back.
    let f = fixture();
    let conn = rusqlite::Connection::open(f.secrets.database()).unwrap();
    for c in ["injection", "tainted", "soul_rule", "sharing"] {
        conn.execute(
            "INSERT INTO mono_browser_grants VALUES('mono-a',1,'y.example',?1,0)",
            [c],
        )
        .unwrap();
    }
    assert_eq!(
        f.secrets.grants_for("mono-a", 1, "y.example"),
        set(&[Category::Sharing])
    );
    // The decision asks even when a grant claims them.
    let none = BTreeSet::new();
    let gate = set(&[Category::Injection, Category::Tainted]);
    let input = DecisionInput {
        blocked: None,
        read_only_level: false,
        tool_read_ok: false,
        gate: &gate,
        declared: &none,
        ext: &none,
        soul: &none,
        grants: &gate,
    };
    assert!(matches!(decide(&input), Decision::Ask(_)));
}

#[test]
fn take_back_ends_the_grants_of_every_conversation_for_that_site() {
    let f = fixture();
    let mut a = TrustState::new(GrantBook::stored(f.secrets.clone(), "mono-a", 1));
    let b = GrantBook::stored(f.secrets.clone(), "mono-a", 2);
    let mut b_mut = b.clone();
    a.grants_mut()
        .grant("files.example", &set(&[Category::Destructive]));
    b_mut.grant("files.example", &set(&[Category::Destructive]));
    b_mut.grant("mail.example", &set(&[Category::Sharing]));
    a.take_back("files.example");
    assert!(b.covered("files.example").is_empty());
    assert!(a.grants().covered("files.example").is_empty());
    assert_eq!(b.covered("mail.example"), set(&[Category::Sharing]));
}

#[test]
fn audit_has_an_index_on_mono_and_time() {
    let f = fixture();
    f.secrets.record_decision("mono-a", &row(1, None)).unwrap();
    let conn = rusqlite::Connection::open(f.secrets.database()).unwrap();
    let n: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name='mono_browser_audit_mono_ts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(n, 1);
}

// ---- the approval queue (the API of the card) ----

mod queue {
    use super::super::approval::{
        all_pending, queue_for, ApprovalQueue, NoticeKind, Outcome, Request, UserAnswer,
    };
    use crate::mono::browser::copy::{ActionDesc, CardFacts, Lang};
    use crate::mono::browser::risk::Category;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn request(session: i64, offer: bool) -> Request {
        Request {
            mono_id: "mono-q".into(),
            session_id: session,
            tool: "browser_click".into(),
            tab: Some(3),
            facts: CardFacts::new(
                "Mira",
                "shop.example",
                ActionDesc::Click {
                    label: "Delete".into(),
                },
                vec![Category::Destructive],
            ),
            screenshot: None,
            offer_conversation: offer,
        }
    }

    fn ask(
        queue: &Arc<ApprovalQueue>,
        request: Request,
        timeout: Duration,
    ) -> std::thread::JoinHandle<Outcome> {
        let queue = queue.clone();
        std::thread::spawn(move || {
            queue
                .ask(request, timeout, Duration::from_millis(20), || {})
                .1
        })
    }

    fn wait_for_card(queue: &ApprovalQueue) -> super::super::approval::PendingApproval {
        for _ in 0..500 {
            if let Some(card) = queue.pending().into_iter().next() {
                return card;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("no card opened");
    }

    #[test]
    fn a_card_waits_for_its_answer_and_notifies_the_front_end() {
        let queue = ApprovalQueue::new("mono-q");
        let changes = Arc::new(AtomicUsize::new(0));
        let seen = changes.clone();
        let _sub = queue.subscribe(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        let waiting = ask(&queue, request(1, true), Duration::from_secs(5));
        let card = wait_for_card(&queue);
        assert_eq!(card.mono_id, "mono-q");
        assert_eq!(card.tab, Some(3));
        let text = card.text_in(Lang::En);
        assert_eq!(
            text.headline,
            "Mira wants to click \"Delete\" on shop.example."
        );
        assert!(text.allow_conversation.is_some());
        assert!(queue.answer(&card.id, UserAnswer::AllowConversation));
        assert_eq!(
            waiting.join().unwrap(),
            Outcome::Answered(UserAnswer::AllowConversation)
        );
        assert!(queue.pending().is_empty());
        // A second answer to the same card is refused.
        assert!(!queue.answer(&card.id, UserAnswer::Deny));
        assert!(changes.load(Ordering::SeqCst) >= 2, "opened and closed");
    }

    #[test]
    fn the_conversation_answer_is_once_when_the_card_does_not_offer_it() {
        let queue = ApprovalQueue::new("mono-q");
        let _sub = queue.subscribe(|| {});
        let waiting = ask(&queue, request(1, false), Duration::from_secs(5));
        let card = wait_for_card(&queue);
        assert!(card.text_in(Lang::Vi).allow_conversation.is_none());
        queue.answer(&card.id, UserAnswer::AllowConversation);
        assert_eq!(
            waiting.join().unwrap(),
            Outcome::Answered(UserAnswer::AllowOnce)
        );
    }

    #[test]
    fn a_card_times_out_and_is_withdrawn() {
        let queue = ApprovalQueue::new("mono-q");
        let _sub = queue.subscribe(|| {});
        let waiting = ask(&queue, request(1, true), Duration::from_millis(60));
        assert_eq!(waiting.join().unwrap(), Outcome::TimedOut);
        assert!(queue.pending().is_empty());
    }

    #[test]
    fn nobody_to_answer_is_a_withdrawn_ask_at_once() {
        let queue = ApprovalQueue::new("mono-q");
        // No front end subscribed.
        let (_, outcome) = queue.ask(
            request(1, true),
            Duration::from_secs(30),
            Duration::from_secs(1),
            || {},
        );
        assert_eq!(outcome, Outcome::Withdrawn);
        // A front end, but the run cannot be answered (plan mode).
        let _sub = queue.subscribe(|| {});
        queue.set_can_answer(false);
        let (_, outcome) = queue.ask(
            request(1, true),
            Duration::from_secs(30),
            Duration::from_secs(1),
            || {},
        );
        assert_eq!(outcome, Outcome::Withdrawn);
        queue.set_can_answer(true);
        assert!(queue.pending().is_empty());
    }

    #[test]
    fn dropping_the_subscription_stops_the_front_end_and_withdrawing_ends_waiting_asks() {
        let queue = ApprovalQueue::new("mono-q");
        let sub = queue.subscribe(|| {});
        assert!(queue.has_listener());
        let a = ask(&queue, request(1, true), Duration::from_secs(5));
        let b = ask(&queue, request(2, true), Duration::from_secs(5));
        for _ in 0..500 {
            if queue.pending().len() == 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        queue.withdraw_session(1);
        assert_eq!(a.join().unwrap(), Outcome::Withdrawn);
        assert_eq!(queue.pending().len(), 1);
        queue.withdraw_all();
        assert_eq!(b.join().unwrap(), Outcome::Withdrawn);
        drop(sub);
        assert!(!queue.has_listener());
    }

    #[test]
    fn notices_are_listed_once_and_dismissed() {
        let queue = ApprovalQueue::new("mono-q");
        queue.post_notice(
            NoticeKind::PasswordNeeded,
            Some(3),
            "Mira",
            "accounts.google.com",
        );
        queue.post_notice(
            NoticeKind::PasswordNeeded,
            Some(3),
            "Mira",
            "accounts.google.com",
        );
        let notices = queue.notices();
        assert_eq!(notices.len(), 1);
        assert_eq!(
            notices[0].text_in(Lang::En).headline,
            "Mira needs your password on accounts.google.com."
        );
        assert!(queue.dismiss_notice(&notices[0].id));
        assert!(!queue.dismiss_notice(&notices[0].id));
        assert!(queue.notices().is_empty());
    }

    #[test]
    fn the_registry_has_one_queue_per_mono_and_lists_every_open_card() {
        let a = queue_for("registry-a");
        assert!(Arc::ptr_eq(&a, &queue_for("registry-a")));
        assert!(!Arc::ptr_eq(&a, &queue_for("registry-b")));
        let _sub = a.subscribe(|| {});
        let mut req = request(1, true);
        req.mono_id = "registry-a".into();
        let waiting = ask(&a, req, Duration::from_secs(5));
        let card = wait_for_card(&a);
        assert!(all_pending().iter().any(|p| p.id == card.id));
        a.answer(&card.id, UserAnswer::Deny);
        assert_eq!(waiting.join().unwrap(), Outcome::Answered(UserAnswer::Deny));
    }
}
