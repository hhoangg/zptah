//! The "Browser connection" card: its states and what Connect does. A temp home stands in for
//! Chrome's folders, the page opener records instead of launching, and keys stay in memory, so
//! nothing here touches a real browser, keychain or socket.
use super::workflows_ui::tests::click;
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::mono::browser::connect::{
    BrowserConnect, ConnectOptions, PageOpener, Phase, Storage,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};

const LOCAL_STATE: &str = r#"{"profile":{"info_cache":{
  "Default":{"name":"Personal"},"Profile 6":{"name":"Work"}}}}"#;

#[derive(Default)]
struct Opener {
    opened: StdMutex<Vec<(String, String)>>,
    fail: StdMutex<bool>,
}
impl PageOpener for Opener {
    fn open(&self, profile_dir: &str, url: &str) -> Result<(), String> {
        if *self.fail.lock().unwrap() {
            return Err("Could not open Chrome.".into());
        }
        self.opened
            .lock()
            .unwrap()
            .push((profile_dir.into(), url.into()));
        Ok(())
    }
}

struct Home(PathBuf);
impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn chrome_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Google/Chrome")
    } else if cfg!(windows) {
        home.join("local/Google/Chrome/User Data")
    } else {
        home.join(".config/google-chrome")
    }
}

struct Card {
    s: Setup,
    visual: VisualTestContext,
    service: Arc<BrowserConnect>,
    opener: Arc<Opener>,
    home: Home,
}

fn open(cx: &mut TestAppContext, with_chrome: bool) -> Card {
    let s = setup(cx);
    cx.update(|cx| {
        cx.set_reduce_motion(true);
        crate::input::bind(cx);
        crate::popover::bind(cx);
    });
    let home = std::env::temp_dir().join(format!("native-connect-{}", mono::new_id()));
    std::fs::create_dir_all(&home).unwrap();
    let home = Home(home);
    if with_chrome {
        let dir = chrome_dir(&home.0);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Local State"), LOCAL_STATE).unwrap();
    }
    let opener = Arc::new(Opener::default());
    let service = Arc::new(BrowserConnect::new(ConnectOptions {
        app_version: "9.9.9-test".into(),
        home: home.0.clone(),
        local_app_data: Some(home.0.join("local")),
        app_data_dir: home.0.join("app"),
        exe: std::env::current_exe().unwrap(),
        flavor_dev: true,
        storage: Storage::Memory,
        opener: opener.clone(),
        socket_dir: None,
    }));
    let injected = service.clone();
    s.manager
        .update(cx, |m, _| m.browser_ui.connect.service = Some(injected));
    let mut visual = VisualTestContext::from_window(s.window.into(), cx);
    click(&mut visual, "mono-settings-Browser");
    visual.refresh().unwrap();
    click(&mut visual, "mono-browser-enabled");
    visual.refresh().unwrap();
    Card {
        s,
        visual,
        service,
        opener,
        home,
    }
}

