//! The Browser page of a Mono's settings: its states, the Ardia readiness, profile and idle quit,
//! and the safety rules. Nothing here runs or looks up Ardia; `Fake` stands in for it.
use super::browser_ui::BrowserOps;
use super::workflows_ui::tests::click;
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::browser::ardia::controller::{IDLE_SETTING_KEY, LiveInfo};
use monocode_core::mono::browser::ardia::{ArdiaProfile, Owner, Readiness, Report};
use monocode_core::mono::browser::{self, BrowserLevel, MonoBrowser, MonoBrowserStore};
use std::sync::{Arc, Mutex as StdMutex};

/// Scrolls the Browser page until `selector` is well inside it, then clicks it: the cards below
/// the Ardia ones are far below the fold of the test window, and a click only lands on what is
/// visible. A selector outside the page (the conversation's) is clicked where it is.
pub(super) fn tap(visual: &mut VisualTestContext, selector: &'static str) {
    use gpui::{ScrollDelta, ScrollWheelEvent, point, px};
    for _ in 0..60 {
        visual.refresh().unwrap();
        let (Some(target), Some(page)) = (
            visual.debug_bounds(selector),
            visual.debug_bounds("mono-browser-page"),
        ) else {
            break;
        };
        let y = target.center().y;
        if y > page.top() + px(120.) && y < page.bottom() - px(60.) {
            break;
        }
        let dy = if y > page.center().y { -240. } else { 240. };
        visual.simulate_event(ScrollWheelEvent {
            position: page.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
            ..Default::default()
        });
        visual.run_until_parked();
    }
    click(visual, selector);
}

/// A fake Ardia: what it answers is set by the test, what was asked is recorded.
pub(super) struct Fake {
    pub(super) report: StdMutex<Report>,
    pub(super) profiles: StdMutex<Result<Vec<ArdiaProfile>, Readiness>>,
    pub(super) can_launch: StdMutex<bool>,
    pub(super) start: StdMutex<Result<bool, Readiness>>,
    pub(super) quit: StdMutex<Result<(), String>>,
    pub(super) started: StdMutex<u32>,
    pub(super) quit_calls: StdMutex<u32>,
    pub(super) idle: StdMutex<Vec<u32>>,
    pub(super) reports_for: StdMutex<Vec<String>>,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            report: StdMutex::new(closed()),
            profiles: StdMutex::new(Ok(listed())),
            can_launch: StdMutex::new(true),
            start: StdMutex::new(Ok(true)),
            quit: StdMutex::new(Ok(())),
            started: StdMutex::new(0),
            quit_calls: StdMutex::new(0),
            idle: StdMutex::new(Vec::new()),
            reports_for: StdMutex::new(Vec::new()),
        }
    }
}
impl Fake {
    pub(super) fn set_report(&self, report: Report) {
        *self.report.lock().unwrap() = report;
    }
}
impl BrowserOps for Fake {
    fn ardia_report(&self, profile: &str) -> Report {
        self.reports_for.lock().unwrap().push(profile.to_owned());
        self.report.lock().unwrap().clone()
    }
    fn ardia_profiles(&self) -> Result<Vec<ArdiaProfile>, Readiness> {
        self.profiles.lock().unwrap().clone()
    }
    fn ardia_start(&self) -> Result<bool, Readiness> {
        *self.started.lock().unwrap() += 1;
        let result = self.start.lock().unwrap().clone();
        if result.is_ok() {
            // Ardia is up now: it lists its profiles.
            *self.profiles.lock().unwrap() = Ok(listed());
            let mut report = self.report.lock().unwrap();
            report.running = true;
            report.owner = Owner::Zptah;
        }
        result
    }
    fn ardia_quit(&self) -> Result<(), String> {
        *self.quit_calls.lock().unwrap() += 1;
        let result = self.quit.lock().unwrap().clone();
        if result.is_ok() {
            *self.report.lock().unwrap() = closed();
            *self.profiles.lock().unwrap() = Err(Readiness::NotRunning);
        }
        result
    }
    fn ardia_can_launch(&self) -> bool {
        *self.can_launch.lock().unwrap()
    }
    fn ardia_owned(&self) -> bool {
        self.report.lock().unwrap().owner == Owner::Zptah
    }
    fn ardia_set_idle_minutes(&self, minutes: u32) {
        self.idle.lock().unwrap().push(minutes);
    }
}

