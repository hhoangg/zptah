//! Fixes of the review of the flow milestone (design spec sections 6 and 7, 12b "Flows"): the
//! measured shapes of an action flow (`fixtures/flow_show_params.json`, `flow_steps_after.json`,
//! `flow_steps_typed.json`, `flow_run_site.json`, `flow_show_ambiguous.json`), flows told apart by
//! `(site, name)`, parameters, secrets, files, the reply of a run, the save card, and the small
//! fixes of the same round (uploads without a staging folder, popups, sweeps, repeats, reloads).
use super::act_tests::*;
use super::fake_page::FailKind;
use super::flow_tests::*;
use super::tests::*;
use super::upstream::flow_model as model;
use super::upstream::*;
use crate::mono::browser::copy::{ActionDesc, Lang};
use crate::mono::browser::gate::{Origins, Policy, Upstream, Verdict};
use crate::mono::browser::risk::Category;
use crate::mono::browser::trust::approval::UserAnswer;
use crate::mono::browser::trust::grants::grant_label;
use crate::mono::browser::{BrowserLevel, MonoBrowserStore};
use serde_json::{json, Value};
use std::collections::BTreeMap;

const SHOW_PARAMS: &str = include_str!("fixtures/flow_show_params.json");
const STEPS_AFTER: &str = include_str!("fixtures/flow_steps_after.json");
const STEPS_TYPED: &str = include_str!("fixtures/flow_steps_typed.json");
const SHOW_AMBIGUOUS: &str = include_str!("fixtures/flow_show_ambiguous.json");
const RUN_SITE: &str = include_str!("fixtures/flow_run_site.json");

/// The measured login flow (an email, a field with no label, a password), moved onto example.com
/// so the test's task site covers it.
fn login_flow() -> Value {
    let text = SHOW_PARAMS
        .replace("http://127.0.0.1:8765", "https://example.com")
        .replace("127.0.0.1:8765", "example.com");
    let mut flow = json_of(&text)["data"]["flow"].clone();
    flow["name"] = json!(NAME);
    flow
}

fn on_other_site(flow: &Value, from: &str, to: &str) -> Value {
    serde_json::from_str(&flow.to_string().replace(from, to)).unwrap()
}

fn heads(steps: &Value) -> Vec<String> {
    steps
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["command"][0].as_str().unwrap().to_owned())
        .collect()
}

// ---- the measured shapes ----

#[test]
fn the_measured_flow_replies_are_read_and_name_params_as_double_braces_only() {
    let parsed = model::parse_flow(
        &json_of(SHOW_PARAMS)["data"],
        "zptah-m0-login",
        "127.0.0.1:8765",
    )
    .unwrap();
    assert_eq!(
        parsed
            .params
            .iter()
            .map(|p| p.name.as_str())
            .collect::<Vec<_>>(),
        ["email", "nolabel", "pw"]
    );
    assert_eq!(parsed.params[2].source, model::ParamSource::Secret);
    assert!(model::uses_param(&parsed.steps[1], "email"));
    assert!(!model::uses_param(&parsed.steps[1], "nolabel"));
    // The secret step has `<redacted>` in its command and `{{pw}}` as its recorded value.
    assert!(parsed.steps[2].redacted);
    assert!(model::uses_param(&parsed.steps[2], "pw"));
    // The same name for another site is not this flow.
    assert!(model::parse_flow(
        &json_of(SHOW_PARAMS)["data"],
        "zptah-m0-login",
        "localhost:8765"
    )
    .is_err());
    // The other spellings are not placeholders.
    for spelling in ["{email}", "${email}", "$email", "<email>", "%email%"] {
        let mut flow = parsed.steps[1].clone();
        flow.command = vec![
            "find".into(),
            "label".into(),
            spelling.into(),
            "fill".into(),
            spelling.into(),
        ];
        flow.value = Some(spelling.to_owned());
        flow.action_value = Some(spelling.to_owned());
        assert!(!model::uses_param(&flow, "email"), "{spelling}");
    }
    let params = BTreeMap::from([("email".to_owned(), "a@b.example".to_owned())]);
    assert_eq!(
        model::substitute("x {email} {{email}}", &params),
        "x {email} a@b.example"
    );
    // The measured replies of the same family.
    assert_eq!(json_of(SHOW_AMBIGUOUS)["code"], "command_failed");
    assert_eq!(json_of(RUN_SITE)["data"]["site"], "localhost:8765");
    // Steps the gate's own probes would make are not recorded (measured).
    for fixture in [STEPS_AFTER, STEPS_TYPED] {
        let recorded = heads(&json_of(fixture)["data"]["steps"]);
        assert!(
            recorded
                .iter()
                .all(|h| !matches!(h.as_str(), "snapshot" | "eval" | "get")),
            "{recorded:?}"
        );
    }
}

// ---- F1: files ----

