//! The M2 review findings, round 2: the taint guard parity with the extension backend, who lifts a
//! Stop, the measured act inputs (isolated eval, dialogs, escaped quotes), popups, keys outside a
//! text field, the form index, the upload block list and staging copy, the user's tab approval,
//! one audit row per act and a level that changes mid-run.
//!
//! Nothing here is exempt from the gate's rules (`act()` is not): every test runs the production
//! taint rules. Each test fails on the behaviour it replaced.
use super::act_tests::{
    act, act_with, blocked, login_page, plain_page, with_user_tab, write, Act, LOGIN_URL,
};
use super::fake_page::{focus_button, focus_input, El, FailKind, Page};
use super::review_tests::{categories, delete_page, focus_in_form, gate_for, gate_tool, SESSIONS};
use super::tests::json_of;
use crate::mono::browser::gate::{Gate, Policy, Upstream};
use crate::mono::browser::risk::Category;
use crate::mono::browser::trust::approval::UserAnswer;
use crate::mono::browser::BrowserLevel;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(files_under(&path));
        } else {
            out.push(path);
        }
    }
    out
}

// ---- A: the taint guard of the extension backend, and no more ----

#[test]
fn a_risky_click_granted_for_the_conversation_covers_the_next_one_after_a_read() {
    let a = act();
    let mut page = delete_page();
    page.elements
        .push(El::new("e13", "button", "Delete project").attr("type", "submit"));
    a.on(page);
    a.answer(Some(UserAnswer::AllowConversation));
    let (error, text) = a.call("browser_click", json!({"ref": "e12"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1, "the first risky click asks");
    // The grant covers the same kind of click: no second card, not even after another read.
    a.snapshot();
    let (error, text) = a.call("browser_click", json!({"ref": "e13"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1, "the grant covers it: {:?}", a.cards());
    assert_eq!(a.rows_of("browser_click")[1].decision, "auto");
}

#[test]
fn a_fill_on_an_ordinary_page_after_a_read_does_not_ask() {
    let a = act();
    a.on_at(plain_page(), "https://notes.example/list");
    let (error, text) = a.call("browser_read", json!({}));
    assert!(!error, "{text}");
    let (error, text) = a.call("browser_fill", json!({"ref": "e1", "text": "buy milk"}));
    assert!(!error, "{text}");
    assert!(a.cards().is_empty(), "{:?}", a.cards());
    assert_eq!(a.rows_of("browser_fill")[0].decision, "auto");
}

#[test]
fn enter_in_the_sign_in_form_after_a_read_does_not_ask() {
    let a = act();
    // The sign-in page of the other tests, with a button that names what it does.
    let mut page = login_page();
    page.form.as_mut().unwrap()["submits"][0]["label"] = json!("Sign in");
    page.elements[6].name = "Sign in".into();
    a.on(page);
    let (error, text) = a.call("browser_press", json!({"key": "Enter", "ref": "e3"}));
    assert!(!error, "{text}");
    assert!(a.cards().is_empty(), "{:?}", a.cards());
    assert_eq!(a.rows_of("browser_press")[0].decision, "auto");
}

#[test]
fn what_was_read_on_one_site_still_asks_when_typed_on_another() {
    // The extension backend's Tainted category stays: data read on one site and carried to
    // another asks.
    let a = act();
    let mut page = plain_page();
    page.elements
        .push(El::new("e3", "text", "Order orderreference1234567890"));
    a.on_at(page, "https://shop.example/orders");
    // The task names the other site too, so the only question left is the carried data.
    a.up.note_user_domain(0, "https://notes.example/new");
    a.call(
        "browser_navigate",
        json!({"url": "https://notes.example/new"}),
    );
    a.snapshot();
    let (error, text) = a.call(
        "browser_fill",
        json!({"ref": "e1", "text": "orderreference1234567890"}),
    );
    assert!(!error, "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1, "carrying read data to another site asks");
    assert!(categories(&cards[0]).contains(&Category::Tainted));
}

#[test]
fn a_declared_intent_never_makes_a_card_on_the_ardia_gate() {
    let session = SESSIONS.fetch_add(1, Ordering::SeqCst);
    let a = act();
    a.set_page(delete_page());
    let (gate, route) = gate_for(&a, session, BrowserLevel::Act);
    a.up.note_user_domain(session, LOGIN_URL);
    gate.begin_turn(&route, "look at my account");
    gate_tool(&gate, &route, "browser_open", json!({"url": LOGIN_URL}));
    gate_tool(
        &gate,
        &route,
        "browser_snapshot",
        json!({"interactive": true}),
    );
    // After a read the extension gate asked for each declaration; here it is only noted.
    let declared = gate_tool(
        &gate,
        &route,
        "browser_declare_intent",
        json!({"actions": [{"kind": "write", "target_desc": "Search", "url": LOGIN_URL}]}),
    );
    assert!(declared["isError"] != true, "{declared}");
    assert!(a.cards().is_empty(), "a declaration never asks");
    // And the click that follows is judged by the classifier alone.
    let done = gate_tool(&gate, &route, "browser_click", json!({"ref": "e9"}));
    assert!(done["isError"] != true, "{done}");
    assert!(a.cards().is_empty());
}

// ---- B: a Stop is lifted by a real user message only ----

#[test]
fn only_a_message_the_user_wrote_lifts_a_stop() {
    let a = act();
    a.on_login();
    a.up.stop_turn(0);
    for run_without_words in ["", "   "] {
        // A scheduler, workflow, internal or Mono-to-Mono run starts with no user prompt.
        a.up.run_started(0, run_without_words);
        let (error, text) = a.call("browser_snapshot", json!({"interactive": true}));
        assert!(error && text.contains("stopped the browser"), "{text}");
    }
    a.up.run_started(0, "carry on");
    let (error, text) = a.call("browser_snapshot", json!({"interactive": true}));
    assert!(!error, "{text}");
}

// ---- measured act inputs ----

#[test]
fn the_focus_script_is_sent_isolated_before_the_dashes() {
    let a = act();
    a.on(delete_page());
    a.r.runner.lock().page.focus = Some(focus_input("text", Some("note"), Some("Note")));
    let (error, text) = a.call("browser_press", json!({"key": "a"}));
    assert!(!error, "{text}");
    let argv =
        a.r.runner
            .lock()
            .calls
            .iter()
            .find(|c| c.iter().any(|w| w == "eval"))
            .cloned()
            .expect("the focus eval ran");
    let at = |word: &str| argv.iter().position(|w| w == word).unwrap();
    assert!(
        at("eval") < at("--isolated") && at("--isolated") < at("--"),
        "{argv:?}"
    );
    assert_eq!(argv.last().unwrap(), super::upstream::FOCUS_SCRIPT);
}

#[test]
fn a_key_pressed_in_an_element_reports_where_it_landed() {
    let a = act();
    a.on_login();
    for (key, target) in [("Enter", "e3"), ("Control+Enter", "e6")] {
        let (error, text) = a.call("browser_press", json!({"key": key, "ref": target}));
        assert!(!error, "{key}: {text}");
    }
    assert_eq!(
        a.sent(),
        vec![
            vec!["press".to_owned(), "@e3".into(), "Enter".into()],
            vec!["press".to_owned(), "@e6".into(), "Control+Enter".into()],
        ]
    );
}

fn dialog_page(full: &str, modal: Value, name: &str) -> Page {
    Page {
        title: "Shop".into(),
        elements: vec![El::new("e1", "button", name).attr("type", "button")],
        forms: vec![],
        form: None,
        full: Some(full.to_owned()),
        modal: Some(modal),
        ..Page::default()
    }
}

fn unclear_card(a: &Act) -> bool {
    a.cards()
        .iter()
        .any(|c| categories(c).contains(&Category::Unclear))
}

#[test]
fn a_modal_page_whose_dialog_cannot_be_found_asks_as_unclear() {
    let a = act();
    // `modal` says a dialog is open, but the full snapshot shows the button outside any dialog.
    a.on_at(
        dialog_page(
            "- main\n  - button \"Save\" [ref=e1]\n",
            json!({"role": "dialog", "name": "", "ref": null}),
            "Save",
        ),
        "https://shop.example/page",
    );
    let (error, text) = a.call("browser_click", json!({"ref": "e1"}));
    assert!(!error, "{text}");
    assert!(unclear_card(&a), "{:?}", a.cards());
}

#[test]
fn a_ref_that_is_not_one_unique_matching_line_asks_as_unclear() {
    for full in [
        // The ref twice.
        "- dialog \"Payment\"\n  - button \"Save\" [ref=e1]\n  - button \"Cancel\" [ref=e1]\n",
        // The line has another name than the refs gave.
        "- dialog \"Payment\"\n  - button \"Delete everything\" [ref=e1]\n",
        // And another role.
        "- dialog \"Payment\"\n  - link \"Save\" [ref=e1]\n",
    ] {
        let a = act();
        a.on_at(
            dialog_page(
                full,
                json!({"role": "dialog", "name": "Payment", "ref": null}),
                "Save",
            ),
            "https://shop.example/page",
        );
        let (error, text) = a.call("browser_click", json!({"ref": "e1"}));
        assert!(!error, "{text}");
        assert!(unclear_card(&a), "{full}: {:?}", a.cards());
    }
}

#[test]
fn a_named_dialog_with_an_escaped_quote_in_a_name_is_read_whole() {
    let a = act();
    let fixture = json_of(include_str!("fixtures/browse_snapshot_dialog_named.json"));
    let full = fixture["data"]["snapshot"]
        .as_str()
        .unwrap()
        .replace("\"Confirm\"", "\"Confirm \\\"now\\\"\"")
        .replace("ref=e2", "ref=e1");
    a.on_at(
        dialog_page(&full, fixture["data"]["modal"].clone(), "Confirm \"now\""),
        "https://shop.example/page",
    );
    let (error, text) = a.call("browser_click", json!({"ref": "e1"}));
    assert!(!error, "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1, "a payment dialog asks");
    assert!(
        !categories(&cards[0]).contains(&Category::Unclear),
        "{:?}",
        cards[0]
    );
    assert!(cards[0].facts.in_dialog);
    assert!(cards[0]
        .facts
        .dialog_text
        .as_deref()
        .unwrap_or("")
        .contains("5,000,000"));
}

// ---- popups ----

pub(super) fn page_with_links() -> Page {
    let mut page = login_page();
    page.elements.push(
        El::new("e50", "link", "Docs")
            .attr("href", "https://www.iana.org/docs")
            .attr("target", "_blank"),
    );
    page.elements.push(
        El::new("e51", "link", "Same")
            .attr("href", "https://www.iana.org/same")
            .attr("target", "_self"),
    );
    page.elements.push(
        El::new("e52", "button", "Popup")
            .attr("type", "button")
            .attr("opens_popup", "https://popup.example/x"),
    );
    page.elements.push(
        El::new("e54", "button", "Popup with a token")
            .attr("type", "button")
            .attr(
                "opens_popup",
                "https://popup.example/x?token=abc123secret456",
            ),
    );
    page
}

#[test]
fn a_link_with_a_target_is_loaded_in_the_session_tab_and_never_clicked() {
    let a = act();
    a.on(page_with_links());
    let (error, text) = a.call("browser_click", json!({"ref": "e50"}));
    assert!(!error, "{text}");
    assert!(text.contains("same tab"), "{text}");
    assert_eq!(
        a.r.runner.lock().popup_clicks,
        0,
        "the link was not clicked"
    );
    assert!(a.sent().is_empty(), "no click command: {:?}", a.sent());
    assert_eq!(a.r.runner.count("open -- https://www.iana.org/docs"), 1);
    assert!(a.r.runner.lock().popups.is_empty());
    assert!(a.r.runner.lock().switches.is_empty());
    assert!(a.rows_of("browser_click")[0]
        .summary
        .contains("loaded in this tab"));
    // `_self` is an ordinary click.
    a.on(page_with_links());
    let (error, text) = a.call("browser_click", json!({"ref": "e51"}));
    assert!(!error, "{text}");
    assert_eq!(a.sent().len(), 1);
}

#[test]
fn a_link_with_a_target_to_a_blocked_origin_is_not_opened_anywhere() {
    let a = act_with(BrowserLevel::Act, blocked("https://www.iana.org"));
    a.on(page_with_links());
    let (error, text) = a.call("browser_click", json!({"ref": "e50"}));
    assert!(error && text.contains("link leads to"), "{text}");
    assert_eq!(a.r.runner.count("open -- https://www.iana.org"), 0);
    assert!(a.sent().is_empty());
}

#[test]
fn a_popup_is_closed_the_window_profile_put_back_and_the_mono_told() {
    let a = act();
    a.on(page_with_links());
    let (error, text) = a.call("browser_click", json!({"ref": "e52"}));
    assert!(!error, "{text}");
    assert!(
        text.contains("opened a new window") && text.contains("zptah closed it"),
        "{text}"
    );
    assert!(text.contains("https://popup.example/x"), "{text}");
    let st = a.r.runner.lock();
    assert!(st.popups.is_empty(), "the new window is gone");
    assert_eq!(st.target_closes, vec!["POPUP0001".to_owned()]);
    assert_eq!(
        st.switches,
        vec![json!({"window": "w-1", "profile": "default"})]
    );
    assert_eq!(st.window_profile, "default");
    assert_eq!(st.tabs.len(), 1, "the session's own tab is untouched");
    drop(st);
    let closed = a.rows_of("popup_closed");
    assert_eq!(closed.len(), 1);
    assert_eq!(a.rows_of("window_profile_restored").len(), 1);
    assert!(a.rows_of("browser_click")[0]
        .summary
        .contains("closed 1 new window"));
}

#[test]
fn the_address_of_a_popup_is_redacted_for_the_mono_and_the_audit() {
    let a = act();
    a.on(page_with_links());
    let (error, text) = a.call("browser_click", json!({"ref": "e54"}));
    assert!(!error, "{text}");
    assert!(text.contains("opened a new window"), "{text}");
    assert!(!text.contains("abc123secret456"), "{text}");
    let conn = rusqlite::Connection::open(&a.db).unwrap();
    let urls: Vec<String> = conn
        .prepare("SELECT url FROM mono_browser_audit WHERE tool='popup_closed'")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(urls.len(), 1);
    assert!(!urls[0].contains("abc123secret456"), "{urls:?}");
}

#[test]
fn the_close_call_has_its_options_before_the_dashes_and_the_method_after() {
    let a = act();
    a.on(page_with_links());
    a.call("browser_click", json!({"ref": "e52"}));
    let argv =
        a.r.runner
            .lock()
            .calls
            .iter()
            .find(|c| c.iter().any(|w| w == "Target.closeTarget"))
            .cloned()
            .expect("the close ran");
    let tail: Vec<&str> = argv
        .iter()
        .skip_while(|w| *w != "cdp")
        .map(String::as_str)
        .collect();
    assert_eq!(
        tail,
        [
            "cdp",
            "--param",
            "targetId=POPUP0001",
            "--target",
            "browser",
            "--",
            "Target.closeTarget"
        ]
    );
}

#[test]
fn a_target_that_is_a_tab_is_never_closed() {
    let a = act();
    a.on(page_with_links());
    a.r.runner.lock().on_cmd = Some((
        "click".into(),
        Box::new(|st| {
            st.new_tabs.push((
                "TABTARGET01".into(),
                "tab-other-7".into(),
                "https://x.example/".into(),
            ))
        }),
    ));
    let (error, text) = a.call("browser_click", json!({"ref": "e52"}));
    assert!(!error, "{text}");
    let st = a.r.runner.lock();
    assert_eq!(
        st.target_closes,
        vec!["POPUP0001".to_owned()],
        "only the window without a tab id"
    );
    assert!(st.new_tabs.iter().any(|t| t.0 == "TABTARGET01"));
}

#[test]
fn a_popup_that_cannot_be_closed_is_reported_to_the_mono() {
    let a = act();
    a.on(page_with_links());
    a.r.runner.lock().close_target_fails = true;
    let (error, text) = a.call("browser_click", json!({"ref": "e52"}));
    assert!(!error, "{text}");
    assert!(text.contains("could not close"), "{text}");
    assert!(a.rows_of("popup_closed").is_empty());
}

#[test]
fn a_page_that_opens_nothing_costs_no_close_and_no_switch() {
    let a = act();
    a.on_login();
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    let st = a.r.runner.lock();
    assert!(st.target_closes.is_empty() && st.switches.is_empty());
}

// ---- 1: keys outside a text field ----

#[test]
fn a_character_key_is_refused_unless_a_text_field_has_the_focus() {
    let a = act();
    a.on(delete_page());
    // Nothing focused.
    let (error, text) = a.call("browser_press", json!({"key": "c"}));
    assert!(
        error && text.contains("only sent into a text field"),
        "{text}"
    );
    // A button focused.
    a.r.runner.lock().page.focus = Some(focus_button("Search", Some(0)));
    let (error, text) = a.call("browser_press", json!({"key": "#"}));
    assert!(
        error && text.contains("only sent into a text field"),
        "{text}"
    );
    // A character key pressed in a ref that is not a text field.
    let (error, text) = a.call("browser_press", json!({"key": "a", "ref": "e9"}));
    assert!(
        error && text.contains("only sent into a text field"),
        "{text}"
    );
    assert!(a.sent().is_empty(), "{:?}", a.sent());
    // Where a text field has it, the key is sent.
    a.r.runner.lock().page.focus = Some(focus_input("text", Some("note"), Some("Note")));
    let (error, text) = a.call("browser_press", json!({"key": "a"}));
    assert!(!error, "{text}");
    assert_eq!(a.sent().len(), 1);
}

#[test]
fn keys_that_move_or_scroll_stay_allowed_with_nothing_focused() {
    let a = act();
    a.on(delete_page());
    let keys = [
        "Tab",
        "ArrowDown",
        "ArrowUp",
        "Escape",
        "PageUp",
        "PageDown",
        "Home",
        "End",
        "Space",
    ];
    for key in keys {
        let (error, text) = a.call("browser_press", json!({"key": key}));
        assert!(!error, "{key}: {text}");
    }
    assert_eq!(a.sent().len(), keys.len());
    assert!(a.cards().is_empty(), "{:?}", a.cards());
}

#[test]
fn delete_and_backspace_outside_a_text_field_or_with_an_unknown_focus_ask_as_unclear() {
    let a = act();
    a.on(delete_page());
    // Nothing focused: a page shortcut could delete something.
    let (error, text) = a.call("browser_press", json!({"key": "Delete"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1);
    assert!(categories(&a.cards()[0]).contains(&Category::Unclear));
    // A focus the gate could not read.
    a.r.runner.lock().focus_fails = true;
    let (error, text) = a.call("browser_press", json!({"key": "Backspace"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 2);
    assert!(categories(&a.cards()[1]).contains(&Category::Unclear));
    // In a text field it is plain editing.
    a.r.runner.lock().focus_fails = false;
    a.r.runner.lock().page.focus = Some(focus_input("text", Some("note"), Some("Note")));
    let (error, text) = a.call("browser_press", json!({"key": "Backspace"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 2, "no card in a text field");
}

// ---- 2: the form index ----

fn two_form_page() -> Page {
    let mut page = delete_page();
    page.forms = vec![
        Some("/search".into()),
        Some("https://pay.example/charge".into()),
    ];
    page
}

fn focus_with(index: Option<usize>, forms: usize) -> Value {
    let mut focus = focus_in_form(index.unwrap_or(0), "Search");
    focus["fi"] = json!(index);
    focus["fl"] = json!(forms);
    focus
}

#[test]
fn a_focus_form_index_counts_only_when_the_form_count_matches() {
    // The count matches: only the focused form (the first, a safe one) is judged.
    let a = act_with(BrowserLevel::Act, blocked("https://pay.example"));
    a.on(two_form_page());
    a.r.runner.lock().page.focus = Some(focus_with(Some(0), 2));
    let (error, text) = a.call("browser_press", json!({"key": "Enter"}));
    assert!(!error, "{text}");
    // A form came or went between the two reads: the index may be another form, so every form
    // of the page is judged.
    let b = act_with(BrowserLevel::Act, blocked("https://pay.example"));
    b.on(two_form_page());
    b.r.runner.lock().page.focus = Some(focus_with(Some(0), 3));
    let (error, text) = b.call("browser_press", json!({"key": "Enter"}));
    assert!(error && text.contains("form leads to"), "{text}");
    assert!(b.sent().is_empty());
    // The index is unknown: the same.
    let c = act_with(BrowserLevel::Act, blocked("https://pay.example"));
    c.on(two_form_page());
    c.r.runner.lock().page.focus = Some(focus_with(None, 2));
    let (error, text) = c.call("browser_press", json!({"key": "Enter"}));
    assert!(error && text.contains("form leads to"), "{text}");
}

// ---- 3: the upload block list ----

#[test]
fn what_apps_keep_of_web_sessions_is_never_uploaded() {
    let a = act();
    a.on_login();
    let home = a.path("home");
    a.configure(vec![home.clone()]);
    for place in [
        "Library/HTTPStorages/com.example.app/cookies.binarycookies",
        "Library/HTTPStorages/com.ardia.browser.binarycookies",
        "Library/Cookies/Cookies.binarycookies",
        "Library/Caches/com.ardia.browser/Cache_Data/f_000001",
        "Library/Preferences/com.ardia.browser.plist",
    ] {
        write(&home.join(place), b"private");
        let (error, text) = a.call(
            "browser_upload",
            json!({"ref": "e11", "paths": [home.join(place).to_string_lossy()]}),
        );
        assert!(error && text.contains("never uploaded"), "{place}: {text}");
    }
    assert!(a.sent().is_empty());
    assert!(files_under(&a.path("staging")).is_empty());
}

// ---- 4: the private copy ----

fn project_file(a: &Act, name: &str, bytes: &[u8]) -> PathBuf {
    let file = a.path("home/proj").join(name);
    write(&file, bytes);
    std::fs::canonicalize(&file).unwrap()
}

fn upload_call(a: &Act, file: &Path) -> (bool, String) {
    a.call(
        "browser_upload",
        json!({"ref": "e11", "paths": [file.to_string_lossy()]}),
    )
}

#[test]
fn ardia_gets_a_private_copy_never_the_original() {
    let a = act();
    a.on_login();
    let file = project_file(&a, "report.txt", b"approved bytes");
    let (error, text) = upload_call(&a, &file);
    assert!(!error, "{text}");
    let sent = a.sent();
    let copy = PathBuf::from(&sent[0][2]);
    assert!(
        copy.starts_with(a.path("staging")) && copy != file,
        "{}",
        copy.display()
    );
    assert_eq!(std::fs::read(&copy).unwrap(), b"approved bytes");
    for call in &a.r.runner.lock().calls {
        assert!(
            !call.iter().any(|word| Path::new(word) == file),
            "the original reached ardia: {call:?}"
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&copy), 0o600);
        assert_eq!(mode(copy.parent().unwrap()), 0o700);
    }
}

#[test]
fn a_file_changed_after_the_upload_is_sent_does_not_change_the_copy() {
    let a = act();
    a.on_login();
    let file = project_file(&a, "report.txt", b"approved bytes");
    let overwrite = file.clone();
    a.r.runner.lock().on_cmd = Some((
        "upload".into(),
        Box::new(move |_| std::fs::write(&overwrite, b"swapped later!").unwrap()),
    ));
    let (error, text) = upload_call(&a, &file);
    assert!(!error, "{text}");
    let copy = PathBuf::from(&a.sent()[0][2]);
    assert_eq!(std::fs::read(copy).unwrap(), b"approved bytes");
}

#[test]
fn a_file_swapped_while_the_user_was_asked_never_reaches_the_upload() {
    let a = act();
    a.on_login();
    let file = project_file(&a, "report.txt", b"approved bytes");
    let swapped = file.clone();
    *a.on_card.lock().unwrap() = Some(Box::new(move || {
        std::fs::write(&swapped, b"a different, longer file").unwrap()
    }));
    let (error, text) = upload_call(&a, &file);
    assert!(
        error && text.contains("changed while the user was asked"),
        "{text}"
    );
    assert!(a.sent().is_empty());
    assert!(
        files_under(&a.path("staging")).is_empty(),
        "nothing was copied"
    );
    assert_eq!(a.rows_of("browser_upload").len(), 1);
}

#[cfg(unix)]
#[test]
fn a_file_replaced_by_a_link_while_the_user_was_asked_never_reaches_the_upload() {
    let a = act();
    a.on_login();
    let file = project_file(&a, "report.txt", b"approved bytes");
    let secret = a.path("elsewhere/secret.txt");
    write(&secret, b"secret");
    let (link, target) = (file.clone(), secret.clone());
    *a.on_card.lock().unwrap() = Some(Box::new(move || {
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
    }));
    let (error, text) = upload_call(&a, &file);
    assert!(error, "{text}");
    assert!(a.sent().is_empty());
    assert!(files_under(&a.path("staging")).is_empty());
}

#[test]
fn the_copies_are_kept_until_the_conversation_ends() {
    let a = act();
    a.on_login();
    let file = project_file(&a, "report.txt", b"approved bytes");
    let (error, text) = upload_call(&a, &file);
    assert!(!error, "{text}");
    let copy = PathBuf::from(&a.sent()[0][2]);
    // The page reads the file when the form is submitted, so the copy outlives the upload.
    assert!(copy.exists());
    assert_eq!(files_under(&a.path("staging")).len(), 1);
    a.up.end_conversation(0);
    assert!(!copy.exists(), "deleted when the conversation ended");
    assert!(files_under(&a.path("staging")).is_empty());
    assert!(file.exists(), "the original is never touched");
}

#[test]
fn two_files_with_the_same_name_get_their_own_folders() {
    let a = act();
    a.on_login();
    let one = project_file(&a, "a/report.txt", b"one");
    let two = project_file(&a, "b/report.txt", b"two");
    let (error, text) = a.call(
        "browser_upload",
        json!({"ref": "e11", "paths": [one.to_string_lossy(), two.to_string_lossy()]}),
    );
    assert!(!error, "{text}");
    let sent = &a.sent()[0];
    assert_eq!(sent.len(), 4);
    assert_ne!(sent[2], sent[3]);
    assert_eq!(std::fs::read(&sent[2]).unwrap(), b"one");
    assert_eq!(std::fs::read(&sent[3]).unwrap(), b"two");
}

// ---- 5: an act that was stopped, or whose page could not be read ----

#[test]
fn a_stop_during_an_act_does_not_close_the_tab() {
    let a = act();
    a.on_login();
    let up = a.up.clone();
    a.r.runner.lock().on_cmd = Some(("click".into(), Box::new(move |_| up.stop_turn(0))));
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(error && text.contains("stopped the browser"), "{text}");
    assert!(!text.contains("Stopped: after"), "{text}");
    assert_eq!(a.r.runner.lock().tabs.len(), 1, "the tab stays open");
    assert!(a.rows_of("origin_blocked").is_empty());
}

#[test]
fn the_address_the_act_reported_is_used_when_the_page_cannot_be_read() {
    let a = act();
    a.on_login();
    a.r.runner.lock().on_cmd = Some((
        "click".into(),
        Box::new(|st| st.fail_on = Some(("get".into(), FailKind::Garbage))),
    ));
    // The link's click reports `changes.url.to`: that address is checked and used.
    let (error, text) = a.call("browser_click", json!({"ref": "e8"}));
    assert!(!error, "{text}");
    assert!(text.contains("iana.org"), "{text}");
    assert_eq!(a.r.runner.lock().tabs.len(), 1);
    assert!(a.rows_of("origin_blocked").is_empty());
}

#[test]
fn a_reported_address_on_a_blocked_origin_still_stops_the_tab() {
    let a = act_with(BrowserLevel::Act, blocked("https://www.iana.org"));
    let mut page = login_page();
    // A button whose script navigates (the click itself shows no address to check before).
    page.elements.push(
        El::new("e53", "button", "Go")
            .attr("type", "button")
            .attr("navigates_to", "https://www.iana.org/domains/example"),
    );
    a.on(page);
    a.r.runner.lock().on_cmd = Some((
        "click".into(),
        Box::new(|st| st.fail_on = Some(("get".into(), FailKind::Garbage))),
    ));
    let (error, text) = a.call("browser_click", json!({"ref": "e53"}));
    assert!(error && text.contains("Stopped"), "{text}");
    assert!(a.r.runner.lock().tabs.is_empty(), "the own tab is closed");
}

// ---- 6: the user's tab, by id and site ----

#[test]
fn the_same_tab_id_on_another_site_asks_again() {
    let a = act();
    a.on_login();
    with_user_tab(&a, "https://docs.example/page");
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1);
    a.call(
        "browser_tabs",
        json!({"action": "close", "tab": "tab-user-1"}),
    );
    // The same id and site: no second card.
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1, "the same tab on the same site");
    a.call(
        "browser_tabs",
        json!({"action": "close", "tab": "tab-user-1"}),
    );
    // The user moved the tab to another site: a new approval.
    a.r.runner.lock().user_tab = Some((
        "tab-user-1".into(),
        "https://mail.example/inbox".into(),
        "Mail".into(),
    ));
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 2, "another site asks again");
}

// ---- 7: one audit row per act ----

#[test]
fn a_refusal_after_the_card_writes_one_row() {
    let a = act_with(BrowserLevel::Act, blocked("https://pay.example"));
    a.on(delete_page());
    // While the user is asked the button learns a `formaction` to a blocked origin.
    *a.on_card.lock().unwrap() = Some(Box::new({
        let runner = a.r.runner.clone();
        move || {
            let mut st = runner.lock();
            for el in &mut st.page.elements {
                if el.r == "e12" {
                    el.attrs
                        .insert("formaction".into(), "https://pay.example/charge".into());
                }
            }
        }
    }));
    let (error, text) = a.call("browser_click", json!({"ref": "e12"}));
    assert!(error, "{text}");
    assert_eq!(a.cards().len(), 1);
    let rows = a.rows_of("browser_click");
    assert_eq!(rows.len(), 1, "one row for one act: {rows:?}");
    assert_eq!(rows[0].decision, "blocked");
}

// ---- 8: the level follows the gate's policy ----

#[test]
fn the_upstream_follows_a_level_raised_while_it_runs() {
    let session = SESSIONS.fetch_add(1, Ordering::SeqCst);
    // Launched read-only; the user raises the level in the settings.
    let a = act_with(BrowserLevel::ReadOnly, super::tests::no_limits());
    a.set_page(delete_page());
    let (gate, route) = gate_for(&a, session, BrowserLevel::ReadOnly);
    gate_tool(&gate, &route, "browser_open", json!({"url": LOGIN_URL}));
    gate_tool(
        &gate,
        &route,
        "browser_snapshot",
        json!({"interactive": true}),
    );
    gate.set_policy(Policy::ardia(BrowserLevel::Act, vec![]));
    let done = gate_tool(&gate, &route, "browser_click", json!({"ref": "e9"}));
    assert!(done["isError"] != true, "{done}");
    assert_eq!(a.sent().len(), 1);
}

#[test]
fn a_level_lowered_while_a_card_is_open_stops_the_act() {
    let session = SESSIONS.fetch_add(1, Ordering::SeqCst);
    let a = act();
    a.set_page(delete_page());
    let (gate, route) = gate_for(&a, session, BrowserLevel::Act);
    gate_tool(&gate, &route, "browser_open", json!({"url": LOGIN_URL}));
    gate_tool(
        &gate,
        &route,
        "browser_snapshot",
        json!({"interactive": true}),
    );
    let lowered: Arc<Gate> = gate.clone();
    *a.on_card.lock().unwrap() = Some(Box::new(move || {
        lowered.set_policy(Policy::ardia(BrowserLevel::ReadOnly, vec![]));
    }));
    let done = gate_tool(&gate, &route, "browser_click", json!({"ref": "e12"}));
    assert_eq!(done["isError"], true, "{done}");
    assert!(
        done["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("read only"),
        "{done}"
    );
    assert!(a.sent().is_empty(), "nothing ran");
    assert_eq!(a.rows_of("browser_click").len(), 1);
}

#[test]
fn the_policy_level_reaches_the_upstream_through_the_trait() {
    // `Upstream::policy_changed` is what the gate calls on `set_policy` and `configure`.
    let a = act_with(BrowserLevel::ReadOnly, super::tests::no_limits());
    a.on_login();
    let (error, _) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(error, "read only at launch");
    a.up.policy_changed(&Policy::ardia(BrowserLevel::Act, vec![]));
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
}
