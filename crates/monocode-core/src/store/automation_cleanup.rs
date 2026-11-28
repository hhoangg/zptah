//! One-time removal of the retired Automations data. The three automation tables
//! live in the session database. Their rows are exported to a JSON file in the app
//! data directory first, and only then are the tables dropped, so nothing is lost.
use super::Store;
use rusqlite::{types::ValueRef, Connection};
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

/// Children first, so the drop never meets a live foreign key.
const TABLES: [&str; 3] = ["automation_event_claims", "automation_runs", "automations"];
const RETRY_SETTING: &str = "native-automation-inbox-retries";

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
        [name],
        |_| Ok(()),
    )
    .is_ok()
}

fn value_json(value: ValueRef) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => json!(n),
        ValueRef::Real(n) => json!(n),
        ValueRef::Text(text) => Value::String(String::from_utf8_lossy(text).into_owned()),
        ValueRef::Blob(bytes) => {
            Value::String(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
        }
    }
}

fn export_table(conn: &Connection, table: &str) -> Result<Value, String> {
    let mut statement = conn
        .prepare(&format!("SELECT * FROM {table}"))
        .map_err(|e| e.to_string())?;
    let names: Vec<String> = statement
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    let mut rows = statement.query([]).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().map_err(|e| e.to_string())? {
        let mut object = Map::new();
        for (index, name) in names.iter().enumerate() {
            object.insert(
                name.clone(),
                value_json(row.get_ref(index).map_err(|e| e.to_string())?),
            );
        }
        out.push(Value::Object(object));
    }
    Ok(Value::Array(out))
}

/// Writes `bytes` to `path` through a temporary file, so a crash never leaves a half-written
/// backup under the final name.
fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let temporary = path.with_extension("json.tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = options
        .open(&temporary)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .and_then(|_| std::fs::rename(&temporary, path));
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

impl Store {
    /// Exports and drops the retired automation tables of the brand database. Never fails the
    /// open: a problem is logged and leaves every table in place for the next open.
    pub(super) fn drop_automation_data(&self, shared: bool) {
        let path = self.database_path();
        let is_brand_db =
            path.file_name().and_then(|name| name.to_str()) == Some(crate::brand::DATABASE_FILE);
        if !is_brand_db || !TABLES.iter().any(|table| table_exists(&self.conn, table)) {
            return;
        }
        match self.export_and_drop(shared, path) {
            Ok(summary) => eprintln!("Removed the retired Automations data. {summary}"),
            Err(error) => eprintln!("Kept the retired Automations data: {error}"),
        }
    }

    fn settings_to_clear(&self, shared: bool) -> Vec<(String, String)> {
        let mut keys = vec![RETRY_SETTING.to_string()];
        if shared {
            keys.extend(
                self.shared_settings_with_prefix("automation:")
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(id, _)| format!("automation:{id}")),
            );
        } else if let Ok(mut query) = self
            .conn
            .prepare("SELECT key FROM native_settings WHERE key LIKE 'automation:%'")
        {
            if let Ok(rows) = query.query_map([], |row| row.get::<_, String>(0)) {
                keys.extend(rows.flatten());
            }
        }
        keys.into_iter()
            .filter_map(|key| {
                let value = self.setting(&key).ok().flatten()?;
                Some((key, value))
            })
            .collect()
    }

    fn export_and_drop(&self, shared: bool, path: &Path) -> Result<String, String> {
        let settings = self.settings_to_clear(shared);
        let mut tables = Map::new();
        let mut counts = Vec::new();
        for table in TABLES.iter().rev() {
            if table_exists(&self.conn, table) {
                let rows = export_table(&self.conn, table)?;
                counts.push(format!(
                    "{table}: {}",
                    rows.as_array().map_or(0, |rows| rows.len())
                ));
                tables.insert((*table).to_string(), rows);
            }
        }
        let now = chrono::Utc::now();
        let backups: PathBuf = path
            .parent()
            .ok_or("the database has no folder")?
            .join("backups");
        std::fs::create_dir_all(&backups)
            .map_err(|e| format!("cannot create {}: {e}", backups.display()))?;
        let file = backups.join(format!(
            "automations-{}.json",
            now.format("%Y%m%dT%H%M%S%3fZ")
        ));
        let document = json!({
            "format": 1,
            "exportedAt": now.to_rfc3339(),
            "tables": tables,
            "settings": settings.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
        });
        let bytes = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
        write_atomically(&file, &bytes)
            .map_err(|e| format!("cannot write {}: {e}", file.display()))?;

        let transaction = self
            .conn
            .unchecked_transaction()
            .map_err(|e| e.to_string())?;
        for table in TABLES {
            transaction
                .execute_batch(&format!("DROP TABLE IF EXISTS {table}"))
                .map_err(|e| e.to_string())?;
        }
        transaction.commit().map_err(|e| e.to_string())?;
        for (key, _) in &settings {
            if let Err(error) = self.delete_setting(key) {
                eprintln!("Could not remove the setting {key}: {error}");
            }
        }
        Ok(format!(
            "Rows saved to {} ({}).",
            file.display(),
            counts.join(", ")
        ))
    }
}

#[cfg(test)]
#[path = "automation_cleanup_tests.rs"]
mod tests;
