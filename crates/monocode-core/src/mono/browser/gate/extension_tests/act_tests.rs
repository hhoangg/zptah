//! The act tools end to end through the gate: a scripted extension answers `act.prepare` and
//! `act.commit` (checked against `protocol.json` like the read path), a test front end answers the
//! cards, and the audit rows, grants and requests are read back.
use super::super::extension::act::ActConfig;
use super::*;
use crate::mono::browser::copy;
use crate::mono::browser::link::protocol::Event;
use crate::mono::browser::risk::Category;
use crate::mono::browser::soul_rules::{self, Parsed};
use crate::mono::browser::trust::approval::{
    ApprovalQueue, NoticeKind, PendingApproval, Subscription, UserAnswer,
};
use crate::mono::browser::trust::BrowserLevel;
use crate::mono::browser::MonoBrowserStore;
use std::collections::VecDeque;

const SHOPPING: &str = "https://shop.example/cart";

fn element(name: &str) -> Value {
    json!({
        "role": "button", "name": name, "text": name, "tag": "button", "type": "button",
        "is_submit": false, "in_dialog": false, "dialog_has_primary": false,
        "dialog_primary_names": [], "in_landmark_menu": false, "nearby_text": "", "amounts": [],
    })
}

/// What the scripted extension answers.
struct Act {
    url: String,
    title: String,
    element: Value,
    form: Value,
    ext_risk: Vec<&'static str>,
    /// A picture comes with a prepare that asked for one.
    screenshot: bool,
    prepare_error: Option<ErrorCode>,
    commit_error: Option<ErrorCode>,
    /// Where the page is after a commit.
    after_url: Option<String>,
    /// The first untrusted commit changes nothing and says so (the retry path).
    ignore_first_commit: bool,
    next: u32,
    commits: Vec<Value>,
}

impl Act {
    fn new(url: &str, name: &str) -> Arc<Mutex<Act>> {
        Arc::new(Mutex::new(Act {
            url: url.into(),
            title: "Cart".into(),
            element: element(name),
            form: json!({"fields": [], "recipients": []}),
            ext_risk: Vec::new(),
            screenshot: true,
            prepare_error: None,
            commit_error: None,
            after_url: None,
            ignore_first_commit: false,
            next: 0,
            commits: Vec::new(),
        }))
    }
}

fn install(browser: &mut Browser, act: &Arc<Mutex<Act>>) {
    let act = act.clone();
    browser.handler = Some(Box::new(move |request| {
        let method = request.method.as_deref()?;
        let params = request.params.clone()?;
        let mut a = act.lock().unwrap();
        Some(match method {
            "act.prepare" => {
                if let Some(code) = a.prepare_error {
                    return Some(Err(ProtocolError::new(code, "refused")));
                }
                a.next += 1;
                let mut v = json!({
                    "prepare_id": format!("p{}", a.next),
                    "expires_ms": crate::time::now_ms() + 30_000,
                    "digest": "d".repeat(64),
                    "url": a.url, "title": a.title, "frame_url": a.url,
                    "element": a.element, "form": a.form, "ext_risk": a.ext_risk,
                    "debugger_attached": false,
                });
                if params["needs_screenshot"] == true && a.screenshot {
                    v["screenshot"] = json!({"png_b64": encode_bytes(&png(64)), "method": "visible", "width": 800, "height": 600});
                }
                Ok(v)
            }
            "act.commit" => {
                a.commits.push(params.clone());
                if let Some(code) = a.commit_error {
                    return Some(Err(ProtocolError::new(code, "the page changed")));
                }
                let trusted = params["input"] == "trusted";
                if a.ignore_first_commit && !trusted {
                    return Some(Ok(json!({"url": a.url, "title": a.title, "effects": {
                        "method": "content", "dom_mutations": 0, "url_changed": false,
                        "focus_changed": false,
                        "hint": "Nothing visibly changed. If the page ignored it, retry once with input: trusted."}})));
                }
                let url = a.after_url.clone().unwrap_or_else(|| a.url.clone());
                a.url = url.clone();
                Ok(json!({"url": url, "title": a.title, "effects": {
                    "method": if trusted { "debugger" } else { "content" },
                    "dom_mutations": 3, "url_changed": false, "focus_changed": false}}))
            }
            "act.extend" => Ok(json!({"expires_ms": crate::time::now_ms() + 30_000})),
            "act.cancel" | "task.end" | "dialog.respond" | "upload.put" => Ok(json!({})),
            "nav.go" => Ok(json!({"url": params["url"], "title": "Opened"})),
            "nav.history" => Ok(json!({"url": a.url, "title": a.title})),
            _ => return None,
        })
    }));
}

/// A front end that answers each card with the next answer of its script.
struct FrontEnd {
    seen: Arc<Mutex<Vec<PendingApproval>>>,
    stop: Arc<AtomicBool>,
    _subscription: Subscription,
}

