//! Conversation-long grants (spec 7.6, answer 9) and the confirmed soul rule set (spec 7.5).
//!
//! A grant ("Allow in this conversation") covers its categories on its site for the whole
//! conversation, across runs, and survives an app restart because it is stored with the
//! conversation in the app database. `payment` and `credential_entry` are never granted. Grants end
//! on Stop, when the user takes the task back, when the conversation is closed and when the Mono's
//! browser settings change.

use super::super::risk::decide::never_granted;
use super::super::risk::Category;
use super::super::{MonoBrowserStore, BROWSER_TABLES, UNAVAILABLE};
use rusqlite::params;
use std::collections::{BTreeMap, BTreeSet};

/// Grants of one conversation of one Mono. With a store they live in the database and are read on
/// every call, so a Stop or a revoke from another handle takes effect at once. Without one (no
/// database) they live in memory.
#[derive(Debug, Clone)]
pub struct GrantBook {
    mono_id: String,
    conversation: i64,
    store: Option<MonoBrowserStore>,
    memory: BTreeMap<String, BTreeSet<Category>>,
}

impl GrantBook {
    pub fn in_memory(mono_id: &str, conversation: i64) -> Self {
        Self {
            mono_id: mono_id.to_owned(),
            conversation,
            store: None,
            memory: BTreeMap::new(),
        }
    }

    pub fn stored(store: MonoBrowserStore, mono_id: &str, conversation: i64) -> Self {
        Self {
            store: Some(store),
            ..Self::in_memory(mono_id, conversation)
        }
    }

    /// Records a grant for `site` (a registrable domain). Never-granted categories are dropped.
    /// Returns the categories that were actually granted.
    pub fn grant(&mut self, site: &str, categories: &BTreeSet<Category>) -> BTreeSet<Category> {
        let granted: BTreeSet<Category> = categories
            .iter()
            .copied()
            .filter(|c| !never_granted(*c))
            .collect();
        if granted.is_empty() || site.is_empty() {
            return BTreeSet::new();
        }
        match &self.store {
            Some(store) => {
                if store
                    .save_grants(&self.mono_id, self.conversation, site, &granted)
                    .is_err()
                {
                    // Not stored: the next action asks again, which is the safe side.
                    return BTreeSet::new();
                }
            }
            None => self
                .memory
                .entry(site.to_owned())
                .or_default()
                .extend(granted.iter().copied()),
        }
        granted
    }

    /// The categories a live grant covers on `site`.
    pub fn covered(&self, site: &str) -> BTreeSet<Category> {
        match &self.store {
            Some(store) => store.grants_for(&self.mono_id, self.conversation, site),
            None => self.memory.get(site).cloned().unwrap_or_default(),
        }
    }

    /// Every grant of this conversation, for Settings > Browser.
    pub fn list(&self) -> Vec<(String, Category)> {
        match &self.store {
            Some(store) => store.list_grants(&self.mono_id, Some(self.conversation)),
            None => self
                .memory
                .iter()
                .flat_map(|(site, cats)| cats.iter().map(|c| (site.clone(), *c)))
                .collect(),
        }
    }

    /// Ends the grants of this conversation for one site (the overlay's Stop, a take-back).
    pub fn end_site(&mut self, site: &str) {
        match &self.store {
            Some(store) => {
                let _ = store.revoke_grants(&self.mono_id, Some(self.conversation), Some(site));
            }
            None => {
                self.memory.remove(site);
            }
        }
    }

    /// Ends the Mono's grants for one site in every conversation (the overlay's Stop, a
    /// take-back): the user took the tab, so no conversation keeps the right to act there.
    pub fn end_site_everywhere(&mut self, site: &str) {
        match &self.store {
            Some(store) => {
                let _ = store.revoke_grants(&self.mono_id, None, Some(site));
            }
            None => {
                self.memory.remove(site);
            }
        }
    }

    /// Ends every grant of this conversation (the conversation is closed or cleared).
    pub fn end_conversation(&mut self) {
        match &self.store {
            Some(store) => {
                let _ = store.revoke_grants(&self.mono_id, Some(self.conversation), None);
            }
            None => self.memory.clear(),
        }
    }
}

