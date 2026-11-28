//! Dedup ledger for workflow event triggers. A claim is taken atomically BEFORE
//! a run is created, so one Inbox item starts at most one run per workflow even
//! when two windows or two polls race. The table lives in the session database
//! and is created only on writable opens.
use super::Store;
use rusqlite::{params, Connection, TransactionBehavior};

pub(super) fn ensure_workflow_claims_table(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS workflow_event_claims (
           workflow_id TEXT NOT NULL,
           trigger_node TEXT NOT NULL,
           event_key TEXT NOT NULL,
           created_at INTEGER NOT NULL,
           run_id TEXT,
           PRIMARY KEY (workflow_id, trigger_node, event_key)
         );",
    )
}

/// A claim that has no run yet: the app stopped between claiming and creating the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkflowClaim {
    pub workflow_id: String,
    pub trigger_node: String,
    pub event_key: String,
    pub created_at: i64,
}

fn sql<T>(result: rusqlite::Result<T>) -> Result<T, String> {
    result.map_err(|e| e.to_string())
}

impl Store {
    /// Creates the table on the first write that needs it, so opening an existing database never
    /// changes it.
    fn ensure_claims(&self) -> Result<(), String> {
        sql(ensure_workflow_claims_table(&self.conn))
    }

    fn has_claims_table(&self) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'workflow_event_claims'",
                [],
                |_| Ok(()),
            )
            .is_ok()
    }

    /// Claim `event_key` for one trigger node. True only when this call inserted
    /// the row. Does not look at other trigger nodes of the workflow.
    pub fn claim_workflow_event(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
    ) -> Result<bool, String> {
        self.claim_workflow(workflow_id, trigger_node, event_key, false)
    }

    /// Claim `event_key` for the whole workflow: one run per workflow per item,
    /// so a second trigger node of the same workflow cannot claim it too.
    pub fn claim_workflow_item(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
    ) -> Result<bool, String> {
        self.claim_workflow(workflow_id, trigger_node, event_key, true)
    }

    fn claim_workflow(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
        per_workflow: bool,
    ) -> Result<bool, String> {
        self.ensure_claims()?;
        let tx = sql(rusqlite::Transaction::new_unchecked(
            &self.conn,
            TransactionBehavior::Immediate,
        ))?;
        if per_workflow {
            let taken: bool = sql(tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM workflow_event_claims
                   WHERE workflow_id = ?1 AND event_key = ?2)",
                params![workflow_id, event_key],
                |row| row.get(0),
            ))?;
            if taken {
                return Ok(false);
            }
        }
        let now = crate::time::now_ms().min(i64::MAX as u64) as i64;
        let changed = sql(tx.execute(
            "INSERT OR IGNORE INTO workflow_event_claims
               (workflow_id, trigger_node, event_key, created_at, run_id)
             VALUES (?1, ?2, ?3, ?4, NULL)",
            params![workflow_id, trigger_node, event_key, now],
        ))?;
        sql(tx.commit())?;
        Ok(changed == 1)
    }

    /// Record the run a claim started.
    pub fn set_workflow_claim_run(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
        run_id: &str,
    ) -> Result<(), String> {
        self.ensure_claims()?;
        sql(self.conn.execute(
            "UPDATE workflow_event_claims SET run_id = ?4
             WHERE workflow_id = ?1 AND trigger_node = ?2 AND event_key = ?3",
            params![workflow_id, trigger_node, event_key, run_id],
        ))?;
        Ok(())
    }

    /// Give a claim back after the run could not be created, so a retry can take it.
    /// A claim that already has a run is kept.
    pub fn release_workflow_event(
        &self,
        workflow_id: &str,
        trigger_node: &str,
        event_key: &str,
    ) -> Result<(), String> {
        if !self.has_claims_table() {
            return Ok(());
        }
        sql(self.conn.execute(
            "DELETE FROM workflow_event_claims
             WHERE workflow_id = ?1 AND trigger_node = ?2 AND event_key = ?3
               AND run_id IS NULL",
            params![workflow_id, trigger_node, event_key],
        ))?;
        Ok(())
    }

    /// Claims with no run, oldest first. For the startup check.
    pub fn unclaimed_workflow_claims(&self) -> Result<Vec<WorkflowClaim>, String> {
        if !self.has_claims_table() {
            return Ok(Vec::new());
        }
        let mut statement = sql(self.conn.prepare(
            "SELECT workflow_id, trigger_node, event_key, created_at
             FROM workflow_event_claims WHERE run_id IS NULL
             ORDER BY created_at, workflow_id, trigger_node, event_key",
        ))?;
        let rows = sql(statement.query_map([], |row| {
            Ok(WorkflowClaim {
                workflow_id: row.get(0)?,
                trigger_node: row.get(1)?,
                event_key: row.get(2)?,
                created_at: row.get(3)?,
            })
        }))?;
        rows.collect::<rusqlite::Result<_>>()
            .map_err(|e| e.to_string())
    }

    /// Drop every claim of a deleted workflow.
    pub fn forget_workflow_claims(&self, workflow_id: &str) -> Result<(), String> {
        if !self.has_claims_table() {
            return Ok(());
        }
        sql(self.conn.execute(
            "DELETE FROM workflow_event_claims WHERE workflow_id = ?1",
            [workflow_id],
        ))?;
        Ok(())
    }

    #[cfg(test)]
    fn workflow_claim_run(&self, workflow_id: &str, event_key: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT run_id FROM workflow_event_claims
                 WHERE workflow_id = ?1 AND event_key = ?2",
                params![workflow_id, event_key],
                |row| row.get(0),
            )
            .ok()
            .flatten()
    }
}

#[cfg(test)]
#[path = "workflow_claims_tests.rs"]
mod tests;
