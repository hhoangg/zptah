//! An in-process stand-in for the extension, for tests of the app side of the link (and of later
//! tickets that sit on top of it, such as the extension upstream). It speaks the real framing and
//! the real handshake through `link::auth::ExtHandshake`, over one end of a socket pair; the other
//! end is given to [`LinkManager::serve`](super::manager::LinkManager::serve).
//!
//! Every read has a timeout, so a broken test fails instead of hanging.
use super::super::link::auth::{Challenge, ExtHandshake, PairingKey, Session};
use super::super::link::frame::{read_frame, write_frame, FrameFormat};
use super::super::link::protocol::{
    AuthParams, AuthResult, Envelope, ErrorBody, Event, HelloParams, HelloResult, Method,
    PairBeginParams, PairBeginResult, ProtocolError,
};
use super::manager::Conn;
use serde_json::{json, Value};
use std::io;
use std::os::unix::net::UnixStream;
use std::time::Duration;

const READ_TIMEOUT: Duration = Duration::from_secs(5);

type HelloTweak = Box<dyn FnOnce(&mut HelloParams) + Send>;

pub(crate) struct FakeExtension {
    stream: UnixStream,
    pub instance_id: String,
    pub key: PairingKey,
    pub ext_version: String,
    pub brand: String,
    hello_tweak: Option<HelloTweak>,
    challenge: Option<Challenge>,
    session: Option<Session>,
    next_id: u64,
}

/// A connected pair: the app's [`Conn`] and the extension that talks to it.
pub(crate) fn socket_pair(instance_id: &str, key: PairingKey) -> (Conn, FakeExtension) {
    let (app, ext) = UnixStream::pair().expect("socket pair");
    (
        Conn::from_unix(app).expect("conn"),
        FakeExtension::new(ext, instance_id, key),
    )
}

