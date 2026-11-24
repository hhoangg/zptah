//! One-time import of a user's Monoptah data into zptah.
//!
//! Monoptah keeps its data in `<platform data>/com.monoptah.desktop`; zptah keeps its own in
//! `<platform data>/app.zptah.desktop`. This module copies what belongs to the user from the
//! first into the second, and never writes to the first.
//!
//! # What is copied (an allowlist; anything not named here is left behind)
//!
//! * `monocode.db`, `monocode.db.gpui.sqlite3` (settings and sealed credentials) and
//!   `monocode.db.search.sqlite3` (search cache), through the SQLite online backup API to the
//!   zptah names. A WAL database is never file-copied.
//! * `credentials.key`: the 32 byte AES-256-GCM key that opens every sealed row. It does not
//!   depend on the bundle id, the machine or the keychain, so a plain copy (mode 0600) is enough.
//!   The additional data of the sealed rows that are kept (provider tokens, Git, Docker, Mono
//!   secrets) hold ids and names only, never a path, so the new location does not matter.
//! * `monocode.db.threads/` (pasted images, cloud caches, evidence), as `zptah.db.threads/`, and
//!   the legacy `threads/`.
//! * `provider-accounts/`, `codex-monos/` (CLI logins), `monos/` (souls, memory, workflows),
//!   `emoji/`, `attachments/`, `today-attachments/`, `checkpoints/`, `composer-messages/`.
//! * `remote-machines.json`, `contributors-ai.json` and the Inbox sign-ins
//!   (`clickup-config.json`, `jira-config.json`, `gitlab-config.json`,
//!   `azure-devops-config.json`, `linear-token`). Permissions are preserved (0600 stays 0600).
//!
//! # What is left behind on purpose
//!
//! * `mobile-access.json`, `mobile-policy.json`, `mobile-audit.jsonl` and every row of
//!   `mobile_identity`: the web/mobile identity is one Durable Object per `desktop_id` on the
//!   relay, so two apps sharing it would kick each other off. zptah makes a new identity and the
//!   user pairs again. (The rows also could not be opened here: their additional data is
//!   `desktop:<absolute path of mobile-access.json>`.)
//! * `file-transfer-keys.json` (keys of objects that expire in a day), `kit-hash.key`,
//!   `tui-hooks-native/`, logs, hang dumps, locks, the orphan guard and temp files: caches or
//!   state the app makes again.
//!
//! # Absolute paths
//!
//! The databases do store absolute paths into the data directory. Evidence: attachments adopted
//! by a conversation are moved to `<database>.threads/<session>/attachments/<file>` and the new
//! absolute path is what the message keeps (`thread_files.rs`, `adopt_attachments`; used by the
//! quick composer); queued follow-up messages keep `<data dir>/composer-messages/<session>/...`
//! (`follow_up.rs`, `retain_attachments`). Both end up in `sessions.blocks_json`,
//! `sessions.queued_messages_json` or a `settings` value. After the copy, every text value of
//! the copied databases (and every small `.json` file that is copied) has the old directory
//! replaced by the zptah one, and `<old>/monocode.db.threads` by `<new>/zptah.db.threads`.
//! Primary key columns, BLOBs and virtual (full text) tables are never touched, and the search
//! cache is not rewritten (a stale fingerprint only makes it reindex).
//!
//! Not rewritten: paths into the `Monoptah Development` folder (`project-images/`,
//! `backgrounds/`, `sounds/`, which the native app wrote to its development directory even in a
//! release build). Those folders are outside the Monoptah data directory and stay where they are.
//!
//! # Folders outside the data directory
//!
//! * `crate::data_dir()` of the native app always returns the *development* folder
//!   (`Monoptah Development`, now `zptah Development`), even in a release build, so
//!   `project-images/` and `backgrounds/` (logos and chat backgrounds, referenced by absolute
//!   path from the settings) live there. They are copied from the Monoptah folder to the zptah
//!   one and those paths are rewritten too. `sounds/` is made again by the app and is skipped.
//! * The desktop preferences (`monocode.*` localStorage of the Tauri-era webview) are read live
//!   from the webview storage of the app id, never saved into the settings. The storage of
//!   `com.monoptah.desktop` is copied to the one of `app.zptah.desktop`: on macOS the origin
//!   folders under `~/Library/WebKit/<id>/WebsiteData/Default` (the localStorage database goes
//!   through the backup API), on Windows the WebView2 LevelDB folders under
//!   `%LOCALAPPDATA%\<id>`, on Linux the `localstorage/` folder in the data directory.
//!
//! # Safety
//!
//! Importing never replaces anything while zptah runs, because the running app holds its
//! databases open. The import builds and verifies everything in `<zptah dir>/.import-staging`
//! (database integrity, and the sealed rows opened with the copied key), then writes the journal
//! `.import-pending` and asks for a restart. At the next start, before any database or file
//! writer is opened, [`apply_pending`] moves the items in, one by one: whatever an item replaces
//! goes to `pre-import-backup` first. It can be run again after a crash, and a failure puts the
//! backed-up files back and leaves `.import-error`. The marker `.imported-from-monoptah` is
//! written only when every item is in place, and only then is the journal deleted.
use super::monocode_import::platform_data_base;
use crate::accounts::sealed;
use rusqlite::{backup::Backup, Connection, OpenFlags};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(test)]
mod tests;

/// Written into the zptah data directory once an import has finished.
pub const MARKER: &str = ".imported-from-monoptah";
const STAGING: &str = ".import-staging";
/// The journal of a verified import that waits for the restart.
pub const PENDING: &str = ".import-pending";
/// Why the last attempt to apply an import failed (shown on the Settings page).
pub const ERROR_FILE: &str = ".import-error";
const BACKUP: &str = "pre-import-backup";
/// Monoptah's database name (the `monocode` spelling is an identifier, see CLAUDE.md).
const SOURCE_DATABASE: &str = "monocode.db";
const GPUI_SUFFIX: &str = "gpui.sqlite3";
const SEARCH_SUFFIX: &str = "search.sqlite3";
const LEGACY_SETTINGS_SUFFIX: &str = "gpui.json";
const KEY_FILE: &str = "credentials.key";
/// Largest text file whose paths are rewritten; bigger files are copied as they are.
const MAX_REWRITE_BYTES: u64 = 32 * 1024 * 1024;

