//! The last small round on the flow tools and the read tests: what a flow hides from a Mono (the
//! allowed list, typed text of any command), the cost of `flow_list`, what a run did compared with
//! what was approved (the attempts of every step), a POST by a flow before a reload or a step
//! back, the sites `flow_show` and `flow_run` pick from, one reading of the grant book per check,
//! and the cards `quiet_act` answers.
use super::act_tests::*;
use super::flow_tests::*;
use super::tests::*;
use super::upstream::*;
use crate::mono::browser::copy::{ActionDesc, Lang};
use crate::mono::browser::gate::{Origins, Upstream};
use crate::mono::browser::trust::approval::UserAnswer;
use crate::mono::browser::BrowserLevel;
use serde_json::{json, Value};

const RUN_YES: &str = include_str!("fixtures/flow_run_yes.json");

fn allowed(origins: &[&str]) -> Origins {
    Origins::new(
        &origins.iter().map(|o| (*o).to_owned()).collect::<Vec<_>>(),
        &[],
    )
}

/// A copy of the quiet flow under another name and site (the start page, origin and hosts follow).
fn flow_on(name: &str, site: &str) -> Value {
    let mut flow = quiet_flow();
    flow["name"] = json!(name);
    flow["site"] = json!(site);
    flow["origin"] = json!(format!("https://{site}"));
    flow["hosts"] = json!([format!("https://{site}")]);
    flow["startUrl"] = json!(format!("https://{site}/"));
    flow["steps"][0]["url"] = json!(format!("https://{site}/"));
    flow["steps"][0]["command"] = json!(["open", format!("https://{site}/")]);
    flow["steps"][1]["url"] = json!(format!("https://{site}/more"));
    flow
}

fn shows(a: &Act) -> usize {
    calls_with(a, "flow show").len()
}

// ---- F-1: what a flow hides, and what it costs to list ----

#[test]
fn a_flow_outside_the_allowed_list_is_hidden_and_never_read() {
    let a = flow_act_with(
        BrowserLevel::ReadOnly,
        allowed(&["https://example.com"]),
        quiet_flow(),
    );
    // A flow of a site that is not allowed, and one of an allowed site that reaches another host.
    let mut reaches = flow_on("reaches-out", "example.com");
    reaches["hosts"] = json!(["https://example.com", "https://elsewhere.example"]);
    set_flows(
        &a,
        vec![
            quiet_flow(),
            flow_on("elsewhere-flow", "elsewhere.example"),
            reaches,
        ],
    );
    let (error, list) = a.call("flow_list", json!({}));
    assert!(!error, "{list}");
    assert!(list.contains(NAME), "{list}");
    assert!(
        !list.contains("elsewhere") && !list.contains("reaches-out"),
        "{list}"
    );
    assert_eq!(
        calls_with(&a, "--site elsewhere.example").len(),
        0,
        "a site that is not allowed is filtered before any flow show"
    );
    for name in ["elsewhere-flow", "reaches-out"] {
        let (error, text) = a.call("flow_show", json!({"name": name}));
        assert!(
            error && text.contains(&format!("No saved flow named {name}")),
            "{name}: {text}"
        );
        assert!(!text.contains("elsewhere.example"), "{name}: {text}");
    }
}

