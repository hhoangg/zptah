//! Browser-host mode (spec sections 4, 5.1, 5.3, 5.4, 12.1): the zptah binary started by Chrome as
//! a native messaging host. It speaks Chrome's stdio framing on one side and connects out to the
//! app's socket (or named pipe) on the other. It has no listener and holds no secrets or policy: it
//! checks that its own parent is a browser, that the app it reaches is our own executable and the
//! holder of the instance lock, then relays frames unchanged after checking their size and shape.
//!
//! Chrome starts it as `<exe> chrome-extension://<id>/` (on Windows with an extra `--parent-window`
//! argument, M0 finding F10). Only extension ids compiled into the binary are accepted: the store
//! ids, and the dev id in debug builds. An explicit `browser-host <origin>` subcommand does the
//! same for tests.
//!
//! The host never logs frame contents: stderr only ever gets a fixed reason or a peer-check error.
//! Nothing else may write to stdout, which belongs to the native messaging stream.
use super::link::frame::{read_frame, write_frame, FrameError, FrameFormat};
use super::link::protocol::{
    negotiate, Envelope, ErrorCode, HelloParams, Method, ProtocolError, Sender,
};
use super::peer::{self, Flavor, PeerPolicy};
use serde_json::Value;
use std::io::{Read, Write};
use std::sync::{mpsc, Arc, Mutex};

#[cfg(test)]
mod tests;

/// Extension id of the dev build (fixed by `manifest.dev.json` `key`). Accepted in debug builds only:
/// its public key is in the repo, so a release binary must never trust it.
pub const DEV_EXTENSION_ID: &str = "bmmfejkhajglhijajgdgfckhonddijjh";

/// Chrome Web Store extension ids. Filled in when the listing exists (ticket T18); until then a
/// release binary accepts no extension, which is the safe default.
pub const STORE_EXTENSION_IDS: &[&str] = &[];

const ORIGIN_PREFIX: &str = "chrome-extension://";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    pub(crate) extension_id: String,
    pub(crate) flavor: Flavor,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LaunchError {
    /// The origin is not `chrome-extension://<32 chars a-p>/`.
    BadOrigin,
    /// A well-formed id that is not on the compiled-in list.
    UnknownExtension,
    /// An argument other than the origin and Chrome's `--parent-window`.
    UnexpectedArgument,
}

impl std::fmt::Display for LaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            LaunchError::BadOrigin => "the caller origin is malformed",
            LaunchError::UnknownExtension => "the caller extension is not allowed",
            LaunchError::UnexpectedArgument => "unexpected argument",
        })
    }
}

/// Decides whether `args` (argv without the program name) ask for host mode. `None` means a normal
/// start. Otherwise the origin is validated against the compiled-in ids.
pub(crate) fn parse_launch(args: &[String]) -> Option<Result<Launch, LaunchError>> {
    parse_launch_with(args, STORE_EXTENSION_IDS, cfg!(debug_assertions))
}

fn parse_launch_with(
    args: &[String],
    store_ids: &[&str],
    allow_dev: bool,
) -> Option<Result<Launch, LaunchError>> {
    let first = args.first()?;
    let rest = if first == "browser-host" {
        &args[1..]
    } else if first.starts_with(ORIGIN_PREFIX) {
        args
    } else {
        return None;
    };
    Some(parse_origin_args(rest, store_ids, allow_dev))
}

fn parse_origin_args(
    args: &[String],
    store_ids: &[&str],
    allow_dev: bool,
) -> Result<Launch, LaunchError> {
    let origin = args.first().ok_or(LaunchError::BadOrigin)?;
    let mut extra = args[1..].iter();
    while let Some(arg) = extra.next() {
        // Windows: Chrome appends `--parent-window=<handle>` (or the flag and the handle apart).
        let ok = match arg.strip_prefix("--parent-window") {
            Some("") => {
                // The next argument must be the numeric handle.
                matches!(extra.next(), Some(v) if v.bytes().all(|b| b.is_ascii_digit()) && !v.is_empty())
            }
            Some(value) => value
                .strip_prefix('=')
                .is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())),
            None => false,
        };
        if !ok {
            return Err(LaunchError::UnexpectedArgument);
        }
    }
    let id = origin
        .strip_prefix(ORIGIN_PREFIX)
        .and_then(|rest| rest.strip_suffix('/'))
        .filter(|id| id.len() == 32 && id.bytes().all(|b| (b'a'..=b'p').contains(&b)))
        .ok_or(LaunchError::BadOrigin)?;
    if store_ids.contains(&id) {
        return Ok(Launch {
            extension_id: id.to_string(),
            flavor: Flavor::Release,
        });
    }
    if allow_dev && id == DEV_EXTENSION_ID {
        return Ok(Launch {
            extension_id: id.to_string(),
            flavor: Flavor::Dev,
        });
    }
    Err(LaunchError::UnknownExtension)
}

