//! The app side of the zptah Browser links (spec sections 5.3, 5.4, 12). One link is one paired
//! extension install (one browser profile) connected through its host process.
//!
//! * [`manager`]: the [`LinkManager`] runs the handshake and pairing on each verified connection,
//!   refuses duplicate instances and browser or data-dir mismatches, keeps one live link per paired
//!   profile and exposes [`LinkHandle`] to send requests.
//! * [`server`]: the listener. It takes the single-instance lock, binds the socket, verifies every
//!   peer with the `peer` module, then hands the connection to the manager.
//! * [`keystore`]: where the pairing key K lives (OS credential store, else `mono_secrets`).
//! * [`pairing`]: the pairing ticket and the page the user is sent to.
//! * [`manifest_install`]: native messaging host manifests per OS and browser.
//! * [`profiles`]: profile names from Chrome's `Local State`.
//! * [`known_builds`]: which extension builds this app accepts (spec 13.5).
//! * `fake_extension` (tests): an in-process extension over a socket pair, reused by later tickets.
//!
//! Nothing here trusts the extension's claims before `auth` succeeded, and the app only ever
//! sends requests; extension requests other than `hello`, `auth` and `pair.begin` are refused.
#![allow(dead_code)] // consumed by the Connect card and the extension upstream (spec tickets T8, T9)

pub(crate) mod keystore;
pub(crate) mod known_builds;
pub(crate) mod manager;
pub(crate) mod manifest_install;
pub(crate) mod pairing;
pub(crate) mod profiles;
pub(crate) mod server;

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fake_extension;

#[cfg(test)]
mod tests;
