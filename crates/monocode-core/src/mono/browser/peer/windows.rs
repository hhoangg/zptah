//! Windows peer checks (spec 5.4). The pipe is created with `FILE_FLAG_FIRST_PIPE_INSTANCE` (the
//! single-instance guard: a second app, or a squatter that got there first, makes creation fail),
//! `PIPE_REJECT_REMOTE_CLIENTS` and a DACL for the current user only. Each end then takes the
//! other's pid from the pipe, requires its image path to equal ours and its file hash to equal the
//! running app's own. The host's ancestor chain must reach Google Chrome, through `cmd.exe` only.
//!
//! Not done yet: reading the parent's `--user-data-dir` from its PEB, so `PeerReport::user_data_dir`
//! is always `None` here.
use super::parse::{is_chrome_windows_exe, is_windows_shim, pipe_name_for_sid};
use super::{own_exe_hash, sha256_file};
use super::{Flavor, PeerError, PeerPolicy, PeerReport};
use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::path::{Path, PathBuf};
use std::ptr;
use windows_sys::Win32::Foundation::{LocalFree, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, TokenUser, PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, TOKEN_QUERY,
    TOKEN_USER,
};
use windows_sys::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Pipes::{
    CreateNamedPipeW, GetNamedPipeClientProcessId, GetNamedPipeServerProcessId, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, QueryFullProcessImageNameW,
    PROCESS_QUERY_LIMITED_INFORMATION,
};

/// How many ancestors to walk looking for the browser (host, optional `cmd.exe`, browser).
const MAX_ANCESTORS: usize = 4;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The string form of the current user's SID (`S-1-5-21-...`).
fn current_user_sid() -> io::Result<String> {
    // SAFETY: the pseudo handle is always valid.
    process_user_sid(unsafe { GetCurrentProcess() })
}