#[test]
fn a_step_that_uploads_or_names_a_file_path_refuses_the_run() {
    let url = "https://example.com/";
    let upload = json!({"seq": 13, "command": ["upload", "@e5", "/Users/user/cv.pdf"], "url": url, "ok": true, "requests": [],
        "action": {"type": "upload", "intent": "attach the file", "candidates": [], "fingerprint": null}});
    let find_upload = json!({"seq": 13, "command": ["find", "role", "button", "upload", "--name", "CV", "--exact"], "url": url, "ok": true, "requests": [],
        "action": {"type": "upload", "intent": "attach", "candidates": [], "fingerprint": null}});
    let typed_path = |value: &str| {
        step(
            13,
            &[
                "find", "role", "textbox", "fill", value, "--name", "Notes", "--exact",
            ],
            url,
            "enter text",
            "Notes",
            "textbox",
        )
    };
    let cases = vec![
        ("upload", upload),
        ("find upload", find_upload),
        ("absolute path", typed_path("/etc/passwd")),
        ("home path", typed_path("~/notes.txt")),
        ("relative path", typed_path("../secrets.txt")),
        ("windows path", typed_path("C:\\Users\\user\\cv.pdf")),
    ];
    for (what, bad) in cases {
        let a = flow_act_with(BrowserLevel::Act, no_limits(), with_step(quiet_flow(), bad));
        a.answer(Some(UserAnswer::AllowConversation));
        let (error, text) = run(&a, json!({"name": NAME}));
        assert!(error && text.contains("file path"), "{what}: {text}");
        assert!(runs(&a).is_empty(), "{what}");
        assert!(a.cards().is_empty(), "{what}: no card can override it");
        assert_eq!(a.rows_of("flow_run")[0].decision, "blocked", "{what}");
        assert_eq!(a.r.runner.count("tabs open"), 0, "{what}");
    }
    // A file address is refused as an address that is not a web page, before any step is read.
    let a = flow_act_with(
        BrowserLevel::Act,
        no_limits(),
        with_step(quiet_flow(), typed_path("file:///tmp/x")),
    );
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(
        error && text.contains("not a web page") && runs(&a).is_empty(),
        "{text}"
    );
    // A name that only looks like a path is not one.
    let slash = step(
        13,
        &[
            "find",
            "role",
            "link",
            "click",
            "--name",
            "/Settings",
            "--exact",
        ],
        "https://example.com/more",
        "click the \"/Settings\" link",
        "/Settings",
        "link",
    );
    let a = flow_act_with(
        BrowserLevel::Act,
        no_limits(),
        with_step(quiet_flow(), slash),
    );
    a.answer(Some(UserAnswer::AllowOnce));
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(!error, "{text}");
}

// ---- F2: parameters ----