#[test]
fn typed_text_of_any_command_and_a_typed_key_is_masked_in_show_and_in_the_save_card() {
    let mut flow = quiet_flow();
    let typed = |seq, command: &[&str], intent: &str| {
        step(
            seq,
            command,
            "https://example.com/",
            intent,
            "Search",
            "textbox",
        )
    };
    flow["steps"] = json!([
        flow["steps"][0].clone(),
        typed(
            13,
            &["keyboard", "type", "hunter2xyz"],
            "type \"hunter2xyz\"",
        ),
        typed(
            14,
            &["frobnicate", "--mode", "secretvalue"],
            "frobnicate secretvalue"
        ),
        typed(15, &["press", "q"], "press \"q\""),
        typed(16, &["press", "Enter"], "press Enter"),
    ]);
    let a = flow_act_with(BrowserLevel::ReadOnly, no_limits(), flow);
    let (error, shown) = a.call("flow_show", json!({"name": NAME}));
    assert!(!error, "{shown}");
    for secret in ["hunter2xyz", "secretvalue", "\"q\""] {
        assert!(!shown.contains(secret), "{secret}: {shown}");
    }
    assert!(
        shown.contains("keyboard type <text, 10 characters>"),
        "{shown}"
    );
    assert!(shown.contains("press <text, 1 characters>"), "{shown}");
    assert!(shown.contains("press Enter"), "a named key stays: {shown}");

    // The save card lists the recorded steps the same way.
    let b = flow_act_with(BrowserLevel::Act, no_limits(), quiet_flow());
    b.r.runner.lock().flow_steps = Some(json!([
        {"seq": 1, "command": ["open", "https://example.com/"], "url": "https://example.com/", "ok": true},
        {"seq": 2, "command": ["keyboard", "type", "hunter2xyz"], "url": "https://example.com/", "ok": true},
        {"seq": 3, "command": ["frobnicate", "secretvalue"], "url": "https://example.com/", "ok": true},
        {"seq": 4, "command": ["press", "q"], "url": "https://example.com/", "ok": true},
        {"seq": 5, "command": ["press", "Enter"], "url": "https://example.com/", "ok": true},
    ]));
    let (error, text) = b.call("flow_save", json!({"name": "typed-flow"}));
    assert!(!error, "{text}");
    let ActionDesc::FlowSave { listed, .. } = &b.cards()[0].facts.action else {
        panic!("{:?}", b.cards()[0].facts.action);
    };
    let joined = listed.join("\n");
    for secret in ["hunter2xyz", "secretvalue", " q"] {
        assert!(!joined.contains(secret), "{secret}: {joined}");
    }
    assert!(
        joined.contains("keyboard type <text, 10 characters>"),
        "{joined}"
    );
    assert!(joined.contains("press Enter"), "{joined}");
}

#[test]
fn flow_list_checks_the_site_first_reads_each_flow_once_and_at_most_twenty() {
    let a = flow_act_with(BrowserLevel::ReadOnly, no_limits(), quiet_flow());
    let many: Vec<Value> = (0..25)
        .map(|n| flow_on(&format!("flow-{n}"), "example.com"))
        .collect();
    set_flows(&a, many);
    let (error, list) = a.call("flow_list", json!({}));
    assert!(!error, "{list}");
    assert_eq!(shows(&a), 20, "{list}");
    assert_eq!(list.matches("(site example.com)").count(), 20, "{list}");
    assert!(
        list.contains("5 more flow(s) were not checked"),
        "the rest is said, and hidden: {list}"
    );
}

#[test]
fn flow_show_reads_a_flow_once_when_resolving_its_site() {
    let a = flow_act_with(BrowserLevel::ReadOnly, no_limits(), quiet_flow());
    // The same name on a second site whose flow reaches a blocked host: it is read to be hidden.
    let mut hidden = flow_on(NAME, "shop.example");
    hidden["hosts"] = json!(["https://shop.example", "https://evil.example"]);
    set_flows(&a, vec![quiet_flow(), hidden]);
    a.up.configure(Config {
        origins: blocked("https://evil.example"),
        ..a.up.config()
    });
    let (error, text) = a.call("flow_show", json!({"name": NAME}));
    assert!(!error && text.contains("Site: example.com"), "{text}");
    assert_eq!(
        shows(&a),
        2,
        "one read for each site, none again: {}",
        shows(&a)
    );
}

// ---- N4: sites that do not count ----

#[test]
fn a_site_whose_flow_cannot_be_read_or_is_hidden_does_not_count_and_is_not_named() {
    // A broken copy of the name on another site does not make the name ambiguous.
    let a = flow_act_with(BrowserLevel::Act, no_limits(), quiet_flow());
    let mut broken = flow_on(NAME, "broken.example");
    broken["kind"] = json!(5);
    set_flows(&a, vec![quiet_flow(), broken]);
    let (error, text) = a.call("flow_show", json!({"name": NAME}));
    assert!(!error && text.contains("Site: example.com"), "{text}");
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(!error, "{text}");

    // A flow that is only on a hidden site: the name is not known, and no address is named.
    let mut hidden = flow_on("solo", "shop.example");
    hidden["hosts"] = json!(["https://shop.example", "https://evil.example"]);
    let b = flow_act_with(
        BrowserLevel::Act,
        blocked("https://evil.example"),
        quiet_flow(),
    );
    set_flows(&b, vec![hidden]);
    for tool in ["flow_show", "flow_run"] {
        let (error, text) = b.call(tool, json!({"name": "solo"}));
        assert!(
            error && text.contains("No saved flow named solo") && !text.contains("evil"),
            "{tool}: {text}"
        );
    }
    // Naming the site does not make the refusal describe the hidden flow.
    let (error, text) = run(&b, json!({"name": "solo", "site": "shop.example"}));
    assert!(error && !text.contains("evil"), "{text}");
    assert!(runs(&b).is_empty());
    assert_eq!(b.r.runner.count("tabs open"), 0);
}