/// Ardia is closed and nothing failed.
pub(super) fn closed() -> Report {
    Report {
        failure: None,
        running: false,
        cached: Some(LiveInfo {
            version: "0.2.49".into(),
            protocol: 4,
        }),
        owner: Owner::Nobody,
    }
}
/// Ardia is up; `owner` says who started it.
pub(super) fn running(owner: Owner) -> Report {
    Report {
        running: true,
        owner,
        ..closed()
    }
}
fn failing(why: Readiness) -> Report {
    Report {
        failure: Some(why),
        ..closed()
    }
}
fn listed() -> Vec<ArdiaProfile> {
    vec![
        ArdiaProfile {
            id: "default".into(),
            name: "Personal".into(),
        },
        ArdiaProfile {
            id: "p-work-1".into(),
            name: "Work".into(),
        },
    ]
}

struct Page {
    s: Setup,
    visual: VisualTestContext,
    ops: Arc<Fake>,
}
fn open_with(cx: &mut TestAppContext, ops: Fake) -> Page {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    let ops = Arc::new(ops);
    let o = ops.clone();
    s.manager.update(cx, |m, _| m.browser_ui.use_fakes(o));
    let mut visual = VisualTestContext::from_window(s.window.into(), cx);
    click(&mut visual, "mono-settings-Browser");
    visual.refresh().unwrap();
    Page { s, visual, ops }
}
fn open(cx: &mut TestAppContext) -> Page {
    open_with(cx, Fake::default())
}

impl Page {
    fn setting(&self) -> Option<MonoBrowser> {
        self.s.manager.read_with(&self.visual, |m, _| {
            m.roster.get(&self.s.id).and_then(|m| m.browser.clone())
        })
    }
    fn on(&mut self, selector: &'static str) -> bool {
        self.visual.debug_bounds(selector).is_some()
    }
    fn top(&mut self, selector: &'static str) -> f32 {
        self.visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not on screen"))
            .top()
            .as_f32()
    }
    /// Lets the worker answers reach the page.
    fn settle(&mut self) {
        self.visual.run_until_parked();
        self.visual.refresh().unwrap();
    }
    fn click(&mut self, selector: &'static str) {
        tap(&mut self.visual, selector);
        self.settle();
    }
    fn manager<R>(&mut self, f: impl FnOnce(&mut Monos, &mut Context<Monos>) -> R) -> R {
        let r = self.s.manager.update(&mut self.visual, f);
        self.settle();
        r
    }
    fn type_into(&mut self, key: &str, text: &str) {
        let field = self
            .s
            .manager
            .read_with(&self.visual, |m, _| m.fields.get(key).cloned())
            .unwrap_or_else(|| panic!("no field {key}"));
        field.update(&mut self.visual, |f, cx| f.set_text(text.to_owned(), cx));
        self.settle();
    }
    fn secrets(&self) -> MonoBrowserStore {
        MonoBrowserStore::for_store(&locked(&self.s.store).unwrap()).unwrap()
    }
    fn enable(&mut self) {
        self.click("mono-browser-enabled");
    }
    fn choose(&mut self, id: &str) {
        let id = id.to_owned();
        self.manager(|m, cx| m.browser_choose_profile(&id, cx));
    }
    fn ui<R>(&self, f: impl FnOnce(&browser_ui::BrowserUi) -> R) -> R {
        self.s
            .manager
            .read_with(&self.visual, |m, _| f(&m.browser_ui))
    }
    fn saved_idle(&self) -> Option<String> {
        locked(&self.s.store)
            .unwrap()
            .setting(IDLE_SETTING_KEY)
            .unwrap()
    }
}

#[gpui::test]
fn the_browser_tab_starts_off_and_shows_only_the_switch(cx: &mut TestAppContext) {
    let mut page = open(cx);
    assert!(page.on("mono-browser-page"));
    assert!(page.on("mono-browser-enabled"));
    for hidden in [
        "mono-browser-readiness-closed",
        "mono-browser-profile",
        "mono-browser-idle-save",
        "mono-browser-control-note",
    ] {
        assert!(!page.on(hidden), "{hidden}");
    }
    assert!(
        page.setting().is_none(),
        "nothing is saved until the user turns it on"
    );
    assert!(
        page.ops.reports_for.lock().unwrap().is_empty(),
        "an Ardia that is not used is never asked"
    );
}