impl FrontEnd {
    fn start(queue: &Arc<ApprovalQueue>, answers: Vec<UserAnswer>) -> FrontEnd {
        let subscription = queue.subscribe(|| {});
        let seen: Arc<Mutex<Vec<PendingApproval>>> = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        let (queue, seen2, stop2) = (queue.clone(), seen.clone(), stop.clone());
        let mut script: VecDeque<UserAnswer> = answers.into();
        std::thread::spawn(move || {
            while !stop2.load(Ordering::SeqCst) {
                for card in queue.pending() {
                    if seen2.lock().unwrap().iter().any(|c| c.id == card.id) {
                        continue;
                    }
                    seen2.lock().unwrap().push(card.clone());
                    if let Some(answer) = script.pop_front() {
                        queue.answer(&card.id, answer);
                    }
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        FrontEnd {
            seen,
            stop,
            _subscription: subscription,
        }
    }
    fn cards(&self) -> Vec<PendingApproval> {
        self.seen.lock().unwrap().clone()
    }
}
impl Drop for FrontEnd {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct ActRig {
    rig: Rig,
    act: Arc<Mutex<Act>>,
    gate: Arc<Gate>,
    queue: Arc<ApprovalQueue>,
    store: MonoBrowserStore,
    out: tempfile::TempDir,
    _root: tempfile::TempDir,
    _core: crate::store::Store,
}

fn rig_with(url: &str, name: &str, answers: Vec<UserAnswer>) -> (ActRig, FrontEnd) {
    let act = Act::new(url, name);
    let mut browser = Browser::new(vec![tab(3, url, "Cart", Some(MONO))]);
    install(&mut browser, &act);
    let rig = start(browser);
    let root = tempfile::tempdir().unwrap();
    let core = crate::store::Store::open(&root.path().join("monocode.db")).unwrap();
    let store = MonoBrowserStore::for_store(&core).unwrap();
    let out = tempfile::tempdir().unwrap();
    rig.upstream.set_output_dir(out.path().to_path_buf());
    let queue = ApprovalQueue::new(MONO);
    rig.upstream.configure_act(
        ActConfig {
            level: BrowserLevel::Act,
            rules: Parsed::default(),
            store: Some(store.clone()),
            mono_label: "Mira".into(),
            profile: Some("Work".into()),
            approval_timeout: Duration::from_secs(5),
            queue: queue.clone(),
        },
        Origins::default(),
    );
    let gate = Gate::start_with_timeout(
        rig.upstream.clone(),
        Policy::new(BrowserTrust::AskRisky, Vec::new())
            .with_extension(true)
            .with_show_actions(false),
        "extension",
        Duration::from_secs(5),
    )
    .unwrap();
    let front = FrontEnd::start(&queue, answers);
    (
        ActRig {
            rig,
            act,
            gate,
            queue,
            store,
            out,
            _root: root,
            _core: core,
        },
        front,
    )
}

impl ActRig {
    fn click(&self) -> Value {
        call(&self.gate, "browser_click", json!({"tab": 3, "ref": "e2"}))
    }
    fn rows(&self) -> Vec<crate::mono::browser::trust::audit::DecisionRow> {
        self.store.recent_decisions(MONO, 50)
    }
    fn reconfigure(&self, change: impl FnOnce(&mut ActConfig)) {
        let mut config = self.rig.upstream.act_config();
        change(&mut config);
        let origins = Origins::default();
        self.rig.upstream.configure_act(config, origins);
    }
}

// ---- allow ----

#[test]
fn a_plain_click_runs_without_asking_and_is_audited_as_auto() {
    let (r, front) = rig_with(SHOPPING, "Next", vec![]);
    let text = ok_text(r.click());
    assert!(text.contains("Ran click on \"Next\""), "{text}");
    assert!(
        text.contains("- Page URL: https://shop.example/cart"),
        "{text}"
    );
    assert_eq!(r.rig.requests("act.prepare").len(), 1);
    assert!(r.rig.requests("act.cancel").is_empty());
    let commits = r.rig.requests("act.commit");
    assert_eq!(commits.len(), 1);
    assert!(commits[0]
        .params
        .as_ref()
        .unwrap()
        .get("approval")
        .is_none());
    assert!(front.cards().is_empty());
    let rows = r.rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].decision.as_deref(), Some("auto"));
    assert_eq!(rows[0].tool, "browser_click");
    assert_eq!(rows[0].tier.as_deref(), Some("A"));
    assert_eq!(rows[0].actor.as_deref(), Some("mono"));
    r.rig.finish();
}

#[test]
fn hovering_never_asks_even_when_the_extension_saw_a_risk() {
    let (r, front) = rig_with(SHOPPING, "Delete account", vec![]);
    r.act.lock().unwrap().ext_risk = vec!["destructive"];
    let text = ok_text(call(
        &r.gate,
        "browser_hover",
        json!({"tab": 3, "ref": "e2"}),
    ));
    assert!(text.contains("Ran hover"), "{text}");
    assert!(front.cards().is_empty());
    // Hovering never claims an approval: the extension refuses a risky one itself.
    let commits = r.rig.requests("act.commit");
    assert!(commits[0]
        .params
        .as_ref()
        .unwrap()
        .get("approval")
        .is_none());
    r.rig.finish();
}

#[test]
fn a_click_that_changed_nothing_is_retried_once_through_trusted_input() {
    let (r, _front) = rig_with(SHOPPING, "Next", vec![]);
    r.act.lock().unwrap().ignore_first_commit = true;
    let text = ok_text(r.click());
    let commits = r.act.lock().unwrap().commits.clone();
    assert_eq!(commits.len(), 2);
    assert!(commits[0].get("input").is_none());
    assert_eq!(commits[1]["input"], "trusted");
    assert!(text.contains("Page changes seen: 3"), "{text}");
    assert_eq!(r.rows()[0].tool, "browser_click");
    r.rig.finish();
}

// ---- ask ----

#[test]
fn a_payment_asks_with_the_picture_and_commits_bound_to_the_digest() {
    let (r, front) = rig_with(SHOPPING, "Pay 1,200,000 VND", vec![UserAnswer::AllowOnce]);
    {
        let mut a = r.act.lock().unwrap();
        a.ext_risk = vec!["payment"];
        a.element["is_submit"] = json!(true);
        a.element["amounts"] = json!(["1,200,000 VND"]);
    }
    let text = ok_text(r.click());
    assert!(text.contains("Ran click"), "{text}");
    // The first prepare had no picture, so the card's prepare asked for one.
    let prepares = r.rig.requests("act.prepare");
    assert_eq!(prepares.len(), 2);
    assert!(prepares[0]
        .params
        .as_ref()
        .unwrap()
        .get("needs_screenshot")
        .is_none());
    assert_eq!(
        prepares[1].params.as_ref().unwrap()["needs_screenshot"],
        true
    );
    assert_eq!(
        r.rig.requests("act.cancel").len(),
        1,
        "the first prepare was cancelled"
    );

    let cards = front.cards();
    assert_eq!(cards.len(), 1);
    let card = &cards[0];
    assert_eq!(card.facts.categories, vec![Category::Payment]);
    assert!(!card.offer_conversation, "a payment is never granted");
    let en = card.text_in(copy::Lang::En);
    assert_eq!(
        en.headline,
        "Mira wants to click \"Pay 1,200,000 VND\" on shop.example."
    );
    assert!(
        en.lines.contains(&"This looks like a payment.".to_owned()),
        "{en:?}"
    );
    assert_eq!(en.allow_conversation, None);
    let shot = card.screenshot.as_ref().expect("a picture");
    assert!(shot.path.exists() && shot.path.starts_with(r.out.path()));

    let commit = &r.rig.requests("act.commit")[0].params.clone().unwrap();
    assert_eq!(commit["prepare_id"], "p2");
    assert_eq!(commit["approval"]["digest"], "d".repeat(64));
    assert_eq!(commit["approval"]["approval_id"], card.id);
    assert!(commit["approval"]["categories"]
        .as_array()
        .unwrap()
        .contains(&json!("payment")));

    let rows = r.rows();
    assert_eq!(rows[0].decision.as_deref(), Some("asked_allowed_once"));
    assert_eq!(rows[0].categories, ["payment"]);
    assert!(r.store.audit_screenshots(MONO).contains(&shot.file_name));
    r.rig.finish();
}

#[test]
fn a_denied_card_does_nothing_and_says_so() {
    let (r, _front) = rig_with(SHOPPING, "Delete", vec![UserAnswer::Deny]);
    r.act.lock().unwrap().ext_risk = vec!["destructive"];
    let result = r.click();
    assert!(is_error(&result));
    assert!(
        text_of(&result).contains("Denied by the user"),
        "{}",
        text_of(&result)
    );
    assert!(r.rig.requests("act.commit").is_empty());
    assert!(!r.rig.requests("act.cancel").is_empty());
    assert_eq!(r.rows()[0].decision.as_deref(), Some("asked_denied"));
    r.rig.finish();
}

#[test]
fn an_unanswered_card_times_out_as_a_deny() {
    let (r, _front) = rig_with(SHOPPING, "Delete", vec![]);
    r.act.lock().unwrap().ext_risk = vec!["destructive"];
    r.reconfigure(|c| c.approval_timeout = Duration::from_millis(200));
    let result = r.click();
    assert!(is_error(&result));
    assert!(
        text_of(&result).contains("did not answer in time"),
        "{}",
        text_of(&result)
    );
    assert!(r.rig.requests("act.commit").is_empty());
    assert_eq!(r.rows()[0].decision.as_deref(), Some("asked_timeout"));
    assert!(
        r.queue.pending().is_empty(),
        "a timed out card is withdrawn"
    );
    r.rig.finish();
}

#[test]
fn a_card_nobody_can_answer_is_a_deny_at_once() {
    let (r, front) = rig_with(SHOPPING, "Delete", vec![UserAnswer::AllowOnce]);
    r.act.lock().unwrap().ext_risk = vec!["destructive"];
    r.queue.set_can_answer(false);
    let result = r.click();
    assert!(is_error(&result));
    assert!(text_of(&result).contains("nobody could answer"));
    assert!(front.cards().is_empty());
    assert!(r.rig.requests("act.commit").is_empty());
    r.rig.finish();
}

#[test]
fn without_a_front_end_an_ask_is_denied_without_waiting() {
    let act = Act::new(SHOPPING, "Delete");
    act.lock().unwrap().ext_risk = vec!["destructive"];
    let mut browser = Browser::new(vec![tab(3, SHOPPING, "Cart", Some(MONO))]);
    install(&mut browser, &act);
    let rig = start(browser);
    rig.upstream.configure_act(
        ActConfig {
            level: BrowserLevel::Act,
            rules: Parsed::default(),
            store: None,
            mono_label: "Mira".into(),
            profile: None,
            approval_timeout: Duration::from_secs(30),
            queue: ApprovalQueue::new(MONO),
        },
        Origins::default(),
    );
    let gate = Gate::start_with_timeout(
        rig.upstream.clone(),
        Policy::new(BrowserTrust::AskRisky, Vec::new()).with_extension(true),
        "extension",
        Duration::from_secs(5),
    )
    .unwrap();
    let started = std::time::Instant::now();
    let result = call(&gate, "browser_click", json!({"tab": 3, "ref": "e2"}));
    assert!(is_error(&result) && started.elapsed() < Duration::from_secs(5));
    rig.finish();
}

#[test]
fn a_stale_prepare_is_reported_and_audited_never_retried() {
    let (r, _front) = rig_with(SHOPPING, "Next", vec![]);
    r.act.lock().unwrap().commit_error = Some(ErrorCode::StalePrepare);
    let result = r.click();
    assert!(is_error(&result));
    assert!(
        text_of(&result).contains("page changed"),
        "{}",
        text_of(&result)
    );
    assert_eq!(r.act.lock().unwrap().commits.len(), 1, "no blind retry");
    let rows = r.rows();
    assert!(rows[0]
        .reason
        .as_deref()
        .unwrap()
        .contains("E_STALE_PREPARE"));
    r.rig.finish();
}

// ---- grants ----

#[test]
fn payment_is_never_granted_and_asks_every_time() {
    let (r, front) = rig_with(
        SHOPPING,
        "Pay now",
        vec![UserAnswer::AllowConversation, UserAnswer::AllowConversation],
    );
    r.act.lock().unwrap().ext_risk = vec!["payment"];
    ok_text(r.click());
    ok_text(r.click());
    let cards = front.cards();
    assert_eq!(cards.len(), 2, "the second payment asked again");
    for card in &cards {
        assert_eq!(card.text_in(copy::Lang::Vi).allow_conversation, None);
    }
    assert!(r.store.list_grants(MONO, None).is_empty());
    assert!(r
        .rows()
        .iter()
        .all(|row| row.decision.as_deref() == Some("asked_allowed_once")));
    r.rig.finish();
}

#[test]
fn allow_in_this_conversation_covers_the_next_destructive_click_on_that_site() {
    let (r, front) = rig_with(SHOPPING, "Remove item", vec![UserAnswer::AllowConversation]);
    r.act.lock().unwrap().ext_risk = vec!["destructive"];
    ok_text(r.click());
    ok_text(r.click());
    assert_eq!(front.cards().len(), 1, "the second click was covered");
    assert!(front.cards()[0].offer_conversation);
    assert_eq!(
        r.store.list_grants(MONO, None),
        vec![("shop.example".to_owned(), Category::Destructive)]
    );
    let rows = r.rows();
    assert_eq!(rows[0].decision.as_deref(), Some("auto"));
    assert_eq!(rows[0].reason.as_deref(), Some("covered by a grant"));
    assert_eq!(rows[1].decision.as_deref(), Some("asked_allowed_task"));
    // The covered commit still tells the extension what was approved.
    let second = &r.rig.requests("act.commit")[1].params.clone().unwrap();
    assert_eq!(second["approval"]["categories"], json!(["destructive"]));
    // The user revoking it (Settings) makes the next one ask again.
    r.store.revoke_grants(MONO, None, None).unwrap();
    r.reconfigure(|_| {});
    r.rig.finish();
}

#[test]
fn the_toolbar_stop_ends_the_grants_and_withdraws_open_cards() {
    let (r, _front) = rig_with(SHOPPING, "Remove item", vec![UserAnswer::AllowConversation]);
    r.act.lock().unwrap().ext_risk = vec!["destructive"];
    ok_text(r.click());
    assert!(!r.store.list_grants(MONO, None).is_empty());
    // (`dispatch_event` finds the upstreams of a link by its instance id; every test rig shares
    // one id, so the event goes straight to this rig's upstream.)
    r.rig.upstream.note_event(&Envelope::event(
        "e-1",
        Event::Stop,
        json!({"by": "toolbar"}),
    ));
    assert!(r.rig.upstream.stopped());
    assert!(r.store.list_grants(MONO, None).is_empty());
    assert_eq!(r.rows()[0].decision.as_deref(), Some("stopped"));
    r.rig.finish();
}

// ---- the categories only the app knows ----

#[test]
fn a_message_to_someone_the_user_did_not_name_asks_as_send_unnamed() {
    let (r, front) = rig_with(
        "https://mail.example/compose",
        "Send",
        vec![UserAnswer::AllowOnce],
    );
    r.act.lock().unwrap().form = json!({"fields": [], "recipients": ["an@example.com"], "conversation_title": "New message"});
    ok_text(r.click());
    let card = &front.cards()[0];
    assert!(card.facts.categories.contains(&Category::SendUnnamed));
    let en = card.text_in(copy::Lang::En);
    assert_eq!(
        en.headline,
        "Mira wants to send a message to an@example.com on mail.example."
    );
    assert_eq!(en.lines, ["You did not mention this person."]);
    r.rig.finish();
}

#[test]
fn a_write_on_a_banking_site_asks_as_sensitive_site() {
    let (r, front) = rig_with(
        "https://online.vietcombank.com.vn/home",
        "Continue",
        vec![UserAnswer::AllowOnce],
    );
    ok_text(r.click());
    let card = &front.cards()[0];
    assert_eq!(card.facts.categories, vec![Category::SensitiveSite]);
    assert!(card.text_in(copy::Lang::En).lines[0].contains("banking site"));
    r.rig.finish();
}

#[test]
fn a_page_reached_by_a_link_is_read_only_until_the_user_approves_it() {
    let (r, front) = rig_with(SHOPPING, "Open partner", vec![UserAnswer::AllowOnce]);
    // The first click follows a link to another site; the second tries to act there.
    r.act.lock().unwrap().after_url = Some("https://partner.example/page".into());
    ok_text(r.click());
    assert!(front.cards().is_empty());
    ok_text(r.click());
    let cards = front.cards();
    assert_eq!(cards.len(), 1, "acting on the unrelated site asked");
    assert!(cards[0]
        .facts
        .categories
        .contains(&Category::UnrelatedDomain));
    r.rig.finish();
}

#[test]
fn a_domain_the_user_named_is_part_of_the_task() {
    let (r, front) = rig_with(SHOPPING, "Open partner", vec![]);
    let route = r.gate.route_for(Some(9));
    r.gate
        .begin_turn(&route, "buy it on partner.example please");
    r.act.lock().unwrap().after_url = Some("https://partner.example/page".into());
    ok_text(call_on(
        &r.gate,
        &route,
        "browser_click",
        json!({"tab": 3, "ref": "e2"}),
    ));
    ok_text(call_on(
        &r.gate,
        &route,
        "browser_click",
        json!({"tab": 3, "ref": "e2"}),
    ));
    assert!(front.cards().is_empty());
    r.rig.finish();
}

fn call_on(gate: &Gate, route: &str, name: &str, args: Value) -> Value {
    gate.handle(
        route,
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": name, "arguments": args}}),
    )
    .expect("a reply")["result"]
        .clone()
}

// ---- stops ----

#[test]
fn a_password_field_is_a_stop_not_a_question() {
    let (r, front) = rig_with(
        "https://accounts.example/login",
        "Password",
        vec![UserAnswer::AllowOnce],
    );
    r.act.lock().unwrap().prepare_error = Some(ErrorCode::PasswordField);
    let result = call(
        &r.gate,
        "browser_type",
        json!({"tab": 3, "ref": "e2", "text": "hunter2"}),
    );
    assert!(is_error(&result));
    assert!(text_of(&result).contains("never allowed"));
    assert!(front.cards().is_empty());
    let notices = r.queue.notices();
    assert_eq!(notices.len(), 1);
    assert_eq!(notices[0].kind, NoticeKind::PasswordNeeded);
    assert!(notices[0]
        .text_in(copy::Lang::Vi)
        .headline
        .contains("cần mật khẩu"));
    let rows = r.rows();
    assert_eq!(rows[0].decision.as_deref(), Some("invariant"));
    // The typed value is in no row.
    assert!(!format!("{rows:?}").contains("hunter2"));
    r.rig.finish();
}

#[test]
fn a_one_time_code_always_asks_and_is_never_granted() {
    let (r, front) = rig_with(
        "https://bank.example/otp",
        "Code",
        vec![UserAnswer::AllowConversation],
    );
    {
        let mut a = r.act.lock().unwrap();
        a.element["autocomplete"] = json!("one-time-code");
        a.element["role"] = json!("textbox");
        a.ext_risk = vec!["credential_entry"];
    }
    ok_text(call(
        &r.gate,
        "browser_type",
        json!({"tab": 3, "ref": "e2", "text": "123456"}),
    ));
    let card = &front.cards()[0];
    assert!(card.facts.categories.contains(&Category::CredentialEntry));
    assert!(!card.offer_conversation);
    assert!(card
        .text_in(copy::Lang::En)
        .headline
        .contains("one-time code"));
    // The audit keeps the length, not the code.
    let rows = r.rows();
    assert!(!format!("{rows:?}").contains("123456"));
    r.rig.finish();
}

#[test]
fn a_read_only_mono_cannot_act() {
    let (r, _front) = rig_with(SHOPPING, "Next", vec![]);
    r.reconfigure(|c| c.level = BrowserLevel::ReadOnly);
    let result = r.click();
    assert!(is_error(&result));
    assert!(
        text_of(&result).contains("can only read"),
        "{}",
        text_of(&result)
    );
    assert!(r.rig.requests("act.prepare").is_empty());
    assert_eq!(r.rows()[0].decision.as_deref(), Some("blocked"));
    r.rig.finish();
}

#[test]
fn a_soul_rule_that_blocks_denies_and_one_that_asks_asks() {
    let (r, front) = rig_with(
        "https://airbnb.example/room",
        "Book",
        vec![UserAnswer::AllowOnce],
    );
    let soul = "## Browser rules\n\n```browser-rules\nask: click \"Book\" on airbnb.example\n```\n";
    r.reconfigure(|c| c.rules = soul_rules::parse(soul));
    ok_text(r.click());
    let card = &front.cards()[0];
    assert!(card.facts.categories.contains(&Category::SoulRule));
    assert!(
        card.text_in(copy::Lang::En).lines[0].starts_with("Your rule for Mira says to ask first:")
    );
    let blocked =
        "## Browser rules\n\n```browser-rules\nblock: click \"Book\" on airbnb.example\n```\n";
    r.reconfigure(|c| c.rules = soul_rules::parse(blocked));
    let result = r.click();
    assert!(is_error(&result) && text_of(&result).contains("blocked by your browser rules"));
    r.rig.finish();
}

// ---- other tools ----

#[test]
fn an_upload_always_asks_with_the_file_names_and_streams_the_files_first() {
    let (r, front) = rig_with(
        "https://drive.example/up",
        "Upload",
        vec![UserAnswer::AllowConversation],
    );
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("invoice-2026-11.pdf");
    std::fs::write(&path, b"%PDF-fake").unwrap();
    ok_text(call(
        &r.gate,
        "browser_file_upload",
        json!({"tab": 3, "ref": "e2", "paths": [path.to_string_lossy()]}),
    ));
    let puts = r.rig.requests("upload.put");
    assert_eq!(puts.len(), 1);
    let put = puts[0].params.as_ref().unwrap();
    assert_eq!(put["seq"], 0);
    assert_eq!(put["last"], true);
    let bytes = B64.decode(put["bytes_b64"].as_str().unwrap()).unwrap();
    assert_eq!(&bytes[..4], &9u32.to_be_bytes());
    assert_eq!(&bytes[4..], b"%PDF-fake");
    // The upload was sent before the prepare, and the action names the file.
    let prepare = &r.rig.requests("act.prepare")[0].params.clone().unwrap();
    assert_eq!(prepare["action"]["kind"], "upload");
    assert_eq!(prepare["action"]["names"], json!(["invoice-2026-11.pdf"]));
    assert_eq!(prepare["action"]["upload_id"], put["upload_id"]);
    let card = &front.cards()[0];
    assert!(card.facts.categories.contains(&Category::Upload));
    assert!(!card.offer_conversation, "an upload always asks");
    assert!(card
        .text_in(copy::Lang::En)
        .headline
        .contains("\"invoice-2026-11.pdf\""));
    assert!(r.store.list_grants(MONO, None).is_empty());
    r.rig.finish();
}

#[test]
fn an_upload_refuses_credential_files_directories_and_relative_paths() {
    let (r, front) = rig_with("https://drive.example/up", "Upload", vec![]);
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".ssh")).unwrap();
    std::fs::write(home.path().join(".ssh/id_ed25519"), b"x").unwrap();
    let secret = home.path().join(".ssh/id_ed25519");
    let refused = super::super::extension::act::read_upload_files(&[secret], Some(home.path()));
    assert!(refused.unwrap_err().contains("never uploaded"));
    let dir = call(
        &r.gate,
        "browser_file_upload",
        json!({"tab": 3, "ref": "e2", "paths": [home.path().to_string_lossy()]}),
    );
    assert!(is_error(&dir) && text_of(&dir).contains("not a regular file"));
    let relative = call(
        &r.gate,
        "browser_file_upload",
        json!({"tab": 3, "ref": "e2", "paths": ["a.txt"]}),
    );
    assert!(is_error(&relative) && text_of(&relative).contains("absolute"));
    assert!(r.rig.requests("upload.put").is_empty() && front.cards().is_empty());
    r.rig.finish();
}

#[test]
fn navigating_to_a_site_the_user_did_not_name_asks_with_the_full_address() {
    let (r, front) = rig_with(SHOPPING, "x", vec![UserAnswer::AllowOnce, UserAnswer::Deny]);
    let route = r.gate.route_for(Some(4));
    r.gate.begin_turn(&route, "look at shop.example");
    let url = "https://evil-example.net/collect?q=1";
    let ok = call_on(
        &r.gate,
        &route,
        "browser_navigate",
        json!({"tab": 3, "url": url}),
    );
    assert!(!is_error(&ok), "{}", text_of(&ok));
    assert_eq!(r.rig.requests("nav.go").len(), 1);
    let card = &front.cards()[0];
    assert!(card.facts.categories.contains(&Category::UnrelatedDomain));
    let en = card.text_in(copy::Lang::En);
    assert_eq!(en.headline, "Mira wants to open evil-example.net.");
    assert!(en.lines[0].contains(url), "{en:?}");
    // A second, denied one opens nothing.
    let denied = call_on(
        &r.gate,
        &route,
        "browser_navigate",
        json!({"tab": 3, "url": "https://other-evil.example/"}),
    );
    assert!(is_error(&denied));
    assert_eq!(r.rig.requests("nav.go").len(), 1);
    // A scheme that is not http(s) never reaches the extension.
    let bad = call_on(
        &r.gate,
        &route,
        "browser_navigate",
        json!({"tab": 3, "url": "javascript:alert(1)"}),
    );
    assert!(is_error(&bad));
    r.rig.finish();
}

#[test]
fn the_agent_s_own_request_for_approval_uses_its_words_for_category_other() {
    let (r, front) = rig_with(SHOPPING, "x", vec![UserAnswer::AllowOnce]);
    let reply = ok_text(call(
        &r.gate,
        "browser_request_approval",
        json!({"category": "other", "action": "Empty the trash", "details": "12 files", "url": SHOPPING}),
    ));
    assert!(reply.contains("Approved by the user"));
    let card = &front.cards()[0];
    let en = card.text_in(copy::Lang::En);
    assert!(en.headline.contains("\"Empty the trash\""), "{en:?}");
    assert!(en.lines.iter().any(|l| l.contains("12 files")));
    assert!(!card.offer_conversation, "tier K always asks");
    assert_eq!(r.rows()[0].tier.as_deref(), Some("K"));
    r.rig.finish();
}

#[test]
fn the_run_ending_tells_the_extension_and_withdraws_its_cards() {
    let (r, _front) = rig_with(SHOPPING, "Delete", vec![]);
    let route = r.gate.route_for(Some(6));
    let attached = r.gate.attach(&route);
    drop(attached);
    let started = std::time::Instant::now();
    while r.rig.requests("task.end").is_empty() && started.elapsed() < WAIT {
        std::thread::sleep(Duration::from_millis(10));
    }
    let ends = r.rig.requests("task.end");
    assert_eq!(ends.len(), 1);
    assert_eq!(ends[0].mono.as_deref(), Some(MONO));
    r.rig.finish();
}

#[test]
fn the_gate_lists_the_act_tools_for_an_extension_mono_and_never_code() {
    let (r, _front) = rig_with(SHOPPING, "x", vec![]);
    let reply = r
        .gate
        .handle(
            SHARED_ROUTE,
            json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}}),
        )
        .unwrap();
    let names: Vec<String> = reply["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_owned())
        .collect();
    for tool in [
        "browser_click",
        "browser_type",
        "browser_fill_form",
        "browser_select_option",
        "browser_press_key",
        "browser_hover",
        "browser_drag",
        "browser_mouse_wheel",
        "browser_navigate",
        "browser_navigate_back",
        "browser_navigate_forward",
        "browser_handle_dialog",
        "browser_file_upload",
        "browser_request_approval",
    ] {
        assert!(
            names.contains(&tool.to_owned()),
            "{tool} is listed: {names:?}"
        );
    }
    assert!(!names
        .iter()
        .any(|n| n == "browser_evaluate" || n == "browser_run_code_unsafe"));
    r.rig.finish();
}

