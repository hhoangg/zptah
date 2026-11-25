//! Browser rules in a Mono's soul (spec 7.5).
//!
//! A user can add rules to `SOUL.md` in a fenced block under a `## Browser rules` heading, so free
//! prose is never misread as a rule:
//!
//! ````text
//! ## Browser rules
//!
//! ```browser-rules
//! ask: click "Book" on airbnb.com
//! block: navigate to facebook.com
//! ```
//! ````
//!
//! Rules can only add asks or blocks: there is no `allow` verb. Because the Mono can edit its own
//! soul, the gate keeps the last confirmed rule set and applies a change at once only when it is a
//! pure addition ([`reconcile`]); anything else waits for the user. A line that does not parse is an
//! error, and until it is fixed the gate asks before every write ([`Parsed::fail_closed`]).

use super::risk::normalize::normalize;
use super::risk::signals::host_of;
use super::risk::Category;
use std::collections::BTreeSet;

/// What a rule does when it matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Ask,
    Block,
}

/// The kind of action a rule names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    Any,
    Click,
    Type,
    Navigate,
    Upload,
    Send,
    Post,
    Select,
}

/// What the action is, for matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactKind {
    Click,
    Type,
    Navigate,
    Upload,
    Select,
    /// Another tool that acts (a key press, a dialog answer, a tab change).
    Other,
}

/// A site pattern: `host`, `*.host` or `*`. A host also covers its subdomains: a rule can only
/// add asks, so matching more is the safe side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Site {
    Any,
    Domain(String),
}

impl Site {
    fn parse(text: &str) -> Result<Site, String> {
        let text = text.trim().to_lowercase();
        if text == "*" {
            return Ok(Site::Any);
        }
        let host = text.strip_prefix("*.").unwrap_or(&text);
        let valid = !host.is_empty()
            && host.contains('.')
            && !host.starts_with('.')
            && !host.ends_with('.')
            && !host.contains("..")
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
        if valid {
            Ok(Site::Domain(host.to_owned()))
        } else {
            Err(format!(
                "`{text}` is not a site (use example.com, *.example.com or *)"
            ))
        }
    }

    fn matches(&self, host: Option<&str>) -> bool {
        match self {
            Site::Any => true,
            Site::Domain(domain) => host.is_some_and(|h| {
                let h = h.trim_end_matches('.').to_lowercase();
                h == *domain || h.ends_with(&format!(".{domain}"))
            }),
        }
    }
}

/// What one rule matches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    Action {
        action: RuleAction,
        /// Normalized (diacritics folded, lowercase) text the element's name or text must contain.
        text: Option<String>,
        site: Site,
    },
    Category {
        category: Category,
        site: Site,
    },
}

/// One parsed rule with the line it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub verb: Verb,
    pub condition: Condition,
    pub line: String,
}

/// A line that did not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError {
    pub line: String,
    pub message: String,
}

/// The rules of a soul.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Parsed {
    pub rules: Vec<Rule>,
    /// The significant lines of the block (trimmed, no comments or blanks), in order.
    pub lines: Vec<String>,
    pub errors: Vec<RuleError>,
}

impl Parsed {
    /// Until an unparseable line is fixed, every write asks.
    pub fn fail_closed(&self) -> bool {
        !self.errors.is_empty()
    }
}

/// What an action looks like to the rules.
#[derive(Debug, Clone)]
pub struct ActionFacts<'a> {
    pub kind: FactKind,
    /// The element's name, text and label (any case, any diacritics).
    pub element_text: &'a str,
    /// The page's URL; for a navigation, the address it goes to.
    pub url: &'a str,
    pub send: bool,
    pub post: bool,
    /// The categories the gate found before the rules ran.
    pub categories: &'a BTreeSet<Category>,
}

/// The outcome of the rules for one action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Matched {
    /// The first `block` rule that matched.
    pub blocked: Option<String>,
    /// At least one `ask` rule matched.
    pub ask: bool,
    /// Lines of every rule that matched, for the prompt.
    pub lines: Vec<String>,
}

fn fold(text: &str) -> String {
    normalize(text).joined(true)
}

