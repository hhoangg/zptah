//! Socket location, single-instance lock and peer verification for the zptah Browser link
//! (spec section 5.4). The app serves a unix socket (macOS, Linux) or a named pipe (Windows);
//! the `zptah-browser-host` process connects to it. Both ends verify each other, so a Mono's
//! shell cannot pose as the host (to receive events) or as the app (to send commands).
//!
//! * [`dir`] (unix): the socket directory, created 0700 and refused when it is a symlink, owned by
//!   someone else or has another mode.
//! * [`lock`] (unix): the exclusive `browser.lock` the app holds for its whole life, and the
//!   `F_GETLK` query the host uses to learn which pid holds it.
//! * [`linux`], [`macos`], [`windows`]: the per-OS peer checks behind [`verify_host_peer`] (app
//!   side, the connecting process must be our own executable launched by a browser) and
//!   [`verify_app_peer`] (host side, the serving process must be our own executable and the
//!   lock holder).
//! * [`parse`]: pure parsers (proc stat, command lines) and the browser allowlists, so they are
//!   tested on every platform.
//!
//! Only a [`PeerPolicy::native_dev`] policy relaxes anything, only in debug builds, and only the code
//! signature check on macOS (replaced by the same-executable check). Release code uses [`PeerPolicy::release`].
//! This module does the checks and exposes no listener: binding the socket, authentication and
//! framing belong to the link layer.
//!
//! Known limits, accepted in spec 5.4: the parent checks read the parent by pid, so a tiny pid-reuse
//! window remains; the executable check is made at connect time, so a process that connects and
//! then replaces its own image is not caught; and none of this stops same-user code that debugs or
//! injects into the signed app (spec 5.4a).
#![allow(dead_code)] // consumed by the host mode and the link manager (spec tickets T5, T7)

use std::fmt;
use std::path::PathBuf;

pub(crate) mod parse;

#[cfg(unix)]
pub(crate) mod dir;
#[cfg(unix)]
pub(crate) mod lock;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[cfg(target_os = "linux")]
use linux as os;
#[cfg(target_os = "macos")]
use macos as os;
#[cfg(windows)]
use windows as os;

#[cfg(windows)]
#[allow(unused_imports)]
pub(crate) use os::{
    create_pipe_instance, inspect_own_parent, pipe_name, verify_app_peer, verify_host_peer,
};
#[cfg(unix)]
#[allow(unused_imports)]
pub(crate) use os::{inspect_own_parent, verify_app_peer, verify_host_peer};

#[cfg(test)]
mod tests;

/// Which app flavor a socket belongs to. Native Dev uses its own socket and lock so a dev app never
/// talks to the store extension (spec 5.4, 12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    Release,
    Dev,
}

impl Flavor {
    pub(crate) fn socket_file(self) -> &'static str {
        match self {
            Flavor::Release => "browser.sock",
            Flavor::Dev => "browser-dev.sock",
        }
    }

    pub(crate) fn lock_file(self) -> &'static str {
        match self {
            Flavor::Release => "browser.lock",
            Flavor::Dev => "browser-dev.lock",
        }
    }
}

/// How strict the peer checks are. Release code must pass [`PeerPolicy::release`]. The relaxed
/// dev policy only exists in debug builds (the `native-dev` profile inherits `dev`, so it has
/// `debug_assertions`), and even a hand-built relaxed policy is ignored by [`PeerPolicy::relaxed`]
/// in a release build.
#[derive(Debug, Clone, Default)]
pub(crate) struct PeerPolicy {
    relax_signature: bool,
    extra_browser_exes: Vec<PathBuf>,
}

/// Relaxing is allowed only in test and debug builds.
const RELAX_ENABLED: bool = cfg!(any(test, debug_assertions));

fn relax_allowed(requested: bool, enabled: bool) -> bool {
    requested && enabled
}

impl PeerPolicy {
    pub(crate) fn release() -> Self {
        Self::default()
    }

    /// Replace the macOS code-signature check with the same-executable check (unsigned dev builds).
    #[cfg(any(test, debug_assertions))]
    pub(crate) fn native_dev() -> Self {
        Self {
            relax_signature: true,
            extra_browser_exes: Vec::new(),
        }
    }

    /// Also accept this browser executable as the host's parent (tests with Chrome for Testing).
    /// Only honored by a relaxed policy.
    #[cfg(any(test, debug_assertions))]
    pub(crate) fn with_extra_browser(mut self, exe: PathBuf) -> Self {
        self.extra_browser_exes.push(exe);
        self
    }

    /// Whether the dev relaxation is in force. Always false in a release build.
    pub(crate) fn relaxed(&self) -> bool {
        relax_allowed(self.relax_signature, RELAX_ENABLED)
    }

    /// Extra accepted browser executables, empty unless [`Self::relaxed`].
    pub(crate) fn extra_browsers(&self) -> &[PathBuf] {
        if self.relaxed() {
            &self.extra_browser_exes
        } else {
            &[]
        }
    }
}

