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

/// The app's one link manager, installed by whoever starts the listener (the Connect card, T8).
/// The act tools find their link through it; without one a Mono's extension browser is
/// unavailable.
pub(crate) mod registry {
    use super::manager::LinkManager;
    use std::sync::Mutex;

    static MANAGER: Mutex<Option<LinkManager>> = Mutex::new(None);

    /// Makes `manager` the one the Mono runtimes use. The extension events of its receiver must
    /// reach `gate::extension::act::dispatch_event`.
    pub(crate) fn install(manager: LinkManager) {
        // The roster the extension gets in `auth` is the Monos' (spec 9.1); a change goes out as
        // `roster.set`.
        manager.set_roster_source(std::sync::Arc::new(
            crate::mono::browser::roster::GlobalRoster,
        ));
        *MANAGER.lock().unwrap_or_else(|e| e.into_inner()) = Some(manager);
    }

    pub(crate) fn manager() -> Option<LinkManager> {
        MANAGER.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

#[cfg(any(test, feature = "test-support"))]
pub(crate) mod fake_extension;

#[cfg(test)]
mod tests;