#[test]
fn a_declared_parameter_no_step_names_asks_and_other_spellings_do_not_count() {
    // `{q}` is not a placeholder: the parameter goes nowhere zptah can see.
    let mut flow = with_step(
        quiet_flow(),
        step(
            13,
            &[
                "find", "role", "textbox", "fill", "{q}", "--name", "Search", "--exact",
            ],
            "https://example.com/more",
            "type into Search",
            "Search",
            "textbox",
        ),
    );
    flow["params"] = params_of(&["q"]);
    let a = flow_act_with(BrowserLevel::Act, no_limits(), flow);
    let (error, text) = run(&a, json!({"name": NAME, "params": {"q": "tea"}}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1);
    assert!(a.cards()[0].facts.categories.contains(&Category::Unclear));
    assert!(!a.cards()[0].offer_conversation, "never granted");
    assert!(risky_of(&a, 0)
        .iter()
        .any(|r| r.contains("not named by any step")));
    // A parameter that is declared but no step names asks even when it is not passed.
    let mut unused = quiet_flow();
    unused["params"] = params_of(&["ghost"]);
    let b = flow_act_with(BrowserLevel::Act, no_limits(), unused);
    let (error, text) = run(&b, json!({"name": NAME}));
    assert!(!error, "{text}");
    assert_eq!(b.cards().len(), 1);
    assert!(b.cards()[0].facts.categories.contains(&Category::Unclear));
    // Named as `{{q}}`: nothing to ask about.
    let mut named = with_step(
        quiet_flow(),
        step(
            13,
            &[
                "find", "role", "textbox", "fill", "{{q}}", "--name", "Search", "--exact",
            ],
            "https://example.com/more",
            "type into Search",
            "Search",
            "textbox",
        ),
    );
    named["params"] = params_of(&["q"]);
    let c = flow_act_with(BrowserLevel::Act, no_limits(), named);
    let (error, text) = run(&c, json!({"name": NAME, "params": {"q": "tea"}}));
    assert!(!error, "{text}");
    assert!(c.cards().is_empty(), "{:?}", risky_of(&c, 0));
}

#[test]
fn a_parameter_value_is_checked_on_every_site_of_the_flow_even_with_no_typing_step() {
    // A card number goes nowhere a step types, and still asks (never granted).
    let mut flow = with_step(
        quiet_flow(),
        step(
            13,
            &[
                "find", "role", "link", "click", "--name", "{{q}}", "--exact",
            ],
            "https://example.com/more",
            "open the item",
            "{{q}}",
            "link",
        ),
    );
    flow["params"] = params_of(&["q"]);
    let a = flow_act_with(BrowserLevel::Act, no_limits(), flow.clone());
    let (error, text) = run(
        &a,
        json!({"name": NAME, "params": {"q": "4111111111111111"}}),
    );
    assert!(!error, "{text}");
    assert!(a.cards()[0]
        .facts
        .categories
        .contains(&Category::CredentialEntry));
    assert!(!a.cards()[0].offer_conversation);
    // Text read on another site and carried in a parameter is tainted on this one.
    let b = flow_act_with(BrowserLevel::Act, no_limits(), flow);
    b.up.note_user_domain(0, "https://other.example/");
    let (error, text) = b.call("browser_open", json!({"url": "https://other.example/"}));
    assert!(!error, "{text}");
    let (error, text) = b.call("browser_read", json!({}));
    assert!(!error, "{text}");
    let (error, text) = run(&b, json!({"name": NAME, "params": {"q": "documentation"}}));
    assert!(!error, "{text}");
    assert!(
        b.cards()[0].facts.categories.contains(&Category::Tainted),
        "{:?}",
        b.cards()[0].facts.categories
    );
}

#[test]
fn an_address_a_parameter_went_into_is_checked_even_on_an_origin_already_looked_at() {
    let mut flow = with_step(
        quiet_flow(),
        json!({"seq": 13, "command": ["open", "https://example.com/s?q={{q}}"], "url": "https://example.com/s", "ok": true, "requests": [],
            "action": {"type": "open", "intent": "open the results", "candidates": [], "fingerprint": null, "value": "https://example.com/s?q={{q}}"}}),
    );
    flow["params"] = params_of(&["q"]);
    let a = flow_act_with(BrowserLevel::Act, no_limits(), flow);
    a.up.note_user_domain(0, "https://other.example/");
    a.call("browser_open", json!({"url": "https://other.example/"}));
    a.call("browser_read", json!({}));
    let (error, text) = run(&a, json!({"name": NAME, "params": {"q": "documentation"}}));
    assert!(!error, "{text}");
    let risky = risky_of(&a, 0).join("\n");
    assert!(risky.contains("goes to example.com"), "{risky}");
    assert!(a.cards()[0].facts.categories.contains(&Category::Tainted));
}

// ---- F3: the card shows the values ----

#[test]
fn the_card_shows_each_value_clipped_and_cleaned_and_masks_only_a_card_number() {
    let mut flow = with_step(
        example_flow(),
        step(
            13,
            &[
                "find", "role", "textbox", "fill", "{{q}}", "--name", "Search", "--exact",
            ],
            "https://www.iana.org/help/example-domains",
            "type into Search",
            "Search",
            "textbox",
        ),
    );
    flow["params"] = params_of(&["q", "n"]);
    let a = flow_act_with(BrowserLevel::Act, no_limits(), flow);
    let long = format!("line one\nline two {}", "z".repeat(200));
    let (error, text) = run(
        &a,
        json!({"name": NAME, "params": {"q": long, "n": "4111 1111 1111 1111"}}),
    );
    assert!(!error, "{text}");
    let lines = card_lines(&a, 0).join("\n");
    assert!(lines.contains("- n: \"<card number>\""), "{lines}");
    assert!(!lines.contains("4111"), "{lines}");
    assert!(lines.contains("- q: \"line one line two zzz"), "{lines}");
    assert!(!lines.contains(&"z".repeat(100)), "clipped: {lines}");
    let vi = a.cards()[0].text_in(Lang::Vi).lines.join("\n");
    assert!(
        vi.contains("Tham số:") && vi.contains("- n: \"<card number>\""),
        "{vi}"
    );
}

// ---- F4: secrets ----

#[test]
fn a_flow_with_a_secret_parameter_or_a_redacted_value_is_never_run() {
    // The measured login flow: its password step is redacted and `pw` is a secret.
    let a = flow_act_with(BrowserLevel::Act, no_limits(), login_flow());
    a.answer(Some(UserAnswer::AllowConversation));
    let (error, text) = run(
        &a,
        json!({"name": NAME, "params": {"email": "a@b.example"}}),
    );
    assert!(error && text.contains("Not run"), "{text}");
    assert!(runs(&a).is_empty() && a.cards().is_empty());
    assert_eq!(a.rows_of("flow_run")[0].decision, "invariant");
    // A secret parameter alone (no redacted step) is enough.
    let mut flow = login_flow();
    flow["steps"].as_array_mut().unwrap().remove(2);
    let b = flow_act_with(BrowserLevel::Act, no_limits(), flow.clone());
    let (error, text) = run(&b, json!({"name": NAME}));
    assert!(error && text.contains("secret parameter"), "{text}");
    assert!(runs(&b).is_empty() && b.cards().is_empty());
    assert_eq!(b.rows_of("flow_run")[0].decision, "invariant");
    // A redacted value in a step that is no field (nothing names a password) is enough too.
    flow["params"] = params_of(&["email", "nolabel", "pw"]);
    let mut odd = quiet_flow();
    odd["steps"].as_array_mut().unwrap().push(json!({"seq": 13, "command": ["open", "https://example.com/?k=<redacted>"], "url": "https://example.com/", "ok": true, "requests": [],
        "action": {"type": "open", "intent": "open", "candidates": [], "fingerprint": null}}));
    let c = flow_act_with(BrowserLevel::Act, no_limits(), odd);
    let (error, text) = run(&c, json!({"name": NAME}));
    assert!(error && text.contains("Not run"), "{text}");
    assert!(runs(&c).is_empty());
    // Without the secret the same flow runs: email and the unlabeled field are typed values.
    let mut ok = login_flow();
    ok["steps"].as_array_mut().unwrap().remove(2);
    ok["params"] = params_of(&["email", "nolabel"]);
    let d = flow_act_with(BrowserLevel::Act, no_limits(), ok);
    d.answer(Some(UserAnswer::AllowOnce));
    let (error, text) = run(
        &d,
        json!({"name": NAME, "params": {"email": "a@b.example", "nolabel": "hello"}}),
    );
    assert!(!error, "{text}");
}

// ---- F5: the reply is the flow that was approved ----

#[test]
fn a_run_that_does_not_match_the_approved_flow_halts_the_tab_and_tells_the_user() {
    let ok_step = |seq: i64, intent: &str| json!({"seq": seq, "intent": intent, "ok": true, "tier": "command", "attempts": []});
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "another intent",
            json!([
                ok_step(11, "open the start page"),
                ok_step(12, "click the \"Delete\" button")
            ]),
            json!({}),
        ),
        (
            "another number",
            json!([
                ok_step(11, "open the start page"),
                ok_step(99, "click the \"Learn more\" link")
            ]),
            json!({}),
        ),
        (
            "fewer steps for a run that went through",
            json!([ok_step(11, "open the start page")]),
            json!({}),
        ),
        (
            "more steps",
            json!([
                ok_step(11, "open the start page"),
                ok_step(12, "click the \"Learn more\" link"),
                ok_step(13, "extra")
            ]),
            json!({}),
        ),
        (
            "another site",
            Value::Null,
            json!({"site": "other.example"}),
        ),
        ("another flow", Value::Null, json!({"flow": "other-flow"})),
    ];
    for (what, steps, extra) in cases {
        let a = flow_act_with(BrowserLevel::Act, no_limits(), quiet_flow());
        if !steps.is_null() {
            a.r.runner.lock().flow_run_steps = Some(steps);
        }
        a.r.runner.lock().flow_run_extra = Some(extra);
        let (error, text) = run(&a, json!({"name": NAME}));
        assert!(error && text.contains("ran differently"), "{what}: {text}");
        assert!(text.contains("Tell the user"), "{what}: {text}");
        let rows = a.rows_of("flow_run");
        assert!(
            rows.iter()
                .any(|r| r.reason.contains("flow changed during run")),
            "{what}: {rows:?}"
        );
        assert!(
            a.r.runner.lock().tabs.is_empty(),
            "{what}: the tab was closed"
        );
        let (error, _) = a.call("browser_read", json!({}));
        assert!(error, "{what}: the tab is halted for the turn");
    }
    // A run that stopped at a failed step is a prefix of the flow, not a different flow.
    let a = flow_act_with(BrowserLevel::Act, no_limits(), quiet_flow());
    a.r.runner.lock().flow_run_steps = Some(json!([
        {"seq": 11, "intent": "open the start page", "ok": false, "tier": "command", "attempts": []}
    ]));
    a.r.runner.lock().flow_run_extra = Some(json!({"ok": false}));
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(
        error && !text.contains("ran differently") && text.contains("failed"),
        "{text}"
    );
}