impl Rule {
    pub fn matches(&self, facts: &ActionFacts<'_>) -> bool {
        let host = host_of(facts.url);
        match &self.condition {
            Condition::Category { category, site } => {
                facts.categories.contains(category) && site.matches(host.as_deref())
            }
            Condition::Action { action, text, site } => {
                let action_ok = match action {
                    RuleAction::Any => true,
                    RuleAction::Click => facts.kind == FactKind::Click,
                    RuleAction::Type => facts.kind == FactKind::Type,
                    RuleAction::Navigate => facts.kind == FactKind::Navigate,
                    RuleAction::Upload => facts.kind == FactKind::Upload,
                    RuleAction::Select => facts.kind == FactKind::Select,
                    RuleAction::Send => facts.send,
                    RuleAction::Post => facts.post,
                };
                if !action_ok || !site.matches(host.as_deref()) {
                    return false;
                }
                match text {
                    Some(wanted) => fold(facts.element_text).contains(wanted.as_str()),
                    None => true,
                }
            }
        }
    }
}

impl Parsed {
    /// Every rule that matches `facts`.
    pub fn matching(&self, facts: &ActionFacts<'_>) -> Matched {
        let mut out = Matched::default();
        for rule in self.rules.iter().filter(|r| r.matches(facts)) {
            out.lines.push(rule.line.clone());
            match rule.verb {
                Verb::Block => {
                    out.blocked.get_or_insert_with(|| rule.line.clone());
                }
                Verb::Ask => out.ask = true,
            }
        }
        out
    }
}

/// The lines of the `browser-rules` fence under a `## Browser rules` heading, with a flag for a
/// fence that never closes.
fn block_lines(soul: &str) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut in_section = false;
    let mut in_fence = false;
    let mut unterminated = false;
    for raw in soul.lines() {
        let trimmed = raw.trim();
        if in_fence {
            if trimmed == "```" {
                in_fence = false;
            } else {
                lines.push(trimmed.to_owned());
            }
            continue;
        }
        if let Some(title) = trimmed.strip_prefix("## ") {
            in_section = title.trim().eq_ignore_ascii_case("browser rules");
            continue;
        }
        if trimmed.starts_with("# ") {
            in_section = false;
            continue;
        }
        if in_section && trimmed == "```browser-rules" {
            in_fence = true;
        }
    }
    if in_fence {
        unterminated = true;
    }
    (lines, unterminated)
}

/// Parses the rules of a soul. Prose outside the fenced block is never read.
pub fn parse(soul: &str) -> Parsed {
    let (raw, unterminated) = block_lines(soul);
    let mut parsed = Parsed::default();
    for line in raw {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        parsed.lines.push(line.to_owned());
        match parse_line(line) {
            Ok(rule) => parsed.rules.push(rule),
            Err(message) => parsed.errors.push(RuleError {
                line: line.to_owned(),
                message,
            }),
        }
    }
    if unterminated {
        parsed.errors.push(RuleError {
            line: "```browser-rules".into(),
            message: "the rules block is never closed".into(),
        });
    }
    parsed
}

const ACTIONS: [(&str, RuleAction); 8] = [
    ("any action", RuleAction::Any),
    ("navigate to", RuleAction::Navigate),
    ("click", RuleAction::Click),
    ("type", RuleAction::Type),
    ("upload", RuleAction::Upload),
    ("send", RuleAction::Send),
    ("post", RuleAction::Post),
    ("select", RuleAction::Select),
];

/// Splits off a leading keyword (case-insensitive) that ends at a space or the end of the text.
fn strip_keyword<'a>(text: &'a str, keyword: &str) -> Option<&'a str> {
    let head = text.get(..keyword.len())?;
    if !head.eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &text[keyword.len()..];
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim_start())
}