impl MonoBrowserStore {
    fn save_grants(
        &self,
        mono_id: &str,
        conversation: i64,
        site: &str,
        categories: &BTreeSet<Category>,
    ) -> Result<(), String> {
        Self::valid_id(mono_id)?;
        let conn = self.open()?.ok_or(UNAVAILABLE)?;
        conn.execute_batch(BROWSER_TABLES)
            .map_err(|_| UNAVAILABLE)?;
        let now = crate::time::now_ms() as i64;
        for category in categories.iter().filter(|c| !never_granted(**c)) {
            conn.execute(
                "INSERT OR REPLACE INTO mono_browser_grants(mono_id,conversation,site,category,granted_at) VALUES(?1,?2,?3,?4,?5)",
                params![mono_id, conversation, site, category.as_str(), now],
            )
            .map_err(|_| UNAVAILABLE)?;
        }
        Ok(())
    }

    /// The live grants of one conversation on one site. Categories that must never be granted are
    /// ignored even if a row says so.
    pub fn grants_for(&self, mono_id: &str, conversation: i64, site: &str) -> BTreeSet<Category> {
        let Ok(Some(conn)) = self.open() else {
            return BTreeSet::new();
        };
        let Ok(mut query) = conn.prepare(
            "SELECT category FROM mono_browser_grants WHERE mono_id=?1 AND conversation=?2 AND site=?3",
        ) else {
            return BTreeSet::new();
        };
        query
            .query_map(params![mono_id, conversation, site], |row| {
                row.get::<_, String>(0)
            })
            .map(|rows| {
                rows.flatten()
                    .filter_map(|id| Category::parse(&id))
                    .filter(|c| !never_granted(*c))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Grants of a Mono (one conversation, or all of them), as `(site, category)`.
    pub fn list_grants(&self, mono_id: &str, conversation: Option<i64>) -> Vec<(String, Category)> {
        let Ok(Some(conn)) = self.open() else {
            return Vec::new();
        };
        let Ok(mut query) = conn.prepare(
            "SELECT site,category FROM mono_browser_grants WHERE mono_id=?1 AND (?2 IS NULL OR conversation=?2) ORDER BY site,category",
        ) else {
            return Vec::new();
        };
        query
            .query_map(params![mono_id, conversation], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map(|rows| {
                rows.flatten()
                    .filter_map(|(site, id)| Some((site, Category::parse(&id)?)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Ends grants of a Mono. `conversation` and `site` narrow it; both `None` ends every grant of
    /// the Mono (the toolbar Stop, a change of the Mono's browser settings, a user revoke of all).
    pub fn revoke_grants(
        &self,
        mono_id: &str,
        conversation: Option<i64>,
        site: Option<&str>,
    ) -> Result<(), String> {
        Self::valid_id(mono_id)?;
        let Some(conn) = self.open()? else {
            return Ok(());
        };
        match conn.execute(
            "DELETE FROM mono_browser_grants WHERE mono_id=?1 AND (?2 IS NULL OR conversation=?2) AND (?3 IS NULL OR site=?3)",
            params![mono_id, conversation, site],
        ) {
            Ok(_) => Ok(()),
            Err(e) if e.to_string().contains("no such table") => Ok(()),
            Err(_) => Err("Could not end the browser grants.".into()),
        }
    }

    /// The rule lines the user last confirmed for this Mono (`None`: none yet).
    pub fn confirmed_rules(&self, mono_id: &str) -> Option<Vec<String>> {
        let Ok(Some(conn)) = self.open() else {
            return None;
        };
        let text: String = conn
            .query_row(
                "SELECT lines FROM mono_browser_rules WHERE mono_id=?1",
                [mono_id],
                |row| row.get(0),
            )
            .ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Stores the confirmed rule lines and their hash.
    pub fn confirm_rules(&self, mono_id: &str, lines: &[String]) -> Result<(), String> {
        Self::valid_id(mono_id)?;
        let conn = self.open()?.ok_or(UNAVAILABLE)?;
        conn.execute_batch(BROWSER_TABLES)
            .map_err(|_| UNAVAILABLE)?;
        let text = serde_json::to_string(lines).map_err(|_| UNAVAILABLE)?;
        let hash = crate::mono::files::content_hash(&lines.join("\n"));
        conn.execute(
            "INSERT OR REPLACE INTO mono_browser_rules(mono_id,lines,hash,confirmed_at) VALUES(?1,?2,?3,?4)",
            params![mono_id, text, hash, crate::time::now_ms() as i64],
        )
        .map(|_| ())
        .map_err(|_| UNAVAILABLE.into())
    }
}
