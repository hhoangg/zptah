//! The Chief for a remote (web or phone) device: `chief.profile` and `chief.proposals` (reads),
//! and `chief.proposal.send`, `.edit` and `.cancel` (Control). Every decision is the manager's
//! (`chief_confirm`, `chief_edit`, `chief_cancel`); this checks who asks and writes the audit
//! line. Send also needs the Chief's own "Send from the web" setting (off by default).
//!
//! The manager's calls and the cards' notes read the shell, so they run in a deferred task, never
//! inside a shell update (that would borrow the shell twice).
use super::*;
use gpui::Entity;
use monocode_core::mobile::{DeviceContext, Request, audit, policy, policy::Failure};
use monocode_core::mono::chief::{self, Surface};
use serde_json::{Value, json};

const INVALID: &str = "Invalid mobile parameters";

impl Shell {
    /// `chief.profile` and `chief.proposals`.
    pub(super) fn mobile_chief_read(&mut self, request: Request, cx: &mut Context<Self>) {
        if !policy::chief_visible(&request.device) {
            return request.reply(Err(policy::Code::NotSelected.failure()));
        }
        let Some(monos) = self.monos.clone() else {
            return request.reply(Ok(json!({"enabled": false})));
        };
        cx.defer(move |cx| {
            let monos = monos.read(cx);
            let value = if request.method == "chief.profile" {
                monos.chief_web_profile(cx)
            } else {
                monos.chief_web_value(cx)
            };
            request.reply(Ok(value));
        });
    }

    /// `chief.update`: the settings a remote device may change (name, look, model, effort and the
    /// delivery notice). Anything else is refused whole (`Chief::apply_remote_update`), so a
    /// device can never turn the Chief on, change its harness or account, allow Send from the web
    /// or touch its soul. The host writes the audit line (the method is a Control call).
    pub(super) fn mobile_chief_update(&mut self, request: Request, cx: &mut Context<Self>) {
        if !policy::chief_visible(&request.device) {
            return request.reply(Err(policy::Code::NotSelected.failure()));
        }
        if request.device.permission != policy::Permission::Control {
            return request.reply(Err(policy::Code::ReadOnly.failure()));
        }
        let Some(monos) = self.monos.clone().filter(|m| m.read(cx).chief_enabled()) else {
            return request.reply(Err(policy::Code::Disabled.failure()));
        };
        let result = monos.update(cx, |m, cx| m.chief_remote_update(&request.params, cx));
        if result.is_ok() {
            self.publish_mobile_catalog(cx);
        }
        let result = result.map_err(|error| match error {
            chief::RemoteUpdateError::Forbidden => policy::Code::Forbidden.failure(),
            chief::RemoteUpdateError::Invalid => Failure::from(INVALID),
        });
        if result.is_err() {
            return request.reply(result.map(|_| Value::Null));
        }
        // The reply reads the shell (the chat's status), so it is built once this update is over.
        cx.defer(move |cx| {
            let value = monos.read(cx).chief_web_profile(cx);
            request.reply(Ok(value));
        });
    }

