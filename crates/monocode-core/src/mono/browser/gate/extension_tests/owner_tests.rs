//! Tab ownership through the gate with two Monos on one link: a scripted browser keeps the
//! ownership table the way the extension does (a user tab needs a user reference to be adopted, a
//! tab of another Mono needs the user's approval, only the owner closes or releases) and the
//! tests read back what the app asked and what it refused to ask.
use super::super::extension::act::dispatch_event;
use super::super::extension::act::ActConfig;
use super::*;
use crate::mono::browser::copy;
use crate::mono::browser::link::protocol::Event;
use crate::mono::browser::ownership;
use crate::mono::browser::soul_rules::Parsed;
use crate::mono::browser::trust::approval::{ApprovalQueue, UserAnswer};
use crate::mono::browser::trust::BrowserLevel;
use std::sync::atomic::AtomicU64;

const MAIL: &str = "https://mail.example/inbox";
const SHEETS: &str = "https://sheets.example/d/1";
const DOCS: &str = "https://docs.example/d/9";

/// What the scripted browser keeps between requests.
#[derive(Default)]
struct World {
    tabs: Vec<TabInfo>,
    next: u64,
}

fn world(tabs: Vec<TabInfo>) -> Arc<Mutex<World>> {
    let next = tabs.iter().map(|t| t.tab).max().unwrap_or(0) + 1;
    Arc::new(Mutex::new(World { tabs, next }))
}

fn not_owner(what: &str) -> Answer {
    Err(ProtocolError::new(ErrorCode::NotOwner, what))
}

/// The extension's rules of section 9.3 for the four tab methods and the lists.
fn install(browser: &mut Browser, world: &Arc<Mutex<World>>) {
    let world = world.clone();
    browser.handler = Some(Box::new(move |request| {
        let method = request.method.as_deref()?;
        let params = request.params.clone()?;
        let mono = request.mono.clone();
        let mut w = world.lock().unwrap();
        let find = |w: &World, id: u64| w.tabs.iter().position(|t| t.tab == id);
        Some(match method {
            "tabs.list" => {
                let all = params["all"].as_bool().unwrap();
                let tabs: Vec<&TabInfo> =
                    w.tabs.iter().filter(|t| all || t.owner == mono).collect();
                Ok(json!({ "tabs": tabs }))
            }
            "tab.open" => {
                let id = w.next;
                w.next += 1;
                let t = tab(
                    id,
                    params["url"].as_str().unwrap(),
                    "New tab",
                    mono.as_deref(),
                );
                w.tabs.push(t.clone());
                Ok(serde_json::to_value(t).unwrap())
            }
            "tab.close" | "tab.release" => {
                let id = params["tab"].as_u64().unwrap();
                let Some(at) = find(&w, id) else {
                    return Some(Err(ProtocolError::new(ErrorCode::TabGone, "gone")));
                };
                if w.tabs[at].owner != mono {
                    return Some(not_owner("not yours"));
                }
                if method == "tab.close" {
                    w.tabs.remove(at);
                } else {
                    w.tabs[at].owner = None;
                }
                Ok(json!({}))
            }
            "tab.adopt" => {
                let id = params["tab"].as_u64().unwrap();
                let Some(at) = find(&w, id) else {
                    return Some(Err(ProtocolError::new(ErrorCode::TabGone, "gone")));
                };
                let kind = params["grant"]["kind"].as_str().unwrap().to_owned();
                match (&w.tabs[at].owner, kind.as_str()) {
                    (None, "user_reference") | (Some(_), "user_approved") => {}
                    _ => return Some(not_owner("a different grant is needed")),
                }
                w.tabs[at].owner = mono;
                Ok(serde_json::to_value(w.tabs[at].clone()).unwrap())
            }
            _ => return None,
        })
    }));
}

struct Two {
    rig: Rig,
    instance: String,
    world: Arc<Mutex<World>>,
    a: Arc<Gate>,
    b: Arc<Gate>,
    front: super::act_tests::FrontEnd,
}

fn config(level: BrowserLevel, label: &str, queue: &Arc<ApprovalQueue>) -> ActConfig {
    ActConfig {
        level,
        rules: Parsed::default(),
        store: None,
        mono_label: label.into(),
        profile: None,
        approval_timeout: Duration::from_secs(5),
        queue: queue.clone(),
    }
}

