//! Which Mono owns which tab, as the app knows it (spec 9.2, 9.3). The extension holds the
//! authoritative table because only it sees the user drag a tab; the app keeps a mirror per link,
//! fed by the `owner` of every `tabs.list` and by `tab.ownership` events, and uses it for what
//! only the app can judge: the tab limit, the user's reference for an adopt, taking a tab back and
//! the questions that come with a transfer.
//!
//! * [`Table`] is the mirror of one link: who owns each tab and how, and which tabs the user took
//!   back (and when, so a later adopt needs words written after the stop, spec 10a).
//! * [`check_reference`] and [`find_tab`] are the adopt rules of 9.3: the quote must be words the
//!   user typed, long enough, and point at exactly one tab.
//! * The mirror never decides alone. The extension refuses an act on a tab the Mono does not own
//!   (`E_NOT_OWNER`) whatever this table says.
use super::link::protocol::TabInfo;
use super::risk::normalize::normalize;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// What the gate tells a Mono whose adopt was refused.
pub const ASK_THE_USER: &str = "Ask the user to name the tab, or to drag it into your group.";

/// How a tab came to be owned (spec 9.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    Created,
    Adopted,
    UserDrag,
    Popup,
}

impl How {
    pub fn as_str(self) -> &'static str {
        match self {
            How::Created => "created",
            How::Adopted => "adopted",
            How::UserDrag => "user_drag",
            How::Popup => "popup",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Owned {
    pub tab: u64,
    pub mono: String,
    pub since_ms: u64,
    pub how: How,
}

/// A change the table saw, for the gate to act on (grants end when the user takes a tab back).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub tab: u64,
    pub from: Option<String>,
    pub to: Option<String>,
    /// The user took the tab out of the Mono's group (a drag out, or ungrouping it).
    pub taken_back: bool,
}

/// Why a tab cannot be claimed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Owned by another Mono (its id).
    OwnedBy(String),
}

/// The mirror of one link's ownership table.
#[derive(Debug, Default, Clone)]
pub struct Table {
    owned: HashMap<u64, Owned>,
    /// Tabs the user took back, with the time (ms). Cleared when the tab is owned again.
    taken_back: HashMap<u64, u64>,
    /// Who owned a tab that is nobody's now, until someone owns it again. Lets a Stop that the app
    /// received late (the extension was offline) still be matched to its Mono.
    last_owner: HashMap<u64, String>,
}

impl Table {
    pub fn owner_of(&self, tab: u64) -> Option<&str> {
        self.owned.get(&tab).map(|o| o.mono.as_str())
    }

    pub fn get(&self, tab: u64) -> Option<&Owned> {
        self.owned.get(&tab)
    }

    /// The tabs a Mono owns, by id.
    pub fn tabs_of(&self, mono: &str) -> Vec<u64> {
        let mut tabs: Vec<u64> = self
            .owned
            .values()
            .filter(|o| o.mono == mono)
            .map(|o| o.tab)
            .collect();
        tabs.sort_unstable();
        tabs
    }

    /// How many tabs a Mono owns: the number the tab limit counts (spec 9.3).
    pub fn count_of(&self, mono: &str) -> usize {
        self.owned.values().filter(|o| o.mono == mono).count()
    }

    /// Whether the Mono may open one more tab under the limit.
    pub fn may_open(&self, mono: &str) -> bool {
        self.count_of(mono) < super::gate::tabs::MAX_TABS
    }

    /// Whether the Mono owns the tab, or owned it last and nobody has since (a late Stop).
    pub fn owned_or_last_by(&self, tab: u64, mono: &str) -> bool {
        match self.owned.get(&tab) {
            Some(o) => o.mono == mono,
            None => self.last_owner.get(&tab).is_some_and(|m| m == mono),
        }
    }

    /// Marks a tab as taken back by the user even when the mirror had already dropped its owner.
    pub fn mark_taken_back(&mut self, tab: u64, now_ms: u64) {
        self.owned.remove(&tab);
        self.taken_back.entry(tab).or_insert(now_ms);
    }

    /// The Mono takes a tab nobody else owns (a new tab, an adopted one, a popup, a user drag).
    /// A tab the Mono already owns stays as it is.
    pub fn claim(&mut self, tab: u64, mono: &str, how: How, now_ms: u64) -> Result<(), Conflict> {
        match self.owned.get(&tab) {
            Some(o) if o.mono == mono => return Ok(()),
            Some(o) => return Err(Conflict::OwnedBy(o.mono.clone())),
            None => {}
        }
        self.taken_back.remove(&tab);
        self.last_owner.remove(&tab);
        self.owned.insert(
            tab,
            Owned {
                tab,
                mono: mono.to_owned(),
                since_ms: now_ms,
                how,
            },
        );
        Ok(())
    }

