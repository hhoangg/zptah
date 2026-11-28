//! Tests of the reviewer nits of the Ardia core: profile-scoped ending, a bounded quit, the
//! "watch was lost" flag and tabs opened while a session ends.
use super::controller::*;
use super::tests::*;
use super::upstream::*;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

const OTHER_PROFILE: &str = "p-other";

#[test]
fn a_profile_change_closes_only_the_old_profiles_tabs() {
    let r = rig(true);
    r.controller.ensure_live(PROFILE).unwrap();
    let up = upstream(&r, no_limits(), None);
    call(&up, "browser_open", json!({"url": "https://example.com"}));
    assert_eq!(r.runner.lock().tabs, ["tab-fake-1"]);
    // The Mono picks another profile, and a tab of the new profile opens right away.
    up.configure(Config {
        profile: OTHER_PROFILE.into(),
        origins: no_limits(),
        output_dir: Some(std::env::temp_dir()),
        ..Config::default()
    });
    r.controller.add_tab(
        &session_name("mono-1", 0),
        "mono-1",
        OTHER_PROFILE,
        "tab-new",
    );
    r.runner.lock().tabs.push("tab-new".into());
    assert!(wait_until(|| !r
        .runner
        .lock()
        .tabs
        .contains(&"tab-fake-1".to_owned())));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        r.runner.lock().tabs,
        ["tab-new"],
        "the new profile's tab stays"
    );
    assert!(r
        .controller
        .owns_tab(&session_name("mono-1", 0), OTHER_PROFILE, "tab-new"));
}

#[test]
fn end_mono_with_none_closes_every_profile_and_with_a_profile_only_that_one() {
    let r = rig(true);
    r.controller.ensure_live(PROFILE).unwrap();
    for (id, profile) in [("a", PROFILE), ("b", OTHER_PROFILE)] {
        r.controller.add_tab("s", "mono-1", profile, id);
        r.runner.lock().tabs.push(id.into());
    }
    r.controller.end_mono("mono-1", Some(PROFILE));
    assert_eq!(r.runner.lock().tabs, ["b"]);
    r.controller.end_mono("mono-1", None);
    assert!(r.runner.lock().tabs.is_empty());
}

#[test]
fn a_quit_is_bounded_by_the_launch_wait_even_with_a_slow_platform_and_many_tabs() {
    // Ardia never goes away and the quit itself is slow: the old code closed every tab first
    // (up to 15 s each) and then waited; now the total is quit time plus one launch wait.
    let r = rig_with(false, |p| {
        p.quit_delay = Duration::from_millis(100);
        p.stays_up = true;
    });
    launched(&r);
    for n in 0..8 {
        let id = format!("tab-{n}");
        r.controller.add_tab("s", "mono-1", PROFILE, &id);
        r.runner.lock().tabs.push(id);
    }
    let started = std::time::Instant::now();
    r.controller.quit_now().unwrap();
    let took = started.elapsed();
    // launch_wait is 60 ms in the rig, the quit 100 ms, one poll of slack.
    assert!(took < Duration::from_millis(600), "{took:?}");
    assert_eq!(r.runner.count("tabs close"), 0, "no per-tab command");
    assert_eq!(r.platform.quits.load(Ordering::SeqCst), 1);
}

#[test]
fn a_restarted_watcher_does_not_clear_the_lost_watch_flag_only_a_closed_ardia_does() {
    let r = rig(false);
    r.runner.lock().end_watch = true;
    launched(&r);
    assert!(wait_until(|| !r.controller.watching()));
    // The stream works again and a live check restarts the watcher.
    r.runner.lock().end_watch = false;
    let before = r.runner.lock().watch_calls;
    r.controller.ensure_live(PROFILE).unwrap();
    assert!(wait_until(|| r.runner.lock().watch_calls > before));
    std::thread::sleep(Duration::from_millis(30));
    assert!(!r.controller.watching(), "still lost after the restart");
    r.minutes(20);
    assert_eq!(r.controller.idle_tick(), IdleOutcome::NotOurs);
    assert_eq!(r.platform.quits.load(Ordering::SeqCst), 0);
    // Ardia seen closed: the flag resets.
    r.runner.lock().up = false;
    let _ = r.controller.check_live(PROFILE, false);
    assert!(r.controller.watching());
}

#[test]
fn a_tab_opened_while_a_session_ends_is_closed_too() {
    let r = rig(true);
    r.controller.ensure_live(PROFILE).unwrap();
    r.controller.add_tab("s", "mono-1", PROFILE, "tab-1");
    r.runner.lock().tabs.push("tab-1".into());
    let controller = r.controller.clone();
    r.runner.lock().on_close = Some(Box::new(move |st| {
        controller.add_tab("s", "mono-1", PROFILE, "tab-late");
        st.tabs.push("tab-late".into());
    }));
    r.controller.end_mono("mono-1", None);
    assert!(r.runner.lock().tabs.is_empty(), "the late tab is closed");
    assert!(r.controller.session_tabs("s").is_empty());
    assert!(r.controller.sessions().is_empty());
}
