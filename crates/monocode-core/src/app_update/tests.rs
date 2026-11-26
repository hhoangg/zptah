use super::*;

const FEED: &[u8] = include_bytes!("fixtures/feed.json");
const KEY: &str = include_str!("fixtures/public-key.txt");
const PACKAGE: &[u8] = include_bytes!("fixtures/package.txt");
fn config(version: &str) -> Config {
    Config::new(
        "https://updates.example.test/native-preview/latest.json",
        KEY,
        version,
    )
    .unwrap()
}

#[test]
fn updater_matches_the_native_bundle_identifier() {
    assert_eq!(APP_ID, "app.zptah.desktop");
}
#[test]
fn signed_tauri_key_and_packages_work_for_each_native_target() {
    for target in [
        "darwin-aarch64-app",
        "darwin-x86_64-app",
        "windows-x86_64-nsis",
        "linux-x86_64-appimage",
        "linux-x86_64-deb",
        "linux-x86_64-rpm",
    ] {
        let update = verify_feed(FEED, &config("0.16.0-preview.1"), target)
            .unwrap()
            .unwrap();
        assert_eq!(update.version.to_string(), "0.16.0-preview.2");
        verify(PACKAGE, &update.artifact.signature, KEY).unwrap();
        assert!(verify(b"tampered package", &update.artifact.signature, KEY).is_err());
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(PACKAGE).unwrap();
        update.verify_file(file.path()).unwrap();
        file.as_file_mut().set_len(0).unwrap();
        assert!(update.verify_file(file.path()).is_err());
    }
}
#[test]
fn modified_feed_cannot_redirect_a_valid_package_or_invent_a_new_version() {
    let mut envelope: serde_json::Value = serde_json::from_slice(FEED).unwrap();
    let mut payload: serde_json::Value = serde_json::from_slice(
        &STANDARD
            .decode(envelope["payload"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    payload["version"] = serde_json::json!("99.0.0");
    envelope["payload"] = serde_json::json!(STANDARD.encode(serde_json::to_vec(&payload).unwrap()));
    assert!(verify_feed(
        &serde_json::to_vec(&envelope).unwrap(),
        &config("0.16.0-preview.1"),
        "darwin-aarch64-app"
    )
    .unwrap_err()
    .contains("signature verification failed"));
}
#[test]
fn equal_older_and_final_versions_are_never_downgraded_to_a_preview() {
    for current in ["0.16.0-preview.2", "0.16.0-preview.10", "0.16.0", "0.17.0"] {
        assert!(verify_feed(FEED, &config(current), "darwin-aarch64-app")
            .unwrap()
            .is_none());
    }
}
#[test]
fn another_architecture_is_not_used_when_a_target_is_missing() {
    assert!(
        verify_feed(FEED, &config("0.16.0-preview.1"), "windows-aarch64-nsis")
            .unwrap_err()
            .contains("No native update package")
    );
}
#[test]
fn wrong_application_and_channel_are_rejected_even_in_a_signed_payload() {
    let envelope: Envelope = serde_json::from_slice(FEED).unwrap();
    let payload = STANDARD.decode(envelope.payload).unwrap();
    for field in ["app_id", "channel"] {
        let mut manifest: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        manifest[field] = serde_json::json!("production-tauri");
        assert!(select(
            serde_json::from_value(manifest).unwrap(),
            &config("0.16.0-preview.1"),
            "darwin-aarch64-app"
        )
        .unwrap_err()
        .contains("different application or release channel"));
    }
}
#[test]
fn update_endpoints_require_https_and_a_valid_embedded_key() {
    for endpoint in [
        "http://updates.example.test/latest.json",
        "file:///tmp/update",
        "https://user:password@example.test/latest.json",
    ] {
        assert!(Config::new(endpoint, KEY, "0.16.0").is_err());
    }
    assert!(Config::new(
        "https://updates.example.test/latest.json",
        "invalid key",
        "0.16.0"
    )
    .is_err());
}
#[test]
fn unsigned_tauri_production_feed_is_never_accepted_as_native() {
    assert!(verify_feed(
        br#"{"version":"99.0.0","platforms":{}}"#,
        &config("0.16.0-preview.1"),
        "darwin-aarch64-app"
    )
    .is_err());
}

mod recheck {
    use super::*;
    use std::{
        io::{BufRead, BufReader},
        net::TcpListener,
        sync::{Arc, Mutex},
    };

    struct Keys {
        pk: minisign::PublicKey,
        sk: minisign::SecretKey,
        public: String,
    }
    fn keys() -> Keys {
        let pair = minisign::KeyPair::generate_unencrypted_keypair().unwrap();
        let public = STANDARD.encode(pair.pk.to_box().unwrap().to_string());
        Keys {
            pk: pair.pk,
            sk: pair.sk,
            public,
        }
    }
    /// Minisign signature (prehashed, as Tauri's signer emits) in the app's base64 form.
    fn sign(keys: &Keys, bytes: &[u8]) -> String {
        let signature = minisign::sign(
            Some(&keys.pk),
            &keys.sk,
            std::io::Cursor::new(bytes),
            Some("test"),
            None,
        )
        .unwrap();
        STANDARD.encode(signature.to_string())
    }
    fn package(version: &str) -> Vec<u8> {
        format!("package {version}").into_bytes()
    }
    fn installation() -> Installation {
        Installation {
            kind: PackageKind::Deb,
            path: PathBuf::from("/unused"),
        }
    }
    /// A feed for `version`, signed by `keys`, whose package is served by `base`.
    fn feed(keys: &Keys, base: &str, version: &str) -> Vec<u8> {
        let payload = serde_json::to_vec(&serde_json::json!({
            "app_id": APP_ID,
            "channel": CHANNEL,
            "version": version,
            "notes": "",
            "platforms": {installation().target().unwrap(): {
                "url": format!("{base}/package-{version}"),
                "signature": sign(keys, &package(version)),
            }},
        }))
        .unwrap();
        serde_json::to_vec(&serde_json::json!({
            "payload": STANDARD.encode(&payload),
            "signature": sign(keys, &payload),
        }))
        .unwrap()
    }

    /// Serves `/latest.json` like a cache: `stale` unless the request carries no-cache.
    struct Server {
        base: String,
        stale: Arc<Mutex<Vec<u8>>>,
        fresh: Arc<Mutex<Vec<u8>>>,
    }
    impl Server {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
            let stale = Arc::new(Mutex::new(Vec::new()));
            let fresh = Arc::new(Mutex::new(Vec::new()));
            let (stale_feed, fresh_feed) = (stale.clone(), fresh.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming() {
                    let Ok(mut stream) = stream else { continue };
                    let mut reader = BufReader::new(stream.try_clone().unwrap());
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap_or(0);
                    let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
                    let mut no_cache = false;
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        let line = line.to_ascii_lowercase();
                        no_cache |= line.starts_with("cache-control:") && line.contains("no-cache");
                    }
                    let body = if path == "/latest.json" {
                        let feed = if no_cache { &fresh_feed } else { &stale_feed };
                        feed.lock().unwrap().clone()
                    } else if let Some(version) = path.strip_prefix("/package-") {
                        package(version)
                    } else {
                        Vec::new()
                    };
                    let head = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = stream.write_all(head.as_bytes());
                    let _ = stream.write_all(&body);
                }
            });
            Self { base, stale, fresh }
        }
        fn config(&self, keys: &Keys, current: &str) -> Config {
            Config::new(&format!("{}/latest.json", self.base), &keys.public, current).unwrap()
        }
        fn publish(&self, keys: &Keys, stale: &str, fresh: &str) {
            *self.stale.lock().unwrap() = feed(keys, &self.base, stale);
            *self.fresh.lock().unwrap() = feed(keys, &self.base, fresh);
        }
    }
    fn version(update: &Update) -> String {
        update.version.to_string()
    }

    #[test]
    fn a_newer_feed_replaces_the_pending_update_and_its_package_verifies() {
        let keys = keys();
        let server = Server::start();
        let config = server.config(&keys, "2.0.0");
        // Installed v2; the app found v3 earlier.
        server.publish(&keys, "3.0.0", "3.0.0");
        let pending = check(&config, &installation()).unwrap().unwrap();
        assert_eq!(version(&pending), "3.0.0");
        // v4 has been published since.
        server.publish(&keys, "4.0.0", "4.0.0");
        let update = latest(pending, &config, &installation());
        assert_eq!(version(&update), "4.0.0");
        let download = update.download(|_, _| {}).unwrap();
        update.verify_file(&download.path).unwrap();
        assert_eq!(std::fs::read(&download.path).unwrap(), package("4.0.0"));
    }

    #[test]
    fn the_feed_request_opts_out_of_caches() {
        let keys = keys();
        let server = Server::start();
        // A cache in front of the feed keeps serving v3 unless asked for no-cache.
        server.publish(&keys, "3.0.0", "4.0.0");
        let found = check(&server.config(&keys, "2.0.0"), &installation())
            .unwrap()
            .unwrap();
        assert_eq!(version(&found), "4.0.0");
    }

    #[test]
    fn pending_is_kept_when_the_feed_is_unreachable_or_has_nothing_newer() {
        let keys = keys();
        let server = Server::start();
        let config = server.config(&keys, "2.0.0");
        server.publish(&keys, "3.0.0", "3.0.0");
        let pending = check(&config, &installation()).unwrap().unwrap();
        // Nothing newer.
        let same = latest(pending.clone(), &config, &installation());
        assert_eq!(version(&same), "3.0.0");
        // An older feed never downgrades the pending update.
        server.publish(&keys, "2.5.0", "2.5.0");
        let older = latest(pending.clone(), &config, &installation());
        assert_eq!(version(&older), "3.0.0");
        // Unreachable: nothing listens on a port that was just closed.
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!(
            "http://127.0.0.1:{}/latest.json",
            closed.local_addr().unwrap().port()
        );
        drop(closed);
        let offline = Config::new(&endpoint, &keys.public, "2.0.0").unwrap();
        assert!(check(&offline, &installation()).is_err());
        let kept = latest(pending, &offline, &installation());
        assert_eq!(version(&kept), "3.0.0");
    }

    #[test]
    fn a_fresh_feed_with_a_bad_signature_is_rejected_and_pending_is_kept() {
        let keys = keys();
        let server = Server::start();
        let config = server.config(&keys, "2.0.0");
        server.publish(&keys, "3.0.0", "3.0.0");
        let pending = check(&config, &installation()).unwrap().unwrap();
        // A v4 feed signed by some other key.
        let attacker = self::keys();
        *server.fresh.lock().unwrap() = feed(&attacker, &server.base, "4.0.0");
        let error = check(&config, &installation()).unwrap_err();
        assert!(error.contains("signature verification failed"), "{error}");
        let kept = latest(pending.clone(), &config, &installation());
        assert_eq!(version(&kept), "3.0.0");
        // A v4 payload edited after signing.
        let mut envelope: serde_json::Value =
            serde_json::from_slice(&feed(&keys, &server.base, "4.0.0")).unwrap();
        let mut payload: serde_json::Value = serde_json::from_slice(
            &STANDARD
                .decode(envelope["payload"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        payload["version"] = serde_json::json!("9.0.0");
        envelope["payload"] =
            serde_json::json!(STANDARD.encode(serde_json::to_vec(&payload).unwrap()));
        *server.fresh.lock().unwrap() = serde_json::to_vec(&envelope).unwrap();
        assert!(check(&config, &installation()).is_err());
        let kept = latest(pending, &config, &installation());
        assert_eq!(version(&kept), "3.0.0");
    }

    #[test]
    fn versions_compare_as_semver_not_as_text() {
        let keys = keys();
        let server = Server::start();
        let config = server.config(&keys, "2.0.0");
        server.publish(&keys, "9.0.0", "10.0.0");
        let nine = verify_feed(
            &feed(&keys, &server.base, "9.0.0"),
            &config,
            &installation().target().unwrap(),
        )
        .unwrap()
        .unwrap();
        let ten = check(&config, &installation()).unwrap().unwrap();
        assert_eq!(
            version(&newest(nine.clone(), Ok(Some(ten.clone())))),
            "10.0.0"
        );
        assert_eq!(version(&newest(ten, Ok(Some(nine)))), "10.0.0");
    }
}