// ---- N1: what Ardia did at each step ----

fn reply_steps() -> Value {
    serde_json::from_str::<Value>(RUN_YES).unwrap()["data"]["steps"].clone()
}

#[test]
fn the_attempts_of_every_step_must_be_the_approved_command_or_target() {
    let mut tampered_command = reply_steps();
    tampered_command[0]["attempts"][0]["argv"] = json!(["open", "https://evil.example/"]);
    let mut tampered_name = reply_steps();
    tampered_name[1]["attempts"][0]["argv"] =
        json!(["find", "role", "link", "click", "--name", "Delete", "--exact"]);
    let mut tampered_verb = reply_steps();
    tampered_verb[1]["attempts"][0]["argv"] = json!([
        "find",
        "role",
        "link",
        "fill",
        "--name",
        "Learn more",
        "--exact"
    ]);
    let mut other_selector = reply_steps();
    other_selector[1]["attempts"][0]["argv"] = json!(["click", "@e9"]);
    let mut unknown_tier = reply_steps();
    unknown_tier[1]["tier"] = json!("vision");
    let mut unknown_attempt = reply_steps();
    unknown_attempt[1]["attempts"][0]["tier"] = json!("vision");
    let mut late_attempt = reply_steps();
    late_attempt[0]["attempts"] = json!([
        {"tier": "selector", "argv": ["find", "role", "link", "click", "--name", "Learn more", "--exact"], "ok": false},
        {"tier": "command", "argv": ["open", "https://evil.example/"], "ok": true}
    ]);
    let cases = [
        ("another command", tampered_command),
        ("another name", tampered_name),
        ("another verb", tampered_verb),
        ("another way to find it", other_selector),
        ("an unknown tier", unknown_tier),
        ("an unknown attempt tier", unknown_attempt),
        ("a later attempt", late_attempt),
    ];
    for (what, steps) in cases {
        let a = flow_act_with(BrowserLevel::Act, no_limits(), example_flow());
        a.answer(Some(UserAnswer::AllowOnce));
        a.r.runner.lock().flow_run_steps = Some(steps);
        let (error, text) = run(&a, json!({"name": NAME}));
        assert!(error && text.contains("ran differently"), "{what}: {text}");
    }
    // What was recorded, with the same words, is the approved flow.
    let a = flow_act_with(BrowserLevel::Act, no_limits(), example_flow());
    a.answer(Some(UserAnswer::AllowOnce));
    a.r.runner.lock().flow_run_steps = Some(reply_steps());
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(!error, "{text}");
}

#[test]
fn a_parameter_is_filled_in_before_the_command_is_compared() {
    let mut flow = quiet_flow();
    flow["params"] = params_of(&["q"]);
    flow["steps"][0]["command"] = json!(["open", "https://example.com/?q={{q}}"]);
    flow["steps"][0]["action"]["value"] = json!("https://example.com/?q={{q}}");
    let a = flow_act_with(BrowserLevel::Act, no_limits(), flow.clone());
    a.answer(Some(UserAnswer::AllowOnce));
    let (error, text) = run(&a, json!({"name": NAME, "params": {"q": "cats"}}));
    assert!(!error, "the fake replays the filled-in command: {text}");
    // A reply that ran the unfilled placeholder, or another value, is not the approved flow.
    for ran in [
        "https://example.com/?q={{q}}",
        "https://example.com/?q=dogs",
    ] {
        let b = flow_act_with(BrowserLevel::Act, no_limits(), flow.clone());
        b.answer(Some(UserAnswer::AllowOnce));
        let mut steps = reply_steps();
        steps[0]["attempts"][0]["argv"] = json!(["open", ran]);
        b.r.runner.lock().flow_run_steps = Some(steps);
        let (error, text) = run(&b, json!({"name": NAME, "params": {"q": "cats"}}));
        assert!(error && text.contains("ran differently"), "{ran}: {text}");
    }
}

#[test]
fn a_data_flow_that_answers_without_steps_is_not_the_approved_flow() {
    let a = flow_act_with(BrowserLevel::Act, no_limits(), data_flow());
    a.r.runner.lock().flow_run_steps = Some(Value::Null);
    let (error, text) = run(&a, json!({"name": NAME, "params": {"q": "x"}}));
    assert!(error && text.contains("ran differently"), "{text}");
    // A data flow with no steps at all may answer with its data alone.
    let mut empty = data_flow();
    empty["steps"] = json!([]);
    let b = flow_act_with(BrowserLevel::Act, no_limits(), empty);
    b.r.runner.lock().flow_run_steps = Some(Value::Null);
    b.r.runner.lock().flow_run_extra = Some(json!({"list": [], "total": 0}));
    let (error, text) = run(&b, json!({"name": NAME, "params": {"q": "x"}}));
    assert!(!error || !text.contains("ran differently"), "{text}");
}

