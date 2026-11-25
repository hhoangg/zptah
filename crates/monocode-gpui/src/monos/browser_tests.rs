//! The Browser page of a Mono's settings: its states, the Chrome profile and the safety rules.
//! Nothing here starts Chrome or reaches a gate; `Fake` stands in for both.
use super::browser_ui::BrowserOps;
use super::workflows_ui::tests::click;
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::browser::recipe::manage::DryRunReport;
use monocode_core::mono::browser::{self, BrowserLevel, MonoBrowser, MonoBrowserStore, PrereqEnv};
use std::sync::{Arc, Mutex as StdMutex};

#[derive(Default)]
struct Fake {
    fda: StdMutex<u32>,
    /// What the next recipe dry run answers, and which ones were asked for.
    dry: StdMutex<Option<Result<DryRunReport, String>>>,
    dry_asked: StdMutex<Vec<(String, String)>>,
}
impl BrowserOps for Fake {
    fn open_full_disk_access(&self) {
        *self.fda.lock().unwrap() += 1;
    }
    fn dry_run_recipe(&self, mono_id: &str, recipe_id: &str) -> Result<DryRunReport, String> {
        self.dry_asked
            .lock()
            .unwrap()
            .push((mono_id.into(), recipe_id.into()));
        self.dry
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(Err("no gate".into()))
    }
}

const LOCAL_STATE: &str = r#"{"profile":{"info_cache":{
  "Default":{"name":"Personal","user_name":"me@example.com"},
  "Profile 6":{"name":"Work","user_name":"work@example.com","avatar_icon":"chrome://theme/IDR_PROFILE_AVATAR_26"}}}}"#;

/// A Chrome that exists, with its profile list readable.
fn chrome_env(dir: &std::path::Path, complete: bool) -> PrereqEnv {
    let chrome = dir.join("chrome-bin");
    let data = dir.join("Chrome");
    if complete {
        std::fs::write(&chrome, "").unwrap();
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("Local State"), LOCAL_STATE).unwrap();
    }
    PrereqEnv {
        chrome_binaries: vec![chrome],
        user_data_dir: data,
    }
}

