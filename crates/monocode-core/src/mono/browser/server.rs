//! The registry of the Monos' browser gates (one per Mono with a browser).
//!
//! The gate (see [`super::gate`]) is the only thing the agent talks to: a loopback MCP endpoint in
//! front of the zptah Browser extension link. Its address stays the same while the Mono's browser
//! stays on, so a parked agent process keeps a working URL.
use super::gate::{self, Gate};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// The gates, one per Mono id, and why the last launch of a Mono could not attach its browser.
#[derive(Default)]
pub struct BrowserServers {
    gates: Mutex<HashMap<String, Arc<Gate>>>,
    /// Why the last launch of a Mono could not attach its browser. The Browser tab shows it; a
    /// launch that attached clears it.
    attach_errors: Mutex<HashMap<String, String>>,
    /// The Ardia controller of each Mono's gate, kept after the gate is gone so the Mono's tabs
    /// can still be closed.
    controllers: Mutex<HashMap<String, Arc<super::ardia::Controller>>>,
}
impl BrowserServers {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set_attach_error(&self, mono_id: &str, reason: Option<String>) {
        let mut errors = self.attach_errors.lock().unwrap_or_else(|e| e.into_inner());
        match reason {
            Some(reason) => errors.insert(mono_id.to_owned(), reason),
            None => errors.remove(mono_id),
        };
    }
    /// Why the Mono's last launch could not attach its browser, if it could not.
    pub fn attach_error(&self, mono_id: &str) -> Option<String> {
        self.attach_errors
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(mono_id)
            .cloned()
    }
    /// This Mono's gate in front of the zptah Browser extension: the one it has (updated to the
    /// current policy and upstream), or a new one.
    #[cfg(test)]
    pub(crate) fn gate_for_extension(
        &self,
        mono_id: &str,
        upstream: Arc<gate::ExtensionUpstream>,
        policy: gate::Policy,
    ) -> Result<Arc<Gate>, String> {
        let key = format!("extension-{:p}", Arc::as_ptr(&upstream));
        self.gate_for(mono_id, &key, upstream, policy)
    }
    /// This Mono's gate in front of Ardia.
    pub(crate) fn gate_for_ardia(
        &self,
        mono_id: &str,
        upstream: Arc<super::ardia::ArdiaUpstream>,
        policy: gate::Policy,
    ) -> Result<Arc<Gate>, String> {
        let key = format!("ardia-{:p}", Arc::as_ptr(&upstream));
        self.controllers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(mono_id.to_owned(), upstream.controller().clone());
        self.gate_for(mono_id, &key, upstream, policy)
    }
    /// This Mono's gate in front of `upstream` (named `key`; a new key means a new backend): the
    /// one it has, updated to the current policy and upstream, or a new one. Its address stays
    /// the same while the Mono's browser stays on.
    pub(crate) fn gate_for(
        &self,
        mono_id: &str,
        key: &str,
        upstream: Arc<dyn gate::Upstream>,
        policy: gate::Policy,
    ) -> Result<Arc<Gate>, String> {
        let mut gates = self.gates.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(gate) = gates.get(mono_id) {
            gate.configure(policy, key, upstream);
            return Ok(gate.clone());
        }
        let gate = Gate::start(upstream, policy, key)?;
        gates.insert(mono_id.to_owned(), gate.clone());
        Ok(gate)
    }
    /// The Mono's gate, if its browser has been started in this app session.
    pub fn gate(&self, mono_id: &str) -> Option<Arc<Gate>> {
        self.gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(mono_id)
            .cloned()
    }

    /// Gives this Mono's gate, if it has one, a new policy at once (the user changed the trust
    /// level or the blocked origins while a run may be using it).
    pub fn update_policy(&self, mono_id: &str, policy: gate::Policy) {
        if let Some(gate) = self
            .gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(mono_id)
        {
            gate.set_policy(policy);
        }
    }

    /// Drops this Mono's gate (its browser was turned off or changed, or the Mono is gone).
    pub fn stop(&self, mono_id: &str) {
        let gate = self
            .gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(mono_id);
        if let Some(gate) = gate {
            // A turn that still holds the old address gets a refusal, not a working browser.
            gate.shut_down();
            gate.clear_output();
        }
    }
    /// Ends every Ardia session of the Mono: closes the tabs zptah opened for it and releases the
    /// holds, on a thread. For a browser turned off, a changed profile or a removed Mono; editing
    /// the origin lists drops the gate ([`BrowserServers::stop`]) but leaves a running turn's
    /// tabs alone.
    pub fn end_sessions(&self, mono_id: &str) {
        let controller = self
            .controllers
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(mono_id)
            .cloned();
        if let Some(controller) = controller {
            controller.end_mono_in_background(mono_id);
        }
    }
    /// Drops every gate (the app quits).
    pub fn stop_everything(&self) {
        let gates: Vec<(String, Arc<Gate>)> = self
            .gates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain()
            .collect();
        for (mono_id, gate) in gates {
            gate.shut_down();
            gate.clear_output();
            self.end_sessions(&mono_id);
        }
    }
    pub fn running(&self) -> usize {
        self.gates.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// The registry the app uses.
pub fn servers() -> &'static BrowserServers {
    static SERVERS: OnceLock<BrowserServers> = OnceLock::new();
    SERVERS.get_or_init(BrowserServers::new)
}