impl FakeExtension {
    pub(crate) fn new(stream: UnixStream, instance_id: &str, key: PairingKey) -> Self {
        // macOS refuses to set a timeout (EINVAL) on a socket whose peer already closed, which
        // happens when the app refuses the connection before this runs. Reads on such a socket
        // return EOF at once, so there is nothing to time out; any other error is a real failure.
        match stream.set_read_timeout(Some(READ_TIMEOUT)) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::InvalidInput => {}
            Err(e) => panic!("read timeout: {e}"),
        }
        Self {
            stream,
            instance_id: instance_id.to_string(),
            key,
            ext_version: "1.0.0".to_string(),
            brand: "Google Chrome".to_string(),
            hello_tweak: None,
            challenge: None,
            session: None,
            next_id: 1,
        }
    }

    /// Changes the next `hello` before it is sent (a wrong protocol range, an unknown version).
    pub(crate) fn tweak_hello(&mut self, tweak: impl FnOnce(&mut HelloParams) + Send + 'static) {
        self.hello_tweak = Some(Box::new(tweak));
    }

    /// Uses this challenge in every `hello` (to replay one).
    pub(crate) fn fix_challenge(&mut self, challenge: Challenge) {
        self.challenge = Some(challenge);
    }

    fn id(&mut self) -> String {
        let id = format!("x-{}", self.next_id);
        self.next_id += 1;
        id
    }

    // ---- raw frames ----

    pub(crate) fn send_raw(&mut self, envelope: &Envelope) {
        write_frame(&mut self.stream, FrameFormat::SOCKET, &envelope.to_vec()).expect("write");
    }

    /// Writes bytes as they are (a broken length prefix, garbage).
    pub(crate) fn send_bytes(&mut self, bytes: &[u8]) {
        use std::io::Write;
        self.stream.write_all(bytes).expect("write");
    }

    /// The next frame from the app, unopened. `None` when the app closed the connection or
    /// nothing came within the timeout.
    pub(crate) fn recv_raw(&mut self) -> Option<Envelope> {
        match read_frame(&mut self.stream, FrameFormat::SOCKET) {
            Ok(Some(payload)) => {
                Some(Envelope::from_slice(&payload).expect("app sent an envelope"))
            }
            _ => None,
        }
    }

    /// True when the app closed its end (the next read is a clean end of stream).
    pub(crate) fn is_closed_by_app(&mut self) -> bool {
        matches!(
            read_frame(&mut self.stream, FrameFormat::SOCKET),
            Ok(None) | Err(_)
        )
    }

    // ---- handshake ----

    fn send_hello(&mut self, paired: bool) -> ExtHandshake {
        let mut hs = match self.challenge {
            Some(c) => ExtHandshake::with_challenge(self.key.clone(), self.instance_id.clone(), c),
            None => ExtHandshake::new(self.key.clone(), self.instance_id.clone()),
        };
        let mut params = hs.hello(paired, &self.ext_version, &self.brand);
        if let Some(tweak) = self.hello_tweak.take() {
            tweak(&mut params);
        }
        let id = self.id();
        self.send_raw(&Envelope::request(
            id,
            Method::Hello,
            serde_json::to_value(&params).expect("hello serializes"),
        ));
        hs
    }

    /// Sends `hello` and returns the app's answer unchanged (success or error).
    pub(crate) fn hello_only(&mut self, paired: bool) -> Option<Envelope> {
        self.send_hello(paired);
        self.recv_raw()
    }

    /// The full paired handshake. Returns the `auth` result, or the error the app answered with
    /// at either step. After `Ok` the session is open and [`Self::next_request`] works.
    pub(crate) fn connect(&mut self) -> Result<AuthResult, ProtocolError> {
        let mut hs = self.send_hello(true);
        let res = self.expect_response()?;
        let hello: HelloResult = serde_json::from_value(res).expect("hello result");
        let auth = hs
            .on_hello_result(&hello)
            .map_err(|e| e.to_protocol_error())?;
        let id = self.id();
        self.send_raw(&Envelope::request(
            id,
            Method::Auth,
            serde_json::to_value(AuthParams { mac: auth.mac }).expect("auth serializes"),
        ));
        let reply = self.recv_raw().ok_or_else(closed)?;
        let mut session = hs.into_session().map_err(|e| e.to_protocol_error())?;
        if reply.ok == Some(false) {
            return Err(error_of(reply));
        }
        // The auth response is the first sealed frame (app seq 1).
        session.open(&reply).map_err(|e| e.to_protocol_error())?;
        self.session = Some(session);
        Ok(serde_json::from_value(reply.result.expect("auth result")).expect("auth result shape"))
    }

    /// The pairing handshake: `hello` with `paired: false`, then `pair.begin`. Returns the key the
    /// app issued and adopts it as this extension's key.
    pub(crate) fn pair(&mut self, nonce: &str) -> Result<PairingKey, ProtocolError> {
        self.send_hello(false);
        self.expect_response()?;
        let id = self.id();
        let params = PairBeginParams {
            nonce: nonce.to_string(),
            instance_id: self.instance_id.clone(),
        };
        self.send_raw(&Envelope::request(
            id,
            Method::PairBegin,
            serde_json::to_value(params).expect("pair params serialize"),
        ));
        let result = self.expect_response()?;
        let result: PairBeginResult = serde_json::from_value(result).expect("pair result");
        let key = PairingKey::from_b64(result.key.expose()).expect("issued key");
        self.key = key.clone();
        Ok(key)
    }

    fn expect_response(&mut self) -> Result<Value, ProtocolError> {
        let res = self.recv_raw().ok_or_else(closed)?;
        if res.ok == Some(false) {
            return Err(error_of(res));
        }
        Ok(res.result.expect("a successful response has a result"))
    }

    // ---- after auth ----

    fn session(&mut self) -> &mut Session {
        self.session
            .as_mut()
            .expect("connect() must succeed before sealed traffic")
    }

    /// Sends an extension event (sealed).
    pub(crate) fn send_event(&mut self, event: Event, params: Value) {
        let id = self.id();
        let mut env = Envelope::event(id, event, params);
        self.session().seal(&mut env).expect("seal");
        self.send_raw(&env);
    }

    /// Sends any sealed envelope (to test what the app does with an illegal one).
    pub(crate) fn send_sealed(&mut self, mut envelope: Envelope) {
        self.session().seal(&mut envelope).expect("seal");
        self.send_raw(&envelope);
    }

    /// The next request from the app, MAC and counter checked. `None` on close or timeout.
    pub(crate) fn next_request(&mut self) -> Option<Envelope> {
        let env = self.recv_raw()?;
        self.session()
            .open(&env)
            .expect("app frame is sealed correctly");
        Some(env)
    }

    pub(crate) fn respond_ok(&mut self, request: &Envelope, result: Value) {
        let mut env = Envelope::response_ok(request.id.clone(), result);
        self.session().seal(&mut env).expect("seal");
        self.send_raw(&env);
    }

    pub(crate) fn respond_err(&mut self, request: &Envelope, error: &ProtocolError) {
        let mut env = Envelope::response_err(request.id.clone(), error);
        self.session().seal(&mut env).expect("seal");
        self.send_raw(&env);
    }

    /// Answers every request with `{"echo": <method>}` until the app closes. Run on a thread.
    pub(crate) fn serve_echo(mut self) {
        while let Some(req) = self.next_request() {
            let method = req.method.clone().unwrap_or_default();
            self.respond_ok(&req, json!({ "echo": method }));
        }
    }

    pub(crate) fn close(self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

fn closed() -> ProtocolError {
    ProtocolError::invalid(io::Error::from(io::ErrorKind::UnexpectedEof).to_string())
}

fn error_of(res: Envelope) -> ProtocolError {
    let ErrorBody {
        code,
        message,
        detail,
    } = res.error.expect("an error response carries an error");
    ProtocolError {
        code,
        message,
        detail,
    }
}
