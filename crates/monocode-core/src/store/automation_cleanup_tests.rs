use super::*;
use rusqlite::Connection;
use std::path::Path;

const DB: &str = crate::brand::DATABASE_FILE;

fn backups(dir: &Path) -> Vec<PathBuf> {
    match std::fs::read_dir(dir.join("backups")) {
        Ok(entries) => entries.map(|entry| entry.unwrap().path()).collect(),
        Err(_) => Vec::new(),
    }
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn has_table(conn: &Connection, table: &str) -> bool {
    table_exists(conn, table)
}

/// Adds the retired tables, with fake rows, to an existing database file.
fn add_automation_tables(path: &Path) {
    let conn = Connection::open(path).unwrap();
    conn.execute_batch(
        "CREATE TABLE automations (id TEXT PRIMARY KEY, definition_json TEXT NOT NULL,
           enabled INTEGER NOT NULL, next_run_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
         CREATE TABLE automation_runs (id TEXT PRIMARY KEY,
           automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
           created_at INTEGER NOT NULL, run_json TEXT NOT NULL);
         CREATE TABLE automation_event_claims (
           automation_id TEXT NOT NULL REFERENCES automations(id) ON DELETE CASCADE,
           event_key TEXT NOT NULL, created_at INTEGER NOT NULL,
           PRIMARY KEY (automation_id, event_key));
         INSERT INTO automations VALUES ('a1','{\"name\":\"fake one\"}',1,10,11);
         INSERT INTO automations VALUES ('a2','{\"name\":\"fake two\"}',0,20,21);
         INSERT INTO automation_runs VALUES ('r1','a1',5,'{\"state\":\"done\"}');
         INSERT INTO automation_event_claims VALUES ('a1','github:issue:1',6);",
    )
    .unwrap();
}

/// A brand database with two sessions, a setting and the retired tables.
fn legacy_database(dir: &Path) -> PathBuf {
    let path = dir.join(DB);
    let store = Store::open_monocode(&path).unwrap();
    store.create("codex", "/project-one").unwrap();
    store.create("codex", "/project-two").unwrap();
    store.save_setting("keep-me", "yes").unwrap();
    store
        .save_setting(RETRY_SETTING, "{\"fake\":true}")
        .unwrap();
    drop(store);
    add_automation_tables(&path);
    path
}

#[test]
fn the_rows_are_backed_up_and_the_tables_and_settings_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = legacy_database(dir.path());
    let store = Store::open_monocode(&path).unwrap();

    let files = backups(dir.path());
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("automations-") && name.ends_with(".json"));
    let backup: Value = serde_json::from_slice(&std::fs::read(&files[0]).unwrap()).unwrap();
    let tables = &backup["tables"];
    assert_eq!(tables["automations"].as_array().unwrap().len(), 2);
    assert_eq!(tables["automations"][0]["id"], "a1");
    assert_eq!(tables["automations"][1]["enabled"], 0);
    assert_eq!(
        tables["automation_runs"][0]["run_json"],
        "{\"state\":\"done\"}"
    );
    assert_eq!(
        tables["automation_event_claims"][0]["event_key"],
        "github:issue:1"
    );
    assert_eq!(backup["settings"][RETRY_SETTING], "{\"fake\":true}");

    let conn = Connection::open(&path).unwrap();
    for table in TABLES {
        assert!(!has_table(&conn, table), "{table} still exists");
    }
    assert_eq!(store.setting(RETRY_SETTING).unwrap(), None);
    assert_eq!(store.setting("keep-me").unwrap().as_deref(), Some("yes"));
    assert_eq!(count(&conn, "sessions"), 2);
    assert_eq!(store.sessions().unwrap().len(), 2);
}

#[test]
fn a_second_open_changes_nothing_and_writes_no_second_backup() {
    let dir = tempfile::tempdir().unwrap();
    let path = legacy_database(dir.path());
    drop(Store::open_monocode(&path).unwrap());
    let before = backups(dir.path());
    std::thread::sleep(std::time::Duration::from_millis(5));
    drop(Store::open_monocode(&path).unwrap());
    assert_eq!(backups(dir.path()), before);
}

#[test]
fn a_failed_export_leaves_everything_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = legacy_database(dir.path());
    // A file where the backups folder should be makes the export fail.
    std::fs::write(dir.path().join("backups"), "not a folder").unwrap();
    let store = Store::open_monocode(&path).unwrap();
    let conn = Connection::open(&path).unwrap();
    assert_eq!(count(&conn, "automations"), 2);
    assert_eq!(count(&conn, "automation_runs"), 1);
    assert_eq!(count(&conn, "automation_event_claims"), 1);
    assert!(store.setting(RETRY_SETTING).unwrap().is_some());
    assert_eq!(count(&conn, "sessions"), 2);
}

#[test]
fn a_read_only_open_changes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let path = legacy_database(dir.path());
    let store = Store::read_monocode(&path).unwrap();
    assert!(store.setting(RETRY_SETTING).unwrap().is_some());
    drop(store);
    let conn = Connection::open(&path).unwrap();
    assert_eq!(count(&conn, "automations"), 2);
    assert!(backups(dir.path()).is_empty());
}

#[test]
fn a_database_that_is_not_the_brand_file_is_never_touched() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("other.db");
    drop(Store::open_monocode(&path).unwrap());
    add_automation_tables(&path);
    drop(Store::open_monocode(&path).unwrap());
    let conn = Connection::open(&path).unwrap();
    assert_eq!(count(&conn, "automations"), 2);
    assert!(backups(dir.path()).is_empty());
}

#[test]
fn the_prototype_store_drops_the_tables_and_the_automation_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(DB);
    let store = Store::open(&path).unwrap();
    let session = store.create("codex", "/project").unwrap();
    store.save_setting("automation:1", "fake").unwrap();
    store.save_setting(RETRY_SETTING, "{}").unwrap();
    store.save_setting("keep-me", "yes").unwrap();
    drop(store);
    add_automation_tables(&path);
    let store = Store::open(&path).unwrap();
    assert_eq!(backups(dir.path()).len(), 1);
    assert_eq!(store.setting("automation:1").unwrap(), None);
    assert_eq!(store.setting(RETRY_SETTING).unwrap(), None);
    assert_eq!(store.setting("keep-me").unwrap().as_deref(), Some("yes"));
    assert!(!has_table(&Connection::open(&path).unwrap(), "automations"));
    assert!(store.sessions().unwrap().iter().any(|s| s.id == session.id));
}
