use super::*;
use crate::mono::browser::link::auth::encode_bytes;
use crate::mono::browser::link::frame::encode_frame;
use crate::mono::browser::link::protocol::{EnvelopeType, Event};
use serde_json::json;

const STORE: &str = "abcdefghijklmnopabcdefghijklmnop";

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

fn origin(id: &str) -> String {
    format!("chrome-extension://{id}/")
}

fn parse(list: &[&str]) -> Option<Result<Launch, LaunchError>> {
    parse_launch_with(&args(list), &[STORE], true)
}

#[test]
fn chrome_argv_selects_host_mode_for_the_store_id() {
    let launch = parse(&[&origin(STORE)]).unwrap().unwrap();
    assert_eq!(launch.extension_id, STORE);
    assert_eq!(launch.flavor, Flavor::Release);
}

#[test]
fn the_dev_id_is_accepted_only_when_dev_is_allowed_and_uses_the_dev_flavor() {
    let dev = origin(DEV_EXTENSION_ID);
    let launch = parse(&[&dev]).unwrap().unwrap();
    assert_eq!(launch.flavor, Flavor::Dev);
    let strict = parse_launch_with(&args(&[&dev]), &[STORE], false).unwrap();
    assert_eq!(strict, Err(LaunchError::UnknownExtension));
}

#[test]
fn release_builds_compile_in_no_dev_id() {
    // `parse_launch` passes `cfg!(debug_assertions)`; the store list is empty until T18.
    let dev = origin(DEV_EXTENSION_ID);
    let result = parse_launch(&args(&[&dev])).unwrap();
    if cfg!(debug_assertions) {
        assert!(result.is_ok());
    } else {
        assert_eq!(result, Err(LaunchError::UnknownExtension));
    }
    assert!(!DEV_EXTENSION_ID.is_empty());
}

#[test]
fn an_explicit_subcommand_takes_the_same_origin() {
    let launch = parse(&["browser-host", &origin(STORE)]).unwrap().unwrap();
    assert_eq!(launch.extension_id, STORE);
    assert_eq!(
        parse(&["browser-host"]).unwrap(),
        Err(LaunchError::BadOrigin)
    );
}

#[test]
fn normal_starts_are_not_host_mode() {
    assert!(parse(&[]).is_none());
    assert!(parse(&["--version"]).is_none());
    assert!(parse(&["app-mcp"]).is_none());
    assert!(parse(&["https://example.com/"]).is_none());
}

#[test]
fn unknown_and_malformed_origins_are_refused() {
    let other = "bbcdefghijklmnopabcdefghijklmnop";
    assert_eq!(
        parse(&[&origin(other)]).unwrap(),
        Err(LaunchError::UnknownExtension)
    );
    for bad in [
        format!("chrome-extension://{STORE}"),
        format!("chrome-extension://{STORE}//"),
        "chrome-extension:///".to_string(),
        format!("chrome-extension://{}/", STORE.to_uppercase()),
        format!("chrome-extension://{}q/", &STORE[..31]),
        format!("chrome-extension://{}/", &STORE[..31]),
        format!("chrome-extension://{STORE}x/"),
    ] {
        assert_eq!(
            parse(&[&bad]).unwrap(),
            Err(LaunchError::BadOrigin),
            "{bad}"
        );
    }
}

#[test]
fn only_the_windows_parent_window_argument_may_follow_the_origin() {
    let o = origin(STORE);
    assert!(parse(&[&o, "--parent-window=1234"]).unwrap().is_ok());
    assert!(parse(&[&o, "--parent-window", "1234"]).unwrap().is_ok());
    for extra in [
        vec![o.as_str(), "--remote-debugging-port=1"],
        vec![o.as_str(), "--parent-window"],
        vec![o.as_str(), "--parent-window=abc"],
        vec![o.as_str(), "--parent-window="],
        vec![o.as_str(), "--parent-windows=1"],
        vec![o.as_str(), "stray"],
    ] {
        assert_eq!(
            parse(&extra).unwrap(),
            Err(LaunchError::UnexpectedArgument),
            "{extra:?}"
        );
    }
}

fn hello_with(protocol: [u32; 2], id: &str) -> Vec<u8> {
    Envelope::request(
        id,
        Method::Hello,
        json!({
            "protocol": protocol,
            "ext_version": "1.0.0",
            "browser": {"brand": "Chrome", "version": "158"},
            "instance_id": "a".repeat(32),
            "paired": false,
            "challenge": encode_bytes(&[7u8; 32]),
        }),
    )
    .to_vec()
}

fn framed(bytes: &[u8]) -> Vec<u8> {
    encode_frame(FrameFormat::HOST_FROM_EXTENSION, bytes).unwrap()
}

#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);
impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Sink {
    fn frames(&self) -> Vec<Envelope> {
        let bytes = self.0.lock().unwrap().clone();
        let mut cursor = std::io::Cursor::new(bytes);
        let mut out = Vec::new();
        while let Some(frame) = read_frame(&mut cursor, FrameFormat::HOST_TO_EXTENSION).unwrap() {
            out.push(Envelope::from_slice(&frame).unwrap());
        }
        out
    }
}