// ---- F6: a flow is (site, name) ----

fn two_sites() -> Act {
    let a = flow_act_with(BrowserLevel::Act, no_limits(), quiet_flow());
    let other = on_other_site(&quiet_flow(), "example.com", "example.net");
    a.r.runner.lock().flows.push(other);
    a.up.note_user_domain(0, "https://example.net/");
    a
}

#[test]
fn a_name_on_two_sites_needs_the_site_and_the_site_is_always_passed() {
    let a = two_sites();
    let (error, text) = a.call("flow_show", json!({"name": NAME}));
    assert!(
        error && text.contains("more than one site") && text.contains("example.net"),
        "{text}"
    );
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(error && text.contains("more than one site"), "{text}");
    assert!(runs(&a).is_empty());
    let (error, text) = a.call("flow_show", json!({"name": NAME, "site": "example.net"}));
    assert!(!error && text.contains("Site: example.net"), "{text}");
    assert!(!calls_with(&a, "--json flow show --site example.net -- zptah-m0-test").is_empty());
    let (error, text) = run(&a, json!({"name": NAME, "site": "example.net"}));
    assert!(!error, "{text}");
    assert_eq!(
        runs(&a),
        vec![vec![
            "flow",
            "run",
            "--site",
            "example.net",
            "--yes",
            "--",
            NAME
        ]]
    );
    // The first site is the other flow.
    let (error, text) = run(&a, json!({"name": NAME, "site": "example.com"}));
    assert!(!error, "{text}");
    assert_eq!(runs(&a)[1][3], "example.com");
    // A site that can not be an option value, or that has no such flow, runs nothing.
    for bad in ["-x", "a b", "x/y"] {
        let (error, _) = run(&a, json!({"name": NAME, "site": bad}));
        assert!(error, "{bad}");
    }
    let (error, text) = run(&a, json!({"name": NAME, "site": "nowhere.example"}));
    assert!(error && text.contains("No saved flow"), "{text}");
    assert_eq!(runs(&a).len(), 2);
    // One flow of that name: the site is found, no `site` needed.
    let b = flow_act();
    let (error, text) = run(&b, json!({"name": NAME}));
    assert!(!error, "{text}");
    assert_eq!(runs(&b)[0][3], "example.com");
}

