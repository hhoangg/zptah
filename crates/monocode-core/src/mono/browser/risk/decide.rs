//! The decision function (spec 7.6).
//!
//! ```text
//! effective = gate_categories ∪ declared_risk ∪ ext_risk ∪ soul_rule_matches
//! blocked rule or invariant                 -> deny with reason
//! level == read and the tool is not read-ok -> deny ("read only")
//! effective is empty                        -> allow
//! every category covered by a live grant    -> allow (grant noted in the audit)
//! otherwise                                 -> ask, all categories in one prompt
//! ```
//!
//! The union means a declared purpose can only add categories, never remove one. Payments and
//! credential entry are never covered by a grant. Turning an ask into a deny when nobody can
//! answer is the caller's job (`can_answer`).

use super::Category;
use std::collections::BTreeSet;

/// The result of [`decide`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    /// Every category is covered by a live "allow for this task" grant.
    AllowByGrant(Vec<Category>),
    /// Ask once, listing every category.
    Ask(Vec<Category>),
    Deny(String),
}

/// What the decision reads.
#[derive(Debug, Clone)]
pub struct DecisionInput<'a> {
    /// A blocked rule or invariant fired (blocked origin, password entry, soul `block`).
    pub blocked: Option<&'a str>,
    /// The Mono's level is read-only.
    pub read_only_level: bool,
    /// The tool is marked "yes" for read-only in 7.1.
    pub tool_read_ok: bool,
    pub gate: &'a BTreeSet<Category>,
    pub declared: &'a BTreeSet<Category>,
    pub ext: &'a BTreeSet<Category>,
    pub soul: &'a BTreeSet<Category>,
    /// Categories covered by live grants for this site (conversation-long, answer 9).
    pub grants: &'a BTreeSet<Category>,
}

/// Categories a grant can never cover.
pub fn never_granted(c: Category) -> bool {
    matches!(c, Category::Payment | Category::CredentialEntry)
}

/// The union of every source of categories.
pub fn effective(input: &DecisionInput<'_>) -> BTreeSet<Category> {
    input
        .gate
        .iter()
        .chain(input.declared)
        .chain(input.ext)
        .chain(input.soul)
        .copied()
        .collect()
}

pub fn decide(input: &DecisionInput<'_>) -> Decision {
    if let Some(reason) = input.blocked {
        return Decision::Deny(reason.to_owned());
    }
    if input.read_only_level && !input.tool_read_ok {
        return Decision::Deny("read only".to_owned());
    }
    let effective = effective(input);
    if effective.is_empty() {
        return Decision::Allow;
    }
    let covered = |c: &Category| input.grants.contains(c) && !never_granted(*c);
    let list: Vec<Category> = effective.iter().copied().collect();
    if effective.iter().all(covered) {
        Decision::AllowByGrant(list)
    } else {
        Decision::Ask(list)
    }
}