/// Top-level directories that hold user data.
const COPIED_DIRS: [&str; 10] = [
    "provider-accounts",
    "codex-monos",
    "monos",
    "emoji",
    "attachments",
    "today-attachments",
    "checkpoints",
    "composer-messages",
    "threads",
    // WebKitGTK localStorage of the desktop preferences (Linux).
    "localstorage",
];
/// Top-level files that hold user data.
const COPIED_FILES: [&str; 7] = [
    "remote-machines.json",
    "contributors-ai.json",
    "clickup-config.json",
    "jira-config.json",
    "gitlab-config.json",
    "azure-devops-config.json",
    "linear-token",
];

/// The data directory of the installed Monoptah app.
pub fn monoptah_data_dir() -> PathBuf {
    platform_data_base().join(crate::brand::MONOPTAH_DESKTOP_APP_ID)
}

/// Whether the Import page is offered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Offer {
    /// No Monoptah data, or it was imported already: the page is not shown.
    Hidden,
    Ready,
    /// Verified and waiting for the restart that moves it in.
    Pending,
    /// Shown but disabled, with the reason.
    Blocked(String),
}

/// What an import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub databases: usize,
    pub files: usize,
    pub rewritten_values: usize,
    pub dropped_mobile_rows: usize,
    pub verified_secrets: usize,
    pub unreadable_secrets: usize,
    /// Where the zptah files the import replaces are kept.
    pub backup: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Built and verified; it is moved in at the next start.
    Staged(Summary),
    /// The marker was there: nothing was done.
    AlreadyImported,
}

/// Folders outside the zptah data directory that the import reads from and writes to.
#[derive(Clone, Debug, Default)]
pub struct Locations {
    /// The development data folders of Monoptah and zptah (`project-images/`, `backgrounds/`).
    pub development: Option<(PathBuf, PathBuf)>,
    /// macOS: WebKit storage roots (`.../WebsiteData/Default`) of Monoptah and zptah.
    pub webkit: Option<(PathBuf, PathBuf)>,
    /// Windows: the `%LOCALAPPDATA%` folders of the two apps that hold the WebView2 profile.
    pub webview2: Option<(PathBuf, PathBuf)>,
}

impl Locations {
    /// Nothing outside the data directory (tests).
    pub fn none() -> Self {
        Self::default()
    }

    /// The real folders of this machine.
    pub fn system() -> Self {
        let base = platform_data_base();
        // Only macOS and Windows fill in more folders below.
        #[cfg_attr(not(any(target_os = "macos", windows)), allow(unused_mut))]
        let mut locations = Self {
            development: Some((
                base.join(crate::brand::MONOPTAH_DEV_DATA_DIR),
                base.join(crate::brand::DEV_DATA_DIR),
            )),
            ..Self::default()
        };
        #[cfg(target_os = "macos")]
        if let Some(home) = crate::accounts::home() {
            let root = |id: &str| {
                home.join("Library/WebKit")
                    .join(id)
                    .join("WebsiteData/Default")
            };
            locations.webkit = Some((
                root(crate::brand::MONOPTAH_DESKTOP_APP_ID),
                root(crate::brand::DESKTOP_APP_ID),
            ));
        }
        #[cfg(windows)]
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            locations.webview2 = Some((
                local.join(crate::brand::MONOPTAH_DESKTOP_APP_ID),
                local.join(crate::brand::DESKTOP_APP_ID),
            ));
        }
        locations
    }
}

/// Lists the commands of the running processes, so the import can refuse while Monoptah runs.
pub trait ProcessProbe: Send + Sync {
    fn commands(&self) -> Result<Vec<String>, String>;
}

/// The real process list, without this process. On Windows `tasklist` only shows image names
/// (both apps are `monocode-gpui.exe`), so the executable path and command line come from CIM.
pub struct SystemProcesses;

impl ProcessProbe for SystemProcesses {
    fn commands(&self) -> Result<Vec<String>, String> {
        let own = std::process::id().to_string();
        #[cfg(unix)]
        let output = std::process::Command::new("ps")
            .env("LC_ALL", "C")
            .args(["-axww", "-o", "pid=,command="])
            .output();
        #[cfg(windows)]
        let output = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "Get-CimInstance Win32_Process | ForEach-Object { '{0}`t{1}`t{2}' -f $_.ProcessId,$_.ExecutablePath,$_.CommandLine }",
            ])
            .output();
        let output = output.map_err(|e| format!("Could not list running apps: {e}"))?;
        if !output.status.success() {
            return Err("Could not list running apps".into());
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let commands: Vec<String> = text
            .lines()
            .filter_map(|line| {
                #[cfg(unix)]
                let (pid, rest) = line.trim_start().split_once(char::is_whitespace)?;
                #[cfg(windows)]
                let (pid, rest) = line.trim_start().split_once('\t')?;
                (pid != own).then(|| rest.replace('\t', " "))
            })
            .collect();
        if commands.is_empty() {
            // Even a bare system has processes: an empty list means the read failed.
            return Err("Could not read the running apps".into());
        }
        Ok(commands)
    }
}

/// A command line of the Monoptah app or Monoptah Native (also its dev bundle): the bundle
/// executable on macOS, the installed `monoptah-native` on Linux, the install folder on Windows,
/// or any `monocode-gpui` that was pointed at the Monoptah data folder. zptah's own paths never
/// contain these names.
fn is_monoptah_command(command: &str) -> bool {
    let lower = command.to_lowercase().replace('\\', "/");
    let by_name = [
        "monoptah.app/contents/",
        "monoptah native.app/contents/",
        "monoptah native dev.app/contents/",
        "monoptah-native",
        "monoptahnative",
        "program files/monoptah",
        "programs/monoptah",
    ]
    .iter()
    .any(|needle| lower.contains(needle));
    let by_data_dir = lower.contains("monocode-gpui")
        && lower.contains(&crate::brand::MONOPTAH_DESKTOP_APP_ID.to_lowercase());
    by_name || by_data_dir
}

