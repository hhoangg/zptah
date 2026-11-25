//! The socket directory: location, creation and the checks both ends run before every bind and
//! connect (spec 5.4).
use super::{Flavor, PeerError};
use std::fs::DirBuilder;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};

pub(crate) fn current_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// The directory that holds the socket and the lock. macOS: `confstr(_CS_DARWIN_USER_TEMP_DIR)`,
/// never `$TMPDIR` (the host inherits the browser's environment, which a launcher could change).
/// Linux: `$XDG_RUNTIME_DIR/zptah` when it is an absolute path, else `/tmp/zptah-<uid>`.
/// The directory is not created here.
pub(crate) fn socket_dir() -> Result<PathBuf, PeerError> {
    #[cfg(target_os = "macos")]
    {
        let base = darwin_user_temp_dir()?;
        Ok(base.join(format!("zptah-{}", current_uid())))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(linux_socket_dir(
            std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
            current_uid(),
        ))
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn linux_socket_dir(xdg_runtime_dir: Option<PathBuf>, uid: u32) -> PathBuf {
    match xdg_runtime_dir {
        Some(dir) if dir.is_absolute() => dir.join("zptah"),
        _ => PathBuf::from(format!("/tmp/zptah-{uid}")),
    }
}

#[cfg(target_os = "macos")]
fn darwin_user_temp_dir() -> Result<PathBuf, PeerError> {
    use std::os::unix::ffi::OsStringExt;
    let mut buf = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: the buffer is writable for `buf.len()` bytes; confstr NUL-terminates within it.
    let n = unsafe {
        libc::confstr(
            libc::_CS_DARWIN_USER_TEMP_DIR,
            buf.as_mut_ptr().cast(),
            buf.len(),
        )
    };
    if n == 0 || n > buf.len() {
        return Err(io::Error::other("confstr(_CS_DARWIN_USER_TEMP_DIR) failed").into());
    }
    buf.truncate(n - 1);
    Ok(PathBuf::from(std::ffi::OsString::from_vec(buf)))
}

/// The socket file inside `dir`, refusing a path that would not fit `sun_path`.
pub(crate) fn socket_path(dir: &Path, flavor: Flavor) -> Result<PathBuf, PeerError> {
    let path = dir.join(flavor.socket_file());
    if path.as_os_str().len() >= super::parse::SUN_PATH_MAX {
        return Err(PeerError::PathTooLong(path));
    }
    Ok(path)
}

/// Create `dir` (one level, mode 0700) when it is missing, then run [`check_dir`]. The app calls
/// this before it takes the lock.
pub(crate) fn ensure_dir(dir: &Path) -> Result<(), PeerError> {
    match DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e.into()),
    }
    check_dir(dir)
}

/// Both sides run this before every bind and connect: a real directory (not a symlink), owned by
/// our uid, mode exactly 0700.
pub(crate) fn check_dir(dir: &Path) -> Result<(), PeerError> {
    check_dir_as(dir, current_uid())
}

pub(crate) fn check_dir_as(dir: &Path, uid: u32) -> Result<(), PeerError> {
    let meta = std::fs::symlink_metadata(dir)?;
    let ty = meta.file_type();
    if ty.is_symlink() {
        return Err(PeerError::Symlink(dir.to_path_buf()));
    }
    if !ty.is_dir() {
        return Err(PeerError::NotDirectory(dir.to_path_buf()));
    }
    if meta.uid() != uid {
        return Err(PeerError::WrongOwner {
            path: dir.to_path_buf(),
            owner: meta.uid(),
            expected: uid,
        });
    }
    let mode = meta.mode() & 0o7777;
    if mode != 0o700 {
        return Err(PeerError::WrongMode {
            path: dir.to_path_buf(),
            mode,
        });
    }
    Ok(())
}

/// Remove a socket file left behind by a crashed app. Call it only while holding the instance
/// lock. It removes nothing but a socket we own: a symlink or a regular file is refused.
pub(crate) fn remove_stale_socket(path: &Path) -> Result<(), PeerError> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    if meta.file_type().is_symlink() {
        return Err(PeerError::Symlink(path.to_path_buf()));
    }
    if !meta.file_type().is_socket() {
        return Err(PeerError::NotDirectory(path.to_path_buf()));
    }
    if meta.uid() != current_uid() {
        return Err(PeerError::WrongOwner {
            path: path.to_path_buf(),
            owner: meta.uid(),
            expected: current_uid(),
        });
    }
    std::fs::remove_file(path)?;
    Ok(())
}