fn parse_line(line: &str) -> Result<Rule, String> {
    let (verb, condition) = line
        .split_once(':')
        .ok_or("expected `ask:` or `block:` at the start")?;
    let verb = match verb.trim().to_lowercase().as_str() {
        "ask" => Verb::Ask,
        "block" => Verb::Block,
        other => return Err(format!("`{other}` is not a verb (use ask or block)")),
    };
    let condition = condition.trim();
    if condition.is_empty() {
        return Err("the rule has no condition".into());
    }
    let condition = if let Some(rest) = strip_keyword(condition, "category") {
        let (name, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
        let category = Category::parse(&name.to_lowercase())
            .ok_or_else(|| format!("`{name}` is not a category"))?;
        Condition::Category {
            category,
            site: parse_on(rest.trim())?,
        }
    } else {
        let (action, rest) = ACTIONS
            .iter()
            .find_map(|(word, action)| strip_keyword(condition, word).map(|rest| (*action, rest)))
            .ok_or("unknown action (use any action, click, type, navigate to, upload, send, post or select)")?;
        if action == RuleAction::Navigate {
            if rest.is_empty() {
                return Err("`navigate to` needs a site".into());
            }
            Condition::Action {
                action,
                text: None,
                site: Site::parse(rest)?,
            }
        } else {
            let (text, rest) = if let Some(quoted) = rest.strip_prefix('"') {
                let (inside, after) = quoted
                    .split_once('"')
                    .ok_or("the quoted text is never closed")?;
                let folded = fold(inside);
                if folded.is_empty() {
                    return Err("the quoted text is empty".into());
                }
                (Some(folded), after.trim())
            } else {
                (None, rest)
            };
            Condition::Action {
                action,
                text,
                site: parse_on(rest)?,
            }
        }
    };
    Ok(Rule {
        verb,
        condition,
        line: line.to_owned(),
    })
}

/// `on <site>` or nothing (any site).
fn parse_on(rest: &str) -> Result<Site, String> {
    if rest.is_empty() {
        return Ok(Site::Any);
    }
    let site = strip_keyword(rest, "on")
        .filter(|s| !s.is_empty())
        .ok_or("expected `on <site>` at the end of the rule")?;
    Site::parse(site)
}

/// What the gate does with the rules of the soul as it is now, given the last confirmed set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciled {
    /// The rules in force: the new ones when the change is a pure addition, else the confirmed set.
    pub active: Parsed,
    /// The soul changed in a way that needs the user's confirmation (Settings > Browser).
    pub needs_confirmation: bool,
    /// The set to store as confirmed (the new lines when applied, else the old ones).
    pub confirmed_lines: Vec<String>,
}

/// Whether `new` only added lines to `confirmed`: every confirmed line is still there, byte for
/// byte and in the same order.
pub fn is_pure_addition(confirmed: &[String], new: &[String]) -> bool {
    let mut at = 0;
    for old in confirmed {
        match new[at..].iter().position(|l| l == old) {
            Some(i) => at += i + 1,
            None => return false,
        }
    }
    true
}

/// Applies the rules of a soul against the last confirmed set (`None`: nothing confirmed yet, so
/// the first set is a pure addition). A change that is not a pure addition keeps the confirmed set
/// in force until the user confirms. Parse errors of the soul as it is now still fail closed.
pub fn reconcile(confirmed: Option<&[String]>, soul: &str) -> Reconciled {
    let current = parse(soul);
    let Some(confirmed) = confirmed else {
        return Reconciled {
            confirmed_lines: current.lines.clone(),
            active: current,
            needs_confirmation: false,
        };
    };
    if is_pure_addition(confirmed, &current.lines) {
        return Reconciled {
            confirmed_lines: current.lines.clone(),
            active: current,
            needs_confirmation: false,
        };
    }
    // Keep the confirmed rules, but a broken current block still makes writes ask.
    let mut active = parse_lines(confirmed);
    active.errors.extend(current.errors);
    Reconciled {
        active,
        needs_confirmation: true,
        confirmed_lines: confirmed.to_vec(),
    }
}

/// Parses already extracted rule lines.
pub fn parse_lines(lines: &[String]) -> Parsed {
    let mut parsed = Parsed::default();
    for line in lines {
        parsed.lines.push(line.clone());
        match parse_line(line) {
            Ok(rule) => parsed.rules.push(rule),
            Err(message) => parsed.errors.push(RuleError {
                line: line.clone(),
                message,
            }),
        }
    }
    parsed
}

#[cfg(test)]
mod tests;