/// Entry point for `main`: `None` for a normal start, otherwise the process exit code.
pub fn maybe_run(args: &[String]) -> Option<i32> {
    let launch = match parse_launch(args)? {
        Ok(launch) => launch,
        Err(error) => {
            eprintln!("zptah-browser-host: {error}");
            return Some(2);
        }
    };
    Some(run(&launch))
}

fn policy_for(flavor: Flavor) -> PeerPolicy {
    #[cfg(any(test, debug_assertions))]
    if flavor == Flavor::Dev {
        return PeerPolicy::native_dev();
    }
    let _ = flavor;
    PeerPolicy::release()
}

fn run(launch: &Launch) -> i32 {
    // Hash our own image now: an updater may replace the file later and the peer checks compare
    // against the code that is actually running.
    let _ = peer::own_exe_hash();
    let policy = policy_for(launch.flavor);
    let flavor = launch.flavor;
    let end = run_session(std::io::stdin(), std::io::stdout(), move || {
        peer::inspect_own_parent(&policy).map_err(|e| e.to_string())?;
        connect_app(flavor, &policy)
    });
    match end {
        End::ExtensionClosed => 0,
        other => {
            eprintln!("zptah-browser-host: {other}");
            1
        }
    }
}

#[cfg(unix)]
fn connect_app(
    flavor: Flavor,
    policy: &PeerPolicy,
) -> Result<
    (
        std::os::unix::net::UnixStream,
        std::os::unix::net::UnixStream,
    ),
    String,
> {
    use peer::dir;
    let dir = dir::socket_dir().map_err(|e| e.to_string())?;
    dir::check_dir(&dir).map_err(|e| e.to_string())?;
    let path = dir::socket_path(&dir, flavor).map_err(|e| e.to_string())?;
    let sock = std::os::unix::net::UnixStream::connect(&path)
        .map_err(|e| format!("the app is not reachable: {e}"))?;
    peer::verify_app_peer(&sock, &dir, flavor, policy).map_err(|e| e.to_string())?;
    let writer = sock.try_clone().map_err(|e| e.to_string())?;
    Ok((sock, writer))
}

#[cfg(windows)]
fn connect_app(
    flavor: Flavor,
    policy: &PeerPolicy,
) -> Result<(std::fs::File, std::fs::File), String> {
    let name = peer::pipe_name(flavor).map_err(|e| e.to_string())?;
    let pipe = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&name)
        .map_err(|e| format!("the app is not reachable: {e}"))?;
    peer::verify_app_peer(&pipe, policy).map_err(|e| e.to_string())?;
    let writer = pipe.try_clone().map_err(|e| e.to_string())?;
    Ok((pipe, writer))
}

/// Why a session ended. Only `ExtensionClosed` is a normal exit.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum End {
    /// Chrome closed the port (EOF on stdin), the usual end.
    ExtensionClosed,
    /// The first frame was not an acceptable `hello`; an error went back to the extension.
    Rejected,
    /// The parent or the app failed verification, or the app is not running.
    Refused(String),
    /// The app closed the link or sent something invalid.
    AppClosed,
    /// A frame from the extension broke a rule; an error went back to the extension.
    BadFrame,
    /// A frame could not be written or read.
    Io,
}

impl std::fmt::Display for End {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            End::ExtensionClosed => f.write_str("the extension closed the port"),
            End::Rejected => f.write_str("the first message was refused"),
            End::Refused(reason) => write!(f, "refused: {reason}"),
            End::AppClosed => f.write_str("the app closed the link"),
            End::BadFrame => f.write_str("the extension sent an invalid frame"),
            End::Io => f.write_str("the link failed"),
        }
    }
}

type SharedOut<W> = Arc<Mutex<W>>;

fn send<W: Write>(out: &SharedOut<W>, format: FrameFormat, payload: &[u8]) -> Result<(), ()> {
    let mut out = out.lock().map_err(|_| ())?;
    write_frame(&mut *out, format, payload).map_err(|_| ())?;
    out.flush().map_err(|_| ())
}

/// The id of a request we could not parse, for the error reply; `x-0` when there is none.
fn id_hint(bytes: &[u8]) -> String {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|v| v.get("id")?.as_str().map(str::to_string))
        .filter(|id| {
            id.len() >= 3
                && id.len() <= 32
                && id.starts_with("x-")
                && id[2..].bytes().all(|b| b.is_ascii_digit())
        })
        .unwrap_or_else(|| "x-0".to_string())
}

