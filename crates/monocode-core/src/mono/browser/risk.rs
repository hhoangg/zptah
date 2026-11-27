//! The risk lexicon, site data and classifier of the Browser gate (spec 7.2 to 7.4 and 7.6,
//! Appendix A).
//!
//! The words and sites are JSON files in `risk/data/`, embedded with
//! `include_str!`, so the extension and the gate share one source of truth:
//!
//! * [`normalize`]: diacritic-aware text normalization.
//! * [`lexicon`]: the phrase lexicon and the control-name classifier.
//! * [`signals`]: the element and form signals of 7.2, field kinds, site data, and the table that
//!   turns signals into categories.
//! * [`decide`]: the decision function of 7.6.

pub mod decide;
pub mod lexicon;
pub mod normalize;
pub mod signals;

use serde::{Deserialize, Serialize};

/// A risk category (spec 7.3). Each maps to a prompt sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Payment,
    Destructive,
    AccountSecurity,
    Sharing,
    SendUnnamed,
    PostPublic,
    Upload,
    CredentialEntry,
    Terms,
    UnrelatedDomain,
    SensitiveSite,
    SoulRule,
    Tainted,
    Injection,
    /// The gate could not read enough of the page to tell what the action does (an Ardia
    /// backend category: the signals were incomplete). Always asks, never granted.
    Unclear,
}

impl Category {
    /// The category with this id (`payment`, `send_unnamed`, ...), as the agent declares it.
    pub fn parse(id: &str) -> Option<Category> {
        const ALL: [Category; 15] = [
            Category::Payment,
            Category::Destructive,
            Category::AccountSecurity,
            Category::Sharing,
            Category::SendUnnamed,
            Category::PostPublic,
            Category::Upload,
            Category::CredentialEntry,
            Category::Terms,
            Category::UnrelatedDomain,
            Category::SensitiveSite,
            Category::SoulRule,
            Category::Tainted,
            Category::Injection,
            Category::Unclear,
        ];
        ALL.into_iter().find(|c| c.as_str() == id.trim())
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Category::Payment => "payment",
            Category::Destructive => "destructive",
            Category::AccountSecurity => "account_security",
            Category::Sharing => "sharing",
            Category::SendUnnamed => "send_unnamed",
            Category::PostPublic => "post_public",
            Category::Upload => "upload",
            Category::CredentialEntry => "credential_entry",
            Category::Terms => "terms",
            Category::UnrelatedDomain => "unrelated_domain",
            Category::SensitiveSite => "sensitive_site",
            Category::SoulRule => "soul_rule",
            Category::Tainted => "tainted",
            Category::Injection => "injection",
            Category::Unclear => "unclear",
        }
    }
}

#[cfg(test)]
mod tests;
