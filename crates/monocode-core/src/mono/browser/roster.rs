//! What the app tells the extension about its Monos (spec 5.5 `roster.set`, 9.1): one entry per
//! Mono that uses the extension, with its name, emoji, exact color, the nearest Chrome tab group
//! color, level and origin lists.
//!
//! * [`group_color`] picks the group color in OKLab, not in RGB; grey only for low-chroma colors.
//! * [`entry_for`] builds one entry from a [`Mono`].
//! * [`RosterBook`] keeps the entries of every Mono, answers the `auth` result of a link (so the
//!   roster is there from the first request) and sends `roster.set` to the live links when it
//!   changes. [`book`] is the process-wide one; the roster of a link holds only the Monos of the
//!   Chrome profile that link belongs to.
use super::link::protocol::{AccessLevel, AuthResult, ChromeGroupColor, Method, RosterEntry};
use super::links::manager::{LinkManager, RosterSource};
use super::{group_name, normalize_origin, BrowserLevel};
use crate::mono::Mono;
use serde_json::json;
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// How long a `roster.set` may take before the link is given up on for this change.
const SET_TIMEOUT: Duration = Duration::from_secs(10);

// ---- group colors (OKLab) ----

/// Chrome's tab group colors with the RGB each is drawn in (light theme).
const GROUP_COLORS: [(ChromeGroupColor, u32); 9] = [
    (ChromeGroupColor::Grey, 0x5f6368),
    (ChromeGroupColor::Blue, 0x1a73e8),
    (ChromeGroupColor::Red, 0xd93025),
    (ChromeGroupColor::Yellow, 0xf9ab00),
    (ChromeGroupColor::Green, 0x188038),
    (ChromeGroupColor::Pink, 0xd01884),
    (ChromeGroupColor::Purple, 0x9334e6),
    (ChromeGroupColor::Cyan, 0x129eaf),
    (ChromeGroupColor::Orange, 0xe8710a),
];

/// Below this OKLab chroma a color is a gray and maps to grey. Every preset of
/// `mono::COLORS_MONO` is well above it (the lowest has about 0.11).
const GREY_CHROMA: f64 = 0.04;

/// A color in OKLab: lightness and the two color axes.
fn oklab(rgb: u32) -> (f64, f64, f64) {
    let linear = |shift: u32| {
        let c = f64::from((rgb >> shift) & 0xff) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = (linear(16), linear(8), linear(0));
    let l = (0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b).cbrt();
    let m = (0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b).cbrt();
    let s = (0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b).cbrt();
    (
        0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
        1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
        0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
    )
}

/// How much a difference in lightness counts next to a difference in hue and chroma. The group
/// colors are all mid-dark, the Mono colors are often lighter: at full weight a light green would
/// sit closer to cyan than to Chrome's green.
const LIGHTNESS_WEIGHT: f64 = 0.5;

/// The Chrome tab group color nearest to a Mono's color by distance in OKLab (lightness counts
/// half, see [`LIGHTNESS_WEIGHT`]). Grey only when the color has almost no chroma (a custom
/// gray); collisions between Monos are allowed.
pub fn group_color(rgb: u32) -> ChromeGroupColor {
    let (l, a, b) = oklab(rgb);
    if a.hypot(b) < GREY_CHROMA {
        return ChromeGroupColor::Grey;
    }
    GROUP_COLORS
        .iter()
        .filter(|(color, _)| *color != ChromeGroupColor::Grey)
        .min_by(|(_, x), (_, y)| {
            let distance = |other: u32| {
                let (l2, a2, b2) = oklab(other);
                (LIGHTNESS_WEIGHT * (l - l2)).powi(2) + (a - a2).powi(2) + (b - b2).powi(2)
            };
            distance(*x).total_cmp(&distance(*y))
        })
        .map_or(ChromeGroupColor::Blue, |(color, _)| *color)
}

/// The name Chrome uses for a color (what `chrome.tabGroups` and the old `--session-color` take).
pub fn group_color_name(color: ChromeGroupColor) -> &'static str {
    match color {
        ChromeGroupColor::Grey => "grey",
        ChromeGroupColor::Blue => "blue",
        ChromeGroupColor::Red => "red",
        ChromeGroupColor::Yellow => "yellow",
        ChromeGroupColor::Green => "green",
        ChromeGroupColor::Pink => "pink",
        ChromeGroupColor::Purple => "purple",
        ChromeGroupColor::Cyan => "cyan",
        ChromeGroupColor::Orange => "orange",
    }
}

// ---- entries ----

/// A Mono id the protocol accepts (32 lowercase hex characters). Older ids that are not are left
/// out of the roster: the extension would refuse the whole list.
fn protocol_id(id: &str) -> bool {
    id.len() == 32 && id.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// The Chrome profile directory and the roster entry of a Mono with its browser on; `None`
/// for a Mono without (or whose id the protocol cannot carry).
pub fn entry_for(mono: &Mono) -> Option<(String, RosterEntry)> {
    let browser = mono.browser.as_ref().filter(|b| b.enabled)?;
    if !protocol_id(&mono.id) || !super::valid_profile_dir(&browser.profile_dir) {
        return None;
    }
    let origins = |list: &[String]| -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for origin in list.iter().filter_map(|o| normalize_origin(o).ok()) {
            if !out.contains(&origin) {
                out.push(origin);
            }
        }
        out
    };
    Some((
        browser.profile_dir.clone(),
        RosterEntry {
            id: mono.id.clone(),
            name: group_name(mono),
            emoji: mono.badge(),
            color: format!("#{:06x}", mono.color & 0xff_ffff),
            group_color: group_color(mono.color),
            level: if browser.trust == BrowserLevel::ReadOnly {
                AccessLevel::Read
            } else {
                AccessLevel::Act
            },
            allowed_origins: origins(&browser.allowed_origins),
            blocked_origins: origins(&browser.blocked_origins),
            cursor: browser.show_actions,
        },
    ))
}

