//! Length-prefixed framing (spec 5.1): a 32-bit length, then that many bytes of UTF-8 JSON.
//!
//! The socket between the host and the app uses a little-endian length and a 16 MiB ceiling.
//! Chrome's native messaging uses the machine's byte order and limits host-to-extension messages
//! to 1 MiB. A length over the ceiling is refused before any payload byte is read or allocated.

use std::io::{self, Read, Write};

/// Largest frame on the host to app socket.
pub const MAX_SOCKET_FRAME: usize = 16 * 1024 * 1024;
/// Largest message Chrome accepts from a native host.
pub const MAX_HOST_TO_EXTENSION_FRAME: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    Little,
    Native,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameFormat {
    pub order: ByteOrder,
    pub max: usize,
}

impl FrameFormat {
    /// Host <-> app socket.
    pub const SOCKET: FrameFormat = FrameFormat {
        order: ByteOrder::Little,
        max: MAX_SOCKET_FRAME,
    };
    /// Chrome -> host (the host refuses over the socket limit before relaying).
    pub const HOST_FROM_EXTENSION: FrameFormat = FrameFormat {
        order: ByteOrder::Native,
        max: MAX_SOCKET_FRAME,
    };
    /// Host -> Chrome.
    pub const HOST_TO_EXTENSION: FrameFormat = FrameFormat {
        order: ByteOrder::Native,
        max: MAX_HOST_TO_EXTENSION_FRAME,
    };

    fn encode_len(self, len: u32) -> [u8; 4] {
        match self.order {
            ByteOrder::Little => len.to_le_bytes(),
            ByteOrder::Native => len.to_ne_bytes(),
        }
    }
    fn decode_len(self, bytes: [u8; 4]) -> u32 {
        match self.order {
            ByteOrder::Little => u32::from_le_bytes(bytes),
            ByteOrder::Native => u32::from_ne_bytes(bytes),
        }
    }
}

#[derive(Debug)]
pub enum FrameError {
    /// The declared or supplied length is over the ceiling.
    TooLarge {
        len: usize,
        max: usize,
    },
    /// A zero-length frame cannot hold a JSON object.
    Empty,
    /// The stream ended inside a frame.
    Truncated,
    Io(io::Error),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::TooLarge { len, max } => write!(f, "frame of {len} bytes exceeds {max}"),
            FrameError::Empty => write!(f, "empty frame"),
            FrameError::Truncated => write!(f, "stream ended inside a frame"),
            FrameError::Io(e) => write!(f, "frame io: {e}"),
        }
    }
}
impl std::error::Error for FrameError {}

impl From<io::Error> for FrameError {
    fn from(e: io::Error) -> Self {
        FrameError::Io(e)
    }
}

/// Length prefix plus payload.
pub fn encode_frame(format: FrameFormat, payload: &[u8]) -> Result<Vec<u8>, FrameError> {
    check_len(format, payload.len())?;
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&format.encode_len(payload.len() as u32));
    out.extend_from_slice(payload);
    Ok(out)
}

pub fn write_frame<W: Write>(
    writer: &mut W,
    format: FrameFormat,
    payload: &[u8],
) -> Result<(), FrameError> {
    let bytes = encode_frame(format, payload)?;
    writer.write_all(&bytes)?;
    Ok(())
}

/// Reads one frame. `Ok(None)` is a clean end of stream at a frame boundary.
pub fn read_frame<R: Read>(
    reader: &mut R,
    format: FrameFormat,
) -> Result<Option<Vec<u8>>, FrameError> {
    let mut head = [0u8; 4];
    let mut filled = 0;
    while filled < head.len() {
        match reader.read(&mut head[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Truncated),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    let len = format.decode_len(head) as usize;
    check_len(format, len)?;
    // `take` grows the buffer as bytes arrive, so a lying length cannot reserve the ceiling.
    let mut payload = Vec::new();
    let read = reader.take(len as u64).read_to_end(&mut payload)?;
    if read != len {
        return Err(FrameError::Truncated);
    }
    Ok(Some(payload))
}

fn check_len(format: FrameFormat, len: usize) -> Result<(), FrameError> {
    if len == 0 {
        return Err(FrameError::Empty);
    }
    if len > format.max {
        return Err(FrameError::TooLarge {
            len,
            max: format.max,
        });
    }
    Ok(())
}

/// Incremental decoder for async or chunked reads.
#[derive(Debug)]
pub struct FrameDecoder {
    format: FrameFormat,
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new(format: FrameFormat) -> Self {
        Self {
            format,
            buf: Vec::new(),
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole frame, `Ok(None)` when more bytes are needed. An error is final: the
    /// stream is out of sync and the link must close.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, FrameError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = self
            .format
            .decode_len([self.buf[0], self.buf[1], self.buf[2], self.buf[3]])
            as usize;
        check_len(self.format, len)?;
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let payload = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(payload))
    }

    /// Bytes buffered but not yet a frame; non-zero at end of stream means a truncated frame.
    pub fn pending(&self) -> usize {
        self.buf.len()
    }
}