/// A link id of its own, so the process-wide registries never mix two tests.
fn fresh_instance() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0xabc0_0000);
    format!("{:032x}", NEXT.fetch_add(1, Ordering::SeqCst))
}

/// Mira (A) and Kai (B) on one link, with a few tabs: 1 and 2 are the user's, 3 is Mira's, 4 Kai's.
/// The answers are for the cards of Mira's queue (a transfer asks there).
fn two(answers: Vec<UserAnswer>) -> Two {
    let w = world(vec![
        tab(1, MAIL, "Inbox (3)", None),
        tab(2, SHEETS, "Quarterly Report - Sheets", None),
        tab(3, "https://own.example/", "Mira's page", Some(MONO)),
        tab(4, DOCS, "Kai's notes - Docs", Some(OTHER_MONO)),
    ]);
    let mut browser = Browser::new(Vec::new());
    install(&mut browser, &w);
    let instance = fresh_instance();
    let rig = start_as(browser, &instance);
    let a_queue = ApprovalQueue::new(MONO);
    rig.upstream.configure_act(
        config(BrowserLevel::Act, "Mira", &a_queue),
        Origins::default(),
    );
    let m = rig._manager.clone();
    let link_id = instance.clone();
    let other = ExtensionUpstream::new(OTHER_MONO, Arc::new(move || m.handle(&link_id)));
    other.configure_act(
        config(BrowserLevel::Act, "Kai", &ApprovalQueue::new(OTHER_MONO)),
        Origins::default(),
    );
    let policy = || {
        Policy::new(BrowserTrust::AskRisky, Vec::new())
            .with_extension(true)
            .with_show_actions(false)
    };
    let a = Gate::start_with_timeout(rig.upstream.clone(), policy(), "extension", WAIT).unwrap();
    let b = Gate::start_with_timeout(other, policy(), "extension", WAIT).unwrap();
    let front = super::act_tests::FrontEnd::start(&a_queue, answers);
    Two {
        rig,
        instance,
        world: w,
        a,
        b,
        front,
    }
}

/// A run of Mira with the user's words.
fn said(two: &Two, text: &str) -> String {
    let route = two.a.route_for(Some(11));
    two.a.begin_turn(&route, text);
    route
}
fn said_b(two: &Two, text: &str) -> String {
    let route = two.b.route_for(Some(12));
    two.b.begin_turn(&route, text);
    route
}

fn call_gate(gate: &Gate, route: &str, args: Value) -> Value {
    gate.handle(
        route,
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "browser_tabs", "arguments": args}}),
    )
    .expect("a reply")["result"]
        .clone()
}

fn owner_in(two: &Two, id: u64) -> Option<String> {
    two.world
        .lock()
        .unwrap()
        .tabs
        .iter()
        .find(|t| t.tab == id)
        .and_then(|t| t.owner.clone())
}

fn mirror_owner(two: &Two, id: u64) -> Option<String> {
    ownership::with_table(&two.instance, |t| t.owner_of(id).map(str::to_owned))
}

// ---- new, close, release ----

#[test]
fn a_new_tab_is_opened_in_the_background_owned_and_selected() {
    let two = two(vec![]);
    let route = said(&two, "open shop.example");
    let text = ok_text(call_gate(
        &two.a,
        &route,
        json!({"action": "new", "url": "https://shop.example/cart"}),
    ));
    let opened = two.rig.requests("tab.open");
    assert_eq!(opened.len(), 1);
    assert_eq!(
        opened[0].params.as_ref().unwrap()["url"],
        "https://shop.example/cart"
    );
    assert_eq!(opened[0].mono.as_deref(), Some(MONO));
    assert_eq!(owner_in(&two, 5).as_deref(), Some(MONO));
    assert!(
        text.contains("(current) [New tab](https://shop.example/cart) [tab=5] [yours]"),
        "{text}"
    );
    assert_eq!(mirror_owner(&two, 5).as_deref(), Some(MONO));
    two.rig.finish();
}

#[test]
fn a_new_tab_follows_the_origin_lists_and_only_opens_web_addresses() {
    let two = two(vec![]);
    two.rig.upstream.configure_act(
        config(BrowserLevel::Act, "Mira", &ApprovalQueue::new(MONO)),
        Origins::new(&[], &["https://bank.example".to_owned()]),
    );
    let route = said(&two, "open things");
    for url in [
        "https://bank.example/login",
        "chrome://settings",
        "file:///etc/passwd",
    ] {
        let result = call_gate(&two.a, &route, json!({"action": "new", "url": url}));
        assert!(is_error(&result), "{url}");
    }
    assert!(two.rig.requests("tab.open").is_empty());
    two.rig.finish();
}

