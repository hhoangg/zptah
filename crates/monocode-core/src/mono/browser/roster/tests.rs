use super::*;
use crate::mono::browser::link::auth::{encode_bytes, PairingKey, NONCE_LEN};
use crate::mono::browser::link::protocol::Method;
use crate::mono::browser::links::fake_extension::socket_pair;
use crate::mono::browser::links::keystore::MemoryKeyStore;
use crate::mono::browser::links::manager::{Deps, LinkEvent, ManagerConfig, MemoryRecordStore};
use crate::mono::browser::links::pairing::ProfileChoice;
use crate::mono::browser::peer::{Flavor, PeerReport};
use crate::mono::browser::{session_color, MonoBrowser};
use crate::mono::COLORS_MONO;
use std::path::PathBuf;
use std::sync::Arc;

const INSTANCE: &str = "aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbb";
const NONCE: [u8; NONCE_LEN] = [9; NONCE_LEN];
const WAIT: Duration = Duration::from_secs(5);

fn mono(id: &str, name: &str, color: u32, mascot: &str, profile: &str) -> Mono {
    Mono {
        id: id.into(),
        name: name.into(),
        color,
        mascot: mascot.into(),
        browser: Some(MonoBrowser {
            enabled: true,
            use_extension: true,
            profile_dir: profile.into(),
            blocked_origins: vec!["https://Bank.example/path".into(), "bank.example".into()],
            allowed_origins: vec!["shop.example".into(), "*.bad".into()],
            ..MonoBrowser::default()
        }),
        ..Mono::default()
    }
}

const MIRA: &str = "0123456789abcdef0123456789abcdef";
const KAI: &str = "fedcba9876543210fedcba9876543210";

// ---- colors ----

#[test]
fn no_preset_mono_color_maps_to_grey_and_each_maps_to_a_named_color() {
    for color in COLORS_MONO {
        let mapped = group_color(color);
        assert_ne!(mapped, ChromeGroupColor::Grey, "{color:06x}");
    }
    let named: Vec<&str> = COLORS_MONO
        .iter()
        .map(|c| group_color_name(group_color(*c)))
        .collect();
    assert_eq!(
        named,
        ["blue", "orange", "yellow", "green", "pink", "purple", "cyan", "orange", "purple"],
        "{named:?}"
    );
}

#[test]
fn colors_follow_the_nearest_chrome_color() {
    for (rgb, want) in [
        (0xd93025, ChromeGroupColor::Red),
        (0x2060f0, ChromeGroupColor::Blue),
        (0x20b030, ChromeGroupColor::Green),
        (0xf9ab00, ChromeGroupColor::Yellow),
        (0x129eaf, ChromeGroupColor::Cyan),
        (0xe8710a, ChromeGroupColor::Orange),
        (0xd01884, ChromeGroupColor::Pink),
        (0x9334e6, ChromeGroupColor::Purple),
    ] {
        assert_eq!(group_color(rgb), want, "{rgb:06x}");
    }
}

#[test]
fn grey_is_for_low_chroma_colors_only() {
    for rgb in [
        0x808080, 0x5f6368, 0x000000, 0xffffff, 0x8b949c, 0x303030, 0x8a6f6f,
    ] {
        assert_eq!(group_color(rgb), ChromeGroupColor::Grey, "{rgb:06x}");
    }
    // A muted but clearly colored one is not grey.
    assert_eq!(group_color(0x8f5a5a), ChromeGroupColor::Red);
    assert_ne!(group_color(0x6f6f9a), ChromeGroupColor::Grey);
    assert_eq!(group_color(0x0b6b73), ChromeGroupColor::Cyan);
    // A light green stays green (lightness counts half).
    assert_eq!(group_color(0x39c363), ChromeGroupColor::Green);
}

#[test]
fn the_old_session_color_names_follow_the_same_mapping() {
    assert_eq!(session_color(0xd93025), "red");
    assert_eq!(session_color(0x808080), "grey");
}

// ---- entries ----