#[gpui::test]
fn turning_it_on_shows_the_ardia_cards_in_order_and_none_of_the_chrome_ones(
    cx: &mut TestAppContext,
) {
    let mut page = open(cx);
    page.enable();
    let setting = page.setting().unwrap();
    assert!(setting.enabled);
    assert_eq!(
        setting.trust,
        BrowserLevel::Act,
        "new Monos act and ask before risky steps"
    );
    // New Monos block nothing: the sensitive-site rule asks on banking and payment sites.
    assert!(setting.blocked_origins.is_empty());
    let order = [
        "mono-browser-readiness-closed",
        "mono-browser-profile",
        "mono-browser-idle-save",
        "mono-browser-control-note",
        browser_ui::trust_selector(BrowserLevel::Act),
    ];
    let tops: Vec<f32> = order.iter().map(|id| page.top(id)).collect();
    assert!(
        tops.windows(2).all(|w| w[0] < w[1]),
        "readiness, profile, idle quit, Control note, then the level: {tops:?}"
    );
    assert!(page.on("mono-browser-ardia-retry"));
    assert!(!page.on("mono-browser-ardia-running"), "Ardia is closed");
    assert!(!page.on("mono-browser-blocked-0"));
    for gone in [
        "mono-browser-prereq-chrome",
        "mono-browser-prereq-fda",
        "mono-browser-fda-fix",
        "mono-connect-status",
        "mono-connect-start",
        "mono-browser-reconnect",
        "mono-browser-notice-dismiss",
        "mono-browser-show-actions",
        "mono-recipes-empty",
        "mono-recipes-import",
        "mono-browser-defaults-switch",
    ] {
        assert!(!page.on(gone), "{gone}");
    }
    // Turning it off again keeps the choices but removes the controls.
    page.enable();
    assert!(!page.setting().unwrap().enabled);
    assert!(!page.on("mono-browser-readiness-closed") && !page.on("mono-browser-profile"));
}

#[gpui::test]
fn the_readiness_row_shows_the_first_failing_check_with_its_fix_and_try_again_rereads_it(
    cx: &mut TestAppContext,
) {
    let mut page = open(cx);
    page.ops.set_report(failing(Readiness::ControlOff));
    page.enable();
    for id in [
        "mono-browser-readiness-check",
        "mono-browser-readiness-problem",
        "mono-browser-readiness-fix",
    ] {
        assert!(page.on(id), "{id}");
    }
    assert!(!page.on("mono-browser-readiness-ok") && !page.on("mono-browser-readiness-closed"));
    let failure = page.ui(|u| u.ardia.report.as_ref().and_then(|r| r.failure.clone()));
    assert_eq!(failure, Some(Readiness::ControlOff));
    assert_eq!(failure.unwrap().check(), "Control on");

    // The user fixes it in Ardia and presses Try again.
    page.ops.set_report(running(Owner::User));
    page.click("mono-browser-ardia-retry");
    assert!(page.on("mono-browser-readiness-ok"));
    assert!(!page.on("mono-browser-readiness-fix"));
    page.ops.set_report(failing(Readiness::NotInstalled));
    page.click("mono-browser-ardia-retry");
    assert!(page.on("mono-browser-readiness-fix") && !page.on("mono-browser-readiness-ok"));
}

#[gpui::test]
fn a_mono_without_a_profile_asks_for_one_and_the_report_is_for_the_saved_profile(
    cx: &mut TestAppContext,
) {
    let mut page = open(cx);
    page.ops.set_report(failing(Readiness::NoProfile));
    page.enable();
    assert_eq!(
        page.ui(|u| u.ardia.report.as_ref().and_then(|r| r.failure.clone())),
        Some(Readiness::NoProfile)
    );
    page.choose("p-work-1");
    assert_eq!(
        page.ops
            .reports_for
            .lock()
            .unwrap()
            .last()
            .map(String::as_str),
        Some("p-work-1"),
        "choosing a profile reads the readiness of that profile"
    );
}