#[test]
fn an_answer_for_another_site_is_not_the_flow_that_was_asked_for() {
    let a = flow_act();
    let mut other = on_other_site(&quiet_flow(), "example.com", "example.net");
    other["site"] = json!("example.net");
    a.r.runner.lock().flow_show_raw =
        Some(json!({"success": true, "data": {"flow": other}, "error": null}).to_string());
    let (error, text) = run(&a, json!({"name": NAME, "site": "example.com"}));
    assert!(
        error && text.contains("not in a shape zptah can read"),
        "{text}"
    );
    assert!(runs(&a).is_empty());
}

#[test]
fn a_flow_grant_is_labeled_with_the_flow_and_its_site() {
    let a = flow_act_with(BrowserLevel::Act, no_limits(), example_flow());
    a.answer(Some(UserAnswer::AllowConversation));
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(!error, "{text}");
    let grants = MonoBrowserStore::at(a.db.clone()).list_grants(&a.mono, None);
    let (key, _) = grants
        .iter()
        .find(|(k, _)| k.starts_with("flow:"))
        .unwrap_or_else(|| panic!("{grants:?}"));
    assert_eq!(grant_label(key), "Flow zptah-m0-test on example.com");
    assert!(!grant_label(key).contains(":flow") && !grant_label(key).contains('@'));
    // A site is shown as it is.
    assert_eq!(grant_label("shop.example"), "shop.example");
}

// ---- F7: Stop ----

#[test]
fn a_stop_says_the_flow_may_have_run_partly() {
    let a = flow_act();
    a.r.runner.lock().hang_on = Some("flow run".into());
    let worker = std::thread::spawn({
        let up = a.up.clone();
        move || call(&up, "flow_run", json!({"name": NAME}))
    });
    assert!(wait_until(|| a.r.runner.lock().hanging == 1));
    a.up.stop_turn(0);
    let (error, text) = worker.join().unwrap();
    assert!(
        error && text.contains("may have run partly") && text.contains("may or may not have run"),
        "{text}"
    );
    assert_eq!(a.r.runner.lock().killed, 1);
}

// ---- F8: the save card ----

fn steps_of(fixture: &str) -> Vec<Value> {
    json_of(fixture)["data"]["steps"]
        .as_array()
        .unwrap()
        .clone()
}

#[test]
fn the_save_card_lists_the_steps_after_saved_through_hides_what_was_typed_and_names_the_site() {
    let a = flow_act_with(BrowserLevel::Act, no_limits(), quiet_flow());
    // Four steps were saved already (seq 1 to 4); four are new (5 to 8).
    let mut all = steps_of(STEPS_TYPED);
    all.extend(steps_of(STEPS_AFTER));
    a.r.runner.lock().flow_steps = Some(json!(all));
    a.r.runner.lock().flow_saved_through = Some(4);
    let (error, text) = a.call("flow_save", json!({"name": "slow-flow", "actions": true}));
    assert!(!error, "{text}");
    let ActionDesc::FlowSave {
        steps,
        site,
        listed,
        replaces,
        ..
    } = &a.cards()[0].facts.action
    else {
        panic!("{:?}", a.cards()[0].facts.action);
    };
    assert_eq!(
        (*steps, site.as_str(), *replaces),
        (4, "127.0.0.1:8765", false)
    );
    assert_eq!(listed.len(), 4);
    assert!(
        listed[0].starts_with("5: open http://127.0.0.1:8765/slow.html"),
        "{listed:?}"
    );
    let lines = card_lines(&a, 0).join("\n");
    assert!(lines.contains("Site: 127.0.0.1:8765."), "{lines}");
    assert!(
        lines.contains("- 6: find role button click --name Step A --exact"),
        "{lines}"
    );
    // Everything is saved: nothing to save.
    a.r.runner.lock().flow_saved_through = Some(8);
    let (error, text) = a.call("flow_save", json!({"name": "slow-flow"}));
    assert!(error && text.contains("no recorded steps"), "{text}");
    // What was typed is hidden on the card: a literal is `<text, N characters>`, `<redacted>` stays.
    let b = flow_act_with(BrowserLevel::Act, no_limits(), quiet_flow());
    b.r.runner.lock().flow_steps = Some(json!(steps_of(STEPS_TYPED)));
    b.r.runner.lock().flow_saved_through = Some(0);
    let (error, text) = b.call("flow_save", json!({"name": "login-flow"}));
    assert!(!error, "{text}");
    let lines = card_lines(&b, 0).join("\n");
    assert!(
        !lines.contains("a@b.example") && !lines.contains("hello"),
        "{lines}"
    );
    assert!(
        lines.contains("fill <text, 11 characters> --name Email"),
        "{lines}"
    );
    assert!(lines.contains("fill <redacted> --name Secret"), "{lines}");
    // M3-3: what was typed is stored as it was typed.
    assert!(lines.contains("exactly as they were typed"), "{lines}");
    let vi = b.cards()[0].text_in(Lang::Vi).lines.join("\n");
    assert!(
        vi.contains("đúng như đã nhập") && vi.contains("Trang: 127.0.0.1:8765."),
        "{vi}"
    );
}

