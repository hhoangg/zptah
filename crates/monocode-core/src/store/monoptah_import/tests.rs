//! Every test uses temporary folders only; the real Monoptah and zptah data are never opened.
use super::*;
use rusqlite::params;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

struct Probe(Result<Vec<String>, String>);
impl ProcessProbe for Probe {
    fn commands(&self) -> Result<Vec<String>, String> {
        self.0.clone()
    }
}
fn quiet() -> Probe {
    Probe(Ok(vec![
        "/Applications/zptah.app/Contents/MacOS/monocode-gpui".into(),
        "/usr/bin/vim notes.md".into(),
    ]))
}

const KEY: [u8; 32] = [7; 32];

struct Fixture {
    _root: tempfile::TempDir,
    source: PathBuf,
    target: PathBuf,
    target_db: PathBuf,
    locations: Locations,
    dev_source: PathBuf,
    dev_target: PathBuf,
    webkit_target: PathBuf,
    /// Keeps the source database open so its WAL still holds data (Monoptah "crashed").
    _live: Connection,
}

fn seal(aad: &[u8], text: &str) -> (Vec<u8>, Vec<u8>) {
    let (nonce, ciphertext) = sealed::encrypt(&KEY, aad, text.as_bytes()).unwrap();
    (nonce.to_vec(), ciphertext)
}