#[test]
fn an_entry_carries_name_emoji_color_level_and_normalized_origins() {
    let mut m = mono(MIRA, "Mira", 0xd93025, "frog", "Profile 1");
    let (dir, entry) = entry_for(&m).expect("an entry");
    assert_eq!(dir, "Profile 1");
    assert_eq!(entry.id, MIRA);
    assert_eq!(entry.name, "Mira");
    assert_eq!(entry.emoji, "\u{1F438}", "the mascot's emoji by default");
    assert_eq!(entry.color, "#d93025");
    assert_eq!(entry.group_color, ChromeGroupColor::Red);
    assert_eq!(entry.level, AccessLevel::Act);
    assert_eq!(entry.blocked_origins, ["https://bank.example"]);
    assert_eq!(entry.allowed_origins, ["https://shop.example"]);
    assert!(entry.cursor);

    m.badge_emoji = Some("\u{1F984}".into());
    m.browser.as_mut().unwrap().trust = BrowserTrust::ReadOnly;
    m.browser.as_mut().unwrap().show_actions = false;
    let (_, entry) = entry_for(&m).unwrap();
    assert_eq!(entry.emoji, "\u{1F984}");
    assert_eq!(entry.level, AccessLevel::Read);
    assert!(!entry.cursor);
}

#[test]
fn a_mono_that_does_not_use_the_extension_has_no_entry() {
    let mut m = mono(MIRA, "Mira", 0xd93025, "frog", "Profile 1");
    m.browser.as_mut().unwrap().use_extension = false;
    assert!(entry_for(&m).is_none());
    m.browser.as_mut().unwrap().use_extension = true;
    m.browser.as_mut().unwrap().enabled = false;
    assert!(entry_for(&m).is_none());
    m.browser.as_mut().unwrap().enabled = true;
    m.browser.as_mut().unwrap().profile_dir = String::new();
    assert!(entry_for(&m).is_none());
    // An id the protocol cannot carry is left out, not sent.
    let old = mono("legacy-id", "Old", 0xd93025, "frog", "Profile 1");
    assert!(entry_for(&old).is_none());
    assert!(entry_for(&Mono::default()).is_none());
}

#[test]
fn the_book_holds_the_roster_of_each_profile_apart() {
    let book = RosterBook::default();
    let monos = [
        mono(MIRA, "Mira", 0xd93025, "frog", "Profile 1"),
        mono(KAI, "Kai", 0x2060f0, "cat", "Profile 2"),
    ];
    assert!(book.set_monos(&monos));
    assert!(!book.set_monos(&monos), "nothing changed");
    assert_eq!(book.roster_for("Profile 1").len(), 1);
    assert_eq!(book.roster_for("Profile 1")[0].name, "Mira");
    assert_eq!(book.roster_for("Profile 2")[0].name, "Kai");
    assert!(book.roster_for("Profile 3").is_empty());
    assert_eq!(book.label_of(KAI).as_deref(), Some("Kai"));
    // A Mono moved to another profile leaves the first one.
    let mut moved = monos[0].clone();
    moved.browser.as_mut().unwrap().profile_dir = "Profile 2".into();
    assert!(book.upsert(&moved));
    assert!(book.roster_for("Profile 1").is_empty());
    assert_eq!(book.roster_for("Profile 2").len(), 2);
    // A Mono that stops using the extension is dropped.
    let mut off = moved.clone();
    off.browser.as_mut().unwrap().use_extension = false;
    assert!(book.upsert(&off));
    assert_eq!(book.roster_for("Profile 2").len(), 1);
}

// ---- sending ----

struct Source(Arc<RosterBook>);
impl RosterSource for Source {
    fn snapshot(&self, instance_id: &str, profile_dir: &str) -> AuthResult {
        self.0.snapshot(instance_id, profile_dir)
    }
}

fn report() -> PeerReport {
    PeerReport {
        pid: 4242,
        parent_pid: Some(100),
        parent_exe: Some(PathBuf::from("/Applications/Google Chrome.app/chrome")),
        user_data_dir: Some(PathBuf::from("/tmp/chrome-data")),
    }
}