/// SHA-256 of a file, streamed.
pub(crate) fn sha256_file(path: &std::path::Path) -> std::io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

static OWN_EXE_HASH: std::sync::OnceLock<Option<[u8; 32]>> = std::sync::OnceLock::new();

/// Hash of this process's own executable, computed once. Call it at startup: an updater may replace
/// the file on disk later, and the hash must describe the code that is running. `None` when the
/// file cannot be read (every hash comparison then fails closed).
pub(crate) fn own_exe_hash() -> Option<[u8; 32]> {
    *OWN_EXE_HASH.get_or_init(|| {
        let path = if cfg!(target_os = "linux") {
            // Reads the running image even after the file was replaced or deleted.
            PathBuf::from("/proc/self/exe")
        } else {
            std::env::current_exe().ok()?
        };
        sha256_file(&path).ok()
    })
}

/// What the app observed about a verified connecting host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PeerReport {
    pub pid: u32,
    pub parent_pid: Option<u32>,
    pub parent_exe: Option<PathBuf>,
    /// `--user-data-dir` of the parent browser, when it was started with one and the OS lets us
    /// read it (not available on Windows yet).
    pub user_data_dir: Option<PathBuf>,
}

/// Why a path, lock or peer was refused. Each variant is a security decision: callers log it and
/// close the connection, they never retry around it.
#[derive(Debug)]
pub(crate) enum PeerError {
    Io(std::io::Error),
    /// The socket path would not fit `sun_path`.
    PathTooLong(PathBuf),
    NotDirectory(PathBuf),
    /// A file that must be a unix socket (a stale socket) is something else.
    NotSocket(PathBuf),
    /// A file that must be a regular file (the lock file) is something else.
    NotRegularFile(PathBuf),
    Symlink(PathBuf),
    WrongOwner {
        path: PathBuf,
        owner: u32,
        expected: u32,
    },
    WrongMode {
        path: PathBuf,
        mode: u32,
    },
    /// Another process holds the instance lock (pid when the OS told us).
    LockHeld(Option<u32>),
    /// The lock holder is not the process on the other end of the socket.
    LockHolderMismatch {
        holder: Option<u32>,
        peer: u32,
    },
    UidMismatch {
        peer: u32,
        expected: u32,
    },
    /// The peer process runs as a different Windows user.
    UserMismatch,
    /// The pid went away or was reused while we looked at it.
    PidReused(u32),
    /// The peer is not the same executable (file identity, or hash on Windows) as ours.
    ExeMismatch(PathBuf),
    /// The peer failed the code-signature check against our own designated requirement.
    SignatureInvalid(String),
    /// Our own code has no designated requirement (unsigned build under a release policy).
    SelfUnsigned(String),
    /// The peer's parent is not an allowed browser.
    ParentNotBrowser(Option<PathBuf>),
    Unsupported(&'static str),
}

impl fmt::Display for PeerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PeerError::Io(e) => write!(f, "io error: {e}"),
            PeerError::PathTooLong(p) => write!(f, "socket path too long: {}", p.display()),
            PeerError::NotDirectory(p) => write!(f, "not a directory: {}", p.display()),
            PeerError::NotSocket(p) => write!(f, "not a socket: {}", p.display()),
            PeerError::NotRegularFile(p) => write!(f, "not a regular file: {}", p.display()),
            PeerError::Symlink(p) => write!(f, "refusing symlink: {}", p.display()),
            PeerError::WrongOwner {
                path,
                owner,
                expected,
            } => write!(
                f,
                "{} is owned by uid {owner}, expected {expected}",
                path.display()
            ),
            PeerError::WrongMode { path, mode } => {
                write!(f, "{} has mode {mode:o}, expected 700", path.display())
            }
            PeerError::LockHeld(pid) => match pid {
                Some(pid) => write!(f, "another instance (pid {pid}) holds the browser lock"),
                None => write!(f, "another instance holds the browser lock"),
            },
            PeerError::LockHolderMismatch { holder, peer } => {
                write!(f, "lock holder {holder:?} is not the socket peer {peer}")
            }
            PeerError::UidMismatch { peer, expected } => {
                write!(f, "peer uid {peer} is not ours ({expected})")
            }
            PeerError::UserMismatch => write!(f, "peer runs as a different user"),
            PeerError::PidReused(pid) => write!(f, "pid {pid} changed while it was verified"),
            PeerError::ExeMismatch(p) => {
                write!(f, "peer executable is not ours: {}", p.display())
            }
            PeerError::SignatureInvalid(e) => write!(f, "peer signature check failed: {e}"),
            PeerError::SelfUnsigned(e) => write!(f, "own code has no designated requirement: {e}"),
            PeerError::ParentNotBrowser(p) => match p {
                Some(p) => write!(f, "parent is not an allowed browser: {}", p.display()),
                None => write!(f, "parent process could not be identified"),
            },
            PeerError::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

impl std::error::Error for PeerError {}

impl From<std::io::Error> for PeerError {
    fn from(e: std::io::Error) -> Self {
        PeerError::Io(e)
    }
}