impl Card {
    fn on(&mut self, selector: &'static str) -> bool {
        self.visual.debug_bounds(selector).is_some()
    }
    fn click(&mut self, selector: &'static str) {
        click(&mut self.visual, selector);
        self.visual.refresh().unwrap();
    }
    fn refresh(&mut self) {
        self.s
            .manager
            .update(&mut self.visual, |m, cx| m.browser_connect_refresh(cx));
        self.visual.run_until_parked();
        self.visual.refresh().unwrap();
    }
    fn phase(&self) -> Phase {
        self.service.status().phase
    }
    fn manifest_exists(&self) -> bool {
        let dir = chrome_dir(&self.home.0).join("NativeMessagingHosts");
        std::fs::read_dir(dir)
            .map(|mut d| d.next().is_some())
            .unwrap_or(false)
    }
    #[cfg(unix)]
    fn nonce(&self) -> String {
        let url = self.opener.opened.lock().unwrap().last().unwrap().1.clone();
        let encoded = url.split("#n=").nth(1).unwrap().to_owned();
        let mut out = Vec::new();
        let bytes = encoded.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                out.push(u8::from_str_radix(&encoded[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }
}

#[cfg(unix)]
fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

#[gpui::test]
fn without_chrome_the_card_says_so_and_offers_no_connect(cx: &mut TestAppContext) {
    let mut card = open(cx, false);
    assert!(card.on("mono-connect-status"));
    assert!(!card.on("mono-connect-start"));
    assert_eq!(card.phase(), Phase::NoBrowser);
}

#[gpui::test]
fn the_card_lists_profiles_and_connect_writes_the_manifest_and_opens_the_pair_page(
    cx: &mut TestAppContext,
) {
    let mut card = open(cx, true);
    assert!(card.on("mono-connect-profile-Default") && card.on("mono-connect-profile-Profile 6"));
    assert!(!card.manifest_exists(), "nothing is written before Connect");
    card.click("mono-connect-start");
    assert!(card.manifest_exists());
    let opened = card.opener.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(
        opened[0].0, "Default",
        "the first profile is the default choice"
    );
    assert!(opened[0].1.contains("/src/pair/pair.html#n="));
    assert_eq!(card.phase(), Phase::Pairing);
    assert!(card.on("mono-connect-cancel"));
}

#[gpui::test]
fn the_chosen_profile_is_the_one_that_opens(cx: &mut TestAppContext) {
    let mut card = open(cx, true);
    card.click("mono-connect-profile-Profile 6");
    card.click("mono-connect-start");
    assert_eq!(card.opener.opened.lock().unwrap()[0].0, "Profile 6");
    assert!(card.service.status().links.is_empty());
}

#[gpui::test]
fn cancelling_a_pairing_returns_to_the_manifest_written_state(cx: &mut TestAppContext) {
    let mut card = open(cx, true);
    card.click("mono-connect-start");
    card.click("mono-connect-cancel");
    assert_eq!(card.phase(), Phase::ManifestWritten);
    assert!(!card.on("mono-connect-cancel") && card.on("mono-connect-start"));
}

#[gpui::test]
fn a_page_that_cannot_open_shows_the_error_and_offers_try_again(cx: &mut TestAppContext) {
    let mut card = open(cx, true);
    *card.opener.fail.lock().unwrap() = true;
    card.click("mono-connect-start");
    assert_eq!(card.phase(), Phase::WaitingForInstall);
    assert!(card.on("mono-connect-error"));
    assert!(card.manifest_exists(), "the manifest stays written");
    *card.opener.fail.lock().unwrap() = false;
    card.click("mono-connect-start");
    assert_eq!(card.phase(), Phase::Pairing);
    assert!(!card.on("mono-connect-error"));
}

#[cfg(unix)]
#[gpui::test]
fn a_paired_browser_shows_connected_and_disconnect_forgets_it(cx: &mut TestAppContext) {
    let mut card = open(cx, true);
    card.click("mono-connect-profile-Profile 6");
    card.click("mono-connect-start");
    let id = "0123456789abcdef0123456789abcdef";
    let nonce = card.nonce();
    let _link = card.service.connect_fake_extension(id, &nonce).unwrap();
    card.refresh();
    assert_eq!(card.phase(), Phase::Connected);
    let disconnect = leak(format!("mono-connect-disconnect-{id}"));
    assert!(card.on(disconnect));
    assert!(!card.on("mono-connect-cancel"));
    card.click(disconnect);
    assert!(!card.on(disconnect));
    assert!(card.service.status().links.is_empty());
}

#[cfg(unix)]
#[gpui::test]
fn an_extension_with_another_protocol_shows_the_mismatch_state(cx: &mut TestAppContext) {
    let mut card = open(cx, true);
    card.click("mono-connect-start");
    card.service
        .refuse_fake_protocol("0123456789abcdef0123456789abcdef");
    let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while card.phase() != Phase::ProtocolMismatch && std::time::Instant::now() < end {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert_eq!(card.phase(), Phase::ProtocolMismatch);
    card.refresh();
    assert!(card.on("mono-connect-alert"), "the refusal is shown");
    assert!(card.on("mono-connect-start"), "Try again stays available");
}

#[gpui::test]
fn a_key_kept_in_the_fallback_store_is_called_out(cx: &mut TestAppContext) {
    let mut card = open(cx, true);
    assert!(!card.on("mono-connect-fallback"));
    card.s.manager.update(&mut card.visual, |m, cx| {
        m.browser_ui
            .connect
            .status
            .as_mut()
            .unwrap()
            .keys_in_fallback = true;
        cx.notify();
    });
    card.visual.run_until_parked();
    card.visual.refresh().unwrap();
    assert!(card.on("mono-connect-fallback"));
}
