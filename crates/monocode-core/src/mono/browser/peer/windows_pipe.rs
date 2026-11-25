//! Overlapped I/O over the app's named pipe (Windows).
//!
//! The relay reads from the app on one thread and writes to it on another. On a pipe opened
//! synchronously Windows serializes I/O per file object, so a pending `ReadFile` would block every
//! `WriteFile` and deadlock the relay as soon as the extension sends a second frame before the app
//! answers. A handle opened with `FILE_FLAG_OVERLAPPED` has no such lock: each call here issues its
//! own `OVERLAPPED` with its own event and waits for that one operation only.
use std::io::{self, Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_OPERATION_ABORTED, HANDLE, WAIT_TIMEOUT,
};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows_sys::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};

/// Largest chunk handed to one `ReadFile` or `WriteFile`; callers loop on short counts.
const MAX_CHUNK: usize = 1 << 20;

/// Stored in `read_timeout_ms` for "wait as long as it takes".
const NO_TIMEOUT: u64 = u64::MAX;

struct Inner {
    handle: OwnedHandle,
    read_timeout_ms: AtomicU64,
    closed: AtomicBool,
}

/// One overlapped pipe handle, shared by the reader and the writer through [`Self::try_clone`].
#[derive(Clone)]
pub(crate) struct OverlappedPipe {
    inner: Arc<Inner>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Op {
    Read,
    Write,
}

impl OverlappedPipe {
    /// `handle` must have been opened or created with `FILE_FLAG_OVERLAPPED`.
    pub(crate) fn new(handle: OwnedHandle) -> Self {
        Self {
            inner: Arc::new(Inner {
                handle,
                read_timeout_ms: AtomicU64::new(NO_TIMEOUT),
                closed: AtomicBool::new(false),
            }),
        }
    }

    pub(crate) fn try_clone(&self) -> Self {
        self.clone()
    }

    /// A read that waits longer than this fails with `TimedOut` (`None`: no limit). Applies to
    /// every clone and to reads already waiting only from their next call.
    pub(crate) fn set_read_timeout(&self, timeout: Option<Duration>) {
        let ms = match timeout {
            // At least 1 ms (0 would poll) and below INFINITE (u32::MAX).
            Some(t) => (t.as_millis().min(u128::from(u32::MAX - 1)) as u64).max(1),
            None => NO_TIMEOUT,
        };
        self.inner.read_timeout_ms.store(ms, Ordering::SeqCst);
    }

    /// Ends the connection for every clone: a pending or later read returns end of stream and a
    /// write fails. The handle itself closes when the last clone is dropped, so data already
    /// written stays readable for the other end until then.
    pub(crate) fn close(&self) {
        self.inner.closed.store(true, Ordering::SeqCst);
        // SAFETY: a live handle; a null OVERLAPPED cancels every pending operation on it. The
        // result is moot: nothing pending is not an error here.
        unsafe { CancelIoEx(self.raw(), ptr::null()) };
    }

    fn raw(&self) -> HANDLE {
        self.inner.handle.as_raw_handle() as HANDLE
    }

    fn transfer(&self, op: Op, buf: *mut u8, len: usize) -> io::Result<usize> {
        if self.inner.closed.load(Ordering::SeqCst) {
            return Err(io::Error::from(io::ErrorKind::BrokenPipe));
        }
        let len = len.min(MAX_CHUNK) as u32;
        // SAFETY: null attributes and name; a manual-reset, initially unsignaled event.
        let event = unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) };
        if event.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: CreateEventW returned a new handle that nothing else owns; it closes on drop.
        let _event_guard = unsafe { OwnedHandle::from_raw_handle(event as RawHandle) };
        // SAFETY: an all-zero OVERLAPPED is valid; a pipe ignores the offset fields.
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.hEvent = event;
        let handle = self.raw();
        // SAFETY: `buf` is valid for `len` bytes and, with `overlapped`, outlives the operation
        // because GetOverlappedResult below waits for it to finish before this function returns.
        let started = unsafe {
            match op {
                Op::Read => ReadFile(handle, buf, len, ptr::null_mut(), &mut overlapped),
                Op::Write => WriteFile(handle, buf, len, ptr::null_mut(), &mut overlapped),
            }
        };
        if started == 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(ERROR_IO_PENDING as i32) {
                return Err(error);
            }
        }
        // `close` may have run between the check above and the start of this operation, when its
        // cancel had nothing to hit yet.
        if self.inner.closed.load(Ordering::SeqCst) {
            // SAFETY: `overlapped` identifies the operation started above.
            unsafe { CancelIoEx(handle, &overlapped) };
        }
        let mut timed_out = false;
        let timeout = self.inner.read_timeout_ms.load(Ordering::SeqCst);
        if op == Op::Read && timeout != NO_TIMEOUT {
            // SAFETY: a live event handle.
            if unsafe { WaitForSingleObject(event, timeout as u32) } == WAIT_TIMEOUT {
                timed_out = true;
                // SAFETY: `overlapped` identifies the operation started above.
                unsafe { CancelIoEx(handle, &overlapped) };
            }
        }
        let mut done = 0u32;
        // SAFETY: `overlapped` is the structure the operation was started with; bWait = TRUE.
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut done, 1) } == 0 {
            let error = io::Error::last_os_error();
            if timed_out && error.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32) {
                return Err(io::Error::from(io::ErrorKind::TimedOut));
            }
            return Err(error);
        }
        Ok(done as usize)
    }
}

impl AsRawHandle for OverlappedPipe {
    fn as_raw_handle(&self) -> RawHandle {
        self.inner.handle.as_raw_handle()
    }
}

impl Read for OverlappedPipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.transfer(Op::Read, buf.as_mut_ptr(), buf.len()) {
            // The other end closed the pipe, or we did: end of stream.
            Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
            Err(e)
                if self.inner.closed.load(Ordering::SeqCst)
                    && (e.kind() == io::ErrorKind::BrokenPipe
                        || e.raw_os_error() == Some(ERROR_OPERATION_ABORTED as i32)) =>
            {
                Ok(0)
            }
            other => other,
        }
    }
}

impl Write for OverlappedPipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        // The pointer is only read: a write never stores through it.
        self.transfer(Op::Write, buf.as_ptr() as *mut u8, buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A connected pair for tests: (server end, client end), both overlapped, the way the app serves
/// the pipe and the host opens it.
#[cfg(test)]
pub(crate) fn test_pair() -> (OverlappedPipe, OverlappedPipe) {
    use std::os::windows::fs::OpenOptionsExt;
    use std::sync::atomic::{AtomicU32, Ordering};
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OVERLAPPED;
    use windows_sys::Win32::System::Pipes::ConnectNamedPipe;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let name = format!(
        r"\\.\pipe\zptah-host-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let server = super::create_pipe_instance(&name, true).expect("create pipe");
    let client = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(FILE_FLAG_OVERLAPPED)
        .open(&name)
        .expect("open pipe");
    // The client is already connected, so this reports ERROR_PIPE_CONNECTED; its result is moot.
    // SAFETY: valid handle; the OVERLAPPED and its event live until the call returns.
    unsafe {
        let mut overlapped: OVERLAPPED = std::mem::zeroed();
        overlapped.hEvent = CreateEventW(ptr::null(), 1, 0, ptr::null());
        ConnectNamedPipe(server.as_raw_handle() as HANDLE, &mut overlapped);
    }
    (
        OverlappedPipe::new(server),
        OverlappedPipe::new(OwnedHandle::from(client)),
    )
}