/// The user SID of the process behind `process` (needs `PROCESS_QUERY_LIMITED_INFORMATION`).
fn process_user_sid(process: HANDLE) -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: `process` is a live process handle and `token` is a valid out pointer.
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcessToken returned a new handle that nothing else owns.
    let token = unsafe { OwnedHandle::from_raw_handle(token as RawHandle) };
    let raw = token.as_raw_handle() as HANDLE;
    let mut len = 0u32;
    // SAFETY: a null buffer of length 0 only asks for the required size.
    unsafe { GetTokenInformation(raw, TokenUser, ptr::null_mut(), 0, &mut len) };
    if len == 0 {
        return Err(io::Error::last_os_error());
    }
    // A u64 buffer keeps the TOKEN_USER view below aligned.
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` is writable for at least `len` bytes.
    if unsafe { GetTokenInformation(raw, TokenUser, buf.as_mut_ptr().cast(), len, &mut len) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the call above filled `buf` with a TOKEN_USER and it is suitably aligned.
    let user = unsafe { &*(buf.as_ptr() as *const TOKEN_USER) };
    let mut sid_str: *mut u16 = ptr::null_mut();
    // SAFETY: `user.User.Sid` points into `buf`, which is alive; `sid_str` is a valid out pointer.
    if unsafe { ConvertSidToStringSidW(user.User.Sid, &mut sid_str) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut n = 0;
    // SAFETY: the string is NUL-terminated and was allocated by the call above.
    let text = unsafe {
        while *sid_str.add(n) != 0 {
            n += 1;
        }
        String::from_utf16_lossy(std::slice::from_raw_parts(sid_str, n))
    };
    // SAFETY: the string was allocated with LocalAlloc by ConvertSidToStringSidW.
    unsafe { LocalFree(sid_str.cast()) };
    Ok(text)
}

/// `\\.\pipe\zptah-browser-<sid hash>` (with `-dev` for Native Dev).
pub(crate) fn pipe_name(flavor: Flavor) -> io::Result<String> {
    Ok(pipe_name_for_sid(
        &current_user_sid()?,
        flavor == Flavor::Dev,
    ))
}

/// Create one pipe instance for the current user only. The app calls it with `first = true` once,
/// which fails when anyone else already created the pipe (the single-instance guard), then with
/// `first = false` for each further instance it needs. Wrap the handle with tokio's named pipe
/// `from_raw_handle` to serve it.
pub(crate) fn create_pipe_instance(name: &str, first: bool) -> io::Result<OwnedHandle> {
    let sddl = wide(&format!("D:P(A;;GA;;;{})", current_user_sid()?));
    let mut sd: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `sddl` is NUL-terminated and `sd` is a valid out pointer.
    let ok = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut sd,
            ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: sd,
        bInheritHandle: 0,
    };
    let mut open_mode = PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED;
    if first {
        open_mode |= FILE_FLAG_FIRST_PIPE_INSTANCE;
    }
    let name = wide(name);
    // SAFETY: `name` is NUL-terminated and `attrs` and the descriptor behind it outlive the call.
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            64 * 1024,
            64 * 1024,
            0,
            &attrs,
        )
    };
    let err = io::Error::last_os_error();
    // SAFETY: the descriptor was allocated by the conversion call above and is no longer used.
    unsafe { LocalFree(sd) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(err);
    }
    // SAFETY: CreateNamedPipeW returned a new handle that nothing else owns.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) })
}

fn image_path(pid: u32) -> io::Result<PathBuf> {
    // SAFETY: plain call; a null result is handled below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcess returned a new handle that nothing else owns.
    let h = unsafe { OwnedHandle::from_raw_handle(h as RawHandle) };
    let mut buf = vec![0u16; 32768];
    let mut len = buf.len() as u32;
    // SAFETY: `buf` is writable for `len` UTF-16 units and `len` is updated to the length written.
    let ok = unsafe {
        QueryFullProcessImageNameW(h.as_raw_handle() as HANDLE, 0, buf.as_mut_ptr(), &mut len)
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(PathBuf::from(OsString::from_wide(&buf[..len as usize])))
}

/// The user SID of the process `pid`.
fn pid_user_sid(pid: u32) -> io::Result<String> {
    // SAFETY: plain call; a null result is handled below.
    let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if h.is_null() {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: OpenProcess returned a new handle that nothing else owns.
    let h = unsafe { OwnedHandle::from_raw_handle(h as RawHandle) };
    process_user_sid(h.as_raw_handle() as HANDLE)
}

/// The process is us: same user SID, same install path, and the file hash equals the hash of the
/// running app taken once at startup ([`own_exe_hash`]). The peer file is read once.
fn verify_same_exe(pid: u32) -> Result<(), PeerError> {
    // The pipe DACL already limits clients to our user; this also covers a squatted server pipe.
    if pid_user_sid(pid)? != current_user_sid()? {
        return Err(PeerError::UserMismatch);
    }
    let peer = image_path(pid)?;
    let own = std::env::current_exe()?;
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let peer_c = canon(&peer);
    if !peer_c
        .to_string_lossy()
        .eq_ignore_ascii_case(&canon(&own).to_string_lossy())
    {
        return Err(PeerError::ExeMismatch(peer));
    }
    match (own_exe_hash(), sha256_file(&peer_c)) {
        (Some(own), Ok(theirs)) if own == theirs => Ok(()),
        _ => Err(PeerError::ExeMismatch(peer)),
    }
}

/// `(pid, parent pid)` for every running process.
fn process_table() -> io::Result<Vec<(u32, u32)>> {
    // SAFETY: plain call; the result is checked below.
    let snap = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snap == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateToolhelp32Snapshot returned a new handle that nothing else owns.
    let snap = unsafe { OwnedHandle::from_raw_handle(snap as RawHandle) };
    let raw = snap.as_raw_handle() as HANDLE;
    // SAFETY: PROCESSENTRY32W is plain data and all-zero is a valid value.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    let mut table = Vec::new();
    // SAFETY: `entry.dwSize` is set and `raw` is a live snapshot.
    let mut ok = unsafe { Process32FirstW(raw, &mut entry) };
    while ok != 0 {
        table.push((entry.th32ProcessID, entry.th32ParentProcessID));
        // SAFETY: as above.
        ok = unsafe { Process32NextW(raw, &mut entry) };
    }
    Ok(table)
}

/// Walk up from `pid` until an allowed browser is found; only `cmd.exe` may sit in between.
fn browser_ancestor(pid: u32, policy: &PeerPolicy) -> Result<PeerReport, PeerError> {
    let table = process_table()?;
    let mut current = pid;
    for _ in 0..MAX_ANCESTORS {
        let Some(&(_, parent)) = table.iter().find(|(p, _)| *p == current) else {
            break;
        };
        let path = image_path(parent).map_err(|_| PeerError::ParentNotBrowser(None))?;
        let text = path.to_string_lossy();
        let allowed = is_chrome_windows_exe(&text)
            || (policy.relaxed()
                && policy
                    .extra_browsers()
                    .iter()
                    .any(|e| e.to_string_lossy().eq_ignore_ascii_case(&text)));
        if allowed {
            return Ok(PeerReport {
                pid,
                parent_pid: Some(parent),
                parent_exe: Some(path),
                user_data_dir: None,
            });
        }
        if !is_windows_shim(&text) {
            return Err(PeerError::ParentNotBrowser(Some(path)));
        }
        current = parent;
    }
    Err(PeerError::ParentNotBrowser(None))
}

/// App side: the connecting process must be our own executable, started by an allowed browser.
pub(crate) fn verify_host_peer(
    pipe: &impl AsRawHandle,
    policy: &PeerPolicy,
) -> Result<PeerReport, PeerError> {
    let mut pid = 0u32;
    // SAFETY: the handle is a live pipe handle and `pid` is a valid out pointer.
    if unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle() as HANDLE, &mut pid) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    verify_same_exe(pid)?;
    browser_ancestor(pid, policy)
}

/// Host side: the serving process must be our own executable. The first-instance flag on the pipe
/// plays the role of the unix instance lock, so there is no lock holder to compare. Returns its pid.
pub(crate) fn verify_app_peer(
    pipe: &impl AsRawHandle,
    _policy: &PeerPolicy,
) -> Result<u32, PeerError> {
    let mut pid = 0u32;
    // SAFETY: the handle is a live pipe handle and `pid` is a valid out pointer.
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as HANDLE, &mut pid) } == 0 {
        return Err(io::Error::last_os_error().into());
    }
    verify_same_exe(pid)?;
    Ok(pid)
}

/// Host side: describe this process's own ancestry (the browser) for the link handshake.
pub(crate) fn inspect_own_parent(policy: &PeerPolicy) -> Result<PeerReport, PeerError> {
    browser_ancestor(std::process::id(), policy)
}