#[gpui::test]
fn a_closed_ardia_shows_what_was_last_seen_and_a_running_one_its_version(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(page.on("mono-browser-readiness-closed") && page.on("mono-browser-readiness-last"));
    page.ops.set_report(running(Owner::User));
    page.click("mono-browser-ardia-retry");
    assert!(page.on("mono-browser-readiness-ok"));
    assert!(!page.on("mono-browser-readiness-closed"));
}

#[gpui::test]
fn the_running_row_and_quit_show_only_for_an_ardia_zptah_started(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    for (report, shown) in [
        (running(Owner::Zptah), true),
        (running(Owner::User), false),
        (running(Owner::Nobody), false),
        (
            Report {
                running: false,
                owner: Owner::Zptah,
                ..closed()
            },
            false,
        ),
    ] {
        assert_eq!(report.quittable(), shown);
        page.ops.set_report(report);
        page.click("mono-browser-ardia-retry");
        assert_eq!(page.on("mono-browser-ardia-running"), shown);
        assert_eq!(page.on("mono-browser-ardia-quit"), shown);
    }
}

#[gpui::test]
fn quit_asks_ardia_to_quit_once_and_the_row_goes_away(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.ops.set_report(running(Owner::Zptah));
    page.enable();
    assert!(page.on("mono-browser-ardia-quit"));
    page.click("mono-browser-ardia-quit");
    assert_eq!(*page.ops.quit_calls.lock().unwrap(), 1);
    assert!(!page.on("mono-browser-ardia-running"));
    assert!(page.on("mono-browser-readiness-closed"));
    assert!(!page.on("mono-browser-ardia-error"));
}

#[gpui::test]
fn a_failed_quit_is_shown_and_the_row_stays(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.ops.set_report(running(Owner::Zptah));
    *page.ops.quit.lock().unwrap() = Err("Ardia would not quit".into());
    page.enable();
    page.click("mono-browser-ardia-quit");
    assert_eq!(*page.ops.quit_calls.lock().unwrap(), 1);
    assert!(page.on("mono-browser-ardia-error"));
    assert!(page.on("mono-browser-ardia-running"));
}

#[gpui::test]
fn the_picker_lists_ardias_profiles_and_choosing_one_saves_its_id_and_name(
    cx: &mut TestAppContext,
) {
    let mut page = open(cx);
    page.enable();
    let ids = page.ui(|u| {
        u.ardia
            .listed()
            .iter()
            .map(|p| (p.id.clone(), p.name.clone()))
            .collect::<Vec<_>>()
    });
    assert_eq!(
        ids,
        [
            ("default".to_owned(), "Personal".to_owned()),
            ("p-work-1".to_owned(), "Work".to_owned())
        ]
    );
    assert!(!page.on("mono-browser-ardia-start") && !page.on("mono-browser-ardia-open"));
    page.click("mono-browser-profile");
    assert!(page.ui(|u| u.select_is_open()));
    assert!(page.on("menu-action-0") && page.on("menu-action-1"));
    page.click("mono-browser-profile");
    assert!(
        !page.ui(|u| u.select_is_open()),
        "the trigger closes it again"
    );

    page.click("mono-browser-profile");
    page.click("menu-action-1");
    let setting = page.setting().unwrap();
    assert_eq!(
        (
            setting.ardia_profile.as_str(),
            setting.ardia_profile_name.as_str()
        ),
        ("p-work-1", "Work")
    );
    assert!(setting.active());
    assert!(!page.ui(|u| u.select_is_open()));
    page.choose("default");
    assert_eq!(page.setting().unwrap().ardia_profile, "default");
}

#[gpui::test]
fn a_closed_ardia_offers_start_ardia_where_zptah_can_launch_it(cx: &mut TestAppContext) {
    let mut page = open(cx);
    *page.ops.profiles.lock().unwrap() = Err(Readiness::NotRunning);
    page.enable();
    assert!(page.on("mono-browser-ardia-start"));
    assert!(!page.on("mono-browser-ardia-open"));
    page.click("mono-browser-ardia-start");
    assert_eq!(*page.ops.started.lock().unwrap(), 1);
    // It started, so its profiles are listed now and the button is gone.
    assert_eq!(page.ui(|u| u.ardia.listed().len()), 2);
    assert!(!page.on("mono-browser-ardia-start"));
    assert!(
        page.on("mono-browser-ardia-running"),
        "zptah started it, so it can quit it"
    );
}