struct Page {
    s: Setup,
    visual: VisualTestContext,
    ops: Arc<Fake>,
    env: PrereqEnv,
    _dir: TempDir,
}
/// A scratch folder that goes away with the test.
struct TempDir(PathBuf);
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn open_with(cx: &mut TestAppContext, complete: bool) -> Page {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    let dir = std::env::temp_dir().join(format!("native-browser-{}", mono::new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let dir = TempDir(dir);
    let env = chrome_env(&dir.0, complete);
    let ops = Arc::new(Fake::default());
    let (o, e) = (ops.clone(), env.clone());
    s.manager.update(cx, |m, _| m.browser_ui.use_fakes(o, e));
    let mut visual = VisualTestContext::from_window(s.window.into(), cx);
    click(&mut visual, "mono-settings-Browser");
    visual.refresh().unwrap();
    Page {
        s,
        visual,
        ops,
        env,
        _dir: dir,
    }
}
fn open(cx: &mut TestAppContext) -> Page {
    open_with(cx, true)
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
    fn click(&mut self, selector: &'static str) {
        click(&mut self.visual, selector);
        self.visual.refresh().unwrap();
    }
    fn manager<R>(&mut self, f: impl FnOnce(&mut Monos, &mut Context<Monos>) -> R) -> R {
        let r = self.s.manager.update(&mut self.visual, f);
        self.visual.run_until_parked();
        self.visual.refresh().unwrap();
        r
    }
    fn type_into(&mut self, key: &str, text: &str) {
        let field = self
            .s
            .manager
            .read_with(&self.visual, |m, _| m.fields.get(key).cloned())
            .unwrap_or_else(|| panic!("no field {key}"));
        field.update(&mut self.visual, |f, cx| f.set_text(text.to_owned(), cx));
        self.visual.run_until_parked();
        self.visual.refresh().unwrap();
    }
    fn secrets(&self) -> MonoBrowserStore {
        MonoBrowserStore::for_store(&locked(&self.s.store).unwrap()).unwrap()
    }
    fn enable(&mut self) {
        self.click("mono-browser-enabled");
    }
    fn choose(&mut self, dir: &str) {
        let dir = dir.to_owned();
        self.manager(|m, cx| m.browser_choose_profile(&dir, cx));
    }
    fn ui<R>(&self, f: impl FnOnce(&browser_ui::BrowserUi) -> R) -> R {
        self.s
            .manager
            .read_with(&self.visual, |m, _| f(&m.browser_ui))
    }
    /// Scrolls the page until `selector` is well inside it, then clicks it: the recipes
    /// sit below the safety rules, and a click only lands on what is visible.
    fn tap(&mut self, selector: &'static str) {
        use gpui::{ScrollDelta, ScrollWheelEvent, point, px};
        for _ in 0..60 {
            self.visual.refresh().unwrap();
            let (Some(target), Some(page)) = (
                self.visual.debug_bounds(selector),
                self.visual.debug_bounds("mono-browser-page"),
            ) else {
                break;
            };
            let y = target.center().y;
            if y > page.top() + px(120.) && y < page.bottom() - px(60.) {
                break;
            }
            let dy = if y > page.center().y { -240. } else { 240. };
            self.visual.simulate_event(ScrollWheelEvent {
                position: page.center(),
                delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                ..Default::default()
            });
            self.visual.run_until_parked();
        }
        self.click(selector);
    }
}

#[gpui::test]
fn the_browser_tab_starts_off_and_shows_only_the_switch(cx: &mut TestAppContext) {
    let mut page = open(cx);
    assert!(page.on("mono-browser-page"));
    assert!(page.on("mono-browser-enabled"));
    assert!(!page.on("mono-browser-prereq-chrome") && !page.on("mono-browser-profile"));
    assert!(
        page.setting().is_none(),
        "nothing is saved until the user turns it on"
    );
}

#[gpui::test]
fn turning_it_on_shows_the_chrome_check_and_an_empty_blocklist(cx: &mut TestAppContext) {
    let mut page = open_with(cx, false);
    page.enable();
    let setting = page.setting().unwrap();
    assert!(setting.enabled && !setting.ready);
    assert_eq!(
        setting.trust,
        BrowserLevel::Act,
        "new Monos act and ask before risky steps"
    );
    // New Monos block nothing: the sensitive-site rule asks on banking and payment sites.
    assert!(setting.blocked_origins.is_empty());
    for id in [
        "mono-browser-prereq-chrome",
        "mono-browser-prereq-fda",
        browser_ui::trust_selector(BrowserLevel::Act),
    ] {
        assert!(page.on(id), "{id}");
    }
    assert!(!page.on("mono-browser-blocked-0"));
    // No Node.js, token, Playwright or connection test: the extension needs none of them.
    for gone in [
        "mono-browser-prereq-node",
        "mono-browser-node-path-save",
        "mono-browser-token-save",
        "mono-browser-open-token-page",
        "mono-browser-test",
        "mono-browser-install",
        "mono-browser-use-extension",
        "mono-browser-blocked-reset",
    ] {
        assert!(!page.on(gone), "{gone}");
    }
    let prereq = page.ui(|u| u.prereq.clone()).unwrap();
    assert!(!prereq.all_ok() && !prereq.chrome.is_ok());
    // Turning it off again keeps the choices but removes the controls.
    page.enable();
    assert!(!page.setting().unwrap().enabled);
    assert!(!page.on("mono-browser-prereq-chrome"));
}

#[gpui::test]
fn a_refused_profile_read_offers_the_full_disk_access_fix(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(!page.on("mono-browser-fda-fix"), "readable: nothing to fix");
    page.manager(|m, cx| {
        let mut prereq = m.browser_ui.prereq.clone().unwrap();
        prereq.full_disk_access = browser::Check::Denied;
        m.browser_ui.prereq = Some(prereq);
        cx.notify();
    });
    page.click("mono-browser-fda-fix");
    assert_eq!(*page.ops.fda.lock().unwrap(), 1);
}

#[gpui::test]
fn the_picker_lists_chrome_profiles_and_changing_one_keeps_it_ready(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    let labels = page.ui(|u| u.profiles.iter().map(|p| p.label()).collect::<Vec<_>>());
    assert_eq!(
        labels,
        ["Personal (me@example.com)", "Work (work@example.com)"]
    );
    page.click("mono-browser-profile");
    assert!(page.ui(|u| u.select_is_open()));
    page.click("mono-browser-profile");
    assert!(
        !page.ui(|u| u.select_is_open()),
        "the trigger closes it again"
    );

    page.choose("Profile 6");
    let setting = page.setting().unwrap();
    assert_eq!(
        (setting.profile_dir.as_str(), setting.profile_name.as_str()),
        ("Profile 6", "Work")
    );
    assert!(page.setting().unwrap().ready, "Chrome and a profile");
    page.choose("Profile 6");
    assert!(page.setting().unwrap().ready, "same profile keeps it");
    page.choose("Default");
    assert_eq!(page.setting().unwrap().profile_dir, "Default");
    assert!(
        page.setting().unwrap().ready,
        "Chrome is there and so is a profile"
    );
}

#[gpui::test]
fn another_mono_on_the_same_profile_gets_a_warning(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.choose("Profile 6");
    assert!(!page.on("mono-browser-shared-warning"));
    page.manager(|m, cx| {
        let other = m.roster.create(Vec::new()).id.clone();
        let mono = m.roster.get_mut(&other).unwrap();
        mono.name = "Other".into();
        mono.browser = Some(MonoBrowser {
            enabled: true,
            profile_dir: "Profile 6".into(),
            ..Default::default()
        });
        cx.notify();
    });
    assert!(page.on("mono-browser-shared-warning"));
}

#[gpui::test]
fn the_browser_is_ready_only_with_chrome_and_a_profile(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(!page.setting().unwrap().ready, "no profile yet");
    page.choose("Profile 6");
    let setting = page.setting().unwrap();
    assert!(setting.ready && setting.active());
    // The Mono's rules now carry the browser paragraph for the chosen profile.
    let rules = page.s.manager.read_with(&page.visual, |m, _| {
        monocode_core::mono::instructions(
            m.roster.get(&page.s.id).unwrap(),
            std::path::Path::new("/bin/app"),
        )
    });
    assert!(rules.contains("## Browser") && rules.contains("profile \"Work\""));
    assert!(!rules.to_lowercase().contains("playwright"));
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

/// A migrated Mono: the notice, the offer and a profile with no connection.
fn migrated() -> MonoBrowser {
    MonoBrowser {
        enabled: true,
        profile_dir: "Default".into(),
        migration_notice: true,
        suggest_new_defaults: true,
        migrated: 1,
        ..MonoBrowser::default()
    }
}

#[gpui::test]
fn the_migration_notice_shows_until_dismissed_and_the_dismissal_is_saved(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    assert!(
        !page.on("mono-browser-notice-dismiss"),
        "a new Mono has no notice"
    );
    page.manager(|m, cx| m.browser_edit(cx, |b| *b = migrated()));
    for id in [
        "mono-browser-notice-dismiss",
        "mono-browser-notice-trust",
        "mono-browser-notice-tab-group",
    ] {
        assert!(page.on(id), "{id}");
    }
    page.click("mono-browser-notice-dismiss");
    assert!(!page.setting().unwrap().migration_notice);
    assert!(!page.on("mono-browser-notice-dismiss"));
    // It is on disk: a fresh load of the roster does not bring it back.
    for _ in 0..200 {
        page.visual.run_until_parked();
        let saved = mono::Roster::load(&locked(&page.s.store).unwrap()).unwrap();
        if saved
            .get(&page.s.id)
            .and_then(|m| m.browser.as_ref())
            .is_some_and(|b| !b.migration_notice)
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("the dismissal was not saved");
}

#[gpui::test]
fn switch_and_keep_resolve_the_blocked_list_offer_once(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    let mut custom = migrated();
    custom.blocked_origins.push("https://bank.example".into());
    page.manager(|m, cx| m.browser_edit(cx, |b| *b = custom.clone()));
    assert!(page.on("mono-browser-defaults-switch") && page.on("mono-browser-defaults-keep"));
    page.manager(|m, cx| m.browser_switch_defaults(cx));
    let after = page.setting().unwrap();
    assert!(!after.suggest_new_defaults);
    assert!(
        after
            .blocked_origins
            .iter()
            .any(|o| o == "https://bank.example")
    );
    assert!(
        !after
            .blocked_origins
            .iter()
            .any(|o| o == "https://www.paypal.com")
    );
    assert!(!page.on("mono-browser-defaults-switch"));

    page.manager(|m, cx| m.browser_edit(cx, |b| *b = custom.clone()));
    page.manager(|m, cx| m.browser_keep_list(cx));
    let kept = page.setting().unwrap();
    assert!(!kept.suggest_new_defaults);
    assert_eq!(kept.blocked_origins, custom.blocked_origins);
    assert!(!page.on("mono-browser-defaults-keep"));
}

#[gpui::test]
fn a_migrated_mono_without_a_connection_shows_reconnect_needed(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.manager(|m, cx| {
        m.browser_edit(cx, |b| {
            *b = migrated();
            b.migrated = 0;
        })
    });
    assert!(!page.on("mono-browser-reconnect"), "never migrated");
    page.manager(|m, cx| m.browser_edit(cx, |b| b.migrated = 1));
    assert!(page.setting().unwrap().needs_reconnect());
    assert!(page.on("mono-browser-reconnect"));
    page.manager(|m, cx| m.browser_edit(cx, |b| b.profile_dir.clear()));
    assert!(!page.on("mono-browser-reconnect"), "no saved profile");
    page.manager(|m, cx| {
        m.browser_edit(cx, |b| {
            b.profile_dir = "Default".into();
            b.connection = Some("link-1".into())
        })
    });
    assert!(!page.on("mono-browser-reconnect"));
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
fn showing_agent_actions_defaults_on_and_the_switch_reaches_the_roster(cx: &mut TestAppContext) {
    use monocode_core::mono::browser::roster::entry_for;
    let mut page = open(cx);
    page.enable();
    page.choose("Profile 6");
    assert!(page.on("mono-browser-show-actions"));
    let entry = |page: &mut Page| {
        page.s.manager.read_with(&page.visual, |m, _| {
            entry_for(m.roster.get(&page.s.id).unwrap()).map(|(_, entry)| entry.cursor)
        })
    };
    let setting = page.setting().unwrap();
    assert!(setting.show_actions, "the cursor is on by default");
    assert_eq!(entry(&mut page), Some(true));
    page.manager(|m, cx| m.browser_toggle_show_actions(cx));
    let setting = page.setting().unwrap();
    assert!(!setting.show_actions);
    assert_eq!(entry(&mut page), Some(false));
    // The switch is not part of the gate's shape: flipping it never drops the gate.
    let mut other = setting.clone();
    other.show_actions = true;
    assert_eq!(setting.server_key(), other.server_key());
    page.manager(|m, cx| m.browser_toggle_show_actions(cx));
    assert!(page.setting().unwrap().show_actions);
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
    page.choose("Profile 6");
    let entry = browser::audit_entry("mcp__playwright_browser__browser_click", "{}", 1).unwrap();
    page.secrets().record(&page.s.id, &entry).unwrap();
    let id = page.s.id.clone();
    page.manager(|m, cx| m.remove_mono(&id, cx));
    assert!(page.secrets().recent(&id, 10).is_empty());
}

#[test]
fn the_full_disk_access_link_uses_the_dotted_system_settings_scheme() {
    assert_eq!(
        browser_ui::FULL_DISK_ACCESS_URL,
        "x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles"
    );
    assert!(!browser_ui::FULL_DISK_ACCESS_URL.starts_with("x-apple-"));
}

#[gpui::test]
fn the_picker_changes_the_profile_when_profiles_are_listed(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.choose("Default");
    assert_eq!(page.setting().unwrap().profile_dir, "Default");
    page.click("mono-browser-profile");
    assert!(page.ui(|u| u.select_is_open()));
    page.choose("Profile 6");
    let setting = page.setting().unwrap();
    assert_eq!(
        (setting.profile_dir.as_str(), setting.profile_name.as_str()),
        ("Profile 6", "Work")
    );
    assert!(!page.ui(|u| u.profiles_unreadable) && !page.on("mono-browser-profiles-note"));
}

#[gpui::test]
fn without_chromes_profile_list_the_saved_profile_still_shows_and_works(cx: &mut TestAppContext) {
    let mut page = open(cx);
    page.enable();
    page.choose("Profile 6");
    assert!(page.setting().unwrap().ready);
    // Full Disk Access is gone: Chrome's Local State cannot be read.
    std::fs::remove_file(page.env.user_data_dir.join("Local State")).unwrap();
    page.manager(|m, cx| m.browser_refresh(cx));
    assert!(page.ui(|u| u.profiles_unreadable));
    assert_eq!(
        page.ui(|u| u.profiles.iter().map(|p| p.dir.clone()).collect::<Vec<_>>()),
        ["Profile 6"]
    );
    assert!(page.on("mono-browser-profiles-note"));
    // The saved profile is not lost and the browser is not marked unready by it.
    let setting = page.setting().unwrap();
    assert_eq!(setting.profile_dir, "Profile 6");
    assert!(setting.ready && setting.active());
    // The picker opens, and a choice (a valid directory name) always takes effect.
    page.click("mono-browser-profile");
    assert!(page.ui(|u| u.select_is_open()));
    page.choose("Default");
    assert_eq!(page.setting().unwrap().profile_dir, "Default");
    page.choose("../etc");
    assert_eq!(page.setting().unwrap().profile_dir, "Default");
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

// ---- Recipes (Phase 4) ----

mod recipes {
    use super::*;
    use crate::monos::recipes_ui::export_file_name;
    use monocode_core::mono::browser::recipe::{
        manage::{self, Status},
        model::Recipe,
        share::{self, Library},
        store::RecipeStore,
    };
    use serde_json::json;

    const ORIGIN: &str = "https://news.example";

    fn recipe() -> Recipe {
        let recipe: Recipe = serde_json::from_value(json!({
            "schema": 1, "id": "rcp_0a1b2c3d", "origin": ORIGIN, "intent": "news.archive",
            "description": "Read stories", "version": 1,
            "params": [{"name": "n", "type": "integer", "description": "how many", "default": 2}],
            "steps": [
                {"op": "open", "url": "https://news.example/archive",
                 "expect": [{"url_matches": "^https://news\\.example/archive"}]},
                {"op": "extract", "scope": {"candidates": [{"role": "list", "name": "Stories"}]},
                 "each": {"role": "listitem"}, "take_first": "{n}",
                 "fields": {"title": {"role": "link", "from": "name"}},
                 "max_chars_per_field": 100, "expect": [{"count_at_least": 1}]},
            ],
            "risk": "read", "playwright_mcp": "0.0.86",
        }))
        .unwrap();
        recipe.validate().unwrap();
        recipe
    }
    fn dry_ok() -> DryRunReport {
        DryRunReport {
            ok: true,
            steps_run: 2,
            stopped_before: None,
            failure: None,
        }
    }

    impl Page {
        fn recipe_store(&self) -> RecipeStore {
            RecipeStore::new(browser::recipes_dir(&self.s.root, &self.s.id))
        }
        fn library(&self) -> Library {
            Library::new(browser::shared_recipes_dir(&self.s.root))
        }
        /// A recipe the user approved earlier, on a Mono that has the browser on.
        fn with_recipe(&mut self, recipe: &Recipe) {
            self.enable();
            self.recipe_store().save(recipe).unwrap();
            self.secrets()
                .approve_recipe(&self.s.id, &recipe.id, &recipe.content_hash())
                .unwrap();
            self.manager(|m, cx| m.browser_refresh(cx));
        }
        fn rows(&self) -> Vec<manage::Row> {
            self.ui(|u| u.recipes.rows.clone())
        }
        fn status(&self) -> Status {
            self.rows()[0].status
        }
        fn notice(&self) -> Option<String> {
            self.ui(|u| u.recipes.notice.clone())
        }
        fn problem(&self) -> Option<String> {
            self.ui(|u| u.recipes.error.clone())
        }
        fn dry_run_answers(&self, answer: Result<DryRunReport, String>) {
            *self.ops.dry.lock().unwrap() = Some(answer);
        }
        fn import(&mut self, text: &str) {
            let text = text.to_owned();
            self.manager(|m, cx| m.recipes_import_text(&text, cx));
        }
    }

    fn exported_text() -> String {
        share::export(&recipe()).unwrap()
    }

    #[gpui::test]
    fn the_list_shows_title_origin_version_last_run_success_rate_and_the_flaky_flag(
        cx: &mut TestAppContext,
    ) {
        let mut page = open(cx);
        page.with_recipe(&recipe());
        page.recipe_store().update_stats(&recipe(), |s| {
            s.runs = 10;
            s.ok = 8;
            s.last_run_ms = monocode_core::time::now_ms() - 2 * 3_600_000;
            s.flaky = true;
        });
        page.manager(|m, cx| m.browser_refresh(cx));
        let row = page.rows().remove(0);
        assert_eq!(
            (row.intent.as_str(), row.origin.as_str(), row.version),
            ("news.archive", ORIGIN, 1)
        );
        assert_eq!(row.success_percent(), Some(80));
        assert!(row.flaky && row.status == Status::Approved);
        for id in [
            "mono-recipe-0",
            "mono-recipe-title-0",
            "mono-recipe-status-0",
            "mono-recipe-meta-0",
            "mono-recipe-flaky-0",
            "mono-recipe-view-0",
            "mono-recipe-dryrun-0",
            "mono-recipe-share-0",
            "mono-recipe-export-0",
            "mono-recipe-delete-0",
            "mono-recipes-import",
        ] {
            assert!(page.on(id), "{id}");
        }
        assert!(!page.on("mono-recipe-rollback-0"), "no earlier version");
        assert!(!page.on("mono-recipes-empty"));
    }

    #[gpui::test]
    fn an_empty_list_says_so_and_the_section_follows_the_browser_switch(cx: &mut TestAppContext) {
        let mut page = open(cx);
        assert!(!page.on("mono-recipes-import"), "off: no recipes section");
        page.enable();
        assert!(page.on("mono-recipes-empty") && page.on("mono-recipes-import"));
        assert!(page.on("mono-recipes-shared-empty"));
    }

    #[gpui::test]
    fn the_steps_are_shown_in_words_with_placeholders_and_no_value(cx: &mut TestAppContext) {
        let mut page = open(cx);
        page.with_recipe(&recipe());
        assert!(!page.on("mono-recipe-steps-0"));
        page.tap("mono-recipe-view-0");
        assert!(page.on("mono-recipe-steps-0"));
        let steps = page.ui(|u| u.recipes.open.clone()).unwrap().steps;
        assert!(
            steps.contains("{n}") && steps.contains("Risk: read"),
            "{steps}"
        );
        assert!(!page.on("mono-recipe-approve-0"), "already approved");
        page.tap("mono-recipe-view-0");
        assert!(!page.on("mono-recipe-steps-0"));
    }

    #[gpui::test]
    fn an_imported_recipe_is_a_draft_that_needs_the_users_approval_and_then_a_dry_run(
        cx: &mut TestAppContext,
    ) {
        let mut page = open(cx);
        page.enable();
        page.import(&exported_text());
        assert_eq!(page.status(), Status::Draft);
        assert_eq!(page.rows()[0].outside.as_deref(), Some("import"));
        // Its steps are open for review, with the approval button; no dry run until approved.
        assert!(page.on("mono-recipe-steps-0") && page.on("mono-recipe-approve-0"));
        assert!(!page.on("mono-recipe-dryrun-0"));
        assert!(!page.on("mono-recipe-share-0"));
        assert!(page.notice().unwrap().contains("approve"));
        page.tap("mono-recipe-approve-0");
        assert_eq!(page.status(), Status::NeedsDryRun);
        assert!(page.notice().unwrap().contains("dry run"));
        assert!(page.on("mono-recipe-dryrun-0"));
        assert!(
            !page.on("mono-recipe-share-0"),
            "not shareable before it passed"
        );
        // A failing dry run keeps it locked and says why; an error from the gate too.
        page.dry_run_answers(Ok(DryRunReport {
            ok: false,
            steps_run: 1,
            stopped_before: None,
            failure: Some("Step 2 (extract): No locator candidate matched.".into()),
        }));
        page.tap("mono-recipe-dryrun-0");
        assert!(page.problem().unwrap().contains("No locator"));
        assert_eq!(page.status(), Status::NeedsDryRun);
        page.dry_run_answers(Err("This Mono's browser is not running yet.".into()));
        page.tap("mono-recipe-dryrun-0");
        assert!(page.problem().unwrap().contains("not running"));
        assert_eq!(page.status(), Status::NeedsDryRun);
        // The gate does the dry run; the page only shows its answer. Here the fake gate also
        // records the pass the way the real one does.
        let (id, mono) = (page.rows()[0].id.clone(), page.s.id.clone());
        let hash = page.rows()[0].hash.clone();
        page.dry_run_answers(Ok(dry_ok()));
        page.tap("mono-recipe-dryrun-0");
        assert_eq!(
            page.ops.dry_asked.lock().unwrap().last().cloned(),
            Some((mono.clone(), id.clone()))
        );
        assert!(page.notice().unwrap().contains("passed"));
        // (The fake did not write the dry run row, so the real status is still locked.)
        assert_eq!(page.status(), Status::NeedsDryRun);
        page.secrets().mark_dry_run(&mono, &id, &hash).unwrap();
        page.manager(|m, cx| m.browser_refresh(cx));
        assert_eq!(page.status(), Status::Approved);
        assert!(page.on("mono-recipe-share-0"));
    }

    #[gpui::test]
    fn import_refuses_a_blocked_site_a_tampered_file_and_says_why(cx: &mut TestAppContext) {
        let mut page = open(cx);
        page.enable();
        page.manager(|m, cx| m.browser_edit(cx, |b| b.blocked_origins.push(ORIGIN.into())));
        page.import(&exported_text());
        assert!(page.problem().unwrap().contains("blocked"));
        assert!(page.on("mono-recipes-error"));
        assert!(page.rows().is_empty());
        page.manager(|m, cx| m.browser_edit(cx, |b| b.blocked_origins.clear()));
        let tampered = exported_text().replace("/archive", "/evil");
        page.import(&tampered);
        assert!(page.problem().unwrap().contains("hash"));
        assert!(page.rows().is_empty());
        page.import("not a recipe");
        assert!(page.problem().is_some() && page.rows().is_empty());
        assert!(!page.recipe_store().dir().join("anything").exists());
    }

    #[gpui::test]
    fn a_picked_file_is_imported_and_a_big_one_is_refused_unread(cx: &mut TestAppContext) {
        let mut page = open(cx);
        page.enable();
        let file = std::env::temp_dir().join(format!("recipe-pick-{}.json", mono::new_id()));
        std::fs::write(&file, exported_text()).unwrap();
        page.tap("mono-recipes-import");
        assert!(page.visual.did_prompt_for_paths());
        let picked = file.clone();
        page.visual
            .simulate_path_prompt_response(move |_| Some(vec![picked.clone()]));
        page.visual.run_until_parked();
        page.visual.refresh().unwrap();
        assert_eq!(page.status(), Status::Draft);
        std::fs::write(&file, "x".repeat(share::MAX_FILE_BYTES + 1)).unwrap();
        page.tap("mono-recipes-import");
        let picked = file.clone();
        page.visual
            .simulate_path_prompt_response(move |_| Some(vec![picked.clone()]));
        page.visual.run_until_parked();
        assert!(page.problem().unwrap().contains("too big"));
        assert_eq!(page.rows().len(), 1);
        let _ = std::fs::remove_file(file);
    }

    #[gpui::test]
    fn approve_is_bound_to_the_steps_that_were_shown(cx: &mut TestAppContext) {
        let mut page = open(cx);
        page.enable();
        page.import(&exported_text());
        let id = page.rows()[0].id.clone();
        // The file is replaced after its steps were shown and before the click.
        let mut changed = page.recipe_store().get(&id).unwrap();
        changed.description = "Something else entirely".into();
        page.recipe_store().save(&changed).unwrap();
        page.tap("mono-recipe-approve-0");
        assert!(page.problem().unwrap().contains("changed"));
        assert_eq!(page.status(), Status::Draft);
        assert!(page.secrets().approved_hash(&page.s.id, &id).is_none());
    }

    #[gpui::test]
    fn export_saves_the_recipe_file_where_the_user_picks(cx: &mut TestAppContext) {
        let mut page = open(cx);
        page.with_recipe(&recipe());
        assert_eq!(
            export_file_name("news.archive"),
            "news.archive.monoptah-recipe.json"
        );
        page.tap("mono-recipe-export-0");
        assert!(page.visual.did_prompt_for_new_path());
        let target = std::env::temp_dir().join(format!("recipe-export-{}.json", mono::new_id()));
        let file = target.clone();
        page.visual.simulate_new_path_selection(move |_| Some(file));
        page.visual.run_until_parked();
        let text = std::fs::read_to_string(&target).unwrap();
        assert!(text.contains("\"hash\": \"sha256:"));
        assert!(share::parse(&text, &[]).is_ok());
        assert!(page.notice().unwrap().contains("Exported"));
        let _ = std::fs::remove_file(target);
    }

    #[gpui::test]
    fn share_adopt_and_unshare_go_through_the_library_and_a_copy_starts_unapproved(
        cx: &mut TestAppContext,
    ) {
        let mut page = open(cx);
        page.with_recipe(&recipe());
        page.tap("mono-recipe-share-0");
        assert_eq!(page.library().list().len(), 1);
        assert!(page.on("mono-recipe-shared-0") && page.on("mono-recipe-unshare-0"));
        assert!(!page.on("mono-recipe-share-0"));
        // This Mono has it already: no copy button. After deleting it there is one.
        assert!(!page.on("mono-recipe-adopt-0"));
        page.tap("mono-recipe-delete-0");
        assert!(page.on("mono-recipe-delete-0"), "the first click only asks");
        assert_eq!(page.rows().len(), 1);
        page.tap("mono-recipe-delete-0");
        assert!(page.rows().is_empty());
        assert!(page.on("mono-recipe-adopt-0"));
        page.tap("mono-recipe-adopt-0");
        let row = page.rows().remove(0);
        assert_eq!(row.status, Status::Draft);
        assert_eq!(row.outside.as_deref(), Some("shared"));
        assert!(
            page.on("mono-recipe-approve-0"),
            "its steps open for review"
        );
        // Unshare takes it out of the library and leaves the copy.
        page.tap("mono-recipe-shared-remove-0");
        assert!(page.library().list().is_empty());
        assert_eq!(page.rows().len(), 1);
        assert!(page.notice().unwrap().contains("Unshared"));
    }

    #[gpui::test]
    fn a_blocked_site_stops_a_copy_from_the_library(cx: &mut TestAppContext) {
        let mut page = open(cx);
        page.with_recipe(&recipe());
        page.tap("mono-recipe-share-0");
        page.tap("mono-recipe-delete-0");
        page.tap("mono-recipe-delete-0");
        page.manager(|m, cx| m.browser_edit(cx, |b| b.blocked_origins.push(ORIGIN.into())));
        page.tap("mono-recipe-adopt-0");
        assert!(page.problem().unwrap().contains("blocked"));
        assert!(page.rows().is_empty());
    }

    #[gpui::test]
    fn rollback_goes_to_the_previous_version_and_delete_asks_twice(cx: &mut TestAppContext) {
        let mut page = open(cx);
        let v1 = recipe();
        page.with_recipe(&v1);
        let mut v2 = v1.clone();
        v2.version = 2;
        v2.description = "Read stories, second version".into();
        page.recipe_store().save(&v2).unwrap();
        page.manager(|m, cx| m.browser_refresh(cx));
        assert_eq!(page.rows()[0].previous, Some(1));
        assert_eq!(page.status(), Status::Draft, "v2 was never approved");
        page.tap("mono-recipe-rollback-0");
        assert_eq!(page.rows()[0].version, 1);
        assert_eq!(page.status(), Status::Approved, "v1 was approved before");
        assert!(page.notice().unwrap().contains("version 1"));
        page.tap("mono-recipe-delete-0");
        page.tap("mono-recipe-view-0");
        page.tap("mono-recipe-delete-0");
        assert_eq!(page.rows().len(), 1, "another click in between asks again");
        page.tap("mono-recipe-delete-0");
        assert!(page.rows().is_empty());
        assert!(page.secrets().approved_hash(&page.s.id, &v1.id).is_none());
    }
}
