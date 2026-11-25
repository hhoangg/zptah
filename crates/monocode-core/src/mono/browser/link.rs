//! The link between the app and the zptah Browser extension (protocol version 1).
//!
//! This module holds the transport-independent half: the envelope and its validation
//! ([`protocol`]), length-prefixed framing with size limits ([`frame`]) and the mutual HMAC
//! handshake, per-frame MAC and pairing nonce ([`auth`]). Sockets, peer verification and the
//! link manager live elsewhere. The JSON Schema in `extensions/zptah-browser/shared/protocol.json`
//! and the vectors in `shared/handshake-vectors.json` are shared with the extension's tests.

pub mod auth;
pub mod frame;
pub mod protocol;

#[cfg(test)]
mod tests;