// ---- the book ----

#[derive(Default)]
struct Entries {
    /// (profile directory, entry), in the order of the Mono roster.
    all: Vec<(String, RosterEntry)>,
}

/// The roster of every Mono and what each live link was last told.
#[derive(Default)]
pub struct RosterBook {
    entries: Mutex<Entries>,
    /// The roster each link has, by instance id: sent in `auth` or by `roster.set`.
    sent: Mutex<HashMap<String, Vec<RosterEntry>>>,
    /// Held while changes go out, so they leave in order and the newest one wins.
    sending: Mutex<()>,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl RosterBook {
    /// Replaces every entry with those of `monos`. True when the roster changed.
    pub fn set_monos(&self, monos: &[Mono]) -> bool {
        let all: Vec<_> = monos.iter().filter_map(entry_for).collect();
        let mut entries = lock(&self.entries);
        let changed = entries.all != all;
        entries.all = all;
        changed
    }

    /// Adds or replaces the entry of one Mono (a launch). True when the roster changed.
    pub fn upsert(&self, mono: &Mono) -> bool {
        let mut entries = lock(&self.entries);
        let before = entries.all.clone();
        entries.all.retain(|(_, e)| e.id != mono.id);
        if let Some(entry) = entry_for(mono) {
            entries.all.push(entry);
        }
        entries.all != before
    }

    /// The roster of the Chrome profile `profile_dir`.
    pub fn roster_for(&self, profile_dir: &str) -> Vec<RosterEntry> {
        lock(&self.entries)
            .all
            .iter()
            .filter(|(dir, _)| dir == profile_dir)
            .map(|(_, e)| e.clone())
            .collect()
    }

    /// Sends `roster.set` to every connected link whose roster differs from what it has. Waits for
    /// the answers; call [`Self::publish_soon`] from code that must not block.
    pub(crate) fn publish_with(&self, manager: &LinkManager) -> bool {
        self.publish_timeout(manager, SET_TIMEOUT)
    }

    /// [`Self::publish_with`] giving each `roster.set` `timeout`. True when every connected link
    /// has the roster it should have afterwards.
    pub(crate) fn publish_timeout(&self, manager: &LinkManager, timeout: Duration) -> bool {
        let _sending = lock(&self.sending);
        let mut in_step = true;
        let Ok(links) = manager.links() else {
            return false;
        };
        for link in links.into_iter().filter(|l| l.connected) {
            let want = self.roster_for(&link.profile_dir);
            if lock(&self.sent).get(&link.instance_id) == Some(&want) {
                continue;
            }
            let Some(handle) = manager.handle(&link.instance_id) else {
                continue;
            };
            let result = handle.request(Method::RosterSet, None, json!({ "monos": want }), timeout);
            let mut sent = lock(&self.sent);
            match result {
                Ok(_) => {
                    sent.insert(link.instance_id.clone(), want);
                }
                Err(_) => {
                    sent.remove(&link.instance_id);
                    in_step = false;
                }
            }
        }
        in_step
    }

    /// Publishes until every link has its roster, waiting `delays` between tries. A link that did
    /// not answer in time (a lowered level must still reach it) is tried again.
    pub(crate) fn publish_retrying(&self, manager: &LinkManager, delays: &[Duration]) -> bool {
        if self.publish_with(manager) {
            return true;
        }
        for delay in delays {
            std::thread::sleep(*delay);
            if self.publish_with(manager) {
                return true;
            }
        }
        false
    }

    /// [`Self::publish_with`] on a thread, for the registered link manager (none: nothing to do).
    pub fn publish_soon(self: &'static RosterBook) {
        let Some(manager) = super::links::registry::manager() else {
            return;
        };
        std::thread::spawn(move || {
            let delays = [1, 2, 4, 8, 16].map(Duration::from_secs);
            self.publish_retrying(&manager, &delays);
        });
    }

    /// The name a Mono goes by in the roster, for text that names another Mono.
    pub fn label_of(&self, id: &str) -> Option<String> {
        lock(&self.entries)
            .all
            .iter()
            .find(|(_, e)| e.id == id)
            .map(|(_, e)| e.name.clone())
    }

    /// Forgets what a link was told (it closed): the next connection gets the roster in `auth`.
    pub fn link_closed(&self, instance_id: &str) {
        lock(&self.sent).remove(instance_id);
    }
}

impl RosterSource for RosterBook {
    fn snapshot(&self, instance_id: &str, profile_dir: &str) -> AuthResult {
        let roster = self.roster_for(profile_dir);
        lock(&self.sent).insert(instance_id.to_owned(), roster.clone());
        AuthResult {
            roster,
            stopped: false,
        }
    }
}

/// The process-wide book: the Mono roster feeds it on every load and save, the registered link
/// manager takes its `auth` rosters from it.
pub fn book() -> &'static RosterBook {
    static BOOK: OnceLock<RosterBook> = OnceLock::new();
    BOOK.get_or_init(RosterBook::default)
}

/// The Monos changed (the Mono roster was loaded or saved): update the book and, when the roster
/// of any browser changed, tell the live links.
pub fn monos_changed(monos: &[Mono]) {
    if book().set_monos(monos) {
        book().publish_soon();
    }
}

/// The book as a [`RosterSource`] for a link manager.
pub(crate) struct GlobalRoster;

impl RosterSource for GlobalRoster {
    fn snapshot(&self, instance_id: &str, profile_dir: &str) -> AuthResult {
        book().snapshot(instance_id, profile_dir)
    }
}

#[cfg(test)]
mod tests;