#[gpui::test]
fn a_failed_start_shows_why_and_keeps_the_button(cx: &mut TestAppContext) {
    let mut page = open(cx);
    *page.ops.profiles.lock().unwrap() = Err(Readiness::NotRunning);
    *page.ops.start.lock().unwrap() = Err(Readiness::DidntStart);
    page.enable();
    page.click("mono-browser-ardia-start");
    assert!(page.on("mono-browser-ardia-error"));
    assert!(page.on("mono-browser-ardia-start"));
}

#[gpui::test]
fn a_closed_ardia_says_open_ardia_where_zptah_cannot_launch_it(cx: &mut TestAppContext) {
    let mut page = open(cx);
    *page.ops.profiles.lock().unwrap() = Err(Readiness::NotRunning);
    *page.ops.can_launch.lock().unwrap() = false;
    page.enable();
    assert!(page.on("mono-browser-ardia-open"));
    assert!(!page.on("mono-browser-ardia-start"));
    assert_eq!(*page.ops.started.lock().unwrap(), 0);
}

#[gpui::test]
fn the_saved_profile_is_the_current_choice_even_when_the_list_cannot_be_read(
    cx: &mut TestAppContext,
) {
    let mut page = open(cx);
    page.enable();
    page.choose("p-work-1");
    *page.ops.profiles.lock().unwrap() = Err(Readiness::NotRunning);
    page.click("mono-browser-ardia-retry");
    assert!(page.ui(|u| u.ardia.listed().is_empty()));
    let choices = page.s.manager.read_with(&page.visual, |m, _| {
        m.browser_ardia_choices()
            .into_iter()
            .map(|c| (c.value, c.label.to_string(), c.checked))
            .collect::<Vec<_>>()
    });
    assert_eq!(choices, [("p-work-1".to_owned(), "Work".to_owned(), true)]);
    // The picker still opens, and a valid id always takes effect.
    page.click("mono-browser-profile");
    assert!(page.ui(|u| u.select_is_open()));
    page.choose("default");
    assert_eq!(page.setting().unwrap().ardia_profile, "default");
    page.choose("../etc");
    assert_eq!(page.setting().unwrap().ardia_profile, "default");
}

#[gpui::test]
fn another_mono_on_the_same_profile_gets_a_warning(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.choose("p-work-1");
    assert!(!page.on("mono-browser-shared-warning"));
    page.manager(|m, cx| {
        let other = m.roster.create(Vec::new()).id.clone();
        let mono = m.roster.get_mut(&other).unwrap();
        mono.name = "Other".into();
        mono.browser = Some(MonoBrowser {
            enabled: true,
            ardia_profile: "p-work-1".into(),
            ..Default::default()
        });
        cx.notify();
    });
    assert!(page.on("mono-browser-shared-warning"));
}

#[gpui::test]
fn the_page_no_longer_writes_the_chrome_ready_flag(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(!page.setting().unwrap().ready, "no profile yet");
    page.choose("p-work-1");
    let setting = page.setting().unwrap();
    assert!(
        !setting.ready && setting.active(),
        "the profile alone makes it active; Ardia's readiness replaces the flag"
    );
    // The Mono's rules now carry the browser paragraph for the chosen profile.
    let rules = page.s.manager.read_with(&page.visual, |m, _| {
        monocode_core::mono::instructions(
            m.roster.get(&page.s.id).unwrap(),
            std::path::Path::new("/bin/app"),
        )
    });
    assert!(rules.contains("## Browser") && rules.contains("(\"Work\")"));
    assert!(!rules.to_lowercase().contains("playwright"));
}

#[gpui::test]
fn the_idle_quit_defaults_to_ten_minutes_and_is_applied_when_the_page_opens(
    cx: &mut TestAppContext,
) {
    let mut page = open(cx);
    page.enable();
    assert_eq!(page.ui(|u| u.ardia.idle_minutes), 10);
    assert_eq!(page.ops.idle.lock().unwrap().last(), Some(&10));
    assert_eq!(page.saved_idle(), None, "the default is not written");
}