#[test]
fn the_tab_limit_counts_the_monos_own_tabs() {
    let two = two(vec![]);
    // Tab 3 is Mira's; seven more tabs of hers (adopted or dragged in, so the gate's own list of
    // tabs it opened knows none of them) bring her to the limit.
    {
        let mut world = two.world.lock().unwrap();
        for id in 10..(10 + tabs::MAX_TABS as u64 - 1) {
            world.tabs.push(tab(
                id,
                &format!("https://own{id}.example/"),
                "Own",
                Some(MONO),
            ));
        }
    }
    let route = said(&two, "open one-too-many.example");
    let refused = call_gate(
        &two.a,
        &route,
        json!({"action": "new", "url": "https://one-too-many.example/"}),
    );
    assert!(is_error(&refused), "{}", text_of(&refused));
    assert!(
        text_of(&refused).contains("tabs open"),
        "{}",
        text_of(&refused)
    );
    assert!(two.rig.requests("tab.open").is_empty());
    // Kai has his own count: his one tab does not hit the limit.
    let route_b = said_b(&two, "open kai.example");
    ok_text(call_gate(
        &two.b,
        &route_b,
        json!({"action": "new", "url": "https://kai.example/"}),
    ));
    // Closing one of Mira's tabs makes room again.
    ok_text(call_gate(
        &two.a,
        &route,
        json!({"action": "close", "tab": 10}),
    ));
    ok_text(call_gate(
        &two.a,
        &route,
        json!({"action": "new", "url": "https://one-too-many.example/"}),
    ));
    two.rig.finish();
}

#[test]
fn only_own_tabs_are_closed_or_released() {
    let two = two(vec![]);
    let route = said(&two, "tidy up, tmp.example is mine");
    call_gate(&two.a, &route, json!({"action": "list", "all": true}));
    for tab_id in [1, 4] {
        for action in ["close", "release"] {
            let result = call_gate(&two.a, &route, json!({"action": action, "tab": tab_id}));
            assert!(is_error(&result), "{action} {tab_id}");
            assert!(
                text_of(&result).contains("not yours"),
                "{}",
                text_of(&result)
            );
        }
    }
    assert!(two.rig.requests("tab.close").is_empty());
    assert!(two.rig.requests("tab.release").is_empty());
    assert_eq!(owner_in(&two, 4).as_deref(), Some(OTHER_MONO));

    let text = ok_text(call_gate(
        &two.a,
        &route,
        json!({"action": "release", "tab": 3}),
    ));
    assert_eq!(two.rig.requests("tab.release").len(), 1);
    assert_eq!(owner_in(&two, 3), None, "the tab is the user's now");
    assert!(!text.contains("[tab=3]"), "{text}");
    assert_eq!(mirror_owner(&two, 3), None);

    // Its own tab can be closed by the index of the last list.
    ok_text(call_gate(
        &two.a,
        &route,
        json!({"action": "new", "url": "https://tmp.example/"}),
    ));
    let list = ok_text(call_gate(&two.a, &route, json!({"action": "list"})));
    let index = tabs::parse_tabs(&list).len() - 1;
    ok_text(call_gate(
        &two.a,
        &route,
        json!({"action": "close", "index": index}),
    ));
    assert_eq!(two.rig.requests("tab.close").len(), 1);
    assert!(two
        .world
        .lock()
        .unwrap()
        .tabs
        .iter()
        .all(|t| t.url != "https://tmp.example/"));
    two.rig.finish();
}

#[test]
fn a_read_only_mono_may_open_a_tab_but_never_closes_adopts_or_releases() {
    let two = two(vec![]);
    two.rig.upstream.configure_act(
        config(BrowserLevel::ReadOnly, "Mira", &ApprovalQueue::new(MONO)),
        Origins::default(),
    );
    // The gate itself is at level 2 here, so this is the upstream's own line of defense.
    let route = said(&two, "read the Quarterly Report sheet and shop.example");
    for args in [
        json!({"action": "close", "tab": 3}),
        json!({"action": "release", "tab": 3}),
        json!({"action": "adopt", "tab": 2, "user_reference": "Quarterly Report"}),
    ] {
        let result = call_gate(&two.a, &route, args.clone());
        assert!(is_error(&result), "{args}");
        assert!(
            text_of(&result).contains("only read"),
            "{}",
            text_of(&result)
        );
    }
    assert!(two.rig.requests("tab.close").is_empty());
    assert!(two.rig.requests("tab.adopt").is_empty());
    ok_text(call_gate(
        &two.a,
        &route,
        json!({"action": "new", "url": "https://shop.example/"}),
    ));
    two.rig.finish();
}

