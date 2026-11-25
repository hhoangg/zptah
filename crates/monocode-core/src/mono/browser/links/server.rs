//! The listener (spec 5.4). The app takes the single-instance lock first and only then binds the
//! socket, so a second copy of the app cannot serve it. Every accepted connection is verified
//! (our own executable, started by a browser) before a byte is read, then handed to the manager on
//! its own thread.
//!
//! Windows serves a named pipe instead; that listener is not part of this ticket (the pipe helpers
//! are in `peer`), so [`LinkServer::start`] reports `Unsupported` there. When it is written, a
//! reader and a writer thread must not share one synchronous pipe handle (that deadlocks): serve
//! the pipe with overlapped I/O (`create_pipe_instance` already sets `FILE_FLAG_OVERLAPPED`).
use super::super::peer::{Flavor, PeerError, PeerPolicy, PeerReport};
use super::manager::LinkManager;

/// More unauthenticated handshakes than this at once are dropped.
const MAX_CONCURRENT: usize = 8;

#[cfg(unix)]
mod unix {
    use super::*;
    use crate::mono::browser::links::manager::Conn;
    use crate::mono::browser::peer::{self, dir, lock::InstanceLock};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
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
            let (manager, verifier, counter) = (manager.clone(), verifier.clone(), active.clone());
            let spawned = std::thread::Builder::new()
                .name("zptah-browser-link".into())
                .spawn(move || {
                    match verifier(&stream) {
                        Ok(report) => match Conn::from_unix(stream) {
                            Ok(conn) => manager.serve(conn, &report),
                            Err(e) => manager.report_peer_refused(&e),
                        },
                        Err(e) => manager.report_peer_refused(&e),
                    }
                    counter.fetch_sub(1, Ordering::SeqCst);
                });
            if spawned.is_err() {
                active.fetch_sub(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

#[cfg(unix)]
#[allow(unused_imports)]
pub(crate) use unix::{LinkServer, Verifier};

#[cfg(windows)]
pub(crate) struct LinkServer;

#[cfg(windows)]
impl LinkServer {
    pub(crate) fn start(
        _manager: LinkManager,
        _flavor: Flavor,
        _policy: PeerPolicy,
    ) -> Result<Self, PeerError> {
        Err(PeerError::Unsupported(
            "the Windows named pipe listener is not implemented yet",
        ))
    }
}