    /// Moves a tab from the Mono that owns it to `to`. The caller has the user's reference and the
    /// user's approval already (spec 9.3). Returns the previous owner; `None` when the tab was
    /// nobody's (then it is a plain adopt).
    pub fn transfer(&mut self, tab: u64, to: &str, now_ms: u64) -> Option<String> {
        let from = self.owned.remove(&tab).map(|o| o.mono);
        self.taken_back.remove(&tab);
        self.last_owner.remove(&tab);
        self.owned.insert(
            tab,
            Owned {
                tab,
                mono: to.to_owned(),
                since_ms: now_ms,
                how: How::Adopted,
            },
        );
        from.filter(|f| f != to)
    }

    /// The tab becomes the user's. Returns who owned it.
    pub fn release(&mut self, tab: u64) -> Option<Owned> {
        let was = self.owned.remove(&tab);
        if let Some(o) = &was {
            self.last_owner.insert(tab, o.mono.clone());
        }
        was
    }

    /// Every tab of a Mono becomes the user's (the Mono was disabled, deleted or moved to another
    /// profile). Returns the tabs.
    pub fn release_mono(&mut self, mono: &str) -> Vec<u64> {
        let tabs = self.tabs_of(mono);
        for tab in &tabs {
            self.owned.remove(tab);
        }
        tabs
    }

    /// The user took the tab out of the Mono's group: it is the user's, and the time is kept.
    pub fn take_back(&mut self, tab: u64, now_ms: u64) -> Option<Owned> {
        let owned = self.owned.remove(&tab)?;
        self.last_owner.insert(tab, owned.mono.clone());
        self.taken_back.insert(tab, now_ms);
        Some(owned)
    }

    /// When the user took this tab back, if they did and it is not owned since.
    pub fn taken_back_at(&self, tab: u64) -> Option<u64> {
        self.taken_back.get(&tab).copied()
    }

    /// Replaces the mirror with what `tabs.list` said. A list of every tab (`all`) also drops the
    /// tabs that are gone; a list of one Mono's tabs only sets the owners it shows.
    pub fn sync(&mut self, tabs: &[TabInfo], all: bool, now_ms: u64) {
        if all {
            let present: Vec<u64> = tabs.iter().map(|t| t.tab).collect();
            self.owned.retain(|id, _| present.contains(id));
            self.taken_back.retain(|id, _| present.contains(id));
        }
        for tab in tabs {
            match (&tab.owner, self.owned.get(&tab.tab)) {
                (Some(owner), Some(o)) if &o.mono == owner => {}
                (Some(owner), _) => {
                    self.taken_back.remove(&tab.tab);
                    self.last_owner.remove(&tab.tab);
                    self.owned.insert(
                        tab.tab,
                        Owned {
                            tab: tab.tab,
                            mono: owner.clone(),
                            since_ms: now_ms,
                            // The list does not say how; the extension owns that fact.
                            how: How::Adopted,
                        },
                    );
                }
                (None, Some(_)) if all => {
                    if let Some(o) = self.owned.remove(&tab.tab) {
                        self.last_owner.insert(tab.tab, o.mono);
                    }
                }
                (None, _) => {}
            }
        }
    }

    /// Applies a `tab.ownership` event (`{tab, owner, by}`) and reports the change. A malformed
    /// event changes nothing.
    pub fn apply_event(&mut self, params: &Value, now_ms: u64) -> Option<Change> {
        let tab = params["tab"].as_u64()?;
        let by = params["by"].as_str()?;
        let owner = match &params["owner"] {
            Value::Null => None,
            Value::String(id) => Some(id.as_str()),
            _ => return None,
        };
        let from = self.owner_of(tab).map(str::to_owned);
        match owner {
            Some(mono) => {
                let how = if by == "popup" {
                    How::Popup
                } else {
                    How::UserDrag
                };
                // The user's drag decides, also over another Mono (9.3: "adopted by the user").
                self.owned.remove(&tab);
                self.claim(tab, mono, how, now_ms).ok()?;
                Some(Change {
                    tab,
                    from,
                    to: Some(mono.to_owned()),
                    taken_back: false,
                })
            }
            None => {
                let taken_back = matches!(by, "user_drag" | "user_ungroup" | "group_closed");
                if taken_back {
                    self.take_back(tab, now_ms);
                } else {
                    self.release(tab);
                }
                from.as_ref()?;
                Some(Change {
                    tab,
                    from,
                    to: None,
                    taken_back,
                })
            }
        }
    }
}