/// The command of a running Monoptah, `None` when none runs. A probe that fails counts as
/// "cannot tell", which refuses the import.
pub fn running_monoptah(probe: &dyn ProcessProbe) -> Result<Option<String>, String> {
    Ok(probe
        .commands()?
        .into_iter()
        .find(|command| is_monoptah_command(command)))
}

const CLOSE_MONOPTAH: &str = "Quit Monoptah and Monoptah Native, then import again.";
const HAS_SESSIONS: &str =
    "zptah already has conversations of its own. Import only works into an empty zptah.";

fn target_dir(target_db: &Path) -> Result<&Path, String> {
    target_db
        .parent()
        .ok_or_else(|| "The zptah database has no folder".to_owned())
}

fn name_of(path: &Path) -> Result<String, String> {
    Ok(path
        .file_name()
        .ok_or_else(|| "The zptah database has no name".to_owned())?
        .to_string_lossy()
        .into_owned())
}

/// Whether the zptah database holds conversations of its own. A chat that never got a message
/// does not count. Missing files and tables hold nothing.
fn has_sessions(database: &Path) -> Result<bool, String> {
    if !database.exists() {
        return Ok(false);
    }
    let conn = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("Cannot read the zptah database: {e}"))?;
    conn.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    for table in ["sessions", "native_sessions"] {
        if !table_exists(&conn, table)? {
            continue;
        }
        let filter = if table == "sessions" && has_column(&conn, table, "has_user_message")? {
            " WHERE has_user_message=1"
        } else {
            ""
        };
        let found: bool = conn
            .query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table}{filter})"),
                [],
                |r| r.get(0),
            )
            .map_err(|e| e.to_string())?;
        if found {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [table],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quote(table)))
        .map_err(|e| e.to_string())?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(names.iter().any(|name| name == column))
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

/// Whether an import is verified and waits for the restart.
pub fn pending(target_db: &Path) -> bool {
    target_dir(target_db).is_ok_and(|dir| dir.join(PENDING).exists())
}

/// Why the last attempt to move an import in failed, if it did.
pub fn last_error(target_db: &Path) -> Option<String> {
    let text = fs::read_to_string(target_dir(target_db).ok()?.join(ERROR_FILE)).ok()?;
    Some(text.trim().to_owned()).filter(|text| !text.is_empty())
}

/// What the Settings page shows for this pair of data directories.
pub fn offer(source_dir: &Path, target_db: &Path) -> Offer {
    let Ok(target) = target_dir(target_db) else {
        return Offer::Hidden;
    };
    if target.join(MARKER).exists() {
        return Offer::Hidden;
    }
    if target.join(PENDING).exists() {
        return Offer::Pending;
    }
    if !source_dir.join(SOURCE_DATABASE).is_file() {
        return Offer::Hidden;
    }
    match has_sessions(target_db) {
        Ok(false) => Offer::Ready,
        Ok(true) => Offer::Blocked(HAS_SESSIONS.into()),
        Err(error) => Offer::Blocked(error),
    }
}

/// Build and verify an import of the user's Monoptah data into the zptah data directory of
/// `target_db`, and leave it waiting for the restart ([`apply_pending`] moves it in).
///
/// Refuses while Monoptah runs, when zptah has conversations, or when the data cannot be
/// verified. Never writes to `source_dir`, and never touches the files zptah is using.
pub fn import_from_monoptah(
    source_dir: &Path,
    target_db: &Path,
    locations: &Locations,
    probe: &dyn ProcessProbe,
    app_version: &str,
) -> Result<Outcome, String> {
    let target = target_dir(target_db)?;
    if target.join(MARKER).exists() {
        return Ok(Outcome::AlreadyImported);
    }
    if target.join(PENDING).exists() {
        return Err("An import is already waiting. Restart zptah to finish it.".into());
    }
    if !source_dir.join(SOURCE_DATABASE).is_file() {
        return Err("No Monoptah data was found.".into());
    }
    if matches!((source_dir.canonicalize(), target.canonicalize()), (Ok(a), Ok(b)) if a == b) {
        return Err("Monoptah and zptah share one folder.".into());
    }
    if running_monoptah(probe)
        .map_err(|e| format!("{e}. {CLOSE_MONOPTAH}"))?
        .is_some()
    {
        return Err(format!("Monoptah is running. {CLOSE_MONOPTAH}"));
    }
    if has_sessions(target_db)? {
        return Err(HAS_SESSIONS.into());
    }
    let _ = fs::remove_file(target.join(ERROR_FILE));
    let staging = target.join(STAGING);
    if staging.exists() {
        // A failed earlier attempt: it only ever holds copies.
        fs::remove_dir_all(&staging).map_err(|e| format!("Cannot clear the old import: {e}"))?;
    }
    create_private_dir(&staging)?;
    let staged = build_staging(source_dir, target_db, locations, &staging, probe).and_then(
        |(plan, mut summary)| {
            summary.backup = Some(target.join(BACKUP));
            write_journal(target, source_dir, app_version, &plan, &summary)?;
            Ok(summary)
        },
    );
    match staged {
        Ok(summary) => Ok(Outcome::Staged(summary)),
        Err(error) => {
            let _ = fs::remove_dir_all(&staging);
            let _ = fs::remove_file(target.join(PENDING));
            Err(error)
        }
    }
}

/// One staged item to move into place.
struct Item {
    /// Its path inside the staging folder.
    staged: String,
    /// Where it goes.
    dest: PathBuf,
    /// A database: its `-wal` and `-shm` files go aside with it.
    database: bool,
}

