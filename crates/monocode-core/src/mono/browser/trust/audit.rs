//! The decision audit log (spec 8.5): one row per gate decision, with the columns the extension
//! backend adds, and the retention rule (90 days or 20,000 rows per Mono).
//!
//! Typed values are never stored: a row keeps the field kind and the length only
//! ([`typed_summary`]).

use super::super::{MonoBrowserStore, UNAVAILABLE};
use rusqlite::{params, Connection};

/// Rows older than this are deleted (90 days).
pub const AUDIT_MAX_AGE_MS: u64 = 90 * 24 * 60 * 60 * 1000;
/// Rows kept per Mono, newest first.
pub const AUDIT_MAX_ROWS: i64 = 20_000;

/// The columns spec 8.5 adds to `mono_browser_audit`. All nullable.
const NEW_COLUMNS: [(&str, &str); 11] = [
    ("tab_id", "INTEGER"),
    ("tab_owner", "TEXT"),
    ("tier", "TEXT"),
    ("categories", "TEXT"),
    ("decision", "TEXT"),
    ("reason", "TEXT"),
    ("approval_id", "TEXT"),
    ("screenshot", "TEXT"),
    ("actor", "TEXT"),
    ("input", "TEXT"),
    ("profile", "TEXT"),
];

/// Adds the spec 8.5 columns to an existing audit table. Idempotent.
pub(crate) fn migrate_audit_columns(conn: &Connection) -> rusqlite::Result<()> {
    let mut have = std::collections::HashSet::new();
    {
        let mut query = conn.prepare("PRAGMA table_info(mono_browser_audit)")?;
        let names = query.query_map([], |row| row.get::<_, String>(1))?;
        for name in names.flatten() {
            have.insert(name);
        }
    }
    if have.is_empty() {
        return Ok(());
    }
    for (name, ty) in NEW_COLUMNS {
        if have.contains(name) {
            continue;
        }
        match conn.execute(
            &format!("ALTER TABLE mono_browser_audit ADD COLUMN {name} {ty}"),
            [],
        ) {
            Ok(_) => {}
            // Another process added it first.
            Err(e) if e.to_string().contains("duplicate column") => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// How the gate decided (spec 8.5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditDecision {
    Auto,
    AskedAllowedOnce,
    AskedAllowedTask,
    AskedDenied,
    AskedTimeout,
    Blocked,
    Invariant,
    Stopped,
}

impl AuditDecision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::AskedAllowedOnce => "asked_allowed_once",
            Self::AskedAllowedTask => "asked_allowed_task",
            Self::AskedDenied => "asked_denied",
            Self::AskedTimeout => "asked_timeout",
            Self::Blocked => "blocked",
            Self::Invariant => "invariant",
            Self::Stopped => "stopped",
        }
    }
}

/// One audit row of the extension backend. `url` is origin plus path (`gate::audit_url`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionAudit {
    pub ts_ms: u64,
    pub session_id: i64,
    pub tool: String,
    pub url: String,
    pub summary: String,
    pub tab_id: Option<i64>,
    pub tab_owner: Option<String>,
    /// `R`, `A` or `K`.
    pub tier: String,
    /// Category ids, comma separated in the table.
    pub categories: Vec<String>,
    pub decision: AuditDecision,
    pub reason: String,
    pub approval_id: Option<String>,
    pub screenshot: Option<String>,
    /// `mono`, `recipe`, `auto_consent` or `user_stop`.
    pub actor: String,
    /// `content` or `debugger`.
    pub input: Option<String>,
    pub profile: Option<String>,
}

/// "typed 14 chars into Email": the only record of a typed value.
pub fn typed_summary(field_label: &str, value: &str) -> String {
    let label: String = field_label.trim().chars().take(60).collect();
    format!("typed {} chars into {label}", value.chars().count())
}

/// Deletes the rows past retention for one Mono: older than [`AUDIT_MAX_AGE_MS`], then everything
/// beyond the newest [`AUDIT_MAX_ROWS`]. Returns the screenshot file names of the deleted rows, so
/// the caller can remove them from the output dir.
pub(crate) fn prune_audit(
    conn: &Connection,
    mono_id: &str,
    now_ms: u64,
) -> rusqlite::Result<Vec<String>> {
    prune_audit_with(conn, mono_id, now_ms, AUDIT_MAX_AGE_MS, AUDIT_MAX_ROWS)
}