// ---- adopt ----

#[test]
fn adopting_a_user_tab_needs_words_the_user_wrote_that_name_one_tab() {
    let two = two(vec![]);
    let route = said(
        &two,
        "please work in the Quarterly Report sheet and in my inbox",
    );
    let adopt = |reference: Value| {
        call_gate(
            &two.a,
            &route,
            json!({"action": "adopt", "tab": 2, "user_reference": reference}),
        )
    };
    // No reference, a made-up one, a too short one: nothing reaches the browser.
    let missing = call_gate(&two.a, &route, json!({"action": "adopt", "tab": 2}));
    assert!(is_error(&missing) && text_of(&missing).contains("user_reference"));
    let invented = adopt(json!("budget forecast"));
    assert!(is_error(&invented));
    assert!(
        text_of(&invented).contains("drag it into your group"),
        "{}",
        text_of(&invented)
    );
    assert!(
        is_error(&adopt(json!("inbox"))),
        "too short and not 8 characters"
    );
    // Words the user wrote, but they point at another tab than the one named.
    let wrong = call_gate(
        &two.a,
        &route,
        json!({"action": "adopt", "tab": 1, "user_reference": "Quarterly Report"}),
    );
    assert!(is_error(&wrong));
    assert!(two.rig.requests("tab.adopt").is_empty());

    let text = ok_text(adopt(json!("Quarterly Report")));
    let sent = two.rig.requests("tab.adopt");
    assert_eq!(sent.len(), 1);
    let grant = &sent[0].params.as_ref().unwrap()["grant"];
    assert_eq!(grant["kind"], "user_reference");
    assert_eq!(grant["quote"], "Quarterly Report");
    assert!(grant["message_ms"].as_u64().unwrap() > 0);
    assert_eq!(owner_in(&two, 2).as_deref(), Some(MONO));
    assert!(text.contains("[tab=2] [yours]"), "{text}");
    assert_eq!(mirror_owner(&two, 2).as_deref(), Some(MONO));
    // A tab that is already Mira's is not adopted again.
    assert!(is_error(&adopt(json!("Quarterly Report"))));
    assert_eq!(two.rig.requests("tab.adopt").len(), 1);
    two.rig.finish();
}

#[test]
fn a_reference_that_fits_two_tabs_is_refused_and_the_mono_is_told_what_to_ask() {
    let two = two(vec![]);
    two.world.lock().unwrap().tabs.push(tab(
        6,
        "https://drafts.example/d/2",
        "Quarterly Report (draft)",
        None,
    ));
    let route = said(&two, "use the Quarterly Report please");
    let result = call_gate(
        &two.a,
        &route,
        json!({"action": "adopt", "user_reference": "Quarterly Report"}),
    );
    assert!(is_error(&result));
    let text = text_of(&result);
    assert!(
        text.contains("more than one tab") && text.contains("drag it into your group"),
        "{text}"
    );
    assert!(two.rig.requests("tab.adopt").is_empty());
    two.rig.finish();
}

#[test]
fn the_reference_may_pick_the_tab_without_its_id() {
    let two = two(vec![]);
    let route = said(&two, "check the Inbox (3) tab for the invoice");
    ok_text(call_gate(
        &two.a,
        &route,
        json!({"action": "adopt", "user_reference": "Inbox (3) tab"}),
    ));
    assert_eq!(owner_in(&two, 1).as_deref(), Some(MONO));
    two.rig.finish();
}

// ---- two Monos ----