fn build_staging(
    source_dir: &Path,
    target_db: &Path,
    locations: &Locations,
    staging: &Path,
    probe: &dyn ProcessProbe,
) -> Result<(Vec<Item>, Summary), String> {
    let target = target_dir(target_db)?;
    let name = name_of(target_db)?;
    let development = locations
        .development
        .as_ref()
        .filter(|(from, _)| from.is_dir());
    let rewrite = Rewrite::new(
        source_dir,
        target,
        SOURCE_DATABASE,
        &name,
        development.map(|(from, to)| (from.as_path(), to.as_path())),
    );
    let leave_alone = Rewrite::none();
    let mut summary = Summary::default();
    let mut plan: Vec<Item> = Vec::new();
    let add = |plan: &mut Vec<Item>, staged: &str, dest: PathBuf, database: bool| {
        plan.push(Item {
            staged: staged.to_owned(),
            dest,
            database,
        });
    };
    // 1. The databases, through the online backup API. The search cache is not rewritten.
    for (suffix, rewritten) in [("", true), (GPUI_SUFFIX, true), (SEARCH_SUFFIX, false)] {
        let (from, to) = if suffix.is_empty() {
            (SOURCE_DATABASE.to_owned(), name.clone())
        } else {
            (
                format!("{SOURCE_DATABASE}.{suffix}"),
                format!("{name}.{suffix}"),
            )
        };
        let source = source_dir.join(&from);
        if !source.is_file() {
            continue;
        }
        let staged = staging.join(&to);
        backup_database(&source, staging, &staged)?;
        let conn = Connection::open(&staged).map_err(|e| e.to_string())?;
        conn.busy_timeout(Duration::from_secs(5))
            .map_err(|e| e.to_string())?;
        summary.dropped_mobile_rows += drop_mobile_identity(&conn)?;
        if rewritten {
            summary.rewritten_values += rewrite_database(&conn, &rewrite)?;
        }
        let check: String = conn
            .query_row("PRAGMA quick_check", [], |r| r.get(0))
            .map_err(|e| e.to_string())?;
        if check != "ok" {
            return Err(format!("The copy of {from} is damaged: {check}"));
        }
        drop(conn);
        remove_sidecars(&staged);
        super::restrict_database_files(&staged);
        summary.databases += 1;
        add(&mut plan, &to, target.join(&to), true);
    }
    let legacy = format!("{SOURCE_DATABASE}.{LEGACY_SETTINGS_SUFFIX}");
    if source_dir.join(&legacy).is_file() {
        let to = format!("{name}.{LEGACY_SETTINGS_SUFFIX}");
        copy_file(&source_dir.join(&legacy), &staging.join(&to), &rewrite)?;
        add(&mut plan, &to, target.join(&to), false);
        summary.files += 1;
    }
    // 2. The key that opens the sealed rows.
    let key_source = source_dir.join(KEY_FILE);
    if key_source.exists() {
        copy_key(&key_source, &staging.join(KEY_FILE))?;
        add(&mut plan, KEY_FILE, target.join(KEY_FILE), false);
        summary.files += 1;
    }
    // 3. The files the conversations keep beside the database.
    let threads_from = source_dir.join(format!("{SOURCE_DATABASE}.threads"));
    let threads_to = format!("{name}.threads");
    if threads_from.is_dir() && !is_link(&threads_from) {
        summary.files += copy_tree(&threads_from, &staging.join(&threads_to), &rewrite)?;
        add(&mut plan, &threads_to, target.join(&threads_to), false);
    }
    // 4. The rest of the user's data. Checkpoints hold file contents, which are not ours to edit.
    for dir in COPIED_DIRS {
        let from = source_dir.join(dir);
        if from.is_dir() && !is_link(&from) {
            let how = if dir == "checkpoints" {
                &leave_alone
            } else {
                &rewrite
            };
            summary.files += copy_tree(&from, &staging.join(dir), how)?;
            add(&mut plan, dir, target.join(dir), false);
        }
    }
    for file in COPIED_FILES {
        let from = source_dir.join(file);
        if from.is_file() && !is_link(&from) {
            copy_file(&from, &staging.join(file), &rewrite)?;
            add(&mut plan, file, target.join(file), false);
            summary.files += 1;
        }
    }
    // 5. Logos and chat backgrounds live in the development folder (see the module docs).
    if let Some((from, to)) = development {
        for dir in DEVELOPMENT_DIRS {
            let source = from.join(dir);
            if source.is_dir() && !is_link(&source) {
                let staged = format!("development/{dir}");
                summary.files += copy_tree(&source, &staging.join(&staged), &rewrite)?;
                add(&mut plan, &staged, to.join(dir), false);
            }
        }
    }
    // 6. The desktop preferences kept in the webview storage.
    #[cfg(any(target_os = "macos", test))]
    if let Some((from, to)) = &locations.webkit {
        stage_webkit(from, to, staging, &mut plan, &mut summary)?;
    }
    if let Some((from, to)) = &locations.webview2 {
        for relative in WEBVIEW2_DIRS {
            let source = from.join(relative);
            if source.is_dir() && !is_link(&source) {
                let staged = format!("webview2/{relative}");
                summary.files += copy_tree(&source, &staging.join(&staged), &leave_alone)?;
                add(&mut plan, &staged, to.join(relative), false);
            }
        }
    }
    // 7. Prove the copy opens before anything is replaced.
    let (verified, unreadable) = verify_secrets(staging, &name)?;
    summary.verified_secrets = verified;
    summary.unreadable_secrets = unreadable;
    // Monoptah may have been started while the copy ran.
    if running_monoptah(probe)
        .map_err(|e| format!("{e}. {CLOSE_MONOPTAH}"))?
        .is_some()
    {
        return Err(format!(
            "Monoptah started during the import. {CLOSE_MONOPTAH}"
        ));
    }
    Ok((plan, summary))
}

/// Folders of the development data directory that hold the user's own files.
const DEVELOPMENT_DIRS: [&str; 2] = ["project-images", "backgrounds"];
/// Where the desktop's WebView2 profile keeps localStorage, below the app's folder.
const WEBVIEW2_DIRS: [&str; 2] = [
    "EBWebView/Default/Local Storage/leveldb",
    "Default/Local Storage/leveldb",
];