/// A paired and connected fake extension of the profile "Profile 1", plus the manager.
fn connected(
    book: &Arc<RosterBook>,
) -> (
    LinkManager,
    crate::mono::browser::links::fake_extension::FakeExtension,
    AuthResult,
    std::sync::mpsc::Receiver<LinkEvent>,
) {
    let (manager, events) = LinkManager::new(
        ManagerConfig::new(Flavor::Release, "9.9.9-test"),
        Deps::new(
            Arc::new(MemoryKeyStore::new()),
            Arc::new(MemoryRecordStore::new()),
        ),
    );
    manager.set_roster_source(Arc::new(Source(book.clone())));
    manager.start_pairing_with(ProfileChoice::new("Profile 1", "Work").unwrap(), NONCE);
    let serve = |key: PairingKey| {
        let (conn, fake) = socket_pair(INSTANCE, key);
        let m = manager.clone();
        let handle = std::thread::spawn(move || m.serve(conn, &report()));
        (fake, handle)
    };
    let (mut fake, handle) = serve(PairingKey::generate());
    let key = fake.pair(&encode_bytes(&NONCE)).expect("pairing works");
    handle.join().unwrap();
    let (mut fake, _serving) = serve(key);
    let auth = fake.connect().expect("connect works");
    loop {
        if let LinkEvent::Connected { .. } = events.recv_timeout(WAIT).expect("a connected event") {
            break;
        }
    }
    (manager, fake, auth, events)
}

#[test]
fn the_extension_gets_the_roster_in_auth_and_roster_set_when_it_changes() {
    let book = Arc::new(RosterBook::default());
    book.set_monos(&[
        mono(MIRA, "Mira", 0xd93025, "frog", "Profile 1"),
        mono(KAI, "Kai", 0x2060f0, "cat", "Profile 2"),
    ]);
    let (manager, mut fake, auth, _events) = connected(&book);
    // Link up: only this profile's Monos, with the emoji, the exact color and the group color.
    assert_eq!(auth.roster.len(), 1, "{:?}", auth.roster);
    assert_eq!(auth.roster[0].name, "Mira");
    assert_eq!(auth.roster[0].emoji, "\u{1F438}");
    assert_eq!(auth.roster[0].group_color, ChromeGroupColor::Red);

    // Nothing changed since auth: nothing is sent.
    book.publish_with(&manager);
    // A Mono is renamed and a second one joins this profile.
    let mut renamed = mono(MIRA, "Mira R", 0xd93025, "frog", "Profile 1");
    renamed.badge_emoji = Some("\u{1F984}".into());
    book.set_monos(&[renamed, mono(KAI, "Kai", 0x2060f0, "cat", "Profile 1")]);
    let publisher = {
        let book = book.clone();
        let manager = manager.clone();
        std::thread::spawn(move || book.publish_with(&manager))
    };
    let request = fake.next_request().expect("a request");
    assert_eq!(request.method.as_deref(), Some(Method::RosterSet.as_str()));
    assert_eq!(request.mono, None, "roster.set is not Mono-scoped");
    let monos = request.params.as_ref().unwrap()["monos"]
        .as_array()
        .unwrap();
    assert_eq!(monos.len(), 2);
    assert_eq!(monos[0]["name"], "Mira R");
    assert_eq!(monos[0]["emoji"], "\u{1F984}");
    assert_eq!(monos[1]["id"], KAI);
    fake.respond_ok(&request, json!({}));
    publisher.join().unwrap();

    // The same roster again is not sent: the next request the app makes is the status poll.
    book.publish_with(&manager);
    let handle = manager.handle(INSTANCE).expect("a live link");
    let poll = std::thread::spawn(move || handle.request(Method::StatusGet, None, json!({}), WAIT));
    let next = fake.next_request().expect("a request");
    assert_eq!(next.method.as_deref(), Some(Method::StatusGet.as_str()));
    fake.respond_ok(
        &next,
        json!({"stopped": false, "debugger_attached": [], "monos_active": 0}),
    );
    poll.join().unwrap().expect("status answered");
}

#[test]
fn a_roster_set_the_extension_refuses_is_sent_again_next_time() {
    let book = Arc::new(RosterBook::default());
    let (manager, mut fake, _auth, _events) = connected(&book);
    book.set_monos(&[mono(MIRA, "Mira", 0xd93025, "frog", "Profile 1")]);
    for attempt in 0..2 {
        let publisher = {
            let book = book.clone();
            let manager = manager.clone();
            std::thread::spawn(move || book.publish_with(&manager))
        };
        let request = fake.next_request().expect("a request");
        assert_eq!(request.method.as_deref(), Some("roster.set"), "{attempt}");
        if attempt == 0 {
            fake.respond_err(
                &request,
                &crate::mono::browser::link::protocol::ProtocolError::new(
                    crate::mono::browser::link::protocol::ErrorCode::Invalid,
                    "no",
                ),
            );
        } else {
            fake.respond_ok(&request, json!({}));
        }
        publisher.join().unwrap();
    }
}