fn write_private(path: &Path, bytes: &[u8], mode: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
    let _ = mode;
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    // The real folder has a space in its path (`Application Support`).
    let source = root
        .path()
        .join("Application Support")
        .join("com.monoptah.desktop");
    let target = root
        .path()
        .join("Application Support")
        .join("app.zptah.desktop");
    fs::create_dir_all(&source).unwrap();
    fs::create_dir_all(&target).unwrap();
    let old = source.to_string_lossy().into_owned();
    let db = source.join("monocode.db");
    let live = Connection::open(&db).unwrap();
    live.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    live.execute_batch(include_str!("../shared_schema.sql"))
        .unwrap();
    live.execute(
        "INSERT INTO sessions(id,cwd,harness,model,runtime_mode,title,blocks_json,created_at,updated_at,\
         has_user_message,queued_messages_json) VALUES('s1','/work/app','claude','m','full','First',?1,1,2,1,?2)",
        params![
            format!(r#"[{{"image":"{old}/monocode.db.threads/s1/attachments/a.png","other":"{old}.dev/keep"}}]"#),
            format!(r#"[{{"attachments":["{old}/composer-messages/1/ab/0/b.png"]}}]"#),
        ],
    )
    .unwrap();
    // Sealed rows of every kind, bound to ids and names, never to a path.
    live.execute_batch(crate::accounts::CREDENTIALS_TABLE)
        .unwrap();
    live.execute_batch(crate::git_credentials::GIT_CREDENTIALS_TABLE)
        .unwrap();
    live.execute_batch(crate::docker_accounts::DOCKER_ACCOUNTS_TABLE)
        .unwrap();
    live.execute_batch(crate::mono::browser::BROWSER_TABLES)
        .unwrap();
    live.execute_batch(crate::mobile::identity_store::MOBILE_IDENTITY_TABLE)
        .unwrap();
    let (nonce, ct) = seal(
        &crate::accounts::stored_associated_data("work", "setup-token").unwrap(),
        "sk-ant-oat01-fake",
    );
    live.execute(
        "INSERT INTO provider_account_credentials VALUES('claude','work','setup-token',?1,?2)",
        params![nonce, ct],
    )
    .unwrap();
    let (nonce, ct) = seal(
        &crate::git_credentials::associated_data("g1", "https://github.com", "me"),
        "fake-git",
    );
    live.execute(
        "INSERT INTO git_credentials VALUES('g1','Git','github','https://github.com','me',?1,?2,1)",
        params![nonce, ct],
    )
    .unwrap();
    let (nonce, ct) = seal(&crate::docker_accounts::associated_data("d1", "me"), "fake");
    live.execute(
        "INSERT INTO docker_accounts VALUES('d1','me',?1,?2,'ok',NULL,NULL,1)",
        params![nonce, ct],
    )
    .unwrap();
    let (nonce, ct) = seal(&crate::mono::browser::secret_aad("m1", "token"), "fake");
    live.execute(
        "INSERT INTO mono_secrets VALUES('m1','token',?1,?2,1)",
        params![nonce, ct],
    )
    .unwrap();
    for account in [format!("desktop:{old}/mobile-access.json"), "web:x".into()] {
        live.execute(
            "INSERT INTO mobile_identity VALUES(?1,x'00',x'00')",
            [account],
        )
        .unwrap();
    }
    // The settings sidecar keeps its own copy of the sealed tables and a path in a value.
    let gpui = Connection::open(source.join("monocode.db.gpui.sqlite3")).unwrap();
    gpui.execute_batch(
        "PRAGMA journal_mode=WAL; CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    )
    .unwrap();
    gpui.execute(
        "INSERT INTO settings VALUES('follow-up:1',?1)",
        [format!(
            r#"{{"attachments":["{old}/composer-messages/1/x.png"]}}"#
        )],
    )
    .unwrap();
    gpui.execute_batch(crate::mobile::identity_store::MOBILE_IDENTITY_TABLE)
        .unwrap();
    gpui.execute(
        "INSERT INTO mobile_identity VALUES('web:y',x'00',x'00')",
        [],
    )
    .unwrap();
    drop(gpui);
    let search = Connection::open(source.join("monocode.db.search.sqlite3")).unwrap();
    search
        .execute_batch("CREATE TABLE docs(id TEXT); INSERT INTO docs VALUES('x');")
        .unwrap();
    drop(search);
    write_private(&source.join("credentials.key"), &KEY, 0o600);
    write_private(
        &source.join("monocode.db.threads/s1/attachments/a.png"),
        b"png",
        0o644,
    );
    write_private(
        &source.join("provider-accounts/claude/work/.claude.json"),
        b"{}",
        0o600,
    );
    write_private(&source.join("monos/m1/SOUL.md"), b"soul", 0o644);
    write_private(&source.join("monos/m1/.DS_Store"), b"junk", 0o644);
    write_private(
        &source.join("remote-machines.json"),
        format!(r#"{{"cache":"{old}/emoji"}}"#).as_bytes(),
        0o600,
    );
    write_private(&source.join("clickup-config.json"), b"{}", 0o600);
    for skipped in [
        "mobile-access.json",
        "mobile-policy.json",
        "mobile-audit.jsonl",
        "file-transfer-keys.json",
        "kit-hash.key",
        "tui-hooks-native/1.json",
        "hang/ui-hang.txt",
        "monocode.lock",
    ] {
        write_private(&source.join(skipped), b"private", 0o600);
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hosts", source.join("monos/m1/link")).unwrap();
    // Text files of any kind get their paths rewritten; binary ones do not.
    write_private(
        &source.join("monos/m1/notes.toml"),
        format!("dir = \"{old}/monos/m1\"\n").as_bytes(),
        0o644,
    );
    let mut binary = b"\0\0\0".to_vec();
    binary.extend(format!("{old}/monos").as_bytes());
    write_private(&source.join("monos/m1/blob.bin"), &binary, 0o644);
    // The development folder, where logos and backgrounds live.
    let base = root.path().join("Application Support");
    let (dev_source, dev_target) = (
        base.join("Monoptah Development"),
        base.join("zptah Development"),
    );
    write_private(&dev_source.join("project-images/p.png"), b"img", 0o644);
    write_private(&dev_source.join("backgrounds/b.jpg"), b"bg", 0o644);
    write_private(&dev_source.join("sounds/finished.wav"), b"snd", 0o644);
    let gpui = Connection::open(source.join("monocode.db.gpui.sqlite3")).unwrap();
    gpui.execute(
        "INSERT INTO settings VALUES('logo',?1)",
        [format!("{}/project-images/p.png", dev_source.display())],
    )
    .unwrap();
    drop(gpui);
    // The desktop preferences in the webview storage.
    let webkit_source = root.path().join("webkit-monoptah");
    let webkit_target = root.path().join("webkit-zptah");
    let origin = webkit_source.join("aa/bb");
    fs::create_dir_all(origin.join("LocalStorage")).unwrap();
    let mut bytes = Vec::new();
    for value in ["tauri", "localhost"] {
        bytes.extend((value.len() as u32).to_le_bytes());
        bytes.push(1);
        bytes.extend(value.as_bytes());
    }
    fs::write(origin.join("origin"), bytes).unwrap();
    let ls = Connection::open(origin.join("LocalStorage/localstorage.sqlite3")).unwrap();
    ls.execute_batch("CREATE TABLE ItemTable(key TEXT,value BLOB); INSERT INTO ItemTable VALUES('monocode.theme','dark');")
        .unwrap();
    drop(ls);
    Fixture {
        locations: Locations {
            development: Some((dev_source.clone(), dev_target.clone())),
            webkit: Some((webkit_source, webkit_target.clone())),
            webview2: None,
        },
        dev_source,
        dev_target,
        webkit_target,
        target_db: target.join("zptah.db"),
        _root: root,
        source,
        target,
        _live: live,
    }
}

/// Hash of every file in the source, including `-wal`, `-shm` and `-journal` files, so a stray
/// file created beside the source shows up.
fn snapshot(dir: &Path) -> BTreeMap<String, String> {
    fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
        for entry in fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            let meta = fs::symlink_metadata(&path).unwrap();
            let rel = path
                .strip_prefix(base)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            if meta.is_dir() {
                walk(base, &path, out);
            } else if meta.is_file() {
                let hash = Sha256::digest(fs::read(&path).unwrap());
                out.insert(rel, format!("{hash:x}"));
            } else {
                out.insert(rel, "link".into());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, dir, &mut out);
    out
}

/// Prepare the import (what the Settings page does).
fn prepare(f: &Fixture) -> Result<Outcome, String> {
    import_from_monoptah(&f.source, &f.target_db, &f.locations, &quiet(), "0.1.0")
}

/// Prepare it and restart: the move that happens at the next start.
fn run(f: &Fixture) -> Result<Outcome, String> {
    let outcome = prepare(f)?;
    if let Outcome::Staged(summary) = &outcome {
        let applied = apply_pending(&f.target)?.expect("an import was pending");
        assert_eq!(applied.databases, summary.databases);
    }
    Ok(outcome)
}

fn summary(outcome: Outcome) -> Summary {
    match outcome {
        Outcome::Staged(summary) => summary,
        Outcome::AlreadyImported => panic!("expected an import"),
    }
}

#[test]
fn the_page_is_hidden_without_monoptah_data_and_after_an_import() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("src");
    let target_db = root.path().join("zptah/zptah.db");
    assert_eq!(offer(&source, &target_db), Offer::Hidden);
    let f = fixture();
    assert_eq!(offer(&f.source, &f.target_db), Offer::Ready);
    summary(run(&f).unwrap());
    assert_eq!(offer(&f.source, &f.target_db), Offer::Hidden);
}

#[test]
fn the_page_is_disabled_with_a_reason_when_zptah_has_conversations() {
    let f = fixture();
    let conn = Connection::open(&f.target_db).unwrap();
    conn.execute_batch(include_str!("../shared_schema.sql"))
        .unwrap();
    // A chat that never got a message does not count.
    conn.execute(
        "INSERT INTO sessions(id,cwd,harness,model,runtime_mode,title,created_at,updated_at,has_user_message) \
         VALUES('e','/x','claude','m','full','New',1,1,0)",
        [],
    )
    .unwrap();
    assert_eq!(offer(&f.source, &f.target_db), Offer::Ready);
    conn.execute("UPDATE sessions SET has_user_message=1", [])
        .unwrap();
    drop(conn);
    assert!(matches!(offer(&f.source, &f.target_db), Offer::Blocked(why) if why.contains("empty")));
    let before = snapshot(&f.source);
    assert!(run(&f).unwrap_err().contains("already has conversations"));
    assert_eq!(snapshot(&f.source), before);
    assert!(!f.target.join(MARKER).exists());
}

#[test]
fn a_running_monoptah_refuses_the_import_and_changes_nothing() {
    let f = fixture();
    for command in [
        "/Applications/Monoptah Native.app/Contents/MacOS/monocode-gpui",
        "/Applications/Monoptah.app/Contents/MacOS/Monoptah",
        "/opt/monoptah-native/bin/monocode-gpui --x",
    ] {
        let probe = Probe(Ok(vec!["/bin/zsh".into(), command.into()]));
        let error = import_from_monoptah(&f.source, &f.target_db, &f.locations, &probe, "0.1.0")
            .unwrap_err();
        assert!(error.contains("Monoptah is running"), "{error}");
    }
    // When the list cannot be read the import cannot tell, so it refuses too.
    let probe = Probe(Err("ps failed".into()));
    let error =
        import_from_monoptah(&f.source, &f.target_db, &f.locations, &probe, "0.1.0").unwrap_err();
    assert!(error.contains("ps failed"), "{error}");
    assert!(!f.target_db.exists());
    assert!(!f.target.join(STAGING).exists());
    assert!(!f.target.join(MARKER).exists());
}

#[test]
fn zptah_itself_is_not_mistaken_for_monoptah() {
    assert!(!is_monoptah_command(
        "/Applications/zptah.app/Contents/MacOS/monocode-gpui"
    ));
    assert!(!is_monoptah_command(
        "/Users/a/monoptah-worktrees/x/target/native-dev/monocode-gpui"
    ));
    // The forms of the other platforms and of the dev bundle.
    for command in [
        "/Applications/Monoptah Native Dev.app/Contents/MacOS/monocode-gpui",
        "C:\\Program Files\\Monoptah\\monocode-gpui.exe",
        "C:\\Users\\a\\AppData\\Local\\Programs\\Monoptah\\monocode-gpui.exe --x",
        "/tmp/monocode-gpui --monocode-db /home/a/.local/share/com.monoptah.desktop/monocode.db",
    ] {
        assert!(is_monoptah_command(command), "{command}");
    }
    assert!(!is_monoptah_command(
        "C:\\Program Files\\zptah\\monocode-gpui.exe"
    ));
}

#[test]
fn import_copies_the_databases_the_key_and_the_user_data() {
    let f = fixture();
    let before = snapshot(&f.source);
    let summary = summary(run(&f).unwrap());
    // The WAL content of the source (nothing was ever checkpointed) arrived.
    assert!(!f.target.join("zptah.db-wal").exists());
    assert!(!f.target.join("source-snapshot").exists());
    let copy = Connection::open(&f.target_db).unwrap();
    let title: String = copy
        .query_row("SELECT title FROM sessions WHERE id='s1'", [], |r| r.get(0))
        .unwrap();
    assert_eq!(title, "First");
    assert!(f.target.join("zptah.db.gpui.sqlite3").is_file());
    assert!(f.target.join("zptah.db.search.sqlite3").is_file());
    assert_eq!(summary.databases, 3);
    // The key is the same bytes and private.
    assert_eq!(fs::read(f.target.join("credentials.key")).unwrap(), KEY);
    // User data came along; caches, mobile pairing and other state did not.
    for kept in [
        "zptah.db.threads/s1/attachments/a.png",
        "provider-accounts/claude/work/.claude.json",
        "monos/m1/SOUL.md",
        "remote-machines.json",
        "clickup-config.json",
    ] {
        assert!(f.target.join(kept).is_file(), "{kept}");
    }
    for left in [
        "mobile-access.json",
        "mobile-policy.json",
        "mobile-audit.jsonl",
        "file-transfer-keys.json",
        "kit-hash.key",
        "tui-hooks-native",
        "hang",
        "monocode.lock",
        "monocode.db",
        "monos/m1/.DS_Store",
        "monos/m1/link",
    ] {
        assert!(!f.target.join(left).exists(), "{left}");
    }
    assert!(!f.target.join(STAGING).exists());
    assert!(f.target.join(MARKER).is_file());
    // Monoptah was only read.
    assert_eq!(snapshot(&f.source), before);
}

#[cfg(unix)]
#[test]
fn permissions_are_kept_and_the_key_is_owner_only() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    summary(run(&f).unwrap());
    let mode = |name: &str| {
        fs::metadata(f.target.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    assert_eq!(mode("credentials.key"), 0o600);
    assert_eq!(mode("zptah.db"), 0o600);
    assert_eq!(mode("zptah.db.gpui.sqlite3"), 0o600);
    assert_eq!(mode("provider-accounts/claude/work/.claude.json"), 0o600);
    assert_eq!(mode("remote-machines.json"), 0o600);
    assert_eq!(mode("monos/m1/SOUL.md"), 0o644);
}

#[test]
fn mobile_identity_rows_are_dropped_from_the_copy_only() {
    let f = fixture();
    let summary = summary(run(&f).unwrap());
    assert_eq!(summary.dropped_mobile_rows, 3);
    for file in ["zptah.db", "zptah.db.gpui.sqlite3"] {
        let conn = Connection::open(f.target.join(file)).unwrap();
        if table_exists(&conn, "mobile_identity").unwrap() {
            let rows: i64 = conn
                .query_row("SELECT COUNT(*) FROM mobile_identity", [], |r| r.get(0))
                .unwrap();
            assert_eq!(rows, 0, "{file}");
        }
    }
    // The source still has its pairing.
    let source = Connection::open_with_flags(
        f.source.join("monocode.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let rows: i64 = source
        .query_row("SELECT COUNT(*) FROM mobile_identity", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 2);
}

#[test]
fn paths_into_the_old_data_folder_point_at_the_zptah_folder() {
    let f = fixture();
    let summary = summary(run(&f).unwrap());
    assert!(summary.rewritten_values >= 3);
    let new = f.target.to_string_lossy().into_owned();
    let old = f.source.to_string_lossy().into_owned();
    let copy = Connection::open(&f.target_db).unwrap();
    let blocks: String = copy
        .query_row("SELECT blocks_json FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert!(blocks.contains(&format!("{new}/zptah.db.threads/s1/attachments/a.png")));
    // A sibling folder that only shares the prefix is not ours to move.
    assert!(blocks.contains(&format!("{old}.dev/keep")));
    let queued: String = copy
        .query_row("SELECT queued_messages_json FROM sessions", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert!(queued.contains(&format!("{new}/composer-messages/1/ab/0/b.png")));
    let gpui = Connection::open(f.target.join("zptah.db.gpui.sqlite3")).unwrap();
    let value: String = gpui
        .query_row("SELECT value FROM settings", [], |r| r.get(0))
        .unwrap();
    assert!(value.contains(&new) && !value.contains(&format!("{old}/")));
    let machines = fs::read_to_string(f.target.join("remote-machines.json")).unwrap();
    assert!(machines.contains(&format!("{new}/emoji")));
    // The source keeps its own paths.
    let source = Connection::open_with_flags(
        f.source.join("monocode.db"),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let blocks: String = source
        .query_row("SELECT blocks_json FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert!(blocks.contains(&format!("{old}/monocode.db.threads")));
}

#[test]
fn the_copied_key_opens_the_copied_sealed_rows() {
    let f = fixture();
    let summary = summary(run(&f).unwrap());
    // One row of each of the four kinds.
    assert_eq!(summary.verified_secrets, 4);
    assert_eq!(summary.unreadable_secrets, 0);
    let key = sealed::read_key(&f.target.join("credentials.key")).unwrap();
    let conn = Connection::open(&f.target_db).unwrap();
    let (nonce, ct): (Vec<u8>, Vec<u8>) = conn
        .query_row(
            "SELECT nonce, ciphertext FROM provider_account_credentials",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    let aad = crate::accounts::stored_associated_data("work", "setup-token").unwrap();
    let plain = sealed::decrypt(&key, &aad, &nonce, &ct).unwrap();
    assert_eq!(plain.as_slice(), b"sk-ant-oat01-fake");
}

#[test]
fn a_key_that_does_not_open_the_rows_fails_without_a_marker_or_changes() {
    let f = fixture();
    write_private(&f.source.join("credentials.key"), &[9; 32], 0o600);
    let before = snapshot(&f.source);
    let error = run(&f).unwrap_err();
    assert!(error.contains("does not open"), "{error}");
    assert!(!f.target.join(MARKER).exists());
    assert!(!f.target_db.exists());
    assert!(!f.target.join("credentials.key").exists());
    assert!(!f.target.join(STAGING).exists());
    assert_eq!(snapshot(&f.source), before);
    // Rows but no key at all fail as well.
    fs::remove_file(f.source.join("credentials.key")).unwrap();
    assert!(run(&f).unwrap_err().contains("no credentials key"));
    // A damaged key is refused before anything is written.
    write_private(&f.source.join("credentials.key"), b"short", 0o600);
    assert!(run(&f).unwrap_err().contains("damaged"));
    assert!(!f.target.join(MARKER).exists());
}

#[test]
fn without_sealed_rows_the_key_only_has_to_load() {
    let f = fixture();
    let conn = Connection::open(f.source.join("monocode.db")).unwrap();
    for table in [
        "provider_account_credentials",
        "git_credentials",
        "docker_accounts",
        "mono_secrets",
    ] {
        conn.execute(&format!("DELETE FROM {table}"), []).unwrap();
    }
    drop(conn);
    let summary = summary(run(&f).unwrap());
    assert_eq!(summary.verified_secrets, 0);
    assert_eq!(fs::read(f.target.join("credentials.key")).unwrap(), KEY);
}

#[test]
fn a_second_run_does_nothing() {
    let f = fixture();
    summary(run(&f).unwrap());
    let done = snapshot(&f.target);
    assert_eq!(run(&f).unwrap(), Outcome::AlreadyImported);
    assert_eq!(snapshot(&f.target), done);
}

#[test]
fn a_failed_attempt_is_cleaned_up_and_the_retry_succeeds() {
    let f = fixture();
    // Leftovers of an import that died half way.
    write_private(&f.target.join(STAGING).join("zptah.db"), b"half", 0o600);
    write_private(&f.target.join(STAGING).join("junk/file"), b"half", 0o600);
    summary(run(&f).unwrap());
    assert!(!f.target.join(STAGING).exists());
    let copy = Connection::open(&f.target_db).unwrap();
    let rows: i64 = copy
        .query_row("SELECT COUNT(*) FROM sessions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(rows, 1);
}

#[test]
fn what_zptah_had_is_kept_in_a_backup_and_a_failed_swap_puts_it_back() {
    let f = fixture();
    let own = Connection::open(&f.target_db).unwrap();
    own.execute_batch("CREATE TABLE own(x); INSERT INTO own VALUES(1);")
        .unwrap();
    drop(own);
    write_private(&f.target.join("monos/own/SOUL.md"), b"mine", 0o644);
    // The marker cannot be written: the whole swap must be undone.
    fs::create_dir(f.target.join(format!("{MARKER}.tmp"))).unwrap();
    fs::write(f.target.join(format!("{MARKER}.tmp/x")), b"x").unwrap();
    let error = run(&f).unwrap_err();
    assert!(error.contains("imported-from-monoptah"), "{error}");
    assert!(!f.target.join(MARKER).exists());
    assert_eq!(
        fs::read(f.target.join("monos/own/SOUL.md")).unwrap(),
        b"mine"
    );
    let own = Connection::open(&f.target_db).unwrap();
    assert!(table_exists(&own, "own").unwrap());
    assert!(!f.target.join("credentials.key").exists());
    assert!(!f.target.join(BACKUP).exists());
    drop(own);
    // Retry once the obstacle is gone: the old files are kept in the backup.
    fs::remove_dir_all(f.target.join(format!("{MARKER}.tmp"))).unwrap();
    let summary = summary(run(&f).unwrap());
    let backup = summary.backup.unwrap();
    assert!(backup.join("0-zptah.db").is_file());
    let monos = fs::read_dir(&backup)
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().ends_with("-monos"))
        .unwrap()
        .path();
    assert_eq!(fs::read(monos.join("own/SOUL.md")).unwrap(), b"mine");
}

#[test]
fn uri_paths_are_percent_encoded() {
    assert_eq!(
        uri_path(Path::new("/a b/c%d?e#f/x.db")),
        "/a%20b/c%25d%3Fe%23f/x.db"
    );
}

#[test]
fn the_import_is_only_prepared_while_zptah_runs() {
    let f = fixture();
    let before = snapshot(&f.source);
    let summary = summary(prepare(&f).unwrap());
    assert_eq!(summary.databases, 3);
    // Nothing the running app uses was touched, and the marker waits for the move.
    assert!(f.target.join(PENDING).is_file());
    assert!(f.target.join(STAGING).join("zptah.db").is_file());
    assert!(!f.target_db.exists());
    assert!(!f.target.join("credentials.key").exists());
    assert!(!f.target.join(MARKER).exists());
    assert_eq!(offer(&f.source, &f.target_db), Offer::Pending);
    assert!(pending(&f.target_db));
    assert!(prepare(&f).unwrap_err().contains("waiting"));
    assert_eq!(snapshot(&f.source), before);
}

#[test]
fn the_start_up_step_moves_everything_in_and_writes_the_marker_last() {
    let f = fixture();
    prepare(&f).unwrap();
    let applied = apply_pending(&f.target).unwrap().unwrap();
    assert_eq!(applied.databases, 3);
    assert!(f.target_db.is_file());
    assert!(f.target.join("credentials.key").is_file());
    assert!(f.target.join(MARKER).is_file());
    assert!(!f.target.join(PENDING).exists());
    assert!(!f.target.join(STAGING).exists());
    assert_eq!(offer(&f.source, &f.target_db), Offer::Hidden);
    // Nothing pending any more.
    assert_eq!(apply_pending(&f.target).unwrap(), None);
}

#[test]
fn a_crash_between_items_is_resumed_at_the_next_start() {
    let f = fixture();
    let own = Connection::open(&f.target_db).unwrap();
    own.execute_batch("CREATE TABLE own(x)").unwrap();
    drop(own);
    prepare(&f).unwrap();
    // The process dies after two items were moved in.
    assert_eq!(
        apply_inner(&f.target, Some(2)).unwrap_err(),
        "simulated crash"
    );
    // Marker last: it must not exist while an item is missing.
    assert!(!f.target.join(MARKER).exists());
    assert!(f.target.join(PENDING).is_file());
    assert!(f.target.join("zptah.db.gpui.sqlite3").is_file());
    assert!(!f.target.join("credentials.key").exists());
    // Another crash, then the real start.
    assert!(apply_inner(&f.target, Some(4)).is_err());
    assert!(!f.target.join(MARKER).exists());
    apply_pending(&f.target).unwrap().unwrap();
    assert!(f.target.join(MARKER).is_file());
    assert!(!f.target.join(PENDING).exists());
    assert!(f.target.join("credentials.key").is_file());
    let copy = Connection::open(&f.target_db).unwrap();
    assert!(!table_exists(&copy, "own").unwrap());
    assert!(table_exists(&copy, "sessions").unwrap());
    // The zptah database it replaced was kept once, not overwritten by the second run.
    let kept = Connection::open(f.target.join(BACKUP).join("0-zptah.db")).unwrap();
    assert!(table_exists(&kept, "own").unwrap());
}

#[test]
fn an_item_that_cannot_be_moved_rolls_everything_back() {
    let f = fixture();
    let own = Connection::open(&f.target_db).unwrap();
    own.execute_batch("CREATE TABLE own(x)").unwrap();
    drop(own);
    write_private(&f.target.join("monos/own/SOUL.md"), b"mine", 0o644);
    prepare(&f).unwrap();
    // The prepared key vanishes after two databases were already moved in.
    fs::remove_file(f.target.join(STAGING).join("credentials.key")).unwrap();
    let error = apply_pending(&f.target).unwrap_err();
    assert!(error.contains("was undone"), "{error}");
    assert!(!f.target.join(MARKER).exists());
    assert!(!f.target.join(PENDING).exists());
    assert!(!f.target.join(STAGING).exists());
    assert!(!f.target.join("credentials.key").exists());
    assert!(!f.target.join("zptah.db.gpui.sqlite3").exists());
    let own = Connection::open(&f.target_db).unwrap();
    assert!(table_exists(&own, "own").unwrap());
    assert!(!table_exists(&own, "sessions").unwrap());
    assert_eq!(
        fs::read(f.target.join("monos/own/SOUL.md")).unwrap(),
        b"mine"
    );
    assert!(last_error(&f.target_db).unwrap().contains("was undone"));
    // The page can offer the import again.
    assert_eq!(offer(&f.source, &f.target_db), Offer::Ready);
    summary(run(&f).unwrap());
    assert!(f.target.join(MARKER).is_file());
    assert_eq!(last_error(&f.target_db), None);
}

#[test]
fn logos_and_backgrounds_move_with_the_development_folder_and_their_paths() {
    let f = fixture();
    summary(run(&f).unwrap());
    assert_eq!(
        fs::read(f.dev_target.join("project-images/p.png")).unwrap(),
        b"img"
    );
    assert!(f.dev_target.join("backgrounds/b.jpg").is_file());
    // The app makes its sounds again.
    assert!(!f.dev_target.join("sounds").exists());
    let gpui = Connection::open(f.target.join("zptah.db.gpui.sqlite3")).unwrap();
    let logo: String = gpui
        .query_row("SELECT value FROM settings WHERE key='logo'", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(
        logo,
        format!("{}/project-images/p.png", f.dev_target.display())
    );
    // Monoptah's own folder is only read.
    assert!(f.dev_source.join("project-images/p.png").is_file());
}

#[test]
fn the_desktop_preferences_follow_to_the_zptah_webview_storage() {
    let f = fixture();
    summary(run(&f).unwrap());
    let dirs = crate::desktop_preferences::webkit_origin_dirs(&f.webkit_target);
    assert_eq!(dirs.len(), 1);
    let conn = Connection::open_with_flags(
        dirs[0].join(crate::desktop_preferences::WEBKIT_DATABASE),
        OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    let theme: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key='monocode.theme'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(theme, "dark");
}

#[test]
fn every_text_file_of_the_copied_folders_gets_its_paths_and_binaries_do_not() {
    let f = fixture();
    summary(run(&f).unwrap());
    let toml = fs::read_to_string(f.target.join("monos/m1/notes.toml")).unwrap();
    assert_eq!(toml, format!("dir = \"{}/monos/m1\"\n", f.target.display()));
    let blob = fs::read(f.target.join("monos/m1/blob.bin")).unwrap();
    assert_eq!(blob, fs::read(f.source.join("monos/m1/blob.bin")).unwrap());
}

#[test]
fn a_second_prepare_after_the_marker_is_not_a_new_import() {
    let f = fixture();
    summary(run(&f).unwrap());
    assert_eq!(prepare(&f).unwrap(), Outcome::AlreadyImported);
}
