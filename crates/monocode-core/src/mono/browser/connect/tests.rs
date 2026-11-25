//! The Connect service over a temp home: no real browser folder, keychain or socket directory is
//! touched, and no browser is launched.
use super::*;
#[cfg(unix)]
use crate::mono::browser::link::auth::PairingKey;
#[cfg(unix)]
use crate::mono::browser::links::fake_extension::socket_pair;
#[cfg(unix)]
use crate::mono::browser::peer::PeerReport;
#[cfg(unix)]
use std::time::{Duration, Instant};

#[cfg(unix)]
const INSTANCE: &str = "0123456789abcdef0123456789abcdef";
const LOCAL_STATE: &str = r#"{"profile":{"info_cache":{
  "Default":{"name":"Personal"},"Profile 6":{"name":"Work"}}}}"#;

#[derive(Default)]
struct FakeOpener {
    opened: Mutex<Vec<(String, String)>>,
    fail: Mutex<bool>,
}

impl PageOpener for FakeOpener {
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

struct Rig {
    connect: BrowserConnect,
    opener: Arc<FakeOpener>,
    home: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn data_dir(home: &Path) -> PathBuf {
    InstallEnv {
        os: manifest_install::Os::current(),
        home: home.to_path_buf(),
        local_app_data: Some(home.join("local")),
        app_data_dir: home.join("app"),
    }
    .data_dir(Browser::Chrome)
    .expect("chrome dir")
}

fn rig(with_chrome: bool) -> Rig {
    let home = std::env::temp_dir().join(format!("zptah-connect-{:016x}", rand::random::<u64>()));
    std::fs::create_dir_all(&home).unwrap();
    if with_chrome {
        let dir = data_dir(&home);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Local State"), LOCAL_STATE).unwrap();
    }
    let opener = Arc::new(FakeOpener::default());
    let connect = BrowserConnect::new(ConnectOptions {
        app_version: "9.9.9-test".into(),
        home: home.clone(),
        local_app_data: Some(home.join("local")),
        app_data_dir: home.join("app"),
        exe: std::env::current_exe().unwrap(),
        flavor_dev: true,
        storage: Storage::Memory,
        real_registry: false,
        opener: opener.clone(),
        socket_dir: None,
    });
    Rig {
        connect,
        opener,
        home,
    }
}

fn manifest_path(rig: &Rig) -> PathBuf {
    let file = format!("{}.json", manifest_install::DEV_HOST_NAME);
    if cfg!(windows) {
        // Windows keeps the file in the app data dir and points the registry at it.
        rig.home.join("app").join("browser-host").join(file)
    } else {
        data_dir(&rig.home).join("NativeMessagingHosts").join(file)
    }
}

#[cfg(unix)]
fn nonce_from(url: &str) -> String {
    let encoded = url.split("#n=").nth(1).expect("a nonce in the url");
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

#[cfg(unix)]
fn report(rig: &Rig) -> PeerReport {
    PeerReport {
        pid: 4242,
        parent_pid: Some(100),
        parent_exe: Some(PathBuf::from("/Applications/Google Chrome.app/chrome")),
        user_data_dir: Some(data_dir(&rig.home)),
    }
}

#[cfg(unix)]
fn wait_for(rig: &Rig, want: Phase) -> ConnectStatus {
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let status = rig.connect.status();
        if status.phase == want || Instant::now() > end {
            return status;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn without_a_browser_the_phase_is_no_browser_and_connect_refuses() {
    let rig = rig(false);
    assert_eq!(rig.connect.status().phase, Phase::NoBrowser);
    assert!(rig.connect.connect("Default", "Personal").is_err());
    assert!(rig.opener.opened.lock().unwrap().is_empty());
    assert!(!manifest_path(&rig).exists());
}

#[test]
fn a_detected_browser_lists_its_profiles_and_waits_for_connect() {
    let rig = rig(true);
    let status = rig.connect.status();
    assert_eq!(status.phase, Phase::Ready);
    assert_eq!(status.browsers.len(), 1);
    let names: Vec<_> = status.browsers[0]
        .profiles
        .iter()
        .map(|p| (p.dir.as_str(), p.name.as_str()))
        .collect();
    assert_eq!(names, [("Default", "Personal"), ("Profile 6", "Work")]);
    assert!(
        !manifest_path(&rig).exists(),
        "nothing is written before Connect"
    );
}

#[test]
fn connect_writes_the_manifest_and_opens_the_pair_page_in_that_profile() {
    let rig = rig(true);
    rig.connect.connect("Profile 6", "Work").unwrap();
    let text = std::fs::read_to_string(manifest_path(&rig)).unwrap();
    assert!(text.contains(manifest_install::DEV_HOST_NAME));
    let opened = rig.opener.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    assert_eq!(opened[0].0, "Profile 6");
    assert!(opened[0].1.starts_with(&format!(
        "chrome-extension://{}/src/pair/pair.html#n=",
        manifest_install::DEV_EXTENSION_ID
    )));
    let status = rig.connect.status();
    assert_eq!(status.phase, Phase::Pairing);
    assert!(status.pairing_expires_ms.is_some());
}

#[test]
fn a_failed_open_waits_for_install_and_keeps_the_manifest() {
    let rig = rig(true);
    *rig.opener.fail.lock().unwrap() = true;
    assert!(rig.connect.connect("Default", "Personal").is_err());
    let status = rig.connect.status();
    assert_eq!(status.phase, Phase::WaitingForInstall);
    assert_eq!(status.alert.as_deref(), Some("Could not open Chrome."));
    assert!(manifest_path(&rig).exists());
    assert!(
        status.pairing_expires_ms.is_none(),
        "the window is closed again"
    );
}

#[test]
fn an_invalid_profile_name_is_refused_before_anything_is_written() {
    let rig = rig(true);
    assert!(rig.connect.connect("../Default", "x").is_err());
    assert!(!manifest_path(&rig).exists());
}

#[test]
fn a_written_manifest_without_a_pairing_is_the_manifest_written_phase() {
    let rig = rig(true);
    rig.connect.connect("Default", "Personal").unwrap();
    rig.connect.cancel_pairing();
    assert_eq!(rig.connect.status().phase, Phase::ManifestWritten);
}

#[cfg(unix)]
#[test]
fn pairing_then_connecting_reaches_connected_and_disconnect_forgets_it() {
    let rig = rig(true);
    rig.connect.connect("Profile 6", "Work").unwrap();
    let nonce = nonce_from(&rig.opener.opened.lock().unwrap()[0].1);
    let (conn, mut fake) = socket_pair(INSTANCE, PairingKey::generate());
    let (manager, rep) = (rig.connect.manager.clone(), report(&rig));
    let handle = std::thread::spawn(move || manager.serve(conn, &rep));
    let key = fake.pair(&nonce).expect("pairing works");
    handle.join().unwrap();
    assert!(rig.connect.status().links.iter().any(|l| l.name == "Work"));

    let (conn, mut fake) = socket_pair(INSTANCE, key);
    let (manager, rep) = (rig.connect.manager.clone(), report(&rig));
    let _serving = std::thread::spawn(move || manager.serve(conn, &rep));
    fake.connect().expect("connects");
    let status = wait_for(&rig, Phase::Connected);
    assert_eq!(status.phase, Phase::Connected);
    assert!(status.links[0].connected && status.links[0].profile_dir == "Profile 6");
    assert!(!status.keys_in_fallback);

    rig.connect.disconnect(INSTANCE).unwrap();
    assert!(rig.connect.status().links.is_empty());
}

#[cfg(unix)]
#[test]
fn an_unsupported_protocol_range_is_the_protocol_mismatch_phase() {
    let rig = rig(true);
    rig.connect.connect("Default", "Personal").unwrap();
    let (conn, mut fake) = socket_pair(INSTANCE, PairingKey::generate());
    let (manager, rep) = (rig.connect.manager.clone(), report(&rig));
    let handle = std::thread::spawn(move || manager.serve(conn, &rep));
    fake.tweak_hello(|p| p.protocol = [2, 3]);
    let _ = fake.hello_only(true);
    handle.join().unwrap();
    let status = wait_for(&rig, Phase::ProtocolMismatch);
    assert_eq!(status.phase, Phase::ProtocolMismatch);
    assert!(status.alert.is_some());
}

#[test]
fn disconnect_all_removes_the_manifest_and_resets_the_card() {
    let rig = rig(true);
    rig.connect.connect("Default", "Personal").unwrap();
    rig.connect.disconnect_all().unwrap();
    assert!(!manifest_path(&rig).exists());
    assert_eq!(rig.connect.status().phase, Phase::Ready);
}

#[test]
fn the_release_flavor_refuses_connect_until_the_store_listing_exists() {
    let mut rig = rig(true);
    rig.connect = BrowserConnect::new(ConnectOptions {
        app_version: "9.9.9-test".into(),
        home: rig.home.clone(),
        local_app_data: Some(rig.home.join("local")),
        app_data_dir: rig.home.join("app"),
        exe: std::env::current_exe().unwrap(),
        flavor_dev: false,
        storage: Storage::Memory,
        real_registry: false,
        opener: rig.opener.clone(),
        socket_dir: None,
    });
    assert_eq!(
        rig.connect.status().unavailable.as_deref(),
        Some(NOT_IN_STORE_YET)
    );
    assert_eq!(
        rig.connect.connect("Default", "Personal").unwrap_err(),
        NOT_IN_STORE_YET
    );
    assert!(rig.opener.opened.lock().unwrap().is_empty());
    assert!(!manifest_path(&rig).exists());
}

#[test]
fn a_socket_dir_error_is_shown_as_the_server_error() {
    let mut rig = rig(true);
    rig.connect = BrowserConnect::new(ConnectOptions {
        app_version: "9.9.9-test".into(),
        home: rig.home.clone(),
        local_app_data: Some(rig.home.join("local")),
        app_data_dir: rig.home.join("app"),
        exe: std::env::current_exe().unwrap(),
        flavor_dev: true,
        storage: Storage::Memory,
        real_registry: false,
        opener: rig.opener.clone(),
        socket_dir: Some(Err("The browser link could not start: no dir.".into())),
    });
    let status = rig.connect.status();
    assert_eq!(
        status.server_error.as_deref(),
        Some("The browser link could not start: no dir.")
    );
    assert!(rig.connect.connect("Default", "Personal").is_err());
}
