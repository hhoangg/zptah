//! The listener (spec 5.4). The app takes the single-instance lock first and only then binds the
//! socket, so a second copy of the app cannot serve it. Every accepted connection is verified
//! (our own executable, started by a browser) before a byte is read, then handed to the manager on
//! its own thread.
//!
//! Windows serves a named pipe instead (`windows` below). The first pipe instance, created with
//! `FILE_FLAG_FIRST_PIPE_INSTANCE`, is the single-instance guard that the lock file is on unix.
//! Every instance is overlapped and the accept loop keeps one instance listening at all times, so a
//! connection is never refused for lack of an instance.
use super::super::peer::{Flavor, PeerError, PeerPolicy};
use super::manager::LinkManager;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

/// More handshakes in progress at once than this are dropped. A link that finished its
/// handshake no longer counts.
const MAX_CONCURRENT: usize = 8;

/// One in-progress handshake in the counter, released exactly once.
struct HandshakeSlot(Arc<AtomicUsize>, AtomicBool);

impl HandshakeSlot {
    fn release(&self) {
        if !self.1.swap(true, Ordering::SeqCst) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
}

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::mono::browser::links::manager::Conn;
    use crate::mono::browser::peer::PeerReport;
    use crate::mono::browser::peer::{self, dir, lock::InstanceLock};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::thread::JoinHandle;
    use std::time::Duration;

    /// Verifies one accepted stream. Production uses `peer::verify_host_peer`; tests inject one.
    pub(crate) type Verifier =
        Arc<dyn Fn(&UnixStream) -> Result<PeerReport, PeerError> + Send + Sync>;

    /// The running listener. Dropping it stops accepting, removes the socket and releases the lock.
    pub(crate) struct LinkServer {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
        socket: PathBuf,
        _lock: InstanceLock,
    }

    impl LinkServer {
        pub(crate) fn start(
            manager: LinkManager,
            dir: &Path,
            flavor: Flavor,
            policy: PeerPolicy,
        ) -> Result<Self, PeerError> {
            let verifier: Verifier =
                Arc::new(move |stream| peer::verify_host_peer(stream, &policy));
            Self::start_with(manager, dir, flavor, verifier)
        }

        pub(crate) fn start_with(
            manager: LinkManager,
            dir: &Path,
            flavor: Flavor,
            verifier: Verifier,
        ) -> Result<Self, PeerError> {
            // Hash the running image before any connect: an updater may replace the file later.
            if peer::own_exe_hash().is_none() {
                return Err(PeerError::Unsupported("own executable cannot be read"));
            }
            dir::ensure_dir(dir)?;
            let lock = InstanceLock::acquire(dir, flavor)?;
            let socket = dir::socket_path(dir, flavor)?;
            dir::remove_stale_socket(&socket)?;
            let listener = UnixListener::bind(&socket)?;
            listener.set_nonblocking(true)?;
            let stop = Arc::new(AtomicBool::new(false));
            let thread = {
                let stop = stop.clone();
                std::thread::Builder::new()
                    .name("zptah-browser-accept".into())
                    .spawn(move || accept_loop(listener, manager, verifier, stop))?
            };
            Ok(Self {
                stop,
                thread: Some(thread),
                socket,
                _lock: lock,
            })
        }

        pub(crate) fn socket_path(&self) -> &Path {
            &self.socket
        }
    }

