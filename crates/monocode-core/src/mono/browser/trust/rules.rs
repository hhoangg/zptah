//! The soul's browser rules as the gate and the Settings screen see them (spec 7.5).
//!
//! The Mono can edit its own soul, so the gate keeps the last set the user confirmed and applies a
//! change at once only when it is a pure addition ([`super::super::soul_rules::reconcile`]).
//! Anything else (a rule removed or edited) leaves the confirmed set in force and waits for the
//! user: [`RuleStatus::needs_confirmation`] is the prompt, [`confirm`] its "Confirm" button, and
//! `copy::rule_review` the words.

use super::super::soul_rules::{reconcile, Parsed};
use super::super::MonoBrowserStore;

/// What is in force and what waits for the user.
#[derive(Debug, Clone)]
pub struct RuleStatus {
    /// The rules the gate applies now.
    pub active: Parsed,
    /// The soul's rules differ from the confirmed set in a way that is not a pure addition.
    pub needs_confirmation: bool,
    /// The lines the user last confirmed.
    pub confirmed_lines: Vec<String>,
    /// The lines the soul holds now (what "Confirm" would accept).
    pub soul_lines: Vec<String>,
    /// Lines that are in the soul now but not in the confirmed set, for the prompt.
    pub changed_lines: Vec<String>,
}

/// The status of a Mono's rules for `soul` (the text of `SOUL.md`). A change that is a pure
/// addition (and the very first set) is recorded as confirmed here.
pub fn status(store: &MonoBrowserStore, mono_id: &str, soul: &str) -> RuleStatus {
    let confirmed = store.confirmed_rules(mono_id);
    let reconciled = reconcile(confirmed.as_deref(), soul);
    let soul_lines = super::super::soul_rules::parse(soul).lines;
    if !reconciled.needs_confirmation && confirmed.as_ref() != Some(&reconciled.confirmed_lines) {
        let _ = store.confirm_rules(mono_id, &reconciled.confirmed_lines);
    }
    let known = reconciled.confirmed_lines.clone();
    let changed_lines = soul_lines
        .iter()
        .filter(|line| !known.contains(line))
        .cloned()
        .collect();
    RuleStatus {
        active: reconciled.active,
        needs_confirmation: reconciled.needs_confirmation,
        confirmed_lines: reconciled.confirmed_lines,
        soul_lines,
        changed_lines,
    }
}

/// The user confirmed the soul's rules as they are now.
pub fn confirm(store: &MonoBrowserStore, mono_id: &str, soul: &str) -> Result<(), String> {
    let lines = super::super::soul_rules::parse(soul).lines;
    store.confirm_rules(mono_id, &lines)
}
