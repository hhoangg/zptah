//! macOS peer checks (spec 5.4). The audit token from `LOCAL_PEERTOKEN` identifies the peer
//! without pid-reuse races; `SecCodeCopyGuestWithAttributes` turns it into a code object that
//! `SecCodeCheckValidity` checks against this app's own designated requirement. The host's parent
//! must be Google Chrome (checked by code signature too). A policy with `relax_signature` swaps the
//! signature check for the same-executable check, for unsigned `native-dev` builds only.
//!
//! The Security and CoreFoundation calls are declared here instead of pulling in a crate: the
//! `security-framework` bindings have no `SecCodeCopyDesignatedRequirement`.
use super::dir::current_uid;
use super::lock::lock_holder_pid;
use super::parse::{user_data_dir_from_procargs2, CHROME_REQUIREMENT};
use super::{Flavor, PeerError, PeerPolicy, PeerReport};
use std::ffi::c_void;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::ptr;

type CFTypeRef = *const c_void;
type OSStatus = i32;

#[repr(C)]
struct CFCallBacks {
    _opaque: [u8; 0],
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFTypeDictionaryKeyCallBacks: CFCallBacks;
    static kCFTypeDictionaryValueCallBacks: CFCallBacks;
    fn CFRelease(cf: CFTypeRef);
    fn CFDataCreate(alloc: CFTypeRef, bytes: *const u8, len: isize) -> CFTypeRef;
    fn CFNumberCreate(alloc: CFTypeRef, kind: i32, value: *const c_void) -> CFTypeRef;
    fn CFStringCreateWithBytes(
        alloc: CFTypeRef,
        bytes: *const u8,
        len: isize,
        encoding: u32,
        is_external: u8,
    ) -> CFTypeRef;
    fn CFDictionaryCreate(
        alloc: CFTypeRef,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        count: isize,
        key_callbacks: *const CFCallBacks,
        value_callbacks: *const CFCallBacks,
    ) -> CFTypeRef;
}

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecGuestAttributeAudit: CFTypeRef;
    static kSecGuestAttributePid: CFTypeRef;
    fn SecCodeCopySelf(flags: u32, out: *mut CFTypeRef) -> OSStatus;
    fn SecCodeCopyDesignatedRequirement(
        code: CFTypeRef,
        flags: u32,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecCodeCopyGuestWithAttributes(
        host: CFTypeRef,
        attributes: CFTypeRef,
        flags: u32,
        out: *mut CFTypeRef,
    ) -> OSStatus;
    fn SecCodeCheckValidity(code: CFTypeRef, flags: u32, requirement: CFTypeRef) -> OSStatus;
    fn SecRequirementCreateWithString(text: CFTypeRef, flags: u32, out: *mut CFTypeRef)
        -> OSStatus;
}

const K_CF_NUMBER_SINT32_TYPE: i32 = 3;
const K_CF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

/// An owned CoreFoundation object, released on drop.
struct Cf(CFTypeRef);

impl Cf {
    fn raw(&self) -> CFTypeRef {
        self.0
    }

    fn from_create(obj: CFTypeRef, what: &str) -> Result<Self, String> {
        if obj.is_null() {
            Err(format!("{what} returned null"))
        } else {
            Ok(Self(obj))
        }
    }

    fn string(text: &str) -> Result<Self, String> {
        // SAFETY: the bytes are valid for `len`; a null allocator means the default one.
        let s = unsafe {
            CFStringCreateWithBytes(
                ptr::null(),
                text.as_ptr(),
                text.len() as isize,
                K_CF_STRING_ENCODING_UTF8,
                0,
            )
        };
        Self::from_create(s, "CFStringCreateWithBytes")
    }
}

impl Drop for Cf {
    fn drop(&mut self) {
        // SAFETY: the object came from a Create or Copy call, so we own one reference.
        unsafe { CFRelease(self.0) };
    }
}

fn status(what: &str, rc: OSStatus) -> Result<(), String> {
    if rc == 0 {
        Ok(())
    } else {
        Err(format!("{what} failed with OSStatus {rc}"))
    }
}