#[test]
fn a_save_replaces_only_a_flow_of_the_same_name_on_the_same_site() {
    let a = flow_act_with(BrowserLevel::Act, no_limits(), quiet_flow());
    a.r.runner.lock().flow_steps = Some(json!(steps_of(STEPS_AFTER)));
    a.r.runner.lock().flow_saved_through = Some(4);
    // The same name on another site (example.com): not replaced.
    let (error, text) = a.call("flow_save", json!({"name": NAME}));
    assert!(!error, "{text}");
    match &a.cards()[0].facts.action {
        ActionDesc::FlowSave { replaces, .. } => assert!(!*replaces),
        other => panic!("{other:?}"),
    }
    // The same name on the same site: replaced.
    let mut same = quiet_flow();
    same["site"] = json!("127.0.0.1:8765");
    a.r.runner.lock().flows.push(same);
    let (error, text) = a.call("flow_save", json!({"name": NAME}));
    assert!(!error, "{text}");
    match &a.cards()[1].facts.action {
        ActionDesc::FlowSave { replaces, .. } => assert!(*replaces),
        other => panic!("{other:?}"),
    }
    assert!(card_lines(&a, 1)
        .iter()
        .any(|l| l.contains("already exists on this site")));
}

#[test]
fn the_run_card_shows_the_target_from_the_command_next_to_the_intent() {
    let mut flow = quiet_flow();
    // The recording says one thing, the command another.
    flow["steps"][1]["action"]["intent"] = json!("click the \"Learn more\" link");
    flow["steps"][1]["command"] = json!([
        "find",
        "role",
        "button",
        "click",
        "--name",
        "Delete account",
        "--exact"
    ]);
    let a = flow_act_with(BrowserLevel::Act, no_limits(), flow);
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(!error, "{text}");
    let risky = risky_of(&a, 0).join("\n");
    assert!(
        risky.contains("Learn more") && risky.contains("button \"Delete account\""),
        "{risky}"
    );
}

// ---- M3-2: values are masked, hidden flows are hidden ----

#[test]
fn flow_show_masks_typed_values_and_keeps_placeholders() {
    let mut flow = with_step(
        quiet_flow(),
        json!({"seq": 13, "command": ["find", "role", "textbox", "fill", "my private note", "--name", "Notes", "--exact"], "url": "https://example.com/", "ok": true, "requests": [],
            "action": {"type": "fill", "intent": "enter my private note in the \"Notes\" textbox", "candidates": [{"by": "role", "value": "textbox", "name": "Notes"}], "fingerprint": {"role": "textbox", "name": "Notes"}, "value": "my private note"}}),
    );
    flow = with_step(
        flow,
        json!({"seq": 14, "command": ["find", "role", "textbox", "fill", "{{q}}", "--name", "Search", "--exact"], "url": "https://example.com/", "ok": true, "requests": [],
            "action": {"type": "fill", "intent": "enter {{q}} in the \"Search\" textbox", "candidates": [{"by": "role", "value": "textbox", "name": "Search"}], "fingerprint": {"role": "textbox", "name": "Search"}, "value": "{{q}}"}}),
    );
    flow["params"] = params_of(&["q"]);
    let a = flow_act_with(BrowserLevel::ReadOnly, no_limits(), flow);
    let (error, text) = a.call("flow_show", json!({"name": NAME}));
    assert!(!error, "{text}");
    assert!(!text.contains("private note"), "{text}");
    assert!(text.contains("<text, 15 characters>"), "{text}");
    assert!(text.contains("fill {{q}} --name Search"), "{text}");
    // The measured secret parameter is told to the Mono as one it can not fill.
    let b = flow_act_with(BrowserLevel::ReadOnly, no_limits(), login_flow());
    let (error, text) = b.call("flow_show", json!({"name": NAME}));
    assert!(!error && text.contains("pw (secret"), "{text}");
}

#[test]
fn flows_of_a_blocked_origin_are_hidden_from_the_list_and_the_show() {
    let evil = on_other_site(&quiet_flow(), "example.com", "evil.example");
    let mut evil = evil;
    evil["name"] = json!("evil-flow");
    let a = flow_act_with(
        BrowserLevel::ReadOnly,
        blocked("https://evil.example"),
        quiet_flow(),
    );
    a.r.runner.lock().flows.push(evil);
    let (error, list) = a.call("flow_list", json!({}));
    assert!(
        !error && list.contains(NAME) && !list.contains("evil"),
        "{list}"
    );
    let (error, text) = a.call("flow_show", json!({"name": "evil-flow"}));
    assert!(error && !text.contains("evil.example"), "{text}");
    let (error, text) = a.call(
        "flow_show",
        json!({"name": "evil-flow", "site": "evil.example"}),
    );
    assert!(error && text.contains("No saved flow"), "{text}");
}

