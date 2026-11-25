//! The single-instance lock (spec 5.4). The app holds an exclusive POSIX lock (`fcntl F_SETLK`) on
//! `browser.lock` for its whole life and only then binds the socket. The host asks `F_GETLK` for
//! the holder's pid and requires it to equal the socket peer's pid.
//!
//! POSIX record locks belong to the process, not the descriptor: closing ANY descriptor for the
//! file in the lock holder drops the lock. The app must therefore keep [`InstanceLock`] alive and
//! never open `browser.lock` anywhere else (including [`lock_holder_pid`]) in its own process.
//! The host is a separate process, so it may call [`lock_holder_pid`] freely.
use super::{dir, Flavor, PeerError};
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

/// The held lock. Dropping it (or exiting) releases the lock.
#[derive(Debug)]
pub(crate) struct InstanceLock {
    _file: File,
    path: PathBuf,
}

impl InstanceLock {
    /// Take the exclusive lock, or fail with [`PeerError::LockHeld`] carrying the holder's pid.
    /// `dir` must already pass [`dir::check_dir`].
    pub(crate) fn acquire(dir: &Path, flavor: Flavor) -> Result<Self, PeerError> {
        let path = dir.join(flavor.lock_file());
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&path)?;
        check_lock_file(&file, &path)?;
        let mut fl = new_flock(libc::F_WRLCK as _);
        // SAFETY: `fl` is a valid flock and the fd is open for the duration of the call.
        let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLK, &mut fl) };
        if rc == 0 {
            return Ok(Self { _file: file, path });
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EACCES) | Some(libc::EAGAIN) => {
                Err(PeerError::LockHeld(query_holder(&file).ok().flatten()))
            }
            _ => Err(err.into()),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

/// The pid of the process holding the lock, `None` when nobody holds it. Never creates the file.
/// Must be called from a process other than the holder (see the module docs).
pub(crate) fn lock_holder_pid(dir: &Path, flavor: Flavor) -> Result<Option<u32>, PeerError> {
    let path = dir.join(flavor.lock_file());
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)?;
    check_lock_file(&file, &path)?;
    Ok(query_holder(&file)?)
}

fn check_lock_file(file: &File, path: &Path) -> Result<(), PeerError> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(PeerError::NotDirectory(path.to_path_buf()));
    }
    let uid = dir::current_uid();
    if meta.uid() != uid {
        return Err(PeerError::WrongOwner {
            path: path.to_path_buf(),
            owner: meta.uid(),
            expected: uid,
        });
    }
    Ok(())
}

fn new_flock(kind: i32) -> libc::flock {
    // SAFETY: flock is plain data and all-zero is a valid value.
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = kind as _;
    fl.l_whence = libc::SEEK_SET as _;
    fl.l_start = 0;
    fl.l_len = 0;
    fl
}

/// Ask who would block a read lock (that is, who holds the write lock).
fn query_holder(file: &File) -> io::Result<Option<u32>> {
    let mut fl = new_flock(libc::F_RDLCK as _);
    // SAFETY: `fl` is a valid flock and the fd is open for the duration of the call.
    let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETLK, &mut fl) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if i64::from(fl.l_type) == i64::from(libc::F_UNLCK) {
        return Ok(None);
    }
    Ok(u32::try_from(fl.l_pid).ok())
}
