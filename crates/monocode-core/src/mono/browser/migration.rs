//! One-time, versioned, idempotent migration of the saved Browser settings of the Monos
//! (spec 15.3, 15.4 and decision 15 of section 18).
//!
//! It runs at the load of the Mono roster ([`crate::mono::Roster::load`]) on the raw JSON, before
//! the typed read, so the old values are still visible. What it does to each Mono that has a
//! `browser` object:
//!
//! * `trust`: `ask-risky` and `autonomous` (and an unknown value, which always read as Level 2)
//!   become the one acting level, stored as `askRisky` until the old enum is removed with the
//!   Playwright backend (it maps to [`super::trust::BrowserLevel::Act`]); `readOnly` stays.
//! * `blockedOrigins`: kept unchanged. A missing list (it read as the defaults) is written out as
//!   [`super::DEFAULT_BLOCKED_ORIGINS`], so changing the default for new Monos later cannot drop a
//!   block. When any old default is present `suggestNewDefaults` is set for the one-time offer.
//! * `connection` stays absent: the Mono shows "Reconnect needed" until bound ([`RECONNECT_NEEDED`]).
//! * `showActions` is kept: it is the per-Mono cursor switch now. `ready` is recomputed (cleared).
//! * `useExtension` is not touched: the Playwright backend is still the default (spec 15.1), so the
//!   migrated Monos keep their backend. The release that removes it turns the switch on.
//! * `migrationNotice` is set for the one-time notice ([`TRUST_NOTICE`], [`TAB_GROUP_NOTICE`]).
//!
//! Existing Playwright tab groups need no data change (15.4): zptah Browser never adopts them.
//!
//! The secret `playwright-extension-token` is deleted by a second stage ([`Options::delete_token`])
//! that stays off while the Playwright backend still reads it ([`PLAYWRIGHT_REMOVED`]).
//!
//! Safety: the whole roster is transformed in memory first; any structure it does not understand
//! aborts the migration with nothing written. Before the roster row is rewritten the old text is
//! saved under [`BACKUP_KEY`] (never overwritten) and read back. The version marker is written
//! last, so a failure at any point leaves the old row in place and the next load retries.

use crate::store::Store;
use serde_json::{json, Map, Value};

/// The version this code migrates to.
pub const VERSION: u32 = 1;
/// Setting holding the version the roster was migrated to.
pub const VERSION_KEY: &str = "mono-browser-migration";
/// Setting holding the roster row as it was before the migration. Written once, never replaced.
pub const BACKUP_KEY: &str = "mono-roster-backup-browser-v1";
/// Setting set once the Playwright token secrets are deleted.
pub const TOKEN_CLEANUP_KEY: &str = "mono-browser-migration-token";

/// What a Mono shows until it is bound to a paired connection.
pub const RECONNECT_NEEDED: &str = "Reconnect needed";
/// One-time notice about the trust levels (spec 15.3).
pub const TRUST_NOTICE: &str =
    "Monos now ask only before risky steps such as payments and deletions.";
/// One-time notice about the old tab groups (spec 15.4).
pub const TAB_GROUP_NOTICE: &str = "zptah Browser gives each Mono a new tab group. To hand over an old Playwright group, drag its tabs into the new one. You can remove \"Playwright MCP Bridge\" from chrome://extensions.";

/// Whether the Playwright backend is gone. While it is not, its token must stay (the ticket that
/// removes the backend flips this).
pub const PLAYWRIGHT_REMOVED: bool = false;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Delete the `playwright-extension-token` secret of every Mono.
    pub delete_token: bool,
}
impl Options {
    pub fn current() -> Self {
        Self {
            delete_token: PLAYWRIGHT_REMOVED,
        }
    }
}

/// Migrates the stored roster `text` (the `mono-roster` setting) when it has not been migrated and
/// returns the text to read. Never fails: when anything goes wrong the old text is returned and
/// nothing is lost; the next load tries again.
pub fn run(store: &Store, text: Option<String>, options: &Options) -> Option<String> {
    let done = store
        .setting(VERSION_KEY)
        .ok()
        .flatten()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .is_some_and(|v| v >= VERSION);
    let text = if done { text } else { migrate_row(store, text) };
    if options.delete_token {
        if let Some(text) = &text {
            delete_tokens(store, text);
        }
    }
    text
}