/// Stage the WebKit origin folders of the desktop preferences. The localStorage database goes
/// through the backup API; the `origin` file says which origin the folder is.
#[cfg(any(target_os = "macos", test))]
fn stage_webkit(
    from: &Path,
    to: &Path,
    staging: &Path,
    plan: &mut Vec<Item>,
    summary: &mut Summary,
) -> Result<(), String> {
    for origin in crate::desktop_preferences::webkit_origin_dirs(from) {
        let Ok(relative) = origin.strip_prefix(from) else {
            continue;
        };
        let staged = format!("webkit/{}", relative.to_string_lossy());
        let folder = staging.join(&staged);
        let database = folder.join(crate::desktop_preferences::WEBKIT_DATABASE);
        fs::create_dir_all(database.parent().unwrap_or(&folder)).map_err(|e| e.to_string())?;
        fs::copy(origin.join("origin"), folder.join("origin")).map_err(|e| e.to_string())?;
        backup_database(
            &origin.join(crate::desktop_preferences::WEBKIT_DATABASE),
            staging,
            &database,
        )?;
        remove_sidecars(&database);
        summary.files += 2;
        plan.push(Item {
            staged,
            dest: to.join(relative),
            database: false,
        });
    }
    Ok(())
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

fn create_private_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("Cannot create {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Copy one database with the online backup API. The source is never written, and no file is
/// created beside it:
///
/// * no WAL, or an empty one (Monoptah quit cleanly): open it `immutable`, which skips locking
///   and the `-wal`/`-shm` files a read-only open would create;
/// * a WAL with content (Monoptah did not quit cleanly): copy the database and its WAL into the
///   staging folder and open that private copy, so SQLite can replay the WAL without touching
///   the original. Monoptah is known to be closed here (the caller checked, and checks again).
fn backup_database(source: &Path, staging: &Path, destination: &Path) -> Result<(), String> {
    let mut wal = source.as_os_str().to_owned();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    let has_wal = fs::metadata(&wal).is_ok_and(|m| m.len() > 0);
    let snapshot_dir = staging.join("source-snapshot");
    let from = if has_wal {
        fs::create_dir_all(&snapshot_dir).map_err(|e| e.to_string())?;
        let name = source.file_name().unwrap_or_default();
        let copy = snapshot_dir.join(name);
        fs::copy(source, &copy).map_err(|e| format!("Cannot read {}: {e}", source.display()))?;
        let mut copy_wal = copy.as_os_str().to_owned();
        copy_wal.push("-wal");
        fs::copy(&wal, PathBuf::from(copy_wal))
            .map_err(|e| format!("Cannot read {}: {e}", wal.display()))?;
        Connection::open(&copy)
    } else {
        Connection::open_with_flags(
            format!("file:{}?immutable=1", uri_path(source)),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )
    }
    .map_err(|e| format!("Cannot read {}: {e}", source.display()))?;
    from.busy_timeout(Duration::from_secs(5))
        .map_err(|e| e.to_string())?;
    let mut to = Connection::open(destination).map_err(|e| e.to_string())?;
    {
        let backup = Backup::new(&from, &mut to).map_err(|e| e.to_string())?;
        backup
            .run_to_completion(256, Duration::from_millis(0), None)
            .map_err(|e| format!("Cannot copy {}: {e}", source.display()))?;
    }
    drop(to);
    drop(from);
    let _ = fs::remove_dir_all(&snapshot_dir);
    super::restrict_database_files(destination);
    Ok(())
}

/// The path part of a `file:` URI: everything but unreserved characters is percent-encoded.
fn uri_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = if text.starts_with('/') {
        text
    } else {
        format!("/{text}")
    };
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'.' | b'_' | b'~' | b':') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn remove_sidecars(database: &Path) {
    for suffix in ["-wal", "-shm"] {
        let mut name = database.as_os_str().to_owned();
        name.push(suffix);
        let _ = fs::remove_file(PathBuf::from(name));
    }
}

/// Remove every saved mobile and web-link identity from the copy. The user pairs again.
fn drop_mobile_identity(conn: &Connection) -> Result<usize, String> {
    if !table_exists(conn, "mobile_identity")? {
        return Ok(0);
    }
    conn.execute("DELETE FROM mobile_identity", [])
        .map_err(|e| e.to_string())
}

/// Old text and its replacement, longest match first.
struct Rewrite {
    pairs: Vec<(String, String)>,
    /// The old data directory spellings: a cheap test for "nothing to do".
    old_dirs: Vec<String>,
}

impl Rewrite {
    /// Rewrites nothing.
    fn none() -> Self {
        Self {
            pairs: Vec::new(),
            old_dirs: Vec::new(),
        }
    }

    fn new(
        source_dir: &Path,
        target_dir: &Path,
        old_db: &str,
        new_db: &str,
        development: Option<(&Path, &Path)>,
    ) -> Self {
        let sep = std::path::MAIN_SEPARATOR;
        let mut olds = vec![source_dir.to_string_lossy().into_owned()];
        // The attachments folder is named from the canonical database path.
        if let Ok(canonical) = source_dir.canonicalize() {
            let text = canonical.to_string_lossy().into_owned();
            if !olds.contains(&text) {
                olds.push(text);
            }
        }
        let new = target_dir.to_string_lossy().into_owned();
        let mut pairs = Vec::new();
        for old in &olds {
            pairs.push((
                format!("{old}{sep}{old_db}.threads"),
                format!("{new}{sep}{new_db}.threads"),
            ));
        }
        for old in &olds {
            pairs.push((format!("{old}{sep}"), format!("{new}{sep}")));
        }
        if let Some((from, to)) = development {
            let (from, to) = (from.to_string_lossy(), to.to_string_lossy());
            pairs.push((format!("{from}{sep}"), format!("{to}{sep}")));
            olds.push(from.into_owned());
        }
        // The same paths inside JSON text on Windows, where a backslash is written twice.
        let escaped: Vec<_> = pairs
            .iter()
            .filter(|(old, _)| old.contains('\\'))
            .map(|(old, new)| (old.replace('\\', "\\\\"), new.replace('\\', "\\\\")))
            .collect();
        pairs.extend(escaped);
        Self {
            pairs,
            old_dirs: olds,
        }
    }

    fn apply(&self, text: &str) -> Option<String> {
        if !self.old_dirs.iter().any(|old| text.contains(old.as_str())) {
            return None;
        }
        let mut out = text.to_owned();
        for (old, new) in &self.pairs {
            if out.contains(old.as_str()) {
                out = out.replace(old.as_str(), new);
            }
        }
        (out != text).then_some(out)
    }
}

/// Replace the old data directory in every text value of the copied database. Returns how many
/// values changed. Primary keys, BLOBs and virtual tables are skipped.
fn rewrite_database(conn: &Connection, rewrite: &Rewrite) -> Result<usize, String> {
    let tables: Vec<(String, String)> = conn
        .prepare(
            "SELECT name, COALESCE(sql,'') FROM sqlite_master \
             WHERE type='table' AND name NOT LIKE 'sqlite_%'",
        )
        .map_err(|e| e.to_string())?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let virtuals: Vec<&str> = tables
        .iter()
        .filter(|(_, sql)| sql.to_uppercase().starts_with("CREATE VIRTUAL"))
        .map(|(name, _)| name.as_str())
        .collect();
    let mut changed = 0;
    let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
    for (table, _) in &tables {
        if virtuals
            .iter()
            .any(|v| table == v || table.starts_with(&format!("{v}_")))
        {
            continue;
        }
        let columns: Vec<String> = tx
            .prepare(&format!("PRAGMA table_info({})", quote(table)))
            .map_err(|e| e.to_string())?
            .query_map([], |row| {
                Ok((row.get::<_, String>(1)?, row.get::<_, i64>(5)?))
            })
            .map_err(|e| e.to_string())?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|(_, pk)| *pk == 0)
            .map(|(name, _)| name)
            .collect();
        for column in columns {
            let (table, column) = (quote(table), quote(&column));
            for (old, new) in &rewrite.pairs {
                changed += tx
                    .execute(
                        &format!(
                            "UPDATE {table} SET {column}=replace({column},?1,?2) \
                             WHERE typeof({column})='text' AND instr({column},?1)>0"
                        ),
                        [old, new],
                    )
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    tx.commit().map_err(|e| e.to_string())?;
    Ok(changed)
}

#[cfg(unix)]
fn mode_of(meta: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

/// Copy the key with owner-only permissions. A key of the wrong size would fail every row.
fn copy_key(from: &Path, to: &Path) -> Result<(), String> {
    use std::io::Write;
    let bytes = zeroize::Zeroizing::new(
        fs::read(from).map_err(|e| format!("Cannot read the credentials key: {e}"))?,
    );
    if bytes.len() != sealed::KEY_LEN {
        return Err("The Monoptah credentials key is damaged, so nothing was imported.".into());
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(to).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Cannot write the credentials key: {e}"))
}

/// Entries that are state of a running app, not user data.
fn is_noise(name: &str) -> bool {
    name == ".DS_Store"
        || name.ends_with(".lock")
        || name.ends_with(".tmp")
        || name.starts_with(".copy-")
        || name.starts_with(".staging-")
}

/// The text of `from` with the old paths replaced, when it is a UTF-8 text file (no NUL byte in
/// its first 8 KiB) of a sane size that mentions one. Binary files are never read whole.
fn rewritten_text(from: &Path, size: u64, rewrite: &Rewrite) -> Option<String> {
    use std::io::Read;
    if rewrite.old_dirs.is_empty() || size > MAX_REWRITE_BYTES {
        return None;
    }
    let mut file = fs::File::open(from).ok()?;
    let mut head = vec![0; 8192.min(size as usize)];
    file.read_exact(&mut head).ok()?;
    if head.contains(&0) {
        return None;
    }
    let mut bytes = head;
    file.read_to_end(&mut bytes).ok()?;
    rewrite.apply(&String::from_utf8(bytes).ok()?)
}

/// Copy one file, keeping its permissions. A text file gets its paths rewritten.
fn copy_file(from: &Path, to: &Path, rewrite: &Rewrite) -> Result<(), String> {
    let meta = fs::metadata(from).map_err(|e| format!("{}: {e}", from.display()))?;
    if let Some(text) = rewritten_text(from, meta.len(), rewrite) {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(mode_of(&meta) | 0o600);
        }
        let mut file = options
            .open(to)
            .map_err(|e| format!("{}: {e}", to.display()))?;
        file.write_all(text.as_bytes())
            .map_err(|e| format!("{}: {e}", to.display()))?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(to, fs::Permissions::from_mode(mode_of(&meta)))
                .map_err(|e| e.to_string())?;
        }
        return Ok(());
    }
    fs::copy(from, to).map_err(|e| format!("Cannot copy {}: {e}", from.display()))?;
    Ok(())
}

/// Copy a folder. Links are never followed or copied. Returns the number of files.
fn copy_tree(from: &Path, to: &Path, rewrite: &Rewrite) -> Result<usize, String> {
    fs::create_dir_all(to).map_err(|e| format!("Cannot create {}: {e}", to.display()))?;
    let mut count = 0;
    let entries = fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let name = entry.file_name();
        if is_noise(&name.to_string_lossy()) {
            continue;
        }
        let meta = fs::symlink_metadata(entry.path()).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let target = to.join(&name);
        if meta.is_dir() {
            count += copy_tree(&entry.path(), &target, rewrite)?;
        } else if meta.is_file() {
            copy_file(&entry.path(), &target, rewrite)?;
            count += 1;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(from).map_err(|e| e.to_string())?;
        // Owner access is kept so the folder can be moved and later deleted.
        fs::set_permissions(to, fs::Permissions::from_mode(mode_of(&meta) | 0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(count)
}

struct SealedKind {
    table: &'static str,
    label: &'static str,
    /// Selects the columns that make the additional data, then `nonce`, `ciphertext`.
    select: &'static str,
    aad: fn(&[String]) -> Option<Vec<u8>>,
}

struct SealedRow {
    aad: Vec<u8>,
    nonce: Vec<u8>,
    ciphertext: Vec<u8>,
}

const SEALED_TABLES: [SealedKind; 4] = [
    SealedKind {
        table: "provider_account_credentials",
        label: "Claude credentials",
        select: "SELECT account_id, kind, nonce, ciphertext FROM provider_account_credentials WHERE provider='claude'",
        aad: |p| crate::accounts::stored_associated_data(&p[0], &p[1]),
    },
    SealedKind {
        table: "git_credentials",
        label: "Git credentials",
        select: "SELECT id, origin, username, nonce, ciphertext FROM git_credentials",
        aad: |p| Some(crate::git_credentials::associated_data(&p[0], &p[1], &p[2])),
    },
    SealedKind {
        table: "docker_accounts",
        label: "Docker accounts",
        select: "SELECT id, docker_id, nonce, ciphertext FROM docker_accounts",
        aad: |p| Some(crate::docker_accounts::associated_data(&p[0], &p[1])),
    },
    SealedKind {
        table: "mono_secrets",
        label: "Mono secrets",
        select: "SELECT mono_id, name, nonce, ciphertext FROM mono_secrets",
        aad: |p| Some(crate::mono::browser::secret_aad(&p[0], &p[1])),
    },
];

fn sealed_rows(conn: &Connection, kind: &SealedKind) -> Result<Vec<SealedRow>, String> {
    let mut stmt = conn.prepare(kind.select).map_err(|e| e.to_string())?;
    let width = stmt.column_count() - 2;
    let rows = stmt
        .query_map([], |row| {
            let parts = (0..width)
                .map(|i| row.get::<_, String>(i))
                .collect::<Result<Vec<_>, _>>()?;
            Ok((
                parts,
                row.get::<_, Vec<u8>>(width)?,
                row.get::<_, Vec<u8>>(width + 1)?,
            ))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(parts, nonce, ciphertext)| SealedRow {
            // An unknown kind cannot be opened; an empty binding makes it count as unreadable.
            aad: (kind.aad)(&parts).unwrap_or_default(),
            nonce,
            ciphertext,
        })
        .collect())
}

/// Open the sealed rows of the copy with the copied key. A table whose rows all fail, or rows
/// without a key, fail the import; single damaged rows are only counted. Returns (opened, failed).
fn verify_secrets(staging: &Path, database_name: &str) -> Result<(usize, usize), String> {
    let key_path = staging.join(KEY_FILE);
    let key = match sealed::read_key(&key_path) {
        Ok(key) => Some(zeroize::Zeroizing::new(key)),
        Err(_) if !key_path.exists() => None,
        Err(_) => return Err("The copied credentials key cannot be read.".into()),
    };
    let (mut verified, mut unreadable) = (0, 0);
    for file in [
        database_name.to_owned(),
        format!("{database_name}.{GPUI_SUFFIX}"),
    ] {
        let path = staging.join(&file);
        if !path.is_file() {
            continue;
        }
        let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| e.to_string())?;
        for kind in &SEALED_TABLES {
            if !table_exists(&conn, kind.table)? {
                continue;
            }
            let rows = sealed_rows(&conn, kind)?;
            if rows.is_empty() {
                continue;
            }
            let Some(key) = key.as_ref() else {
                return Err(format!(
                    "Monoptah has saved {} but no credentials key, so they cannot be opened.",
                    kind.label
                ));
            };
            let (mut good, mut bad) = (0, 0);
            for row in rows {
                match sealed::decrypt(key, &row.aad, &row.nonce, &row.ciphertext) {
                    Ok(_) => good += 1,
                    Err(()) => bad += 1,
                }
            }
            if good == 0 {
                return Err(format!(
                    "The copied key does not open the saved {}, so nothing was imported.",
                    kind.label
                ));
            }
            verified += good;
            unreadable += bad;
        }
    }
    Ok((verified, unreadable))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Write `text` to `path` through a private temp file, so a reader never sees half of it.
fn write_atomically(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let _ = fs::remove_file(&temporary);
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("Cannot write {}: {e}", path.display()))?;
    drop(file);
    fs::rename(&temporary, path).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        format!("Cannot write {}: {e}", path.display())
    })
}

fn summary_json(summary: &Summary) -> serde_json::Value {
    json!({
        "databases": summary.databases,
        "files": summary.files,
        "rewritten_values": summary.rewritten_values,
        "dropped_mobile_rows": summary.dropped_mobile_rows,
        "verified_secrets": summary.verified_secrets,
        "unreadable_secrets": summary.unreadable_secrets,
    })
}

/// The journal: everything [`apply_pending`] needs, so it can finish after a crash.
fn write_journal(
    target: &Path,
    source_dir: &Path,
    app_version: &str,
    plan: &[Item],
    summary: &Summary,
) -> Result<(), String> {
    let items: Vec<_> = plan
        .iter()
        .map(|item| {
            json!({
                "staged": item.staged,
                "dest": item.dest.to_string_lossy(),
                "database": item.database,
            })
        })
        .collect();
    let text = serde_json::to_string_pretty(&json!({
        "version": 1,
        "created_ms": now_ms(),
        "zptah_version": app_version,
        "source": source_dir.to_string_lossy(),
        "source_database_bytes": fs::metadata(source_dir.join(SOURCE_DATABASE)).map_or(0, |m| m.len()),
        "backup_dir": target.join(BACKUP).to_string_lossy(),
        "summary": summary_json(summary),
        "items": items,
    }))
    .map_err(|e| e.to_string())?;
    write_atomically(&target.join(PENDING), &text)
}

struct Journal {
    items: Vec<Item>,
    backup_dir: PathBuf,
    summary: Summary,
    raw: serde_json::Value,
}

fn read_journal(text: &str) -> Result<Journal, String> {
    let raw: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("The import journal is damaged: {e}"))?;
    let damaged = || "The import journal is damaged".to_owned();
    let items = raw["items"]
        .as_array()
        .ok_or_else(damaged)?
        .iter()
        .map(|item| {
            Some(Item {
                staged: item["staged"].as_str()?.to_owned(),
                dest: PathBuf::from(item["dest"].as_str()?),
                database: item["database"].as_bool()?,
            })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(damaged)?;
    let number = |key: &str| raw["summary"][key].as_u64().unwrap_or(0) as usize;
    let backup_dir = PathBuf::from(raw["backup_dir"].as_str().ok_or_else(damaged)?);
    Ok(Journal {
        items,
        summary: Summary {
            databases: number("databases"),
            files: number("files"),
            rewritten_values: number("rewritten_values"),
            dropped_mobile_rows: number("dropped_mobile_rows"),
            verified_secrets: number("verified_secrets"),
            unreadable_secrets: number("unreadable_secrets"),
            backup: Some(backup_dir.clone()),
        },
        backup_dir,
        raw,
    })
}

/// Rename, falling back to copy and delete when the two folders are on different volumes.
fn move_path(from: &Path, to: &Path) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Cannot create {}: {e}", parent.display()))?;
    }
    if fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let none = Rewrite::none();
    if from.is_dir() {
        copy_tree(from, to, &none)?;
        fs::remove_dir_all(from)
    } else {
        fs::copy(from, to)
            .map(|_| ())
            .map_err(|e| e.to_string())
            .and_then(|_| fs::remove_file(from).map_err(|e| e.to_string()))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    .map_err(|e| format!("Cannot move {} to {}: {e}", from.display(), to.display()))
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// The paths an item replaces: itself, and for a database its WAL and index files, which would
/// otherwise be replayed onto the new database.
fn victims(item: &Item) -> Vec<PathBuf> {
    let mut paths = vec![item.dest.clone()];
    if item.database {
        for suffix in ["-wal", "-shm"] {
            let mut name = item.dest.as_os_str().to_owned();
            name.push(suffix);
            paths.push(PathBuf::from(name));
        }
    }
    paths
}

fn saved_name(backup_dir: &Path, index: usize, victim: &Path) -> PathBuf {
    backup_dir.join(format!(
        "{index}-{}",
        victim.file_name().unwrap_or_default().to_string_lossy()
    ))
}

/// Put one item in place. Safe to run again: an item whose staged copy is gone is done.
fn apply_item(index: usize, item: &Item, staging: &Path, backup_dir: &Path) -> Result<(), String> {
    let staged = staging.join(&item.staged);
    if !exists(&staged) {
        return if exists(&item.dest) {
            Ok(())
        } else {
            Err(format!(
                "The prepared copy of {} is missing",
                item.dest.display()
            ))
        };
    }
    for victim in victims(item) {
        if !exists(&victim) {
            continue;
        }
        let mut saved = saved_name(backup_dir, index, &victim);
        if exists(&saved) {
            // Something new grew where the first one was moved aside: keep both.
            saved.set_extension(format!("extra-{}", now_ms()));
        }
        move_path(&victim, &saved)?;
    }
    move_path(&staged, &item.dest)
}

/// Undo what [`apply_item`] did, newest first: remove what was put in place and bring the
/// backed-up files back.
fn rollback(items: &[Item], staging: &Path, backup_dir: &Path) {
    for (index, item) in items.iter().enumerate().rev() {
        let placed = !exists(&staging.join(&item.staged));
        let saved: Vec<_> = victims(item)
            .into_iter()
            .map(|victim| (saved_name(backup_dir, index, &victim), victim))
            .filter(|(saved, _)| exists(saved))
            .collect();
        if placed && exists(&item.dest) {
            // A placed item always came from staging, so what is there now is ours.
            if item.dest.is_dir() {
                let _ = fs::remove_dir_all(&item.dest);
            } else {
                let _ = fs::remove_file(&item.dest);
            }
        }
        for (saved, victim) in saved {
            if !exists(&victim) {
                let _ = move_path(&saved, &victim);
            }
        }
    }
    let _ = fs::remove_dir(backup_dir);
}

/// Finish an import that [`import_from_monoptah`] prepared. Call it at start-up, before a
/// database, search index or file writer opens the data directory.
///
/// `Ok(None)` when nothing is pending. A crash in the middle is picked up here at the next
/// start. On failure everything is put back, `.import-error` says why, and the journal is
/// removed so the import can be tried again.
pub fn apply_pending(target: &Path) -> Result<Option<Summary>, String> {
    apply_inner(target, None)
}

fn apply_inner(target: &Path, crash_after: Option<usize>) -> Result<Option<Summary>, String> {
    let journal_path = target.join(PENDING);
    let text = match fs::read_to_string(&journal_path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("Cannot read the import journal: {e}")),
    };
    let staging = target.join(STAGING);
    if target.join(MARKER).exists() {
        // Finished, but the clean-up did not run.
        let _ = fs::remove_file(&journal_path);
        let _ = fs::remove_dir_all(&staging);
        return Ok(None);
    }
    let journal = match read_journal(&text) {
        Ok(journal) => journal,
        Err(error) => {
            let _ = fs::remove_file(&journal_path);
            let _ = write_atomically(&target.join(ERROR_FILE), &error);
            return Err(error);
        }
    };
    create_private_dir(&journal.backup_dir)?;
    let run = || -> Result<(), String> {
        for (index, item) in journal.items.iter().enumerate() {
            if crash_after == Some(index) {
                return Err(CRASH.into());
            }
            apply_item(index, item, &staging, &journal.backup_dir)?;
        }
        let marker = marker_text(&journal);
        write_atomically(&target.join(MARKER), &marker)
    };
    match run() {
        Ok(()) => {
            let _ = fs::remove_file(&journal_path);
            let _ = fs::remove_file(target.join(ERROR_FILE));
            let _ = fs::remove_dir_all(&staging);
            let _ = fs::remove_dir(&journal.backup_dir);
            let mut summary = journal.summary;
            if !exists(&journal.backup_dir) {
                summary.backup = None;
            }
            Ok(Some(summary))
        }
        // A simulated crash leaves everything as it is, like a process that died.
        Err(error) if error == CRASH => Err(error),
        Err(error) => {
            let _ = fs::remove_file(target.join(MARKER));
            rollback(&journal.items, &staging, &journal.backup_dir);
            let _ = fs::remove_file(&journal_path);
            let _ = fs::remove_dir_all(&staging);
            let message = format!("The import from Monoptah failed and was undone: {error}");
            let _ = write_atomically(&target.join(ERROR_FILE), &message);
            Err(message)
        }
    }
}

const CRASH: &str = "simulated crash";

fn marker_text(journal: &Journal) -> String {
    let raw = &journal.raw;
    let mut text = serde_json::to_string_pretty(&json!({
        "imported_at_ms": now_ms(),
        "prepared_at_ms": raw["created_ms"],
        "zptah_version": raw["zptah_version"],
        "source": raw["source"],
        "source_database_bytes": raw["source_database_bytes"],
        "backup_dir": raw["backup_dir"],
        "summary": raw["summary"],
    }))
    .unwrap_or_default();
    text.push('\n');
    text
}