#[gpui::test]
fn a_saved_idle_quit_is_read_back_and_applied_when_the_page_opens(cx: &mut TestAppContext) {
    let mut page = open(cx);
    locked(&page.s.store)
        .unwrap()
        .save_setting(IDLE_SETTING_KEY, "30")
        .unwrap();
    page.enable();
    assert_eq!(page.ui(|u| u.ardia.idle_minutes), 30);
    assert_eq!(page.ops.idle.lock().unwrap().last(), Some(&30));
}

#[gpui::test]
fn saving_the_idle_quit_writes_the_store_key_and_applies_it_and_zero_means_never(
    cx: &mut TestAppContext,
) {
    let mut page = open(cx);
    page.enable();
    page.type_into("browser-idle-minutes", "25");
    page.click("mono-browser-idle-save");
    assert_eq!(page.saved_idle().as_deref(), Some("25"));
    assert_eq!(page.ops.idle.lock().unwrap().last(), Some(&25));
    assert_eq!(page.ui(|u| u.ardia.idle_minutes), 25);
    page.type_into("browser-idle-minutes", " 0 ");
    page.click("mono-browser-idle-save");
    assert_eq!(page.saved_idle().as_deref(), Some("0"));
    assert_eq!(page.ops.idle.lock().unwrap().last(), Some(&0));
}

#[gpui::test]
fn an_idle_quit_that_is_not_a_number_is_refused_and_changes_nothing(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    for bad in ["soon", "-5", "", "99999999"] {
        page.type_into("browser-idle-minutes", bad);
        page.click("mono-browser-idle-save");
        assert!(page.ui(|u| u.error().is_some()), "{bad:?}");
        assert_eq!(page.saved_idle(), None, "{bad:?}");
    }
    assert_eq!(page.ops.idle.lock().unwrap().last(), Some(&10));
}

#[gpui::test]
fn the_page_says_that_ardias_control_lets_any_local_program_drive_the_browser(
    cx: &mut TestAppContext,
) {
    let mut page = open(cx);
    page.enable();
    assert!(page.on("mono-browser-control-note"));
    assert!(browser_ardia::CONTROL_NOTE.contains("any local program"));
}

#[gpui::test]
fn safety_rules_can_be_edited_and_invalid_origins_are_refused(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(page.setting().unwrap().blocked_origins.is_empty());

    // The lists are far below the fold of the test window, so the actions are called directly;
    // the buttons call the same methods.
    page.type_into("browser-blocked-add", "Bank.Example");
    page.manager(|m, cx| m.browser_add_origin(false, cx));
    let blocked = page.setting().unwrap().blocked_origins;
    assert_eq!(blocked, ["https://bank.example"]);

    page.type_into("browser-blocked-add", "*.wildcard.example");
    page.manager(|m, cx| m.browser_add_origin(false, cx));
    assert!(page.ui(|u| u.error().is_some()));
    assert_eq!(page.setting().unwrap().blocked_origins.len(), 1);

    page.manager(|m, cx| m.browser_remove_origin(false, 0, cx));
    assert!(page.setting().unwrap().blocked_origins.is_empty());

    page.type_into("browser-allowed-add", "https://docs.example/some/page");
    page.manager(|m, cx| m.browser_add_origin(true, cx));
    assert_eq!(
        page.setting().unwrap().allowed_origins,
        ["https://docs.example"]
    );
    page.manager(|m, cx| m.browser_remove_origin(true, 0, cx));
    assert!(page.setting().unwrap().allowed_origins.is_empty());
}

#[gpui::test]
fn the_trust_selector_offers_only_read_only_and_act_for_me(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    // The old "ask before send, post, delete or pay" switch and its read-only note are gone.
    assert!(!page.on("mono-browser-confirm") && !page.on("mono-browser-full-access-note"));
    for trust in BrowserLevel::ALL {
        let selector = browser_ui::trust_selector(trust);
        assert!(page.on(selector), "{selector}");
    }
    // The "ask every action" and "autonomous" choices are gone.
    assert!(!page.on("mono-browser-trust-ask-risky") && !page.on("mono-browser-trust-autonomous"));
    // Each row has its own explanation, and one note says what is enforced.
    for selector in [
        "mono-browser-trust-read-only-text",
        "mono-browser-trust-act-text",
        "mono-browser-trust-note",
    ] {
        assert!(page.on(selector), "{selector}");
    }
    assert_eq!(page.setting().unwrap().trust, BrowserLevel::Act);

    for trust in BrowserLevel::ALL {
        // The rows are far below the fold of the test window, so the row's action is called
        // directly; the click calls the same method.
        page.manager(|m, cx| m.browser_set_trust(trust, cx));
        assert_eq!(page.setting().unwrap().trust, trust);
    }
}