pub(crate) fn prune_audit_with(
    conn: &Connection,
    mono_id: &str,
    now_ms: u64,
    max_age_ms: u64,
    max_rows: i64,
) -> rusqlite::Result<Vec<String>> {
    let cutoff = now_ms.saturating_sub(max_age_ms) as i64;
    // The id past which rows are too many: the (max_rows + 1)th newest.
    let overflow: Option<i64> = conn
        .query_row(
            "SELECT id FROM mono_browser_audit WHERE mono_id=?1 ORDER BY id DESC LIMIT 1 OFFSET ?2",
            params![mono_id, max_rows],
            |row| row.get(0),
        )
        .ok();
    let overflow = overflow.unwrap_or(0);
    let mut shots = Vec::new();
    {
        let mut query = conn.prepare(
            "SELECT screenshot FROM mono_browser_audit
             WHERE mono_id=?1 AND (ts<?2 OR id<=?3) AND screenshot IS NOT NULL",
        )?;
        let rows = query.query_map(params![mono_id, cutoff, overflow], |row| {
            row.get::<_, String>(0)
        })?;
        shots.extend(rows.flatten());
    }
    conn.execute(
        "DELETE FROM mono_browser_audit WHERE mono_id=?1 AND (ts<?2 OR id<=?3)",
        params![mono_id, cutoff, overflow],
    )?;
    Ok(shots)
}

/// A decision row read back (the Activity view and the tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionRow {
    pub ts_ms: u64,
    pub tool: String,
    pub tier: Option<String>,
    pub categories: Vec<String>,
    pub decision: Option<String>,
    pub reason: Option<String>,
    pub actor: Option<String>,
}

impl MonoBrowserStore {
    /// Writes one decision row and applies retention. Returns the screenshot files of pruned rows.
    pub fn record_decision(
        &self,
        mono_id: &str,
        row: &DecisionAudit,
    ) -> Result<Vec<String>, String> {
        Self::valid_id(mono_id)?;
        let conn = self.open()?.ok_or(UNAVAILABLE)?;
        conn.execute_batch(super::super::BROWSER_TABLES)
            .map_err(|_| UNAVAILABLE)?;
        migrate_audit_columns(&conn).map_err(|_| UNAVAILABLE)?;
        conn.execute(
            "INSERT INTO mono_browser_audit(mono_id,session_id,ts,tool,url,summary,tab_id,tab_owner,tier,categories,decision,reason,approval_id,screenshot,actor,input,profile)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)",
            params![
                mono_id,
                row.session_id,
                row.ts_ms as i64,
                row.tool,
                row.url,
                row.summary.chars().take(200).collect::<String>(),
                row.tab_id,
                row.tab_owner,
                row.tier,
                row.categories.join(","),
                row.decision.as_str(),
                row.reason.chars().take(200).collect::<String>(),
                row.approval_id,
                row.screenshot,
                row.actor,
                row.input,
                row.profile,
            ],
        )
        .map_err(|_| UNAVAILABLE)?;
        prune_audit(&conn, mono_id, row.ts_ms).map_err(|_| UNAVAILABLE.to_owned())
    }

    /// Applies retention now (the Settings view can call it when it opens).
    pub fn prune_audit(&self, mono_id: &str, now_ms: u64) -> Result<Vec<String>, String> {
        Self::valid_id(mono_id)?;
        let Some(conn) = self.open()? else {
            return Ok(Vec::new());
        };
        prune_audit(&conn, mono_id, now_ms).map_err(|_| UNAVAILABLE.to_owned())
    }

    /// The newest `limit` decision rows, newest first.
    pub fn recent_decisions(&self, mono_id: &str, limit: usize) -> Vec<DecisionRow> {
        let Ok(Some(conn)) = self.open() else {
            return Vec::new();
        };
        if migrate_audit_columns(&conn).is_err() {
            return Vec::new();
        }
        let Ok(mut query) = conn.prepare(
            "SELECT ts,tool,tier,categories,decision,reason,actor FROM mono_browser_audit WHERE mono_id=?1 ORDER BY id DESC LIMIT ?2",
        ) else {
            return Vec::new();
        };
        query
            .query_map(params![mono_id, limit as i64], |row| {
                let categories: Option<String> = row.get(3)?;
                Ok(DecisionRow {
                    ts_ms: row.get::<_, i64>(0)?.max(0) as u64,
                    tool: row.get(1)?,
                    tier: row.get(2)?,
                    categories: categories
                        .unwrap_or_default()
                        .split(',')
                        .filter(|c| !c.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    decision: row.get(4)?,
                    reason: row.get(5)?,
                    actor: row.get(6)?,
                })
            })
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }
}