fn reply_error<W: Write>(out: &SharedOut<W>, id: String, error: &ProtocolError) {
    let bytes = Envelope::response_err(id, error).to_vec();
    let _ = send(out, FrameFormat::HOST_TO_EXTENSION, &bytes);
}

/// Checks the first frame: an extension `hello` whose protocol range overlaps ours (spec 5.3).
fn check_hello(bytes: &[u8]) -> Result<(), ProtocolError> {
    let envelope = Envelope::from_slice(bytes)?;
    envelope.check_from(Sender::Extension)?;
    if envelope.method.as_deref() != Some(Method::Hello.as_str()) {
        return Err(ProtocolError::invalid("the first message must be hello"));
    }
    let hello = HelloParams::parse(&envelope.params)?;
    negotiate(hello.protocol)?;
    Ok(())
}

/// One host session. `prepare` verifies the parent and the app and returns the app's two stream
/// halves; it runs only after the extension's `hello` passed the protocol check, so a bad version
/// is answered without contacting the app.
pub(crate) fn run_session<CI, CO, AR, AW>(
    mut chrome_in: CI,
    chrome_out: CO,
    prepare: impl FnOnce() -> Result<(AR, AW), String>,
) -> End
where
    CI: Read + Send + 'static,
    CO: Write + Send + 'static,
    AR: Read + Send + 'static,
    AW: Write + Send + 'static,
{
    let chrome_out: SharedOut<CO> = Arc::new(Mutex::new(chrome_out));
    let hello = match read_frame(&mut chrome_in, FrameFormat::HOST_FROM_EXTENSION) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return End::ExtensionClosed,
        Err(FrameError::TooLarge { .. }) => {
            reply_error(
                &chrome_out,
                "x-0".into(),
                &ProtocolError::new(ErrorCode::TooLarge, "frame is too large"),
            );
            return End::Rejected;
        }
        Err(_) => return End::Io,
    };
    if let Err(error) = check_hello(&hello) {
        reply_error(&chrome_out, id_hint(&hello), &error);
        return End::Rejected;
    }
    let (mut app_in, app_out) = match prepare() {
        Ok(halves) => halves,
        Err(reason) => return End::Refused(reason),
    };
    let app_out: SharedOut<AW> = Arc::new(Mutex::new(app_out));
    if send(&app_out, FrameFormat::SOCKET, &hello).is_err() {
        return End::Io;
    }
    drop(hello);

    let (done_tx, done_rx) = mpsc::channel::<End>();
    let to_app = {
        let (chrome_out, app_out, done) = (chrome_out.clone(), app_out, done_tx.clone());
        std::thread::spawn(move || {
            let end = loop {
                let bytes = match read_frame(&mut chrome_in, FrameFormat::HOST_FROM_EXTENSION) {
                    Ok(Some(bytes)) => bytes,
                    Ok(None) => break End::ExtensionClosed,
                    Err(FrameError::TooLarge { .. }) => {
                        reply_error(
                            &chrome_out,
                            "x-0".into(),
                            &ProtocolError::new(ErrorCode::TooLarge, "frame is too large"),
                        );
                        break End::BadFrame;
                    }
                    Err(_) => break End::Io,
                };
                let checked =
                    Envelope::from_slice(&bytes).and_then(|e| e.check_from(Sender::Extension));
                if let Err(error) = checked {
                    reply_error(&chrome_out, id_hint(&bytes), &error);
                    break End::BadFrame;
                }
                if send(&app_out, FrameFormat::SOCKET, &bytes).is_err() {
                    break End::Io;
                }
            };
            let _ = done.send(end);
        })
    };
    drop(to_app);
    let _from_app = std::thread::spawn(move || {
        let end = loop {
            let bytes = match read_frame(&mut app_in, FrameFormat::SOCKET) {
                Ok(Some(bytes)) => bytes,
                Ok(None) | Err(_) => break End::AppClosed,
            };
            let checked = Envelope::from_slice(&bytes).and_then(|e| e.check_from(Sender::App));
            if checked.is_err() {
                break End::AppClosed;
            }
            // Over 1 MiB Chrome would drop the host; the app keeps frames under that, so a longer
            // one is a bug or an attack and the link closes.
            if send(&chrome_out, FrameFormat::HOST_TO_EXTENSION, &bytes).is_err() {
                break End::AppClosed;
            }
        };
        let _ = done_tx.send(end);
    });
    done_rx.recv().unwrap_or(End::Io)
}
