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

/// Creates the backups folder, readable by the owner only.
fn create_backups_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)
}

/// Writes `bytes` to `path` through a temporary file, so a crash never leaves a half-written
/// backup under the final name. The folder is synced too, so the rename survives a power cut.
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
        .and_then(|_| std::fs::rename(&temporary, path))
        .and_then(|_| {
            #[cfg(unix)]
            if let Some(folder) = path.parent() {
                std::fs::File::open(folder)?.sync_all()?;
            }
            Ok(())
        });
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

/// Removes `file` when another backup in its folder has exactly the same bytes.
fn remove_if_duplicate(file: &Path, bytes: &[u8]) {
    let Some(folder) = file.parent() else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    let duplicate = entries.flatten().map(|entry| entry.path()).any(|other| {
        other != file
            && other.extension().is_some_and(|ext| ext == "json")
            && std::fs::read(&other).is_ok_and(|existing| existing == bytes)
    });
    if duplicate {
        let _ = std::fs::remove_file(file);
    }
}

impl Store {
    /// What the retired-data cleanup did or could not do, one line each. The open runs before the
    /// app log exists, so the app takes these once the log is up.
    pub fn take_cleanup_notes(&self) -> Vec<String> {
        std::mem::take(&mut *self.cleanup_notes.borrow_mut())
    }

    fn cleanup_note(&self, note: String) {
        eprintln!("{note}");
        self.cleanup_notes.borrow_mut().push(note);
    }

    /// Exports and drops the retired automation tables of the brand database, and clears their
    /// settings on every writable open. Never fails the
    /// open: a problem is noted and leaves every table in place for the next open.
    pub(super) fn drop_automation_data(&self, shared: bool) {
        let path = self.database_path();
        let is_brand_db =
            path.file_name().and_then(|name| name.to_str()) == Some(crate::brand::DATABASE_FILE);
        if !is_brand_db {
            return;
        }
        if !TABLES.iter().any(|table| table_exists(&self.conn, table)) {
            // The tables are gone and their rows are in the backup. Settings a failed delete
            // left behind are removed now, without a new backup.
            self.clear_settings(&self.settings_to_clear(shared));
            return;
        }
        match self.export_and_drop(shared, path) {
            Ok(summary) => {
                self.cleanup_note(format!("Removed the retired Automations data. {summary}"))
            }
            Err(error) => self.cleanup_note(format!("Kept the retired Automations data: {error}")),
        }
    }

    fn clear_settings(&self, settings: &[(String, String)]) {
        for (key, _) in settings {
            if let Err(error) = self.delete_setting(key) {
                self.cleanup_note(format!("Could not remove the setting {key}: {error}"));
            }
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

    /// Holds the write lock from the export to the drop, so no row can arrive in between.
    fn export_and_drop(&self, shared: bool, path: &Path) -> Result<String, String> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| e.to_string())?;
        match self.export_and_drop_locked(shared, path) {
            Ok(summary) => Ok(summary),
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    fn export_and_drop_locked(&self, shared: bool, path: &Path) -> Result<String, String> {
        let settings = self.settings_to_clear(shared);
        let mut tables = Map::new();
        let mut counts = Vec::new();
        let mut total = 0;
        for table in TABLES.iter().rev() {
            if table_exists(&self.conn, table) {
                let rows = export_table(&self.conn, table)?;
                let len = rows.as_array().map_or(0, |rows| rows.len());
                total += len;
                counts.push(format!("{table}: {len}"));
                tables.insert((*table).to_string(), rows);
            }
        }
        let mut backup = None;
        if total > 0 || !settings.is_empty() {
            let backups: PathBuf = path
                .parent()
                .ok_or("the database has no folder")?
                .join("backups");
            create_backups_dir(&backups)
                .map_err(|e| format!("cannot create {}: {e}", backups.display()))?;
            let file = backups.join(format!(
                "automations-{}.json",
                chrono::Utc::now().format("%Y%m%dT%H%M%S%3fZ")
            ));
            let document = json!({
                "format": 1,
                "tables": tables,
                "settings": settings.iter().cloned().collect::<std::collections::BTreeMap<_, _>>(),
            });
            let bytes = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
            write_atomically(&file, &bytes)
                .map_err(|e| format!("cannot write {}: {e}", file.display()))?;
            backup = Some((file, bytes));
        }

        let dropped = TABLES
            .iter()
            .try_for_each(|table| {
                self.conn
                    .execute_batch(&format!("DROP TABLE IF EXISTS {table}"))
            })
            .and_then(|_| self.conn.execute_batch("COMMIT"));
        if let Err(error) = dropped {
            // Not dropped: the next open exports again, so a copy of an earlier backup is noise.
            if let Some((file, bytes)) = &backup {
                remove_if_duplicate(file, bytes);
            }
            return Err(error.to_string());
        }
        self.clear_settings(&settings);
        Ok(match backup {
            Some((file, _)) => format!("Rows saved to {} ({}).", file.display(), counts.join(", ")),
            None => "The tables held no rows, so no backup was written.".to_string(),
        })
    }
}

#[cfg(test)]
#[path = "automation_cleanup_tests.rs"]
mod tests;
