//! Linux peer checks (spec 5.4): `SO_PEERCRED` uid, `SO_PEERPIDFD` (Linux 6.5+, else pid plus start
//! time from `/proc/<pid>/stat`) against pid reuse, and `/proc/<pid>/exe` being the same file
//! (device and inode) as our own executable.
use super::dir::current_uid;
use super::lock::lock_holder_pid;
use super::parse::{is_chrome_linux_exe, parse_proc_stat, user_data_dir_from_cmdline};
use super::{Flavor, PeerError, PeerPolicy, PeerReport};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

/// `SO_PEERPIDFD` is not exported by every libc version.
const SO_PEERPIDFD: libc::c_int = 77;

struct Identified {
    pid: u32,
}

fn peer_cred(fd: RawFd) -> io::Result<libc::ucred> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are valid for the call and `len` is its exact size.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut cred as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred)
}

/// A pidfd for the peer, `None` on kernels without `SO_PEERPIDFD`.
fn peer_pidfd(fd: RawFd) -> Option<OwnedFd> {
    let mut pidfd: libc::c_int = -1;
    let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `pidfd` and `len` are valid for the call and `len` is its exact size.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            SO_PEERPIDFD,
            (&mut pidfd as *mut libc::c_int).cast(),
            &mut len,
        )
    };
    if rc != 0 || pidfd < 0 {
        return None;
    }
    // SAFETY: the kernel returned a new descriptor that nothing else owns.
    Some(unsafe { OwnedFd::from_raw_fd(pidfd) })
}

/// The pid a pidfd refers to, read from `/proc/self/fdinfo`. `None` when the process is gone.
fn pidfd_pid(pidfd: &OwnedFd) -> Option<u32> {
    let info = std::fs::read_to_string(format!("/proc/self/fdinfo/{}", pidfd.as_raw_fd())).ok()?;
    let pid: i64 = info
        .lines()
        .find_map(|l| l.strip_prefix("Pid:"))?
        .trim()
        .parse()
        .ok()?;
    u32::try_from(pid).ok()
}

fn proc_stat(pid: u32) -> io::Result<(u32, u64)> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    parse_proc_stat(&text).ok_or_else(|| io::Error::other("unparsable /proc stat"))
}

fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    let (a, b) = (std::fs::metadata(a)?, std::fs::metadata(b)?);
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

/// The peer is our uid, a live process and the same executable file as this process.
fn identify_same_exe_peer(fd: RawFd) -> Result<Identified, PeerError> {
    let cred = peer_cred(fd)?;
    let uid = current_uid();
    if cred.uid != uid {
        return Err(PeerError::UidMismatch {
            peer: cred.uid,
            expected: uid,
        });
    }
    let pid = u32::try_from(cred.pid).map_err(|_| PeerError::PidReused(0))?;
    let pidfd = peer_pidfd(fd);
    if let Some(pidfd) = &pidfd {
        if pidfd_pid(pidfd) != Some(pid) {
            return Err(PeerError::PidReused(pid));
        }
    }
    let (_, start_before) = proc_stat(pid)?;
    let exe = PathBuf::from(format!("/proc/{pid}/exe"));
    if !same_file(&exe, Path::new("/proc/self/exe"))? {
        return Err(PeerError::ExeMismatch(
            std::fs::read_link(&exe).unwrap_or(exe),
        ));
    }
    let (_, start_after) = proc_stat(pid)?;
    if start_before != start_after || pidfd.as_ref().is_some_and(|p| pidfd_pid(p).is_none()) {
        return Err(PeerError::PidReused(pid));
    }
    Ok(Identified { pid })
}

fn parent_report(pid: u32, policy: &PeerPolicy) -> Result<PeerReport, PeerError> {
    let (ppid, _) = proc_stat(pid)?;
    let parent_exe = std::fs::read_link(format!("/proc/{ppid}/exe")).ok();
    let allowed = parent_exe.as_deref().is_some_and(|exe| {
        is_chrome_linux_exe(exe)
            || (policy.relax_signature && policy.extra_browser_exes.iter().any(|e| e == exe))
    });
    if !allowed {
        return Err(PeerError::ParentNotBrowser(parent_exe));
    }
    let user_data_dir = std::fs::read(format!("/proc/{ppid}/cmdline"))
        .ok()
        .and_then(|raw| user_data_dir_from_cmdline(&raw));
    Ok(PeerReport {
        pid,
        parent_pid: Some(ppid),
        parent_exe,
        user_data_dir,
    })
}

/// App side: the connecting process must be our own executable, started by an allowed browser.
pub(crate) fn verify_host_peer(
    sock: &impl AsRawFd,
    policy: &PeerPolicy,
) -> Result<PeerReport, PeerError> {
    let id = identify_same_exe_peer(sock.as_raw_fd())?;
    parent_report(id.pid, policy)
}

/// Host side: the serving process must be our own executable and the holder of the instance lock
/// in `dir`. Returns its pid.
pub(crate) fn verify_app_peer(
    sock: &impl AsRawFd,
    dir: &Path,
    flavor: Flavor,
    _policy: &PeerPolicy,
) -> Result<u32, PeerError> {
    let id = identify_same_exe_peer(sock.as_raw_fd())?;
    let holder = lock_holder_pid(dir, flavor)?;
    if holder != Some(id.pid) {
        return Err(PeerError::LockHolderMismatch {
            holder,
            peer: id.pid,
        });
    }
    Ok(id.pid)
}

/// Host side: describe this process's own parent (the browser) for the link handshake, applying
/// the same browser allowlist as the app does.
pub(crate) fn inspect_own_parent(policy: &PeerPolicy) -> Result<PeerReport, PeerError> {
    parent_report(std::process::id(), policy)
}