    impl Drop for LinkServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
            let _ = std::fs::remove_file(&self.socket);
        }
    }

    fn accept_loop(
        listener: UnixListener,
        manager: LinkManager,
        verifier: Verifier,
        stop: Arc<AtomicBool>,
    ) {
        let active = Arc::new(AtomicUsize::new(0));
        while !stop.load(Ordering::SeqCst) {
            let stream = match listener.accept() {
                Ok((stream, _)) => stream,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            };
            // An accepted socket can inherit the listener's non-blocking mode.
            if stream.set_nonblocking(false).is_err() {
                continue;
            }
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
                active.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let (manager, verifier) = (manager.clone(), verifier.clone());
            let slot = Arc::new(HandshakeSlot(active.clone(), AtomicBool::new(false)));
            let thread_slot = slot.clone();
            let spawned = std::thread::Builder::new()
                .name("zptah-browser-link".into())
                .spawn(move || {
                    match verifier(&stream) {
                        Ok(report) => match Conn::from_unix(stream) {
                            Ok(conn) => {
                                manager.serve_notify(conn, &report, || thread_slot.release())
                            }
                            Err(e) => manager.report_peer_refused(&e),
                        },
                        Err(e) => manager.report_peer_refused(&e),
                    }
                    thread_slot.release();
                });
            if spawned.is_err() {
                slot.release();
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[cfg(unix)]
#[allow(unused_imports)]
pub(crate) use unix::{LinkServer, Verifier};

#[cfg(windows)]
mod windows {
    use super::*;
    use crate::mono::browser::links::manager::Conn;
    use crate::mono::browser::peer::windows_pipe::OverlappedPipe;
    use crate::mono::browser::peer::{self, PeerReport};
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use std::ptr;
    use std::thread::JoinHandle;
    use std::time::Duration;
    use windows_sys::Win32::Foundation::{
        ERROR_IO_PENDING, ERROR_PIPE_CONNECTED, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Pipes::{ConnectNamedPipe, DisconnectNamedPipe};
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

    /// How often the accept loop looks at the stop flag while it waits for a client.
    const STOP_POLL_MS: u32 = 100;

    /// Verifies one accepted pipe. Production uses `peer::verify_host_peer`; tests inject one.
    pub(crate) type Verifier =
        Arc<dyn Fn(&OverlappedPipe) -> Result<PeerReport, PeerError> + Send + Sync>;

    /// The running listener. Dropping it stops accepting and closes the listening instance.
    pub(crate) struct LinkServer {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
        name: String,
    }

    impl LinkServer {
        pub(crate) fn start(
            manager: LinkManager,
            flavor: Flavor,
            policy: PeerPolicy,
        ) -> Result<Self, PeerError> {
            let name = peer::pipe_name(flavor)?;
            let verifier: Verifier = Arc::new(move |pipe| peer::verify_host_peer(pipe, &policy));
            Self::start_named(manager, &name, verifier)
        }

        /// Serves the pipe `name` (tests use a private one) and checks each client with `verifier`
        /// before a byte is read from it.
        pub(crate) fn start_named(
            manager: LinkManager,
            name: &str,
            verifier: Verifier,
        ) -> Result<Self, PeerError> {
            // Hash the running image before any connect: an updater may replace the file later.
            if peer::own_exe_hash().is_none() {
                return Err(PeerError::Unsupported("own executable cannot be read"));
            }
            // Fails when anyone else already owns the name: a second app, or a squatter.
            let first = peer::create_pipe_instance(name, true)?;
            let stop = Arc::new(AtomicBool::new(false));
            let thread = {
                let stop = stop.clone();
                let name = name.to_string();
                std::thread::Builder::new()
                    .name("zptah-browser-accept".into())
                    .spawn(move || accept_loop(first, name, manager, verifier, stop))?
            };
            Ok(Self {
                stop,
                thread: Some(thread),
                name: name.to_string(),
            })
        }

        pub(crate) fn pipe_name(&self) -> &str {
            &self.name
        }
    }

    impl Drop for LinkServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    /// Waits until a client connects to `pipe`. `Ok(false)` when `stop` was set first.
    fn wait_for_client(pipe: &OwnedHandle, stop: &AtomicBool) -> io::Result<bool> {
        let handle = pipe.as_raw_handle() as HANDLE;
        // SAFETY: null attributes and name; a manual-reset, initially unsignaled event.
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateEventW returned a new handle that nothing else owns; it closes on drop.
        let _event_guard = unsafe { OwnedHandle::from_raw_handle(event as RawHandle) };
        // SAFETY: an all-zero OVERLAPPED is valid.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event;
        // SAFETY: a live overlapped pipe handle; `overlapped` outlives the operation because every
        // path below that leaves it pending waits for it to finish first.
        if unsafe { ConnectNamedPipe(handle, &mut overlapped) } != 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        match error.raw_os_error() {
            // The client got in between the creation of the instance and this call.
            Some(code) if code == ERROR_PIPE_CONNECTED as i32 => return Ok(true),
            Some(code) if code == ERROR_IO_PENDING as i32 => {}
            _ => return Err(error),
        }
        let mut transferred = 0u32;
        loop {
            if stop.load(Ordering::SeqCst) {
                // SAFETY: `overlapped` identifies the pending connect; waiting for the result
                // keeps it alive until the cancel took effect.
                unsafe {
                    CancelIoEx(handle, &overlapped);
                    GetOverlappedResult(handle, &overlapped, &mut transferred, 1);
                }
                return Ok(false);
            }
            // SAFETY: a live event handle.
            if unsafe { WaitForSingleObject(event, STOP_POLL_MS) } == WAIT_OBJECT_0 {
                break;
            }
        }
        // SAFETY: `overlapped` is the structure the connect was started with and it completed.
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(true)
    }

    fn accept_loop(
        first: OwnedHandle,
        name: String,
        manager: LinkManager,
        verifier: Verifier,
        stop: Arc<AtomicBool>,
    ) {
        let active = Arc::new(AtomicUsize::new(0));
        let mut listening = first;
        while !stop.load(Ordering::SeqCst) {
            match wait_for_client(&listening, &stop) {
                Ok(true) => {}
                Ok(false) => break,
                Err(_) => {
                    // A client that came and went before the connect finished leaves the instance
                    // in a state that needs a disconnect before it can listen again.
                    // SAFETY: a live pipe handle.
                    unsafe { DisconnectNamedPipe(listening.as_raw_handle() as HANDLE) };
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
            }
            // Listen again before serving, so the next client never finds no instance.
            let connected = loop {
                match peer::create_pipe_instance(&name, false) {
                    Ok(next) => break std::mem::replace(&mut listening, next),
                    Err(_) if stop.load(Ordering::SeqCst) => return,
                    Err(_) => std::thread::sleep(Duration::from_millis(200)),
                }
            };
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONCURRENT {
                active.fetch_sub(1, Ordering::SeqCst);
                continue;
            }
            let (manager, verifier) = (manager.clone(), verifier.clone());
            let slot = Arc::new(HandshakeSlot(active.clone(), AtomicBool::new(false)));
            let thread_slot = slot.clone();
            let spawned = std::thread::Builder::new()
                .name("zptah-browser-link".into())
                .spawn(move || {
                    let pipe = OverlappedPipe::new(connected);
                    match verifier(&pipe) {
                        Ok(report) => manager
                            .serve_notify(Conn::from_pipe(pipe), &report, || thread_slot.release()),
                        Err(e) => manager.report_peer_refused(&e),
                    }
                    thread_slot.release();
                });
            if spawned.is_err() {
                slot.release();
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[cfg(windows)]
#[allow(unused_imports)]
pub(crate) use windows::{LinkServer, Verifier};