#[test]
fn a_tab_of_another_mono_moves_only_with_a_reference_and_the_users_yes() {
    let two = two(vec![UserAnswer::Deny, UserAnswer::AllowOnce]);
    let route = said(&two, "take over the Kai's notes tab for yourself");
    let args = json!({"action": "adopt", "tab": 4, "user_reference": "Kai's notes"});

    // The user says no: nothing is sent, and Kai keeps the tab.
    let denied = call_gate(&two.a, &route, args.clone());
    assert!(is_error(&denied), "{}", text_of(&denied));
    assert!(two.rig.requests("tab.adopt").is_empty());
    assert_eq!(owner_in(&two, 4).as_deref(), Some(OTHER_MONO));
    let cards = two.front.cards();
    assert_eq!(cards.len(), 1);
    let en = cards[0].text_in(copy::Lang::En);
    assert!(
        en.headline.contains("take over the tab") && en.headline.contains("Kai"),
        "{}",
        en.headline
    );
    assert!(
        en.allow_conversation.is_none(),
        "a transfer is never a standing grant"
    );

    // The user says yes: the browser gets the user's approval as the grant.
    let text = ok_text(call_gate(&two.a, &route, args));
    let sent = two.rig.requests("tab.adopt");
    assert_eq!(sent.len(), 1);
    let grant = &sent[0].params.as_ref().unwrap()["grant"];
    assert_eq!(grant["kind"], "user_approved");
    assert_eq!(grant["approval_id"].as_str().unwrap().len(), 32);
    assert_eq!(owner_in(&two, 4).as_deref(), Some(MONO));
    assert!(text.contains("[tab=4] [yours]"), "{text}");
    assert_eq!(mirror_owner(&two, 4).as_deref(), Some(MONO));

    // Kai no longer sees it as his, and cannot close it.
    let route_b = said_b(&two, "close the notes");
    let own_b = ok_text(call_gate(&two.b, &route_b, json!({"action": "list"})));
    assert!(!own_b.contains("[tab=4]"), "{own_b}");
    let closing = call_gate(&two.b, &route_b, json!({"action": "close", "tab": 4}));
    assert!(is_error(&closing));
    assert!(two.rig.requests("tab.close").is_empty());
    two.rig.finish();
}

#[test]
fn two_monos_cannot_both_adopt_one_user_tab() {
    let two = two(vec![]);
    let route_a = said(&two, "work in the Quarterly Report sheet");
    let route_b = said_b(&two, "work in the Quarterly Report sheet");
    let args = json!({"action": "adopt", "tab": 2, "user_reference": "Quarterly Report"});
    ok_text(call_gate(&two.a, &route_a, args.clone()));
    // Kai now meets Mira's tab: it needs the user's yes, which nobody can give here (Kai has no
    // front end), so it is refused and Mira keeps it.
    let second = call_gate(&two.b, &route_b, args);
    assert!(is_error(&second));
    assert_eq!(owner_in(&two, 2).as_deref(), Some(MONO));
    assert_eq!(two.rig.requests("tab.adopt").len(), 1);
    two.rig.finish();
}

// ---- the user takes a tab back ----

fn ownership_event(instance: &str, tab: u64, owner: Option<&str>, by: &str) {
    let envelope = Envelope::event(
        "x-1",
        Event::TabOwnership,
        json!({"tab": tab, "owner": owner, "by": by}),
    );
    dispatch_event(instance, &envelope);
}

#[test]
fn a_tab_dragged_out_is_taken_back_and_needs_newer_words_to_adopt_again() {
    let two = two(vec![]);
    let route = said(&two, "work in the Quarterly Report sheet");
    let args = json!({"action": "adopt", "tab": 2, "user_reference": "Quarterly Report"});
    ok_text(call_gate(&two.a, &route, args.clone()));
    assert_eq!(mirror_owner(&two, 2).as_deref(), Some(MONO));

    // The user drags the tab out of Mira's group.
    std::thread::sleep(Duration::from_millis(5));
    two.world.lock().unwrap().tabs[1].owner = None;
    ownership_event(&two.instance, 2, None, "user_drag");
    assert_eq!(mirror_owner(&two, 2), None);
    let taken = ownership::with_table(&two.instance, |t| t.taken_back_at(2));
    assert!(taken.is_some());

    // The old words do not count any more.
    let again = call_gate(&two.a, &route, args.clone());
    assert!(is_error(&again));
    assert!(
        text_of(&again).contains("took it back"),
        "{}",
        text_of(&again)
    );
    assert_eq!(two.rig.requests("tab.adopt").len(), 1);

    // New words after the stop do.
    std::thread::sleep(Duration::from_millis(5));
    two.a
        .begin_turn(&route, "yes, the Quarterly Report sheet again");
    ok_text(call_gate(&two.a, &route, args));
    assert_eq!(two.rig.requests("tab.adopt").len(), 2);
    assert_eq!(owner_in(&two, 2).as_deref(), Some(MONO));
    two.rig.finish();
}