// ---- screenshots of audit rows (spec 8.5) ----

#[test]
fn the_sweep_keeps_the_pictures_of_audit_rows_and_pruned_rows_lose_theirs() {
    use crate::mono::browser::output;
    use crate::mono::browser::trust::audit::AUDIT_MAX_AGE_MS;
    let dir = tempfile::tempdir().unwrap();
    let kept = dir.path().join("approval-kept.png");
    let other = dir.path().join("page-1.yml");
    std::fs::write(&kept, b"x").unwrap();
    std::fs::write(&other, b"y").unwrap();
    // Everything is older than the sweep's age: only the file a row points to survives.
    let future = std::time::SystemTime::now() + Duration::from_secs(24 * 3600);
    let keep: std::collections::HashSet<String> = ["approval-kept.png".to_owned()].into();
    output::sweep_keeping_with(
        dir.path(),
        future,
        output::MAX_AGE,
        output::MAX_FILES,
        &keep,
    );
    assert!(kept.exists() && !other.exists());

    // The upstream deletes the pictures of the rows retention removed.
    let (r, _front) = rig_with(SHOPPING, "Next", vec![]);
    let old = r.out.path().join("approval-old.png");
    std::fs::write(&old, b"x").unwrap();
    let now = crate::time::now_ms();
    let mut row = crate::mono::browser::trust::audit::DecisionAudit {
        ts_ms: now - AUDIT_MAX_AGE_MS - 10,
        session_id: 0,
        tool: "browser_click".into(),
        url: "u".into(),
        summary: "s".into(),
        tab_id: None,
        tab_owner: None,
        tier: "A".into(),
        categories: vec![],
        decision: crate::mono::browser::trust::audit::AuditDecision::Auto,
        reason: "r".into(),
        approval_id: None,
        screenshot: Some("approval-old.png".into()),
        actor: "mono".into(),
        input: None,
        profile: None,
    };
    r.store.record_decision(MONO, &row).unwrap();
    row.ts_ms = now;
    row.screenshot = None;
    ok_text(r.click());
    assert!(!old.exists(), "the picture of a pruned row was deleted");
    assert_eq!(
        output::delete_files(r.out.path(), &["../escape.png".into(), "nope.png".into()]),
        0
    );
    r.rig.finish();
}