// ---- the mirrors of every link ----

fn tables() -> &'static Mutex<HashMap<String, Table>> {
    static TABLES: OnceLock<Mutex<HashMap<String, Table>>> = OnceLock::new();
    TABLES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs `f` on the mirror of the link `instance_id`.
pub fn with_table<R>(instance_id: &str, f: impl FnOnce(&mut Table) -> R) -> R {
    let mut all = lock(tables());
    f(all.entry(instance_id.to_owned()).or_default())
}

/// Forgets a link's mirror (the browser was disconnected for good).
pub fn forget(instance_id: &str) {
    lock(tables()).remove(instance_id);
}

// ---- adopting: the user's reference (spec 9.3) ----

/// Words that do not point at a tab.
const FILLER: [&str; 13] = [
    "the", "a", "an", "tab", "page", "my", "this", "that", "of", "on", "in", "trang", "cua",
];

/// Why a reference is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReferenceError {
    /// Fewer than 2 words and fewer than 8 characters.
    TooShort,
    /// Not words the user typed in this task.
    NotSaid,
}

impl ReferenceError {
    pub fn text(self) -> String {
        let why = match self {
            ReferenceError::TooShort => {
                "`user_reference` is too short: quote at least 2 words, or 8 characters, the user wrote."
            }
            ReferenceError::NotSaid => {
                "`user_reference` is not something the user wrote in this task. Quote their words exactly."
            }
        };
        format!("{why} {ASK_THE_USER}")
    }
}

fn collapse(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The quote must have at least 2 words or 8 characters (after normalization) and be a verbatim
/// piece, ignoring case and spacing, of one of `user_messages` (text and time in ms): what the
/// user typed in the task. Returns the time of the newest message that holds it.
pub fn check_reference(
    quote: &str,
    user_messages: &[(String, u64)],
) -> Result<u64, ReferenceError> {
    let norm = normalize(quote);
    let chars: usize = norm
        .segments
        .iter()
        .flatten()
        .map(|w| w.plain.chars().count())
        .sum();
    if norm.word_count() < 2 && chars < 8 {
        return Err(ReferenceError::TooShort);
    }
    let wanted = collapse(quote);
    if wanted.is_empty() {
        return Err(ReferenceError::NotSaid);
    }
    user_messages
        .iter()
        .filter(|(text, _)| collapse(text).contains(&wanted))
        .map(|(_, ms)| *ms)
        .max()
        .ok_or(ReferenceError::NotSaid)
}

/// Why the quote does not point at one tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchError {
    None,
    /// Several tabs fit (their ids).
    Several(Vec<u64>),
}

impl MatchError {
    pub fn text(&self) -> String {
        match self {
            MatchError::None => format!("No open tab fits that reference. {ASK_THE_USER}"),
            MatchError::Several(_) => {
                format!("The reference fits more than one tab. {ASK_THE_USER}")
            }
        }
    }
}

fn words_of(text: &str) -> Vec<String> {
    normalize(text)
        .segments
        .into_iter()
        .flatten()
        .map(|w| w.plain)
        .collect()
}

/// The one tab whose title or address holds every significant word of the quote. The tabs are the
/// eligible ones, including those another Mono owns: a quote that also fits another tab is
/// refused.
pub fn find_tab<'a>(quote: &str, tabs: &'a [TabInfo]) -> Result<&'a TabInfo, MatchError> {
    let wanted: Vec<String> = words_of(quote)
        .into_iter()
        .filter(|w| !FILLER.contains(&w.as_str()))
        .collect();
    if wanted.is_empty() {
        return Err(MatchError::None);
    }
    let fits: Vec<&TabInfo> = tabs
        .iter()
        .filter(|tab| {
            let host = url::Url::parse(&tab.url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_owned))
                .unwrap_or_default();
            let mut have = words_of(&tab.title);
            have.extend(words_of(&host));
            wanted.iter().all(|w| have.contains(w))
        })
        .collect();
    match fits.as_slice() {
        [] => Err(MatchError::None),
        [one] => Ok(one),
        many => Err(MatchError::Several(many.iter().map(|t| t.tab).collect())),
    }
}

#[cfg(test)]
mod tests;