// ---- M3-6: the gate's probes are not recorded ----

#[test]
fn the_gates_own_probes_never_become_steps_of_a_saved_flow() {
    let a = act_with(BrowserLevel::Act, no_limits());
    a.on_login();
    let (error, text) = a.call("browser_fill", json!({"ref": "e3", "text": "a@b.example"}));
    assert!(!error, "{text}");
    let (error, text) = a.call("browser_click", json!({"ref": "e7"}));
    assert!(!error, "{text}");
    // The gate did probe the page (focus eval, attributes, forms) ...
    assert!(a.r.runner.count("snapshot") > 0);
    // ... and only the acts reached the recorder (the fake's `act_sent`, as Ardia 0.7.0 does).
    let recorded: Vec<String> = a.sent().iter().map(|c| c[0].clone()).collect();
    assert_eq!(recorded, ["fill", "click"]);
}

// ---- the user's own tab ----

#[test]
fn a_flow_does_not_run_in_the_users_own_tab() {
    let a = flow_act();
    a.on_login();
    with_user_tab(&a, "https://docs.example/page");
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    let (error, text) = run(&a, json!({"name": NAME}));
    assert!(error && text.contains("user's own tab"), "{text}");
    assert!(runs(&a).is_empty());
    assert_eq!(a.rows_of("flow_run")[0].decision, "blocked");
}

// ---- M3-1: the rules text ----

#[test]
fn the_rules_agree_with_the_tools() {
    use crate::mono::browser::{prompt_block_for, MonoBrowser};
    let at = |trust| MonoBrowser {
        trust,
        ..MonoBrowser::default()
    };
    let read = prompt_block_for(&at(BrowserLevel::ReadOnly), true, true);
    assert!(read.contains("`flow_list`, `flow_show`"), "{read}");
    assert!(!read.contains("`flow_run`"), "{read}");
    let act = prompt_block_for(&at(BrowserLevel::Act), true, true);
    assert!(
        act.contains("adopt_active") && act.contains("approval"),
        "{act}"
    );
    for text in [&read, &act] {
        assert!(!text.contains("Enter a password only when"), "{text}");
        assert!(text.contains("or a password"), "{text}");
    }
    // No private upload folder: the Mono is not told about uploads.
    assert!(act.contains("`browser_upload`"));
    let none = prompt_block_for(&at(BrowserLevel::Act), true, false);
    assert!(!none.contains("browser_upload"), "{none}");
}

// ---- R3-3: uploads without a private staging folder ----

#[test]
fn without_a_staging_folder_browser_upload_is_not_offered_anywhere() {
    let list = |up: &std::sync::Arc<ArdiaUpstream>| -> Vec<String> {
        let (reply, _) = up
            .post(
                "",
                None,
                &json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}),
            )
            .unwrap();
        reply.unwrap()["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let a = act();
    assert!(list(&a.up).contains(&"browser_upload".to_owned()));
    let r = rig(true);
    let bare = upstream(&r, no_limits(), None);
    let tools = list(&bare);
    assert!(!tools.contains(&"browser_upload".to_owned()), "{tools:?}");
    assert!(tools.contains(&"browser_click".to_owned()));
    // The gate's policy and the harness's disabled list agree.
    let policy = Policy::ardia(BrowserLevel::Act, vec![]).with_uploads(false);
    assert!(!policy.offers("browser_upload") && policy.offers("browser_click"));
    assert!(matches!(
        policy.evaluate("browser_upload", &json!({})),
        Verdict::Deny(_)
    ));
    assert!(Policy::ardia(BrowserLevel::Act, vec![]).offers("browser_upload"));
    assert!(BrowserLevel::Act
        .disabled_tools_ardia_for(false)
        .contains(&"browser_upload"));
    assert!(!BrowserLevel::Act
        .disabled_tools_ardia_for(true)
        .contains(&"browser_upload"));
}

// ---- N2: a popup of an unknown opener ----

#[test]
fn a_popup_when_the_opener_of_the_tab_is_unknown_is_reported_as_unchecked() {
    let a = act();
    a.r.runner.lock().bind_no_target = true;
    a.on(super::review2_tests::page_with_links());
    let (error, text) = a.call("browser_click", json!({"ref": "e52"}));
    assert!(!error, "{text}");
    assert!(
        text.contains("could not check whether the page opened a new window"),
        "{text}"
    );
}

// ---- N3: the sweep looks at the newest entry ----

