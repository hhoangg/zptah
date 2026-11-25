//! Tests of the app side of the link. Nothing here touches a real keychain, browser directory or
//! the user's Chrome: keys and records are in memory and installs go to temp directories.
use super::super::link::auth::{NONCE_LEN, PAIRING_TTL_MS};
use super::super::link::protocol::{AuthResult, ErrorCode, Event, Method, ProtocolError};
use super::super::peer::{Flavor, PeerReport};
use super::keystore::{
    keyring_service, FallbackKeyStore, LinkKeyStore, MemoryKeyStore, MonoSecretsKeyStore,
};
use super::manager::{
    Deps, FileRecordStore, LinkEvent, LinkManager, LinkRecord, LinkRecordStore, ManagerConfig,
    MemoryRecordStore, Refusal, RequestError,
};
use super::pairing::{open_in_profile_args, pair_page_url, PairingState, ProfileChoice};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

const INSTANCE: &str = "0123456789abcdef0123456789abcdef";
const OTHER_INSTANCE: &str = "fedcba9876543210fedcba9876543210";
const NONCE: [u8; NONCE_LEN] = [7; NONCE_LEN];
const WAIT: Duration = Duration::from_secs(5);

fn nonce_text() -> String {
    super::super::link::auth::encode_bytes(&NONCE)
}

fn report() -> PeerReport {
    PeerReport {
        pid: 4242,
        parent_pid: Some(100),
        parent_exe: Some(PathBuf::from("/Applications/Google Chrome.app/chrome")),
        user_data_dir: Some(PathBuf::from("/tmp/chrome-data")),
    }
}

fn profile() -> ProfileChoice {
    ProfileChoice::new("Profile 1", "Work").unwrap()
}

struct Rig {
    manager: LinkManager,
    events: Receiver<LinkEvent>,
    keys: Arc<MemoryKeyStore>,
    records: Arc<MemoryRecordStore>,
    clock: Arc<AtomicU64>,
}

fn rig_with(flavor: Flavor, tweak: impl FnOnce(&mut ManagerConfig)) -> Rig {
    let keys = Arc::new(MemoryKeyStore::new());
    let records = Arc::new(MemoryRecordStore::new());
    let clock = Arc::new(AtomicU64::new(1_000));
    let mut deps = Deps::new(keys.clone(), records.clone());
    let c = clock.clone();
    deps.clock = Arc::new(move || c.load(Ordering::SeqCst));
    let mut config = ManagerConfig::new(flavor, "9.9.9-test");
    tweak(&mut config);
    let (manager, events) = LinkManager::new(config, deps);
    Rig {
        manager,
        events,
        keys,
        records,
        clock,
    }
}

fn rig() -> Rig {
    rig_with(Flavor::Release, |_| {})
}

#[cfg(unix)]
mod over_socket {
    use super::super::fake_extension::{socket_pair, FakeExtension};
    use super::*;
    use crate::mono::browser::link::auth::PairingKey;

    impl Rig {
        fn serve(
            &self,
            fake_id: &str,
            key: PairingKey,
            rep: PeerReport,
        ) -> (FakeExtension, std::thread::JoinHandle<()>) {
            let (conn, fake) = socket_pair(fake_id, key);
            let manager = self.manager.clone();
            let handle = std::thread::spawn(move || manager.serve(conn, &rep));
            (fake, handle)
        }

        fn next(&self, mut pred: impl FnMut(&LinkEvent) -> bool) -> LinkEvent {
            loop {
                let event = self
                    .events
                    .recv_timeout(WAIT)
                    .expect("an event within the timeout");
                if pred(&event) {
                    return event;
                }
            }
        }

        /// Pairs a fresh extension and returns its key.
        fn pair(&self, instance: &str) -> PairingKey {
            self.manager.start_pairing_with(profile(), NONCE);
            let (mut fake, handle) = self.serve(instance, PairingKey::generate(), report());
            let key = fake.pair(&nonce_text()).expect("pairing works");
            handle.join().unwrap();
            key
        }

        /// Pairs, reconnects and returns the live extension.
        fn connected(&self, instance: &str) -> (FakeExtension, std::thread::JoinHandle<()>) {
            let key = self.pair(instance);
            let (mut fake, handle) = self.serve(instance, key, report());
            fake.connect().expect("connect works");
            self.next(|e| matches!(e, LinkEvent::Connected { .. }));
            (fake, handle)
        }
    }

    fn code(e: ProtocolError) -> ErrorCode {
        e.code
    }

    #[test]
    fn pairing_stores_the_key_and_a_record_then_the_extension_connects() {
        let rig = rig();
        let key = rig.pair(INSTANCE);
        assert!(rig.keys.get(INSTANCE).unwrap().is_some());
        assert_eq!(rig.keys.len(), 1);
        let record = rig.records.get(INSTANCE).unwrap().expect("record");
        assert_eq!(record.name, "Work");
        assert_eq!(record.profile_dir, "Profile 1");
        assert_eq!(record.brand, "Google Chrome");
        assert_eq!(record.user_data_dir, report().user_data_dir);
        assert_eq!(record.parent_exe, report().parent_exe);
        assert_eq!(rig.manager.pairing_state(), PairingState::Idle);
        rig.next(|e| matches!(e, LinkEvent::Paired { name, .. } if name == "Work"));

        let (mut fake, handle) = rig.serve(INSTANCE, key, report());
        let result: AuthResult = fake.connect().expect("paired connect");
        assert!(!result.stopped);
        rig.next(|e| matches!(e, LinkEvent::Connected { .. }));
        let info = rig.manager.links().unwrap();
        assert_eq!(info.len(), 1);
        assert!(info[0].connected);
        fake.close();
        handle.join().unwrap();
        rig.next(|e| matches!(e, LinkEvent::Disconnected { .. }));
        assert!(!rig.manager.links().unwrap()[0].connected);
    }

    #[test]
    fn the_pairing_nonce_works_once() {
        let rig = rig();
        rig.pair(INSTANCE);
        let (mut again, handle) = rig.serve(OTHER_INSTANCE, PairingKey::generate(), report());
        let err = again.pair(&nonce_text()).expect_err("window is gone");
        assert_eq!(code(err), ErrorCode::NotPaired);
        handle.join().unwrap();
        assert_eq!(rig.keys.len(), 1);
    }