    /// `chief.proposal.send`, `.edit` and `.cancel`, audited with the proposal id only.
    pub(super) fn mobile_chief_command(
        &mut self,
        request: Request,
        finish: impl FnOnce(&mut Shell, Request, Result<Value, Failure>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let fingerprint = self.mobile_fingerprint(&request.device.id);
        let monos = self.monos.clone();
        let shell = cx.weak_entity();
        cx.defer(move |cx| {
            let mut proposal_id = None;
            let result = run_command(
                monos.as_ref(),
                &request.device,
                &request.method,
                &request.params,
                &mut proposal_id,
                cx,
            );
            let outcome = match &result {
                Ok(_) => Some(audit::Outcome::Ok),
                Err(failure) => policy::code_of_failure(failure).map(audit::Outcome::Denied),
            };
            if let Some(outcome) = outcome {
                audit::record_chief_command_with(
                    &request.device,
                    &audit::ChiefCommand {
                        fingerprint: &fingerprint,
                        method: &request.method,
                        proposal_id: proposal_id.as_deref(),
                        command_id: Some(&request.command_id),
                        transport: request.device.kind.into(),
                        outcome,
                    },
                );
            }
            let _ = shell.update(cx, |this, cx| {
                this.refresh_chief_web(cx);
                finish(this, request, result);
            });
        });
    }
}

/// Checks the call in order (parameter shape, selection, permission, the Chief being on, the
/// Send setting) and runs it. `seen` gets the proposal id once it is well formed, for the audit.
fn run_command(
    monos: Option<&Entity<crate::monos::Monos>>,
    device: &DeviceContext,
    method: &str,
    params: &Value,
    seen: &mut Option<String>,
    cx: &mut App,
) -> Result<Value, Failure> {
    let invalid = || Failure::from(INVALID);
    let map = params.as_object().ok_or_else(invalid)?;
    let allowed: &[&str] = match method {
        "chief.proposal.cancel" => &["proposalId"],
        "chief.proposal.send" => &["proposalId", "instruction", "expectedInstruction"],
        _ => &["proposalId", "instruction"],
    };
    if map.keys().any(|key| !allowed.contains(&key.as_str())) {
        return Err(invalid());
    }
    let id = map
        .get("proposalId")
        .and_then(Value::as_str)
        .filter(|id| chief::valid_proposal_id(id))
        .ok_or_else(invalid)?
        .to_owned();
    *seen = Some(id.clone());
    let text = match map.get("instruction") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => return Err(invalid()),
    };
    // A Send names the text the device showed: the owner may have edited it on the desktop
    // since, and the click must never confirm text this device never saw.
    let expected = match (method, map.get("expectedInstruction")) {
        ("chief.proposal.send", Some(Value::String(text))) if !text.is_empty() => {
            Some(text.clone())
        }
        ("chief.proposal.send", _) => return Err(invalid()),
        _ => None,
    };
    if !policy::chief_visible(device) {
        return Err(policy::Code::NotSelected.failure());
    }
    if device.permission != policy::Permission::Control {
        return Err(policy::Code::ReadOnly.failure());
    }
    let monos = monos
        .filter(|m| m.read(cx).chief_enabled())
        .ok_or_else(|| policy::Code::Disabled.failure())?;
    match method {
        "chief.proposal.send" => {
            let allow = monos.read(cx).chief_allow_web_send();
            policy::chief_send_allowed(device, allow)?;
            let shown = expected.ok_or_else(invalid)?;
            let stored = monos
                .read(cx)
                .chief_proposals()
                .iter()
                .find(|p| p.id == id)
                .map(|p| p.instruction.clone());
            if stored.as_deref().is_some_and(|stored| stored != shown) {
                return Err(Failure::from("The text changed. Review the command again"));
            }
            monos
                .update(cx, |m, cx| m.chief_confirm(&id, Surface::Web, text, cx))
                .map_err(Failure::from)?;
        }
        "chief.proposal.edit" => {
            let text = text.ok_or_else(invalid)?;
            monos
                .update(cx, |m, cx| m.chief_edit(&id, &text, cx))
                .map_err(Failure::from)?;
        }
        "chief.proposal.cancel" => {
            monos
                .update(cx, |m, cx| m.chief_cancel(&id, cx))
                .map_err(Failure::from)?;
        }
        _ => return Err(policy::Code::Unsupported.failure()),
    }
    let manager = monos.read(cx);
    let proposal = manager
        .chief_proposals()
        .iter()
        .find(|p| p.id == id)
        .ok_or("Unknown proposal")?;
    let notes = if proposal.is_open() {
        manager.chief_card_notes(proposal, cx)
    } else {
        Vec::new()
    };
    Ok(json!({"proposal": manager.chief_web_proposal(proposal, &notes)}))
}