#[test]
fn a_staging_folder_is_swept_by_the_newest_entry_inside_not_its_own_time() {
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 24 * 60 * 60);
    let set_old = |path: &std::path::Path| {
        std::fs::File::open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    };
    let a = act();
    a.on_login();
    let root = a.path("staging");
    let live = root.join(staging_name(&session_name(&a.mono, 0)));
    std::fs::create_dir_all(live.join("1")).unwrap();
    std::fs::write(live.join("1/cv.pdf"), b"x").unwrap();
    // The folders look old, the file in them was just written (an upload in progress).
    set_old(&live.join("1"));
    set_old(&live);
    // Nothing is new in this one.
    let stale = root.join(staging_name(&session_name(&a.mono, 0)));
    assert_eq!(stale, live);
    let configure = |mono: &str| {
        let up = ArdiaUpstream::with_bind_delay(
            mono,
            a.r.controller.clone(),
            std::time::Duration::from_millis(1),
        );
        up.configure(Config {
            profile: PROFILE.into(),
            staging_dir: Some(root.clone()),
            ..Config::default()
        });
        up
    };
    let _second = configure("mono-sweep-new");
    assert!(
        live.join("1/cv.pdf").exists(),
        "a fresh file keeps the live folder"
    );
    // Everything old and live: removed (a day is the limit).
    set_old(&live.join("1/cv.pdf"));
    set_old(&live.join("1"));
    set_old(&live);
    let _third = configure("mono-sweep-old");
    assert!(!live.exists(), "all old: swept");
}

// ---- N4: a call that showed a card is not repeated ----

#[test]
fn a_navigation_that_showed_a_card_is_not_repeated_when_ardia_goes_away() {
    let a = act();
    a.on_login();
    a.r.runner.lock().fail_on = Some(("open".into(), FailKind::Gone));
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://other.example/page"}),
    );
    assert!(error, "{text}");
    assert_eq!(
        a.cards().len(),
        1,
        "one card, not asked again for the repeat"
    );
    assert_eq!(
        a.r.runner.count("open -- https://other.example/page"),
        1,
        "sent once, never again"
    );
    // A read with no card is still repeated once (the rule for the tools that only read).
    let b = act();
    b.on_login();
    b.r.runner.lock().gone_on_next_browse = true;
    let (error, text) = b.call("browser_read", json!({}));
    assert!(!error, "{text}");
}

// ---- the nit: no act settings, nothing is judged, nothing opens ----

#[test]
fn without_act_settings_a_page_is_neither_opened_nor_read() {
    let r = rig(true);
    let up = ArdiaUpstream::with_bind_delay(
        "mono-bare",
        r.controller.clone(),
        std::time::Duration::from_millis(1),
    );
    up.configure(Config {
        profile: PROFILE.into(),
        origins: Origins::default(),
        ..Config::default()
    });
    let (error, text) = call(&up, "browser_open", json!({"url": "https://example.com/"}));
    assert!(error && text.contains("no settings loaded"), "{text}");
    assert_eq!(r.runner.count("tabs open"), 0);
    // The settings go away after a page was opened: reading it is refused too.
    let a = act();
    a.on_login();
    a.up.configure(Config {
        profile: PROFILE.into(),
        origins: Origins::default(),
        output_dir: None,
        database: None,
        act: None,
        project_dirs: vec![],
        home: None,
        staging_dir: None,
    });
    let (error, text) = a.call("browser_snapshot", json!({}));
    assert!(error && text.contains("no settings loaded"), "{text}");
}

// ---- reload after a POST ----

#[test]
fn a_reload_after_an_act_that_posted_asks_first() {
    let a = act();
    a.on_login();
    {
        let mut st = a.r.runner.lock();
        st.enter_to = Some("https://shop.example/done".into());
        st.posts = vec!["https://shop.example/session".into()];
    }
    let (error, text) = a.call("browser_press", json!({"key": "Enter", "ref": "e3"}));
    assert!(!error, "{text}");
    let before = a.cards().len();
    a.answer(Some(UserAnswer::Deny));
    let (error, text) = a.call("browser_reload", json!({}));
    assert!(error && text.contains("Denied"), "{text}");
    assert_eq!(a.cards().len(), before + 1);
    let card = a.cards().last().unwrap().clone();
    assert!(matches!(card.facts.action, ActionDesc::Reload));
    assert!(card.facts.categories.contains(&Category::Unclear));
    assert!(!card.offer_conversation);
    let en = card.text_in(Lang::En);
    assert!(
        en.lines
            .iter()
            .any(|l| l.contains("may send the form again")),
        "{en:?}"
    );
    let vi = card.text_in(Lang::Vi);
    assert!(
        vi.lines.iter().any(|l| l.contains("gửi lại biểu mẫu")),
        "{vi:?}"
    );
    assert_eq!(a.r.runner.count("reload"), 0, "denied: not reloaded");
    // Allowed once: it reloads, and the next reload asks nothing.
    a.answer(Some(UserAnswer::AllowOnce));
    let (error, text) = a.call("browser_reload", json!({}));
    assert!(!error, "{text}");
    assert_eq!(a.r.runner.count("reload"), 1);
    let cards = a.cards().len();
    let (error, _) = a.call("browser_reload", json!({}));
    assert!(!error);
    assert_eq!(a.cards().len(), cards);
    // A page no act posted to reloads freely.
    let b = act();
    b.on_login();
    let (error, text) = b.call("browser_reload", json!({}));
    assert!(!error && b.cards().is_empty(), "{text}");
}