#[test]
fn keys_pressed_one_by_one_never_reach_the_audit_and_still_feed_the_card_check() {
    let (r, front) = rig_with(
        "https://shop.example/pay",
        "Card number",
        vec![UserAnswer::AllowOnce],
    );
    {
        let mut a = r.act.lock().unwrap();
        a.element["role"] = json!("textbox");
        a.element["tag"] = json!("input");
        a.element["type"] = json!("text");
    }
    for digit in "4111111111111111".chars() {
        let key = digit.to_string();
        let result = call(&r.gate, "browser_press_key", json!({"tab": 3, "key": key}));
        if is_error(&result) {
            break;
        }
    }
    let rows = r.rows();
    assert!(!rows.is_empty());
    for row in &rows {
        assert!(
            row.tool != "browser_press_key" || !format!("{row:?}").contains("press 4"),
            "{row:?}"
        );
    }
    let entries = r.store.recent(MONO, 50);
    let dump = format!("{rows:?} {entries:?}");
    assert!(
        !dump.contains("4111") && !dump.contains("press 4"),
        "{dump}"
    );
    assert!(dump.contains("pressed a text key into"), "{dump}");
    // The last digits made a card number: the card check asked.
    assert!(front
        .cards()
        .iter()
        .any(|c| c.facts.categories.contains(&Category::CredentialEntry)));
    r.rig.finish();
}

#[test]
fn an_upload_is_refused_at_read_only_before_any_byte_is_sent() {
    let (r, _front) = rig_with("https://drive.example/up", "Upload", vec![]);
    r.reconfigure(|c| c.level = BrowserLevel::ReadOnly);
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("a.txt");
    std::fs::write(&path, b"x").unwrap();
    let result = call(
        &r.gate,
        "browser_file_upload",
        json!({"tab": 3, "ref": "e2", "paths": [path.to_string_lossy()]}),
    );
    assert!(is_error(&result));
    assert!(r.rig.requests("upload.put").is_empty());
    r.rig.finish();
}

#[test]
fn accepting_a_dialog_whose_text_is_unknown_asks() {
    let (r, front) = rig_with(SHOPPING, "x", vec![UserAnswer::Deny]);
    let result = call(
        &r.gate,
        "browser_handle_dialog",
        json!({"tab": 3, "accept": true}),
    );
    assert!(is_error(&result));
    assert_eq!(front.cards().len(), 1);
    assert!(r.rig.requests("dialog.respond").is_empty());
    r.rig.finish();
}
