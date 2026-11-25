use super::*;
use crate::mono::browser::{
    BrowserTrust, MonoBrowser, MonoBrowserStore, DEFAULT_BLOCKED_ORIGINS, SECRET_NAME,
};
use crate::mono::Roster;

// Obviously fake: not a real extension token.
const TOKEN: &str = "fake-extension-token-0123456789-ABCD";
const ON: Options = Options { delete_token: true };
const OFF: Options = Options {
    delete_token: false,
};

struct Fixture {
    _root: tempfile::TempDir,
    store: Store,
}
fn fixture(roster: Option<&str>) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(&root.path().join("monocode.db")).unwrap();
    if let Some(text) = roster {
        store.save_setting("mono-roster", text).unwrap();
    }
    Fixture { _root: root, store }
}
fn roster_of(browsers: &[(&str, &str)]) -> String {
    let monos: Vec<String> = browsers
        .iter()
        .map(|(id, browser)| format!(r#"{{"id":"{id}","browser":{browser}}}"#))
        .collect();
    format!(r#"{{"monos":[{}],"hiddenSessions":[7]}}"#, monos.join(","))
}
fn stored(f: &Fixture) -> String {
    f.store.setting("mono-roster").unwrap().unwrap()
}
fn browser_of(f: &Fixture, id: &str) -> MonoBrowser {
    Roster::load(&f.store)
        .unwrap()
        .get(id)
        .unwrap()
        .browser
        .clone()
        .unwrap()
}
fn migrate(f: &Fixture, options: &Options) -> Option<String> {
    run(&f.store, f.store.setting("mono-roster").unwrap(), options)
}
fn value(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn trust_levels_become_act_and_read_only_stays() {
    let old = roster_of(&[
        ("a", r#"{"enabled":true,"trust":"askRisky"}"#),
        ("b", r#"{"enabled":true,"trust":"autonomous"}"#),
        ("c", r#"{"enabled":true,"trust":"readOnly"}"#),
        ("d", r#"{"enabled":true,"trust":"ask-every"}"#),
        ("e", r#"{"enabled":true,"trust":"ask-risky"}"#),
        ("f", r#"{"enabled":true,"trust":"read-only"}"#),
        ("g", r#"{"enabled":true,"trust":"futureLevel"}"#),
        ("i", r#"{"enabled":true,"trust":5}"#),
        ("h", r#"{"enabled":true}"#),
    ]);
    let f = fixture(Some(&old));
    migrate(&f, &OFF);
    for id in ["a", "b", "d", "e", "h"] {
        let level: crate::mono::browser::trust::BrowserLevel = browser_of(&f, id).trust.into();
        assert_eq!(
            level,
            crate::mono::browser::trust::BrowserLevel::Act,
            "{id}"
        );
    }
    for id in ["c", "f", "g", "i"] {
        assert_eq!(browser_of(&f, id).trust, BrowserTrust::ReadOnly, "{id}");
    }
}

#[test]
fn each_other_field_is_kept_or_set_as_the_spec_says() {
    let old = roster_of(&[(
        "a",
        r#"{"enabled":true,"profileDir":"Profile 6","profileName":"Work",
            "blockedOrigins":["https://bank.example","https://paypal.com"],
            "allowedOrigins":["https://docs.example"],"showActions":false,
            "useExtension":false,"ready":true}"#,
    )]);
    let f = fixture(Some(&old));
    migrate(&f, &OFF);
    let b = browser_of(&f, "a");
    assert!(b.enabled);
    assert_eq!(
        (b.profile_dir.as_str(), b.profile_name.as_str()),
        ("Profile 6", "Work")
    );
    assert_eq!(
        b.blocked_origins,
        ["https://bank.example", "https://paypal.com"]
    );
    assert_eq!(b.allowed_origins, ["https://docs.example"]);
    assert!(!b.show_actions, "the cursor switch keeps its value");
    assert!(!b.use_extension, "Playwright is still the backend");
    assert!(!b.ready);
    assert_eq!(b.connection, None);
    assert!(b.needs_reconnect());
    assert!(b.migration_notice);
    assert!(b.suggest_new_defaults, "paypal.com is an old default");
    // Fields the migration does not know are not lost.
    assert_eq!(value(&stored(&f))["hiddenSessions"], json!([7]));
}

#[test]
fn a_roster_without_a_list_keeps_the_old_defaults_and_a_custom_list_gets_no_suggestion() {
    let old = roster_of(&[
        ("a", r#"{"enabled":true,"profileDir":"Default"}"#),
        (
            "b",
            r#"{"enabled":true,"blockedOrigins":["https://mine.example"]}"#,
        ),
        ("c", r#"{"enabled":true,"blockedOrigins":[]}"#),
    ]);
    let f = fixture(Some(&old));
    migrate(&f, &OFF);
    let a = browser_of(&f, "a");
    assert_eq!(a.blocked_origins.len(), DEFAULT_BLOCKED_ORIGINS.len());
    assert!(a.suggest_new_defaults);
    let b = browser_of(&f, "b");
    assert_eq!(b.blocked_origins, ["https://mine.example"]);
    assert!(!b.suggest_new_defaults);
    let c = browser_of(&f, "c");
    assert!(c.blocked_origins.is_empty() && !c.suggest_new_defaults);
}

#[test]
fn switching_to_the_new_defaults_removes_only_old_default_entries() {
    let mut b = MonoBrowser {
        suggest_new_defaults: true,
        ..Default::default()
    };
    b.blocked_origins.push("https://mine.example".into());
    let mut kept = b.clone();
    kept.keep_blocked_list();
    assert!(!kept.suggest_new_defaults);
    assert_eq!(
        kept.blocked_origins.len(),
        DEFAULT_BLOCKED_ORIGINS.len() + 1
    );
    b.switch_to_new_defaults();
    assert_eq!(b.blocked_origins, ["https://mine.example"]);
    assert!(!b.suggest_new_defaults);
}

#[test]
fn a_mono_without_a_configured_browser_gets_no_notice() {
    let old = roster_of(&[("a", r#"{}"#), ("b", r#"{"enabled":false}"#)]);
    let f = fixture(Some(&old));
    migrate(&f, &OFF);
    assert!(!browser_of(&f, "a").migration_notice);
    assert!(!browser_of(&f, "b").migration_notice);
}

#[test]
fn it_backs_up_the_old_row_first_and_runs_once() {
    let old = roster_of(&[("a", r#"{"enabled":true,"trust":"autonomous"}"#)]);
    let f = fixture(Some(&old));
    let first = migrate(&f, &OFF).unwrap();
    assert_ne!(first, old);
    assert_eq!(
        f.store.setting(BACKUP_KEY).unwrap().as_deref(),
        Some(old.as_str())
    );
    assert_eq!(f.store.setting(VERSION_KEY).unwrap().as_deref(), Some("1"));
    assert_eq!(stored(&f), first);

    // Second run: nothing changes, the backup still holds the original, a dismissed notice stays.
    let mut roster = Roster::load(&f.store).unwrap();
    roster
        .get_mut("a")
        .unwrap()
        .browser
        .as_mut()
        .unwrap()
        .migration_notice = false;
    roster.save(&f.store).unwrap();
    let saved = stored(&f);
    assert_eq!(migrate(&f, &OFF).as_deref(), Some(saved.as_str()));
    assert_eq!(stored(&f), saved);
    assert_eq!(
        f.store.setting(BACKUP_KEY).unwrap().as_deref(),
        Some(old.as_str())
    );
    assert!(!browser_of(&f, "a").migration_notice);
}

#[test]
fn the_transform_itself_is_idempotent() {
    let mut once = value(&roster_of(&[(
        "a",
        r#"{"enabled":true,"trust":"autonomous","profileDir":"Default"}"#,
    )]));
    migrate_roster(&mut once).unwrap();
    let mut twice = once.clone();
    migrate_roster(&mut twice).unwrap();
    assert_eq!(once, twice);
}

#[test]
fn an_existing_backup_is_never_replaced() {
    let old = roster_of(&[("a", r#"{"enabled":true}"#)]);
    let f = fixture(Some(&old));
    f.store
        .save_setting(BACKUP_KEY, "the first backup")
        .unwrap();
    migrate(&f, &OFF);
    assert_eq!(
        f.store.setting(BACKUP_KEY).unwrap().as_deref(),
        Some("the first backup")
    );
}

#[test]
fn the_token_is_deleted_only_when_asked_and_only_once() {
    let old = roster_of(&[("a", r#"{"enabled":true}"#), ("b", r#"{}"#)]);
    let f = fixture(Some(&old));
    let secrets = MonoBrowserStore::for_store(&f.store).unwrap();
    secrets.set_secret("gone", SECRET_NAME, TOKEN).unwrap();
    secrets.set_secret("a", SECRET_NAME, TOKEN).unwrap();
    secrets.set_secret("b", SECRET_NAME, TOKEN).unwrap();
    secrets.set_secret("a", "other-secret", TOKEN).unwrap();

    // Playwright still in use: the token stays.
    migrate(&f, &OFF);
    assert!(secrets.has_row("a", SECRET_NAME) && secrets.has_row("b", SECRET_NAME));

    // Later release: the token goes, other secrets stay, and it is recorded.
    migrate(&f, &ON);
    assert!(!secrets.has_row("a", SECRET_NAME) && !secrets.has_row("b", SECRET_NAME));
    assert!(
        !secrets.has_row("gone", SECRET_NAME),
        "also a Mono no longer in the roster"
    );
    assert!(secrets.has_row("a", "other-secret"));
    assert!(f.store.setting(TOKEN_CLEANUP_KEY).unwrap().is_some());
    migrate(&f, &ON);
}

#[test]
fn corrupt_or_partial_input_fails_safe_and_leaves_the_old_row_untouched() {
    for bad in [
        "not json at all",
        "[]",
        r#"{"monos":"x"}"#,
        r#"{"monos":[1]}"#,
        r#"{"monos":[{"id":"a","browser":"on"}]}"#,
        r#"{"monos":[{"id":"a","browser":{"blockedOrigins":"https://x.example"}}]}"#,
        r#"{"monos":[{"id":"a","browser":{"blockedOrigins":[1]}}]}"#,
        // One good Mono next to a bad one: nothing is half migrated.
        r#"{"monos":[{"id":"a","browser":{"enabled":true,"trust":"autonomous"}},{"id":"b","browser":5}]}"#,
    ] {
        let f = fixture(Some(bad));
        assert_eq!(migrate(&f, &ON).as_deref(), Some(bad), "{bad}");
        assert_eq!(stored(&f), bad);
        assert!(f.store.setting(BACKUP_KEY).unwrap().is_none(), "{bad}");
        assert!(f.store.setting(VERSION_KEY).unwrap().is_none(), "{bad}");
    }
}

#[test]
fn a_fresh_install_is_marked_and_later_monos_are_not_migrated() {
    let f = fixture(None);
    assert_eq!(migrate(&f, &OFF), None);
    assert_eq!(f.store.setting(VERSION_KEY).unwrap().as_deref(), Some("1"));
    let mut roster = Roster::default();
    roster.create(vec![]);
    roster.monos[0].browser = Some(MonoBrowser {
        enabled: true,
        ..Default::default()
    });
    roster.save(&f.store).unwrap();
    let id = roster.monos[0].id.clone();
    let b = browser_of(&f, &id);
    assert!(!b.migration_notice && !b.suggest_new_defaults);
}

#[test]
fn the_new_fields_round_trip_and_old_json_without_them_loads() {
    let b = MonoBrowser {
        connection: Some("conn-1".into()),
        migration_notice: true,
        suggest_new_defaults: true,
        ..Default::default()
    };
    let back: MonoBrowser = serde_json::from_str(&serde_json::to_string(&b).unwrap()).unwrap();
    assert_eq!(back, b);
    let plain = serde_json::to_string(&MonoBrowser::default()).unwrap();
    assert!(!plain.contains("connection") && !plain.contains("migrationNotice"));
}

#[test]
fn level_ids_accept_the_forms_the_migration_stores() {
    use crate::mono::browser::trust::BrowserLevel;
    assert_eq!(
        BrowserLevel::from_id("readOnly"),
        Some(BrowserLevel::ReadOnly)
    );
    assert_eq!(BrowserLevel::from_id("askRisky"), Some(BrowserLevel::Act));
    assert_eq!(BrowserLevel::from_id("nonsense"), None);
    for text in [r#""readOnly""#, r#""read-only""#] {
        assert_eq!(
            serde_json::from_str::<BrowserTrust>(text).unwrap(),
            BrowserTrust::ReadOnly
        );
    }
}

#[test]
fn a_row_already_carrying_the_version_is_not_migrated_again() {
    let mut once = value(&roster_of(&[(
        "a",
        r#"{"enabled":true,"trust":"autonomous"}"#,
    )]));
    migrate_roster(&mut once).unwrap();
    // The user dismissed both flags (they are no longer written), then the marker write failed.
    let browser = once["monos"][0]["browser"].as_object_mut().unwrap();
    browser.remove("migrationNotice");
    browser.remove("suggestNewDefaults");
    let dismissed = once.clone();
    migrate_roster(&mut once).unwrap();
    assert_eq!(once, dismissed);
}

#[test]
fn switching_matches_origins_the_way_the_suggestion_does() {
    let mut b = MonoBrowser {
        blocked_origins: vec!["HTTPS://PayPal.com/".into(), "https://mine.example".into()],
        ..Default::default()
    };
    b.switch_to_new_defaults();
    assert_eq!(b.blocked_origins, ["https://mine.example"]);
}
