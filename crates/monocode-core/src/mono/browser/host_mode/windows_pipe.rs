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
use std::sync::Arc;
use windows_sys::Win32::Foundation::{ERROR_BROKEN_PIPE, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{ReadFile, WriteFile};
use windows_sys::Win32::System::Threading::CreateEventW;
use windows_sys::Win32::System::IO::{GetOverlappedResult, OVERLAPPED};

/// Largest chunk handed to one `ReadFile` or `WriteFile`; callers loop on short counts.
const MAX_CHUNK: usize = 1 << 20;

/// One overlapped pipe handle, shared by the reader and the writer through [`Self::try_clone`].
#[derive(Clone)]
pub(crate) struct OverlappedPipe {
    handle: Arc<OwnedHandle>,
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
            handle: Arc::new(handle),
        }
    }

    pub(crate) fn try_clone(&self) -> Self {
        self.clone()
    }

    fn transfer(&self, op: Op, buf: *mut u8, len: usize) -> io::Result<usize> {
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
        let handle = self.handle.as_raw_handle() as HANDLE;
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
            if error.raw_os_error() != Some(windows_sys::Win32::Foundation::ERROR_IO_PENDING as i32)
            {
                return Err(error);
            }
        }
        let mut done = 0u32;
        // SAFETY: `overlapped` is the structure the operation was started with; bWait = TRUE.
        if unsafe { GetOverlappedResult(handle, &overlapped, &mut done, 1) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(done as usize)
    }
}

impl Read for OverlappedPipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match self.transfer(Op::Read, buf.as_mut_ptr(), buf.len()) {
            // The other end closed the pipe: end of stream.
            Err(e) if e.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) => Ok(0),
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
    let server = super::super::peer::create_pipe_instance(&name, true).expect("create pipe");
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