#[test]
fn a_tab_dragged_into_another_monos_group_changes_hands_without_a_prompt() {
    let two = two(vec![]);
    ownership_event(&two.instance, 3, Some(OTHER_MONO), "user_drag");
    assert_eq!(mirror_owner(&two, 3).as_deref(), Some(OTHER_MONO));
    assert!(two.front.cards().is_empty());
    // A popup of one of Kai's tabs is Kai's.
    ownership_event(&two.instance, 9, Some(OTHER_MONO), "popup");
    assert_eq!(mirror_owner(&two, 9).as_deref(), Some(OTHER_MONO));
    two.rig.finish();
}

#[test]
fn adopting_is_refused_at_the_tab_limit_too() {
    let two = two(vec![]);
    {
        let mut world = two.world.lock().unwrap();
        for id in 10..(10 + tabs::MAX_TABS as u64 - 1) {
            world.tabs.push(tab(
                id,
                &format!("https://own{id}.example/"),
                "Own",
                Some(MONO),
            ));
        }
    }
    let route = said(&two, "work in the Quarterly Report sheet");
    let result = call_gate(
        &two.a,
        &route,
        json!({"action": "adopt", "tab": 2, "user_reference": "Quarterly Report"}),
    );
    assert!(is_error(&result));
    assert!(
        text_of(&result).contains("tabs open"),
        "{}",
        text_of(&result)
    );
    assert!(two.rig.requests("tab.adopt").is_empty());
    assert!(
        two.front.cards().is_empty(),
        "nobody is asked for a tab that cannot be taken"
    );
    two.rig.finish();
}

#[test]
fn a_tab_dragged_to_another_mono_is_no_longer_the_old_monos_selection() {
    let two = two(vec![]);
    let route = said(&two, "look around");
    call_gate(&two.a, &route, json!({"action": "list"}));
    let listed = call_gate(&two.a, &route, json!({"action": "select", "index": 0}));
    assert!(
        text_of(&listed).contains("(current)"),
        "{}",
        text_of(&listed)
    );
    ownership_event(&two.instance, 3, Some(OTHER_MONO), "user_drag");
    two.world.lock().unwrap().tabs[2].owner = Some(OTHER_MONO.into());
    let after = ok_text(call_gate(&two.a, &route, json!({"action": "list"})));
    assert!(!after.contains("(current)"), "{after}");
    two.rig.finish();
}

#[test]
fn a_selected_tab_does_not_deadlock_the_agents_own_approval_request() {
    let two = two(vec![UserAnswer::AllowOnce]);
    let route = said(&two, "look around");
    call_gate(&two.a, &route, json!({"action": "list"}));
    call_gate(&two.a, &route, json!({"action": "select", "index": 0}));
    let reply = two
        .a
        .handle(
            &route,
            json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "browser_request_approval",
                "arguments": {"action": "do a thing", "category": "other"}}}),
        )
        .expect("a reply");
    assert!(reply.get("result").is_some(), "{reply}");
    two.rig.finish();
}

// ---- the user presses Stop in the page overlay ----

fn call_tool(gate: &Gate, route: &str, name: &str, args: Value) -> Value {
    gate.handle(
        route,
        json!({"jsonrpc": "2.0", "id": 8, "method": "tools/call", "params": {"name": name, "arguments": args}}),
    )
    .expect("a reply")["result"]
        .clone()
}

fn tab_stopped_event(instance: &str, mono: &str, tab: u64) {
    // Shaped like the extension's: no envelope `mono`, the Mono is in the params.
    let envelope = Envelope::event(
        "x-2",
        Event::TabStopped,
        json!({"tab": tab, "mono": mono, "by": "overlay"}),
    );
    dispatch_event(instance, &envelope);
}