// ---- N3: a reload or a step back after a flow posted ----

fn posted_card(action: &str, tool: &str) {
    let a = flow_act();
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(!error, "{text}");
    assert!(a.cards().is_empty());
    a.answer(Some(UserAnswer::Deny));
    let (error, text) = a.call(tool, json!({}));
    assert!(error && text.contains("Denied"), "{tool}: {text}");
    assert_eq!(a.cards().len(), 1, "{tool}");
    let card = a.cards()[0].clone();
    match action {
        "reload" => assert!(matches!(card.facts.action, ActionDesc::Reload)),
        _ => assert!(matches!(card.facts.action, ActionDesc::GoBack)),
    }
    let en = card.text_in(Lang::En);
    let vi = card.text_in(Lang::Vi);
    let (en_line, vi_line) = if action == "reload" {
        ("may send the form again", "gửi lại biểu mẫu")
    } else {
        ("Going back may send the form again", "gửi lại biểu mẫu")
    };
    assert!(en.lines.iter().any(|l| l.contains(en_line)), "{en:?}");
    assert!(vi.lines.iter().any(|l| l.contains(vi_line)), "{vi:?}");
    assert_eq!(a.r.runner.count(action), 0, "denied: not sent");
    // Allowed once: it goes, and the next one asks nothing.
    a.answer(Some(UserAnswer::AllowOnce));
    let (error, text) = a.call(tool, json!({}));
    assert!(!error, "{tool}: {text}");
    let cards = a.cards().len();
    let (error, text) = a.call(tool, json!({}));
    assert!(!error && a.cards().len() == cards, "{tool}: {text}");
}

#[test]
fn a_reload_or_a_step_back_after_a_flow_that_writes_asks_first() {
    posted_card("reload", "browser_reload");
    posted_card("back", "browser_back");
    // A flow that only reads leaves nothing to send again.
    let a = flow_act_with(BrowserLevel::Act, no_limits(), data_flow());
    let (error, text) = run(&a, json!({"name": NAME, "params": {"q": "x"}}));
    assert!(!error, "{text}");
    let cards = a.cards().len();
    for tool in ["browser_reload", "browser_back"] {
        let (error, text) = a.call(tool, json!({}));
        assert!(!error, "{tool}: {text}");
    }
    assert_eq!(a.cards().len(), cards);
}

// ---- Windows cost: the grant book is read once per check ----

#[test]
fn the_grant_book_is_read_once_per_check_whatever_the_number_of_steps() {
    let reads_for = |links: i64| {
        let mut flow = quiet_flow();
        for n in 0..links {
            let name = format!("Learn more about item {n}");
            let seq = 20 + n;
            flow = with_step(
                flow,
                step(
                    seq,
                    &["find", "role", "link", "click", "--name", &name, "--exact"],
                    "https://example.com/more",
                    &format!("click the \"{name}\" link"),
                    &name,
                    "link",
                ),
            );
        }
        let a = flow_act_with(BrowserLevel::Act, no_limits(), flow);
        a.answer(Some(UserAnswer::AllowOnce));
        let before = a.up.grant_reads(0);
        let (error, text) = a.call("flow_show", json!({"name": NAME}));
        assert!(!error, "{text}");
        let (_, text) = run(&a, json!({"name": NAME}));
        assert!(!text.contains("not in a shape"), "{text}");
        a.up.grant_reads(0) - before
    };
    let few = reads_for(0);
    let many = reads_for(8);
    assert_eq!(few, many, "{few} reads for 2 steps, {many} for 10");
}

// ---- the cards quiet_act answers are recorded ----

#[test]
fn quiet_act_records_the_cards_it_answers() {
    let r = rig(true);
    let up = upstream(&r, no_limits(), None);
    assert_no_cards(&up);
    // The user's words name one site; another address is outside the task and asks.
    up.run_started(0, "look at https://example.com");
    let (error, text) = call(
        &up,
        "browser_open",
        json!({"url": "https://elsewhere.example"}),
    );
    assert!(!error, "{text}");
    assert!(
        !answered_cards(&up).is_empty(),
        "a card was asked and answered silently, and it is on record"
    );
}