fn never_prepared() -> Result<(std::io::Empty, std::io::Sink), String> {
    panic!("the app must not be contacted")
}

#[test]
fn an_unsupported_protocol_is_answered_by_the_host_without_contacting_the_app() {
    let out = Sink::default();
    let input = std::io::Cursor::new(framed(&hello_with([2, 3], "x-1")));
    let end = run_session(input, out.clone(), never_prepared);
    assert_eq!(end, End::Rejected);
    let frames = out.frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].id, "x-1");
    assert_eq!(frames[0].kind, EnvelopeType::Res);
    assert_eq!(frames[0].error.as_ref().unwrap().code, ErrorCode::Protocol);
}

#[test]
fn the_first_frame_must_be_a_hello_from_the_extension() {
    for bytes in [
        Envelope::request("x-2", Method::Auth, json!({"mac": "x"})).to_vec(),
        Envelope::request("a-1", Method::Hello, json!({})).to_vec(),
        Envelope::event("x-3", Event::Stop, json!({})).to_vec(),
        b"{\"v\":1}".to_vec(),
        b"not json".to_vec(),
        b"[]".to_vec(),
    ] {
        let out = Sink::default();
        let end = run_session(
            std::io::Cursor::new(framed(&bytes)),
            out.clone(),
            never_prepared,
        );
        assert_eq!(end, End::Rejected);
        let frames = out.frames();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].ok, Some(false));
    }
}

#[test]
fn an_oversize_first_frame_is_refused_before_it_is_read() {
    let mut input = Vec::new();
    input.extend_from_slice(&(17u32 * 1024 * 1024).to_ne_bytes());
    let out = Sink::default();
    let end = run_session(std::io::Cursor::new(input), out.clone(), never_prepared);
    assert_eq!(end, End::Rejected);
    assert_eq!(
        out.frames()[0].error.as_ref().unwrap().code,
        ErrorCode::TooLarge
    );
}

#[test]
fn an_immediate_eof_ends_quietly_and_a_failed_verification_sends_nothing() {
    let out = Sink::default();
    let end = run_session(std::io::empty(), out.clone(), never_prepared);
    assert_eq!(end, End::ExtensionClosed);
    assert!(out.frames().is_empty());

    let hello = std::io::Cursor::new(framed(&hello_with([1, 1], "x-1")));
    let end = run_session(hello, out.clone(), || {
        Err::<(std::io::Empty, std::io::Sink), _>("parent is not an allowed browser".into())
    });
    assert!(matches!(end, End::Refused(_)));
    assert!(out.frames().is_empty());
}

#[cfg(unix)]
mod relay {
    use super::*;
    use crate::mono::browser::link::frame::MAX_HOST_TO_EXTENSION_FRAME;
    use std::os::unix::net::UnixStream;

    struct Rig {
        /// The test plays Chrome: writes to `chrome_tx`, reads from `chrome_rx`.
        chrome_tx: UnixStream,
        chrome_rx: UnixStream,
        /// The test plays the app.
        app: UnixStream,
        session: std::thread::JoinHandle<End>,
    }

    fn rig() -> Rig {
        let (chrome_tx, host_in) = UnixStream::pair().unwrap();
        let (host_out, chrome_rx) = UnixStream::pair().unwrap();
        let (host_app, app) = UnixStream::pair().unwrap();
        let host_app_out = host_app.try_clone().unwrap();
        let session = std::thread::spawn(move || {
            run_session(host_in, host_out, move || Ok((host_app, host_app_out)))
        });
        Rig {
            chrome_tx,
            chrome_rx,
            app,
            session,
        }
    }

    fn read_app(app: &mut UnixStream) -> Vec<u8> {
        read_frame(app, FrameFormat::SOCKET).unwrap().unwrap()
    }

    #[test]
    fn frames_are_relayed_unchanged_in_both_directions_until_chrome_closes() {
        let mut rig = rig();
        let hello = hello_with([1, 1], "x-1");
        rig.chrome_tx.write_all(&framed(&hello)).unwrap();
        assert_eq!(read_app(&mut rig.app), hello);

        // An extension event with extra, unknown members is relayed byte for byte.
        let event = br#"{"v":1,"type":"evt","id":"x-2","method":"stop","params":{"junk":[1,2]},"seq":9,"mac":"zz"}"#;
        rig.chrome_tx.write_all(&framed(event)).unwrap();
        assert_eq!(read_app(&mut rig.app), event.to_vec());

        let reply = Envelope::response_ok("x-1", json!({"protocol": 1})).to_vec();
        write_frame(&mut rig.app, FrameFormat::SOCKET, &reply).unwrap();
        let got = read_frame(&mut rig.chrome_rx, FrameFormat::HOST_TO_EXTENSION)
            .unwrap()
            .unwrap();
        assert_eq!(got, reply);

        drop(rig.chrome_tx);
        assert_eq!(rig.session.join().unwrap(), End::ExtensionClosed);
    }