#[test]
fn the_overlay_stop_ends_every_call_on_that_tab_until_the_user_gives_it_back() {
    let two = two(vec![]);
    let route = two.a.route_for(Some(11));
    two.a.begin_turn(&route, "look at the page");
    ok_text(call_gate(&two.a, &route, json!({"action": "list"})));

    tab_stopped_event(&two.instance, MONO, 3);
    assert_eq!(mirror_owner(&two, 3), None, "the tab is the user's again");
    assert!(ownership::with_table(&two.instance, |t| t.taken_back_at(3)).is_some());

    // Mira, not Kai, is stopped: the next call on that tab fails with the stop text and never
    // reaches the browser.
    let stopped = call_tool(&two.a, &route, "browser_snapshot", json!({"tab": 3}));
    assert!(is_error(&stopped));
    let text = text_of(&stopped);
    assert!(text.contains("The user stopped you on the tab"), "{text}");
    assert!(text.contains("Mira's page"), "{text}");
    assert!(two.rig.requests("page.snapshot").is_empty());

    // Another tab of the same Mono is not affected.
    let other = call_tool(&two.a, &route, "browser_snapshot", json!({"tab": 1}));
    assert!(
        !text_of(&other).contains("The user stopped you"),
        "{}",
        text_of(&other)
    );

    // The user drags the tab back: calls on it work again.
    ownership_event(&two.instance, 3, Some(MONO), "user_drag");
    let again = call_tool(&two.a, &route, "browser_snapshot", json!({"tab": 3}));
    assert!(
        !text_of(&again).contains("The user stopped you"),
        "{}",
        text_of(&again)
    );
    two.rig.finish();
}

#[test]
fn the_overlay_stop_of_one_mono_does_not_stop_another() {
    let two = two(vec![]);
    let route = said_b(&two, "look at the page");
    tab_stopped_event(&two.instance, MONO, 3);
    let reply = call_tool(&two.b, &route, "browser_snapshot", json!({"tab": 4}));
    assert!(
        !text_of(&reply).contains("The user stopped you"),
        "{}",
        text_of(&reply)
    );
    two.rig.finish();
}

#[test]
fn a_tab_stopped_event_needs_its_mono_and_ownership_and_touches_no_other_mono() {
    let two = two(vec![]);
    let route = two.a.route_for(Some(11));
    two.a.begin_turn(&route, "look");
    ok_text(call_gate(&two.a, &route, json!({"action": "list"})));
    let route_b = said_b(&two, "look");
    ok_text(call_gate(&two.b, &route_b, json!({"action": "list"})));

    // No Mono in the params: ignored.
    let bare = Envelope::event("x-3", Event::TabStopped, json!({"tab": 3, "by": "overlay"}));
    dispatch_event(&two.instance, &bare);
    assert_eq!(mirror_owner(&two, 3).as_deref(), Some(MONO));
    // Kai names a tab that is Mira's: ignored, nothing changes for either.
    tab_stopped_event(&two.instance, OTHER_MONO, 3);
    assert_eq!(mirror_owner(&two, 3).as_deref(), Some(MONO));
    // Mira names a tab that is Kai's (4): ignored.
    tab_stopped_event(&two.instance, MONO, 4);
    assert_eq!(mirror_owner(&two, 4).as_deref(), Some(OTHER_MONO));
    let fine = call_tool(&two.a, &route, "browser_snapshot", json!({"tab": 3}));
    assert!(
        !text_of(&fine).contains("The user stopped you"),
        "{}",
        text_of(&fine)
    );

    // The real one: only Mira's tab 3 is taken back, Kai keeps tab 4 and can still use it.
    tab_stopped_event(&two.instance, MONO, 3);
    assert_eq!(mirror_owner(&two, 3), None);
    assert_eq!(mirror_owner(&two, 4).as_deref(), Some(OTHER_MONO));
    let kai = call_tool(&two.b, &route_b, "browser_snapshot", json!({"tab": 4}));
    assert!(
        !text_of(&kai).contains("The user stopped you"),
        "{}",
        text_of(&kai)
    );
    two.rig.finish();
}

#[test]
fn only_the_user_dragging_a_stopped_tab_back_ends_the_mark() {
    let two = two(vec![]);
    let route = two.a.route_for(Some(11));
    two.a.begin_turn(&route, "look");
    ok_text(call_gate(&two.a, &route, json!({"action": "list"})));
    tab_stopped_event(&two.instance, MONO, 3);
    // A popup or any other claim does not clear it.
    ownership_event(&two.instance, 3, Some(MONO), "popup");
    let still = call_tool(&two.a, &route, "browser_snapshot", json!({"tab": 3}));
    assert!(
        text_of(&still).contains("The user stopped you"),
        "{}",
        text_of(&still)
    );
    ownership_event(&two.instance, 3, Some(MONO), "user_drag");
    let again = call_tool(&two.a, &route, "browser_snapshot", json!({"tab": 3}));
    assert!(
        !text_of(&again).contains("The user stopped you"),
        "{}",
        text_of(&again)
    );
    two.rig.finish();
}