#[gpui::test]
fn a_stored_legacy_level_reads_as_act_for_me_and_an_unknown_one_as_read_only(
    cx: &mut TestAppContext,
) {
    use super::browser_migration::{CHOICES, trust_label};
    use monocode_core::mono::browser::copy::Lang;
    assert_eq!(CHOICES, [BrowserLevel::ReadOnly, BrowserLevel::Act]);
    for (stored, shown) in [
        ("autonomous", BrowserLevel::Act),
        ("askRisky", BrowserLevel::Act),
        ("ask-every", BrowserLevel::Act),
        ("readOnly", BrowserLevel::ReadOnly),
        // An unknown value never reads as a level that acts.
        ("someFutureLevel", BrowserLevel::ReadOnly),
    ] {
        let loaded: MonoBrowser =
            serde_json::from_str(&format!(r#"{{"enabled":true,"trust":"{stored}"}}"#)).unwrap();
        assert_eq!(loaded.trust, shown, "{stored}");
    }
    assert_eq!(trust_label(BrowserLevel::Act, Lang::En), "Act for me");
    assert_eq!(trust_label(BrowserLevel::ReadOnly, Lang::En), "Read only");
    assert_ne!(
        trust_label(BrowserLevel::Act, Lang::Vi),
        trust_label(BrowserLevel::Act, Lang::En)
    );
    let mut page = open(cx);
    page.enable();
    assert!(page.on(browser_ui::trust_selector(BrowserLevel::Act)));
}

#[gpui::test]
fn changing_the_level_reconfigures_the_gate_and_keeps_it(cx: &mut TestAppContext) {
    use monocode_core::mono::browser::gate::Policy;
    let mut page = open(cx);
    page.enable();
    let before = page.setting().unwrap().server_key().1.to_owned();
    page.manager(|m, cx| m.browser_edit(cx, |b| b.trust = BrowserLevel::ReadOnly));
    let setting = page.setting().unwrap();
    assert_eq!(setting.trust, BrowserLevel::ReadOnly);
    assert_eq!(setting.server_key().1, before);
    // The level is not part of the gate's shape, so choosing one never drops the gate.
    let mut other = setting.clone();
    other.trust = BrowserLevel::Act;
    assert_eq!(setting.server_key(), other.server_key());
    assert_eq!(
        Policy::new(setting.trust, setting.blocked_origins.clone()).trust,
        BrowserLevel::ReadOnly
    );
}

#[gpui::test]
fn the_activity_log_lists_this_monos_browser_calls(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(page.on("mono-browser-audit-empty"));
    let entry = browser::audit_entry(
        "mcp__playwright_browser__browser_navigate",
        r#"{"url":"https://a.example/"}"#,
        page.s.session,
    )
    .unwrap();
    page.secrets().record(&page.s.id, &entry).unwrap();
    page.manager(|m, cx| m.browser_refresh(cx));
    assert!(page.on("mono-browser-audit-0") && !page.on("mono-browser-audit-empty"));
}

#[gpui::test]
fn deleting_a_mono_deletes_its_activity(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.choose("p-work-1");
    let entry = browser::audit_entry("mcp__playwright_browser__browser_click", "{}", 1).unwrap();
    page.secrets().record(&page.s.id, &entry).unwrap();
    let id = page.s.id.clone();
    page.manager(|m, cx| m.remove_mono(&id, cx));
    assert!(page.secrets().recent(&id, 10).is_empty());
}

#[gpui::test]
fn the_reason_a_launch_could_not_attach_the_browser_is_shown_on_the_tab(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    let id = page.s.id.clone();
    assert!(!page.on("mono-browser-attach-error"));
    browser::server::servers()
        .set_attach_error(&id, Some("the zptah Browser link is not running".into()));
    page.manager(|m, cx| m.browser_refresh(cx));
    assert!(page.on("mono-browser-attach-error"));
    browser::server::servers().set_attach_error(&id, None);
    page.manager(|m, cx| m.browser_refresh(cx));
    assert!(!page.on("mono-browser-attach-error"));
}