fn requirement_from_text(text: &str) -> Result<Cf, String> {
    let text = Cf::string(text)?;
    let mut req: CFTypeRef = ptr::null();
    // SAFETY: `text` is a live CFString and `req` is a valid out pointer.
    status("SecRequirementCreateWithString", unsafe {
        SecRequirementCreateWithString(text.raw(), 0, &mut req)
    })?;
    Cf::from_create(req, "SecRequirementCreateWithString")
}

/// This app's own designated requirement, the one the peer has to satisfy.
fn own_designated_requirement() -> Result<Cf, String> {
    let mut code: CFTypeRef = ptr::null();
    // SAFETY: `code` is a valid out pointer.
    status("SecCodeCopySelf", unsafe { SecCodeCopySelf(0, &mut code) })?;
    let code = Cf::from_create(code, "SecCodeCopySelf")?;
    let mut req: CFTypeRef = ptr::null();
    // SAFETY: `code` is a live SecCode and `req` is a valid out pointer.
    status("SecCodeCopyDesignatedRequirement", unsafe {
        SecCodeCopyDesignatedRequirement(code.raw(), 0, &mut req)
    })?;
    Cf::from_create(req, "SecCodeCopyDesignatedRequirement")
}

fn guest_code(attr_key: CFTypeRef, value: &Cf) -> Result<Cf, String> {
    let keys = [attr_key];
    let values = [value.raw()];
    // SAFETY: one key and one value, both live CF objects, with the standard CFType callbacks.
    let dict = unsafe {
        CFDictionaryCreate(
            ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            1,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    };
    let dict = Cf::from_create(dict, "CFDictionaryCreate")?;
    let mut code: CFTypeRef = ptr::null();
    // SAFETY: a null host means the kernel; `dict` is live and `code` is a valid out pointer.
    status("SecCodeCopyGuestWithAttributes", unsafe {
        SecCodeCopyGuestWithAttributes(ptr::null(), dict.raw(), 0, &mut code)
    })?;
    Cf::from_create(code, "SecCodeCopyGuestWithAttributes")
}

fn guest_by_audit_token(token: &[u32; 8]) -> Result<Cf, String> {
    // SAFETY: the pointer and length describe the 32 bytes of `token`.
    let data = unsafe { CFDataCreate(ptr::null(), token.as_ptr().cast(), 32) };
    let data = Cf::from_create(data, "CFDataCreate")?;
    // SAFETY: the extern static is initialised by the Security framework at load time.
    guest_code(unsafe { kSecGuestAttributeAudit }, &data)
}

fn guest_by_pid(pid: u32) -> Result<Cf, String> {
    let pid = pid as i32;
    // SAFETY: `pid` is a live i32 and the type tag says SInt32.
    let num = unsafe {
        CFNumberCreate(
            ptr::null(),
            K_CF_NUMBER_SINT32_TYPE,
            (&pid as *const i32).cast(),
        )
    };
    let num = Cf::from_create(num, "CFNumberCreate")?;
    // SAFETY: the extern static is initialised by the Security framework at load time.
    guest_code(unsafe { kSecGuestAttributePid }, &num)
}

fn check_validity(code: &Cf, requirement: &Cf) -> Result<(), String> {
    // SAFETY: both arguments are live Security objects.
    status("SecCodeCheckValidity", unsafe {
        SecCodeCheckValidity(code.raw(), 0, requirement.raw())
    })
}

/// `audit_token_t` of the socket peer (`LOCAL_PEERTOKEN`). Layout: auid, euid, egid, ruid, rgid,
/// pid, asid, pidversion.
fn peer_token(fd: RawFd) -> io::Result<[u32; 8]> {
    let mut token = [0u32; 8];
    let mut len = std::mem::size_of_val(&token) as libc::socklen_t;
    // SAFETY: `token` and `len` are valid for the call and `len` is the token size.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            token.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(token)
}

fn proc_path(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer is writable for the given size.
    let n = unsafe { libc::proc_pidpath(pid as i32, buf.as_mut_ptr().cast(), buf.len() as u32) };
    if n <= 0 {
        return None;
    }
    buf.truncate(n as usize);
    Some(PathBuf::from(std::ffi::OsString::from_vec(buf)))
}

fn parent_pid(pid: u32) -> Option<u32> {
    // SAFETY: proc_bsdinfo is plain data and all-zero is a valid value.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    // SAFETY: `info` is writable for `size` bytes.
    let n = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (n == size).then_some(info.pbi_ppid)
}

fn procargs2(pid: u32) -> Option<Vec<u8>> {
    let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as i32];
    let mut len: usize = 0;
    // SAFETY: a null buffer asks for the size; `len` is a valid out pointer.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            ptr::null_mut(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len == 0 {
        return None;
    }
    let mut buf = vec![0u8; len];
    // SAFETY: `buf` is writable for `len` bytes.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            3,
            buf.as_mut_ptr().cast(),
            &mut len,
            ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return None;
    }
    buf.truncate(len);
    Some(buf)
}

fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    let (a, b) = (std::fs::metadata(a)?, std::fs::metadata(b)?);
    Ok(a.dev() == b.dev() && a.ino() == b.ino())
}

/// The peer is our uid and satisfies our own designated requirement (release) or runs the same
/// executable file (relaxed dev policy). Returns its pid.
fn verify_peer_identity(fd: RawFd, policy: &PeerPolicy) -> Result<u32, PeerError> {
    let token = peer_token(fd)?;
    let uid = current_uid();
    if token[1] != uid {
        return Err(PeerError::UidMismatch {
            peer: token[1],
            expected: uid,
        });
    }
    let pid = token[5];
    if policy.relax_signature {
        let exe = proc_path(pid).ok_or(PeerError::PidReused(pid))?;
        let own = std::env::current_exe()?;
        if !same_file(&exe, &own)? {
            return Err(PeerError::ExeMismatch(exe));
        }
    } else {
        let requirement = own_designated_requirement().map_err(PeerError::SelfUnsigned)?;
        let code = guest_by_audit_token(&token).map_err(PeerError::SignatureInvalid)?;
        check_validity(&code, &requirement).map_err(PeerError::SignatureInvalid)?;
    }
    Ok(pid)
}

fn parent_report(pid: u32, policy: &PeerPolicy) -> Result<PeerReport, PeerError> {
    let ppid = parent_pid(pid).ok_or(PeerError::ParentNotBrowser(None))?;
    let parent_exe = proc_path(ppid);
    let extra = policy.relax_signature
        && parent_exe
            .as_ref()
            .is_some_and(|exe| policy.extra_browser_exes.iter().any(|e| e == exe));
    if !extra {
        let ok = requirement_from_text(CHROME_REQUIREMENT)
            .and_then(|req| guest_by_pid(ppid).and_then(|code| check_validity(&code, &req)))
            .is_ok();
        if !ok {
            return Err(PeerError::ParentNotBrowser(parent_exe));
        }
    }
    let user_data_dir = procargs2(ppid).and_then(|raw| user_data_dir_from_procargs2(&raw));
    Ok(PeerReport {
        pid,
        parent_pid: Some(ppid),
        parent_exe,
        user_data_dir,
    })
}

/// App side: the connecting process must be our own signed code, started by an allowed browser.
pub(crate) fn verify_host_peer(
    sock: &impl AsRawFd,
    policy: &PeerPolicy,
) -> Result<PeerReport, PeerError> {
    let pid = verify_peer_identity(sock.as_raw_fd(), policy)?;
    parent_report(pid, policy)
}

/// Host side: the serving process must be our own signed code and the holder of the instance lock
/// in `dir`. Returns its pid.
pub(crate) fn verify_app_peer(
    sock: &impl AsRawFd,
    dir: &Path,
    flavor: Flavor,
    policy: &PeerPolicy,
) -> Result<u32, PeerError> {
    let pid = verify_peer_identity(sock.as_raw_fd(), policy)?;
    let holder = lock_holder_pid(dir, flavor)?;
    if holder != Some(pid) {
        return Err(PeerError::LockHolderMismatch { holder, peer: pid });
    }
    Ok(pid)
}

/// Host side: describe this process's own parent (the browser) for the link handshake, applying
/// the same browser allowlist as the app does.
pub(crate) fn inspect_own_parent(policy: &PeerPolicy) -> Result<PeerReport, PeerError> {
    parent_report(std::process::id(), policy)
}