fn migrate_row(store: &Store, text: Option<String>) -> Option<String> {
    let Some(old) = text else {
        // Nothing saved yet: every Mono created from now on is already new.
        let _ = store.save_setting(VERSION_KEY, &VERSION.to_string());
        return None;
    };
    let Ok(mut roster) = serde_json::from_str::<Value>(&old) else {
        return Some(old);
    };
    if migrate_roster(&mut roster).is_err() {
        return Some(old);
    }
    let Ok(new) = serde_json::to_string(&roster) else {
        return Some(old);
    };
    if backup(store, &old).is_err() || store.save_setting("mono-roster", &new).is_err() {
        return Some(old);
    }
    // When the marker cannot be written the next load migrates again; the result is the same
    // and the first backup is kept.
    let _ = store.save_setting(VERSION_KEY, &VERSION.to_string());
    Some(new)
}

/// Saves the old row once and reads it back. The first backup is the one that matters: an
/// existing one is kept.
fn backup(store: &Store, old: &str) -> Result<(), String> {
    if store.setting(BACKUP_KEY)?.is_some() {
        return Ok(());
    }
    store.save_setting(BACKUP_KEY, old)?;
    match store.setting(BACKUP_KEY)? {
        Some(saved) if saved == old => Ok(()),
        _ => Err("The backup could not be verified.".into()),
    }
}

fn delete_tokens(store: &Store, text: &str) {
    if store.setting(TOKEN_CLEANUP_KEY).ok().flatten().is_some() {
        return;
    }
    let Ok(roster) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let ids: Vec<&str> = roster["monos"]
        .as_array()
        .map(|monos| monos.iter().filter_map(|m| m["id"].as_str()).collect())
        .unwrap_or_default();
    // No database file means no secret to delete.
    if let Ok(secrets) = super::MonoBrowserStore::for_store(store) {
        for id in ids {
            if secrets.delete_secret(id, super::SECRET_NAME).is_err() {
                return;
            }
        }
    }
    let _ = store.save_setting(TOKEN_CLEANUP_KEY, "1");
}

/// Transforms the roster in place. On error the value may be half changed: the caller drops it.
pub fn migrate_roster(roster: &mut Value) -> Result<(), String> {
    let Some(root) = roster.as_object_mut() else {
        return Err("The roster is not an object.".into());
    };
    let Some(monos) = root.get_mut("monos") else {
        return Ok(());
    };
    let Some(monos) = monos.as_array_mut() else {
        return Err("The monos are not a list.".into());
    };
    for mono in monos {
        let Some(mono) = mono.as_object_mut() else {
            return Err("A Mono is not an object.".into());
        };
        match mono.get_mut("browser") {
            None | Some(Value::Null) => {}
            Some(Value::Object(browser)) => migrate_browser(browser)?,
            Some(_) => return Err("A Mono's browser setting is not an object.".into()),
        }
    }
    Ok(())
}

fn migrate_browser(browser: &mut Map<String, Value>) -> Result<(), String> {
    // Trust: an unreadable or unknown value read as Level 2 before, so it becomes `act`.
    let read_only = matches!(
        browser.get("trust").and_then(Value::as_str),
        Some("readOnly" | "read-only")
    );
    if read_only {
        browser.insert("trust".into(), json!("readOnly"));
    } else if browser.contains_key("trust") {
        browser.insert("trust".into(), json!("askRisky"));
    }

    // Blocked origins: keep, and write the old defaults out when the list was never saved.
    match browser.get("blockedOrigins") {
        None | Some(Value::Null) => {
            browser.insert(
                "blockedOrigins".into(),
                json!(super::DEFAULT_BLOCKED_ORIGINS),
            );
        }
        Some(Value::Array(list)) => {
            if list.iter().any(|o| !o.is_string()) {
                return Err("A blocked origin is not text.".into());
            }
        }
        Some(_) => return Err("The blocked origins are not a list.".into()),
    }
    let has_default = browser["blockedOrigins"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|o| super::normalize_origin(o).ok())
        .any(|o| super::DEFAULT_BLOCKED_ORIGINS.contains(&o.as_str()));
    if has_default {
        browser.insert("suggestNewDefaults".into(), json!(true));
    }

    // `ready` is recomputed by the Connect card; a migrated Mono is not connected.
    if browser.contains_key("ready") {
        browser.insert("ready".into(), json!(false));
    }

    let configured = browser.get("enabled").and_then(Value::as_bool) == Some(true)
        || browser
            .get("profileDir")
            .and_then(Value::as_str)
            .is_some_and(|d| !d.trim().is_empty());
    if configured {
        browser.insert("migrationNotice".into(), json!(true));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
