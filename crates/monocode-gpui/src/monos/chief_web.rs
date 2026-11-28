//! The Chief as the web sees it: its profile, its proposals and the catalog entry. Plain JSON,
//! built from the roster; the manager's own methods (`chief_confirm`, `chief_edit`,
//! `chief_cancel`) stay the only way to change anything. The Mono's one-line reply on a proposal
//! is untrusted model text and is never sent. Notes read the shell, so build these outside a
//! shell update (a deferred task), never inside one.
use super::*;
use mono::chief::{Proposal, ProposalKind, ProposalState, Route};
use serde_json::{Value, json};

fn hex(color: u32) -> String {
    format!("#{:06x}", color & 0xff_ffff)
}

/// The state a proposal is in, as the web names it, and the reason a refusal or failure gave.
fn state_of(state: &ProposalState) -> (&'static str, Option<&str>) {
    match state {
        ProposalState::AwaitingOwner => ("awaitingOwner", None),
        ProposalState::Cancelled => ("cancelled", None),
        ProposalState::QueuedAtMono => ("queuedAtMono", None),
        ProposalState::QueuedInSession => ("queuedInSession", None),
        ProposalState::Sent => ("sent", None),
        ProposalState::Refused(reason) => ("refused", Some(reason)),
        ProposalState::NotAttempted => ("notAttempted", None),
        ProposalState::Failed(reason) => ("failed", Some(reason)),
    }
}

impl Monos {
    /// One proposal for the web. `notes` come from [`Monos::chief_card_notes`] (read live).
    pub(crate) fn chief_web_proposal(&self, proposal: &Proposal, notes: &[&str]) -> Value {
        let (state, reason) = state_of(&proposal.state);
        let route = match &proposal.route {
            Route::ViaMono { mono_id } => json!({
                "type": "viaMono",
                "monoId": mono_id,
                "monoName": self.roster.get(mono_id).map_or_else(|| "Unknown Mono".to_owned(), Mono::label),
            }),
            Route::Direct => json!({"type": "direct"}),
        };
        let mut value = json!({
            "id": proposal.id,
            "createdAt": proposal.created_at,
            "route": route,
            "sessionTitle": proposal.session_title.trim(),
            "project": proposal.project,
            "instruction": proposal.instruction,
            "notes": notes,
            "state": state,
            "open": proposal.is_open(),
            "kind": match proposal.kind {
                ProposalKind::Command => "command",
                ProposalKind::Start { .. } => "start",
            },
        });
        if let Some(at) = proposal.confirmed_at {
            value["confirmedAt"] = json!(at);
        }
        if let Some(id) = proposal.session_id {
            value["sessionNumber"] = json!(id);
        }
        if let Some(branch) = proposal.branch.as_ref().filter(|b| !b.is_empty()) {
            value["branch"] = json!(branch);
        }
        if let Some(reason) = reason {
            value["reason"] = json!(reason);
        }
        value
    }

    /// The proposals, newest first, with what each card warns about. Reads the shell.
    pub(crate) fn chief_web_proposals(&self, cx: &gpui::App) -> Vec<Value> {
        self.chief_proposals()
            .iter()
            .rev()
            .map(|proposal| {
                let notes = if proposal.is_open() {
                    self.chief_card_notes(proposal, cx)
                } else {
                    Vec::new()
                };
                self.chief_web_proposal(proposal, &notes)
            })
            .collect()
    }

    /// The `chief` field a read or a watch frame of the Chief's chat carries, and the answer of
    /// `chief.proposals` (with `enabled`). Reads the shell.
    pub(crate) fn chief_web_value(&self, cx: &gpui::App) -> Value {
        if !self.chief_enabled() {
            return json!({"enabled": false, "proposals": [], "openCount": 0, "allowWebSend": false});
        }
        json!({
            "enabled": true,
            "proposals": self.chief_web_proposals(cx),
            "openCount": self.chief_open_count(),
            "allowWebSend": self.chief_allow_web_send(),
        })
    }

    /// `chief.update` from a remote device: saves the change and makes the chat follow it, as
    /// the desktop's own settings page does. The caller checked the device.
    pub(crate) fn chief_remote_update(
        &mut self,
        params: &Value,
        cx: &mut Context<Self>,
    ) -> Result<(), mono::chief::RemoteUpdateError> {
        let mut chief = self.roster.chief.clone().unwrap_or_default();
        chief.apply_remote_update(params)?;
        self.update_chief(|c| *c = chief, cx);
        Ok(())
    }

    pub(crate) fn chief_allow_web_send(&self) -> bool {
        self.roster.chief.as_ref().is_some_and(|c| c.allow_web_send)
    }

    /// What the chat's status is: `idle`, `working` or `needs-you`.
    fn chief_chat_status(&self, cx: &gpui::App) -> &'static str {
        match self.chief_chat().map(|chat| chat.read(cx).mono_status()) {
            Some("working") => "working",
            Some("needs-you") => "needs-you",
            _ => "idle",
        }
    }

    /// The Chief's entry of `workspace.list`: the identity and what waits for the owner. Never
    /// reads the shell.
    pub(crate) fn chief_catalog_entry(&self, cx: &gpui::App) -> Value {
        let Some(chief) = self.roster.chief.as_ref().filter(|_| self.chief_enabled()) else {
            return json!({"enabled": false, "openCount": 0});
        };
        json!({
            "enabled": true,
            "name": chief.label(),
            "mascot": chief.mascot,
            "emoji": chief.badge(),
            "color": hex(chief.color),
            "status": self.chief_chat_status(cx),
            "openCount": self.chief_open_count(),
        })
    }

    /// `chief.profile`: the Chief's identity, status and read-only settings summary.
    pub(crate) fn chief_web_profile(&self, cx: &gpui::App) -> Value {
        let Some(chief) = self.roster.chief.as_ref().filter(|_| self.chief_enabled()) else {
            return json!({"enabled": false});
        };
        let kind = self.chief_kind();
        let refused = kind != Kind::Claude;
        let line = if refused {
            monocode_core::provider::CHIEF_HARNESS_REFUSAL.to_owned()
        } else if let Some(status) = &self.chief_status {
            status.clone()
        } else {
            "Claude Code".to_owned()
        };
        let mut harness = json!({
            "id": kind.id(),
            "ok": !refused && self.chief_status.is_none(),
            "line": line,
        });
        if let Some(model) = &chief.model {
            harness["model"] = json!(model);
        }
        if let Some(effort) = &chief.effort {
            harness["effort"] = json!(effort);
        }
        json!({
            "enabled": true,
            "name": chief.label(),
            "mascot": chief.mascot,
            "emoji": chief.badge(),
            "color": hex(chief.color),
            "status": self.chief_chat_status(cx),
            "openCount": self.chief_open_count(),
            "allowWebSend": chief.allow_web_send,
            "notifyOnDelivery": chief.notify_on_delivery,
            "harness": harness,
        })
    }
}