    #[test]
    fn an_invalid_frame_from_the_extension_is_answered_and_not_forwarded() {
        let mut rig = rig();
        rig.chrome_tx
            .write_all(&framed(&hello_with([1, 1], "x-1")))
            .unwrap();
        read_app(&mut rig.app);
        // The app may not be asked to run a method only the app may call.
        let bad = Envelope::request("x-4", Method::TabsList, json!({})).to_vec();
        rig.chrome_tx.write_all(&framed(&bad)).unwrap();
        let reply = read_frame(&mut rig.chrome_rx, FrameFormat::HOST_TO_EXTENSION)
            .unwrap()
            .unwrap();
        let reply = Envelope::from_slice(&reply).unwrap();
        assert_eq!(reply.id, "x-4");
        assert_eq!(reply.error.unwrap().code, ErrorCode::Unsupported);
        assert_eq!(rig.session.join().unwrap(), End::BadFrame);
        rig.app.set_nonblocking(true).unwrap();
        assert!(matches!(
            read_frame(&mut rig.app, FrameFormat::SOCKET),
            Ok(None) | Err(_)
        ));
    }

    #[test]
    fn an_oversize_frame_from_the_app_closes_the_link_instead_of_reaching_chrome() {
        let mut rig = rig();
        rig.chrome_tx
            .write_all(&framed(&hello_with([1, 1], "x-1")))
            .unwrap();
        read_app(&mut rig.app);
        let big = Envelope::response_ok(
            "x-1",
            json!({"blob": "z".repeat(MAX_HOST_TO_EXTENSION_FRAME)}),
        )
        .to_vec();
        write_frame(&mut rig.app, FrameFormat::SOCKET, &big).unwrap();
        assert_eq!(rig.session.join().unwrap(), End::AppClosed);
        rig.chrome_rx.set_nonblocking(true).unwrap();
        assert!(read_frame(&mut rig.chrome_rx, FrameFormat::HOST_TO_EXTENSION).is_err());
    }

    #[test]
    fn a_malformed_frame_from_the_app_closes_the_link() {
        let mut rig = rig();
        rig.chrome_tx
            .write_all(&framed(&hello_with([1, 1], "x-1")))
            .unwrap();
        read_app(&mut rig.app);
        write_frame(&mut rig.app, FrameFormat::SOCKET, b"{\"v\":1}").unwrap();
        assert_eq!(rig.session.join().unwrap(), End::AppClosed);
    }

    #[test]
    fn the_app_closing_ends_the_session() {
        let mut rig = rig();
        rig.chrome_tx
            .write_all(&framed(&hello_with([1, 1], "x-1")))
            .unwrap();
        read_app(&mut rig.app);
        drop(rig.app);
        assert_eq!(rig.session.join().unwrap(), End::AppClosed);
    }
}

#[test]
fn the_error_reply_to_a_hidden_id_falls_back_to_x0() {
    assert_eq!(id_hint(br#"{"id":"x-12"}"#), "x-12");
    assert_eq!(id_hint(br#"{"id":"a-12"}"#), "x-0");
    assert_eq!(id_hint(br#"{"id":"x-1a"}"#), "x-0");
    assert_eq!(id_hint(b"garbage"), "x-0");
    assert_eq!(id_hint(br#"{"id":7}"#), "x-0");
}

#[cfg(windows)]
mod windows_relay {
    use super::*;
    use crate::mono::browser::peer::windows_pipe::test_pair;

    #[test]
    fn a_pending_read_does_not_block_a_write_on_the_same_pipe() {
        let (mut server, client) = test_pair();
        let mut reader = client.try_clone();
        let mut writer = client;
        let pending = std::thread::spawn(move || {
            let mut buf = [0u8; 5];
            reader.read_exact(&mut buf).unwrap();
            buf
        });
        // Give the reader time to be pending, then write from the same pipe.
        std::thread::sleep(std::time::Duration::from_millis(200));
        writer.write_all(b"hello").unwrap();
        let mut got = [0u8; 5];
        server.read_exact(&mut got).unwrap();
        assert_eq!(&got, b"hello");
        server.write_all(b"world").unwrap();
        assert_eq!(&pending.join().unwrap(), b"world");
    }

    #[test]
    fn two_extension_frames_reach_the_app_before_it_answers() {
        let (mut app, host) = test_pair();
        let host_out = host.try_clone();
        let mut input = framed(&hello_with([1, 1], "x-1"));
        input.extend(framed(
            &Envelope::event("x-2", Event::Stop, json!({})).to_vec(),
        ));
        let out = Sink::default();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let sink = out.clone();
        std::thread::spawn(move || {
            let end = run_session(std::io::Cursor::new(input), sink, move || {
                Ok((host, host_out))
            });
            let _ = done_tx.send(end);
        });
        for _ in 0..2 {
            let frame = read_frame(&mut app, FrameFormat::SOCKET).unwrap().unwrap();
            assert!(Envelope::from_slice(&frame).is_ok());
        }
        let reply = Envelope::response_ok("x-1", json!({"protocol": 1})).to_vec();
        write_frame(&mut app, FrameFormat::SOCKET, &reply).unwrap();
        assert!(done_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .is_ok());
    }
}