    #[test]
    fn a_wrong_nonce_is_refused_and_stores_nothing() {
        let rig = rig();
        rig.manager.start_pairing_with(profile(), NONCE);
        let (mut fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        let wrong = super::super::super::link::auth::encode_bytes(&[9; NONCE_LEN]);
        let err = fake.pair(&wrong).expect_err("wrong nonce");
        assert_eq!(code(err), ErrorCode::NotPaired);
        handle.join().unwrap();
        assert_eq!(rig.keys.len(), 0);
        assert!(rig.records.list().unwrap().is_empty());
        // The window survives one bad attempt.
        assert!(matches!(
            rig.manager.pairing_state(),
            PairingState::Waiting { .. }
        ));
    }

    #[test]
    fn an_expired_window_refuses_pairing() {
        let rig = rig();
        rig.manager.start_pairing_with(profile(), NONCE);
        rig.clock.fetch_add(PAIRING_TTL_MS + 1, Ordering::SeqCst);
        assert_eq!(rig.manager.pairing_state(), PairingState::Idle);
        let (mut fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        let err = fake.pair(&nonce_text()).expect_err("expired");
        assert_eq!(code(err), ErrorCode::NotPaired);
        handle.join().unwrap();
        assert_eq!(rig.keys.len(), 0);
    }

    #[test]
    fn no_pairing_window_means_not_paired() {
        let rig = rig();
        let (mut fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        let res = fake.hello_only(false).expect("an answer");
        assert_eq!(res.error.unwrap().code, ErrorCode::NotPaired);
        handle.join().unwrap();
    }

    #[test]
    fn a_failing_key_store_aborts_pairing_without_a_record() {
        let rig = rig();
        rig.keys.fail_writes(true);
        rig.manager.start_pairing_with(profile(), NONCE);
        let (mut fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        let err = fake.pair(&nonce_text()).expect_err("storage failed");
        assert_eq!(code(err), ErrorCode::Invalid);
        handle.join().unwrap();
        assert!(rig.records.list().unwrap().is_empty());
        rig.next(|e| {
            matches!(
                e,
                LinkEvent::Refused {
                    refusal: Refusal::Storage(_),
                    ..
                }
            )
        });
    }

    #[test]
    fn pairing_the_same_profile_again_replaces_the_old_install() {
        let rig = rig();
        rig.pair(INSTANCE);
        rig.pair(OTHER_INSTANCE);
        let ids: Vec<String> = rig
            .records
            .list()
            .unwrap()
            .into_iter()
            .map(|r| r.instance_id)
            .collect();
        assert_eq!(ids, vec![OTHER_INSTANCE.to_string()]);
        assert!(rig.keys.get(INSTANCE).unwrap().is_none());
        assert!(rig.keys.get(OTHER_INSTANCE).unwrap().is_some());
    }

    #[test]
    fn a_wrong_key_fails_authentication() {
        let rig = rig();
        rig.pair(INSTANCE);
        let (mut fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        // The app proves K first; a fake without K cannot even verify it.
        let err = fake.connect().expect_err("wrong key");
        assert_eq!(code(err), ErrorCode::Auth);
        drop(fake);
        handle.join().unwrap();
        assert!(rig.manager.handle(INSTANCE).is_none());
    }

    #[test]
    fn a_replayed_hello_challenge_is_refused() {
        let rig = rig();
        let key = rig.pair(INSTANCE);
        let challenge = [5u8; 32];
        let (mut first, h1) = rig.serve(INSTANCE, key.clone(), report());
        first.fix_challenge(challenge);
        first.connect().expect("first use");
        first.close();
        h1.join().unwrap();
        let (mut second, h2) = rig.serve(INSTANCE, key, report());
        second.fix_challenge(challenge);
        let err = second.connect().expect_err("replay");
        assert_eq!(code(err), ErrorCode::Auth);
        h2.join().unwrap();
    }

    #[test]
    fn a_second_link_with_the_same_instance_is_refused_and_alerts() {
        let rig = rig();
        let (first, h1) = rig.connected(INSTANCE);
        let key = rig.keys.get(INSTANCE).unwrap().unwrap();
        let (mut second, h2) = rig.serve(INSTANCE, key, report());
        let err = second.connect().expect_err("duplicate");
        assert_eq!(code(err), ErrorCode::Auth);
        h2.join().unwrap();
        match rig.next(|e| matches!(e, LinkEvent::Refused { .. })) {
            LinkEvent::Refused { refusal, .. } => {
                assert!(matches!(refusal, Refusal::Duplicate { .. }));
                assert_eq!(
                    refusal.message(),
                    "A second browser tried to connect as 'Work'."
                );
            }
            _ => unreachable!(),
        }
        // The first link is untouched.
        let handle = rig.manager.handle(INSTANCE).expect("still live");
        let t = std::thread::spawn(move || first.serve_echo());
        handle
            .request(Method::StatusGet, None, json!({}), WAIT)
            .expect("still answers");
        handle.close();
        t.join().unwrap();
        h1.join().unwrap();
    }

    fn refused_after_pairing(
        rep: PeerReport,
        tweak: impl FnOnce(&mut ManagerConfig),
    ) -> (Rig, ErrorCode, Refusal) {
        let rig = rig_with(Flavor::Release, tweak);
        let key = rig.pair(INSTANCE);
        let (mut fake, handle) = rig.serve(INSTANCE, key, rep);
        let err = fake.connect().expect_err("refused");
        handle.join().unwrap();
        let refusal = match rig.next(|e| matches!(e, LinkEvent::Refused { .. })) {
            LinkEvent::Refused { refusal, .. } => refusal,
            _ => unreachable!(),
        };
        (rig, err.code, refusal)
    }

    #[test]
    fn another_data_dir_is_refused() {
        let mut rep = report();
        rep.user_data_dir = Some(PathBuf::from("/tmp/copied-profile"));
        let (rig, code, refusal) = refused_after_pairing(rep, |_| {});
        assert_eq!(code, ErrorCode::Auth);
        assert!(matches!(refusal, Refusal::DataDirMismatch { .. }));
        assert!(rig.manager.handle(INSTANCE).is_none());
    }

    #[test]
    fn a_missing_data_dir_is_a_mismatch_too() {
        let mut rep = report();
        rep.user_data_dir = None;
        let (_rig, _code, refusal) = refused_after_pairing(rep, |_| {});
        assert!(matches!(refusal, Refusal::DataDirMismatch { .. }));
    }

    #[test]
    fn another_browser_executable_is_refused() {
        let mut rep = report();
        rep.parent_exe = Some(PathBuf::from("/opt/other/chrome"));
        let (_rig, _code, refusal) = refused_after_pairing(rep, |_| {});
        assert!(matches!(refusal, Refusal::BrowserMismatch { .. }));
    }

    #[test]
    fn without_a_readable_data_dir_the_handshake_brand_and_exe_decide() {
        // Windows: the host cannot read `--user-data-dir`, so only the brand and executable are
        // compared. A different data dir passes, a different executable still fails.
        let rig = rig_with(Flavor::Release, |c| c.check_data_dir = false);
        let key = rig.pair(INSTANCE);
        let mut rep = report();
        rep.user_data_dir = None;
        let (mut fake, handle) = rig.serve(INSTANCE, key.clone(), rep);
        fake.connect().expect("accepted without a data dir");
        fake.close();
        handle.join().unwrap();
        rig.next(|e| matches!(e, LinkEvent::Disconnected { .. }));

        let mut rep = report();
        rep.parent_exe = Some(PathBuf::from("/opt/other/chrome"));
        let (mut fake, handle) = rig.serve(INSTANCE, key.clone(), rep);
        assert_eq!(
            code(fake.connect().expect_err("exe differs")),
            ErrorCode::Auth
        );
        handle.join().unwrap();

        let (mut fake, handle) = rig.serve(INSTANCE, key, report());
        fake.brand = "Brave".to_string();
        assert_eq!(
            code(fake.connect().expect_err("brand differs")),
            ErrorCode::Auth
        );
        handle.join().unwrap();
    }

    #[test]
    fn an_unknown_extension_build_is_refused_with_the_update_message() {
        let rig = rig();
        let key = rig.pair(INSTANCE);
        let (mut fake, handle) = rig.serve(INSTANCE, key, report());
        fake.ext_version = "7.7.7".to_string();
        let err = fake.connect().expect_err("unknown build");
        assert_eq!(err.code, ErrorCode::Protocol);
        assert_eq!(
            err.message,
            "This extension version is not known to this zptah. Update zptah."
        );
        handle.join().unwrap();
        assert!(rig.manager.handle(INSTANCE).is_none());
    }

    #[test]
    fn native_dev_skips_the_known_build_list() {
        let rig = rig_with(Flavor::Dev, |_| {});
        let key = rig.pair(INSTANCE);
        let (mut fake, handle) = rig.serve(INSTANCE, key, report());
        fake.ext_version = "1.0.0-dev".to_string();
        fake.connect().expect("dev build accepted");
        fake.close();
        handle.join().unwrap();
    }

    #[test]
    fn a_protocol_range_without_overlap_gets_e_protocol_with_the_supported_range() {
        let rig = rig();
        let (mut fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        fake.tweak_hello(|p| p.protocol = [2, 3]);
        let res = fake.hello_only(true).expect("an answer");
        let error = res.error.expect("an error");
        assert_eq!(error.code, ErrorCode::Protocol);
        assert_eq!(error.detail, Some(json!({ "supported": [1, 1] })));
        handle.join().unwrap();
    }

    #[test]
    fn the_first_frame_must_be_hello() {
        let rig = rig();
        let (mut fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        let env = super::super::super::link::protocol::Envelope::request(
            "x-1",
            Method::Auth,
            json!({ "mac": "AAAA" }),
        );
        fake.send_raw(&env);
        let res = fake.recv_raw().expect("an answer");
        assert_eq!(res.error.unwrap().code, ErrorCode::Auth);
        handle.join().unwrap();
    }

    #[test]
    fn garbage_and_oversize_frames_close_the_connection() {
        let rig = rig();
        let (mut fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        fake.send_bytes(&[0xff, 0xff, 0xff, 0x7f]);
        assert!(fake.is_closed_by_app() || fake.recv_raw().is_some());
        handle.join().unwrap();

        let (mut fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        let mut junk = 5u32.to_le_bytes().to_vec();
        junk.extend_from_slice(b"nojsn");
        fake.send_bytes(&junk);
        let res = fake.recv_raw().expect("an error answer");
        assert_eq!(res.error.unwrap().code, ErrorCode::Invalid);
        handle.join().unwrap();
    }

    #[test]
    fn a_silent_peer_times_out() {
        let rig = rig_with(Flavor::Release, |c| {
            c.handshake_timeout = Duration::from_millis(150)
        });
        let (_fake, handle) = rig.serve(INSTANCE, PairingKey::generate(), report());
        handle.join().unwrap();
        rig.next(|e| {
            matches!(
                e,
                LinkEvent::Refused {
                    refusal: Refusal::Timeout,
                    ..
                }
            )
        });
    }

    #[test]
    fn requests_and_responses_cross_the_link_and_events_come_back() {
        let rig = rig();
        let (mut fake, handle) = rig.connected(INSTANCE);
        let live = rig.manager.handle(INSTANCE).expect("live");
        let mono = "0123456789abcdef0123456789abcdef";
        let asker = {
            let live = live.clone();
            std::thread::spawn(move || {
                live.request(
                    Method::TabsList,
                    Some(mono),
                    json!({ "deadline_ms": 1000 }),
                    WAIT,
                )
            })
        };
        let req = fake.next_request().expect("request arrives sealed");
        assert_eq!(req.method.as_deref(), Some("tabs.list"));
        assert_eq!(req.mono.as_deref(), Some(mono));
        assert!(req.id.starts_with("a-"));
        // seq 1 was the auth response, so the first request is seq 2.
        assert_eq!(req.seq, Some(2));
        fake.respond_ok(&req, json!({ "tabs": [] }));
        assert_eq!(asker.join().unwrap().unwrap(), json!({ "tabs": [] }));

        fake.send_event(Event::TabRemoved, json!({ "tab": 3 }));
        match rig.next(|e| matches!(e, LinkEvent::Extension { .. })) {
            LinkEvent::Extension {
                instance_id,
                envelope,
            } => {
                assert_eq!(instance_id, INSTANCE);
                assert_eq!(envelope.method.as_deref(), Some("tab.removed"));
            }
            _ => unreachable!(),
        }

        // An error answer comes back as a Remote error.
        let asker = {
            let live = live.clone();
            std::thread::spawn(move || live.request(Method::StatusGet, None, json!({}), WAIT))
        };
        let req = fake.next_request().unwrap();
        fake.respond_err(&req, &ProtocolError::new(ErrorCode::Busy, "busy"));
        match asker.join().unwrap() {
            Err(RequestError::Remote(body)) => assert_eq!(body.code, ErrorCode::Busy),
            other => panic!("expected a remote error, got {other:?}"),
        }
        live.close();
        handle.join().unwrap();
    }

    #[test]
    fn a_request_times_out_and_a_closed_link_fails_pending_requests() {
        let rig = rig();
        let (mut fake, handle) = rig.connected(INSTANCE);
        let live = rig.manager.handle(INSTANCE).unwrap();
        let err = live
            .request(
                Method::StatusGet,
                None,
                json!({}),
                Duration::from_millis(100),
            )
            .expect_err("nobody answers");
        assert!(matches!(err, RequestError::Timeout));
        let _ = fake.next_request();
        let asker = {
            let live = live.clone();
            std::thread::spawn(move || live.request(Method::StatusGet, None, json!({}), WAIT))
        };
        let _ = fake.next_request().expect("second request");
        fake.close();
        assert!(matches!(asker.join().unwrap(), Err(RequestError::Closed)));
        handle.join().unwrap();
        assert!(!live.is_open());
    }

    #[test]
    fn mono_requests_without_a_mono_and_oversize_requests_are_refused_locally() {
        let rig = rig();
        let (_fake, handle) = rig.connected(INSTANCE);
        let live = rig.manager.handle(INSTANCE).unwrap();
        let err = live
            .request(Method::PageSnapshot, None, json!({}), WAIT)
            .expect_err("needs a mono");
        assert!(matches!(err, RequestError::Invalid(_)));
        let big = "x".repeat(1024 * 1024);
        let err = live
            .request(Method::StatusGet, None, json!({ "pad": big }), WAIT)
            .expect_err("over 1 MB");
        assert!(matches!(err, RequestError::TooLarge));
        live.close();
        handle.join().unwrap();
    }

    #[test]
    fn a_forged_replayed_or_illegal_frame_closes_the_link() {
        use super::super::super::link::protocol::Envelope;
        // Unsealed (no seq, no MAC).
        let rig = rig();
        let (mut fake, handle) = rig.connected(INSTANCE);
        fake.send_raw(&Envelope::event("x-9", Event::Stop, json!({})));
        handle.join().unwrap();
        rig.next(|e| matches!(e, LinkEvent::Disconnected { .. }));

        // Replayed: the same sealed frame twice.
        let rig = self::rig();
        let (mut fake, handle) = rig.connected(INSTANCE);
        fake.send_event(Event::Stop, json!({}));
        let mut replay = Envelope::event("x-20", Event::Stop, json!({}));
        replay.seq = Some(1);
        replay.mac = Some("AAAA".into());
        fake.send_raw(&replay);
        handle.join().unwrap();

        // A request from the extension after auth.
        let rig = self::rig();
        let (mut fake, handle) = rig.connected(INSTANCE);
        fake.send_sealed(Envelope::request("x-30", Method::Hello, json!({})));
        handle.join().unwrap();
        rig.next(|e| matches!(e, LinkEvent::Disconnected { .. }));
        assert!(rig.manager.handle(INSTANCE).is_none());
    }

    #[test]
    fn disconnect_all_removes_keys_records_and_closes_links() {
        let rig = rig();
        let (mut fake, handle) = rig.connected(INSTANCE);
        rig.manager.disconnect_all().unwrap();
        assert!(fake.is_closed_by_app());
        handle.join().unwrap();
        assert_eq!(rig.keys.len(), 0);
        assert!(rig.records.list().unwrap().is_empty());
        assert!(rig.manager.links().unwrap().is_empty());
    }

    // ---- server over a real unix socket ----

    mod server {
        use super::super::super::server::{LinkServer, Verifier};
        use super::*;
        use crate::mono::browser::peer::{PeerError, PeerPolicy};
        use std::os::unix::net::UnixStream;

        fn dir() -> tempfile::TempDir {
            use std::os::unix::fs::PermissionsExt;
            let tmp = tempfile::Builder::new()
                .prefix("zl")
                .tempdir_in("/tmp")
                .unwrap();
            std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
            tmp
        }

        fn fake_verifier() -> Verifier {
            Arc::new(|_| Ok(report()))
        }

        #[test]
        fn a_verified_connection_is_served_over_the_socket() {
            let tmp = dir();
            let rig = rig_with(Flavor::Dev, |_| {});
            let key = rig.pair(INSTANCE);
            let server = LinkServer::start_with(
                rig.manager.clone(),
                tmp.path(),
                Flavor::Dev,
                fake_verifier(),
            )
            .expect("server starts");
            // The lock is exclusive across processes only (POSIX record locks); that is tested
            // with a helper process in `peer::tests`.
            let stream = UnixStream::connect(server.socket_path()).expect("connect");
            let mut fake = FakeExtension::new(stream, INSTANCE, key);
            fake.connect().expect("handshake over the socket");
            rig.next(|e| matches!(e, LinkEvent::Connected { .. }));
            let socket = server.socket_path().to_path_buf();
            fake.close();
            rig.next(|e| matches!(e, LinkEvent::Disconnected { .. }));
            drop(server);
            assert!(!socket.exists(), "the socket is removed on drop");
        }

        #[test]
        fn a_peer_that_fails_verification_gets_no_protocol_bytes() {
            let tmp = dir();
            let rig = rig_with(Flavor::Dev, |_| {});
            let deny: Verifier = Arc::new(|_| Err(PeerError::UserMismatch));
            let server =
                LinkServer::start_with(rig.manager.clone(), tmp.path(), Flavor::Dev, deny).unwrap();
            let stream = UnixStream::connect(server.socket_path()).unwrap();
            let mut fake = FakeExtension::new(stream, INSTANCE, PairingKey::generate());
            assert!(fake.is_closed_by_app());
            rig.next(|e| {
                matches!(
                    e,
                    LinkEvent::Refused {
                        refusal: Refusal::Peer(_),
                        ..
                    }
                )
            });
        }

        #[test]
        fn the_real_peer_check_refuses_a_process_whose_parent_is_not_a_browser() {
            let tmp = dir();
            let rig = rig_with(Flavor::Dev, |_| {});
            // Release policy: the test process is our own executable, but cargo is not a browser.
            let server = LinkServer::start(
                rig.manager.clone(),
                tmp.path(),
                Flavor::Dev,
                PeerPolicy::release(),
            )
            .unwrap();
            let stream = UnixStream::connect(server.socket_path()).unwrap();
            let mut fake = FakeExtension::new(stream, INSTANCE, PairingKey::generate());
            assert!(fake.is_closed_by_app());
            rig.next(|e| {
                matches!(
                    e,
                    LinkEvent::Refused {
                        refusal: Refusal::Peer(_),
                        ..
                    }
                )
            });
        }

        #[test]
        fn a_symlinked_socket_dir_is_refused() {
            let tmp = dir();
            let real = tmp.path().join("real");
            std::fs::create_dir(&real).unwrap();
            let link = tmp.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            let rig = rig_with(Flavor::Dev, |_| {});
            let res =
                LinkServer::start_with(rig.manager.clone(), &link, Flavor::Dev, fake_verifier());
            assert!(matches!(res, Err(PeerError::Symlink(_))));
        }

        use super::super::super::fake_extension::FakeExtension;
        use crate::mono::browser::link::auth::PairingKey;
    }
}

// ---- key stores ----

#[test]
fn the_fallback_store_uses_the_secondary_when_the_credential_store_fails() {
    use super::super::link::auth::PairingKey;
    let primary = Arc::new(MemoryKeyStore::new());
    let fallback = Arc::new(MemoryKeyStore::new());
    let both = FallbackKeyStore::new(primary.clone(), fallback.clone());
    let key = PairingKey::generate();

    primary.fail_writes(true);
    both.set(INSTANCE, &key).unwrap();
    assert_eq!(primary.len(), 0);
    assert_eq!(fallback.len(), 1);
    assert_eq!(
        both.get(INSTANCE).unwrap().unwrap().to_b64(),
        key.to_b64(),
        "reads find the fallback copy"
    );

    primary.fail_writes(false);
    let rotated = PairingKey::generate();
    both.set(INSTANCE, &rotated).unwrap();
    assert_eq!(fallback.len(), 0, "a stale fallback copy is dropped");
    assert_eq!(
        both.get(INSTANCE).unwrap().unwrap().to_b64(),
        rotated.to_b64()
    );

    both.delete(INSTANCE).unwrap();
    assert!(both.get(INSTANCE).unwrap().is_none());
}

#[test]
fn the_mono_secrets_store_round_trips_a_key_in_a_temp_database() {
    use super::super::link::auth::PairingKey;
    use super::super::MonoBrowserStore;
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("store.db");
    let conn = rusqlite::Connection::open(&db).unwrap();
    conn.execute_batch(super::super::BROWSER_TABLES).unwrap();
    drop(conn);
    let store = MonoSecretsKeyStore::new(MonoBrowserStore::at(db));
    let key = PairingKey::generate();
    assert!(store.get(INSTANCE).unwrap().is_none());
    store.set(INSTANCE, &key).unwrap();
    assert_eq!(store.get(INSTANCE).unwrap().unwrap().to_b64(), key.to_b64());
    store.delete(INSTANCE).unwrap();
    assert!(store.get(INSTANCE).unwrap().is_none());
}

#[test]
fn dev_and_release_use_different_credential_store_services() {
    assert_ne!(keyring_service(true), keyring_service(false));
}

#[test]
fn the_file_record_store_survives_a_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("nested").join("links.json");
    let record = LinkRecord {
        instance_id: INSTANCE.into(),
        name: "Work".into(),
        profile_dir: "Default".into(),
        brand: "Google Chrome".into(),
        parent_exe: None,
        user_data_dir: Some(PathBuf::from("/x")),
        paired_at_ms: 5,
    };
    FileRecordStore::new(path.clone()).save(&record).unwrap();
    let again = FileRecordStore::new(path.clone());
    assert_eq!(again.list().unwrap(), vec![record]);
    again.remove(INSTANCE).unwrap();
    assert!(again.list().unwrap().is_empty());
    std::fs::write(&path, b"not json").unwrap();
    assert!(
        again.list().is_err(),
        "a damaged file is an error, not an empty list"
    );
}

// ---- pairing helpers ----

#[test]
fn the_pair_url_encodes_base64_so_it_survives_a_url_fragment() {
    let url = pair_page_url("abcdefghijklmnopabcdefghijklmnop", "a+b/c==");
    assert_eq!(
        url,
        "chrome-extension://abcdefghijklmnopabcdefghijklmnop/src/pair/pair.html#n=a%2Bb%2Fc%3D%3D"
    );
    let args = open_in_profile_args(&profile(), &url);
    assert_eq!(args[0], "--profile-directory=Profile 1");
    assert_eq!(args[1], url);
}

#[test]
fn profile_choices_reject_paths_and_flags() {
    assert!(ProfileChoice::new("../x", "n").is_err());
    assert!(ProfileChoice::new("a/b", "n").is_err());
    assert!(ProfileChoice::new("--user-data-dir=/x", "n").is_err());
    assert!(ProfileChoice::new("", "n").is_err());
    assert_eq!(ProfileChoice::new("Default", "  ").unwrap().name, "Default");
}

// ---- profiles ----

#[test]
fn profile_names_come_from_local_state() {
    use super::profiles::{parse_local_state, read_profiles};
    let text = br#"{"profile":{"info_cache":{
        "Profile 1":{"name":"Work"},
        "Default":{"name":"Person 1"},
        "../evil":{"name":"x"},
        "Profile 2":{"name":"  "}}}}"#;
    let profiles = parse_local_state(text);
    let pairs: Vec<(&str, &str)> = profiles
        .iter()
        .map(|p| (p.dir.as_str(), p.name.as_str()))
        .collect();
    assert_eq!(
        pairs,
        vec![
            ("Default", "Person 1"),
            ("Profile 1", "Work"),
            ("Profile 2", "Profile 2")
        ]
    );
    assert!(parse_local_state(b"{}").is_empty());
    assert!(parse_local_state(b"nope").is_empty());
    let tmp = tempfile::tempdir().unwrap();
    assert!(
        read_profiles(tmp.path()).is_empty(),
        "no Local State is not an error"
    );
    std::fs::write(tmp.path().join("Local State"), text).unwrap();
    assert_eq!(read_profiles(tmp.path()).len(), 3);
}

// ---- known builds ----

mod builds {
    use super::super::known_builds::*;
    use super::*;

    fn builds(hash: Option<&'static str>) -> KnownBuilds {
        KnownBuilds::with(
            vec![KnownBuild {
                version: "1.0.0",
                sha256: hash,
            }],
            Flavor::Release,
        )
    }

    #[test]
    fn only_listed_versions_pass_and_native_dev_skips_the_list() {
        assert_eq!(builds(None).check("1.0.0", None), BuildVerdict::Accepted);
        assert_eq!(
            builds(None).check("1.0.1", None),
            BuildVerdict::UnknownVersion("1.0.1".into())
        );
        let dev = KnownBuilds::with(Vec::new(), Flavor::Dev);
        assert_eq!(dev.check("anything", None), BuildVerdict::Accepted);
        assert!(!KNOWN_BUILDS.is_empty());
    }

    fn write_extension(dir: &std::path::Path, key: &str, script: &str) {
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::create_dir_all(dir.join("_metadata")).unwrap();
        std::fs::write(
            dir.join("manifest.json"),
            format!(r#"{{"name":"x","version":"1.0.0","key":"{key}"}}"#),
        )
        .unwrap();
        std::fs::write(dir.join("src/main.js"), script).unwrap();
        std::fs::write(dir.join("_metadata/verified.json"), key).unwrap();
    }

    #[test]
    fn the_package_hash_ignores_the_key_and_metadata_but_not_code() {
        let tmp = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            tmp.path().join("a"),
            tmp.path().join("b"),
            tmp.path().join("c"),
        );
        write_extension(&a, "KEY1", "run();");
        write_extension(&b, "KEY2", "run();");
        write_extension(&c, "KEY1", "run(); steal();");
        let (ha, hb, hc) = (
            hash_extension_dir(&a).unwrap(),
            hash_extension_dir(&b).unwrap(),
            hash_extension_dir(&c).unwrap(),
        );
        assert_eq!(ha, hb);
        assert_ne!(ha, hc);
        assert_eq!(ha.len(), 64);
    }

    #[test]
    fn a_readable_folder_must_match_the_recorded_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("ext");
        write_extension(&dir, "K", "run();");
        let good: &'static str = Box::leak(hash_extension_dir(&dir).unwrap().into_boxed_str());
        assert_eq!(
            builds(Some(good)).check("1.0.0", Some(&dir)),
            BuildVerdict::Accepted
        );
        std::fs::write(dir.join("src/main.js"), "tampered();").unwrap();
        assert_eq!(
            builds(Some(good)).check("1.0.0", Some(&dir)),
            BuildVerdict::HashMismatch("1.0.0".into())
        );
        // No folder or an unreadable one falls back to the version check.
        assert_eq!(
            builds(Some(good)).check("1.0.0", None),
            BuildVerdict::Accepted
        );
        let gone = tmp.path().join("gone");
        assert_eq!(
            builds(Some(good)).check("1.0.0", Some(&gone)),
            BuildVerdict::Accepted
        );
    }

    #[test]
    fn the_installed_folder_is_found_by_version_and_the_newest_suffix_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().join("Profile 1/Extensions/someid");
        for name in ["1.0.0_0", "1.0.0_2", "1.0.0_10", "1.0.1_0", "1.0.0_x"] {
            std::fs::create_dir_all(base.join(name)).unwrap();
        }
        let found = installed_extension_dir(tmp.path(), "Profile 1", "someid", "1.0.0").unwrap();
        assert!(found.ends_with("1.0.0_10"));
        assert!(installed_extension_dir(tmp.path(), "Profile 1", "someid", "2.0.0").is_none());
        assert!(installed_extension_dir(tmp.path(), "../x", "someid", "1.0.0").is_none());
    }
}

// ---- manifest installer (temp dirs and a fake registry only) ----

mod install {
    use super::super::manifest_install::*;
    use super::*;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRegistry(Mutex<BTreeMap<String, String>>);

    impl Registry for FakeRegistry {
        fn set_default(&self, key: &str, value: &str) -> std::io::Result<()> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
        fn delete_key(&self, key: &str) -> std::io::Result<()> {
            self.0.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn env(os: Os, root: &std::path::Path) -> InstallEnv {
        InstallEnv {
            os,
            home: root.join("home"),
            local_app_data: Some(root.join("local")),
            app_data_dir: root.join("appdata"),
        }
    }

    fn exe() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Program Files\zptah\zptah.exe")
        } else {
            PathBuf::from("/Applications/zptah.app/Contents/MacOS/zptah")
        }
    }

    fn chrome_dir(env: &InstallEnv) -> PathBuf {
        match env.os {
            Os::Mac => env.home.join("Library/Application Support/Google/Chrome"),
            Os::Linux => env.home.join(".config/google-chrome"),
            Os::Windows => env
                .local_app_data
                .clone()
                .unwrap()
                .join(r"Google\Chrome\User Data"),
        }
    }

    fn read_manifest(path: &std::path::Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn the_release_manifest_lists_the_store_id_and_never_the_dev_id() {
        let text = manifest_json(Flavor::Release, &exe()).unwrap();
        assert!(text.contains("chrome-extension://pppppppppppppppppppppppppppppppp/"));
        assert!(!text.contains(DEV_EXTENSION_ID));
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["name"], "app.zptah.browser_host");
        assert_eq!(v["type"], "stdio");
        assert_eq!(v["path"], exe().to_str().unwrap());
    }

    #[test]
    fn the_dev_manifest_has_its_own_host_name_and_only_the_dev_id() {
        let v: Value = serde_json::from_str(&manifest_json(Flavor::Dev, &exe()).unwrap()).unwrap();
        assert_eq!(v["name"], "app.zptah.browser_host_dev");
        assert_eq!(
            v["allowed_origins"],
            json!([format!("chrome-extension://{DEV_EXTENSION_ID}/")])
        );
    }

    #[test]
    fn store_ids_are_valid_chrome_ids() {
        for id in STORE_EXTENSION_IDS.iter().chain([&DEV_EXTENSION_ID]) {
            assert_eq!(id.len(), 32);
            assert!(id.bytes().all(|b| (b'a'..=b'p').contains(&b)), "{id}");
        }
    }

    #[test]
    fn the_dev_id_and_host_names_match_the_extension_sources() {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let manifest: Value = serde_json::from_str(include_str!(
            "../../../../../../extensions/zptah-browser/manifest.dev.json"
        ))
        .unwrap();
        let der = base64::engine::general_purpose::STANDARD
            .decode(manifest["key"].as_str().unwrap())
            .unwrap();
        let id: String = Sha256::digest(&der)[..16]
            .iter()
            .flat_map(|b| [b >> 4, b & 15])
            .map(|n| (b'a' + n) as char)
            .collect();
        assert_eq!(id, DEV_EXTENSION_ID);
        let config =
            include_str!("../../../../../../extensions/zptah-browser/src/worker/config.js");
        assert!(config.contains(&format!("\"{HOST_NAME}\"")));
        let build = include_str!("../../../../../../extensions/zptah-browser/scripts/build.mjs");
        assert!(build.contains(&format!("\"{DEV_HOST_NAME}\"")));
    }

    #[test]
    fn relative_or_empty_executables_are_refused() {
        for bad in ["", "zptah", "./zptah", "../zptah"] {
            assert!(matches!(
                manifest_json(Flavor::Release, std::path::Path::new(bad)),
                Err(InstallError::BadExecutable(_))
            ));
        }
    }

    #[test]
    fn the_unix_tables_point_at_each_browsers_native_messaging_dir() {
        let root = PathBuf::from("/h");
        let mac = env(Os::Mac, &root);
        let linux = env(Os::Linux, &root);
        let at = |env: &InstallEnv, b| match target(env, b, Flavor::Release) {
            Some(Target::File(p)) => p,
            other => panic!("{other:?}"),
        };
        let cases = [
            (
                Browser::Chrome,
                "Library/Application Support/Google/Chrome",
                ".config/google-chrome",
            ),
            (
                Browser::ChromeBeta,
                "Library/Application Support/Google/Chrome Beta",
                ".config/google-chrome-beta",
            ),
            (
                Browser::ChromeCanary,
                "Library/Application Support/Google/Chrome Canary",
                ".config/google-chrome-unstable",
            ),
            (
                Browser::Chromium,
                "Library/Application Support/Chromium",
                ".config/chromium",
            ),
            (
                Browser::Edge,
                "Library/Application Support/Microsoft Edge",
                ".config/microsoft-edge",
            ),
            (
                Browser::Brave,
                "Library/Application Support/BraveSoftware/Brave-Browser",
                ".config/BraveSoftware/Brave-Browser",
            ),
        ];
        for (browser, m, l) in cases {
            assert_eq!(
                at(&mac, browser),
                root.join("home")
                    .join(m)
                    .join("NativeMessagingHosts/app.zptah.browser_host.json")
            );
            assert_eq!(
                at(&linux, browser),
                root.join("home")
                    .join(l)
                    .join("NativeMessagingHosts/app.zptah.browser_host.json")
            );
        }
        assert!(matches!(
            target(&mac, Browser::Arc, Flavor::Release),
            Some(Target::File(_))
        ));
        assert!(target(&linux, Browser::Arc, Flavor::Release).is_none());
    }

    #[test]
    fn the_windows_table_uses_hkcu_keys_and_a_manifest_in_the_app_data_dir() {
        let root = PathBuf::from(r"C:\u");
        let win = env(Os::Windows, &root);
        let key = |b| match target(&win, b, Flavor::Release) {
            Some(Target::Registry { key, file }) => (key, file),
            other => panic!("{other:?}"),
        };
        let (chrome, file) = key(Browser::Chrome);
        assert_eq!(
            chrome,
            r"Software\Google\Chrome\NativeMessagingHosts\app.zptah.browser_host"
        );
        assert_eq!(
            file,
            win.app_data_dir
                .join("browser-host")
                .join("app.zptah.browser_host.json")
        );
        assert_eq!(
            key(Browser::ChromeBeta).0,
            chrome,
            "Beta and Canary share the Chrome key"
        );
        assert_eq!(key(Browser::ChromeCanary).0, chrome);
        assert_eq!(
            key(Browser::Chromium).0,
            r"Software\Chromium\NativeMessagingHosts\app.zptah.browser_host"
        );
        assert_eq!(
            key(Browser::Edge).0,
            r"Software\Microsoft\Edge\NativeMessagingHosts\app.zptah.browser_host"
        );
        assert_eq!(
            key(Browser::Brave).0,
            r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts\app.zptah.browser_host"
        );
        assert!(target(&win, Browser::Arc, Flavor::Release).is_none());
        let dev = target(&win, Browser::Chrome, Flavor::Dev);
        assert!(
            matches!(dev, Some(Target::Registry { key, .. }) if key.ends_with("app.zptah.browser_host_dev"))
        );
    }

    #[test]
    fn install_writes_only_for_detected_browsers_and_is_idempotent() {
        for os in [Os::Mac, Os::Linux] {
            let tmp = tempfile::tempdir().unwrap();
            let env = env(os, tmp.path());
            let reg = FakeRegistry::default();
            // Nothing installed: nothing written.
            assert!(install(&env, Flavor::Release, &exe(), V1_BROWSERS, &reg)
                .unwrap()
                .is_empty());
            assert!(!env.home.exists());

            std::fs::create_dir_all(chrome_dir(&env)).unwrap();
            let done = install(&env, Flavor::Release, &exe(), V1_BROWSERS, &reg).unwrap();
            assert_eq!(done.len(), 1);
            assert_eq!(done[0].change, Change::Written);
            let Target::File(path) = &done[0].target else {
                panic!()
            };
            assert_eq!(read_manifest(path)["path"], exe().to_str().unwrap());

            let again = install(&env, Flavor::Release, &exe(), V1_BROWSERS, &reg).unwrap();
            assert_eq!(again[0].change, Change::Unchanged);

            // The app moved: the manifest is rewritten.
            let moved = PathBuf::from("/opt/zptah/zptah");
            let again = install(&env, Flavor::Release, &moved, V1_BROWSERS, &reg).unwrap();
            assert_eq!(again[0].change, Change::Written);
            assert_eq!(read_manifest(path)["path"], "/opt/zptah/zptah");

            // V1 never touches the other browsers, even when installed.
            let edge = env.home.join(match os {
                Os::Mac => "Library/Application Support/Microsoft Edge",
                _ => ".config/microsoft-edge",
            });
            std::fs::create_dir_all(&edge).unwrap();
            install(&env, Flavor::Release, &moved, V1_BROWSERS, &reg).unwrap();
            assert!(!edge.join("NativeMessagingHosts").exists());
            // A table-driven call with Edge does write it.
            let both = install(
                &env,
                Flavor::Release,
                &moved,
                &[Browser::Chrome, Browser::Edge],
                &reg,
            )
            .unwrap();
            assert_eq!(both.len(), 2);

            // Removing deletes our files and leaves siblings alone.
            let sibling = path.parent().unwrap().join("com.other.host.json");
            std::fs::write(&sibling, "{}").unwrap();
            remove(
                &env,
                Flavor::Release,
                &[Browser::Chrome, Browser::Edge],
                &reg,
            )
            .unwrap();
            assert!(!path.exists());
            assert!(sibling.exists());
        }
    }

    #[test]
    fn both_flavors_coexist_in_one_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(Os::Linux, tmp.path());
        std::fs::create_dir_all(chrome_dir(&env)).unwrap();
        let reg = FakeRegistry::default();
        install(&env, Flavor::Release, &exe(), V1_BROWSERS, &reg).unwrap();
        install(&env, Flavor::Dev, &exe(), V1_BROWSERS, &reg).unwrap();
        let hosts = chrome_dir(&env).join("NativeMessagingHosts");
        let release = read_manifest(&hosts.join("app.zptah.browser_host.json"));
        let dev = read_manifest(&hosts.join("app.zptah.browser_host_dev.json"));
        assert!(!release["allowed_origins"]
            .to_string()
            .contains(DEV_EXTENSION_ID));
        assert!(dev["allowed_origins"]
            .to_string()
            .contains(DEV_EXTENSION_ID));
        remove(&env, Flavor::Dev, V1_BROWSERS, &reg).unwrap();
        assert!(hosts.join("app.zptah.browser_host.json").exists());
        assert!(!hosts.join("app.zptah.browser_host_dev.json").exists());
    }

    #[test]
    fn windows_install_writes_the_file_and_the_registry_value_then_removes_both() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(Os::Windows, tmp.path());
        std::fs::create_dir_all(chrome_dir(&env)).unwrap();
        let reg = FakeRegistry::default();
        let exe = if cfg!(windows) {
            exe()
        } else {
            PathBuf::from("/c/zptah.exe")
        };
        let done = install(&env, Flavor::Release, &exe, V1_BROWSERS, &reg).unwrap();
        let Target::Registry { key, file } = &done[0].target else {
            panic!()
        };
        assert_eq!(
            reg.0.lock().unwrap().get(key).unwrap(),
            &file.to_string_lossy().to_string()
        );
        assert!(file.starts_with(tmp.path()));
        assert_eq!(read_manifest(file)["name"], "app.zptah.browser_host");
        remove(&env, Flavor::Release, V1_BROWSERS, &reg).unwrap();
        assert!(reg.0.lock().unwrap().is_empty());
        assert!(!file.exists());
    }

    #[test]
    fn a_symlinked_destination_is_never_written_through() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(Os::Linux, tmp.path());
        let hosts = chrome_dir(&env).join("NativeMessagingHosts");
        std::fs::create_dir_all(&hosts).unwrap();
        let victim = tmp.path().join("victim.txt");
        std::fs::write(&victim, "keep").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&victim, hosts.join("app.zptah.browser_host.json")).unwrap();
            let reg = FakeRegistry::default();
            let err = install(&env, Flavor::Release, &exe(), V1_BROWSERS, &reg).unwrap_err();
            assert!(matches!(err, InstallError::UnsafeDestination(_)));
            assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
            assert!(matches!(
                remove(&env, Flavor::Release, V1_BROWSERS, &reg),
                Err(InstallError::UnsafeDestination(_))
            ));
            assert!(victim.exists());
        }
    }

    #[test]
    fn foreign_manifests_are_reported_and_left_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let env = env(Os::Linux, tmp.path());
        let hosts = chrome_dir(&env).join("NativeMessagingHosts");
        std::fs::create_dir_all(&hosts).unwrap();
        let theirs = hosts.join("com.leeguoo.chrome_use.json");
        std::fs::write(&theirs, "{}").unwrap();
        assert_eq!(foreign_manifests(&env, V1_BROWSERS), vec![theirs.clone()]);
        let reg = FakeRegistry::default();
        remove(&env, Flavor::Release, V1_BROWSERS, &reg).unwrap();
        assert!(theirs.exists());
    }
}
