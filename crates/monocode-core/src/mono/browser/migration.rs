//! One-time, versioned, idempotent migration of the saved Browser settings of the Monos
//! (spec 15.3, 15.4 and decision 15 of section 18).
//!
//! It runs at the load of the Mono roster ([`crate::mono::Roster::load`]) on the raw JSON, before
//! the typed read, so the old values are still visible. What it does to each Mono that has a
//! `browser` object:
//!
//! * `trust`: `ask-risky` and `autonomous` become the one acting level, stored as `act`
//!   ([`super::trust::BrowserLevel::Act`]); `readOnly` stays; an unknown value becomes `readOnly`.
//! * `blockedOrigins`: kept unchanged. A missing list (it read as the defaults) is written out as
//!   [`super::DEFAULT_BLOCKED_ORIGINS`], so changing the default for new Monos later cannot drop a
//!   block. When any old default is present `suggestNewDefaults` is set for the one-time offer.
//! * `connection` stays absent: the Mono shows "Reconnect needed" until bound ([`RECONNECT_NEEDED`]).
//! * `showActions` is kept: it is the per-Mono cursor switch now. `ready` is recomputed (cleared).
//! * `useExtension` (the preview switch of the retired Playwright backend) is dropped: the zptah
//!   Browser extension is the only backend now (spec 15.1).
//! * `migrationNotice` is set for the one-time notice ([`TRUST_NOTICE`], [`TAB_GROUP_NOTICE`]).
//!
//! Existing Playwright tab groups need no data change (15.4): zptah Browser never adopts them.
//!
//! The secret `playwright-extension-token` is deleted by a second stage ([`Options::delete_token`]),
//! which runs on every load until it has succeeded once ([`TOKEN_CLEANUP_KEY`]).
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
/// The name of the secret the retired Playwright backend kept per Mono in `mono_secrets`.
pub const LEGACY_TOKEN_SECRET: &str = "playwright-extension-token";
/// Setting set once the saved recipes were marked for a dry run.
pub const RECIPE_DRY_RUN_KEY: &str = "mono-browser-migration-recipes";
/// Setting set once the Playwright token secrets are deleted.
pub const TOKEN_CLEANUP_KEY: &str = "mono-browser-migration-token";

/// What a Mono shows until it is bound to a paired connection.
pub const RECONNECT_NEEDED: &str = "Reconnect needed";
/// One-time notice about the trust levels (spec 15.3).
pub const TRUST_NOTICE: &str =
    "Monos now ask only before risky steps such as payments and deletions.";
/// One-time notice about the old tab groups (spec 15.4).
pub const TAB_GROUP_NOTICE: &str = "zptah Browser gives each Mono a new tab group. To hand over an old Playwright group, drag its tabs into the new one. You can remove \"Playwright MCP Bridge\" from chrome://extensions.";

/// Whether the Playwright backend is gone. Nothing reads its token any more, so it is deleted.
pub const PLAYWRIGHT_REMOVED: bool = true;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Delete the `playwright-extension-token` secret of every Mono.
    pub delete_token: bool,
    /// Make every saved recipe wait for a dry run on the new backend (spec 15.5).
    pub recipe_dry_run: bool,
}
impl Options {
    pub fn current() -> Self {
        Self {
            delete_token: PLAYWRIGHT_REMOVED,
            recipe_dry_run: true,
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
        delete_tokens(store);
    }
    if options.recipe_dry_run {
        mark_recipes_for_dry_run(store);
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

fn delete_tokens(store: &Store) {
    if store.setting(TOKEN_CLEANUP_KEY).ok().flatten().is_some() {
        return;
    }
    // Without a database file there is no secret to delete, and nothing is recorded.
    let Ok(secrets) = super::MonoBrowserStore::for_store(store) else {
        return;
    };
    if secrets.delete_secrets_named(LEGACY_TOKEN_SECRET).is_ok() {
        let _ = store.save_setting(TOKEN_CLEANUP_KEY, "1");
    }
}

/// Spec 15.5: recipes were recorded on the retired backend, where an element could be found with a
/// selector and a name was computed slightly differently. Every saved recipe of every Mono waits
/// for a dry run on the zptah Browser before it runs: the dry run (`Gate::dry_run_recipe`, started
/// from Settings > Browser) replays its reading steps against the next real page, turning each
/// locator into a ref of that page, and a locator that does not match goes through the existing
/// repair flow. The mark is the one an imported recipe has ([`super::recipe::share::SOURCE_MIGRATED`]);
/// the recipe files and their approved hashes are not touched. Done once.
fn mark_recipes_for_dry_run(store: &Store) {
    if store.setting(RECIPE_DRY_RUN_KEY).ok().flatten().is_some() {
        return;
    }
    let Ok(approvals) = super::MonoBrowserStore::for_store(store) else {
        return;
    };
    let root = crate::mono::root(store);
    let Ok(entries) = std::fs::read_dir(&root) else {
        // No Mono folder yet: nothing was ever recorded. Look again next time.
        return;
    };
    let mut done = true;
    for entry in entries.flatten() {
        let Some(mono_id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // The shared library (`_shared_recipes`) and anything else that is not a Mono id.
        if !mono_id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
        {
            continue;
        }
        let recipes = super::recipe::store::RecipeStore::new(super::recipes_dir(&root, &mono_id));
        for recipe in recipes.list() {
            // An imported or shared recipe keeps its own mark: it needs a dry run already.
            if approvals.outside_source(&mono_id, &recipe.id).is_some() {
                continue;
            }
            if approvals
                .mark_outside(&mono_id, &recipe.id, super::recipe::share::SOURCE_MIGRATED)
                .is_err()
            {
                done = false;
            }
        }
    }
    if done {
        let _ = store.save_setting(RECIPE_DRY_RUN_KEY, "1");
    }
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
    if browser.get("migrated").and_then(Value::as_u64) >= Some(u64::from(VERSION)) {
        return Ok(());
    }
    // Trust: the acting levels become `act`; an unknown value never becomes `act`.
    let trust = browser
        .get("trust")
        .and_then(Value::as_str)
        .map(|t| match t {
            "askRisky" | "ask-risky" | "askEvery" | "ask-every" | "autonomous" | "act" => "act",
            _ => "readOnly",
        });
    if let Some(trust) = trust {
        browser.insert("trust".into(), json!(trust));
    } else if browser.contains_key("trust") {
        browser.insert("trust".into(), json!("readOnly"));
    }
    browser.remove("useExtension");
    browser.insert("migrated".into(), json!(VERSION));

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
